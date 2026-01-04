//! Crypto-specific types for 15-minute directional trading
//!
//! Types for exploiting latency between Binance prices and Polymarket orderbooks.

use crate::{MarketId, Side, TokenId};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Crypto asset type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CryptoAsset {
    BTC,
    ETH,
}

impl std::fmt::Display for CryptoAsset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CryptoAsset::BTC => write!(f, "BTC"),
            CryptoAsset::ETH => write!(f, "ETH"),
        }
    }
}

/// Direction of price movement
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Up,
    Down,
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Direction::Up => write!(f, "UP"),
            Direction::Down => write!(f, "DOWN"),
        }
    }
}

/// 15-minute crypto market on Polymarket
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CryptoMarket {
    /// Market identifier (slug)
    pub market_id: MarketId,
    /// Crypto asset being tracked
    pub asset: CryptoAsset,
    /// Token ID for "Up" outcome
    pub up_token_id: TokenId,
    /// Token ID for "Down" outcome
    pub down_token_id: TokenId,
    /// Strike price (reference price at market start)
    pub strike_price: Option<Decimal>,
    /// Market end time (resolution time)
    pub end_time: DateTime<Utc>,
    /// When this market was discovered
    pub discovered_at: DateTime<Utc>,
    /// Market question text
    pub question: String,
}

impl CryptoMarket {
    /// Get time remaining until resolution (seconds)
    pub fn time_to_resolution_secs(&self) -> i64 {
        (self.end_time - Utc::now()).num_seconds()
    }

    /// Check if market is expired
    pub fn is_expired(&self) -> bool {
        self.time_to_resolution_secs() <= 0
    }

    /// Check if market is a 15-minute window (within tolerance)
    pub fn is_15min_window(&self) -> bool {
        let secs = self.time_to_resolution_secs();
        secs > 0 && secs <= 20 * 60 // Up to 20 minutes
    }

    /// Check if safe to trade (enough time before resolution)
    pub fn is_safe_to_trade(&self, min_secs_before_resolution: i64) -> bool {
        self.time_to_resolution_secs() >= min_secs_before_resolution
    }

    /// Get the token ID for a direction
    pub fn token_for_direction(&self, direction: Direction) -> &TokenId {
        match direction {
            Direction::Up => &self.up_token_id,
            Direction::Down => &self.down_token_id,
        }
    }
}

/// Signal from latency detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencySignal {
    /// Unique signal ID
    pub signal_id: String,
    /// Crypto asset
    pub asset: CryptoAsset,
    /// Detected direction
    pub direction: Direction,
    /// Binance spot price when signal was generated
    pub binance_price: Decimal,
    /// Price change in basis points (Binance)
    pub price_change_bps: i32,
    /// Current Polymarket outcome price (ask for the direction)
    pub polymarket_price: Decimal,
    /// Expected fair price based on Binance movement
    pub expected_price: Decimal,
    /// Estimated edge in basis points
    pub edge_bps: u32,
    /// Confidence score (0.0 to 1.0)
    pub confidence: f64,
    /// When signal was detected (ns since epoch)
    pub detected_at_ns: u64,
    /// When Binance price was received (ns since epoch)
    pub binance_update_ns: u64,
    /// When Polymarket orderbook was last updated (ns since epoch)
    pub polymarket_update_ns: u64,
    /// Measured latency/delay (ms)
    pub latency_ms: u64,
}

impl LatencySignal {
    /// Generate a new signal ID
    pub fn new_id() -> String {
        Uuid::new_v4().to_string()
    }

    /// Check if signal is still fresh
    pub fn is_fresh(&self, max_age_ms: u64) -> bool {
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let age_ms = (now_ns - self.detected_at_ns) / 1_000_000;
        age_ms < max_age_ms
    }

    /// Get age in milliseconds
    pub fn age_ms(&self) -> u64 {
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        (now_ns - self.detected_at_ns) / 1_000_000
    }
}

/// Trade status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TradeStatus {
    /// Trade is pending execution
    Pending,
    /// Order submitted, awaiting fill
    Submitted,
    /// Order filled, position open
    Filled,
    /// Partially filled
    PartialFill,
    /// Position is being closed
    Exiting,
    /// Position closed
    Closed,
    /// Trade failed
    Failed,
    /// Trade was cancelled
    Cancelled,
}

/// Directional trade execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectionalTrade {
    /// Unique trade ID
    pub trade_id: String,
    /// Signal that triggered this trade
    pub signal_id: String,
    /// Market being traded
    pub market_id: MarketId,
    /// Asset type
    pub asset: CryptoAsset,
    /// Trade direction
    pub direction: Direction,
    /// Token ID being traded
    pub token_id: TokenId,
    /// Order side (always Buy for directional entry)
    pub side: Side,
    /// Entry price
    pub entry_price: Decimal,
    /// Position size in USD
    pub size_usd: Decimal,
    /// Number of shares
    pub shares: Decimal,
    /// Order ID from CLOB (if real trade)
    pub order_id: Option<String>,
    /// Trade status
    pub status: TradeStatus,
    /// Entry time
    pub entry_time: DateTime<Utc>,
    /// Exit price (if closed)
    pub exit_price: Option<Decimal>,
    /// Exit time (if closed)
    pub exit_time: Option<DateTime<Utc>>,
    /// Realized P&L (if closed)
    pub realized_pnl: Option<Decimal>,
    /// Expected edge at entry (bps)
    pub expected_edge_bps: u32,
    /// Whether this was a simulated (paper) trade
    pub is_paper: bool,
}

impl DirectionalTrade {
    /// Create a new trade ID
    pub fn new_id() -> String {
        Uuid::new_v4().to_string()
    }

    /// Calculate unrealized P&L given current price
    pub fn unrealized_pnl(&self, current_price: Decimal) -> Decimal {
        if self.status != TradeStatus::Filled {
            return Decimal::ZERO;
        }
        (current_price - self.entry_price) * self.shares
    }

    /// Calculate realized P&L (requires exit price)
    pub fn calculate_pnl(&self) -> Option<Decimal> {
        let exit = self.exit_price?;
        Some((exit - self.entry_price) * self.shares)
    }

    /// Check if trade is open
    pub fn is_open(&self) -> bool {
        matches!(self.status, TradeStatus::Filled | TradeStatus::PartialFill)
    }

    /// Get holding time in seconds
    pub fn holding_time_secs(&self) -> i64 {
        let end = self.exit_time.unwrap_or_else(Utc::now);
        (end - self.entry_time).num_seconds()
    }
}

/// Configuration for crypto latency trading
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CryptoLatencyConfig {
    /// Minimum price change in bps to trigger signal
    pub min_trigger_bps: i32,
    /// Minimum edge in bps to execute
    pub min_edge_bps: u32,
    /// Maximum Polymarket data age to consider (ms)
    pub max_polymarket_age_ms: u64,
    /// Minimum Polymarket data age to consider (ms) - needs some lag to exploit
    pub min_polymarket_age_ms: u64,
    /// Minimum confidence to execute
    pub min_confidence: f64,
    /// Cooldown between signals on same asset (ms)
    pub asset_cooldown_ms: u64,
    /// Maximum signals per minute
    pub max_signals_per_minute: u32,
    /// Stop trading N seconds before resolution
    pub stop_before_resolution_secs: u64,
    /// Position size per trade (USD)
    pub position_size_usd: Decimal,
    /// Take profit threshold (bps)
    pub take_profit_bps: u32,
    /// Stop loss threshold (bps)
    pub stop_loss_bps: u32,
    /// Maximum hold time (seconds)
    pub max_hold_secs: u64,
    /// Enable paper trading mode
    pub paper_mode: bool,
    /// Maximum daily trades
    pub max_daily_trades: u32,
    /// Maximum open positions
    pub max_positions: usize,
}

impl Default for CryptoLatencyConfig {
    fn default() -> Self {
        Self {
            min_trigger_bps: 15,        // 0.15% move triggers evaluation
            min_edge_bps: 10,           // 0.10% minimum edge
            max_polymarket_age_ms: 500, // Max 500ms stale
            min_polymarket_age_ms: 50,  // At least 50ms lag needed
            min_confidence: 0.6,
            asset_cooldown_ms: 5000, // 5 seconds between same-asset signals
            max_signals_per_minute: 30,
            stop_before_resolution_secs: 60, // Stop 1 min before resolution
            position_size_usd: Decimal::from(50),
            take_profit_bps: 15, // Exit at +0.15%
            stop_loss_bps: 20,   // Exit at -0.20%
            max_hold_secs: 180,  // 3 minutes max hold
            paper_mode: true,    // Start in paper mode
            max_daily_trades: 300,
            max_positions: 5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_crypto_market_time_check() {
        let market = CryptoMarket {
            market_id: MarketId::new("test"),
            asset: CryptoAsset::BTC,
            up_token_id: TokenId::new("up"),
            down_token_id: TokenId::new("down"),
            strike_price: Some(dec!(50000)),
            end_time: Utc::now() + chrono::Duration::minutes(10),
            discovered_at: Utc::now(),
            question: "Will BTC be up?".to_string(),
        };

        assert!(!market.is_expired());
        assert!(market.is_15min_window());
        assert!(market.is_safe_to_trade(60));
    }

    #[test]
    fn test_direction_token() {
        let market = CryptoMarket {
            market_id: MarketId::new("test"),
            asset: CryptoAsset::ETH,
            up_token_id: TokenId::new("up_token"),
            down_token_id: TokenId::new("down_token"),
            strike_price: None,
            end_time: Utc::now() + chrono::Duration::minutes(15),
            discovered_at: Utc::now(),
            question: "ETH price direction".to_string(),
        };

        assert_eq!(market.token_for_direction(Direction::Up).0, "up_token");
        assert_eq!(market.token_for_direction(Direction::Down).0, "down_token");
    }

    #[test]
    fn test_trade_pnl() {
        let mut trade = DirectionalTrade {
            trade_id: DirectionalTrade::new_id(),
            signal_id: "sig1".to_string(),
            market_id: MarketId::new("test"),
            asset: CryptoAsset::BTC,
            direction: Direction::Up,
            token_id: TokenId::new("up"),
            side: Side::Buy,
            entry_price: dec!(0.50),
            size_usd: dec!(50),
            shares: dec!(100),
            order_id: None,
            status: TradeStatus::Filled,
            entry_time: Utc::now(),
            exit_price: None,
            exit_time: None,
            realized_pnl: None,
            expected_edge_bps: 15,
            is_paper: true,
        };

        // Unrealized P&L when price goes up
        assert_eq!(trade.unrealized_pnl(dec!(0.52)), dec!(2)); // +$2

        // Set exit
        trade.exit_price = Some(dec!(0.52));
        trade.status = TradeStatus::Closed;
        assert_eq!(trade.calculate_pnl(), Some(dec!(2)));
    }
}
