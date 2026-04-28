use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::types::{Exchange, Side, Trade, now_millis};

static KRAKEN_MARKET_DATA_WS_URL: &str = "wss://ws.kraken.com/v2";

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

pub async fn run() -> anyhow::Result<()> {
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
        match ws_stream.next().await {
            Some(Ok(Message::Text(text))) => {
                let value: serde_json::Value = serde_json::from_str(&text)?;

                // Subscription ACK - has no channel field
                if value.get("method").and_then(|v| v.as_str()) == Some("subscribe") {
                    let success = value
                        .get("success")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if success {
                        tracing::info!("kraken subscription confirmed");
                        subscribed = true;
                    } else {
                        anyhow::bail!("Kraken subscription failed: {:?}", text);
                    }
                    continue;
                }

                // Otherwise process as typed enum KrakenMessage
                let msg: KrakenMessage = serde_json::from_value(value.clone())?;

                match msg {
                    KrakenMessage::Trade { data } => {
                        if !subscribed {
                            tracing::warn!("received trade before subscription ack");
                        }
                        for raw_trade in data {
                            let trade: Trade = raw_trade.into();
                            tracing::debug!(?trade, "received trade");
                        }
                    }
                    KrakenMessage::Heartbeat => {
                        _last_hb_ts = now_millis();
                        tracing::trace!("heartbeat");
                        continue;
                    }
                    KrakenMessage::Other => {
                        tracing::debug!(?value, "unhandled message");
                    }
                }
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(e.into()),
            None => anyhow::bail!("kraken stream closed before subscription confirmed"),
        }
    }
}
