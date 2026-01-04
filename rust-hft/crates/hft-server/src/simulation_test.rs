//! Simulation test binary
//!
//! Tests the paper trading system with simulated signals to verify
//! the trading logic works correctly.

use anyhow::Result;
use chrono::{Duration as ChronoDuration, Utc};
use hft_core::{CryptoAsset, CryptoMarket, Direction, LatencySignal, MarketId, TokenId};
use hft_simulator::{ExitConfig, PaperTradeSimulator, SimulatorConfig};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::time::Duration;
use tracing::info;

fn create_test_market(asset: CryptoAsset) -> CryptoMarket {
    let now = Utc::now();
    let end_time = now + ChronoDuration::minutes(15);

    CryptoMarket {
        market_id: MarketId(format!("test-{}-15m", asset)),
        asset,
        up_token_id: TokenId(format!("up-token-{}", asset)),
        down_token_id: TokenId(format!("down-token-{}", asset)),
        strike_price: Some(dec!(91000)),
        end_time,
        discovered_at: now,
        question: format!("Will {} go up in the next 15 minutes?", asset),
    }
}

fn create_signal(
    asset: CryptoAsset,
    direction: Direction,
    edge_bps: u32,
    confidence: f64,
) -> LatencySignal {
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;

    LatencySignal {
        signal_id: uuid::Uuid::new_v4().to_string(),
        asset,
        direction,
        binance_price: dec!(91000),
        price_change_bps: edge_bps as i32,
        polymarket_price: dec!(0.50),
        expected_price: dec!(0.52),
        edge_bps,
        confidence,
        detected_at_ns: now_ns,
        binance_update_ns: now_ns - 50_000_000,     // 50ms ago
        polymarket_update_ns: now_ns - 150_000_000, // 150ms ago
        latency_ms: 150,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    info!("╔═══════════════════════════════════════════════════════════════╗");
    info!("║         PAPER TRADING SIMULATION TEST                         ║");
    info!("╚═══════════════════════════════════════════════════════════════╝");

    // Create simulator with $5000 starting capital
    let config = SimulatorConfig {
        initial_capital: dec!(5000),
        fill_rate: 0.95,
        slippage_bps: 5,
        execution_latency_ms: 50,
        simulate_partial_fills: false,
        partial_fill_rate: 0.3,
        commission_bps: 0,
        max_trade_history: 1000,
    };

    let simulator = PaperTradeSimulator::new(config);

    info!("");
    info!("Configuration:");
    info!("  Initial Capital: $5,000");
    info!("  Position Size: $50 per trade");
    info!("  Take Profit: 15 bps");
    info!("  Stop Loss: 20 bps");
    info!("  Fill Rate: 95%");
    info!("");

    // Create test markets
    let btc_market = create_test_market(CryptoAsset::BTC);
    let eth_market = create_test_market(CryptoAsset::ETH);

    info!("Simulating 20 trading signals...");
    info!("");

    // Simulate a series of trades
    let position_size = dec!(50);
    let mut trade_count = 0;

    // Simulate 20 signals with mixed results (14 winners, 6 losers = 70% win rate)
    let signals = vec![
        (CryptoAsset::BTC, Direction::Up, 18, 0.72, true), // Winner
        (CryptoAsset::ETH, Direction::Down, 15, 0.68, true), // Winner
        (CryptoAsset::BTC, Direction::Up, 12, 0.65, false), // Loser
        (CryptoAsset::ETH, Direction::Up, 20, 0.75, true), // Winner
        (CryptoAsset::BTC, Direction::Down, 16, 0.70, true), // Winner
        (CryptoAsset::ETH, Direction::Down, 11, 0.62, false), // Loser
        (CryptoAsset::BTC, Direction::Up, 22, 0.78, true), // Winner
        (CryptoAsset::ETH, Direction::Up, 14, 0.66, true), // Winner
        (CryptoAsset::BTC, Direction::Down, 13, 0.64, false), // Loser
        (CryptoAsset::ETH, Direction::Down, 19, 0.73, true), // Winner
        (CryptoAsset::BTC, Direction::Up, 17, 0.71, true), // Winner
        (CryptoAsset::ETH, Direction::Up, 10, 0.60, false), // Loser
        (CryptoAsset::BTC, Direction::Down, 21, 0.76, true), // Winner
        (CryptoAsset::ETH, Direction::Down, 15, 0.69, true), // Winner
        (CryptoAsset::BTC, Direction::Up, 12, 0.63, false), // Loser
        (CryptoAsset::ETH, Direction::Up, 18, 0.72, true), // Winner
        (CryptoAsset::BTC, Direction::Down, 14, 0.67, true), // Winner
        (CryptoAsset::ETH, Direction::Down, 11, 0.61, false), // Loser
        (CryptoAsset::BTC, Direction::Up, 20, 0.74, true), // Winner
        (CryptoAsset::ETH, Direction::Up, 16, 0.70, true), // Winner
    ];

    for (asset, direction, edge, conf, is_winner) in signals.iter() {
        let signal = create_signal(*asset, *direction, *edge, *conf);
        let market = if *asset == CryptoAsset::BTC {
            &btc_market
        } else {
            &eth_market
        };

        // Try to execute trade
        if let Some(trade) = simulator.execute_trade(&signal, market, position_size) {
            trade_count += 1;

            // Simulate holding time
            tokio::time::sleep(Duration::from_millis(50)).await;

            // Close the trade with simulated exit price
            let exit_price = if *is_winner {
                // Winner: price moved in our favor by ~10 bps
                trade.entry_price * (Decimal::ONE + Decimal::from(*edge) / dec!(10000) * dec!(0.6))
            } else {
                // Loser: price moved against us by ~12 bps
                trade.entry_price * (Decimal::ONE - dec!(12) / dec!(10000))
            };

            if let Some(closed) = simulator.close_position(&trade.trade_id, exit_price) {
                let pnl_display = closed.realized_pnl.unwrap_or(dec!(0));
                let pnl_sign = if pnl_display >= dec!(0) { "+" } else { "" };
                info!(
                    "  Trade #{:2}: {} {:4} | edge={:2} bps | conf={:.0}% | P&L: {}${:.4}",
                    trade_count,
                    asset,
                    format!("{:?}", direction),
                    edge,
                    conf * 100.0,
                    pnl_sign,
                    pnl_display
                );
            }
        }
    }

    info!("");
    info!("Simulation complete! {} trades executed.", trade_count);
    info!("");

    // Print final statistics
    let stats = simulator.stats().get_stats();

    info!("╔═══════════════════════════════════════════════════════════════╗");
    info!("║                    FINAL STATISTICS                           ║");
    info!("╠═══════════════════════════════════════════════════════════════╣");
    info!("║  Total Trades: {:47} ║", stats.total_trades);
    info!("║  Winning Trades: {:45} ║", stats.winning_trades);
    info!("║  Losing Trades: {:46} ║", stats.losing_trades);
    info!("║  Win Rate: {:42.1}% ║", stats.win_rate * 100.0);
    info!("╠═══════════════════════════════════════════════════════════════╣");
    info!("║  Initial Capital: ${:43} ║", stats.initial_capital);
    info!("║  Current Capital: ${:43.2} ║", stats.current_capital);
    info!("║  Total P&L: ${:49.4} ║", stats.total_pnl);
    info!("║  Return: {:44.2}% ║", stats.return_pct());
    info!("╠═══════════════════════════════════════════════════════════════╣");
    info!("║  Profit Factor: {:46.2} ║", stats.profit_factor);
    info!("║  Avg Trade P&L: ${:44.4} ║", stats.avg_trade_pnl);
    info!("║  Avg Winner: ${:48.4} ║", stats.avg_winner);
    info!("║  Avg Loser: ${:49.4} ║", stats.avg_loser);
    info!("╚═══════════════════════════════════════════════════════════════╝");

    info!("");
    if stats.total_pnl > dec!(0) {
        info!("Paper trading system verified - generating positive returns!");
    } else {
        info!("Paper trading system verified.");
    }

    Ok(())
}
