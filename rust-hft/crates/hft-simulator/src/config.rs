//! Paper trading configuration

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Paper trading simulation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulatorConfig {
    /// Starting capital in USD
    pub initial_capital: Decimal,
    /// Simulated fill rate (0.0 to 1.0)
    pub fill_rate: f64,
    /// Simulated slippage in basis points
    pub slippage_bps: u32,
    /// Simulated execution latency (ms)
    pub execution_latency_ms: u64,
    /// Whether to simulate partial fills
    pub simulate_partial_fills: bool,
    /// Partial fill rate when partial fills enabled
    pub partial_fill_rate: f64,
    /// Commission per trade in basis points
    pub commission_bps: u32,
    /// Maximum trades to keep in history
    pub max_trade_history: usize,
}

impl Default for SimulatorConfig {
    fn default() -> Self {
        Self {
            initial_capital: Decimal::from(5000),
            fill_rate: 0.95,
            slippage_bps: 5,
            execution_latency_ms: 50,
            simulate_partial_fills: false,
            partial_fill_rate: 0.3,
            commission_bps: 0,
            max_trade_history: 10000,
        }
    }
}

impl SimulatorConfig {
    /// Create optimistic config (high fill rate, low slippage)
    pub fn optimistic() -> Self {
        Self {
            fill_rate: 0.98,
            slippage_bps: 2,
            execution_latency_ms: 30,
            ..Default::default()
        }
    }

    /// Create pessimistic config (lower fill rate, higher slippage)
    pub fn pessimistic() -> Self {
        Self {
            fill_rate: 0.85,
            slippage_bps: 10,
            execution_latency_ms: 100,
            simulate_partial_fills: true,
            partial_fill_rate: 0.2,
            ..Default::default()
        }
    }
}
