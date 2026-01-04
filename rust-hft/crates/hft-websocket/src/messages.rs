//! WebSocket message types for Polymarket

use hft_core::{PriceLevel, Side};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Subscription request
#[derive(Debug, Clone, Serialize)]
pub struct SubscribeRequest {
    pub assets_ids: Vec<String>,
    #[serde(rename = "type")]
    pub channel_type: String,
}

impl SubscribeRequest {
    pub fn market(asset_ids: Vec<String>) -> Self {
        Self {
            assets_ids: asset_ids,
            channel_type: "market".to_string(),
        }
    }
}

/// Incoming WebSocket message
#[derive(Debug, Clone)]
pub enum WsMessage {
    /// Full orderbook snapshot
    Book(BookMessage),
    /// Price change update
    PriceChange(PriceChangeMessage),
    /// Last trade price
    LastTrade(LastTradeMessage),
    /// Tick size update
    TickSize(TickSizeMessage),
    /// Unknown message type
    Unknown(String),
}

/// Full orderbook message
#[derive(Debug, Clone, Deserialize)]
pub struct BookMessage {
    pub asset_id: String,
    pub market: String,
    pub bids: Vec<BookLevel>,
    pub asks: Vec<BookLevel>,
    pub timestamp: String,
    pub hash: Option<String>,
}

/// Order book level from WS
#[derive(Debug, Clone, Deserialize)]
pub struct BookLevel {
    pub price: String,
    pub size: String,
}

impl BookLevel {
    pub fn to_price_level(&self) -> Option<PriceLevel> {
        let price = self.price.parse::<Decimal>().ok()?;
        let size = self.size.parse::<Decimal>().ok()?;
        Some(PriceLevel::new(price, size))
    }
}

/// Price change message
#[derive(Debug, Clone, Deserialize)]
pub struct PriceChangeMessage {
    pub asset_id: String,
    #[serde(default)]
    pub market: String,
    pub price: String,
    pub size: String,
    #[serde(default)]
    pub side: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub fee: Option<String>,
}

impl PriceChangeMessage {
    pub fn side(&self) -> Side {
        match self.side.as_deref() {
            Some(s) if s.to_uppercase() == "BUY" => Side::Buy,
            _ => Side::Sell,
        }
    }

    pub fn price(&self) -> Option<Decimal> {
        self.price.parse().ok()
    }

    pub fn size(&self) -> Option<Decimal> {
        self.size.parse().ok()
    }
}

/// Last trade message
#[derive(Debug, Clone, Deserialize)]
pub struct LastTradeMessage {
    pub asset_id: String,
    pub market: String,
    pub price: String,
    pub size: String,
    pub side: String,
    pub timestamp: String,
}

/// Tick size message
#[derive(Debug, Clone, Deserialize)]
pub struct TickSizeMessage {
    pub asset_id: String,
    pub minimum_tick_size: String,
}

/// Price update for callbacks
#[derive(Debug, Clone)]
pub struct PriceUpdate {
    pub token_id: String,
    pub best_bid: Option<Decimal>,
    pub best_ask: Option<Decimal>,
    pub midpoint: Option<Decimal>,
    pub timestamp_ns: u64,
}

impl PriceUpdate {
    pub fn from_book(msg: &BookMessage) -> Self {
        let best_bid = msg.bids.first().and_then(|l| l.price.parse().ok());
        let best_ask = msg.asks.first().and_then(|l| l.price.parse().ok());
        let midpoint = match (best_bid, best_ask) {
            (Some(b), Some(a)) => Some((b + a) / Decimal::TWO),
            _ => None,
        };

        Self {
            token_id: msg.asset_id.clone(),
            best_bid,
            best_ask,
            midpoint,
            timestamp_ns: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64,
        }
    }
}
