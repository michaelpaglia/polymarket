//! P&L tracking

use chrono::{DateTime, Utc};
use hft_core::{ArbitrageExecution, ExecutionStatus, MarketId};
use parking_lot::RwLock;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Trade record for P&L tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeRecord {
    pub execution_id: String,
    pub market_id: MarketId,
    pub cost: Decimal,
    pub expected_profit: Decimal,
    pub realized_pnl: Option<Decimal>,
    pub status: ExecutionStatus,
    pub executed_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

impl From<&ArbitrageExecution> for TradeRecord {
    fn from(exec: &ArbitrageExecution) -> Self {
        Self {
            execution_id: exec.execution_id.clone(),
            market_id: exec.opportunity.market_id.clone(),
            cost: exec.total_cost_usd,
            expected_profit: exec.expected_profit_usd,
            realized_pnl: None,
            status: exec.status,
            executed_at: exec.executed_at,
            closed_at: None,
        }
    }
}

/// P&L summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PnlSummary {
    /// Total realized P&L
    pub realized_pnl: Decimal,
    /// Total unrealized P&L
    pub unrealized_pnl: Decimal,
    /// Total P&L (realized + unrealized)
    pub total_pnl: Decimal,
    /// Number of winning trades
    pub wins: u64,
    /// Number of losing trades
    pub losses: u64,
    /// Win rate (0.0 - 1.0)
    pub win_rate: f64,
    /// Total trades
    pub total_trades: u64,
    /// Average profit per trade
    pub avg_profit: Decimal,
    /// Largest win
    pub max_win: Decimal,
    /// Largest loss
    pub max_loss: Decimal,
    /// Profit factor (gross profit / gross loss)
    pub profit_factor: f64,
}

/// P&L tracker
pub struct PnlTracker {
    /// All trades (most recent last)
    trades: RwLock<VecDeque<TradeRecord>>,
    /// Maximum trades to keep in history
    max_history: usize,
    /// Aggregate stats
    stats: RwLock<PnlStats>,
}

#[derive(Debug, Clone, Default)]
struct PnlStats {
    realized_pnl: Decimal,
    unrealized_pnl: Decimal,
    wins: u64,
    losses: u64,
    gross_profit: Decimal,
    gross_loss: Decimal,
    max_win: Decimal,
    max_loss: Decimal,
}

impl PnlTracker {
    /// Create new P&L tracker
    pub fn new(max_history: usize) -> Self {
        Self {
            trades: RwLock::new(VecDeque::with_capacity(max_history)),
            max_history,
            stats: RwLock::new(PnlStats::default()),
        }
    }

    /// Record a new trade
    pub fn record_trade(&self, execution: &ArbitrageExecution) {
        let record = TradeRecord::from(execution);

        let mut trades = self.trades.write();
        trades.push_back(record);

        // Trim history if needed
        while trades.len() > self.max_history {
            trades.pop_front();
        }
    }

    /// Close a trade and record realized P&L
    pub fn close_trade(&self, execution_id: &str, pnl: Decimal) {
        let mut trades = self.trades.write();
        let mut stats = self.stats.write();

        for trade in trades.iter_mut() {
            if trade.execution_id == execution_id {
                trade.realized_pnl = Some(pnl);
                trade.closed_at = Some(Utc::now());

                // Update stats
                stats.realized_pnl += pnl;

                if pnl > Decimal::ZERO {
                    stats.wins += 1;
                    stats.gross_profit += pnl;
                    if pnl > stats.max_win {
                        stats.max_win = pnl;
                    }
                } else if pnl < Decimal::ZERO {
                    stats.losses += 1;
                    stats.gross_loss += pnl.abs();
                    if pnl < stats.max_loss {
                        stats.max_loss = pnl;
                    }
                }

                break;
            }
        }
    }

    /// Update unrealized P&L for open positions
    pub fn update_unrealized(&self, unrealized: Decimal) {
        self.stats.write().unrealized_pnl = unrealized;
    }

    /// Get P&L summary
    pub fn summary(&self) -> PnlSummary {
        let stats = self.stats.read();
        let total_trades = stats.wins + stats.losses;

        let win_rate = if total_trades > 0 {
            stats.wins as f64 / total_trades as f64
        } else {
            0.0
        };

        let avg_profit = if total_trades > 0 {
            stats.realized_pnl / Decimal::from(total_trades)
        } else {
            Decimal::ZERO
        };

        let profit_factor = if stats.gross_loss > Decimal::ZERO {
            (stats.gross_profit / stats.gross_loss)
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0)
        } else if stats.gross_profit > Decimal::ZERO {
            f64::INFINITY
        } else {
            0.0
        };

        PnlSummary {
            realized_pnl: stats.realized_pnl,
            unrealized_pnl: stats.unrealized_pnl,
            total_pnl: stats.realized_pnl + stats.unrealized_pnl,
            wins: stats.wins,
            losses: stats.losses,
            win_rate,
            total_trades,
            avg_profit,
            max_win: stats.max_win,
            max_loss: stats.max_loss,
            profit_factor,
        }
    }

    /// Get recent trades
    pub fn recent_trades(&self, limit: usize) -> Vec<TradeRecord> {
        let trades = self.trades.read();
        trades.iter().rev().take(limit).cloned().collect()
    }

    /// Get all open trades
    pub fn open_trades(&self) -> Vec<TradeRecord> {
        let trades = self.trades.read();
        trades
            .iter()
            .filter(|t| t.closed_at.is_none())
            .cloned()
            .collect()
    }

    /// Reset all stats (for new trading day)
    pub fn reset(&self) {
        *self.stats.write() = PnlStats::default();
    }

    /// Record a paper trade (simulated profit, instantly realized)
    pub fn record_paper_trade(&self, profit: Decimal) {
        let mut stats = self.stats.write();

        stats.realized_pnl += profit;

        if profit > Decimal::ZERO {
            stats.wins += 1;
            stats.gross_profit += profit;
            if profit > stats.max_win {
                stats.max_win = profit;
            }
        } else if profit < Decimal::ZERO {
            stats.losses += 1;
            stats.gross_loss += profit.abs();
            if profit < stats.max_loss {
                stats.max_loss = profit;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hft_core::{ArbitrageOpportunity, TokenId};

    fn make_execution(id: &str, pnl: Decimal) -> ArbitrageExecution {
        ArbitrageExecution {
            execution_id: id.to_string(),
            opportunity: ArbitrageOpportunity {
                opportunity_id: "opp".to_string(),
                market_id: MarketId::new("market"),
                yes_token_id: TokenId::new("yes"),
                no_token_id: TokenId::new("no"),
                yes_price: dec!(0.45),
                no_price: dec!(0.52),
                spread: dec!(0.03),
                profit_bps: 300,
                max_size_usd: dec!(100),
                detected_at_ns: 0,
                confidence: 0.8,
            },
            yes_order_id: Some("yes_order".to_string()),
            no_order_id: Some("no_order".to_string()),
            yes_filled: true,
            no_filled: true,
            yes_fill_price: Some(dec!(0.45)),
            no_fill_price: Some(dec!(0.52)),
            total_cost_usd: dec!(200),
            expected_profit_usd: pnl,
            execution_time_us: 1000,
            executed_at: Utc::now(),
            status: ExecutionStatus::Success,
        }
    }

    #[test]
    fn test_pnl_tracking() {
        let tracker = PnlTracker::new(100);

        // Record trades
        let exec1 = make_execution("1", dec!(3));
        tracker.record_trade(&exec1);
        tracker.close_trade("1", dec!(3));

        let exec2 = make_execution("2", dec!(-1));
        tracker.record_trade(&exec2);
        tracker.close_trade("2", dec!(-1));

        let summary = tracker.summary();
        assert_eq!(summary.realized_pnl, dec!(2)); // 3 - 1
        assert_eq!(summary.wins, 1);
        assert_eq!(summary.losses, 1);
        assert_eq!(summary.win_rate, 0.5);
    }
}
