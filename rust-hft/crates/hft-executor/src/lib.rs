//! HFT Executor - Order execution with EIP-712 signing
//!
//! Handles order creation, signing, and submission to Polymarket CLOB.

pub mod client;
pub mod redeemer;
pub mod signer;

pub use client::*;
pub use redeemer::*;
pub use signer::*;
