//! HFT WebSocket - High-performance WebSocket client for Polymarket
//!
//! Features:
//! - SIMD-accelerated JSON parsing
//! - Lock-free price updates
//! - Automatic reconnection

pub mod client;
pub mod parser;
pub mod messages;

pub use client::*;
pub use parser::*;
pub use messages::*;
