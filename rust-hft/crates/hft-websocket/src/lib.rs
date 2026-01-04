//! HFT WebSocket - High-performance WebSocket client for Polymarket
//!
//! Features:
//! - SIMD-accelerated JSON parsing
//! - Lock-free price updates
//! - Automatic reconnection

pub mod client;
pub mod messages;
pub mod parser;

pub use client::*;
pub use messages::*;
pub use parser::*;
