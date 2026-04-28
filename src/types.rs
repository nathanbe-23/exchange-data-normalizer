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

