//! Trading session statistics

use chrono::{DateTime, Utc};
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use serde::Serialize;
use std::collections::VecDeque;

/// Individual trade record
#[derive(Debug, Clone, Serialize)]
pub struct TradeRecord {
    /// Trade ID
    pub trade_id: String,
    /// Entry time
    pub entry_time: DateTime<Utc>,
    /// Exit time
    pub exit_time: DateTime<Utc>,
    /// Trade size (USD)
    pub size_usd: Decimal,
    /// Entry price
    pub entry_price: Decimal,
    /// Exit price
    pub exit_price: Decimal,
    /// P&L amount
    pub pnl: Decimal,
    /// P&L in basis points
    pub pnl_bps: i32,
    /// Expected edge at entry
    pub expected_edge_bps: u32,
    /// Actual latency exploited (ms)
    pub latency_ms: u64,
    /// Slippage experienced (bps)
    pub slippage_bps: i32,
    /// Was this a win?
    pub is_win: bool,
}

/// Session statistics
#[derive(Debug, Clone, Serialize)]
pub struct SessionStats {
    /// Session ID
    pub session_id: String,
    /// Session start time
    pub started_at: DateTime<Utc>,
    /// Last update time
    pub last_update: DateTime<Utc>,
    /// Initial capital
    pub initial_capital: Decimal,
    /// Current capital
    pub current_capital: Decimal,
    /// Total trades executed
    pub total_trades: u64,
    /// Winning trades
    pub winning_trades: u64,
    /// Losing trades
    pub losing_trades: u64,
    /// Total P&L
    pub total_pnl: Decimal,
    /// Gross profit (sum of winning trades)
    pub gross_profit: Decimal,
    /// Gross loss (sum of losing trades)
    pub gross_loss: Decimal,
    /// Win rate (0.0 to 1.0)
    pub win_rate: f64,
    /// Profit factor (gross profit / gross loss)
    pub profit_factor: f64,
    /// Average trade P&L
    pub avg_trade_pnl: Decimal,
    /// Average winner
    pub avg_winner: Decimal,
    /// Average loser
    pub avg_loser: Decimal,
    /// Best trade
    pub max_win: Decimal,
    /// Worst trade
    pub max_loss: Decimal,
    /// Maximum drawdown
    pub max_drawdown: Decimal,
    /// Maximum drawdown percentage
    pub max_drawdown_pct: f64,
    /// Sharpe ratio (annualized, simplified)
    pub sharpe_ratio: f64,
    /// Trades per hour
    pub trades_per_hour: f64,
    /// Signals detected
    pub signals_detected: u64,
    /// Signals executed
    pub signals_executed: u64,
    /// Average edge captured (bps)
    pub avg_edge_captured_bps: f64,
    /// Average latency exploited (ms)
    pub avg_latency_ms: f64,
    /// Average slippage (bps)
    pub avg_slippage_bps: f64,
}

impl SessionStats {
    /// Create new session stats
    pub fn new(session_id: String, initial_capital: Decimal) -> Self {
        let now = Utc::now();
        Self {
            session_id,
            started_at: now,
            last_update: now,
            initial_capital,
            current_capital: initial_capital,
            total_trades: 0,
            winning_trades: 0,
            losing_trades: 0,
            total_pnl: Decimal::ZERO,
            gross_profit: Decimal::ZERO,
            gross_loss: Decimal::ZERO,
            win_rate: 0.0,
            profit_factor: 0.0,
            avg_trade_pnl: Decimal::ZERO,
            avg_winner: Decimal::ZERO,
            avg_loser: Decimal::ZERO,
            max_win: Decimal::ZERO,
            max_loss: Decimal::ZERO,
            max_drawdown: Decimal::ZERO,
            max_drawdown_pct: 0.0,
            sharpe_ratio: 0.0,
            trades_per_hour: 0.0,
            signals_detected: 0,
            signals_executed: 0,
            avg_edge_captured_bps: 0.0,
            avg_latency_ms: 0.0,
            avg_slippage_bps: 0.0,
        }
    }

    /// Get return percentage
    pub fn return_pct(&self) -> f64 {
        if self.initial_capital.is_zero() {
            return 0.0;
        }
        let pnl_f64 = self.total_pnl.to_f64().unwrap_or(0.0);
        let capital_f64 = self.initial_capital.to_f64().unwrap_or(1.0);
        (pnl_f64 / capital_f64) * 100.0
    }

    /// Get session duration in hours
    pub fn duration_hours(&self) -> f64 {
        (Utc::now() - self.started_at).num_seconds() as f64 / 3600.0
    }
}

/// Statistics tracker
pub struct StatsTracker {
    stats: parking_lot::RwLock<SessionStats>,
    trades: parking_lot::RwLock<VecDeque<TradeRecord>>,
    max_history: usize,
    /// For drawdown calculation
    peak_capital: parking_lot::RwLock<Decimal>,
    /// For Sharpe ratio
    returns: parking_lot::RwLock<Vec<f64>>,
}

impl StatsTracker {
    /// Create new tracker
    pub fn new(session_id: String, initial_capital: Decimal, max_history: usize) -> Self {
        Self {
            stats: parking_lot::RwLock::new(SessionStats::new(session_id, initial_capital)),
            trades: parking_lot::RwLock::new(VecDeque::with_capacity(max_history)),
            max_history,
            peak_capital: parking_lot::RwLock::new(initial_capital),
            returns: parking_lot::RwLock::new(Vec::with_capacity(max_history)),
        }
    }

    /// Record a completed trade
    pub fn record_trade(&self, record: TradeRecord) {
        let mut stats = self.stats.write();
        let mut trades = self.trades.write();
        let mut peak = self.peak_capital.write();
        let mut returns = self.returns.write();

        // Update trade counts
        stats.total_trades += 1;
        if record.is_win {
            stats.winning_trades += 1;
            stats.gross_profit += record.pnl;
            if record.pnl > stats.max_win {
                stats.max_win = record.pnl;
            }
        } else {
            stats.losing_trades += 1;
            stats.gross_loss += record.pnl.abs();
            if record.pnl < stats.max_loss {
                stats.max_loss = record.pnl;
            }
        }

        // Update P&L
        stats.total_pnl += record.pnl;
        stats.current_capital += record.pnl;

        // Update peak for drawdown
        if stats.current_capital > *peak {
            *peak = stats.current_capital;
        }

        // Calculate drawdown
        let drawdown = *peak - stats.current_capital;
        if drawdown > stats.max_drawdown {
            stats.max_drawdown = drawdown;
            if !peak.is_zero() {
                let dd_f64 = drawdown.to_f64().unwrap_or(0.0);
                let peak_f64 = peak.to_f64().unwrap_or(1.0);
                stats.max_drawdown_pct = (dd_f64 / peak_f64) * 100.0;
            }
        }

        // Update derived stats
        if stats.total_trades > 0 {
            stats.win_rate = stats.winning_trades as f64 / stats.total_trades as f64;
            stats.avg_trade_pnl = stats.total_pnl / Decimal::from(stats.total_trades);
        }

        if stats.winning_trades > 0 {
            stats.avg_winner = stats.gross_profit / Decimal::from(stats.winning_trades);
        }

        if stats.losing_trades > 0 {
            stats.avg_loser = stats.gross_loss / Decimal::from(stats.losing_trades);
        }

        if !stats.gross_loss.is_zero() {
            let profit_f64: f64 = stats.gross_profit.to_string().parse().unwrap_or(0.0);
            let loss_f64: f64 = stats.gross_loss.to_string().parse().unwrap_or(1.0);
            stats.profit_factor = profit_f64 / loss_f64;
        }

        // Update trades per hour
        let hours = stats.duration_hours();
        if hours > 0.0 {
            stats.trades_per_hour = stats.total_trades as f64 / hours;
        }

        // Track return for Sharpe ratio
        let pnl_f64: f64 = record.pnl.to_string().parse().unwrap_or(0.0);
        let size_f64: f64 = record.size_usd.to_string().parse().unwrap_or(1.0);
        let return_pct = pnl_f64 / size_f64;
        returns.push(return_pct);

        // Update Sharpe ratio (simplified)
        if returns.len() >= 10 {
            let mean: f64 = returns.iter().sum::<f64>() / returns.len() as f64;
            let variance: f64 =
                returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / returns.len() as f64;
            let std_dev = variance.sqrt();
            if std_dev > 0.0 {
                // Annualized (assuming ~250 trading days, ~6.5 hours/day, ~7.8 trades/hour)
                let trades_per_year = 250.0 * 6.5 * stats.trades_per_hour.max(1.0);
                stats.sharpe_ratio = (mean / std_dev) * trades_per_year.sqrt();
            }
        }

        // Update edge and latency averages
        let n = stats.total_trades as f64;
        stats.avg_edge_captured_bps =
            (stats.avg_edge_captured_bps * (n - 1.0) + record.pnl_bps as f64) / n;
        stats.avg_latency_ms = (stats.avg_latency_ms * (n - 1.0) + record.latency_ms as f64) / n;
        stats.avg_slippage_bps =
            (stats.avg_slippage_bps * (n - 1.0) + record.slippage_bps as f64) / n;

        stats.last_update = Utc::now();

        // Add to history
        trades.push_back(record);
        while trades.len() > self.max_history {
            trades.pop_front();
        }
    }

    /// Record signal detected
    pub fn record_signal_detected(&self) {
        self.stats.write().signals_detected += 1;
    }

    /// Record signal executed
    pub fn record_signal_executed(&self) {
        self.stats.write().signals_executed += 1;
    }

    /// Get current stats
    pub fn get_stats(&self) -> SessionStats {
        self.stats.read().clone()
    }

    /// Get recent trades
    pub fn get_trades(&self, limit: usize) -> Vec<TradeRecord> {
        let trades = self.trades.read();
        trades.iter().rev().take(limit).cloned().collect()
    }

    /// Reset stats
    pub fn reset(&self, session_id: String, initial_capital: Decimal) {
        *self.stats.write() = SessionStats::new(session_id, initial_capital);
        self.trades.write().clear();
        *self.peak_capital.write() = initial_capital;
        self.returns.write().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_session_stats_creation() {
        let stats = SessionStats::new("test".to_string(), dec!(5000));
        assert_eq!(stats.initial_capital, dec!(5000));
        assert_eq!(stats.total_trades, 0);
    }

    #[test]
    fn test_stats_tracker() {
        let tracker = StatsTracker::new("test".to_string(), dec!(5000), 100);

        // Record a winning trade
        tracker.record_trade(TradeRecord {
            trade_id: "1".to_string(),
            entry_time: Utc::now(),
            exit_time: Utc::now(),
            size_usd: dec!(50),
            entry_price: dec!(0.50),
            exit_price: dec!(0.51),
            pnl: dec!(2),
            pnl_bps: 200,
            expected_edge_bps: 15,
            latency_ms: 100,
            slippage_bps: 3,
            is_win: true,
        });

        let stats = tracker.get_stats();
        assert_eq!(stats.total_trades, 1);
        assert_eq!(stats.winning_trades, 1);
        assert_eq!(stats.total_pnl, dec!(2));
    }
}
