//! Arbitrage detection algorithm for YES/NO mispricing
//!
//! Detects opportunities when YES_ask + NO_ask < 1.0

use crate::error::{HftError, HftResult};
use crate::orderbook::MarketOrderbook;
use crate::types::{ArbitrageOpportunity, HftConfig, MarketId, TokenId};
use dashmap::DashMap;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Arbitrage detector configuration
#[derive(Debug, Clone)]
pub struct ArbitrageConfig {
    /// Minimum spread in basis points to trade
    pub min_spread_bps: u32,
    /// Maximum price sum (must be < 1.0 for arbitrage)
    pub max_price_sum: Decimal,
    /// Minimum liquidity on each side (USD)
    pub min_liquidity_usd: Decimal,
    /// Maximum position size per trade (USD)
    pub max_position_size_usd: Decimal,
    /// Cooldown between trades on same market (microseconds)
    pub cooldown_us: u64,
    /// Maximum age of price data to consider (milliseconds)
    pub max_data_age_ms: u64,
}

impl Default for ArbitrageConfig {
    fn default() -> Self {
        Self {
            min_spread_bps: 30,                  // 0.30% minimum edge
            max_price_sum: dec!(0.995),          // YES + NO must be < 0.995
            min_liquidity_usd: dec!(100),        // $100 min on each side
            max_position_size_usd: dec!(500),    // $500 max per trade
            cooldown_us: 100_000,                // 100ms between same-market trades
            max_data_age_ms: 1000,               // 1 second max data age
        }
    }
}

impl From<&HftConfig> for ArbitrageConfig {
    fn from(config: &HftConfig) -> Self {
        Self {
            min_spread_bps: config.min_spread_bps,
            max_price_sum: config.max_price_sum,
            min_liquidity_usd: config.min_liquidity_usd,
            max_position_size_usd: config.max_position_size_usd,
            cooldown_us: config.cooldown_ms * 1000,
            max_data_age_ms: 1000,
        }
    }
}

/// Arbitrage detector
pub struct ArbitrageDetector {
    config: ArbitrageConfig,
    /// Market orderbooks
    markets: DashMap<MarketId, Arc<MarketOrderbook>>,
    /// Last trade timestamp per market (nanoseconds)
    last_trade: DashMap<MarketId, u64>,
    /// Opportunities detected count
    opportunities_count: std::sync::atomic::AtomicU64,
}

impl ArbitrageDetector {
    /// Create new arbitrage detector
    pub fn new(config: ArbitrageConfig) -> Self {
        Self {
            config,
            markets: DashMap::new(),
            last_trade: DashMap::new(),
            opportunities_count: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Register a market for monitoring
    pub fn register_market(
        &self,
        market_id: MarketId,
        yes_token_id: TokenId,
        no_token_id: TokenId,
    ) -> Arc<MarketOrderbook> {
        let orderbook = Arc::new(MarketOrderbook::new(
            market_id.clone(),
            yes_token_id,
            no_token_id,
        ));
        self.markets.insert(market_id, orderbook.clone());
        orderbook
    }

    /// Get market orderbook
    pub fn get_market(&self, market_id: &MarketId) -> Option<Arc<MarketOrderbook>> {
        self.markets.get(market_id).map(|r| r.clone())
    }

    /// Remove market from monitoring
    pub fn remove_market(&self, market_id: &MarketId) {
        self.markets.remove(market_id);
        self.last_trade.remove(market_id);
    }

    /// Check for arbitrage opportunity on a specific market
    pub fn check_opportunity(&self, market_id: &MarketId) -> HftResult<Option<ArbitrageOpportunity>> {
        let market = self
            .markets
            .get(market_id)
            .ok_or_else(|| HftError::MarketNotFound(market_id.to_string()))?;

        // Check if market data is ready
        if !market.is_ready() {
            debug!(market_id = %market_id, "Market data not ready");
            return Ok(None);
        }

        // Check data age
        let age_ms = market.min_age_ms();
        if age_ms > self.config.max_data_age_ms {
            debug!(
                market_id = %market_id,
                age_ms = age_ms,
                max_age_ms = self.config.max_data_age_ms,
                "Market data too stale"
            );
            return Ok(None);
        }

        // Get current prices
        let yes_ask = market.yes_ask();
        let no_ask = market.no_ask();
        let price_sum = yes_ask + no_ask;

        // Check if arbitrage exists
        if price_sum >= Decimal::ONE {
            return Ok(None);
        }

        // Calculate spread
        let spread = Decimal::ONE - price_sum;
        let spread_bps = (spread * dec!(10000))
            .to_string()
            .parse::<u32>()
            .unwrap_or(0);

        // Check minimum spread threshold
        if spread_bps < self.config.min_spread_bps {
            return Ok(None);
        }

        // Check price sum threshold
        if price_sum > self.config.max_price_sum {
            return Ok(None);
        }

        // Check cooldown
        let now_ns = Self::now_ns();
        if let Some(last) = self.last_trade.get(market_id) {
            let elapsed_us = (now_ns - *last) / 1000;
            if elapsed_us < self.config.cooldown_us {
                let remaining_ms = (self.config.cooldown_us - elapsed_us) / 1000;
                debug!(
                    market_id = %market_id,
                    remaining_ms = remaining_ms,
                    "Cooldown active"
                );
                return Ok(None);
            }
        }

        // Calculate maximum executable size
        let max_size = self.calculate_max_size(&market, yes_ask, no_ask);
        if max_size < dec!(10) {
            debug!(
                market_id = %market_id,
                max_size = %max_size,
                "Insufficient size for trade"
            );
            return Ok(None);
        }

        // Create opportunity
        let opportunity = ArbitrageOpportunity {
            opportunity_id: ArbitrageOpportunity::new_id(),
            market_id: market_id.clone(),
            yes_token_id: market.yes_token_id.clone(),
            no_token_id: market.no_token_id.clone(),
            yes_price: yes_ask,
            no_price: no_ask,
            spread,
            profit_bps: spread_bps,
            max_size_usd: max_size,
            detected_at_ns: now_ns,
            confidence: self.calculate_confidence(spread_bps, max_size),
        };

        self.opportunities_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        info!(
            market_id = %market_id,
            yes_price = %yes_ask,
            no_price = %no_ask,
            spread_bps = spread_bps,
            max_size = %max_size,
            "Arbitrage opportunity detected"
        );

        Ok(Some(opportunity))
    }

    /// Scan all markets for opportunities
    pub fn scan_all(&self) -> Vec<ArbitrageOpportunity> {
        let mut opportunities = Vec::new();

        for entry in self.markets.iter() {
            match self.check_opportunity(entry.key()) {
                Ok(Some(opp)) => opportunities.push(opp),
                Ok(None) => {}
                Err(e) => {
                    warn!(
                        market_id = %entry.key(),
                        error = %e,
                        "Error checking opportunity"
                    );
                }
            }
        }

        // Sort by profit (highest first)
        opportunities.sort_by(|a, b| b.profit_bps.cmp(&a.profit_bps));

        opportunities
    }

    /// Record a trade (updates cooldown)
    pub fn record_trade(&self, market_id: &MarketId) {
        self.last_trade.insert(market_id.clone(), Self::now_ns());
    }

    /// Get number of monitored markets
    pub fn market_count(&self) -> usize {
        self.markets.len()
    }

    /// Get total opportunities detected
    pub fn opportunities_detected(&self) -> u64 {
        self.opportunities_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Calculate maximum executable size
    fn calculate_max_size(
        &self,
        market: &MarketOrderbook,
        yes_price: Decimal,
        no_price: Decimal,
    ) -> Decimal {
        // Get orderbook depths
        let yes_snap = market.yes_book.snapshot();
        let no_snap = market.no_book.snapshot();

        // Calculate available liquidity at best ask
        let yes_liquidity = yes_snap
            .asks
            .first()
            .map(|l| l.price * l.size)
            .unwrap_or(Decimal::ZERO);
        let no_liquidity = no_snap
            .asks
            .first()
            .map(|l| l.price * l.size)
            .unwrap_or(Decimal::ZERO);

        // Size is limited by smaller side
        let available = yes_liquidity.min(no_liquidity);

        // Check minimum liquidity
        if available < self.config.min_liquidity_usd {
            return Decimal::ZERO;
        }

        // Cap at maximum position size
        available.min(self.config.max_position_size_usd)
    }

    /// Calculate confidence score
    fn calculate_confidence(&self, spread_bps: u32, size: Decimal) -> f64 {
        // Spread score: 1% spread = 1.0 confidence
        let spread_score = (spread_bps as f64 / 100.0).min(1.0);

        // Size score: $1000 = 1.0 confidence
        let size_f64 = size.to_string().parse::<f64>().unwrap_or(0.0);
        let size_score = (size_f64 / 1000.0).min(1.0);

        // Weighted average (spread matters more)
        (spread_score * 0.7 + size_score * 0.3).min(1.0)
    }

    /// Get current time in nanoseconds
    fn now_ns() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orderbook::{OrderbookState, PriceLevel};

    #[test]
    fn test_arbitrage_detection() {
        let config = ArbitrageConfig {
            min_spread_bps: 30,
            cooldown_us: 0, // No cooldown for testing
            ..Default::default()
        };

        let detector = ArbitrageDetector::new(config);

        let market_id = MarketId::new("test-market");
        let market = detector.register_market(
            market_id.clone(),
            TokenId::new("yes-token"),
            TokenId::new("no-token"),
        );

        // Set prices: YES = 0.45, NO = 0.52, sum = 0.97 (3% spread)
        market.yes_book.update(OrderbookState {
            bids: vec![PriceLevel::new(dec!(0.44), dec!(1000))],
            asks: vec![PriceLevel::new(dec!(0.45), dec!(1000))],
            timestamp_ns: Self::now_ns(),
        });

        market.no_book.update(OrderbookState {
            bids: vec![PriceLevel::new(dec!(0.51), dec!(1000))],
            asks: vec![PriceLevel::new(dec!(0.52), dec!(1000))],
            timestamp_ns: Self::now_ns(),
        });

        // Should detect opportunity
        let result = detector.check_opportunity(&market_id).unwrap();
        assert!(result.is_some());

        let opp = result.unwrap();
        assert_eq!(opp.yes_price, dec!(0.45));
        assert_eq!(opp.no_price, dec!(0.52));
        assert_eq!(opp.spread, dec!(0.03));
        assert_eq!(opp.profit_bps, 300);
    }

    fn now_ns() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    }
}
