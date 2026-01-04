//! Signal detection implementation

use crate::SignalConfig;
use dashmap::DashMap;
use hft_binance::{CryptoAsset, Direction, PriceMomentum};
use hft_core::{CryptoMarket, LatencySignal, MarketOrderbook};
use parking_lot::RwLock;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Get current time in nanoseconds
fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

/// Signal detector
pub struct SignalDetector {
    config: SignalConfig,
    /// Last signal time per asset (for cooldown)
    last_signal_time: DashMap<CryptoAsset, u64>,
    /// Recent signals for rate limiting
    recent_signals: RwLock<VecDeque<u64>>,
    /// Statistics
    signals_detected: AtomicU64,
    signals_rejected: AtomicU64,
}

impl SignalDetector {
    /// Create new signal detector
    pub fn new(config: SignalConfig) -> Self {
        Self {
            config,
            last_signal_time: DashMap::new(),
            recent_signals: RwLock::new(VecDeque::with_capacity(100)),
            signals_detected: AtomicU64::new(0),
            signals_rejected: AtomicU64::new(0),
        }
    }

    /// Get number of signals detected
    pub fn signals_detected(&self) -> u64 {
        self.signals_detected.load(Ordering::Relaxed)
    }

    /// Get number of signals rejected
    pub fn signals_rejected(&self) -> u64 {
        self.signals_rejected.load(Ordering::Relaxed)
    }

    /// Detect signal from momentum and orderbook
    pub fn detect_signal(
        &self,
        momentum: &PriceMomentum,
        orderbook: &MarketOrderbook,
        market: &CryptoMarket,
    ) -> Option<LatencySignal> {
        // 1. Check if price movement is significant
        let change = momentum.change_1s_bps;
        if change.abs() < self.config.min_trigger_bps {
            return None;
        }

        // 2. Determine direction
        let direction = if change > 0 {
            Direction::Up
        } else {
            Direction::Down
        };

        // 3. Get current Polymarket price for the directional outcome
        // For "up" direction, we want to buy the "up" token
        // The ask price is what we pay to buy
        let pm_price = match direction {
            Direction::Up => orderbook.yes_book.best_ask(),
            Direction::Down => orderbook.no_book.best_ask(),
        };

        // Check if price is valid (non-zero means we have data)
        if pm_price.is_zero() {
            return None;
        }

        // 4. Check Polymarket data freshness
        let pm_age_ms = orderbook.min_age_ms();

        if pm_age_ms < self.config.min_polymarket_age_ms {
            debug!(
                age_ms = pm_age_ms,
                min = self.config.min_polymarket_age_ms,
                "Polymarket data too fresh (no lag to exploit)"
            );
            return None;
        }

        if pm_age_ms > self.config.max_polymarket_age_ms {
            debug!(
                age_ms = pm_age_ms,
                max = self.config.max_polymarket_age_ms,
                "Polymarket data too stale"
            );
            return None;
        }

        // 5. Calculate expected fair price based on Binance movement
        let expected_price = self.calculate_fair_price(momentum, market, direction);

        // 6. Calculate edge
        let edge = expected_price - pm_price;
        if edge <= Decimal::ZERO {
            debug!(
                expected = %expected_price,
                current = %pm_price,
                "No positive edge"
            );
            return None;
        }

        let edge_bps = self.decimal_to_bps(edge / pm_price);

        if edge_bps < self.config.min_edge_bps {
            debug!(
                edge_bps = edge_bps,
                min = self.config.min_edge_bps,
                "Edge too small"
            );
            return None;
        }

        // 7. Calculate confidence score
        let confidence = self.calculate_confidence(momentum, edge_bps, pm_age_ms);

        if confidence < self.config.min_confidence {
            debug!(
                confidence = confidence,
                min = self.config.min_confidence,
                "Confidence too low"
            );
            self.signals_rejected.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        // 8. Check cooldowns and rate limits
        if !self.check_rate_limits(momentum.asset) {
            debug!("Rate limit or cooldown active");
            self.signals_rejected.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        // 9. Record signal time
        let now = now_ns();
        self.last_signal_time.insert(momentum.asset, now);
        {
            let mut recent = self.recent_signals.write();
            recent.push_back(now);
        }

        self.signals_detected.fetch_add(1, Ordering::Relaxed);

        info!(
            asset = %momentum.asset,
            direction = %direction,
            edge_bps = edge_bps,
            confidence = format!("{:.2}", confidence),
            latency_ms = pm_age_ms,
            "Signal detected"
        );

        Some(LatencySignal {
            signal_id: Uuid::new_v4().to_string(),
            asset: self.convert_asset(momentum.asset),
            direction: self.convert_direction(direction),
            binance_price: momentum.current_price,
            price_change_bps: change,
            polymarket_price: pm_price,
            expected_price,
            edge_bps,
            confidence,
            detected_at_ns: now,
            binance_update_ns: momentum.last_update_ns,
            polymarket_update_ns: orderbook
                .yes_book
                .last_update_ns()
                .min(orderbook.no_book.last_update_ns()),
            latency_ms: pm_age_ms,
        })
    }

    /// Calculate expected fair price based on price movement
    fn calculate_fair_price(
        &self,
        momentum: &PriceMomentum,
        market: &CryptoMarket,
        direction: Direction,
    ) -> Decimal {
        // Model: probability moves based on price deviation from strike
        // If strike is unknown, use momentum directly

        let change_bps = momentum.change_1s_bps as f64;

        // Base probability is 0.5 (50/50)
        // Each bps of price movement shifts probability by sensitivity factor
        let prob_shift = change_bps * self.config.price_sensitivity;

        let prob_up = (self.config.base_probability + prob_shift)
            .max(0.05)
            .min(0.95);

        let fair_price = match direction {
            Direction::Up => prob_up,
            Direction::Down => 1.0 - prob_up,
        };

        Decimal::from_f64_retain(fair_price).unwrap_or(dec!(0.5))
    }

    /// Calculate confidence score
    fn calculate_confidence(
        &self,
        momentum: &PriceMomentum,
        edge_bps: u32,
        latency_ms: u64,
    ) -> f64 {
        // Weight factors for confidence calculation
        const MOMENTUM_WEIGHT: f64 = 0.30;
        const EDGE_WEIGHT: f64 = 0.30;
        const LATENCY_WEIGHT: f64 = 0.25;
        const VOLATILITY_WEIGHT: f64 = 0.15;

        // 1. Momentum strength score (0 to 1)
        let momentum_score = momentum.strength;

        // 2. Edge score (larger edge = more confident)
        // 50 bps = max score
        let edge_score = (edge_bps as f64 / 50.0).min(1.0);

        // 3. Latency score (sweet spot is 100-300ms)
        let latency_score = if latency_ms < 50 {
            0.0 // Too fresh
        } else if latency_ms < 100 {
            0.5 // Maybe some lag
        } else if latency_ms < 300 {
            1.0 // Ideal lag window
        } else if latency_ms < 500 {
            0.7 // Getting stale
        } else {
            0.3 // Too stale
        };

        // 4. Volatility score (high volatility = less confident)
        // 1% volatility = 0 score
        let volatility_score = (1.0 - momentum.volatility_1min / 0.01).max(0.0).min(1.0);

        // Weighted average
        momentum_score * MOMENTUM_WEIGHT
            + edge_score * EDGE_WEIGHT
            + latency_score * LATENCY_WEIGHT
            + volatility_score * VOLATILITY_WEIGHT
    }

    /// Check rate limits and cooldowns
    fn check_rate_limits(&self, asset: CryptoAsset) -> bool {
        let now = now_ns();
        let now_ms = now / 1_000_000;

        // Check asset cooldown
        if let Some(last) = self.last_signal_time.get(&asset) {
            let elapsed_ms = (now - *last) / 1_000_000;
            if elapsed_ms < self.config.asset_cooldown_ms {
                return false;
            }
        }

        // Check signals per minute
        let one_minute_ago = now - (60 * 1_000_000_000);
        {
            let mut recent = self.recent_signals.write();

            // Remove old signals
            while let Some(&front) = recent.front() {
                if front < one_minute_ago {
                    recent.pop_front();
                } else {
                    break;
                }
            }

            if recent.len() >= self.config.max_signals_per_minute as usize {
                return false;
            }
        }

        true
    }

    /// Convert to bps
    fn decimal_to_bps(&self, value: Decimal) -> u32 {
        let bps = value * dec!(10000);
        bps.round().to_u32().unwrap_or(0)
    }

    /// Convert binance asset to core asset
    fn convert_asset(&self, asset: CryptoAsset) -> hft_core::CryptoAsset {
        match asset {
            CryptoAsset::BTC => hft_core::CryptoAsset::BTC,
            CryptoAsset::ETH => hft_core::CryptoAsset::ETH,
        }
    }

    /// Convert binance direction to core direction
    fn convert_direction(&self, direction: Direction) -> hft_core::Direction {
        match direction {
            Direction::Up => hft_core::Direction::Up,
            Direction::Down => hft_core::Direction::Down,
        }
    }

    /// Update configuration
    pub fn update_config(&mut self, config: SignalConfig) {
        self.config = config;
    }

    /// Get current configuration
    pub fn config(&self) -> &SignalConfig {
        &self.config
    }

    /// Reset statistics
    pub fn reset_stats(&self) {
        self.signals_detected.store(0, Ordering::Relaxed);
        self.signals_rejected.store(0, Ordering::Relaxed);
        self.last_signal_time.clear();
        self.recent_signals.write().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detector_creation() {
        let detector = SignalDetector::new(SignalConfig::default());
        assert_eq!(detector.signals_detected(), 0);
    }

    #[test]
    fn test_confidence_calculation() {
        let detector = SignalDetector::new(SignalConfig::default());

        let momentum = PriceMomentum {
            asset: CryptoAsset::BTC,
            current_price: dec!(50000),
            change_1s_bps: 20,
            change_5s_bps: 50,
            change_1min_bps: 100,
            volatility_1min: 0.002,
            direction: Some(Direction::Up),
            strength: 0.8,
            last_update_ns: now_ns(),
        };

        let confidence = detector.calculate_confidence(&momentum, 25, 150);
        assert!(confidence > 0.5);
        assert!(confidence < 1.0);
    }
}
