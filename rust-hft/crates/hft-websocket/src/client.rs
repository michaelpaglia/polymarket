//! High-performance WebSocket client for Polymarket with proxy support

use crate::messages::{BookMessage, PriceUpdate, SubscribeRequest, WsMessage};
use crate::parser::{parse_all_books, parse_message};
use arc_swap::ArcSwap;
use base64::Engine;
use flume::{Receiver, Sender};
use futures_util::{SinkExt, StreamExt};
use hft_core::{HftError, HftResult, MarketId, MarketOrderbook, OrderbookState, PriceLevel};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::{client_async, connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

/// Proxy configuration for WebSocket connection
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Proxy host (e.g., "premiumbeu.ahiddenproxy.com")
    pub host: String,
    /// Proxy port
    pub port: u16,
    /// Username for authentication
    pub username: Option<String>,
    /// Password for authentication
    pub password: Option<String>,
}

impl ProxyConfig {
    /// Create new proxy config from host:port:user:pass format
    pub fn from_string(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() >= 2 {
            let host = parts[0].to_string();
            let port = parts[1].parse().ok()?;
            let username = parts.get(2).map(|s| s.to_string());
            let password = parts.get(3).map(|s| s.to_string());
            Some(Self { host, port, username, password })
        } else {
            None
        }
    }

    /// Get proxy URL with auth
    pub fn to_url(&self) -> String {
        match (&self.username, &self.password) {
            (Some(user), Some(pass)) => {
                format!("http://{}:{}@{}:{}", user, pass, self.host, self.port)
            }
            _ => format!("http://{}:{}", self.host, self.port),
        }
    }
}

/// Proxy rotator for cycling through multiple proxies
#[derive(Debug)]
pub struct ProxyRotator {
    proxies: Vec<ProxyConfig>,
    current_index: std::sync::atomic::AtomicUsize,
}

impl ProxyRotator {
    /// Create new proxy rotator from a list of proxies
    pub fn new(proxies: Vec<ProxyConfig>) -> Self {
        Self {
            proxies,
            current_index: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Load proxies from a file (one per line, format: host:port:user:pass)
    pub fn from_file(path: &str) -> std::io::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let proxies: Vec<ProxyConfig> = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| ProxyConfig::from_string(line.trim()))
            .collect();

        if proxies.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "No valid proxies found in file",
            ));
        }

        info!(count = proxies.len(), path = path, "Loaded proxies from file");
        Ok(Self::new(proxies))
    }

    /// Get the next proxy in rotation
    pub fn next(&self) -> Option<&ProxyConfig> {
        if self.proxies.is_empty() {
            return None;
        }
        let idx = self.current_index.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Some(&self.proxies[idx % self.proxies.len()])
    }

    /// Get current proxy without advancing
    pub fn current(&self) -> Option<&ProxyConfig> {
        if self.proxies.is_empty() {
            return None;
        }
        let idx = self.current_index.load(std::sync::atomic::Ordering::Relaxed);
        Some(&self.proxies[idx % self.proxies.len()])
    }

    /// Number of proxies available
    pub fn len(&self) -> usize {
        self.proxies.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.proxies.is_empty()
    }
}

/// Establish HTTP CONNECT tunnel through proxy
async fn connect_via_proxy(
    proxy: &ProxyConfig,
    target_host: &str,
    target_port: u16,
) -> Result<TcpStream, HftError> {
    // Connect to proxy
    let proxy_addr = format!("{}:{}", proxy.host, proxy.port);
    info!(proxy = %proxy_addr, target = %target_host, "Connecting via proxy");

    let stream = TcpStream::connect(&proxy_addr)
        .await
        .map_err(|e| HftError::WebSocketConnection(format!("Proxy connection failed: {}", e)))?;

    // Build CONNECT request
    let mut connect_request = format!(
        "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\n",
        target_host, target_port, target_host, target_port
    );

    // Add proxy authentication if provided
    if let (Some(user), Some(pass)) = (&proxy.username, &proxy.password) {
        let credentials = format!("{}:{}", user, pass);
        let encoded = base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes());
        connect_request.push_str(&format!("Proxy-Authorization: Basic {}\r\n", encoded));
    }

    connect_request.push_str("\r\n");

    // Send CONNECT request
    let (reader, mut writer) = stream.into_split();
    writer.write_all(connect_request.as_bytes()).await
        .map_err(|e| HftError::WebSocketConnection(format!("Failed to send CONNECT: {}", e)))?;

    // Read response
    let mut buf_reader = BufReader::new(reader);
    let mut response_line = String::new();
    buf_reader.read_line(&mut response_line).await
        .map_err(|e| HftError::WebSocketConnection(format!("Failed to read proxy response: {}", e)))?;

    // Check for 200 OK
    if !response_line.contains(" 200 ") && !response_line.starts_with("HTTP/1.1 200") && !response_line.starts_with("HTTP/1.0 200") {
        return Err(HftError::WebSocketConnection(format!(
            "Proxy CONNECT failed: {}", response_line.trim()
        )));
    }

    // Read remaining headers until empty line
    loop {
        let mut header = String::new();
        buf_reader.read_line(&mut header).await
            .map_err(|e| HftError::WebSocketConnection(format!("Failed to read proxy headers: {}", e)))?;
        if header.trim().is_empty() {
            break;
        }
    }

    // Reunite the stream
    let stream = buf_reader.into_inner().reunite(writer)
        .map_err(|e| HftError::WebSocketConnection(format!("Failed to reunite stream: {}", e)))?;

    info!(proxy = %proxy_addr, "Proxy tunnel established");
    Ok(stream)
}

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
    /// Optional proxy configuration
    pub proxy: Option<ProxyConfig>,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            url: "wss://ws-subscriptions-clob.polymarket.com/ws/market".to_string(),
            ping_interval_ms: 10_000,
            reconnect_delay_ms: 1_000,
            max_reconnect_attempts: 0, // Infinite
            proxy: None,
        }
    }
}

impl WebSocketConfig {
    /// Create config with proxy from string (host:port:user:pass)
    pub fn with_proxy(mut self, proxy_str: &str) -> Self {
        self.proxy = ProxyConfig::from_string(proxy_str);
        self
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
    /// Optional proxy rotator for rotating through proxies on reconnect
    proxy_rotator: Option<Arc<ProxyRotator>>,
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
            proxy_rotator: None,
        }
    }

    /// Create new WebSocket client with proxy rotator
    pub fn with_proxy_rotator(config: WebSocketConfig, rotator: ProxyRotator) -> Self {
        let mut client = Self::new(config);
        client.proxy_rotator = Some(Arc::new(rotator));
        client
    }

    /// Set proxy rotator (consumes and returns self for builder pattern)
    pub fn set_proxy_rotator(mut self, rotator: ProxyRotator) -> Self {
        self.proxy_rotator = Some(Arc::new(rotator));
        self
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

            // Get next proxy from rotator (if available) on each connection attempt
            let current_proxy = if let Some(ref rotator) = self.proxy_rotator {
                let proxy = rotator.next();
                if let Some(p) = proxy {
                    info!(
                        proxy_host = %p.host,
                        proxy_port = p.port,
                        "Using proxy for connection"
                    );
                }
                proxy.cloned()
            } else {
                self.config.proxy.clone()
            };

            match self.run_connection_with_proxy(current_proxy.as_ref()).await {
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

                    // Shorter delay if we have proxies to rotate through
                    let base_delay = if self.proxy_rotator.is_some() {
                        500 // Faster rotation through proxies
                    } else {
                        self.config.reconnect_delay_ms
                    };
                    let delay = base_delay * reconnect_attempts.min(10) as u64;
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

    /// Run a single connection with optional proxy
    async fn run_connection_with_proxy(&self, proxy: Option<&ProxyConfig>) -> HftResult<()> {
        info!(url = %self.config.url, proxy = proxy.is_some(), "Connecting to WebSocket...");

        if let Some(proxy_config) = proxy {
            // Connect via HTTP CONNECT proxy
            self.connect_via_proxy_impl(proxy_config).await
        } else {
            // Direct connection (no proxy)
            self.connect_direct_impl().await
        }
    }

    /// Connect directly without proxy
    async fn connect_direct_impl(&self) -> HftResult<()> {
        let (ws_stream, _response) = connect_async(&self.config.url)
            .await
            .map_err(|e| HftError::WebSocketConnection(e.to_string()))?;

        info!("WebSocket connected (direct)");
        self.state.store(Arc::new(ConnectionState::Connected));

        let (mut write, mut read) = ws_stream.split();

        // Subscribe to markets
        self.send_subscriptions(&mut write).await?;

        // Run message loop
        self.run_message_loop(&mut read).await
    }

    /// Connect via HTTP CONNECT proxy
    async fn connect_via_proxy_impl(&self, proxy_config: &ProxyConfig) -> HftResult<()> {
        // Parse the WebSocket URL to extract host and port
        let uri: Uri = self.config.url.parse()
            .map_err(|e| HftError::WebSocketConnection(format!("Invalid URL: {}", e)))?;

        let host = uri.host()
            .ok_or_else(|| HftError::WebSocketConnection("URL missing host".to_string()))?;

        let port = uri.port_u16().unwrap_or(if uri.scheme_str() == Some("wss") { 443 } else { 80 });
        let use_tls = uri.scheme_str() == Some("wss");

        // Connect via HTTP CONNECT proxy
        let tcp_stream = connect_via_proxy(proxy_config, host, port).await?;

        if use_tls {
            // Upgrade to TLS
            let tls_connector = tokio_tungstenite::Connector::NativeTls(
                native_tls::TlsConnector::new()
                    .map_err(|e| HftError::WebSocketConnection(format!("TLS error: {}", e)))?
            );

            let request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(&self.config.url)
                .map_err(|e| HftError::WebSocketConnection(format!("Request build error: {}", e)))?;

            let (ws_stream, _) = tokio_tungstenite::client_async_tls_with_config(
                request,
                tcp_stream,
                None,
                Some(tls_connector),
            ).await.map_err(|e| HftError::WebSocketConnection(format!("WebSocket handshake failed: {}", e)))?;

            info!("WebSocket connected via proxy (TLS)");
            self.state.store(Arc::new(ConnectionState::Connected));

            let (mut write, mut read) = ws_stream.split();
            self.send_subscriptions(&mut write).await?;
            self.run_message_loop(&mut read).await
        } else {
            // Plain WebSocket over proxy tunnel
            let request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(&self.config.url)
                .map_err(|e| HftError::WebSocketConnection(format!("Request build error: {}", e)))?;

            let (ws_stream, _) = client_async(request, tcp_stream)
                .await
                .map_err(|e| HftError::WebSocketConnection(format!("WebSocket handshake failed: {}", e)))?;

            info!("WebSocket connected via proxy");
            self.state.store(Arc::new(ConnectionState::Connected));

            let (mut write, mut read) = ws_stream.split();
            self.send_subscriptions(&mut write).await?;
            self.run_message_loop(&mut read).await
        }
    }

    /// Send subscription messages
    async fn send_subscriptions<S>(&self, write: &mut S) -> HftResult<()>
    where
        S: SinkExt<Message> + Unpin,
        S::Error: std::fmt::Display,
    {
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
        Ok(())
    }

    /// Run the message processing loop
    async fn run_message_loop<S>(&self, read: &mut S) -> HftResult<()>
    where
        S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
    {
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
                Ok(Message::Ping(_data)) => {
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
        // First try to parse as array of books (common case)
        let books = parse_all_books(text);
        if !books.is_empty() {
            for book in books {
                self.handle_book_update(book).await;
            }
            return;
        }

        // Fall back to single message parsing
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
