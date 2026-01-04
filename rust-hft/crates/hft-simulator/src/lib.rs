//! Paper trading simulation for crypto latency strategy
//!
//! Simulates order execution with configurable fill rates,
//! slippage, and tracks comprehensive P&L statistics.

mod config;
mod simulator;
mod stats;

pub use config::*;
pub use simulator::*;
pub use stats::*;
