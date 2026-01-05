//! Crypto Latency HFT - Exploits price delay between Binance and Polymarket
//!
//! Targets 15-minute ETH/BTC up/down markets, executing directional trades
//! when Binance price moves before Polymarket orderbook catches up.

use anyhow::Result;
use hft_binance::{BinanceConfig, CoinbaseConfig, CryptoAsset, KrakenConfig, PriceFeed};
use hft_core::{CryptoLatencyConfig, MarketOrderbook, Side, TradingState};
use hft_discovery::{CryptoMarketDiscovery, DiscoveryConfig};
use hft_executor::{ExecutorConfig, OrderExecutor, PositionRedeemer};
use hft_signal::{SignalConfig, SignalDetector};
use hft_simulator::{ExitConfig, PaperTradeSimulator, SimulatorConfig};
use hft_websocket::{WebSocketClient, WebSocketConfig};
use rust_decimal::prelude::ToPrimitive;
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
    /// Live order executor (None if paper mode)
    executor: Option<Arc<OrderExecutor>>,
    /// Exit configuration
    #[allow(dead_code)]
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
    /// Session statistics
    session_wins: AtomicU64,
    session_losses: AtomicU64,
    session_pnl_cents: std::sync::atomic::AtomicI64, // in cents to avoid float atomics
    /// Last signal time per market (for rate limiting)
    last_signal: Arc<RwLock<HashMap<String, std::time::Instant>>>,
    /// Last status log time per market (for rate limiting)
    last_status_log: Arc<RwLock<HashMap<String, std::time::Instant>>>,
    /// Strike prices by market ID (captured when window starts)
    strike_prices: Arc<RwLock<HashMap<String, Decimal>>>,
    /// Live positions tracking (trade_id -> (token_id, direction, entry_price, size))
    live_positions: Arc<RwLock<HashMap<String, LivePosition>>>,
}

/// A live position for tracking
#[derive(Debug, Clone)]
pub struct LivePosition {
    pub trade_id: String,
    pub market_id: String,
    pub token_id: String,
    pub direction: hft_core::Direction,
    pub entry_price: Decimal,
    pub size_usd: Decimal,
    pub asset: hft_core::CryptoAsset,
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
        executor: Option<Arc<OrderExecutor>>,
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
            executor,
            exit_config,
            state: Arc::new(RwLock::new(TradingState::Stopped)),
            running: Arc::new(AtomicBool::new(false)),
            orderbooks: Arc::new(RwLock::new(HashMap::new())),
            signals_detected: AtomicU64::new(0),
            trades_executed: AtomicU64::new(0),
            session_wins: AtomicU64::new(0),
            session_losses: AtomicU64::new(0),
            session_pnl_cents: std::sync::atomic::AtomicI64::new(0),
            last_signal: Arc::new(RwLock::new(HashMap::new())),
            last_status_log: Arc::new(RwLock::new(HashMap::new())),
            strike_prices: Arc::new(RwLock::new(HashMap::new())),
            live_positions: Arc::new(RwLock::new(HashMap::new())),
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
            None,
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
            None,
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

        // Run INITIAL discovery BEFORE connecting WebSocket
        // This ensures subscriptions are ready when WebSocket connects
        info!("Running initial market discovery...");
        match self.discovery.discover().await {
            Ok(markets) => {
                info!(count = markets.len(), "Initial discovery found markets");
                for market in &markets {
                    let mut books = self.orderbooks.write().await;
                    if !books.contains_key(&market.market_id.0) {
                        let orderbook = Arc::new(MarketOrderbook::new(
                            market.market_id.clone(),
                            market.up_token_id.clone(),
                            market.down_token_id.clone(),
                        ));
                        self.polymarket.register_market(orderbook.clone()).await;
                        info!(
                            up_token = %market.up_token_id.0,
                            down_token = %market.down_token_id.0,
                            market = %market.market_id,
                            "Pre-subscribing to token IDs"
                        );
                        self.polymarket
                            .subscribe(vec![
                                market.up_token_id.0.clone(),
                                market.down_token_id.0.clone(),
                            ])
                            .await;
                        books.insert(market.market_id.0.clone(), orderbook);
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "Initial discovery failed, will retry in loop");
            }
        }

        // NOW start Polymarket WebSocket (subscriptions are ready)
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

        // Start market discovery loop for ongoing updates
        let discovery = self.discovery.clone();
        let discovery_running = self.running.clone();
        let orderbooks = self.orderbooks.clone();
        let ws_client = self.polymarket.clone();
        let discovery_handle = tokio::spawn(async move {
            let refresh_interval = Duration::from_secs(30);
            // Skip first iteration since we already did initial discovery
            tokio::time::sleep(refresh_interval).await;

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

                                // Subscribe to tokens - log the exact IDs
                                // NOTE: This only adds to the list, won't be sent until reconnect
                                info!(
                                    up_token = %market.up_token_id.0,
                                    down_token = %market.down_token_id.0,
                                    "Subscribing to token IDs (pending until reconnect)"
                                );
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
                                    up_token = %market.up_token_id.0,
                                    down_token = %market.down_token_id.0,
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

        // Track for periodic debug logging
        let mut last_debug_log = std::time::Instant::now();

        // Main trading loop
        while self.running.load(Ordering::SeqCst) {
            let state = self.state().await;
            if state != TradingState::Running {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }

            // Get momentum from price feed
            let momenta = self.price_feed.get_all_momenta();

            // STRIKE CAPTURE: The strike is the crypto price at window start
            // For 15M markets, UP wins if end_price >= start_price
            // We capture the strike when we first see the market as started
            for market in self.discovery.get_active_markets() {
                if market.has_started() {
                    let market_id = &market.market_id.0;
                    let strikes = self.strike_prices.read().await;
                    let has_strike = strikes.contains_key(market_id);
                    drop(strikes);

                    if !has_strike {
                        // Get current crypto price from Kraken as proxy for strike
                        // Ideally we'd have the exact price at eventStartTime
                        let asset_type = match market.asset {
                            hft_core::CryptoAsset::BTC => CryptoAsset::BTC,
                            hft_core::CryptoAsset::ETH => CryptoAsset::ETH,
                        };
                        if let Some(price) = momenta
                            .iter()
                            .find(|m| m.asset == asset_type)
                            .map(|m| m.current_price)
                        {
                            let mut strikes = self.strike_prices.write().await;
                            strikes.insert(market_id.clone(), price);
                            let time_left = market.time_to_resolution_secs();
                            info!(
                                market = %market_id,
                                asset = %market.asset,
                                strike = %price,
                                time_left_secs = time_left,
                                "STRIKE CAPTURED: {} @ {} ({}s left in window)",
                                market.asset,
                                price,
                                time_left
                            );
                        }
                    }
                }
            }

            // Trading strategy for active markets
            // Uses discovery prices (which update every 30s) to track momentum
            let all_markets = self.discovery.get_active_markets();
            for market in &all_markets {
                // Only trade active markets (window has started, hasn't ended)
                if !market.is_active_window() {
                    continue;
                }

                // Skip if too close to resolution
                if !market.is_safe_to_trade(30) {
                    continue;
                }

                let market_id = &market.market_id.0;
                let up_price = self.discovery.get_market_up_price(market_id);
                let down_price = self.discovery.get_market_down_price(market_id);
                let time_left = market.time_to_resolution_secs();

                // Rate limit signals - only emit once per 5 seconds per market
                let should_signal = {
                    let signals = self.last_signal.read().await;
                    signals
                        .get(market_id)
                        .map(|last| last.elapsed() > Duration::from_secs(5))
                        .unwrap_or(true)
                };

                if !should_signal {
                    continue;
                }

                // =======================================================
                // MECHANICAL SIGNAL DETECTION - Repeatable every 15 minutes
                // =======================================================
                //
                // Signal Types:
                // 1. TILT: Buy cheap side when market heavily tilted (< 0.40)
                // 2. FLOW: Buy side with strong orderbook momentum (flow > 0.6)
                // 3. COMBO: Both TILT and FLOW agree = higher conviction
                //
                // All signals rate-limited to once per 5 seconds per market
                // =======================================================

                // Get orderbook flow data
                let (yes_flow, no_flow, yes_velocity, no_velocity) = {
                    let books = self.orderbooks.read().await;
                    if let Some(orderbook) = books.get(market_id) {
                        (
                            orderbook.yes_book.flow_imbalance(),
                            orderbook.no_book.flow_imbalance(),
                            orderbook.yes_book.velocity_bps_per_sec(),
                            orderbook.no_book.velocity_bps_per_sec(),
                        )
                    } else {
                        (0.0, 0.0, 0.0, 0.0)
                    }
                };

                // Detect signal conditions
                let tilt_threshold = 0.40;
                let flow_threshold = 0.8; // Increased from 0.6 to reduce noise
                let velocity_threshold = 10.0;

                let tilt_up = up_price < tilt_threshold;
                let tilt_down = down_price < tilt_threshold;
                let flow_up = yes_flow > flow_threshold && yes_velocity > velocity_threshold;
                let flow_down = no_flow > flow_threshold && no_velocity > velocity_threshold;

                // Determine trade direction and signal type
                let (should_trade, direction, signal_type, entry_price, edge_bps) =
                    if tilt_up && flow_up {
                        // COMBO: Both tilt AND flow favor UP
                        let edge = ((0.50 - up_price.min(0.50)) * 10000.0) as u32 + 500; // +500 for combo
                        (true, hft_core::Direction::Up, "COMBO", up_price, edge)
                    } else if tilt_down && flow_down {
                        // COMBO: Both tilt AND flow favor DOWN
                        let edge = ((0.50 - down_price.min(0.50)) * 10000.0) as u32 + 500;
                        (true, hft_core::Direction::Down, "COMBO", down_price, edge)
                    } else if tilt_up {
                        // TILT only: cheap UP side
                        let edge = ((0.50 - up_price.min(0.50)) * 10000.0) as u32;
                        (true, hft_core::Direction::Up, "TILT", up_price, edge)
                    } else if tilt_down {
                        // TILT only: cheap DOWN side
                        let edge = ((0.50 - down_price.min(0.50)) * 10000.0) as u32;
                        (true, hft_core::Direction::Down, "TILT", down_price, edge)
                    } else if flow_up && up_price < 0.55 {
                        // FLOW only: strong UP momentum, price not too expensive
                        let edge = (yes_flow * 1000.0) as u32;
                        (true, hft_core::Direction::Up, "FLOW", up_price, edge)
                    } else if flow_down && down_price < 0.55 {
                        // FLOW only: strong DOWN momentum, price not too expensive
                        let edge = (no_flow * 1000.0) as u32;
                        (true, hft_core::Direction::Down, "FLOW", down_price, edge)
                    } else {
                        (false, hft_core::Direction::Up, "", 0.0, 0)
                    };

                // Rate limit signals by type (COMBO=10s, TILT=30s, FLOW=120s)
                let rate_limit_secs = match signal_type {
                    "COMBO" => 10, // High conviction, allow more frequent
                    "TILT" => 30,  // Moderate conviction
                    "FLOW" => 120, // Low conviction, increased from 60s to reduce noise
                    _ => 30,
                };

                let can_trade = {
                    let signals = self.last_signal.read().await;
                    signals
                        .get(market_id)
                        .map(|t| t.elapsed() > Duration::from_secs(rate_limit_secs))
                        .unwrap_or(true)
                };

                // Execute trade if signal detected and rate limit allows
                if should_trade && can_trade {
                    let direction_str = match direction {
                        hft_core::Direction::Up => "Up",
                        hft_core::Direction::Down => "Down",
                    };
                    let payout_if_wins = 1.0 / entry_price;

                    info!(
                        market = %market_id,
                        asset = %market.asset,
                        signal = signal_type,
                        side = direction_str,
                        entry = format!("{:.3}", entry_price),
                        edge_bps = edge_bps,
                        payout = format!("{:.2}x", payout_if_wins),
                        flow_up = format!("{:.2}", yes_flow),
                        flow_down = format!("{:.2}", no_flow),
                        time_left = time_left,
                        live = !self.config.paper_mode,
                        "SIGNAL: {} {} @ {:.3} (edge={}bps, payout={:.2}x) [{}]",
                        signal_type, direction_str, entry_price, edge_bps, payout_if_wins,
                        if self.config.paper_mode { "PAPER" } else { "LIVE" }
                    );

                    // Get token ID for the direction we want to buy
                    let token_id = match direction {
                        hft_core::Direction::Up => market.up_token_id.0.clone(),
                        hft_core::Direction::Down => market.down_token_id.0.clone(),
                    };

                    let position_size = self.config.position_size_usd;
                    let entry_price_decimal =
                        Decimal::try_from(entry_price).unwrap_or(Decimal::new(50, 2));

                    if self.config.paper_mode {
                        // Paper trading - use simulator
                        let signal = hft_core::LatencySignal {
                            signal_id: uuid::Uuid::new_v4().to_string(),
                            asset: market.asset,
                            direction,
                            binance_price: Decimal::ZERO,
                            price_change_bps: edge_bps as i32,
                            polymarket_price: entry_price_decimal,
                            expected_price: Decimal::try_from(entry_price + 0.02)
                                .unwrap_or(Decimal::new(52, 2)),
                            edge_bps,
                            confidence: if signal_type == "COMBO" { 0.70 } else { 0.55 },
                            detected_at_ns: 0,
                            binance_update_ns: 0,
                            polymarket_update_ns: 0,
                            latency_ms: 0,
                        };

                        if let Some(trade) =
                            self.simulator.execute_trade(&signal, market, position_size)
                        {
                            self.trades_executed.fetch_add(1, Ordering::Relaxed);
                            info!(
                                trade_id = %trade.trade_id,
                                signal = signal_type,
                                "PAPER TRADE: {} {} @ {} ($1 bet via {})",
                                market.asset, direction_str, trade.entry_price, signal_type
                            );

                            // Update last signal time
                            let mut signals = self.last_signal.write().await;
                            signals.insert(market_id.clone(), std::time::Instant::now());
                        }
                    } else if let Some(executor) = &self.executor {
                        // LIVE trading - place real order
                        let trade_id = uuid::Uuid::new_v4().to_string();
                        let shares = position_size / entry_price_decimal;

                        info!(
                            trade_id = %trade_id,
                            token_id = %token_id,
                            side = "BUY",
                            price = %entry_price_decimal,
                            shares = %shares,
                            "LIVE ORDER: Placing order..."
                        );

                        // Create signed order using the executor's signer
                        match executor
                            .create_and_submit_order(
                                &token_id,
                                Side::Buy,
                                entry_price_decimal,
                                shares,
                            )
                            .await
                        {
                            Ok(Some(order_id)) => {
                                self.trades_executed.fetch_add(1, Ordering::Relaxed);
                                info!(
                                    trade_id = %trade_id,
                                    order_id = %order_id,
                                    signal = signal_type,
                                    "LIVE TRADE: {} {} @ {} (${} via {})",
                                    market.asset, direction_str, entry_price_decimal, position_size, signal_type
                                );

                                // Track live position for resolution
                                let live_pos = LivePosition {
                                    trade_id: trade_id.clone(),
                                    market_id: market_id.clone(),
                                    token_id: token_id.clone(),
                                    direction,
                                    entry_price: entry_price_decimal,
                                    size_usd: position_size,
                                    asset: market.asset,
                                };
                                let mut positions = self.live_positions.write().await;
                                positions.insert(trade_id, live_pos);
                            }
                            Ok(None) => {
                                warn!(
                                    trade_id = %trade_id,
                                    "LIVE ORDER: No order ID returned"
                                );
                            }
                            Err(e) => {
                                error!(
                                    trade_id = %trade_id,
                                    error = %e,
                                    "LIVE ORDER FAILED: {}", e
                                );
                            }
                        }

                        // Update last signal time even on failure (avoid rapid retries)
                        let mut signals = self.last_signal.write().await;
                        signals.insert(market_id.clone(), std::time::Instant::now());
                    } else {
                        warn!("Live mode enabled but no executor configured!");
                    }
                }

                // Log orderbook status every 30 seconds (rate limited properly)
                let should_log_status = {
                    let status_logs = self.last_status_log.read().await;
                    status_logs
                        .get(market_id)
                        .map(|t| t.elapsed() > Duration::from_secs(30))
                        .unwrap_or(true)
                };

                if should_log_status {
                    // Update last status log time
                    let mut status_logs = self.last_status_log.write().await;
                    status_logs.insert(market_id.clone(), std::time::Instant::now());
                    drop(status_logs);

                    let books = self.orderbooks.read().await;
                    if let Some(orderbook) = books.get(market_id) {
                        let yes_snapshot = orderbook.yes_book.snapshot();
                        let yes_bids: Vec<String> = yes_snapshot
                            .bids
                            .iter()
                            .take(2)
                            .map(|l| format!("{:.2}", l.price))
                            .collect();
                        let yes_asks: Vec<String> = yes_snapshot
                            .asks
                            .iter()
                            .take(2)
                            .map(|l| format!("{:.2}", l.price))
                            .collect();

                        // Get session stats
                        let wins = self.session_wins.load(Ordering::Relaxed);
                        let losses = self.session_losses.load(Ordering::Relaxed);
                        let pnl_cents = self
                            .session_pnl_cents
                            .load(std::sync::atomic::Ordering::Relaxed);

                        info!(
                            market = %market_id,
                            asset = %market.asset,
                            up_price = format!("{:.3}", up_price),
                            down_price = format!("{:.3}", down_price),
                            yes_bid_ask = format!("{:?}/{:?}", yes_bids, yes_asks),
                            flow_up = format!("{:.2}", yes_flow),
                            flow_down = format!("{:.2}", no_flow),
                            time_left = time_left,
                            session = format!("{}W-{}L ${:.2}", wins, losses, pnl_cents as f64 / 100.0),
                            "STATUS: UP={:.1}% DOWN={:.1}% ({}s left) [{}W-{}L ${:.2}]",
                            up_price * 100.0, down_price * 100.0, time_left, wins, losses, pnl_cents as f64 / 100.0
                        );
                    }
                }

                // End of market processing - signal detection and execution handled above
            }

            // Periodic debug logging every 10 seconds
            if last_debug_log.elapsed() > Duration::from_secs(10) {
                last_debug_log = std::time::Instant::now();
                let active_markets = self.discovery.active_count();

                if momenta.is_empty() {
                    info!("No momentum data - price feed may not be receiving trades");
                } else {
                    for m in &momenta {
                        info!(
                            asset = %m.asset,
                            price = %m.current_price,
                            change_1s_bps = m.change_1s_bps,
                            change_5s_bps = m.change_5s_bps,
                            "Momentum (need ±10bps in aggressive mode)"
                        );
                    }
                }

                // Log discovery prices and momentum for active markets
                for market in &all_markets {
                    if !market.is_active_window() {
                        continue;
                    }
                    let market_id = &market.market_id.0;
                    let up_price = self.discovery.get_market_up_price(market_id);
                    let down_price = self.discovery.get_market_down_price(market_id);
                    let momentum = self.discovery.get_market_momentum(market_id);
                    let velocity = self.discovery.get_market_velocity(market_id);
                    let time_left = market.time_to_resolution_secs();

                    let momentum_str = match momentum {
                        Some((up, bps)) => format!("{} {}bps", if up { "UP" } else { "DN" }, bps),
                        None => "no data".to_string(),
                    };
                    let velocity_str = match velocity {
                        Some(v) => format!("{:.2}/min", v),
                        None => "-".to_string(),
                    };

                    info!(
                        market = %market_id,
                        asset = %market.asset,
                        up_price = format!("{:.3}", up_price),
                        down_price = format!("{:.3}", down_price),
                        momentum = momentum_str,
                        velocity = velocity_str,
                        time_left = time_left,
                        "Market prices"
                    );
                }

                if active_markets == 0 {
                    info!("No active 15-minute crypto markets found");
                } else {
                    info!(count = active_markets, "Active markets");
                }
            }

            for momentum in &momenta {
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
                    if let Some(signal) = self.detector.detect_signal(momentum, &orderbook, &market)
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

            // Check for market resolution and close positions
            // For 15M markets, we hold until resolution and check if crypto went UP or DOWN
            if self.config.paper_mode {
                let open_positions = self.simulator.get_open_positions();
                for trade in open_positions {
                    // Get the market for this trade
                    if let Some(market) = self.discovery.get_market(&trade.market_id.0) {
                        // Check if market has ended (resolved)
                        if market.is_expired() {
                            // Get current crypto price from momentum data
                            let asset_type = match market.asset {
                                hft_core::CryptoAsset::BTC => CryptoAsset::BTC,
                                hft_core::CryptoAsset::ETH => CryptoAsset::ETH,
                            };
                            let crypto_price = momenta
                                .iter()
                                .find(|m| m.asset == asset_type)
                                .map(|m| m.current_price)
                                .unwrap_or(Decimal::ZERO);

                            // Get strike price from our cache
                            let strike = {
                                let strikes = self.strike_prices.read().await;
                                strikes.get(&market.market_id.0).cloned()
                            };

                            // Determine winner based on crypto price vs strike
                            // If strike is set, compare against it
                            // Otherwise use current market prices as heuristic
                            let up_won = if let Some(strike) = strike {
                                crypto_price > strike
                            } else {
                                // Use market prices as fallback - if up_price > 0.5, UP likely won
                                let up_price =
                                    self.discovery.get_market_up_price(&market.market_id.0);
                                up_price > 0.5
                            };

                            // Close position at 1.0 (win) or 0.0 (loss)
                            let exit_price = if (trade.direction == hft_core::Direction::Up
                                && up_won)
                                || (trade.direction == hft_core::Direction::Down && !up_won)
                            {
                                Decimal::ONE // Winner - resolve at $1
                            } else {
                                Decimal::ZERO // Loser - resolve at $0
                            };

                            let win_str = if exit_price == Decimal::ONE {
                                "WIN"
                            } else {
                                "LOSS"
                            };
                            let pnl = if exit_price == Decimal::ONE {
                                // Won: payout is $1, profit = $1 - entry_price
                                Decimal::ONE - trade.entry_price
                            } else {
                                // Lost: lose entire stake
                                -trade.entry_price
                            };

                            // Update session statistics
                            if exit_price == Decimal::ONE {
                                self.session_wins.fetch_add(1, Ordering::Relaxed);
                            } else {
                                self.session_losses.fetch_add(1, Ordering::Relaxed);
                            }

                            // Update P&L (in cents to avoid float atomics)
                            // Use round() then to_i64() instead of string parsing (which fails on decimals)
                            let pnl_cents =
                                (pnl * Decimal::from(100)).round().to_i64().unwrap_or(0);
                            self.session_pnl_cents
                                .fetch_add(pnl_cents, std::sync::atomic::Ordering::Relaxed);

                            // Get current session stats for logging
                            let wins = self.session_wins.load(Ordering::Relaxed);
                            let losses = self.session_losses.load(Ordering::Relaxed);
                            let total_pnl_cents = self
                                .session_pnl_cents
                                .load(std::sync::atomic::Ordering::Relaxed);
                            let total_pnl = total_pnl_cents as f64 / 100.0;
                            let win_rate = if wins + losses > 0 {
                                (wins as f64 / (wins + losses) as f64) * 100.0
                            } else {
                                0.0
                            };

                            info!(
                                trade_id = %trade.trade_id,
                                asset = %market.asset,
                                direction = %trade.direction,
                                entry_price = %trade.entry_price,
                                up_won = up_won,
                                crypto_price = %crypto_price,
                                strike = ?market.strike_price,
                                pnl = %pnl,
                                session_wins = wins,
                                session_losses = losses,
                                session_pnl = format!("${:.2}", total_pnl),
                                win_rate = format!("{:.1}%", win_rate),
                                "RESOLUTION: {} bet {} - PnL: ${:.4} | Session: {}/{} ({:.1}%) ${:.2}",
                                win_str,
                                trade.direction,
                                pnl,
                                wins, losses, win_rate, total_pnl
                            );

                            self.simulator.close_position(&trade.trade_id, exit_price);
                        }
                    }
                }

                // Print session summary every 60 seconds if we have trades
                let wins = self.session_wins.load(Ordering::Relaxed);
                let losses = self.session_losses.load(Ordering::Relaxed);
                if (wins + losses > 0) && (last_debug_log.elapsed() > Duration::from_secs(60)) {
                    let total_pnl_cents = self
                        .session_pnl_cents
                        .load(std::sync::atomic::Ordering::Relaxed);
                    let total_pnl = total_pnl_cents as f64 / 100.0;
                    let win_rate = (wins as f64 / (wins + losses) as f64) * 100.0;
                    let trades = self.trades_executed.load(Ordering::Relaxed);

                    info!(
                        trades = trades,
                        wins = wins,
                        losses = losses,
                        win_rate = format!("{:.1}%", win_rate),
                        pnl = format!("${:.2}", total_pnl),
                        "=== SESSION STATS: {} trades, {}/{} wins ({:.1}%), ${:.2} P&L ===",
                        trades,
                        wins,
                        losses,
                        win_rate,
                        total_pnl
                    );
                }
            }

            // Small delay to avoid busy loop (1 millisecond)
            tokio::time::sleep(Duration::from_millis(1)).await;
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
    if dotenvy::dotenv().is_err() {
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
    let mut config = CryptoLatencyConfig::default();

    // Check for live trading mode
    // LIVE_TRADING=1 or PAPER_MODE=0 enables live trading
    let live_trading = std::env::var("LIVE_TRADING")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false)
        || std::env::var("PAPER_MODE")
            .map(|v| v == "0" || v.to_lowercase() == "false")
            .unwrap_or(false);

    // Position size from env var (default $1 for safety)
    if let Ok(size_str) = std::env::var("POSITION_SIZE_USD") {
        if let Ok(size) = size_str.parse::<i64>() {
            config.position_size_usd = Decimal::from(size);
            info!("Position size: ${}", size);
        }
    } else {
        config.position_size_usd = Decimal::ONE; // Safe default for testing
        info!("Position size: $1 (default, set POSITION_SIZE_USD to change)");
    }

    if live_trading {
        config.paper_mode = false;
        info!("🔴 LIVE TRADING MODE ENABLED - Real money at risk!");
        info!("   Position size: ${}", config.position_size_usd);
    } else {
        info!("📝 Paper trading mode (set LIVE_TRADING=1 for real trades)");
    }

    // Use aggressive signal config if AGGRESSIVE_MODE=1
    let aggressive_mode = std::env::var("AGGRESSIVE_MODE")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false);

    let signal_config = if aggressive_mode {
        info!("Using AGGRESSIVE signal detection (lower thresholds)");
        SignalConfig::aggressive()
    } else {
        info!("Using DEFAULT signal detection (min_trigger=15bps, min_edge=10bps)");
        info!("Tip: Set AGGRESSIVE_MODE=1 for lower thresholds during testing");
        SignalConfig::default()
    };

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

    // Create price feed - check env vars for feed selection
    // USE_COINBASE=1 for Coinbase (US testing)
    // USE_KRAKEN=1 for Kraken (works globally)
    // Default: Binance (for production on EU/NL server)
    let use_coinbase = std::env::var("USE_COINBASE")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false);

    let use_kraken = std::env::var("USE_KRAKEN")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false);

    let price_feed = if use_kraken {
        info!("Using Kraken price feed (works globally)");
        Arc::new(PriceFeed::kraken(KrakenConfig::default()))
    } else if use_coinbase {
        info!("Using Coinbase price feed (works from US for testing)");
        Arc::new(PriceFeed::coinbase(CoinbaseConfig::default()))
    } else {
        info!("Using Binance price feed (works from EU/NL server, blocked in US)");
        Arc::new(PriceFeed::binance(BinanceConfig::default()))
    };

    // Create Polymarket client with proxy support
    // WebSocket works direct from US - don't use proxies for WebSocket
    // Only use proxies for HTTP order execution (configured separately)
    let polymarket_client = if false && std::path::Path::new(&proxy_file).exists() {
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

    // Create order executor for live trading
    let executor: Option<Arc<OrderExecutor>> = if live_trading {
        // Get private key from environment
        let private_key = std::env::var("POLYMARKET_PRIVATE_KEY")
            .or_else(|_| std::env::var("POLYGON_PRIVATE_KEY"))
            .expect("POLYMARKET_PRIVATE_KEY or POLYGON_PRIVATE_KEY required for live trading");

        info!("Deriving API credentials from private key...");

        // Load HTTP proxy from proxy file for order submission (EU proxy needed to bypass US block)
        let http_proxy_url = if std::path::Path::new(&proxy_file).exists() {
            match std::fs::read_to_string(&proxy_file) {
                Ok(content) => {
                    // Get first proxy line: host:port:user:pass -> http://user:pass@host:port
                    if let Some(first_line) = content.lines().next() {
                        let parts: Vec<&str> = first_line.trim().split(':').collect();
                        if parts.len() >= 4 {
                            let proxy_url = format!(
                                "http://{}:{}@{}:{}",
                                parts[2], parts[3], parts[0], parts[1]
                            );
                            info!(proxy_host = %parts[0], "Using EU proxy for order execution");
                            Some(proxy_url)
                        } else {
                            warn!("Invalid proxy format in {}", proxy_file);
                            None
                        }
                    } else {
                        None
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Failed to read proxy file");
                    None
                }
            }
        } else {
            info!("No proxy file found - orders will be sent directly");
            None
        };

        let mut exec_config = ExecutorConfig::default();
        exec_config.proxy_url = http_proxy_url;

        // Derive credentials automatically (like Python py_clob_client)
        match OrderExecutor::new_with_derived_creds(
            exec_config,
            &private_key,
            137, // Polygon mainnet
        )
        .await
        {
            Ok(executor) => {
                info!(
                    wallet = %executor.address(),
                    "Live order executor initialized with derived credentials"
                );
                Some(Arc::new(executor))
            }
            Err(e) => {
                error!(error = %e, "Failed to create order executor");
                panic!("Cannot start live trading without executor: {}", e);
            }
        }
    } else {
        None
    };

    let app = Arc::new(CryptoLatencyApp::new_with_clients(
        config,
        signal_config,
        simulator_config,
        discovery_config,
        price_feed,
        polymarket_client,
        executor,
    ));

    // Start stats printer with detailed momentum logging
    let stats_app = app.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let stats = stats_app.stats();

            info!(
                signals = stats.signals_detected,
                trades = stats.trades_executed,
                open = stats.open_positions,
                markets = stats.active_markets,
                pnl = %stats.total_pnl,
                win_rate = format!("{:.1}%", stats.win_rate * 100.0),
                price_feed = stats.price_feed_source,
                price_connected = stats.price_feed_connected,
                pm_connected = stats.polymarket_connected,
                "Stats"
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
