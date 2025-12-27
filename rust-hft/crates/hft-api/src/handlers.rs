//! API request handlers

use crate::state::AppState;
use axum::{extract::State, http::StatusCode, Json};
use hft_core::{MarketId, TokenId, TradingState};
use hft_risk::{BalanceInfo, CircuitBreakerState, PnlSummary, RiskLimits, RiskStats};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Status response
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub state: TradingState,
    pub uptime_seconds: u64,
    pub connected: bool,
    pub subscribed_markets: u64,
    pub opportunities_detected: u64,
    pub trades_executed: u64,
    pub current_pnl_usd: Decimal,
    pub latency_avg_us: u64,
    pub latency_max_us: u64,
    pub circuit_breaker_tripped: bool,
}

/// Get current status
pub async fn get_status(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let pnl = state.pnl_tracker.summary();

    Json(StatusResponse {
        state: state.state(),
        uptime_seconds: state.uptime_seconds(),
        connected: state.is_ws_connected(),
        subscribed_markets: state.markets_count(),
        opportunities_detected: state.opportunities_count(),
        trades_executed: state.trades_count(),
        current_pnl_usd: pnl.total_pnl,
        latency_avg_us: state.avg_latency_us(),
        latency_max_us: state.max_latency_us(),
        circuit_breaker_tripped: state.circuit_breaker.is_tripped(),
    })
}

/// Simple response
#[derive(Debug, Serialize)]
pub struct SimpleResponse {
    pub success: bool,
    pub message: String,
}

/// Start trading
pub async fn start_trading(State(state): State<Arc<AppState>>) -> Json<SimpleResponse> {
    let current = state.state();

    if current == TradingState::Running {
        return Json(SimpleResponse {
            success: false,
            message: "Already running".to_string(),
        });
    }

    if state.circuit_breaker.is_tripped() {
        return Json(SimpleResponse {
            success: false,
            message: "Circuit breaker is tripped".to_string(),
        });
    }

    state.set_state(TradingState::Running);

    Json(SimpleResponse {
        success: true,
        message: "Trading started".to_string(),
    })
}

/// Stop trading
pub async fn stop_trading(State(state): State<Arc<AppState>>) -> Json<SimpleResponse> {
    state.set_state(TradingState::Stopped);

    Json(SimpleResponse {
        success: true,
        message: "Trading stopped".to_string(),
    })
}

/// Pause trading
pub async fn pause_trading(State(state): State<Arc<AppState>>) -> Json<SimpleResponse> {
    state.set_state(TradingState::Paused);

    Json(SimpleResponse {
        success: true,
        message: "Trading paused".to_string(),
    })
}

/// Capital request
#[derive(Debug, Deserialize)]
pub struct CapitalRequest {
    pub allocation_usd: Decimal,
    pub max_position_size_usd: Option<Decimal>,
    pub max_exposure_usd: Option<Decimal>,
}

/// Capital response
#[derive(Debug, Serialize)]
pub struct CapitalResponse {
    pub success: bool,
    pub allocation_usd: Decimal,
    pub available_usd: Decimal,
}

/// Set capital allocation
pub async fn set_capital(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CapitalRequest>,
) -> Json<CapitalResponse> {
    state.risk_manager.set_capital(req.allocation_usd);

    // Update limits if provided
    if req.max_position_size_usd.is_some() || req.max_exposure_usd.is_some() {
        let current = state.risk_manager.stats();
        let mut limits = RiskLimits::default();

        if let Some(max_pos) = req.max_position_size_usd {
            limits.max_position_size_usd = max_pos;
        }
        if let Some(max_exp) = req.max_exposure_usd {
            limits.max_total_exposure_usd = max_exp;
        }

        state.risk_manager.update_limits(limits);
    }

    Json(CapitalResponse {
        success: true,
        allocation_usd: req.allocation_usd,
        available_usd: state.risk_manager.available_capital(),
    })
}

/// Stats response
#[derive(Debug, Serialize)]
pub struct StatsResponse {
    pub risk: RiskStats,
    pub pnl: PnlSummary,
    pub circuit_breaker: CircuitBreakerState,
    pub opportunities_detected: u64,
    pub trades_executed: u64,
    pub latency_avg_us: u64,
    pub latency_max_us: u64,
}

/// Get statistics
pub async fn get_stats(State(state): State<Arc<AppState>>) -> Json<StatsResponse> {
    Json(StatsResponse {
        risk: state.risk_manager.stats(),
        pnl: state.pnl_tracker.summary(),
        circuit_breaker: state.circuit_breaker.state(),
        opportunities_detected: state.opportunities_count(),
        trades_executed: state.trades_count(),
        latency_avg_us: state.avg_latency_us(),
        latency_max_us: state.max_latency_us(),
    })
}

/// Get P&L
pub async fn get_pnl(State(state): State<Arc<AppState>>) -> Json<PnlSummary> {
    Json(state.pnl_tracker.summary())
}

/// Health check
pub async fn health_check() -> StatusCode {
    StatusCode::OK
}

/// Reset circuit breaker
pub async fn reset_circuit_breaker(State(state): State<Arc<AppState>>) -> Json<SimpleResponse> {
    state.circuit_breaker.reset();

    Json(SimpleResponse {
        success: true,
        message: "Circuit breaker reset".to_string(),
    })
}

/// Paper trade request
#[derive(Debug, Deserialize)]
pub struct PaperTradeRequest {
    pub market_id: Option<String>,
    pub yes_price: Option<Decimal>,
    pub no_price: Option<Decimal>,
    pub size_usd: Option<Decimal>,
}

/// Paper trade response
#[derive(Debug, Serialize)]
pub struct PaperTradeResponse {
    pub success: bool,
    pub execution_id: String,
    pub market_id: String,
    pub yes_price: Decimal,
    pub no_price: Decimal,
    pub price_sum: Decimal,
    pub spread_bps: u32,
    pub size_usd: Decimal,
    pub expected_profit_usd: Decimal,
    pub execution_time_us: u64,
    pub message: String,
}

/// Execute a paper trade for testing
pub async fn paper_trade(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PaperTradeRequest>,
) -> Json<PaperTradeResponse> {
    use rust_decimal_macros::dec;
    use std::time::Instant;
    use uuid::Uuid;

    let start = Instant::now();

    // Use provided values or defaults that show a profitable arbitrage
    let market_id = req.market_id.unwrap_or_else(|| "paper-test-market".to_string());
    let yes_price = req.yes_price.unwrap_or(dec!(0.45));  // 45 cents
    let no_price = req.no_price.unwrap_or(dec!(0.52));    // 52 cents
    let size_usd = req.size_usd.unwrap_or(dec!(100));     // $100 test trade

    let price_sum = yes_price + no_price;
    let spread = Decimal::ONE - price_sum;
    let spread_bps = ((spread * dec!(10000)).to_string().parse::<f64>().unwrap_or(0.0)) as u32;

    // Check if this would be a valid arbitrage
    if price_sum >= Decimal::ONE {
        return Json(PaperTradeResponse {
            success: false,
            execution_id: Uuid::new_v4().to_string(),
            market_id,
            yes_price,
            no_price,
            price_sum,
            spread_bps: 0,
            size_usd,
            expected_profit_usd: Decimal::ZERO,
            execution_time_us: start.elapsed().as_micros() as u64,
            message: format!("No arbitrage: price sum {} >= 1.0", price_sum),
        });
    }

    // Calculate expected profit
    let expected_profit = spread * size_usd;

    // Simulate execution time (would be faster in real scenario)
    let execution_time_us = start.elapsed().as_micros() as u64;

    // Record the simulated trade in stats
    state.inc_opportunities();
    state.inc_trades();
    state.record_latency(execution_time_us);

    // Record P&L (simulated profit)
    state.pnl_tracker.record_paper_trade(expected_profit);

    let execution_id = Uuid::new_v4().to_string();

    tracing::info!(
        execution_id = %execution_id,
        market_id = %market_id,
        yes_price = %yes_price,
        no_price = %no_price,
        spread_bps = spread_bps,
        size_usd = %size_usd,
        expected_profit = %expected_profit,
        "Paper trade executed"
    );

    Json(PaperTradeResponse {
        success: true,
        execution_id,
        market_id,
        yes_price,
        no_price,
        price_sum,
        spread_bps,
        size_usd,
        expected_profit_usd: expected_profit,
        execution_time_us,
        message: format!(
            "Paper trade: BUY YES@{} + BUY NO@{} = {} spread ({} bps), profit ${}",
            yes_price, no_price, spread, spread_bps, expected_profit
        ),
    })
}

/// Subscribe market request
#[derive(Debug, Deserialize)]
pub struct SubscribeMarketRequest {
    /// Market ID (for identification)
    pub market_id: String,
    /// YES token ID from Polymarket
    pub yes_token_id: String,
    /// NO token ID from Polymarket
    pub no_token_id: String,
}

/// Subscribe response
#[derive(Debug, Serialize)]
pub struct SubscribeResponse {
    pub success: bool,
    pub market_id: String,
    pub message: String,
    pub subscribed_count: u64,
}

/// Subscribe to a market for arbitrage monitoring
pub async fn subscribe_market(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SubscribeMarketRequest>,
) -> Json<SubscribeResponse> {
    // Check if we have WebSocket client and arbitrage detector
    let (ws_client, arb_detector) = match (&state.ws_client, &state.arb_detector) {
        (Some(ws), Some(arb)) => (ws.clone(), arb.clone()),
        _ => {
            return Json(SubscribeResponse {
                success: false,
                market_id: req.market_id,
                message: "WebSocket client not initialized".to_string(),
                subscribed_count: 0,
            });
        }
    };

    let market_id = MarketId::new(&req.market_id);
    let yes_token = TokenId::new(&req.yes_token_id);
    let no_token = TokenId::new(&req.no_token_id);

    // Register market with arbitrage detector
    let orderbook = arb_detector.register_market(
        market_id.clone(),
        yes_token.clone(),
        no_token.clone(),
    );

    // Register with WebSocket client
    ws_client.register_market(orderbook).await;

    // Subscribe to both token IDs
    ws_client.subscribe(vec![
        req.yes_token_id.clone(),
        req.no_token_id.clone(),
    ]).await;

    // Update subscribed count
    let count = arb_detector.market_count() as u64;
    state.set_markets_count(count);

    tracing::info!(
        market_id = %req.market_id,
        yes_token = %req.yes_token_id,
        no_token = %req.no_token_id,
        "Market subscribed"
    );

    Json(SubscribeResponse {
        success: true,
        market_id: req.market_id,
        message: "Market subscribed for arbitrage monitoring".to_string(),
        subscribed_count: count,
    })
}

/// Connect request
#[derive(Debug, Deserialize)]
pub struct ConnectRequest {
    /// Optional list of markets to subscribe on connect
    pub markets: Option<Vec<SubscribeMarketRequest>>,
}

/// Connect response
#[derive(Debug, Serialize)]
pub struct ConnectResponse {
    pub success: bool,
    pub connected: bool,
    pub subscribed_markets: u64,
    pub message: String,
}

/// Connect to WebSocket and optionally subscribe to markets
pub async fn connect_websocket(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ConnectRequest>,
) -> Json<ConnectResponse> {
    let ws_client = match &state.ws_client {
        Some(ws) => ws.clone(),
        None => {
            return Json(ConnectResponse {
                success: false,
                connected: false,
                subscribed_markets: 0,
                message: "WebSocket client not initialized".to_string(),
            });
        }
    };

    // Subscribe to markets if provided
    if let Some(markets) = req.markets {
        if let Some(arb) = &state.arb_detector {
            for market in markets {
                let market_id = MarketId::new(&market.market_id);
                let yes_token = TokenId::new(&market.yes_token_id);
                let no_token = TokenId::new(&market.no_token_id);

                let orderbook = arb.register_market(market_id, yes_token, no_token);
                ws_client.register_market(orderbook).await;
                ws_client.subscribe(vec![
                    market.yes_token_id,
                    market.no_token_id,
                ]).await;
            }
            state.set_markets_count(arb.market_count() as u64);
        }
    }

    // Start WebSocket connection in background
    let ws_for_task = ws_client.clone();
    let state_for_task = state.clone();
    tokio::spawn(async move {
        state_for_task.set_ws_connected(true);
        if let Err(e) = ws_for_task.connect().await {
            tracing::error!(error = %e, "WebSocket connection failed");
        }
        state_for_task.set_ws_connected(false);
    });

    // Give it a moment to connect
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    Json(ConnectResponse {
        success: true,
        connected: ws_client.is_connected(),
        subscribed_markets: state.markets_count(),
        message: "WebSocket connection started".to_string(),
    })
}

/// Get live orderbook for a market
#[derive(Debug, Serialize)]
pub struct OrderbookResponse {
    pub market_id: String,
    pub yes_best_bid: Option<String>,
    pub yes_best_ask: Option<String>,
    pub no_best_bid: Option<String>,
    pub no_best_ask: Option<String>,
    pub price_sum: Option<String>,
    pub spread_bps: Option<u32>,
    pub has_arbitrage: bool,
}

/// Get orderbook for a specific market
pub async fn get_orderbook(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(market_id): axum::extract::Path<String>,
) -> Json<OrderbookResponse> {
    let arb = match &state.arb_detector {
        Some(a) => a.clone(),
        None => {
            return Json(OrderbookResponse {
                market_id,
                yes_best_bid: None,
                yes_best_ask: None,
                no_best_bid: None,
                no_best_ask: None,
                price_sum: None,
                spread_bps: None,
                has_arbitrage: false,
            });
        }
    };

    let mid = MarketId::new(&market_id);

    if let Some(market) = arb.get_market(&mid) {
        let yes_snap = market.yes_book.snapshot();
        let no_snap = market.no_book.snapshot();

        let yes_bid = yes_snap.bids.first().map(|l| l.price.to_string());
        let yes_ask = yes_snap.asks.first().map(|l| l.price.to_string());
        let no_bid = no_snap.bids.first().map(|l| l.price.to_string());
        let no_ask = no_snap.asks.first().map(|l| l.price.to_string());

        let (price_sum, spread_bps, has_arb) = if let (Some(ya), Some(na)) =
            (yes_snap.asks.first(), no_snap.asks.first())
        {
            let sum = ya.price + na.price;
            let spread = rust_decimal::Decimal::ONE - sum;
            let bps = if spread > rust_decimal::Decimal::ZERO {
                ((spread * rust_decimal_macros::dec!(10000)).to_string().parse::<f64>().unwrap_or(0.0)) as u32
            } else {
                0
            };
            (Some(sum.to_string()), Some(bps), sum < rust_decimal::Decimal::ONE)
        } else {
            (None, None, false)
        };

        Json(OrderbookResponse {
            market_id,
            yes_best_bid: yes_bid,
            yes_best_ask: yes_ask,
            no_best_bid: no_bid,
            no_best_ask: no_ask,
            price_sum,
            spread_bps,
            has_arbitrage: has_arb,
        })
    } else {
        Json(OrderbookResponse {
            market_id,
            yes_best_bid: None,
            yes_best_ask: None,
            no_best_bid: None,
            no_best_ask: None,
            price_sum: None,
            spread_bps: None,
            has_arbitrage: false,
        })
    }
}

/// Balance sync request
#[derive(Debug, Deserialize)]
pub struct BalanceSyncRequest {
    /// Total account balance in USD
    pub account_balance_usd: Decimal,
}

/// Balance sync response
#[derive(Debug, Serialize)]
pub struct BalanceSyncResponse {
    pub success: bool,
    pub balance_info: BalanceInfo,
    pub message: String,
}

/// Sync account balance - automatically recalculates available capital based on 50% cap
pub async fn sync_balance(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BalanceSyncRequest>,
) -> Json<BalanceSyncResponse> {
    state.risk_manager.sync_balance(req.account_balance_usd);
    let info = state.risk_manager.balance_info();

    tracing::info!(
        account_balance = %req.account_balance_usd,
        max_allowed = %info.max_allowed,
        available = %info.available_capital,
        "Balance synced"
    );

    Json(BalanceSyncResponse {
        success: true,
        balance_info: info.clone(),
        message: format!(
            "Balance synced: {} total, {} max for HFT ({}%), {} available",
            req.account_balance_usd,
            info.max_allowed,
            (info.max_percentage * rust_decimal_macros::dec!(100)).round(),
            info.available_capital
        ),
    })
}

/// Get current balance allocation info
pub async fn get_balance_info(
    State(state): State<Arc<AppState>>,
) -> Json<BalanceInfo> {
    Json(state.risk_manager.balance_info())
}

/// Set balance percentage request
#[derive(Debug, Deserialize)]
pub struct SetBalancePercentageRequest {
    /// Percentage of account balance to use (0.0 - 1.0, e.g., 0.5 for 50%)
    pub percentage: Decimal,
}

/// Set the maximum percentage of account balance to use
pub async fn set_balance_percentage(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetBalancePercentageRequest>,
) -> Json<BalanceSyncResponse> {
    state.risk_manager.set_balance_percentage(req.percentage);
    let info = state.risk_manager.balance_info();

    tracing::info!(
        percentage = %req.percentage,
        max_allowed = %info.max_allowed,
        "Balance percentage updated"
    );

    Json(BalanceSyncResponse {
        success: true,
        balance_info: info.clone(),
        message: format!(
            "Balance percentage set to {}%, max allowed: {}",
            (req.percentage * rust_decimal_macros::dec!(100)).round(),
            info.max_allowed
        ),
    })
}
