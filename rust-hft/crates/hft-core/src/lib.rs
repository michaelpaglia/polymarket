//! HFT Core - Core types and trading logic for Polymarket HFT
//!
//! This crate provides:
//! - Core data types (MarketId, TokenId, prices, etc.)
//! - Lock-free orderbook structure
//! - Arbitrage opportunity detection
//! - Crypto directional trading types (for 15-min markets)
//! - Error types

pub mod arbitrage;
pub mod crypto;
pub mod error;
pub mod orderbook;
pub mod types;

pub use arbitrage::*;
pub use crypto::*;
pub use error::*;
pub use orderbook::*;
pub use types::*;
