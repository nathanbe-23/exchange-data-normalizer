//! Kraken v2 WebSocket connector for trade data.
//!
//! ## Liveness detection
//!
//! Kraken v2 sends application-level `heartbeat` messages every ~1 second when
//! the connection is otherwise idle, in addition to WebSocket protocol-level
//! pings. We use a 15s liveness timeout: if no message of any kind arrives
//! within that window, the session is considered dead and the outer loop
//! reconnects.
//!
//! 15s = 15× the documented heartbeat interval, which gives generous headroom
//! for transient delays without false-positive reconnects.
//!
//! ## Known limitations
//!
//! Liveness only detects dead *connections*, not dead *subscriptions*.
//! Heartbeats keep arriving even if the subscription was silently dropped
//! server-side. Detecting subscription staleness would require a separate
//! timer that resets only on actual trade messages, with per-symbol
//! thresholds (BTC/USD trades constantly; thinner pairs do not).
//! See README "Roadmap".
//!

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::types::{Exchange, Side, Trade, now_millis};

static KRAKEN_MARKET_DATA_WS_URL: &str = "wss://ws.kraken.com/v2";
const KRAKEN_LIVENESS_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Deserialize)]
#[serde(tag = "channel")]
enum KrakenMessage {
    #[serde(rename = "trade")]
    Trade { data: Vec<KrakenTrade> },
    #[serde(rename = "heartbeat")]
    Heartbeat,
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct KrakenTrade {
    symbol: String,
    side: String, // buy or sell
    price: f64,
    qty: f64,
    timestamp: String, // ISO 8601
                       // ord_type: String, trade_id ignored for now (limit or market)
}

impl From<KrakenTrade> for crate::types::Trade {
    fn from(trade: KrakenTrade) -> Self {
        crate::types::Trade {
            exchange: Exchange::Kraken,
            symbol: trade.symbol,
            price: trade.price,
            quantity: trade.qty,
            side: if trade.side == "buy" {
                Side::Buy
            } else {
                Side::Sell
            },
            exchange_ts_ms: chrono::DateTime::parse_from_rfc3339(&trade.timestamp)
                .map(|dt| dt.timestamp_millis() as u64)
                .unwrap_or(0), // TODO: Surface error,
            recv_ts_ms: now_millis(),
        }
    }
}

pub async fn run(tx: mpsc::Sender<Trade>) -> anyhow::Result<()> {
    let mut backoff = backoff_initial();

    loop {
        match run_session(&tx).await {
            Ok(()) => {
                // Stream emded cleanly (rare) -> reset backoff and reconnect
                tracing::warn!("kraken session ended cleanly, reconnecting");
                backoff = backoff_initial();
            }
            Err(e) => {
                tracing::warn!(error= %e, backoff_ms = backoff.as_millis(), "kraken session failed");
                tokio::time::sleep(backoff).await;
                backoff = next_backoff(backoff);
            }
        }
    }
}

use rand::RngExt;
use std::time::Duration;

const INITIAL_BACKOFF_MS: u64 = 500;
const MAX_BACKOFF_MS: u64 = 30_000;

fn backoff_initial() -> Duration {
    Duration::from_millis(INITIAL_BACKOFF_MS)
}

fn next_backoff(current: Duration) -> Duration {
    let doubled = (current.as_millis() as u64)
        .saturating_mul(2)
        .min(MAX_BACKOFF_MS);
    let jitter = rand::rng().random_range(0..=doubled / 4);
    Duration::from_millis(doubled.saturating_add(jitter))
}

async fn run_session(tx: &mpsc::Sender<Trade>) -> anyhow::Result<()> {
    let (mut ws_stream, _) = connect_async(KRAKEN_MARKET_DATA_WS_URL).await?;
    let subscribe = serde_json::json!({
        "method": "subscribe",
        "params": {"channel": "trade", "symbol": ["BTC/USD"]}
    });
    ws_stream
        .send(Message::Text(subscribe.to_string().into()))
        .await?;

    let mut subscribed = false;
    // TODO: use last_hb_ts as liveness signal for reconnect
    let mut _last_hb_ts: u64 = 0;
    loop {
        tokio::select! {
            maybe_msg = ws_stream.next() => {
                match maybe_msg {
                    Some(Ok(Message::Text(text))) => handle_message(&text, tx, &mut subscribed, &mut _last_hb_ts).await?,
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(e.into()),
                    None => return Ok(()), // Stream ended, outer loop reconnects
                }

            }
            _ = sleep(KRAKEN_LIVENESS_TIMEOUT) => {
                anyhow::bail!("no messages from kraken in {:?}, treating as dead", KRAKEN_LIVENESS_TIMEOUT);
            }
        }
    }
}

async fn handle_message(
    text: &str,
    tx: &mpsc::Sender<Trade>,
    subscribed: &mut bool, // write on ack
    _last_hb_ts: &mut u64,
) -> anyhow::Result<()> {
    match parse_message(text)? {
        DispatchedMessage::SubscriptionAck { success, text } => {
            if success {
                tracing::info!("Kraken subscription confirmed");
                *subscribed = true;
            } else {
                anyhow::bail!("Kraken subscription failed: {}", text);
            }
        }
        DispatchedMessage::KrakenChannel(KrakenMessage::Trade { data }) => {
            dispatch_trades(data, tx, *subscribed).await;
        }
        DispatchedMessage::KrakenChannel(KrakenMessage::Heartbeat) => {
            *_last_hb_ts = now_millis();
            tracing::trace!("heartbeat");
        }
        DispatchedMessage::KrakenChannel(KrakenMessage::Other) => {}
        DispatchedMessage::Unknown(value) => {
            tracing::debug!(?value, "unknown kraken message");
        }
    }
    Ok(())
}

enum DispatchedMessage {
    KrakenChannel(KrakenMessage),
    SubscriptionAck { success: bool, text: String },
    Unknown(serde_json::Value),
}

fn parse_message(text: &str) -> anyhow::Result<DispatchedMessage> {
    let value: serde_json::Value = serde_json::from_str(text)?;

    // Subscription ACK - has no channel field
    if value.get("method").and_then(|v| v.as_str()) == Some("subscribe") {
        let success = value
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        return Ok(DispatchedMessage::SubscriptionAck {
            success,
            text: (text.to_string()), // for error logging
        });
    }

    // try typed enum

    match serde_json::from_value::<KrakenMessage>(value.clone()) {
        Ok(msg) => Ok(DispatchedMessage::KrakenChannel(msg)),
        Err(_) => Ok(DispatchedMessage::Unknown(value)),
    }
}

async fn dispatch_trades(
    trades: Vec<KrakenTrade>,
    tx: &mpsc::Sender<Trade>,
    subscribed: bool, // copy - only read
) {
    if !subscribed {
        tracing::warn!("received trade before subscription ack");
    }
    for kt in trades {
        let trade: Trade = kt.into();
        if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(trade) {
            // Publisher is behind. Drop newest (this trade) rather than block,
            // which backs up the into WS and cause exchange-side disconnect for slow
            // consumers
            // TODO: switch to drop oldest semantics for better freshness .
            tracing::warn!("publisher channel full, dropping trade");
        }
    }
}
