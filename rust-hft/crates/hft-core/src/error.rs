//! Error types for HFT operations

use rust_decimal::Decimal;
use thiserror::Error;

/// HFT error types
#[derive(Debug, Error)]
pub enum HftError {
    /// WebSocket connection failed
    #[error("WebSocket connection failed: {0}")]
    WebSocketConnection(String),

    /// WebSocket message parsing failed
    #[error("Failed to parse WebSocket message: {0}")]
    MessageParse(String),

    /// Order rejected by CLOB
    #[error("Order rejected: {reason} (code: {code:?})")]
    OrderRejected { reason: String, code: Option<i32> },

    /// Insufficient liquidity for trade
    #[error("Insufficient liquidity: need {needed} USD, available {available} USD")]
    InsufficientLiquidity { needed: Decimal, available: Decimal },

    /// Rate limited by API
    #[error("Rate limited: retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },

    /// Circuit breaker triggered
    #[error("Circuit breaker triggered: {reason}")]
    CircuitBreaker { reason: String },

    /// Position limit exceeded
    #[error("Position limit exceeded: current {current}, max {max}")]
    PositionLimitExceeded { current: usize, max: usize },

    /// Exposure limit exceeded
    #[error("Exposure limit exceeded: current {current} USD, max {max} USD")]
    ExposureLimitExceeded { current: Decimal, max: Decimal },

    /// Market not found
    #[error("Market not found: {0}")]
    MarketNotFound(String),

    /// Signing error
    #[error("Failed to sign order: {0}")]
    SigningError(String),

    /// HTTP request failed
    #[error("HTTP request failed: {0}")]
    HttpError(String),

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),

    /// Opportunity expired
    #[error("Arbitrage opportunity expired: age {age_ms}ms > max {max_age_ms}ms")]
    OpportunityExpired { age_ms: u64, max_age_ms: u64 },

    /// Cooldown active
    #[error("Cooldown active for market {market_id}: {remaining_ms}ms remaining")]
    CooldownActive {
        market_id: String,
        remaining_ms: u64,
    },
}

impl HftError {
    /// Check if error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            HftError::WebSocketConnection(_)
                | HftError::RateLimited { .. }
                | HftError::HttpError(_)
        )
    }

    /// Check if error should trigger circuit breaker
    pub fn should_circuit_break(&self) -> bool {
        matches!(
            self,
            HftError::CircuitBreaker { .. } | HftError::SigningError(_) | HftError::ConfigError(_)
        )
    }
}

/// Result type for HFT operations
pub type HftResult<T> = Result<T, HftError>;
