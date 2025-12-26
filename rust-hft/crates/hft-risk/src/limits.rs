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
    /// Total account balance (fetched periodically)
    account_balance: RwLock<Decimal>,
    /// Maximum percentage of account to use (0.0 - 1.0)
    max_balance_percentage: RwLock<Decimal>,
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
            account_balance: RwLock::new(initial_capital * Decimal::TWO), // Assume 50% allocation initially
            max_balance_percentage: RwLock::new(dec!(0.50)), // 50% default
            checks_passed: AtomicU64::new(0),
            checks_failed: AtomicU64::new(0),
        }
    }

    /// Create with specific balance percentage cap
    pub fn with_balance_cap(limits: RiskLimits, initial_capital: Decimal, max_percentage: Decimal) -> Self {
        Self {
            limits: RwLock::new(limits),
            positions: DashMap::new(),
            total_exposure: RwLock::new(Decimal::ZERO),
            available_capital: RwLock::new(initial_capital),
            account_balance: RwLock::new(initial_capital / max_percentage),
            max_balance_percentage: RwLock::new(max_percentage),
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

    /// Update account balance and recalculate available capital based on percentage cap
    /// This should be called periodically to sync with actual wallet balance
    pub fn sync_balance(&self, total_account_balance: Decimal) {
        let max_pct = *self.max_balance_percentage.read();
        let max_allowed = total_account_balance * max_pct;

        // Update account balance
        {
            let mut balance = self.account_balance.write();
            *balance = total_account_balance;
        }

        // Recalculate available capital (max_allowed minus current exposure)
        let current_exposure = *self.total_exposure.read();
        let new_available = (max_allowed - current_exposure).max(Decimal::ZERO);

        {
            let mut capital = self.available_capital.write();
            *capital = new_available;
        }

        debug!(
            account_balance = %total_account_balance,
            max_percentage = %max_pct,
            max_allowed = %max_allowed,
            current_exposure = %current_exposure,
            available_capital = %new_available,
            "Balance synced"
        );
    }

    /// Set the maximum balance percentage (0.0 - 1.0)
    pub fn set_balance_percentage(&self, percentage: Decimal) {
        let clamped = percentage.max(dec!(0.01)).min(dec!(1.0));
        {
            let mut pct = self.max_balance_percentage.write();
            *pct = clamped;
        }

        // Recalculate with current balance
        let current_balance = *self.account_balance.read();
        self.sync_balance(current_balance);
    }

    /// Get current balance allocation info
    pub fn balance_info(&self) -> BalanceInfo {
        let account_balance = *self.account_balance.read();
        let max_pct = *self.max_balance_percentage.read();
        let max_allowed = account_balance * max_pct;
        let current_exposure = *self.total_exposure.read();
        let available = *self.available_capital.read();

        BalanceInfo {
            account_balance,
            max_percentage: max_pct,
            max_allowed,
            current_exposure,
            available_capital: available,
            utilization: if max_allowed > Decimal::ZERO {
                current_exposure / max_allowed
            } else {
                Decimal::ZERO
            },
        }
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

/// Balance allocation information
#[derive(Debug, Clone, Serialize)]
pub struct BalanceInfo {
    /// Total account balance (from wallet)
    pub account_balance: Decimal,
    /// Maximum percentage of account to use (0.0 - 1.0)
    pub max_percentage: Decimal,
    /// Maximum allowed capital (account_balance * max_percentage)
    pub max_allowed: Decimal,
    /// Current exposure in positions
    pub current_exposure: Decimal,
    /// Available capital for new trades
    pub available_capital: Decimal,
    /// Current utilization (exposure / max_allowed)
    pub utilization: Decimal,
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
