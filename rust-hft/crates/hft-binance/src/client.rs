//! Binance WebSocket client implementation

use crate::types::*;
use arc_swap::ArcSwap;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

/// Proxy configuration
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ProxyConfig {
    /// Parse from "host:port:user:pass" format
    pub fn from_string(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() >= 2 {
            Some(Self {
                host: parts[0].to_string(),
                port: parts[1].parse().ok()?,
                username: parts.get(2).map(|s| s.to_string()),
                password: parts.get(3).map(|s| s.to_string()),
            })
        } else {
            None
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
    pub fn new(proxies: Vec<ProxyConfig>) -> Self {
        Self {
            proxies,
            current_index: std::sync::atomic::AtomicUsize::new(0),
        }
    }

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
                "No valid proxies found",
            ));
        }

        info!(count = proxies.len(), path = path, "Loaded Binance proxies");
        Ok(Self::new(proxies))
    }

    pub fn next(&self) -> Option<&ProxyConfig> {
        if self.proxies.is_empty() {
            return None;
        }
        let idx = self
            .current_index
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Some(&self.proxies[idx % self.proxies.len()])
    }

    pub fn len(&self) -> usize {
        self.proxies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.proxies.is_empty()
    }
}

/// Binance WebSocket client
pub struct BinanceClient {
    config: BinanceConfig,
    /// Connection state
    state: Arc<ArcSwap<ConnectionState>>,
    /// Running flag
    running: Arc<AtomicBool>,
    /// Price windows per asset
    price_windows: Arc<HashMap<CryptoAsset, PriceWindow>>,
    /// Statistics
    messages_received: AtomicU64,
    reconnect_count: AtomicU64,
    /// Optional proxy rotator
    proxy_rotator: Option<Arc<ProxyRotator>>,
}

impl BinanceClient {
    /// Create new Binance client
    pub fn new(config: BinanceConfig) -> Self {
        let mut windows = HashMap::new();

        // Create price windows for BTC and ETH
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
            proxy_rotator: None,
        }
    }

    /// Create with proxy rotator for geo-restricted regions
    pub fn with_proxy_rotator(config: BinanceConfig, rotator: ProxyRotator) -> Self {
        let mut client = Self::new(config);
        client.proxy_rotator = Some(Arc::new(rotator));
        client
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

    /// Get latest price for asset (fast, lock-free)
    pub fn latest_price(&self, asset: CryptoAsset) -> Option<rust_decimal::Decimal> {
        self.price_windows.get(&asset)?.latest_price()
    }

    /// Get momentum for asset
    pub fn get_momentum(&self, asset: CryptoAsset) -> Option<PriceMomentum> {
        self.price_windows.get(&asset)?.calculate_momentum()
    }

    /// Get all momenta (BTC and ETH)
    pub fn get_all_momenta(&self) -> Vec<PriceMomentum> {
        [CryptoAsset::BTC, CryptoAsset::ETH]
            .iter()
            .filter_map(|&asset| self.get_momentum(asset))
            .collect()
    }

    /// Start WebSocket connection (blocking)
    pub async fn connect(&self) -> Result<(), BinanceError> {
        self.running.store(true, Ordering::SeqCst);

        let mut reconnect_attempts = 0;

        while self.running.load(Ordering::SeqCst) {
            self.state.store(Arc::new(ConnectionState::Connecting));

            // Get next proxy if available
            let proxy = if let Some(ref rotator) = self.proxy_rotator {
                let p = rotator.next();
                if let Some(proxy) = p {
                    info!(host = %proxy.host, port = proxy.port, "Using proxy for Binance");
                }
                p.cloned()
            } else {
                None
            };

            let result = if let Some(ref proxy_config) = proxy {
                self.run_connection_via_proxy(proxy_config).await
            } else {
                self.run_connection().await
            };

            match result {
                Ok(_) => {
                    info!("Binance WebSocket disconnected cleanly");
                    break;
                }
                Err(e) => {
                    error!(error = %e, "Binance WebSocket error");

                    if !self.running.load(Ordering::SeqCst) {
                        break;
                    }

                    reconnect_attempts += 1;
                    self.reconnect_count.fetch_add(1, Ordering::Relaxed);

                    self.state.store(Arc::new(ConnectionState::Reconnecting));

                    // Shorter delay if using proxies
                    let base_delay = if self.proxy_rotator.is_some() {
                        500
                    } else {
                        self.config.reconnect_delay_ms
                    };
                    let delay = base_delay * reconnect_attempts.min(10);
                    warn!(
                        delay_ms = delay,
                        attempt = reconnect_attempts,
                        "Reconnecting to Binance..."
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

    /// Run connection via HTTP CONNECT proxy
    async fn run_connection_via_proxy(&self, proxy: &ProxyConfig) -> Result<(), BinanceError> {
        let streams = self.config.streams.join("/");
        let url = format!("{}/{}", self.config.ws_url, streams);

        info!(url = %url, proxy_host = %proxy.host, "Connecting to Binance via proxy...");

        // Parse URL
        let uri: Uri = url
            .parse()
            .map_err(|e| BinanceError::Connection(format!("Invalid URL: {}", e)))?;

        let host = uri
            .host()
            .ok_or_else(|| BinanceError::Connection("URL missing host".to_string()))?;
        let port = uri
            .port_u16()
            .unwrap_or(if uri.scheme_str() == Some("wss") {
                443
            } else {
                80
            });

        // Connect to proxy
        let proxy_addr = format!("{}:{}", proxy.host, proxy.port);
        let stream = TcpStream::connect(&proxy_addr)
            .await
            .map_err(|e| BinanceError::Connection(format!("Proxy connection failed: {}", e)))?;

        // Build CONNECT request
        let mut connect_request = format!(
            "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\n",
            host, port, host, port
        );

        if let (Some(user), Some(pass)) = (&proxy.username, &proxy.password) {
            use base64::Engine;
            let credentials = format!("{}:{}", user, pass);
            let encoded = base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes());
            connect_request.push_str(&format!("Proxy-Authorization: Basic {}\r\n", encoded));
        }
        connect_request.push_str("\r\n");

        // Send CONNECT
        let (reader, mut writer) = stream.into_split();
        writer
            .write_all(connect_request.as_bytes())
            .await
            .map_err(|e| BinanceError::Connection(format!("Failed to send CONNECT: {}", e)))?;

        // Read response
        let mut buf_reader = BufReader::new(reader);
        let mut response_line = String::new();
        buf_reader
            .read_line(&mut response_line)
            .await
            .map_err(|e| {
                BinanceError::Connection(format!("Failed to read proxy response: {}", e))
            })?;

        if !response_line.contains(" 200 ") {
            return Err(BinanceError::Connection(format!(
                "Proxy CONNECT failed: {}",
                response_line.trim()
            )));
        }

        // Read remaining headers
        loop {
            let mut header = String::new();
            buf_reader.read_line(&mut header).await.map_err(|e| {
                BinanceError::Connection(format!("Failed to read proxy headers: {}", e))
            })?;
            if header.trim().is_empty() {
                break;
            }
        }

        // Reunite stream
        let stream = buf_reader
            .into_inner()
            .reunite(writer)
            .map_err(|e| BinanceError::Connection(format!("Failed to reunite stream: {}", e)))?;

        // Upgrade to TLS
        let tls_connector = tokio_tungstenite::Connector::NativeTls(
            native_tls::TlsConnector::new()
                .map_err(|e| BinanceError::Connection(format!("TLS error: {}", e)))?,
        );

        let request =
            tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(&url)
                .map_err(|e| BinanceError::Connection(format!("Request build error: {}", e)))?;

        let (ws_stream, _) = tokio_tungstenite::client_async_tls_with_config(
            request,
            stream,
            None,
            Some(tls_connector),
        )
        .await
        .map_err(|e| BinanceError::Connection(format!("WebSocket handshake failed: {}", e)))?;

        info!("Binance WebSocket connected via proxy");
        self.state.store(Arc::new(ConnectionState::Connected));

        let (mut write, mut read) = ws_stream.split();
        self.run_message_loop(&mut write, &mut read).await
    }

    /// Run a single connection (direct, no proxy)
    async fn run_connection(&self) -> Result<(), BinanceError> {
        // Build combined stream URL
        let streams = self.config.streams.join("/");
        let url = format!("{}/{}", self.config.ws_url, streams);

        info!(url = %url, "Connecting to Binance WebSocket...");

        let (ws_stream, _response) = connect_async(&url)
            .await
            .map_err(|e| BinanceError::Connection(e.to_string()))?;

        info!("Binance WebSocket connected (direct)");
        self.state.store(Arc::new(ConnectionState::Connected));

        let (mut write, mut read) = ws_stream.split();
        self.run_message_loop(&mut write, &mut read).await
    }

    /// Run the message processing loop
    async fn run_message_loop<W, R>(&self, write: &mut W, read: &mut R) -> Result<(), BinanceError>
    where
        W: futures_util::Sink<Message> + Unpin,
        W::Error: std::fmt::Display,
        R: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + Unpin,
    {
        // Message processing loop (ping/pong handled automatically by tokio-tungstenite)
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
                    debug!("Received ping from Binance");
                    if let Err(e) = write.send(Message::Pong(data)).await {
                        warn!(error = %e, "Failed to send pong");
                    }
                }
                Ok(Message::Close(frame)) => {
                    info!(frame = ?frame, "Binance sent close frame");
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    error!(error = %e, "Binance WebSocket read error");
                    return Err(BinanceError::Connection(e.to_string()));
                }
            }
        }

        Ok(())
    }

    /// Handle incoming message
    fn handle_message(&self, text: &str) {
        let received_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time before UNIX epoch")
            .as_nanos() as u64;

        // Try to parse as aggTrade
        match serde_json::from_str::<AggTradeMessage>(text) {
            Ok(msg) => {
                if let Some(tick) = BinanceTick::from_agg_trade(&msg, received_ns) {
                    if let Some(window) = self.price_windows.get(&tick.asset) {
                        window.add_tick(tick);
                    }
                }
            }
            Err(e) => {
                // Check if it's a combined stream wrapper
                if let Ok(wrapper) = serde_json::from_str::<CombinedStreamMessage>(text) {
                    if let Some(tick) = BinanceTick::from_agg_trade(&wrapper.data, received_ns) {
                        if let Some(window) = self.price_windows.get(&tick.asset) {
                            window.add_tick(tick);
                        }
                    }
                } else {
                    debug!(error = %e, "Failed to parse Binance message");
                }
            }
        }
    }
}

/// Combined stream message wrapper
#[derive(Debug, serde::Deserialize)]
struct CombinedStreamMessage {
    #[allow(dead_code)]
    stream: String,
    data: AggTradeMessage,
}

/// Binance client errors
#[derive(Debug, thiserror::Error)]
pub enum BinanceError {
    #[error("Connection error: {0}")]
    Connection(String),
    #[error("Parse error: {0}")]
    Parse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binance_config_default() {
        let config = BinanceConfig::default();
        assert!(config.ws_url.contains("binance.com"));
        assert_eq!(config.streams.len(), 2);
    }

    #[test]
    fn test_parse_agg_trade() {
        let json = r#"{
            "e": "aggTrade",
            "E": 1234567890123,
            "s": "BTCUSDT",
            "a": 123456789,
            "p": "50000.50",
            "q": "1.5",
            "f": 100,
            "l": 101,
            "T": 1234567890123,
            "m": false
        }"#;

        let msg: AggTradeMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.symbol, "BTCUSDT");
        assert_eq!(msg.price, "50000.50");
    }
}
