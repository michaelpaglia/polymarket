//! HFT Server - High-Frequency Trading module for Polymarket
//!
//! Runs the arbitrage detection and execution loop with REST API for control.

use anyhow::Result;
use config::{Config, File};
use hft_api::{start_server, AppState};
use hft_core::{ArbitrageConfig, ArbitrageDetector, TradingState};
use hft_executor::{ApiCredentials, ExecutorConfig, OrderExecutor};
use hft_metrics::{init_logging, init_metrics};
use hft_risk::{CircuitBreaker, CircuitBreakerConfig, PnlTracker, RiskLimits, RiskManager};
use hft_websocket::{WebSocketClient, WebSocketConfig};
use rust_decimal::Decimal;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

/// Application configuration
#[derive(Debug, Clone, serde::Deserialize)]
struct AppConfig {
    server: ServerConfig,
    polymarket: PolymarketConfig,
    arbitrage: ArbitrageSettings,
    risk: RiskSettings,
    metrics: MetricsConfig,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ServerConfig {
    host: String,
    port: u16,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct PolymarketConfig {
    ws_url: String,
    rest_url: String,
    chain_id: u64,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ArbitrageSettings {
    min_spread_bps: u32,
    max_position_size_usd: f64,
    cooldown_ms: u64,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RiskSettings {
    max_total_exposure_usd: f64,
    max_daily_loss_usd: f64,
    max_positions: usize,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct MetricsConfig {
    prometheus_port: u16,
    log_level: String,
    json_logs: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables from .env file in project root
    if let Err(_) = dotenvy::dotenv() {
        // Try loading from parent directories
        let _ = dotenvy::from_filename("../../.env");
    }

    // Load configuration
    let config_path = std::env::var("CONFIG_PATH")
        .unwrap_or_else(|_| "../../config/default.toml".to_string());

    let config: AppConfig = Config::builder()
        .add_source(File::with_name(&config_path).required(false))
        .add_source(File::with_name("config/default").required(false))
        .set_default("server.host", "127.0.0.1")?
        .set_default("server.port", 8080)?
        .set_default("polymarket.ws_url", "wss://ws-subscriptions-clob.polymarket.com/ws/market")?
        .set_default("polymarket.rest_url", "https://clob.polymarket.com")?
        .set_default("polymarket.chain_id", 137)?
        .set_default("arbitrage.min_spread_bps", 30)?
        .set_default("arbitrage.max_position_size_usd", 500.0)?
        .set_default("arbitrage.cooldown_ms", 100)?
        .set_default("risk.max_total_exposure_usd", 5000.0)?
        .set_default("risk.max_daily_loss_usd", 500.0)?
        .set_default("risk.max_positions", 10)?
        .set_default("metrics.prometheus_port", 9090)?
        .set_default("metrics.log_level", "info")?
        .set_default("metrics.json_logs", false)?
        .build()?
        .try_deserialize()?;

    // Initialize logging
    init_logging(config.metrics.json_logs);
    info!("HFT Server starting...");

    // Initialize Prometheus metrics
    let metrics_addr: SocketAddr = format!("0.0.0.0:{}", config.metrics.prometheus_port).parse()?;
    if let Err(e) = init_metrics(metrics_addr) {
        warn!(error = %e, "Failed to initialize metrics exporter");
    } else {
        info!(addr = %metrics_addr, "Metrics exporter running");
    }

    // Get credentials from environment
    let private_key = std::env::var("POLYMARKET_PRIVATE_KEY")
        .expect("POLYMARKET_PRIVATE_KEY must be set");

    // API credentials - use defaults for paper trading if not set
    let api_key = std::env::var("POLYMARKET_API_KEY")
        .unwrap_or_else(|_| "paper_trading_key".to_string());
    let api_secret = std::env::var("POLYMARKET_API_SECRET")
        .unwrap_or_else(|_| "paper_trading_secret".to_string());
    let api_passphrase = std::env::var("POLYMARKET_API_PASSPHRASE")
        .unwrap_or_else(|_| "paper_trading_passphrase".to_string());

    let credentials = ApiCredentials {
        api_key,
        api_secret,
        api_passphrase,
    };

    // Get initial capital from environment
    let initial_capital: Decimal = std::env::var("HFT_CAPITAL_USD")
        .unwrap_or_else(|_| "5000".to_string())
        .parse()
        .unwrap_or(Decimal::from(5000));

    info!(capital = %initial_capital, "Initial capital set");

    // Initialize components
    let risk_limits = RiskLimits {
        max_total_exposure_usd: Decimal::from_f64_retain(config.risk.max_total_exposure_usd)
            .unwrap_or(Decimal::from(5000)),
        max_positions: config.risk.max_positions,
        max_position_size_usd: Decimal::from_f64_retain(config.arbitrage.max_position_size_usd)
            .unwrap_or(Decimal::from(500)),
        ..Default::default()
    };

    let risk_manager = Arc::new(RiskManager::new(risk_limits, initial_capital));

    let circuit_breaker_config = CircuitBreakerConfig {
        max_daily_loss_usd: Decimal::from_f64_retain(config.risk.max_daily_loss_usd)
            .unwrap_or(Decimal::from(500)),
        ..Default::default()
    };
    let circuit_breaker = Arc::new(CircuitBreaker::new(circuit_breaker_config));

    let pnl_tracker = Arc::new(PnlTracker::new(1000));

    // Initialize arbitrage detector
    let arb_config = ArbitrageConfig {
        min_spread_bps: config.arbitrage.min_spread_bps,
        max_position_size_usd: Decimal::from_f64_retain(config.arbitrage.max_position_size_usd)
            .unwrap_or(Decimal::from(500)),
        cooldown_us: config.arbitrage.cooldown_ms * 1000,
        ..Default::default()
    };
    let detector = Arc::new(ArbitrageDetector::new(arb_config));

    // Initialize WebSocket client with optional proxy rotation
    let ws_config = WebSocketConfig {
        url: config.polymarket.ws_url.clone(),
        ..Default::default()
    };

    // Load proxies from file or environment
    let proxy_file = std::env::var("HFT_PROXY_FILE").unwrap_or_else(|_| "eu_proxies.txt".to_string());
    let ws_client = if std::path::Path::new(&proxy_file).exists() {
        match hft_websocket::ProxyRotator::from_file(&proxy_file) {
            Ok(rotator) => {
                info!(
                    count = rotator.len(),
                    path = %proxy_file,
                    "Proxy rotation enabled - will rotate on reconnect"
                );
                Arc::new(WebSocketClient::with_proxy_rotator(ws_config, rotator))
            }
            Err(e) => {
                warn!(error = %e, path = %proxy_file, "Failed to load proxy file, using direct connection");
                Arc::new(WebSocketClient::new(ws_config))
            }
        }
    } else if let Ok(proxy_str) = std::env::var("HFT_PROXY") {
        // Fallback to single proxy from environment
        let config_with_proxy = ws_config.with_proxy(&proxy_str);
        if let Some(ref proxy) = config_with_proxy.proxy {
            info!(host = %proxy.host, port = proxy.port, "Single proxy configured from env");
        }
        Arc::new(WebSocketClient::new(config_with_proxy))
    } else {
        info!("No proxy configured, using direct connection");
        Arc::new(WebSocketClient::new(ws_config))
    };

    // Create application state with WebSocket and arbitrage detector
    let app_state = Arc::new(AppState::with_ws(
        risk_manager.clone(),
        circuit_breaker.clone(),
        pnl_tracker.clone(),
        ws_client.clone(),
        detector.clone(),
    ));

    // Initialize order executor
    let executor_config = ExecutorConfig {
        clob_url: config.polymarket.rest_url.clone(),
        ..Default::default()
    };
    let executor = Arc::new(
        OrderExecutor::new(
            executor_config,
            &private_key,
            credentials,
            config.polymarket.chain_id,
        )
        .expect("Failed to create order executor"),
    );

    info!(address = %executor.address(), "Order executor initialized");

    // Start API server
    let api_bind = format!("{}:{}", config.server.host, config.server.port);
    info!(bind = %api_bind, "API server starting");
    let api_state = app_state.clone();
    let api_handle = tokio::spawn(async move {
        if let Err(e) = start_server(api_state, &api_bind).await {
            error!(error = %e, "API server error");
        }
    });

    // Main trading loop
    let trading_state = app_state.clone();
    let trading_detector = detector.clone();
    let trading_executor = executor.clone();
    let trading_risk = risk_manager.clone();
    let trading_cb = circuit_breaker.clone();
    let trading_pnl = pnl_tracker.clone();

    let trading_handle = tokio::spawn(async move {
        info!("Trading loop started");

        loop {
            // Check if trading is enabled
            let state = trading_state.state();
            if state != TradingState::Running {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }

            // Check circuit breaker
            if trading_cb.is_tripped() {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }

            // Scan for opportunities
            let opportunities = trading_detector.scan_all();

            for opportunity in opportunities {
                // Check risk limits
                match trading_risk.check_opportunity(&opportunity) {
                    Ok(approved_size) => {
                        trading_state.inc_opportunities();

                        // Create modified opportunity with approved size
                        let mut opp = opportunity.clone();
                        opp.max_size_usd = approved_size;

                        // Execute arbitrage
                        match trading_executor.execute_arbitrage(&opp).await {
                            Ok(execution) => {
                                trading_state.inc_trades();
                                trading_state.record_latency(execution.execution_time_us);

                                // Record in P&L tracker
                                trading_pnl.record_trade(&execution);

                                // Record in circuit breaker
                                let success = execution.yes_filled && execution.no_filled;
                                trading_cb.record_trade(
                                    execution.expected_profit_usd,
                                    success,
                                );

                                // Record cooldown
                                trading_detector.record_trade(&opportunity.market_id);

                                info!(
                                    execution_id = %execution.execution_id,
                                    profit_bps = opportunity.profit_bps,
                                    "Arbitrage executed"
                                );
                            }
                            Err(e) => {
                                error!(error = %e, "Arbitrage execution failed");
                                trading_cb.record_trade(Decimal::ZERO, false);
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, market_id = %opportunity.market_id, "Risk check failed");
                    }
                }
            }

            // Small delay between scans
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("Shutdown signal received");

    // Cleanup
    app_state.set_state(TradingState::Stopping);
    ws_client.stop();

    // Wait for tasks to finish
    api_handle.abort();
    trading_handle.abort();

    info!("HFT Server stopped");
    Ok(())
}
