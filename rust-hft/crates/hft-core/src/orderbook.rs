//! Lock-free orderbook using copy-on-write pattern
//!
//! Uses `arc_swap` for atomic pointer swapping, enabling
//! lock-free reads and writes without blocking.

use crate::types::{AtomicPrice, MarketId, PriceLevel, TokenId};
use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
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
        }
    }

    /// Update orderbook atomically (lock-free)
    pub fn update(&self, new_state: OrderbookState) {
        // Update cached prices
        if let Some(bid) = new_state.best_bid() {
            self.best_bid.store(bid);
        }
        if let Some(ask) = new_state.best_ask() {
            self.best_ask.store(ask);
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
