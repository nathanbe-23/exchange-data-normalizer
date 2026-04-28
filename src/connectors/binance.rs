// use std::fmt::format;

use serde::{Deserialize};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite:: Message;
use futures_util::StreamExt;

static BINANCE_SPOT_WS_URL: &str = "wss://stream.binance.com:9443";

#[derive(Debug, Deserialize)]
struct Trade {
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

pub async fn test_loop_trades() -> anyhow::Result<()> {
    let url = format!("{}/ws/btcusdt@trade", BINANCE_SPOT_WS_URL);
    let (mut ws_stream, _) = connect_async(url).await?;
    println!("Websocket client connected");

    while let Some(msg) = ws_stream.next().await {
        match msg? {
            Message::Text(text) => {
                let new_trade: Trade = serde_json::from_slice(text.as_bytes())?;
                println!("{:?}", new_trade);
            }
            _ => {}
        }
    }
    Ok(())

}
