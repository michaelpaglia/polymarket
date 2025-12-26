//! Circuit breaker for emergency stop

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{error, warn};

/// Circuit breaker configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Maximum daily loss before triggering (USD)
    pub max_daily_loss_usd: Decimal,
    /// Maximum consecutive failures before triggering
    pub max_consecutive_failures: u32,
    /// Maximum loss per trade before triggering (USD)
    pub max_trade_loss_usd: Decimal,
    /// Auto-reset after this many seconds (0 = manual reset only)
    pub auto_reset_seconds: u64,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            max_daily_loss_usd: dec!(500),
            max_consecutive_failures: 10,
            max_trade_loss_usd: dec!(100),
            auto_reset_seconds: 0, // Manual reset only
        }
    }
}

/// Circuit breaker state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerState {
    pub is_tripped: bool,
    pub trip_reason: Option<String>,
    pub tripped_at: Option<DateTime<Utc>>,
    pub daily_pnl: Decimal,
    pub consecutive_failures: u32,
    pub last_reset: DateTime<Utc>,
}

/// Circuit breaker for emergency stop
pub struct CircuitBreaker {
    config: RwLock<CircuitBreakerConfig>,
    tripped: AtomicBool,
    state: RwLock<CircuitBreakerState>,
}

impl CircuitBreaker {
    /// Create new circuit breaker
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config: RwLock::new(config),
            tripped: AtomicBool::new(false),
            state: RwLock::new(CircuitBreakerState {
                is_tripped: false,
                trip_reason: None,
                tripped_at: None,
                daily_pnl: Decimal::ZERO,
                consecutive_failures: 0,
                last_reset: Utc::now(),
            }),
        }
    }

    /// Check if circuit breaker is tripped
    pub fn is_tripped(&self) -> bool {
        self.tripped.load(Ordering::SeqCst)
    }

    /// Record a trade result
    pub fn record_trade(&self, pnl: Decimal, success: bool) {
        let config = self.config.read();
        let mut state = self.state.write();

        // Update daily P&L
        state.daily_pnl += pnl;

        // Update failure count
        if success {
            state.consecutive_failures = 0;
        } else {
            state.consecutive_failures += 1;
        }

        // Check daily loss limit
        if state.daily_pnl < -config.max_daily_loss_usd {
            self.trip(&format!(
                "Daily loss limit exceeded: {} < -{}",
                state.daily_pnl, config.max_daily_loss_usd
            ));
            return;
        }

        // Check consecutive failures
        if state.consecutive_failures >= config.max_consecutive_failures {
            self.trip(&format!(
                "Max consecutive failures reached: {}",
                state.consecutive_failures
            ));
            return;
        }

        // Check single trade loss
        if pnl < -config.max_trade_loss_usd {
            self.trip(&format!(
                "Single trade loss too large: {} < -{}",
                pnl, config.max_trade_loss_usd
            ));
        }
    }

    /// Manually trip the circuit breaker
    pub fn trip(&self, reason: &str) {
        if self.tripped.swap(true, Ordering::SeqCst) {
            return; // Already tripped
        }

        error!(reason = reason, "Circuit breaker TRIPPED");

        let mut state = self.state.write();
        state.is_tripped = true;
        state.trip_reason = Some(reason.to_string());
        state.tripped_at = Some(Utc::now());
    }

    /// Reset the circuit breaker
    pub fn reset(&self) {
        if !self.tripped.swap(false, Ordering::SeqCst) {
            return; // Already reset
        }

        warn!("Circuit breaker RESET");

        let mut state = self.state.write();
        state.is_tripped = false;
        state.trip_reason = None;
        state.tripped_at = None;
        state.consecutive_failures = 0;
    }

    /// Reset daily P&L (call at start of trading day)
    pub fn reset_daily(&self) {
        let mut state = self.state.write();
        state.daily_pnl = Decimal::ZERO;
        state.last_reset = Utc::now();
    }

    /// Check if should auto-reset
    pub fn check_auto_reset(&self) -> bool {
        let config = self.config.read();
        if config.auto_reset_seconds == 0 {
            return false;
        }

        let state = self.state.read();
        if let Some(tripped_at) = state.tripped_at {
            let elapsed = Utc::now() - tripped_at;
            if elapsed.num_seconds() as u64 >= config.auto_reset_seconds {
                drop(state);
                self.reset();
                return true;
            }
        }

        false
    }

    /// Get current state
    pub fn state(&self) -> CircuitBreakerState {
        self.state.read().clone()
    }

    /// Update configuration
    pub fn update_config(&self, config: CircuitBreakerConfig) {
        *self.config.write() = config;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_daily_loss() {
        let config = CircuitBreakerConfig {
            max_daily_loss_usd: dec!(100),
            ..Default::default()
        };

        let cb = CircuitBreaker::new(config);

        // Record losing trades
        cb.record_trade(dec!(-30), true);
        assert!(!cb.is_tripped());

        cb.record_trade(dec!(-30), true);
        assert!(!cb.is_tripped());

        cb.record_trade(dec!(-50), true); // Total: -110
        assert!(cb.is_tripped());
    }

    #[test]
    fn test_circuit_breaker_failures() {
        let config = CircuitBreakerConfig {
            max_consecutive_failures: 3,
            ..Default::default()
        };

        let cb = CircuitBreaker::new(config);

        cb.record_trade(dec!(0), false);
        assert!(!cb.is_tripped());

        cb.record_trade(dec!(0), false);
        assert!(!cb.is_tripped());

        cb.record_trade(dec!(0), false);
        assert!(cb.is_tripped());
    }
}
