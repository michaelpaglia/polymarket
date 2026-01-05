//! Core types for HFT trading
//!
//! Uses atomic operations and scaled integers for lock-free, precise price handling.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use uuid::Uuid;

/// Price scale factor: 1_000_000 = 1.0
/// This allows us to store prices as u64 for atomic operations
pub const PRICE_SCALE: u64 = 1_000_000;

/// Market condition identifier (Polymarket uses hex strings)
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct MarketId(pub String);

impl MarketId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for MarketId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Token identifier for YES or NO outcome
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct TokenId(pub String);

impl TokenId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for TokenId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Atomic price for lock-free updates
/// Stores price as scaled u64 (PRICE_SCALE = 1.0)
#[derive(Debug)]
pub struct AtomicPrice(AtomicU64);

impl AtomicPrice {
    /// Create from decimal price (0.0 to 1.0)
    pub fn new(price: Decimal) -> Self {
        let scaled = Self::decimal_to_scaled(price);
        Self(AtomicU64::new(scaled))
    }

    /// Create with zero value
    pub fn zero() -> Self {
        Self(AtomicU64::new(0))
    }

    /// Load current price as Decimal
    pub fn load(&self) -> Decimal {
        Self::scaled_to_decimal(self.0.load(Ordering::Acquire))
    }

    /// Store new price
    pub fn store(&self, price: Decimal) {
        let scaled = Self::decimal_to_scaled(price);
        self.0.store(scaled, Ordering::Release);
    }

    /// Load raw scaled value (for comparisons)
    pub fn load_raw(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    fn decimal_to_scaled(price: Decimal) -> u64 {
        let scaled = price * Decimal::from(PRICE_SCALE);
        // Truncate to integer - using floor to handle any fractional parts
        scaled.floor().to_string().parse::<u64>().unwrap_or(0)
    }

    fn scaled_to_decimal(scaled: u64) -> Decimal {
        Decimal::from(scaled) / Decimal::from(PRICE_SCALE)
    }
}

impl Default for AtomicPrice {
    fn default() -> Self {
        Self::zero()
    }
}

/// Price level in orderbook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: Decimal,
    pub size: Decimal,
}

impl PriceLevel {
    pub fn new(price: Decimal, size: Decimal) -> Self {
        Self { price, size }
    }
}

/// Order side
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Side {
    Buy,
    Sell,
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Side::Buy => write!(f, "BUY"),
            Side::Sell => write!(f, "SELL"),
        }
    }
}

/// Order type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    /// Good Till Cancelled (limit order)
    GTC,
    /// Fill or Kill (market order)
    FOK,
}

/// Trading state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TradingState {
    #[default]
    Stopped,
    Starting,
    Running,
    Paused,
    Stopping,
    Error,
}

/// Arbitrage opportunity detected
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArbitrageOpportunity {
    /// Unique opportunity ID
    pub opportunity_id: String,
    /// Market identifier
    pub market_id: MarketId,
    /// YES token ID
    pub yes_token_id: TokenId,
    /// NO token ID
    pub no_token_id: TokenId,
    /// Current YES ask price (price to buy YES)
    pub yes_price: Decimal,
    /// Current NO ask price (price to buy NO)
    pub no_price: Decimal,
    /// Spread = 1.0 - (yes_price + no_price)
    pub spread: Decimal,
    /// Profit in basis points
    pub profit_bps: u32,
    /// Maximum executable size in USD
    pub max_size_usd: Decimal,
    /// Detection timestamp (nanoseconds since epoch)
    pub detected_at_ns: u64,
    /// Confidence score (0.0 to 1.0)
    pub confidence: f64,
}

impl ArbitrageOpportunity {
    /// Create a new opportunity ID
    pub fn new_id() -> String {
        Uuid::new_v4().to_string()
    }

    /// Check if opportunity is still valid (not stale)
    pub fn is_valid(&self, max_age_ms: u64) -> bool {
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time before UNIX epoch")
            .as_nanos() as u64;
        let age_ms = (now_ns - self.detected_at_ns) / 1_000_000;
        age_ms < max_age_ms
    }
}

/// Arbitrage execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArbitrageExecution {
    pub execution_id: String,
    pub opportunity: ArbitrageOpportunity,
    pub yes_order_id: Option<String>,
    pub no_order_id: Option<String>,
    pub yes_filled: bool,
    pub no_filled: bool,
    pub yes_fill_price: Option<Decimal>,
    pub no_fill_price: Option<Decimal>,
    pub total_cost_usd: Decimal,
    pub expected_profit_usd: Decimal,
    pub execution_time_us: u64,
    pub executed_at: DateTime<Utc>,
    pub status: ExecutionStatus,
}

/// Execution status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    /// Both orders filled successfully
    Success,
    /// One order filled, other pending or failed (needs hedging)
    PartialFill,
    /// Both orders failed
    Failed,
    /// Orders submitted, awaiting confirmation
    Pending,
}

/// Market state snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub market_id: MarketId,
    pub yes_token_id: TokenId,
    pub no_token_id: TokenId,
    pub yes_best_bid: Decimal,
    pub yes_best_ask: Decimal,
    pub no_best_bid: Decimal,
    pub no_best_ask: Decimal,
    pub yes_midpoint: Decimal,
    pub no_midpoint: Decimal,
    pub price_sum: Decimal,
    pub timestamp: DateTime<Utc>,
}

impl MarketSnapshot {
    /// Check if arbitrage opportunity exists
    pub fn has_arbitrage(&self, min_spread_bps: u32) -> bool {
        let spread = Decimal::ONE - self.price_sum;
        let spread_bps = (spread * Decimal::from(10000))
            .to_string()
            .parse::<u32>()
            .unwrap_or(0);
        spread_bps >= min_spread_bps
    }
}

/// Configuration for the HFT engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HftConfig {
    /// Polymarket WebSocket URL
    pub ws_url: String,
    /// Polymarket REST API URL
    pub rest_url: String,
    /// Chain ID (137 for Polygon)
    pub chain_id: u64,
    /// Minimum spread in basis points to trade
    pub min_spread_bps: u32,
    /// Maximum price sum to consider (should be < 1.0)
    pub max_price_sum: Decimal,
    /// Minimum liquidity on each side (USD)
    pub min_liquidity_usd: Decimal,
    /// Maximum position size per trade (USD)
    pub max_position_size_usd: Decimal,
    /// Cooldown between trades on same market (milliseconds)
    pub cooldown_ms: u64,
    /// Maximum total exposure (USD)
    pub max_total_exposure_usd: Decimal,
    /// Maximum daily loss before circuit breaker (USD)
    pub max_daily_loss_usd: Decimal,
    /// Maximum number of simultaneous positions
    pub max_positions: usize,
}

impl Default for HftConfig {
    fn default() -> Self {
        Self {
            ws_url: "wss://ws-subscriptions-clob.polymarket.com/ws/market".to_string(),
            rest_url: "https://clob.polymarket.com".to_string(),
            chain_id: 137,
            min_spread_bps: 30,
            max_price_sum: Decimal::new(995, 3), // 0.995
            min_liquidity_usd: Decimal::from(100),
            max_position_size_usd: Decimal::from(500),
            cooldown_ms: 100,
            max_total_exposure_usd: Decimal::from(5000),
            max_daily_loss_usd: Decimal::from(500),
            max_positions: 10,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_atomic_price() {
        let price = AtomicPrice::new(dec!(0.55));
        assert_eq!(price.load(), dec!(0.55));

        price.store(dec!(0.67));
        assert_eq!(price.load(), dec!(0.67));
    }

    #[test]
    fn test_market_snapshot_arbitrage() {
        let snapshot = MarketSnapshot {
            market_id: MarketId::new("test"),
            yes_token_id: TokenId::new("yes"),
            no_token_id: TokenId::new("no"),
            yes_best_bid: dec!(0.44),
            yes_best_ask: dec!(0.45),
            no_best_bid: dec!(0.51),
            no_best_ask: dec!(0.52),
            yes_midpoint: dec!(0.445),
            no_midpoint: dec!(0.515),
            price_sum: dec!(0.97), // 0.45 + 0.52 = 0.97
            timestamp: Utc::now(),
        };

        // 3% spread = 300 bps
        assert!(snapshot.has_arbitrage(30)); // 0.3% threshold
        assert!(snapshot.has_arbitrage(100)); // 1% threshold
        assert!(snapshot.has_arbitrage(300)); // 3% threshold
        assert!(!snapshot.has_arbitrage(400)); // 4% threshold - not enough
    }
}
