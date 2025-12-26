//! HFT API - REST API for Python control
//!
//! Provides HTTP endpoints for:
//! - Start/stop trading
//! - Configuration management
//! - Statistics and metrics
//! - Market subscription

pub mod routes;
pub mod handlers;
pub mod state;

pub use routes::*;
pub use handlers::*;
pub use state::*;
