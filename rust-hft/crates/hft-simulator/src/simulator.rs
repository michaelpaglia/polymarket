//! Paper trading simulator implementation

use crate::{SimulatorConfig, StatsTracker, TradeRecord};
use chrono::Utc;
use dashmap::DashMap;
use hft_core::{CryptoMarket, DirectionalTrade, LatencySignal, Side, TradeStatus};
use parking_lot::RwLock;
use rand::Rng;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use std::sync::Arc;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Simulated order status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimOrderStatus {
    Pending,
    Filled,
    PartialFill,
    Rejected,
    Cancelled,
}

/// Simulated order
#[derive(Debug, Clone)]
pub struct SimulatedOrder {
    pub order_id: String,
    pub trade_id: String,
    pub requested_price: Decimal,
    pub requested_size: Decimal,
    pub fill_price: Option<Decimal>,
    pub filled_size: Option<Decimal>,
    pub status: SimOrderStatus,
    pub slippage_bps: i32,
}

/// Paper trade simulator
pub struct PaperTradeSimulator {
    config: SimulatorConfig,
    /// Stats tracker
    stats: Arc<StatsTracker>,
    /// Open positions
    positions: DashMap<String, DirectionalTrade>,
    /// Current capital
    capital: RwLock<Decimal>,
}

impl PaperTradeSimulator {
    /// Create new simulator
    pub fn new(config: SimulatorConfig) -> Self {
        let initial = config.initial_capital;
        let max_history = config.max_trade_history;
        let session_id = Uuid::new_v4().to_string();

        Self {
            config: config.clone(),
            stats: Arc::new(StatsTracker::new(session_id, initial, max_history)),
            positions: DashMap::new(),
            capital: RwLock::new(initial),
        }
    }

    /// Get stats tracker
    pub fn stats(&self) -> Arc<StatsTracker> {
        self.stats.clone()
    }

    /// Get current capital
    pub fn current_capital(&self) -> Decimal {
        *self.capital.read()
    }

    /// Get open positions count
    pub fn open_positions_count(&self) -> usize {
        self.positions.len()
    }

    /// Get all open positions
    pub fn get_open_positions(&self) -> Vec<DirectionalTrade> {
        self.positions.iter().map(|r| r.value().clone()).collect()
    }

    /// Execute a simulated trade
    pub fn execute_trade(
        &self,
        signal: &LatencySignal,
        market: &CryptoMarket,
        size_usd: Decimal,
    ) -> Option<DirectionalTrade> {
        self.stats.record_signal_detected();

        // Simulate fill probability
        let mut rng = rand::thread_rng();
        let fill_roll: f64 = rng.gen();

        if fill_roll > self.config.fill_rate {
            debug!(
                fill_roll = format!("{:.2}", fill_roll),
                threshold = format!("{:.2}", self.config.fill_rate),
                "Order rejected (simulated fill failure)"
            );
            return None;
        }

        // Simulate slippage (entry slippage is negative - we pay more)
        let slippage_bps = rng.gen_range(0..=self.config.slippage_bps as i32);
        let slippage_pct = Decimal::from(slippage_bps) / Decimal::from(10000);
        let fill_price = signal.polymarket_price * (Decimal::ONE + slippage_pct);

        // Calculate shares
        let shares = size_usd / fill_price;

        // Check if partial fill
        let (final_shares, final_size, status) = if self.config.simulate_partial_fills {
            let partial_roll: f64 = rng.gen();
            if partial_roll < self.config.partial_fill_rate {
                let fill_pct: f64 = rng.gen_range(0.3..0.9);
                let partial_shares =
                    shares * Decimal::from_f64_retain(fill_pct).unwrap_or(dec!(0.5));
                let partial_size =
                    size_usd * Decimal::from_f64_retain(fill_pct).unwrap_or(dec!(0.5));
                (partial_shares, partial_size, TradeStatus::PartialFill)
            } else {
                (shares, size_usd, TradeStatus::Filled)
            }
        } else {
            (shares, size_usd, TradeStatus::Filled)
        };

        // Deduct from capital
        {
            let mut cap = self.capital.write();
            *cap -= final_size;
        }

        let trade = DirectionalTrade {
            trade_id: Uuid::new_v4().to_string(),
            signal_id: signal.signal_id.clone(),
            market_id: market.market_id.clone(),
            asset: signal.asset,
            direction: signal.direction,
            token_id: market.token_for_direction(signal.direction).clone(),
            side: Side::Buy,
            entry_price: fill_price,
            size_usd: final_size,
            shares: final_shares,
            order_id: Some(format!("SIM-{}", Uuid::new_v4())),
            status,
            entry_time: Utc::now(),
            exit_price: None,
            exit_time: None,
            realized_pnl: None,
            expected_edge_bps: signal.edge_bps,
            is_paper: true,
        };

        // Store position
        self.positions.insert(trade.trade_id.clone(), trade.clone());
        self.stats.record_signal_executed();

        info!(
            trade_id = %trade.trade_id,
            asset = %signal.asset,
            direction = %signal.direction,
            entry_price = %fill_price,
            size_usd = %final_size,
            shares = %final_shares,
            slippage_bps = slippage_bps,
            "Paper trade executed"
        );

        Some(trade)
    }

    /// Close a position at current price
    pub fn close_position(
        &self,
        trade_id: &str,
        current_price: Decimal,
    ) -> Option<DirectionalTrade> {
        let mut trade = self.positions.remove(trade_id)?.1;

        // Simulate exit slippage (negative - we get less)
        let mut rng = rand::thread_rng();
        let slippage_bps = rng.gen_range(0..=self.config.slippage_bps as i32);
        let slippage_pct = Decimal::from(slippage_bps) / Decimal::from(10000);
        let exit_price = current_price * (Decimal::ONE - slippage_pct);

        // Calculate P&L
        let gross_pnl = (exit_price - trade.entry_price) * trade.shares;

        // Subtract commission
        let commission =
            trade.size_usd * Decimal::from(self.config.commission_bps) / Decimal::from(10000);
        let net_pnl = gross_pnl - commission;

        trade.exit_price = Some(exit_price);
        trade.exit_time = Some(Utc::now());
        trade.realized_pnl = Some(net_pnl);
        trade.status = TradeStatus::Closed;

        // Return capital + P&L
        {
            let mut cap = self.capital.write();
            *cap += trade.size_usd + net_pnl;
        }

        // Calculate P&L in bps
        let pnl_bps = if !trade.size_usd.is_zero() {
            ((net_pnl / trade.size_usd) * dec!(10000)).to_i32().unwrap_or(0)
        } else {
            0
        };

        // Record trade in stats
        self.stats.record_trade(TradeRecord {
            trade_id: trade.trade_id.clone(),
            entry_time: trade.entry_time,
            exit_time: trade.exit_time.unwrap(),
            size_usd: trade.size_usd,
            entry_price: trade.entry_price,
            exit_price,
            pnl: net_pnl,
            pnl_bps,
            expected_edge_bps: trade.expected_edge_bps,
            latency_ms: 0, // Would need to track this
            slippage_bps,
            is_win: net_pnl > Decimal::ZERO,
        });

        info!(
            trade_id = %trade.trade_id,
            exit_price = %exit_price,
            pnl = %net_pnl,
            pnl_bps = pnl_bps,
            "Paper trade closed"
        );

        Some(trade)
    }

    /// Check and close positions that should exit (take profit, stop loss, timeout)
    pub fn check_exits(
        &self,
        get_price: impl Fn(&DirectionalTrade) -> Option<Decimal>,
        config: &ExitConfig,
    ) {
        let now = Utc::now();
        let mut to_close = Vec::new();

        for entry in self.positions.iter() {
            let trade = entry.value();

            // Get current price for this position's token
            if let Some(current_price) = get_price(trade) {
                let pnl_bps = self.calculate_pnl_bps(trade, current_price);

                // Check take profit
                if pnl_bps >= config.take_profit_bps as i32 {
                    to_close.push((trade.trade_id.clone(), current_price));
                    continue;
                }

                // Check stop loss
                if pnl_bps <= -(config.stop_loss_bps as i32) {
                    to_close.push((trade.trade_id.clone(), current_price));
                    continue;
                }
            }

            // Check max hold time
            let hold_secs = (now - trade.entry_time).num_seconds();
            if hold_secs >= config.max_hold_secs as i64 {
                // Use entry price as fallback
                to_close.push((trade.trade_id.clone(), trade.entry_price));
            }
        }

        // Close positions
        for (trade_id, price) in to_close {
            self.close_position(&trade_id, price);
        }
    }

    fn calculate_pnl_bps(&self, trade: &DirectionalTrade, current_price: Decimal) -> i32 {
        if trade.entry_price.is_zero() {
            return 0;
        }
        let change = (current_price - trade.entry_price) / trade.entry_price;
        (change * dec!(10000)).to_i32().unwrap_or(0)
    }

    /// Reset simulator
    pub fn reset(&self) {
        let session_id = Uuid::new_v4().to_string();
        self.positions.clear();
        *self.capital.write() = self.config.initial_capital;
        self.stats.reset(session_id, self.config.initial_capital);
    }
}

/// Exit configuration
#[derive(Debug, Clone)]
pub struct ExitConfig {
    /// Take profit threshold (bps)
    pub take_profit_bps: u32,
    /// Stop loss threshold (bps)
    pub stop_loss_bps: u32,
    /// Maximum hold time (seconds)
    pub max_hold_secs: u64,
}

impl Default for ExitConfig {
    fn default() -> Self {
        Self {
            take_profit_bps: 15,
            stop_loss_bps: 20,
            max_hold_secs: 180,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hft_core::{Direction, MarketId, TokenId};

    fn create_test_signal() -> LatencySignal {
        LatencySignal {
            signal_id: "test".to_string(),
            asset: hft_core::CryptoAsset::BTC,
            direction: Direction::Up,
            binance_price: dec!(50000),
            price_change_bps: 20,
            polymarket_price: dec!(0.50),
            expected_price: dec!(0.52),
            edge_bps: 15,
            confidence: 0.75,
            detected_at_ns: 0,
            binance_update_ns: 0,
            polymarket_update_ns: 0,
            latency_ms: 100,
        }
    }

    fn create_test_market() -> CryptoMarket {
        CryptoMarket {
            market_id: MarketId::new("test"),
            asset: hft_core::CryptoAsset::BTC,
            up_token_id: TokenId::new("up"),
            down_token_id: TokenId::new("down"),
            strike_price: None,
            start_time: Utc::now() - chrono::Duration::minutes(5),
            end_time: Utc::now() + chrono::Duration::minutes(10),
            discovered_at: Utc::now(),
            question: "Test market".to_string(),
        }
    }

    #[test]
    fn test_simulator_creation() {
        let config = SimulatorConfig::default();
        let sim = PaperTradeSimulator::new(config);
        assert_eq!(sim.current_capital(), dec!(5000));
        assert_eq!(sim.open_positions_count(), 0);
    }

    #[test]
    fn test_execute_trade() {
        let mut config = SimulatorConfig::default();
        config.fill_rate = 1.0; // Always fill
        config.slippage_bps = 0; // No slippage

        let sim = PaperTradeSimulator::new(config);
        let signal = create_test_signal();
        let market = create_test_market();

        let trade = sim.execute_trade(&signal, &market, dec!(50));
        assert!(trade.is_some());

        let trade = trade.unwrap();
        assert_eq!(trade.size_usd, dec!(50));
        assert_eq!(sim.open_positions_count(), 1);
    }
}
