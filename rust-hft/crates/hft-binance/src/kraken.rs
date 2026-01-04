//! Kraken WebSocket client for real-time BTC/ETH price feeds
//!
//! European exchange alternative that works globally without geo-restrictions.

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

/// Kraken WebSocket configuration
#[derive(Debug, Clone)]
pub struct KrakenConfig {
    /// WebSocket URL
    pub ws_url: String,
    /// Pairs to subscribe (e.g., ["XBT/USD", "ETH/USD"])
    pub pairs: Vec<String>,
    /// Reconnect delay in milliseconds
    pub reconnect_delay_ms: u64,
    /// Price window size (number of ticks)
    pub window_size: usize,
    /// Price window max age (ms)
    pub window_max_age_ms: u64,
}

impl Default for KrakenConfig {
    fn default() -> Self {
        Self {
            ws_url: "wss://ws.kraken.com".to_string(),
            pairs: vec!["XBT/USD".to_string(), "ETH/USD".to_string()],
            reconnect_delay_ms: 1000,
            window_size: 1000,
            window_max_age_ms: 60000,
        }
    }
}

/// Kraken WebSocket client
pub struct KrakenClient {
    config: KrakenConfig,
    running: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    messages_received: AtomicU64,
    state: ArcSwap<ConnectionState>,
    /// Price windows by asset
    price_windows: Arc<parking_lot::RwLock<HashMap<CryptoAsset, PriceWindow>>>,
}

impl KrakenClient {
    /// Create new Kraken client
    pub fn new(config: KrakenConfig) -> Self {
        let mut windows = HashMap::new();
        windows.insert(
            CryptoAsset::BTC,
            PriceWindow::new(CryptoAsset::BTC, config.window_size, config.window_max_age_ms),
        );
        windows.insert(
            CryptoAsset::ETH,
            PriceWindow::new(CryptoAsset::ETH, config.window_size, config.window_max_age_ms),
        );

        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            connected: Arc::new(AtomicBool::new(false)),
            messages_received: AtomicU64::new(0),
            state: ArcSwap::from_pointee(ConnectionState::Disconnected),
            price_windows: Arc::new(parking_lot::RwLock::new(windows)),
        }
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// Get current momentum for an asset
    pub fn get_momentum(&self, asset: CryptoAsset) -> Option<PriceMomentum> {
        let windows = self.price_windows.read();
        windows.get(&asset)?.calculate_momentum()
    }

    /// Get all current momenta
    pub fn get_all_momenta(&self) -> Vec<PriceMomentum> {
        let windows = self.price_windows.read();
        windows.values().filter_map(|w| w.calculate_momentum()).collect()
    }

    /// Get messages received count
    pub fn messages_received(&self) -> u64 {
        self.messages_received.load(Ordering::Relaxed)
    }

    /// Start the client
    pub async fn connect(&self) -> Result<(), KrakenError> {
        self.running.store(true, Ordering::SeqCst);
        self.state.store(Arc::new(ConnectionState::Connecting));

        info!(url = %self.config.ws_url, "Connecting to Kraken WebSocket...");

        loop {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            match self.connect_and_run().await {
                Ok(()) => {
                    info!("Kraken WebSocket disconnected normally");
                    break;
                }
                Err(e) => {
                    error!(error = %e, "Kraken WebSocket error");
                    self.connected.store(false, Ordering::SeqCst);
                    self.state.store(Arc::new(ConnectionState::Reconnecting));

                    if self.running.load(Ordering::SeqCst) {
                        warn!(delay_ms = self.config.reconnect_delay_ms, "Reconnecting to Kraken...");
                        tokio::time::sleep(Duration::from_millis(self.config.reconnect_delay_ms)).await;
                    }
                }
            }
        }

        self.connected.store(false, Ordering::SeqCst);
        self.state.store(Arc::new(ConnectionState::Disconnected));
        Ok(())
    }

    /// Stop the client
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    async fn connect_and_run(&self) -> Result<(), KrakenError> {
        let (ws_stream, _) = connect_async(&self.config.ws_url)
            .await
            .map_err(|e| KrakenError::Connection(e.to_string()))?;

        let (mut write, mut read) = ws_stream.split();

        // Subscribe to trades
        let subscribe_msg = serde_json::json!({
            "event": "subscribe",
            "pair": self.config.pairs,
            "subscription": {
                "name": "trade"
            }
        });

        write
            .send(Message::Text(subscribe_msg.to_string()))
            .await
            .map_err(|e| KrakenError::Connection(e.to_string()))?;

        info!(pairs = ?self.config.pairs, "Subscribed to Kraken trades");

        self.connected.store(true, Ordering::SeqCst);
        self.state.store(Arc::new(ConnectionState::Connected));
        info!("Kraken WebSocket connected");

        while let Some(msg_result) = read.next().await {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            match msg_result {
                Ok(Message::Text(text)) => {
                    self.messages_received.fetch_add(1, Ordering::Relaxed);
                    self.handle_message(&text);
                }
                Ok(Message::Ping(data)) => {
                    let _ = write.send(Message::Pong(data)).await;
                }
                Ok(Message::Close(_)) => {
                    info!("Kraken sent close frame");
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    return Err(KrakenError::Connection(e.to_string()));
                }
            }
        }

        Ok(())
    }

    fn handle_message(&self, text: &str) {
        let received_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        // Kraken sends different message types
        // Trade messages are arrays: [channelID, [[price, volume, time, side, orderType, misc], ...], "trade", "XBT/USD"]
        // System messages are objects: {"event": "...", ...}

        // Skip system/event messages (JSON objects)
        if text.starts_with('{') {
            // Parse to check for errors
            if let Ok(obj) = serde_json::from_str::<serde_json::Value>(text) {
                if let Some(event) = obj.get("event").and_then(|e| e.as_str()) {
                    match event {
                        "systemStatus" => {
                            info!("Kraken system status: {}", text);
                        }
                        "subscriptionStatus" => {
                            if let Some(status) = obj.get("status").and_then(|s| s.as_str()) {
                                if status == "error" {
                                    warn!("Kraken subscription error: {}", text);
                                } else {
                                    info!("Kraken subscription {}: {}", status, text);
                                }
                            }
                        }
                        "heartbeat" => {}
                        _ => {
                            debug!(event = %event, "Kraken event");
                        }
                    }
                }
            }
            return;
        }

        // Parse trade array
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(text) {
            // Format: [channelID, [[price, volume, time, side, orderType, misc], ...], "trade", "XBT/USD"]
            if arr.len() >= 4 {
                let msg_type = arr.get(2).and_then(|v| v.as_str());
                let pair = arr.get(3).and_then(|v| v.as_str());

                debug!(msg_type = ?msg_type, pair = ?pair, arr_len = arr.len(), "Kraken array message");

                if msg_type == Some("trade") {
                    if let Some(trades) = arr.get(1).and_then(|v| v.as_array()) {
                        info!(pair = ?pair, trade_count = trades.len(), "Kraken trades received");
                        for trade in trades {
                            if let Some(tick) = self.parse_trade(trade, pair, received_ns) {
                                info!(asset = ?tick.asset, price = %tick.price, qty = %tick.quantity, "Kraken tick parsed");
                                let mut windows = self.price_windows.write();
                                if let Some(window) = windows.get_mut(&tick.asset) {
                                    window.add_tick(tick);
                                }
                            } else {
                                warn!(trade = ?trade, "Failed to parse Kraken trade");
                            }
                        }
                    }
                }
            } else {
                debug!(arr_len = arr.len(), "Kraken short array: {}", text);
            }
        } else {
            warn!("Kraken unknown message format: {}", &text[..text.len().min(200)]);
        }
    }

    fn parse_trade(
        &self,
        trade: &serde_json::Value,
        pair: Option<&str>,
        received_ns: u64,
    ) -> Option<BinanceTick> {
        // Trade format: [price, volume, time, side, orderType, misc]
        let arr = trade.as_array()?;
        if arr.len() < 3 {
            return None;
        }

        let price_str = arr[0].as_str()?;
        let volume_str = arr[1].as_str()?;

        let price = price_str.parse::<rust_decimal::Decimal>().ok()?;
        let quantity = volume_str.parse::<rust_decimal::Decimal>().ok()?;

        let asset = match pair? {
            "XBT/USD" => CryptoAsset::BTC,
            "ETH/USD" => CryptoAsset::ETH,
            _ => return None,
        };

        // Kraken trade format includes side at index 3: "b" = buy, "s" = sell
        let is_sell = arr.get(3).and_then(|v| v.as_str()) == Some("s");

        Some(BinanceTick {
            asset,
            price,
            quantity,
            trade_time_ms: received_ns / 1_000_000,
            received_ns,
            is_sell,
        })
    }
}

/// Kraken errors
#[derive(Debug, thiserror::Error)]
pub enum KrakenError {
    #[error("Connection error: {0}")]
    Connection(String),
    #[error("Parse error: {0}")]
    Parse(String),
}
