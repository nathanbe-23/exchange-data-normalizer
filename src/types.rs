use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub exchange: Exchange,
    pub symbol: String,
    pub price: f64,
    pub quantity: f64,
    pub side: Side,
    pub exchange_ts_ms: u64,
    pub recv_ts_ms: u64,
}


#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Exchange {Binance, Coinbase, Kraken}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Side {Buy, Sell}

pub fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System clock before unix epoch")
        .as_millis() as u64
}