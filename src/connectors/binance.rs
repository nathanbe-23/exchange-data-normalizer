//! Binance public spot WebSocket connector for trade data.
//!
//! ## Liveness detection
//!
//! Binance's WebSocket server sends a ping frame every 20 seconds (per
//! Binance docs). The ping payload is the server's wall-clock time in
//! milliseconds, which could be used to measure exchange↔client clock skew
//! (not yet exposed as a metric — see roadmap).
//!
//! `tokio-tungstenite` auto-responds to pings with pongs; we never see the
//! pong on our side. We use a 240s liveness timeout (12× the documented
//! ping interval) — conservative but robust to ping delays or
//! infrastructure changes upstream.
//!
//! Unlike Kraken, Binance does not send application-level heartbeats on
//! the public trade stream. Liveness on a quiet market relies on protocol
//! pings.
//!
//! ## Known limitations
//!
//! Same as Kraken: this detects dead connections, not dead subscriptions.
//! On highly liquid pairs (BTC/USDT), trade frequency itself is a strong
//! liveness signal — but on quieter symbols, only pings keep the timer fed.

use serde::Deserialize;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::types::{Exchange, Side, Trade, now_millis};

static BINANCE_SPOT_WS_URL: &str = "wss://stream.binance.com:9443";
const BINANCE_LIVENESS_TIMEOUT: Duration = Duration::from_secs(240);

#[derive(Debug, Deserialize)]
struct BinanceTrade {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "p")]
    price: String,
    #[serde(rename = "q")]
    quantity: String,
    #[serde(rename = "T")]
    timestamp: u64,
    #[serde(rename = "m")]
    maker: bool,
}

impl From<BinanceTrade> for crate::types::Trade {
    fn from(b: BinanceTrade) -> Self {
        crate::types::Trade {
            exchange: Exchange::Binance,
            symbol: normalize_symbol(&b.symbol),
            price: b.price.parse().unwrap_or(0.0), // TODO: surface parse errors via Result<Trade, ConvertError>
            quantity: b.quantity.parse().unwrap_or(0.0), // TODO: surface parse errors via Result<Trade, ConvertError>
            // Binance's `m` = "buyer is maker". We normalize to taker side:
            // m=true  -> seller was the taker -> Sell
            // m=false -> buyer was the taker  -> Buy
            side: if b.maker { Side::Sell } else { Side::Buy },
            exchange_ts_ms: b.timestamp,
            recv_ts_ms: now_millis(),
        }
    }
}

fn normalize_symbol(symbol: &str) -> String {
    match symbol {
        "BTCUSDT" => "BTC/USDT".to_string(),
        _other => "".to_string(),
    }
}

pub async fn run(tx: mpsc::Sender<Trade>) -> anyhow::Result<()> {
    let mut backoff = backoff_initial();

    loop {
        match run_session(&tx).await {
            Ok(()) => {
                // Stream emded cleanly (rare) -> reset backoff and reconnect
                tracing::warn!("binance session ended cleanly, reconnecting");
                backoff = backoff_initial();
            }
            Err(e) => {
                tracing::warn!(error= %e, backoff_ms = backoff.as_millis(), "binance session failed");
                tokio::time::sleep(backoff).await;
                backoff = next_backoff(backoff);
            }
        }
    }
}

use rand::RngExt;

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

pub async fn run_session(tx: &mpsc::Sender<Trade>) -> anyhow::Result<()> {
    let url = format!("{}/ws/btcusdt@trade", BINANCE_SPOT_WS_URL);
    let (mut ws_stream, _) = connect_async(url).await?;
    tracing::info!("websocket connected");

    loop {
        tokio::select! {
            maybe_msg = ws_stream.next() => {
                match maybe_msg {
                    Some(Ok(Message::Text(text))) => {
                        let deser_trade: BinanceTrade = serde_json::from_slice(text.as_bytes())?;
                        let trade: Trade = deser_trade.into();

                        if let Err(tokio::sync::mpsc::error::TrySendError::Full(_))= tx.try_send(trade) {
                            // Publisher is behind. Drop newest (this trade) rather than block,
                            // which backs up the into WS and cause exchange-side disconnect for slow
                            // consumers
                            // TODO: switch to drop oldest semantics for better freshness .
                            tracing::warn!("publisher channel full, dropping trade");
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        tracing::debug!(payload = ?payload, "binance ping");
                    },
                    Some(Ok(Message::Pong(payload))) => {
                        tracing::trace!(payload_len = payload.len(), "binance pong");
                    },
                    Some(Ok(Message::Close(frame))) => {
                        tracing::warn!(?frame, "binance sent close frame");
                        return Ok(());  // graceful disconnect, outer loop reconnects
                    },
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(e.into()),
                    None => return Ok(()),
                }
            }
            _ = sleep(BINANCE_LIVENESS_TIMEOUT) => {
                anyhow::bail!("no messages from binance in {:?}, treating as dead", BINANCE_LIVENESS_TIMEOUT);
            }


        }
    }
}

mod tests {
    use super::*;

    #[test]
    fn parse_message_deserialization() {
        let text = r#"{
            "e": "trade",           
            "E": 1672515782136,     
            "s": "BTCUSDT",        
            "t": 12345,            
            "p": "0.001",          
            "q": "100",            
            "T": 1672515782136,     
            "m": true,              
            "M": true               
        }"#;

        let deserialized_t: BinanceTrade = serde_json::from_slice(text.as_bytes()).unwrap();
        assert_eq!(deserialized_t.symbol, "BTCUSDT");
        assert_eq!(deserialized_t.price, "0.001");
        assert_eq!(deserialized_t.quantity, "100");
        assert!(deserialized_t.maker);
    }

    #[test]
    fn parse_message_trade_conversion() {
        let d_trade = BinanceTrade {
            symbol: "BTCUSDT".to_string(),
            price: "0.001".to_string(),
            quantity: "100".to_string(),
            timestamp: 1672515782136,
            maker: true,
        };
        let trade: Trade = d_trade.into();
        assert_eq!(trade.symbol, "BTC/USDT");
        assert_eq!(trade.price, 0.001);
        assert_eq!(trade.quantity, 100_f64);
        assert_eq!(trade.side, Side::Sell);
    }

    // TODO: additional Binance parse tests:
    // - maker = false maps to Side::Buy (companion to existing maker=true → Sell test)
    // - malformed JSON returns Err
    // - missing required field (e.g., no "p") returns Err
}
