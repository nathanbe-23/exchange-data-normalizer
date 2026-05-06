use std::time::Duration;

use exchange_data_normalizer::connectors::binance;
use exchange_data_normalizer::types::Trade;

use futures_util::SinkExt;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::{accept_async, tungstenite};

#[tokio::test]
async fn binance_connector_streams_canonical_trades() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let test_url = format!("ws://{}", addr);

    // Server task: accept connection and send messages
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        let trade_str = r#"{"s": "BTCUSDT", "p": "85000.1", "q": "0.001", "T": 1, "m": false}"#;
        ws.send(tungstenite::Message::Text(trade_str.into()))
            .await
            .unwrap();
    });

    // Link connector to server, read message and parse trade
    let (tx, mut rx) = mpsc::channel::<Trade>(100);

    // run connector and receiver concurrently
    tokio::select! {

        // receiver returns first when trade arrives
        Some(trade) = rx.recv() => {
            tracing::info!("received trade from server");
            assert_eq!(trade.symbol,"BTC/USDT");
            assert_eq!(trade.price, 85000.1);
        }

        // connector runs indefinitely until first trades arrives and receiver returns
        _ = binance::run(tx, test_url.as_str()) => {
            panic!("connector returned before trade");
        }

        // Neither happened in 5s -> timeout and test failed
        _ = tokio::time::sleep(Duration::from_secs(5)) => {
            panic!("timed out waiting for trade")
        }

    }
}
