use std::any;

use serde::Deserialize;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::{SinkExt, StreamExt};

use crate::types::{Trade, Exchange, Side, now_millis};

static KRAKEN_MARKET_DATA_WS_URL: &str = "wss://ws.kraken.com/v2";

#[derive(Debug, Deserialize)]
#[serde(tag="channel")]
enum KrakenMessage {
    #[serde(rename="trade")]
    Trade {
        #[serde(rename="type")]
        msg_type: String,  // "snapshot" or "update"
        data: Vec<KrakenTrade>,
    },
    #[serde(rename="heartbeat")]
    Heartbeat,
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct KrakenTrade {
    symbol: String,
    side: String,  // buy or sell
    price: f64,
    qty: f64,
    timestamp: String, // ISO 8601
    // ord_type: String, trade_id ignored for now (limit or market)
}

impl From<KrakenTrade> for crate::types::Trade {
    fn from(trade: KrakenTrade) -> Self {
        crate::types::Trade{
            exchange: Exchange::Kraken,
            symbol: trade.symbol,
            price: trade.price,
            quantity: trade.qty,
            side: if trade.side == "buy" {Side::Buy} else {Side::Sell},
            exchange_ts_ms: chrono::DateTime::parse_from_rfc3339(&trade.timestamp)
                .map(|dt| dt.timestamp_millis() as u64)
                .unwrap_or(0),  // TODO: Surface error,
            recv_ts_ms: now_millis(),
        }
    }
}

pub async fn test_loop_trades() -> anyhow::Result<()> {
    let (mut ws_stream, _) = connect_async(KRAKEN_MARKET_DATA_WS_URL).await?;
    let subscribe = serde_json::json!({
        "method": "subscribe",
        "params": {"channel": "trade", "symbol": ["BTC/USD"]}
    });
    ws_stream.send(Message::Text(subscribe.to_string().into())).await?;

    let mut subscribed = false;
    loop {
        match ws_stream.next().await {
            Some(Ok(Message::Text(text))) => {
                let value: serde_json::Value = serde_json::from_str(&text)?;

                if value.get("channel") == Some(&serde_json::json!("status")) {
                    tracing::info!(?value, "kraken status received");
                    continue;
                }
                if value.get("method") == Some(&serde_json::json!("subscribe"))
                    && value.get("success") == Some(&serde_json::json!(true))
                {
                    tracing::info!("kraken subscription confirmed");
                    subscribed = true;
                    continue;
                } else if value.get("method") == Some(&serde_json::json!("subscribe")) {
                        anyhow::bail!("Kraken subscription failed: {:?}", text);
                }
                
                if subscribed && value.get("channel") == Some(&serde_json::json!("trade")) {
                    tracing::info!("Got trade")
                }
                tracing::warn!(?value, "unexpected message");
            },
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(e.into()),
            None => anyhow::bail!("kraken stream closed before subscription confirmed"),
        }
    }
}
