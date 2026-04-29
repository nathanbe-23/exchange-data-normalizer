use exchange_data_normalizer::connectors::{binance, kraken};
use exchange_data_normalizer::publisher;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Tracing setup
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,exchange_data_normalizer=debug".into()),
        )
        .init();

    // Bounded channel: drop-oldest backpressure if publisher falls behind (currently newest, still TODO)
    let (tx, rx) = mpsc::channel(10_000);

    let publisher_task = tokio::spawn(publisher::run(rx));
    let binance_task = tokio::spawn(binance::run(tx.clone()));
    let kraken_task = tokio::spawn(kraken::run(tx.clone()));

    drop(tx); // important: drop the original sender so the channel can close cleanly
    let _ = tokio::try_join!(
        async { publisher_task.await? },
        async { binance_task.await? },
        async { kraken_task.await? },
    );

    Ok(())
}
