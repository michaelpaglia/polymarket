//! Lock-free orderbook using copy-on-write pattern
//!
//! Uses `arc_swap` for atomic pointer swapping, enabling
//! lock-free reads and writes without blocking.
//!
//! Includes flow tracking for detecting order velocity and imbalance.

use crate::types::{AtomicPrice, MarketId, PriceLevel, TokenId};
use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Lock-free orderbook state
#[derive(Debug, Clone)]
pub struct OrderbookState {
    /// Bid levels sorted descending by price (best bid first)
    pub bids: Vec<PriceLevel>,
    /// Ask levels sorted ascending by price (best ask first)
    pub asks: Vec<PriceLevel>,
    /// Timestamp in nanoseconds
    pub timestamp_ns: u64,
}

impl OrderbookState {
    /// Create empty orderbook state
    pub fn empty() -> Self {
        Self {
            bids: Vec::new(),
            asks: Vec::new(),
            timestamp_ns: 0,
        }
    }
}

/// Price snapshot for flow tracking
#[derive(Debug, Clone)]
struct PriceSnapshot {
    bid: Decimal,
    ask: Decimal,
    timestamp_ns: u64,
}

/// Flow tracker for detecting order velocity and imbalance
/// Tracks price movements over a sliding window to detect momentum
pub struct FlowTracker {
    /// Recent price snapshots (newest at back)
    history: Mutex<VecDeque<PriceSnapshot>>,
    /// Maximum history size
    max_size: usize,
    /// Window duration in nanoseconds (default: 5 seconds)
    window_ns: u64,
}

impl FlowTracker {
    /// Create new flow tracker
    pub fn new() -> Self {
        Self {
            history: Mutex::new(VecDeque::with_capacity(100)),
            max_size: 100,
            window_ns: 5_000_000_000, // 5 seconds
        }
    }

    /// Record a price update
    pub fn record(&self, bid: Decimal, ask: Decimal, timestamp_ns: u64) {
        let mut history = self.history.lock();

        // Add new snapshot
        history.push_back(PriceSnapshot {
            bid,
            ask,
            timestamp_ns,
        });

        // Trim old entries beyond window
        let cutoff = timestamp_ns.saturating_sub(self.window_ns);
        while history.len() > 1 && history.front().map(|s| s.timestamp_ns < cutoff).unwrap_or(false) {
            history.pop_front();
        }

        // Also trim if too large
        while history.len() > self.max_size {
            history.pop_front();
        }
    }

    /// Calculate price velocity (bps per second)
    /// Positive = price moving up, Negative = price moving down
    pub fn velocity_bps_per_sec(&self) -> f64 {
        let history = self.history.lock();
        if history.len() < 2 {
            return 0.0;
        }

        let oldest = history.front().unwrap();
        let newest = history.back().unwrap();

        let time_delta_secs = (newest.timestamp_ns - oldest.timestamp_ns) as f64 / 1_000_000_000.0;
        if time_delta_secs < 0.1 {
            return 0.0; // Not enough time elapsed
        }

        // Use midpoints for velocity calculation
        let old_mid = (oldest.bid + oldest.ask) / dec!(2);
        let new_mid = (newest.bid + newest.ask) / dec!(2);

        if old_mid.is_zero() {
            return 0.0;
        }

        let price_change_bps = ((new_mid - old_mid) / old_mid * dec!(10000))
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0);

        price_change_bps / time_delta_secs
    }

    /// Calculate order flow imbalance (-1.0 to 1.0)
    /// Positive = more buying pressure, Negative = more selling pressure
    /// Based on which direction the midpoint is moving
    pub fn flow_imbalance(&self) -> f64 {
        let mut history = self.history.lock();
        if history.len() < 3 {
            return 0.0;
        }

        // Count up moves vs down moves
        let mut up_moves = 0;
        let mut down_moves = 0;

        for window in history.make_contiguous().windows(2) {
            let old_mid = (window[0].bid + window[0].ask) / dec!(2);
            let new_mid = (window[1].bid + window[1].ask) / dec!(2);

            if new_mid > old_mid {
                up_moves += 1;
            } else if new_mid < old_mid {
                down_moves += 1;
            }
        }

        let total = up_moves + down_moves;
        if total == 0 {
            return 0.0;
        }

        // Returns -1.0 (all down) to 1.0 (all up)
        (up_moves as f64 - down_moves as f64) / total as f64
    }

    /// Get price change over the window in bps
    pub fn price_change_bps(&self) -> i32 {
        let history = self.history.lock();
        if history.len() < 2 {
            return 0;
        }

        let oldest = history.front().unwrap();
        let newest = history.back().unwrap();

        let old_mid = (oldest.bid + oldest.ask) / dec!(2);
        let new_mid = (newest.bid + newest.ask) / dec!(2);

        if old_mid.is_zero() {
            return 0;
        }

        ((new_mid - old_mid) / old_mid * dec!(10000))
            .to_string()
            .parse::<i32>()
            .unwrap_or(0)
    }

    /// Check if there's significant momentum (velocity > threshold)
    pub fn has_momentum(&self, threshold_bps_per_sec: f64) -> bool {
        self.velocity_bps_per_sec().abs() > threshold_bps_per_sec
    }
}

impl Default for FlowTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl OrderbookState {

    /// Best bid price (highest buy price)
    pub fn best_bid(&self) -> Option<Decimal> {
        self.bids.first().map(|l| l.price)
    }

    /// Best ask price (lowest sell price)
    pub fn best_ask(&self) -> Option<Decimal> {
        self.asks.first().map(|l| l.price)
    }

    /// Calculate midpoint price
    pub fn midpoint(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some((bid + ask) / Decimal::TWO),
            _ => None,
        }
    }

    /// Calculate spread in basis points
    pub fn spread_bps(&self) -> Option<u32> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) if bid > Decimal::ZERO => {
                let spread = (ask - bid) / bid * Decimal::from(10000);
                spread.to_string().parse::<u32>().ok()
            }
            _ => None,
        }
    }

    /// Total bid depth in USD
    pub fn bid_depth(&self) -> Decimal {
        self.bids.iter().map(|l| l.price * l.size).sum()
    }

    /// Total ask depth in USD
    pub fn ask_depth(&self) -> Decimal {
        self.asks.iter().map(|l| l.price * l.size).sum()
    }
}

/// Lock-free orderbook for a single token
pub struct Orderbook {
    pub token_id: TokenId,
    state: ArcSwap<OrderbookState>,
    /// Cached best bid for fast access
    best_bid: AtomicPrice,
    /// Cached best ask for fast access
    best_ask: AtomicPrice,
    /// Last update timestamp (nanoseconds)
    last_update_ns: AtomicU64,
    /// Flow tracker for velocity/imbalance detection
    flow: FlowTracker,
}

impl Orderbook {
    /// Create new empty orderbook
    pub fn new(token_id: TokenId) -> Self {
        Self {
            token_id,
            state: ArcSwap::from_pointee(OrderbookState::empty()),
            best_bid: AtomicPrice::zero(),
            best_ask: AtomicPrice::zero(),
            last_update_ns: AtomicU64::new(0),
            flow: FlowTracker::new(),
        }
    }

    /// Update orderbook atomically (lock-free)
    pub fn update(&self, new_state: OrderbookState) {
        // Update cached prices
        let bid = new_state.best_bid().unwrap_or(Decimal::ZERO);
        let ask = new_state.best_ask().unwrap_or(Decimal::ZERO);

        if !bid.is_zero() {
            self.best_bid.store(bid);
        }
        if !ask.is_zero() {
            self.best_ask.store(ask);
        }

        // Record flow data for velocity/imbalance tracking
        if !bid.is_zero() || !ask.is_zero() {
            self.flow.record(bid, ask, new_state.timestamp_ns);
        }

        self.last_update_ns
            .store(new_state.timestamp_ns, Ordering::Release);

        // Atomically swap state
        self.state.store(Arc::new(new_state));
    }

    /// Get current state snapshot (lock-free read)
    pub fn snapshot(&self) -> Arc<OrderbookState> {
        self.state.load_full()
    }

    /// Fast best bid access (atomic read)
    pub fn best_bid(&self) -> Decimal {
        self.best_bid.load()
    }

    /// Fast best ask access (atomic read)
    pub fn best_ask(&self) -> Decimal {
        self.best_ask.load()
    }

    /// Get midpoint (atomic read)
    pub fn midpoint(&self) -> Decimal {
        let bid = self.best_bid.load();
        let ask = self.best_ask.load();
        (bid + ask) / Decimal::TWO
    }

    /// Check if orderbook has data
    pub fn is_ready(&self) -> bool {
        self.last_update_ns.load(Ordering::Acquire) > 0
    }

    /// Get last update timestamp
    pub fn last_update_ns(&self) -> u64 {
        self.last_update_ns.load(Ordering::Acquire)
    }

    /// Get age in milliseconds
    pub fn age_ms(&self) -> u64 {
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let last = self.last_update_ns.load(Ordering::Acquire);
        if last == 0 {
            u64::MAX
        } else {
            (now_ns - last) / 1_000_000
        }
    }

    /// Get price velocity (bps per second)
    /// Positive = price moving up, Negative = price moving down
    pub fn velocity_bps_per_sec(&self) -> f64 {
        self.flow.velocity_bps_per_sec()
    }

    /// Get flow imbalance (-1.0 to 1.0)
    /// Positive = buying pressure, Negative = selling pressure
    pub fn flow_imbalance(&self) -> f64 {
        self.flow.flow_imbalance()
    }

    /// Get price change over tracking window (bps)
    pub fn price_change_bps(&self) -> i32 {
        self.flow.price_change_bps()
    }

    /// Check if there's significant momentum
    pub fn has_momentum(&self, threshold_bps_per_sec: f64) -> bool {
        self.flow.has_momentum(threshold_bps_per_sec)
    }
}

/// Market orderbook containing both YES and NO sides
pub struct MarketOrderbook {
    pub market_id: MarketId,
    pub yes_token_id: TokenId,
    pub no_token_id: TokenId,
    pub yes_book: Orderbook,
    pub no_book: Orderbook,
}

impl MarketOrderbook {
    /// Create new market orderbook
    pub fn new(market_id: MarketId, yes_token_id: TokenId, no_token_id: TokenId) -> Self {
        let yes_book = Orderbook::new(yes_token_id.clone());
        let no_book = Orderbook::new(no_token_id.clone());
        Self {
            market_id,
            yes_token_id,
            no_token_id,
            yes_book,
            no_book,
        }
    }

    /// Get YES best ask (price to buy YES)
    pub fn yes_ask(&self) -> Decimal {
        self.yes_book.best_ask()
    }

    /// Get NO best ask (price to buy NO)
    pub fn no_ask(&self) -> Decimal {
        self.no_book.best_ask()
    }

    /// Get price sum (YES_ask + NO_ask)
    pub fn price_sum(&self) -> Decimal {
        self.yes_ask() + self.no_ask()
    }

    /// Check if arbitrage exists
    pub fn has_arbitrage(&self, min_spread_bps: u32) -> bool {
        let sum = self.price_sum();
        if sum >= Decimal::ONE {
            return false;
        }
        let spread = Decimal::ONE - sum;
        let spread_bps = (spread * Decimal::from(10000))
            .to_string()
            .parse::<u32>()
            .unwrap_or(0);
        spread_bps >= min_spread_bps
    }

    /// Check if both orderbooks are ready
    pub fn is_ready(&self) -> bool {
        self.yes_book.is_ready() && self.no_book.is_ready()
    }

    /// Get minimum age across both books
    pub fn min_age_ms(&self) -> u64 {
        self.yes_book.age_ms().min(self.no_book.age_ms())
    }

    /// Get snapshot for both sides
    pub fn snapshot(&self) -> MarketOrderbookSnapshot {
        MarketOrderbookSnapshot {
            market_id: self.market_id.clone(),
            yes_token_id: self.yes_token_id.clone(),
            no_token_id: self.no_token_id.clone(),
            yes_best_bid: self.yes_book.best_bid(),
            yes_best_ask: self.yes_book.best_ask(),
            no_best_bid: self.no_book.best_bid(),
            no_best_ask: self.no_book.best_ask(),
            price_sum: self.price_sum(),
            timestamp: Utc::now(),
        }
    }

    /// Get YES book flow imbalance (-1 to 1)
    /// Positive = buying pressure on YES, Negative = selling
    pub fn yes_flow_imbalance(&self) -> f64 {
        self.yes_book.flow_imbalance()
    }

    /// Get NO book flow imbalance (-1 to 1)
    /// Positive = buying pressure on NO, Negative = selling
    pub fn no_flow_imbalance(&self) -> f64 {
        self.no_book.flow_imbalance()
    }

    /// Get YES book velocity (bps/sec)
    pub fn yes_velocity(&self) -> f64 {
        self.yes_book.velocity_bps_per_sec()
    }

    /// Get NO book velocity (bps/sec)
    pub fn no_velocity(&self) -> f64 {
        self.no_book.velocity_bps_per_sec()
    }

    /// Detect if orderbook is tilting toward a direction
    /// Returns: (is_tilting, direction, strength)
    /// direction: true = YES favored, false = NO favored
    /// strength: 0.0 to 1.0 indicating confidence
    pub fn detect_tilt(&self) -> (bool, bool, f64) {
        let yes_imbalance = self.yes_flow_imbalance();
        let no_imbalance = self.no_flow_imbalance();

        // Compare flow imbalances
        // If YES has strong buying pressure (positive) OR NO has selling pressure (negative)
        // Then market is tilting toward YES
        let yes_signal = yes_imbalance - no_imbalance;

        let is_tilting = yes_signal.abs() > 0.3; // Threshold for "significant" tilt
        let yes_favored = yes_signal > 0.0;
        let strength = yes_signal.abs().min(1.0);

        (is_tilting, yes_favored, strength)
    }

    /// Get combined flow signal for trading
    /// Returns direction (Up=true, Down=false) and edge estimate in bps
    pub fn flow_signal(&self) -> Option<(bool, i32)> {
        let (is_tilting, yes_favored, strength) = self.detect_tilt();

        if !is_tilting || strength < 0.3 {
            return None;
        }

        // Estimate edge based on flow strength vs current price
        // If flow is strongly favoring YES but YES price is still high (0.5+)
        // that suggests edge exists
        let yes_ask = self.yes_ask();
        let no_ask = self.no_ask();

        let edge_bps = if yes_favored {
            // Flow favors YES - edge exists if YES ask is still relatively high
            // At 0.50, edge = 0; at 0.40, edge ~= strength * 100
            let price_discount = (dec!(0.50) - yes_ask) / dec!(0.50);
            let base_edge = (strength * 50.0) as i32; // Up to 50 bps from flow
            let price_edge = (price_discount * dec!(100))
                .to_string()
                .parse::<i32>()
                .unwrap_or(0);
            base_edge + price_edge
        } else {
            // Flow favors NO
            let price_discount = (dec!(0.50) - no_ask) / dec!(0.50);
            let base_edge = (strength * 50.0) as i32;
            let price_edge = (price_discount * dec!(100))
                .to_string()
                .parse::<i32>()
                .unwrap_or(0);
            base_edge + price_edge
        };

        // Only signal if edge is positive
        if edge_bps > 0 {
            Some((yes_favored, edge_bps))
        } else {
            None
        }
    }
}

/// Snapshot of market orderbook state
#[derive(Debug, Clone)]
pub struct MarketOrderbookSnapshot {
    pub market_id: MarketId,
    pub yes_token_id: TokenId,
    pub no_token_id: TokenId,
    pub yes_best_bid: Decimal,
    pub yes_best_ask: Decimal,
    pub no_best_bid: Decimal,
    pub no_best_ask: Decimal,
    pub price_sum: Decimal,
    pub timestamp: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_orderbook_update() {
        let book = Orderbook::new(TokenId::new("test"));

        let state = OrderbookState {
            bids: vec![
                PriceLevel::new(dec!(0.50), dec!(100)),
                PriceLevel::new(dec!(0.49), dec!(200)),
            ],
            asks: vec![
                PriceLevel::new(dec!(0.52), dec!(150)),
                PriceLevel::new(dec!(0.53), dec!(250)),
            ],
            timestamp_ns: 1234567890,
        };

        book.update(state);

        assert_eq!(book.best_bid(), dec!(0.50));
        assert_eq!(book.best_ask(), dec!(0.52));
        assert!(book.is_ready());
    }

    #[test]
    fn test_market_orderbook_arbitrage() {
        let market = MarketOrderbook::new(
            MarketId::new("test"),
            TokenId::new("yes"),
            TokenId::new("no"),
        );

        // Set YES ask = 0.45
        market.yes_book.update(OrderbookState {
            bids: vec![PriceLevel::new(dec!(0.44), dec!(100))],
            asks: vec![PriceLevel::new(dec!(0.45), dec!(100))],
            timestamp_ns: 1,
        });

        // Set NO ask = 0.52
        market.no_book.update(OrderbookState {
            bids: vec![PriceLevel::new(dec!(0.51), dec!(100))],
            asks: vec![PriceLevel::new(dec!(0.52), dec!(100))],
            timestamp_ns: 1,
        });

        // Sum = 0.97, spread = 3%
        assert_eq!(market.price_sum(), dec!(0.97));
        assert!(market.has_arbitrage(30)); // 0.3% threshold - should pass
        assert!(market.has_arbitrage(300)); // 3% threshold - should pass
        assert!(!market.has_arbitrage(400)); // 4% threshold - should fail
    }
}
