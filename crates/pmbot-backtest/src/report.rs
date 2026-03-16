//! Backtest performance reporting.
//!
//! [`ReportBuilder`] accumulates completed trades and equity points,
//! then computes a [`BacktestReport`] with key performance metrics.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

/// A completed round-trip trade for reporting.
#[derive(Debug, Clone, Serialize)]
pub struct CompletedTrade {
    /// Strategy that generated the trade.
    pub strategy: String,
    /// Trade side (buy/sell).
    pub side: String,
    /// Entry fill price.
    pub entry_price: Decimal,
    /// Exit fill price.
    pub exit_price: Decimal,
    /// Trade size.
    pub size: Decimal,
    /// Realized PnL (before fees).
    pub pnl: Decimal,
    /// Total fees paid.
    pub fees: Decimal,
    /// Entry timestamp.
    pub entry_time: DateTime<Utc>,
    /// Exit timestamp.
    pub exit_time: DateTime<Utc>,
}

/// Full backtest performance report.
#[derive(Debug, Clone, Serialize)]
pub struct BacktestReport {
    /// Gross PnL across all trades.
    pub total_pnl: Decimal,
    /// Total fees paid.
    pub total_fees: Decimal,
    /// Net PnL (total_pnl - total_fees).
    pub net_pnl: Decimal,
    /// Number of completed trades.
    pub trade_count: usize,
    /// Number of winning trades.
    pub win_count: usize,
    /// Number of losing trades.
    pub loss_count: usize,
    /// Win rate as a decimal (0-1).
    pub win_rate: Decimal,
    /// Gross profit / gross loss ratio.
    pub profit_factor: Decimal,
    /// Maximum peak-to-trough drawdown.
    pub max_drawdown: Decimal,
    /// Annualized Sharpe ratio.
    pub sharpe_ratio: Decimal,
    /// Per-strategy breakdowns.
    pub per_strategy: Vec<StrategyReport>,
    /// Equity curve over time.
    pub equity_curve: Vec<EquityPoint>,
}

/// Per-strategy performance summary.
#[derive(Debug, Clone, Serialize)]
pub struct StrategyReport {
    /// Strategy name.
    pub name: String,
    /// Net PnL for this strategy.
    pub pnl: Decimal,
    /// Number of trades.
    pub trades: usize,
    /// Win rate for this strategy.
    pub win_rate: Decimal,
}

/// A point on the equity curve.
#[derive(Debug, Clone, Serialize)]
pub struct EquityPoint {
    /// Timestamp.
    pub timestamp: DateTime<Utc>,
    /// Equity value at this point.
    pub equity: Decimal,
}

/// Builder that accumulates trades and computes the final report.
pub struct ReportBuilder {
    trades: Vec<CompletedTrade>,
    equity_curve: Vec<EquityPoint>,
    _initial_balance: Decimal,
}

impl ReportBuilder {
    /// Create a new report builder with the given initial balance.
    pub fn new(initial_balance: Decimal) -> Self {
        Self {
            trades: Vec::new(),
            equity_curve: Vec::new(),
            _initial_balance: initial_balance,
        }
    }

    /// Add a completed trade.
    pub fn add_trade(&mut self, trade: CompletedTrade) {
        self.trades.push(trade);
    }

    /// Add an equity curve data point.
    pub fn add_equity_point(&mut self, ts: DateTime<Utc>, equity: Decimal) {
        self.equity_curve.push(EquityPoint {
            timestamp: ts,
            equity,
        });
    }

    /// Build the final report from accumulated data.
    pub fn build(&self) -> BacktestReport {
        let total_pnl: Decimal = self.trades.iter().map(|t| t.pnl).sum();
        let total_fees: Decimal = self.trades.iter().map(|t| t.fees).sum();
        let net_pnl = total_pnl - total_fees;
        let trade_count = self.trades.len();

        let win_count = self.trades.iter().filter(|t| t.pnl > Decimal::ZERO).count();
        let loss_count = self.trades.iter().filter(|t| t.pnl < Decimal::ZERO).count();

        let win_rate = if trade_count > 0 {
            Decimal::new(win_count as i64, 0) / Decimal::new(trade_count as i64, 0)
        } else {
            Decimal::ZERO
        };

        let gross_profit: Decimal = self
            .trades
            .iter()
            .filter(|t| t.pnl > Decimal::ZERO)
            .map(|t| t.pnl)
            .sum();
        let gross_loss: Decimal = self
            .trades
            .iter()
            .filter(|t| t.pnl < Decimal::ZERO)
            .map(|t| t.pnl.abs())
            .sum();

        let profit_factor = if gross_loss.is_zero() {
            Decimal::MAX
        } else {
            gross_profit / gross_loss
        };

        let max_drawdown = self.compute_max_drawdown();
        let sharpe_ratio = self.compute_sharpe_ratio();

        let per_strategy = self.compute_per_strategy();

        BacktestReport {
            total_pnl,
            total_fees,
            net_pnl,
            trade_count,
            win_count,
            loss_count,
            win_rate,
            profit_factor,
            max_drawdown,
            sharpe_ratio,
            per_strategy,
            equity_curve: self.equity_curve.clone(),
        }
    }

    /// Compute maximum peak-to-trough drawdown from the equity curve.
    fn compute_max_drawdown(&self) -> Decimal {
        if self.equity_curve.is_empty() {
            return Decimal::ZERO;
        }

        let mut peak = self.equity_curve[0].equity;
        let mut max_dd = Decimal::ZERO;

        for point in &self.equity_curve {
            if point.equity > peak {
                peak = point.equity;
            }
            let dd = peak - point.equity;
            if dd > max_dd {
                max_dd = dd;
            }
        }

        max_dd
    }

    /// Compute annualized Sharpe ratio from trade PnLs.
    ///
    /// Uses f64 intermediates: `mean(returns) / stddev(returns) * sqrt(252)`.
    fn compute_sharpe_ratio(&self) -> Decimal {
        if self.trades.len() < 2 {
            return Decimal::ZERO;
        }

        let returns: Vec<f64> = self
            .trades
            .iter()
            .map(|t| {
                let net = t.pnl - t.fees;
                // Use f64 for statistical computation
                f64::try_from(net).unwrap_or(0.0)
            })
            .collect();

        let n = returns.len() as f64;
        let mean = returns.iter().sum::<f64>() / n;
        let variance = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0);
        let stddev = variance.sqrt();

        if stddev < f64::EPSILON {
            return Decimal::ZERO;
        }

        let sharpe = (mean / stddev) * 252.0_f64.sqrt();
        Decimal::try_from(sharpe).unwrap_or(Decimal::ZERO)
    }

    /// Build per-strategy performance summaries.
    fn compute_per_strategy(&self) -> Vec<StrategyReport> {
        let mut by_strat: HashMap<&str, Vec<&CompletedTrade>> = HashMap::new();
        for trade in &self.trades {
            by_strat
                .entry(trade.strategy.as_str())
                .or_default()
                .push(trade);
        }

        let mut reports: Vec<StrategyReport> = by_strat
            .into_iter()
            .map(|(name, trades)| {
                let pnl: Decimal = trades.iter().map(|t| t.pnl - t.fees).sum();
                let total = trades.len();
                let wins = trades.iter().filter(|t| t.pnl > Decimal::ZERO).count();
                let wr = if total > 0 {
                    Decimal::new(wins as i64, 0) / Decimal::new(total as i64, 0)
                } else {
                    Decimal::ZERO
                };

                StrategyReport {
                    name: name.to_string(),
                    pnl,
                    trades: total,
                    win_rate: wr,
                }
            })
            .collect();

        reports.sort_by(|a, b| a.name.cmp(&b.name));
        reports
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    fn ts(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 1, hour, 0, 0).unwrap()
    }

    fn make_trade(strategy: &str, pnl: Decimal, fees: Decimal, hour: u32) -> CompletedTrade {
        CompletedTrade {
            strategy: strategy.into(),
            side: "Buy".into(),
            entry_price: dec!(0.50),
            exit_price: dec!(0.55),
            size: dec!(100),
            pnl,
            fees,
            entry_time: ts(hour),
            exit_time: ts(hour + 1),
        }
    }

    #[test]
    fn test_empty_report() {
        let builder = ReportBuilder::new(dec!(1000));
        let report = builder.build();

        assert_eq!(report.total_pnl, dec!(0));
        assert_eq!(report.trade_count, 0);
        assert_eq!(report.win_rate, dec!(0));
        assert_eq!(report.max_drawdown, dec!(0));
    }

    #[test]
    fn test_pnl_and_fees() {
        let mut builder = ReportBuilder::new(dec!(1000));
        builder.add_trade(make_trade("strat_a", dec!(10), dec!(1), 1));
        builder.add_trade(make_trade("strat_a", dec!(-5), dec!(1), 2));

        let report = builder.build();
        assert_eq!(report.total_pnl, dec!(5));
        assert_eq!(report.total_fees, dec!(2));
        assert_eq!(report.net_pnl, dec!(3));
    }

    #[test]
    fn test_win_rate_and_counts() {
        let mut builder = ReportBuilder::new(dec!(1000));
        builder.add_trade(make_trade("s", dec!(10), dec!(0), 1));
        builder.add_trade(make_trade("s", dec!(5), dec!(0), 2));
        builder.add_trade(make_trade("s", dec!(-3), dec!(0), 3));

        let report = builder.build();
        assert_eq!(report.trade_count, 3);
        assert_eq!(report.win_count, 2);
        assert_eq!(report.loss_count, 1);
        // win_rate = 2/3
        assert!(report.win_rate > dec!(0.66));
        assert!(report.win_rate < dec!(0.67));
    }

    #[test]
    fn test_max_drawdown() {
        let mut builder = ReportBuilder::new(dec!(1000));
        builder.add_equity_point(ts(0), dec!(1000));
        builder.add_equity_point(ts(1), dec!(1050)); // new peak
        builder.add_equity_point(ts(2), dec!(1020)); // dd = 30
        builder.add_equity_point(ts(3), dec!(1000)); // dd = 50
        builder.add_equity_point(ts(4), dec!(1060)); // new peak

        let report = builder.build();
        assert_eq!(report.max_drawdown, dec!(50));
    }

    #[test]
    fn test_per_strategy_breakdown() {
        let mut builder = ReportBuilder::new(dec!(1000));
        builder.add_trade(make_trade("alpha", dec!(10), dec!(1), 1));
        builder.add_trade(make_trade("alpha", dec!(-5), dec!(1), 2));
        builder.add_trade(make_trade("beta", dec!(20), dec!(2), 3));

        let report = builder.build();
        assert_eq!(report.per_strategy.len(), 2);

        let alpha = report
            .per_strategy
            .iter()
            .find(|s| s.name == "alpha")
            .unwrap();
        assert_eq!(alpha.trades, 2);
        // alpha net pnl = (10-1) + (-5-1) = 3
        assert_eq!(alpha.pnl, dec!(3));

        let beta = report
            .per_strategy
            .iter()
            .find(|s| s.name == "beta")
            .unwrap();
        assert_eq!(beta.trades, 1);
        // beta net pnl = 20-2 = 18
        assert_eq!(beta.pnl, dec!(18));
    }
}
