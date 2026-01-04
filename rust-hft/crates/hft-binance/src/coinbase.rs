//! Coinbase WebSocket client for real-time BTC/ETH price feeds
//!
//! Alternative to Binance that works in the US without geo-restrictions.

use crate::client::ConnectionState;
use crate::types::*;
use arc_swap::ArcSwap;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

/// Coinbase WebSocket configuration
#[derive(Debug, Clone)]
pub struct CoinbaseConfig {
    /// WebSocket URL
    pub ws_url: String,
    /// Products to subscribe (e.g., ["BTC-USD", "ETH-USD"])
    pub products: Vec<String>,
    /// Reconnect delay in milliseconds
    pub reconnect_delay_ms: u64,
    /// Price window size (number of ticks)
    pub window_size: usize,
    /// Price window max age (ms)
    pub window_max_age_ms: u64,
}

impl Default for CoinbaseConfig {
    fn default() -> Self {
        Self {
            ws_url: "wss://ws-feed.exchange.coinbase.com".to_string(),
            products: vec!["BTC-USD".to_string(), "ETH-USD".to_string()],
            reconnect_delay_ms: 1000,
            window_size: 1000,
            window_max_age_ms: 60000,
        }
    }
}

/// Coinbase match message
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CoinbaseMatch {
    /// Message type (should be "match")
    #[serde(rename = "type")]
    pub msg_type: String,
    /// Trade ID
    pub trade_id: Option<u64>,
    /// Product ID (e.g., "BTC-USD")
    pub product_id: String,
    /// Trade price
    pub price: String,
    /// Trade size
    pub size: String,
    /// Side ("buy" or "sell")
    pub side: String,
    /// Timestamp
    pub time: Option<String>,
}

/// Coinbase WebSocket client
pub struct CoinbaseClient {
    config: CoinbaseConfig,
    /// Connection state
    state: Arc<ArcSwap<ConnectionState>>,
    /// Running flag
    running: Arc<AtomicBool>,
    /// Price windows per asset
    price_windows: Arc<HashMap<CryptoAsset, PriceWindow>>,
    /// Statistics
    messages_received: AtomicU64,
    reconnect_count: AtomicU64,
}

impl CoinbaseClient {
    /// Create new Coinbase client
    pub fn new(config: CoinbaseConfig) -> Self {
        let mut windows = HashMap::new();

        windows.insert(
            CryptoAsset::BTC,
            PriceWindow::new(
                CryptoAsset::BTC,
                config.window_size,
                config.window_max_age_ms,
            ),
        );
        windows.insert(
            CryptoAsset::ETH,
            PriceWindow::new(
                CryptoAsset::ETH,
                config.window_size,
                config.window_max_age_ms,
            ),
        );

        Self {
            config,
            state: Arc::new(ArcSwap::from_pointee(ConnectionState::Disconnected)),
            running: Arc::new(AtomicBool::new(false)),
            price_windows: Arc::new(windows),
            messages_received: AtomicU64::new(0),
            reconnect_count: AtomicU64::new(0),
        }
    }

    /// Get connection state
    pub fn connection_state(&self) -> ConnectionState {
        **self.state.load()
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.connection_state() == ConnectionState::Connected
    }

    /// Get message count
    pub fn messages_received(&self) -> u64 {
        self.messages_received.load(Ordering::Relaxed)
    }

    /// Get latest price for asset
    pub fn latest_price(&self, asset: CryptoAsset) -> Option<rust_decimal::Decimal> {
        self.price_windows.get(&asset)?.latest_price()
    }

    /// Get momentum for asset
    pub fn get_momentum(&self, asset: CryptoAsset) -> Option<PriceMomentum> {
        self.price_windows.get(&asset)?.calculate_momentum()
    }

    /// Get all momenta
    pub fn get_all_momenta(&self) -> Vec<PriceMomentum> {
        [CryptoAsset::BTC, CryptoAsset::ETH]
            .iter()
            .filter_map(|&asset| self.get_momentum(asset))
            .collect()
    }

    /// Start WebSocket connection
    pub async fn connect(&self) -> Result<(), CoinbaseError> {
        self.running.store(true, Ordering::SeqCst);

        let mut reconnect_attempts = 0;

        while self.running.load(Ordering::SeqCst) {
            self.state.store(Arc::new(ConnectionState::Connecting));

            match self.run_connection().await {
                Ok(_) => {
                    info!("Coinbase WebSocket disconnected cleanly");
                    break;
                }
                Err(e) => {
                    error!(error = %e, "Coinbase WebSocket error");

                    if !self.running.load(Ordering::SeqCst) {
                        break;
                    }

                    reconnect_attempts += 1;
                    self.reconnect_count.fetch_add(1, Ordering::Relaxed);

                    self.state.store(Arc::new(ConnectionState::Reconnecting));

                    let delay = self.config.reconnect_delay_ms * reconnect_attempts.min(10);
                    warn!(
                        delay_ms = delay,
                        attempt = reconnect_attempts,
                        "Reconnecting to Coinbase..."
                    );
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
            }
        }

        self.state.store(Arc::new(ConnectionState::Disconnected));
        Ok(())
    }

    /// Stop WebSocket connection
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Run a single connection
    async fn run_connection(&self) -> Result<(), CoinbaseError> {
        info!(url = %self.config.ws_url, "Connecting to Coinbase WebSocket...");

        let (ws_stream, _) = connect_async(&self.config.ws_url)
            .await
            .map_err(|e| CoinbaseError::Connection(e.to_string()))?;

        info!("Coinbase WebSocket connected");
        self.state.store(Arc::new(ConnectionState::Connected));

        let (mut write, mut read) = ws_stream.split();

        // Send subscribe message
        let subscribe = serde_json::json!({
            "type": "subscribe",
            "product_ids": self.config.products,
            "channels": ["matches"]
        });

        write
            .send(Message::Text(subscribe.to_string()))
            .await
            .map_err(|e| CoinbaseError::Connection(e.to_string()))?;

        info!(products = ?self.config.products, "Subscribed to Coinbase matches");

        // Message processing loop
        while let Some(msg_result) = read.next().await {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            match msg_result {
                Ok(Message::Text(text)) => {
                    self.handle_message(&text);
                }
                Ok(Message::Ping(data)) => {
                    debug!("Received ping from Coinbase");
                    if let Err(e) = write.send(Message::Pong(data)).await {
                        warn!(error = %e, "Failed to send pong");
                    }
                }
                Ok(Message::Close(frame)) => {
                    info!(frame = ?frame, "Coinbase sent close frame");
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    error!(error = %e, "Coinbase WebSocket read error");
                    return Err(CoinbaseError::Connection(e.to_string()));
                }
            }
        }

        Ok(())
    }

    /// Handle incoming message
    fn handle_message(&self, text: &str) {
        let received_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        // Try to parse as match
        if let Ok(msg) = serde_json::from_str::<CoinbaseMatch>(text) {
            if msg.msg_type == "match" {
                self.messages_received.fetch_add(1, Ordering::Relaxed);

                if let Some(tick) = self.parse_match(&msg, received_ns) {
                    if let Some(window) = self.price_windows.get(&tick.asset) {
                        window.add_tick(tick);
                    }
                }
            }
        }
    }

    /// Parse match message into tick
    fn parse_match(&self, msg: &CoinbaseMatch, received_ns: u64) -> Option<BinanceTick> {
        let asset = if msg.product_id.starts_with("BTC") {
            CryptoAsset::BTC
        } else if msg.product_id.starts_with("ETH") {
            CryptoAsset::ETH
        } else {
            return None;
        };

        let price = msg.price.parse::<rust_decimal::Decimal>().ok()?;
        let quantity = msg.size.parse::<rust_decimal::Decimal>().ok()?;

        Some(BinanceTick {
            asset,
            price,
            quantity,
            trade_time_ms: received_ns / 1_000_000,
            received_ns,
            is_sell: msg.side == "sell",
        })
    }
}

/// Coinbase client errors
#[derive(Debug, thiserror::Error)]
pub enum CoinbaseError {
    #[error("Connection error: {0}")]
    Connection(String),
    #[error("Parse error: {0}")]
    Parse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coinbase_config_default() {
        let config = CoinbaseConfig::default();
        assert!(config.ws_url.contains("coinbase.com"));
        assert_eq!(config.products.len(), 2);
    }

    #[test]
    fn test_parse_match() {
        let json = r#"{
            "type": "match",
            "trade_id": 123,
            "product_id": "BTC-USD",
            "price": "50000.50",
            "size": "0.5",
            "side": "buy",
            "time": "2024-01-01T00:00:00.000000Z"
        }"#;

        let msg: CoinbaseMatch = serde_json::from_str(json).unwrap();
        assert_eq!(msg.product_id, "BTC-USD");
        assert_eq!(msg.price, "50000.50");
    }
}
