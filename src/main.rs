use exchange_data_normalizer::connectors::binance;

#[tokio::main]
async fn main() -> anyhow::Result<()>{
    let _ = binance::test_loop_trades().await?;
    Ok(())
    
}
