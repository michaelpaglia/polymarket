//! Signal detection engine for crypto latency exploitation
//!
//! Detects opportunities when Binance price moves significantly
//! before Polymarket orderbook catches up.

mod config;
mod detector;

pub use config::*;
pub use detector::*;
