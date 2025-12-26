//! API routes

use crate::handlers::*;
use crate::state::AppState;
use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

/// Create API router
pub fn create_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // Control endpoints
        .route("/api/v1/status", get(get_status))
        .route("/api/v1/start", post(start_trading))
        .route("/api/v1/stop", post(stop_trading))
        .route("/api/v1/pause", post(pause_trading))
        // Capital management
        .route("/api/v1/capital", post(set_capital))
        // Statistics
        .route("/api/v1/stats", get(get_stats))
        .route("/api/v1/stats/pnl", get(get_pnl))
        // Circuit breaker
        .route("/api/v1/circuit-breaker/reset", post(reset_circuit_breaker))
        // Testing / Paper trading
        .route("/api/v1/test/paper-trade", post(paper_trade))
        // Market subscription & WebSocket
        .route("/api/v1/markets/subscribe", post(subscribe_market))
        .route("/api/v1/markets/:market_id/orderbook", get(get_orderbook))
        .route("/api/v1/connect", post(connect_websocket))
        // Health
        .route("/health", get(health_check))
        // State and middleware
        .with_state(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
}

/// Start API server
pub async fn start_server(state: Arc<AppState>, bind_addr: &str) -> Result<(), std::io::Error> {
    let router = create_router(state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(addr = %bind_addr, "API server listening");

    axum::serve(listener, router).await
}
