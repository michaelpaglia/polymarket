//! HFT Metrics - Prometheus metrics and structured logging

use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::PrometheusBuilder;
use std::net::SocketAddr;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Initialize logging
pub fn init_logging(json_format: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    if json_format {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().json())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer())
            .init();
    }
}

/// Initialize Prometheus metrics exporter
pub fn init_metrics(bind_addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    PrometheusBuilder::new()
        .with_http_listener(bind_addr)
        .install()?;

    Ok(())
}

/// Record opportunity detected
pub fn record_opportunity(market_id: &str, spread_bps: u32) {
    counter!("hft_opportunities_total", "market_id" => market_id.to_string()).increment(1);
    histogram!("hft_opportunity_spread_bps").record(spread_bps as f64);
}

/// Record trade execution
pub fn record_execution(success: bool, execution_time_us: u64) {
    if success {
        counter!("hft_executions_success_total").increment(1);
    } else {
        counter!("hft_executions_failed_total").increment(1);
    }
    histogram!("hft_execution_time_us").record(execution_time_us as f64);
}

/// Record WebSocket message
pub fn record_ws_message() {
    counter!("hft_ws_messages_total").increment(1);
}

/// Record WebSocket reconnection
pub fn record_ws_reconnect() {
    counter!("hft_ws_reconnects_total").increment(1);
}

/// Set current position count
pub fn set_position_count(count: usize) {
    gauge!("hft_positions_current").set(count as f64);
}

/// Set current exposure
pub fn set_exposure(usd: f64) {
    gauge!("hft_exposure_usd").set(usd);
}

/// Set realized P&L
pub fn set_realized_pnl(usd: f64) {
    gauge!("hft_realized_pnl_usd").set(usd);
}

/// Set unrealized P&L
pub fn set_unrealized_pnl(usd: f64) {
    gauge!("hft_unrealized_pnl_usd").set(usd);
}

/// Record circuit breaker trip
pub fn record_circuit_breaker_trip(reason: &str) {
    counter!("hft_circuit_breaker_trips_total", "reason" => reason.to_string()).increment(1);
}

/// Set subscribed markets count
pub fn set_subscribed_markets(count: usize) {
    gauge!("hft_subscribed_markets").set(count as f64);
}

/// Record order submission latency
pub fn record_order_latency(latency_us: u64) {
    histogram!("hft_order_latency_us").record(latency_us as f64);
}

/// Record price update latency (from WS message to orderbook update)
pub fn record_price_update_latency(latency_us: u64) {
    histogram!("hft_price_update_latency_us").record(latency_us as f64);
}
