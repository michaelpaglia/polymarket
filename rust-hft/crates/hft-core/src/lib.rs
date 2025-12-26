//! HFT Core - Core types and arbitrage detection for Polymarket HFT
//!
//! This crate provides:
//! - Core data types (MarketId, TokenId, prices, etc.)
//! - Lock-free orderbook structure
//! - Arbitrage opportunity detection
//! - Error types

pub mod types;
pub mod orderbook;
pub mod arbitrage;
pub mod error;

pub use types::*;
pub use orderbook::*;
pub use arbitrage::*;
pub use error::*;
