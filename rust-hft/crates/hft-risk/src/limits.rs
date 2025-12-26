//! Position and exposure limits

use dashmap::DashMap;
use hft_core::{ArbitrageOpportunity, HftError, HftResult, MarketId};
use parking_lot::RwLock;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, warn};

/// Risk limits configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskLimits {
    /// Maximum total exposure in USD
    pub max_total_exposure_usd: Decimal,
    /// Maximum exposure per market in USD
    pub max_market_exposure_usd: Decimal,
    /// Maximum number of simultaneous positions
    pub max_positions: usize,
    /// Maximum position size per trade in USD
    pub max_position_size_usd: Decimal,
    /// Minimum position size in USD
    pub min_position_size_usd: Decimal,
}

impl Default for RiskLimits {
    fn default() -> Self {
        Self {
            max_total_exposure_usd: dec!(5000),
            max_market_exposure_usd: dec!(1000),
            max_positions: 10,
            max_position_size_usd: dec!(500),
            min_position_size_usd: dec!(10),
        }
    }
}

/// Position in a market
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub market_id: MarketId,
    pub yes_shares: Decimal,
    pub no_shares: Decimal,
    pub yes_cost: Decimal,
    pub no_cost: Decimal,
    pub opened_at_ns: u64,
}

impl Position {
    /// Total cost of this position
    pub fn total_cost(&self) -> Decimal {
        self.yes_cost + self.no_cost
    }

    /// Calculate current value at market prices
    pub fn current_value(&self, yes_price: Decimal, no_price: Decimal) -> Decimal {
        self.yes_shares * yes_price + self.no_shares * no_price
    }

    /// Calculate unrealized P&L
    pub fn unrealized_pnl(&self, yes_price: Decimal, no_price: Decimal) -> Decimal {
        self.current_value(yes_price, no_price) - self.total_cost()
    }
}

/// Risk manager for enforcing limits
pub struct RiskManager {
    limits: RwLock<RiskLimits>,
    positions: DashMap<MarketId, Position>,
    total_exposure: RwLock<Decimal>,
    available_capital: RwLock<Decimal>,
    checks_passed: AtomicU64,
    checks_failed: AtomicU64,
}

impl RiskManager {
    /// Create new risk manager
    pub fn new(limits: RiskLimits, initial_capital: Decimal) -> Self {
        Self {
            limits: RwLock::new(limits),
            positions: DashMap::new(),
            total_exposure: RwLock::new(Decimal::ZERO),
            available_capital: RwLock::new(initial_capital),
            checks_passed: AtomicU64::new(0),
            checks_failed: AtomicU64::new(0),
        }
    }

    /// Check if opportunity can be executed
    pub fn check_opportunity(&self, opportunity: &ArbitrageOpportunity) -> HftResult<Decimal> {
        let limits = self.limits.read();

        // Check position count
        if self.positions.len() >= limits.max_positions {
            self.checks_failed.fetch_add(1, Ordering::Relaxed);
            return Err(HftError::PositionLimitExceeded {
                current: self.positions.len(),
                max: limits.max_positions,
            });
        }

        // Check if already have position in this market
        if self.positions.contains_key(&opportunity.market_id) {
            self.checks_failed.fetch_add(1, Ordering::Relaxed);
            return Err(HftError::Internal(format!(
                "Already have position in market {}",
                opportunity.market_id
            )));
        }

        // Calculate required exposure (buy both YES and NO)
        let required = opportunity.max_size_usd * Decimal::TWO;

        // Check total exposure
        let current_exposure = *self.total_exposure.read();
        if current_exposure + required > limits.max_total_exposure_usd {
            self.checks_failed.fetch_add(1, Ordering::Relaxed);
            return Err(HftError::ExposureLimitExceeded {
                current: current_exposure,
                max: limits.max_total_exposure_usd,
            });
        }

        // Check per-market exposure
        if required > limits.max_market_exposure_usd {
            self.checks_failed.fetch_add(1, Ordering::Relaxed);
            return Err(HftError::ExposureLimitExceeded {
                current: required,
                max: limits.max_market_exposure_usd,
            });
        }

        // Check available capital
        let available = *self.available_capital.read();
        if required > available {
            self.checks_failed.fetch_add(1, Ordering::Relaxed);
            return Err(HftError::InsufficientLiquidity {
                needed: required,
                available,
            });
        }

        // Check min/max position size
        let position_size = opportunity.max_size_usd;
        if position_size < limits.min_position_size_usd {
            self.checks_failed.fetch_add(1, Ordering::Relaxed);
            return Err(HftError::Internal(format!(
                "Position size {} below minimum {}",
                position_size, limits.min_position_size_usd
            )));
        }

        if position_size > limits.max_position_size_usd {
            // Cap at max, don't reject
            let capped = limits.max_position_size_usd;
            debug!(
                original = %position_size,
                capped = %capped,
                "Position size capped at maximum"
            );
            self.checks_passed.fetch_add(1, Ordering::Relaxed);
            return Ok(capped);
        }

        self.checks_passed.fetch_add(1, Ordering::Relaxed);
        Ok(position_size)
    }

    /// Record a new position
    pub fn record_position(&self, position: Position) {
        let cost = position.total_cost();

        // Update exposure
        {
            let mut exposure = self.total_exposure.write();
            *exposure += cost;
        }

        // Update available capital
        {
            let mut capital = self.available_capital.write();
            *capital -= cost;
        }

        // Store position
        self.positions.insert(position.market_id.clone(), position);
    }

    /// Close a position
    pub fn close_position(&self, market_id: &MarketId, pnl: Decimal) {
        if let Some((_, position)) = self.positions.remove(market_id) {
            let cost = position.total_cost();

            // Update exposure
            {
                let mut exposure = self.total_exposure.write();
                *exposure -= cost;
            }

            // Update available capital (return cost + P&L)
            {
                let mut capital = self.available_capital.write();
                *capital += cost + pnl;
            }
        }
    }

    /// Get current total exposure
    pub fn total_exposure(&self) -> Decimal {
        *self.total_exposure.read()
    }

    /// Get available capital
    pub fn available_capital(&self) -> Decimal {
        *self.available_capital.read()
    }

    /// Get position count
    pub fn position_count(&self) -> usize {
        self.positions.len()
    }

    /// Get all positions
    pub fn positions(&self) -> Vec<Position> {
        self.positions.iter().map(|r| r.value().clone()).collect()
    }

    /// Update limits
    pub fn update_limits(&self, limits: RiskLimits) {
        *self.limits.write() = limits;
    }

    /// Set capital allocation
    pub fn set_capital(&self, amount: Decimal) {
        let current_exposure = *self.total_exposure.read();
        let mut capital = self.available_capital.write();
        *capital = amount - current_exposure;
    }

    /// Get statistics
    pub fn stats(&self) -> RiskStats {
        RiskStats {
            total_exposure: self.total_exposure(),
            available_capital: self.available_capital(),
            position_count: self.position_count(),
            checks_passed: self.checks_passed.load(Ordering::Relaxed),
            checks_failed: self.checks_failed.load(Ordering::Relaxed),
        }
    }
}

/// Risk statistics
#[derive(Debug, Clone, Serialize)]
pub struct RiskStats {
    pub total_exposure: Decimal,
    pub available_capital: Decimal,
    pub position_count: usize,
    pub checks_passed: u64,
    pub checks_failed: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use hft_core::TokenId;

    #[test]
    fn test_risk_check() {
        let limits = RiskLimits {
            max_positions: 5,
            max_total_exposure_usd: dec!(1000),
            ..Default::default()
        };

        let manager = RiskManager::new(limits, dec!(1000));

        let opportunity = ArbitrageOpportunity {
            opportunity_id: "test".to_string(),
            market_id: MarketId::new("market1"),
            yes_token_id: TokenId::new("yes"),
            no_token_id: TokenId::new("no"),
            yes_price: dec!(0.45),
            no_price: dec!(0.52),
            spread: dec!(0.03),
            profit_bps: 300,
            max_size_usd: dec!(100),
            detected_at_ns: 0,
            confidence: 0.8,
        };

        // Should pass
        let result = manager.check_opportunity(&opportunity);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), dec!(100));
    }
}
