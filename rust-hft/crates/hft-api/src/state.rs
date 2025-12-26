//! Application state for API handlers

use hft_core::{ArbitrageDetector, TradingState};
use hft_risk::{CircuitBreaker, PnlTracker, RiskManager};
use hft_websocket::WebSocketClient;
use parking_lot::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Shared application state
pub struct AppState {
    /// Trading state
    pub trading_state: RwLock<TradingState>,
    /// Start time
    pub start_time: Instant,
    /// Risk manager
    pub risk_manager: Arc<RiskManager>,
    /// Circuit breaker
    pub circuit_breaker: Arc<CircuitBreaker>,
    /// P&L tracker
    pub pnl_tracker: Arc<PnlTracker>,
    /// WebSocket client for market data
    pub ws_client: Option<Arc<WebSocketClient>>,
    /// Arbitrage detector
    pub arb_detector: Option<Arc<ArbitrageDetector>>,
    /// Subscribed markets count
    pub subscribed_markets: AtomicU64,
    /// Opportunities detected count
    pub opportunities_detected: AtomicU64,
    /// Trades executed count
    pub trades_executed: AtomicU64,
    /// Latency tracking (microseconds)
    pub latency_sum_us: AtomicU64,
    pub latency_count: AtomicU64,
    pub latency_max_us: AtomicU64,
    /// WebSocket connected flag
    pub ws_connected: std::sync::atomic::AtomicBool,
}

impl AppState {
    /// Create new application state
    pub fn new(
        risk_manager: Arc<RiskManager>,
        circuit_breaker: Arc<CircuitBreaker>,
        pnl_tracker: Arc<PnlTracker>,
    ) -> Self {
        Self {
            trading_state: RwLock::new(TradingState::Stopped),
            start_time: Instant::now(),
            risk_manager,
            circuit_breaker,
            pnl_tracker,
            ws_client: None,
            arb_detector: None,
            subscribed_markets: AtomicU64::new(0),
            opportunities_detected: AtomicU64::new(0),
            trades_executed: AtomicU64::new(0),
            latency_sum_us: AtomicU64::new(0),
            latency_count: AtomicU64::new(0),
            latency_max_us: AtomicU64::new(0),
            ws_connected: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Create with WebSocket and arbitrage detector
    pub fn with_ws(
        risk_manager: Arc<RiskManager>,
        circuit_breaker: Arc<CircuitBreaker>,
        pnl_tracker: Arc<PnlTracker>,
        ws_client: Arc<WebSocketClient>,
        arb_detector: Arc<ArbitrageDetector>,
    ) -> Self {
        Self {
            trading_state: RwLock::new(TradingState::Stopped),
            start_time: Instant::now(),
            risk_manager,
            circuit_breaker,
            pnl_tracker,
            ws_client: Some(ws_client),
            arb_detector: Some(arb_detector),
            subscribed_markets: AtomicU64::new(0),
            opportunities_detected: AtomicU64::new(0),
            trades_executed: AtomicU64::new(0),
            latency_sum_us: AtomicU64::new(0),
            latency_count: AtomicU64::new(0),
            latency_max_us: AtomicU64::new(0),
            ws_connected: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Check if WebSocket is connected
    pub fn is_ws_connected(&self) -> bool {
        self.ws_connected.load(Ordering::Relaxed)
    }

    /// Set WebSocket connected status
    pub fn set_ws_connected(&self, connected: bool) {
        self.ws_connected.store(connected, Ordering::Relaxed);
    }

    /// Get trading state
    pub fn state(&self) -> TradingState {
        *self.trading_state.read()
    }

    /// Set trading state
    pub fn set_state(&self, state: TradingState) {
        *self.trading_state.write() = state;
    }

    /// Get uptime in seconds
    pub fn uptime_seconds(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    /// Record latency
    pub fn record_latency(&self, latency_us: u64) {
        self.latency_sum_us.fetch_add(latency_us, Ordering::Relaxed);
        self.latency_count.fetch_add(1, Ordering::Relaxed);

        // Update max
        let mut current_max = self.latency_max_us.load(Ordering::Relaxed);
        while latency_us > current_max {
            match self.latency_max_us.compare_exchange_weak(
                current_max,
                latency_us,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(c) => current_max = c,
            }
        }
    }

    /// Get average latency in microseconds
    pub fn avg_latency_us(&self) -> u64 {
        let sum = self.latency_sum_us.load(Ordering::Relaxed);
        let count = self.latency_count.load(Ordering::Relaxed);
        if count > 0 {
            sum / count
        } else {
            0
        }
    }

    /// Get max latency in microseconds
    pub fn max_latency_us(&self) -> u64 {
        self.latency_max_us.load(Ordering::Relaxed)
    }

    /// Increment opportunity count
    pub fn inc_opportunities(&self) {
        self.opportunities_detected.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment trade count
    pub fn inc_trades(&self) {
        self.trades_executed.fetch_add(1, Ordering::Relaxed);
    }

    /// Get opportunity count
    pub fn opportunities_count(&self) -> u64 {
        self.opportunities_detected.load(Ordering::Relaxed)
    }

    /// Get trade count
    pub fn trades_count(&self) -> u64 {
        self.trades_executed.load(Ordering::Relaxed)
    }

    /// Get subscribed markets count
    pub fn markets_count(&self) -> u64 {
        self.subscribed_markets.load(Ordering::Relaxed)
    }

    /// Set subscribed markets count
    pub fn set_markets_count(&self, count: u64) {
        self.subscribed_markets.store(count, Ordering::Relaxed);
    }
}
