//! High-performance WebSocket client for Polymarket

use crate::messages::{BookMessage, PriceUpdate, SubscribeRequest, WsMessage};
use crate::parser::parse_message;
use arc_swap::ArcSwap;
use flume::{Receiver, Sender};
use futures_util::{SinkExt, StreamExt};
use hft_core::{HftError, HftResult, MarketId, MarketOrderbook, OrderbookState, PriceLevel, TokenId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

/// WebSocket configuration
#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    /// WebSocket URL
    pub url: String,
    /// Ping interval in milliseconds
    pub ping_interval_ms: u64,
    /// Reconnect delay in milliseconds
    pub reconnect_delay_ms: u64,
    /// Maximum reconnect attempts (0 = infinite)
    pub max_reconnect_attempts: u32,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            url: "wss://ws-subscriptions-clob.polymarket.com/ws/market".to_string(),
            ping_interval_ms: 10_000,
            reconnect_delay_ms: 1_000,
            max_reconnect_attempts: 0, // Infinite
        }
    }
}

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

/// WebSocket client for real-time market data
pub struct WebSocketClient {
    config: WebSocketConfig,
    /// Connection state
    state: Arc<ArcSwap<ConnectionState>>,
    /// Running flag
    running: Arc<AtomicBool>,
    /// Market orderbooks (updated on price changes)
    markets: Arc<RwLock<HashMap<String, Arc<MarketOrderbook>>>>,
    /// Token ID to market mapping
    token_to_market: Arc<RwLock<HashMap<String, MarketId>>>,
    /// Subscribed asset IDs
    subscriptions: Arc<RwLock<Vec<String>>>,
    /// Message channel for external consumers
    message_tx: Sender<PriceUpdate>,
    message_rx: Receiver<PriceUpdate>,
    /// Statistics
    messages_received: AtomicU64,
    reconnect_count: AtomicU64,
}

impl WebSocketClient {
    /// Create new WebSocket client
    pub fn new(config: WebSocketConfig) -> Self {
        let (message_tx, message_rx) = flume::bounded(10000);

        Self {
            config,
            state: Arc::new(ArcSwap::from_pointee(ConnectionState::Disconnected)),
            running: Arc::new(AtomicBool::new(false)),
            markets: Arc::new(RwLock::new(HashMap::new())),
            token_to_market: Arc::new(RwLock::new(HashMap::new())),
            subscriptions: Arc::new(RwLock::new(Vec::new())),
            message_tx,
            message_rx,
            messages_received: AtomicU64::new(0),
            reconnect_count: AtomicU64::new(0),
        }
    }

    /// Register a market for monitoring
    pub async fn register_market(&self, market: Arc<MarketOrderbook>) {
        let mut markets = self.markets.write().await;
        let mut token_map = self.token_to_market.write().await;

        // Map token IDs to market
        token_map.insert(market.yes_token_id.0.clone(), market.market_id.clone());
        token_map.insert(market.no_token_id.0.clone(), market.market_id.clone());

        markets.insert(market.market_id.0.clone(), market);
    }

    /// Subscribe to asset IDs
    pub async fn subscribe(&self, asset_ids: Vec<String>) {
        let mut subs = self.subscriptions.write().await;
        subs.extend(asset_ids);
    }

    /// Get message receiver for external consumers
    pub fn message_receiver(&self) -> Receiver<PriceUpdate> {
        self.message_rx.clone()
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

    /// Get reconnect count
    pub fn reconnect_count(&self) -> u64 {
        self.reconnect_count.load(Ordering::Relaxed)
    }

    /// Start WebSocket connection (blocking)
    pub async fn connect(&self) -> HftResult<()> {
        self.running.store(true, Ordering::SeqCst);

        let mut reconnect_attempts = 0;

        while self.running.load(Ordering::SeqCst) {
            self.state.store(Arc::new(ConnectionState::Connecting));

            match self.run_connection().await {
                Ok(_) => {
                    // Clean disconnect
                    info!("WebSocket disconnected cleanly");
                    break;
                }
                Err(e) => {
                    error!(error = %e, "WebSocket error");

                    if !self.running.load(Ordering::SeqCst) {
                        break;
                    }

                    reconnect_attempts += 1;
                    self.reconnect_count.fetch_add(1, Ordering::Relaxed);

                    if self.config.max_reconnect_attempts > 0
                        && reconnect_attempts >= self.config.max_reconnect_attempts
                    {
                        error!("Max reconnect attempts reached");
                        return Err(e);
                    }

                    self.state.store(Arc::new(ConnectionState::Reconnecting));

                    let delay = self.config.reconnect_delay_ms * reconnect_attempts.min(10) as u64;
                    warn!(delay_ms = delay, attempt = reconnect_attempts, "Reconnecting...");
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
    async fn run_connection(&self) -> HftResult<()> {
        info!(url = %self.config.url, "Connecting to WebSocket...");

        let (ws_stream, _response) = connect_async(&self.config.url)
            .await
            .map_err(|e| HftError::WebSocketConnection(e.to_string()))?;

        info!("WebSocket connected");
        self.state.store(Arc::new(ConnectionState::Connected));

        let (mut write, mut read) = ws_stream.split();

        // Subscribe to markets
        let subs = self.subscriptions.read().await;
        if !subs.is_empty() {
            let subscribe_msg = SubscribeRequest::market(subs.clone());
            let msg_json = serde_json::to_string(&subscribe_msg)
                .map_err(|e| HftError::Internal(e.to_string()))?;

            write
                .send(Message::Text(msg_json))
                .await
                .map_err(|e| HftError::WebSocketConnection(e.to_string()))?;

            info!(count = subs.len(), "Subscribed to assets");
        }
        drop(subs);

        // Spawn ping task
        let running = self.running.clone();
        let ping_interval = self.config.ping_interval_ms;
        let ping_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(ping_interval));
            while running.load(Ordering::SeqCst) {
                interval.tick().await;
                // Ping is handled by tungstenite automatically
            }
        });

        // Message processing loop
        while let Some(msg_result) = read.next().await {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            match msg_result {
                Ok(Message::Text(text)) => {
                    self.messages_received.fetch_add(1, Ordering::Relaxed);
                    self.handle_message(&text).await;
                }
                Ok(Message::Ping(data)) => {
                    // Pong is sent automatically by tungstenite
                    debug!("Received ping");
                }
                Ok(Message::Close(frame)) => {
                    info!(frame = ?frame, "Received close frame");
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    error!(error = %e, "WebSocket read error");
                    ping_handle.abort();
                    return Err(HftError::WebSocketConnection(e.to_string()));
                }
            }
        }

        ping_handle.abort();
        Ok(())
    }

    /// Handle incoming message
    async fn handle_message(&self, text: &str) {
        match parse_message(text) {
            Ok(WsMessage::Book(book)) => {
                self.handle_book_update(book).await;
            }
            Ok(WsMessage::PriceChange(change)) => {
                debug!(
                    asset_id = %change.asset_id,
                    price = %change.price,
                    side = %change.side,
                    "Price change"
                );
            }
            Ok(WsMessage::LastTrade(trade)) => {
                debug!(
                    asset_id = %trade.asset_id,
                    price = %trade.price,
                    "Last trade"
                );
            }
            Ok(WsMessage::TickSize(_)) => {}
            Ok(WsMessage::Unknown(_)) => {}
            Err(e) => {
                warn!(error = %e, "Failed to parse message");
            }
        }
    }

    /// Handle book update message
    async fn handle_book_update(&self, book: BookMessage) {
        // Create price update
        let price_update = PriceUpdate::from_book(&book);

        // Send to consumers
        if let Err(e) = self.message_tx.try_send(price_update.clone()) {
            warn!(error = %e, "Failed to send price update (channel full?)");
        }

        // Update orderbook
        let token_map = self.token_to_market.read().await;
        if let Some(market_id) = token_map.get(&book.asset_id) {
            let markets = self.markets.read().await;
            if let Some(market) = markets.get(&market_id.0) {
                // Convert book levels to PriceLevels
                let bids: Vec<PriceLevel> = book
                    .bids
                    .iter()
                    .filter_map(|l| l.to_price_level())
                    .collect();
                let asks: Vec<PriceLevel> = book
                    .asks
                    .iter()
                    .filter_map(|l| l.to_price_level())
                    .collect();

                let state = OrderbookState {
                    bids,
                    asks,
                    timestamp_ns: price_update.timestamp_ns,
                };

                // Update the appropriate side
                if book.asset_id == market.yes_token_id.0 {
                    market.yes_book.update(state);
                } else if book.asset_id == market.no_token_id.0 {
                    market.no_book.update(state);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_websocket_config_default() {
        let config = WebSocketConfig::default();
        assert!(config.url.contains("polymarket.com"));
        assert!(config.ping_interval_ms > 0);
    }
}
