//! Crypto Latency HFT - Exploits price delay between Binance and Polymarket
//!
//! Targets 15-minute ETH/BTC up/down markets, executing directional trades
//! when Binance price moves before Polymarket orderbook catches up.

use anyhow::Result;
use hft_binance::{BinanceConfig, CoinbaseConfig, CryptoAsset, PriceFeed};
use hft_core::{CryptoLatencyConfig, MarketOrderbook, TradingState};
use hft_discovery::{CryptoMarketDiscovery, DiscoveryConfig};
use hft_executor::PositionRedeemer;
use hft_signal::{SignalConfig, SignalDetector};
use hft_simulator::{ExitConfig, PaperTradeSimulator, SimulatorConfig};
use hft_websocket::{WebSocketClient, WebSocketConfig};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Crypto latency trading application
pub struct CryptoLatencyApp {
    /// Trading configuration
    config: CryptoLatencyConfig,
    /// Price feed (Binance or Coinbase)
    price_feed: Arc<PriceFeed>,
    /// Polymarket WebSocket client
    polymarket: Arc<WebSocketClient>,
    /// Market discovery
    discovery: Arc<CryptoMarketDiscovery>,
    /// Signal detector
    detector: Arc<SignalDetector>,
    /// Paper trade simulator
    simulator: Arc<PaperTradeSimulator>,
    /// Exit configuration
    exit_config: ExitConfig,
    /// Trading state
    state: Arc<RwLock<TradingState>>,
    /// Running flag
    running: Arc<AtomicBool>,
    /// Market orderbooks (market_id -> orderbook)
    orderbooks: Arc<RwLock<HashMap<String, Arc<MarketOrderbook>>>>,
    /// Statistics
    signals_detected: AtomicU64,
    trades_executed: AtomicU64,
}

impl CryptoLatencyApp {
    /// Create new application with pre-configured clients
    pub fn new_with_clients(
        config: CryptoLatencyConfig,
        signal_config: SignalConfig,
        simulator_config: SimulatorConfig,
        discovery_config: DiscoveryConfig,
        price_feed: Arc<PriceFeed>,
        polymarket: Arc<WebSocketClient>,
    ) -> Self {
        let exit_config = ExitConfig {
            take_profit_bps: config.take_profit_bps,
            stop_loss_bps: config.stop_loss_bps,
            max_hold_secs: config.max_hold_secs,
        };

        Self {
            config,
            price_feed,
            polymarket,
            discovery: Arc::new(CryptoMarketDiscovery::new(discovery_config)),
            detector: Arc::new(SignalDetector::new(signal_config)),
            simulator: Arc::new(PaperTradeSimulator::new(simulator_config)),
            exit_config,
            state: Arc::new(RwLock::new(TradingState::Stopped)),
            running: Arc::new(AtomicBool::new(false)),
            orderbooks: Arc::new(RwLock::new(HashMap::new())),
            signals_detected: AtomicU64::new(0),
            trades_executed: AtomicU64::new(0),
        }
    }

    /// Create new application with Binance (convenience method)
    pub fn new_with_binance(
        config: CryptoLatencyConfig,
        signal_config: SignalConfig,
        simulator_config: SimulatorConfig,
        discovery_config: DiscoveryConfig,
        binance_config: BinanceConfig,
        polymarket_config: WebSocketConfig,
    ) -> Self {
        Self::new_with_clients(
            config,
            signal_config,
            simulator_config,
            discovery_config,
            Arc::new(PriceFeed::binance(binance_config)),
            Arc::new(WebSocketClient::new(polymarket_config)),
        )
    }

    /// Create new application with Coinbase (for US testing)
    pub fn new_with_coinbase(
        config: CryptoLatencyConfig,
        signal_config: SignalConfig,
        simulator_config: SimulatorConfig,
        discovery_config: DiscoveryConfig,
        coinbase_config: CoinbaseConfig,
        polymarket_config: WebSocketConfig,
    ) -> Self {
        Self::new_with_clients(
            config,
            signal_config,
            simulator_config,
            discovery_config,
            Arc::new(PriceFeed::coinbase(coinbase_config)),
            Arc::new(WebSocketClient::new(polymarket_config)),
        )
    }

    /// Get trading state
    pub async fn state(&self) -> TradingState {
        *self.state.read().await
    }

    /// Set trading state
    pub async fn set_state(&self, state: TradingState) {
        *self.state.write().await = state;
    }

    /// Get simulator for stats access
    pub fn simulator(&self) -> &Arc<PaperTradeSimulator> {
        &self.simulator
    }

    /// Get signal detector for stats access
    pub fn detector(&self) -> &Arc<SignalDetector> {
        &self.detector
    }

    /// Start the trading application
    pub async fn start(&self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        self.set_state(TradingState::Starting).await;

        info!("Starting Crypto Latency HFT...");
        info!(
            paper_mode = self.config.paper_mode,
            position_size = %self.config.position_size_usd,
            min_edge_bps = self.config.min_edge_bps,
            "Configuration loaded"
        );

        // Start price feed WebSocket (Binance or Coinbase)
        let price_feed = self.price_feed.clone();
        let feed_running = self.running.clone();
        let feed_handle = tokio::spawn(async move {
            while feed_running.load(Ordering::SeqCst) {
                if let Err(e) = price_feed.connect().await {
                    error!(error = %e, "Price feed connection error");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        });

        // Start Polymarket WebSocket
        let polymarket = self.polymarket.clone();
        let pm_running = self.running.clone();
        let pm_handle = tokio::spawn(async move {
            while pm_running.load(Ordering::SeqCst) {
                if let Err(e) = polymarket.connect().await {
                    error!(error = %e, "Polymarket connection error");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        });

        // Start market discovery loop
        let discovery = self.discovery.clone();
        let discovery_running = self.running.clone();
        let orderbooks = self.orderbooks.clone();
        let ws_client = self.polymarket.clone();
        let discovery_handle = tokio::spawn(async move {
            let refresh_interval = Duration::from_secs(30);

            while discovery_running.load(Ordering::SeqCst) {
                match discovery.discover().await {
                    Ok(markets) => {
                        info!(count = markets.len(), "Discovered crypto markets");

                        // Register new markets
                        for market in markets {
                            let mut books = orderbooks.write().await;
                            if !books.contains_key(&market.market_id.0) {
                                // Create orderbook for this market
                                let orderbook = Arc::new(MarketOrderbook::new(
                                    market.market_id.clone(),
                                    market.up_token_id.clone(),
                                    market.down_token_id.clone(),
                                ));

                                // Register with WebSocket client
                                ws_client.register_market(orderbook.clone()).await;

                                // Subscribe to tokens
                                ws_client
                                    .subscribe(vec![
                                        market.up_token_id.0.clone(),
                                        market.down_token_id.0.clone(),
                                    ])
                                    .await;

                                books.insert(market.market_id.0.clone(), orderbook);

                                info!(
                                    market = %market.market_id,
                                    asset = %market.asset,
                                    end_time = %market.end_time,
                                    "Registered new market"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Market discovery failed");
                    }
                }

                tokio::time::sleep(refresh_interval).await;
            }
        });

        // Start auto-redemption loop (every 60 seconds, only if not paper mode)
        let redemption_running = self.running.clone();
        let paper_mode = self.config.paper_mode;
        let redemption_handle = tokio::spawn(async move {
            if paper_mode {
                info!("Paper mode - skipping auto-redemption");
                return;
            }

            // Get private key for redemption
            let private_key = match std::env::var("POLYMARKET_PRIVATE_KEY")
                .or_else(|_| std::env::var("POLYGON_PRIVATE_KEY"))
            {
                Ok(key) => key,
                Err(_) => {
                    warn!("No private key found, auto-redemption disabled");
                    return;
                }
            };

            let redeemer = match PositionRedeemer::new(&private_key, None) {
                Ok(r) => r,
                Err(e) => {
                    error!(error = %e, "Failed to create redeemer");
                    return;
                }
            };

            info!("Auto-redemption enabled, checking every 60 seconds");

            let mut interval = tokio::time::interval(Duration::from_secs(60));

            while redemption_running.load(Ordering::SeqCst) {
                interval.tick().await;

                match redeemer.check_and_redeem().await {
                    Ok((count, total)) if count > 0 => {
                        info!(
                            count = count,
                            total = format!("${:.2}", total),
                            "Auto-redeemed positions"
                        );
                    }
                    Ok(_) => {
                        debug!("No positions to redeem");
                    }
                    Err(e) => {
                        warn!(error = %e, "Redemption check failed");
                    }
                }
            }
        });

        // Wait for connections
        tokio::time::sleep(Duration::from_secs(2)).await;
        self.set_state(TradingState::Running).await;
        info!("Trading loop active - scanning for signals");

        // Main trading loop
        while self.running.load(Ordering::SeqCst) {
            let state = self.state().await;
            if state != TradingState::Running {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }

            // Get momentum from price feed
            let momenta = self.price_feed.get_all_momenta();

            for momentum in momenta {
                // Get active markets for this asset
                let markets = self.discovery.get_markets_for_asset(match momentum.asset {
                    CryptoAsset::BTC => hft_core::CryptoAsset::BTC,
                    CryptoAsset::ETH => hft_core::CryptoAsset::ETH,
                });

                for market in markets {
                    // Check if market is safe to trade
                    if !market.is_safe_to_trade(self.config.stop_before_resolution_secs as i64) {
                        continue;
                    }

                    // Get orderbook for this market
                    let books = self.orderbooks.read().await;
                    let orderbook = match books.get(&market.market_id.0) {
                        Some(ob) => ob.clone(),
                        None => continue,
                    };
                    drop(books);

                    // Detect signal
                    if let Some(signal) =
                        self.detector.detect_signal(&momentum, &orderbook, &market)
                    {
                        self.signals_detected.fetch_add(1, Ordering::Relaxed);

                        info!(
                            asset = %signal.asset,
                            direction = %signal.direction,
                            edge_bps = signal.edge_bps,
                            confidence = format!("{:.2}", signal.confidence),
                            latency_ms = signal.latency_ms,
                            "Signal detected!"
                        );

                        // Check position limits
                        if self.simulator.open_positions_count() >= self.config.max_positions {
                            debug!("Max positions reached, skipping signal");
                            continue;
                        }

                        // Execute trade (paper or real)
                        if self.config.paper_mode {
                            if let Some(trade) = self.simulator.execute_trade(
                                &signal,
                                &market,
                                self.config.position_size_usd,
                            ) {
                                self.trades_executed.fetch_add(1, Ordering::Relaxed);
                                info!(
                                    trade_id = %trade.trade_id,
                                    entry_price = %trade.entry_price,
                                    size = %trade.size_usd,
                                    "Paper trade executed"
                                );
                            }
                        } else {
                            // TODO: Real order execution
                            warn!("Real trading not yet implemented");
                        }
                    }
                }
            }

            // Check exits for open positions
            if self.config.paper_mode {
                let orderbooks = self.orderbooks.clone();
                self.simulator.check_exits(
                    |trade| {
                        // Get current price from orderbook
                        let books = futures::executor::block_on(orderbooks.read());
                        let ob = books.get(&trade.market_id.0)?;
                        match trade.direction {
                            hft_core::Direction::Up => Some(ob.yes_book.best_bid()),
                            hft_core::Direction::Down => Some(ob.no_book.best_bid()),
                        }
                    },
                    &self.exit_config,
                );
            }

            // Small delay to avoid busy loop (100 microseconds)
            tokio::time::sleep(Duration::from_micros(100)).await;
        }

        // Cleanup
        feed_handle.abort();
        pm_handle.abort();
        discovery_handle.abort();
        redemption_handle.abort();

        self.set_state(TradingState::Stopped).await;
        info!("Crypto Latency HFT stopped");

        Ok(())
    }

    /// Stop the trading application
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.price_feed.stop();
        self.polymarket.stop();
    }

    /// Get statistics
    pub fn stats(&self) -> CryptoLatencyStats {
        let session = self.simulator.stats().get_stats();

        CryptoLatencyStats {
            signals_detected: self.signals_detected.load(Ordering::Relaxed),
            trades_executed: self.trades_executed.load(Ordering::Relaxed),
            open_positions: self.simulator.open_positions_count(),
            total_pnl: session.total_pnl,
            win_rate: session.win_rate,
            profit_factor: session.profit_factor,
            trades_per_hour: session.trades_per_hour,
            avg_edge_bps: session.avg_edge_captured_bps,
            price_feed_connected: self.price_feed.is_connected(),
            price_feed_source: self.price_feed.source_name().to_string(),
            polymarket_connected: self.polymarket.is_connected(),
            active_markets: self.discovery.active_count(),
        }
    }
}

/// Trading statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct CryptoLatencyStats {
    pub signals_detected: u64,
    pub trades_executed: u64,
    pub open_positions: usize,
    pub total_pnl: Decimal,
    pub win_rate: f64,
    pub profit_factor: f64,
    pub trades_per_hour: f64,
    pub avg_edge_bps: f64,
    pub price_feed_connected: bool,
    pub price_feed_source: String,
    pub polymarket_connected: bool,
    pub active_markets: usize,
}

/// Main entry point for crypto latency binary
#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables
    if let Err(_) = dotenvy::dotenv() {
        let _ = dotenvy::from_filename("../../.env");
    }

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("hft=info".parse().unwrap())
                .add_directive("crypto_latency=info".parse().unwrap()),
        )
        .with_target(false)
        .init();

    info!("=================================================");
    info!("  Crypto Latency HFT - 15-Minute Market Scalper");
    info!("=================================================");

    // Load configuration
    let config = CryptoLatencyConfig::default();
    let signal_config = SignalConfig::default();
    let simulator_config = SimulatorConfig::default();
    let discovery_config = DiscoveryConfig::default();

    // Load proxies if available (needed for geo-restricted regions like US)
    // Check multiple locations for the proxy file
    let proxy_file = std::env::var("HFT_PROXY_FILE").unwrap_or_else(|_| {
        // Try parent directory first (when running from rust-hft/)
        if std::path::Path::new("../eu_proxies.txt").exists() {
            "../eu_proxies.txt".to_string()
        } else if std::path::Path::new("eu_proxies.txt").exists() {
            "eu_proxies.txt".to_string()
        } else {
            // Default that will trigger the warning
            "../eu_proxies.txt".to_string()
        }
    });

    // Create price feed - use Coinbase if USE_COINBASE=1 (for US testing)
    // Otherwise use Binance (for production on EU/NL server)
    let use_coinbase = std::env::var("USE_COINBASE")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false);

    let price_feed = if use_coinbase {
        info!("Using Coinbase price feed (works from US for testing)");
        Arc::new(PriceFeed::coinbase(CoinbaseConfig::default()))
    } else {
        info!("Using Binance price feed (works from EU/NL server, blocked in US)");
        Arc::new(PriceFeed::binance(BinanceConfig::default()))
    };

    // Create Polymarket client with proxy support
    let polymarket_client = if std::path::Path::new(&proxy_file).exists() {
        match hft_websocket::ProxyRotator::from_file(&proxy_file) {
            Ok(rotator) => {
                info!(count = rotator.len(), "Loaded proxies for Polymarket");
                Arc::new(WebSocketClient::with_proxy_rotator(
                    WebSocketConfig::default(),
                    rotator,
                ))
            }
            Err(e) => {
                warn!(error = %e, "Failed to load Polymarket proxies");
                Arc::new(WebSocketClient::new(WebSocketConfig::default()))
            }
        }
    } else {
        Arc::new(WebSocketClient::new(WebSocketConfig::default()))
    };

    let app = Arc::new(CryptoLatencyApp::new_with_clients(
        config,
        signal_config,
        simulator_config,
        discovery_config,
        price_feed,
        polymarket_client,
    ));

    // Start stats printer
    let stats_app = app.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let stats = stats_app.stats();
            info!(
                signals = stats.signals_detected,
                trades = stats.trades_executed,
                open = stats.open_positions,
                pnl = %stats.total_pnl,
                win_rate = format!("{:.1}%", stats.win_rate * 100.0),
                profit_factor = format!("{:.2}", stats.profit_factor),
                trades_per_hour = format!("{:.1}", stats.trades_per_hour),
                "Stats update"
            );
        }
    });

    // Handle shutdown
    let shutdown_app = app.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Shutdown signal received");
        shutdown_app.stop();
    });

    // Run the application
    app.start().await?;

    // Print final stats
    let final_stats = app.simulator().stats().get_stats();
    info!("=================================================");
    info!("                 FINAL STATISTICS");
    info!("=================================================");
    info!("Total Trades: {}", final_stats.total_trades);
    info!(
        "Winning: {} | Losing: {}",
        final_stats.winning_trades, final_stats.losing_trades
    );
    info!("Win Rate: {:.1}%", final_stats.win_rate * 100.0);
    info!("Total P&L: ${}", final_stats.total_pnl);
    info!("Profit Factor: {:.2}", final_stats.profit_factor);
    info!(
        "Max Drawdown: ${} ({:.1}%)",
        final_stats.max_drawdown, final_stats.max_drawdown_pct
    );
    info!("Sharpe Ratio: {:.2}", final_stats.sharpe_ratio);
    info!(
        "Avg Edge Captured: {:.1} bps",
        final_stats.avg_edge_captured_bps
    );
    info!("Trades/Hour: {:.1}", final_stats.trades_per_hour);
    info!("=================================================");

    Ok(())
}
