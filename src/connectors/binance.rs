use serde::Deserialize;

use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite:: Message;
use futures_util::StreamExt;

use crate::types::{Trade, Exchange, Side, now_millis};

static BINANCE_SPOT_WS_URL: &str = "wss://stream.binance.com:9443";

#[derive(Debug, Deserialize)]
struct BinanceTrade {
    # [serde(rename = "s")]
    symbol:String,
    # [serde(rename = "p")]
    price: String,
    # [serde(rename = "q")]
    quantity: String,
    # [serde(rename = "T")]
    timestamp: u64,
    # [serde(rename = "m")]
    maker: bool,
}

impl From<BinanceTrade> for crate::types::Trade {
    fn from(b: BinanceTrade) -> Self {
        crate::types::Trade {
            exchange: Exchange::Binance,
            symbol: normalize_symbol(&b.symbol),
            price: b.price.parse().unwrap_or(0.0),  // TODO: surface parse errors via Result<Trade, ConvertError>
            quantity: b.quantity.parse().unwrap_or(0.0),  // TODO: surface parse errors via Result<Trade, ConvertError>
            // Binance's `m` = "buyer is maker". We normalize to taker side:
            // m=true  -> seller was the taker -> Sell
            // m=false -> buyer was the taker  -> Buy
            side: if b.maker {Side::Sell} else {Side::Buy},
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

pub async fn test_loop_trades() -> anyhow::Result<()> {
    let url = format!("{}/ws/btcusdt@trade", BINANCE_SPOT_WS_URL);
    let (mut ws_stream, _) = connect_async(url).await?;
    tracing::info!("websocket connected");

    while let Some(msg) = ws_stream.next().await {
        match msg? {
            Message::Text(text) => {
                let deser_trade: BinanceTrade = serde_json::from_slice(text.as_bytes())?;
                let trade: Trade = deser_trade.into();
                tracing::debug!(?trade, "received trade");
            }
            _ => {}
        }
    }
    Ok(())

}
