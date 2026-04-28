use exchange_data_normalizer::connectors::binance;
use exchange_data_normalizer::connectors::kraken;

#[tokio::main]
async fn main() -> anyhow::Result<()>{
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,exchange_data_normalizer=debug".into()),
        ).init();

    let (binance_result, kraken_result) = tokio::join!(
        binance::test_loop_trades(),
        kraken::test_loop_trades(),
    );
    binance_result?;
    kraken_result?;
    Ok(())
}
