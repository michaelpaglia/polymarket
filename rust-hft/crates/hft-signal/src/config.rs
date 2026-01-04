//! Signal detection configuration

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Signal detection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalConfig {
    /// Minimum price change in bps to trigger evaluation
    pub min_trigger_bps: i32,
    /// Minimum edge in bps to generate signal
    pub min_edge_bps: u32,
    /// Maximum Polymarket data age (ms) - too stale is unreliable
    pub max_polymarket_age_ms: u64,
    /// Minimum Polymarket data age (ms) - need some lag to exploit
    pub min_polymarket_age_ms: u64,
    /// Minimum confidence score (0.0 to 1.0)
    pub min_confidence: f64,
    /// Cooldown between signals on same asset (ms)
    pub asset_cooldown_ms: u64,
    /// Maximum signals per minute (rate limiting)
    pub max_signals_per_minute: u32,
    /// Sensitivity for price-to-probability mapping
    pub price_sensitivity: f64,
    /// Base probability at strike price
    pub base_probability: f64,
}

impl Default for SignalConfig {
    fn default() -> Self {
        Self {
            min_trigger_bps: 15,        // 0.15% move triggers evaluation
            min_edge_bps: 10,           // 0.10% minimum edge
            max_polymarket_age_ms: 500, // 500ms max stale
            min_polymarket_age_ms: 50,  // 50ms minimum lag
            min_confidence: 0.6,
            asset_cooldown_ms: 5000, // 5 seconds per asset
            max_signals_per_minute: 30,
            price_sensitivity: 0.003, // Prob change per bps of price move
            base_probability: 0.5,
        }
    }
}

impl SignalConfig {
    /// Create config for more aggressive trading
    pub fn aggressive() -> Self {
        Self {
            min_trigger_bps: 10,
            min_edge_bps: 8,
            max_polymarket_age_ms: 800,
            min_polymarket_age_ms: 30,
            min_confidence: 0.5,
            asset_cooldown_ms: 3000,
            max_signals_per_minute: 50,
            price_sensitivity: 0.004,
            base_probability: 0.5,
        }
    }

    /// Create config for more conservative trading
    pub fn conservative() -> Self {
        Self {
            min_trigger_bps: 20,
            min_edge_bps: 15,
            max_polymarket_age_ms: 400,
            min_polymarket_age_ms: 80,
            min_confidence: 0.7,
            asset_cooldown_ms: 10000,
            max_signals_per_minute: 15,
            price_sensitivity: 0.002,
            base_probability: 0.5,
        }
    }
}
