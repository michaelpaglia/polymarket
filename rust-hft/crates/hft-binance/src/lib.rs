//! Crypto price feeds for real-time BTC/ETH prices
//!
//! Supports multiple sources:
//! - Binance (preferred, works from EU)
//! - Coinbase (fallback, works in US)
//! - Kraken (alternative, works globally)
//!
//! Provides low-latency price streaming with automatic reconnection
//! and momentum calculation.

mod client;
mod coinbase;
mod kraken;
mod types;

pub use client::*;
pub use coinbase::*;
pub use kraken::*;
pub use types::*;

use std::sync::Arc;
use thiserror::Error;

/// Unified error type for price feeds
#[derive(Debug, Error)]
pub enum PriceFeedError {
    #[error("Connection error: {0}")]
    Connection(String),
    #[error("Parse error: {0}")]
    Parse(String),
}

impl From<BinanceError> for PriceFeedError {
    fn from(e: BinanceError) -> Self {
        match e {
            BinanceError::Connection(s) => PriceFeedError::Connection(s),
            BinanceError::Parse(s) => PriceFeedError::Parse(s),
        }
    }
}

impl From<CoinbaseError> for PriceFeedError {
    fn from(e: CoinbaseError) -> Self {
        match e {
            CoinbaseError::Connection(s) => PriceFeedError::Connection(s),
            CoinbaseError::Parse(s) => PriceFeedError::Parse(s),
        }
    }
}

impl From<KrakenError> for PriceFeedError {
    fn from(e: KrakenError) -> Self {
        match e {
            KrakenError::Connection(s) => PriceFeedError::Connection(s),
            KrakenError::Parse(s) => PriceFeedError::Parse(s),
        }
    }
}

/// Unified price feed that can use Binance, Coinbase, or Kraken
pub enum PriceFeed {
    Binance(Arc<BinanceClient>),
    Coinbase(Arc<CoinbaseClient>),
    Kraken(Arc<KrakenClient>),
}

impl PriceFeed {
    /// Create a new Binance price feed
    pub fn binance(config: BinanceConfig) -> Self {
        PriceFeed::Binance(Arc::new(BinanceClient::new(config)))
    }

    /// Create a new Coinbase price feed
    pub fn coinbase(config: CoinbaseConfig) -> Self {
        PriceFeed::Coinbase(Arc::new(CoinbaseClient::new(config)))
    }

    /// Create a new Kraken price feed
    pub fn kraken(config: KrakenConfig) -> Self {
        PriceFeed::Kraken(Arc::new(KrakenClient::new(config)))
    }

    /// Connect to the price feed
    pub async fn connect(&self) -> Result<(), PriceFeedError> {
        match self {
            PriceFeed::Binance(client) => client.connect().await.map_err(Into::into),
            PriceFeed::Coinbase(client) => client.connect().await.map_err(Into::into),
            PriceFeed::Kraken(client) => client.connect().await.map_err(Into::into),
        }
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        match self {
            PriceFeed::Binance(client) => client.is_connected(),
            PriceFeed::Coinbase(client) => client.is_connected(),
            PriceFeed::Kraken(client) => client.is_connected(),
        }
    }

    /// Stop the connection
    pub fn stop(&self) {
        match self {
            PriceFeed::Binance(client) => client.stop(),
            PriceFeed::Coinbase(client) => client.stop(),
            PriceFeed::Kraken(client) => client.stop(),
        }
    }

    /// Get momentum for a specific asset
    pub fn get_momentum(&self, asset: CryptoAsset) -> Option<PriceMomentum> {
        match self {
            PriceFeed::Binance(client) => client.get_momentum(asset),
            PriceFeed::Coinbase(client) => client.get_momentum(asset),
            PriceFeed::Kraken(client) => client.get_momentum(asset),
        }
    }

    /// Get all current momenta
    pub fn get_all_momenta(&self) -> Vec<PriceMomentum> {
        match self {
            PriceFeed::Binance(client) => client.get_all_momenta(),
            PriceFeed::Coinbase(client) => client.get_all_momenta(),
            PriceFeed::Kraken(client) => client.get_all_momenta(),
        }
    }

    /// Get the name of the feed source
    pub fn source_name(&self) -> &'static str {
        match self {
            PriceFeed::Binance(_) => "Binance",
            PriceFeed::Coinbase(_) => "Coinbase",
            PriceFeed::Kraken(_) => "Kraken",
        }
    }

    /// Get messages received count (for debugging)
    pub fn messages_received(&self) -> u64 {
        match self {
            PriceFeed::Binance(client) => client.messages_received(),
            PriceFeed::Coinbase(client) => client.messages_received(),
            PriceFeed::Kraken(client) => client.messages_received(),
        }
    }
}
