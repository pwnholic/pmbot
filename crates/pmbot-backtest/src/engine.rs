//! Backtest replay engine.
//!
//! [`BacktestEngine`] replays recorded events through strategies,
//! simulates order execution, and produces a [`BacktestReport`].

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use pmbot_core::messages::{MarketSnapshot, Position, WorldState};
use pmbot_core::types::{
    FillEvent, Level, MarketId, MarketInfo, OrderbookSnapshot, PricePoint, PositionId, Side,
    SpotPrice, Symbol, TokenId,
};
use pmbot_strategy::Strategy;

use crate::recorder::{EventReader, RecordedEvent};
use crate::report::{BacktestReport, CompletedTrade, ReportBuilder};
use crate::sim_executor::SimExecutor;

/// Configuration for a backtest run.
pub struct BacktestConfig {
    /// Starting USDC balance.
    pub initial_balance: Decimal,
    /// Fee rate in basis points.
    pub fee_rate_bps: u32,
    /// Slippage in basis points.
    pub slippage_bps: u32,
    /// Tick interval in milliseconds (unused in event-driven replay).
    pub tick_interval_ms: u64,
}

/// The backtest replay engine.
///
/// Feeds recorded events into strategies, executes signals via
/// [`SimExecutor`], and builds a performance report.
pub struct BacktestEngine {
    config: BacktestConfig,
    strategies: Vec<Box<dyn Strategy>>,
    executor: SimExecutor,
    report_builder: ReportBuilder,
}

/// Internal tracking of an open position during backtest.
struct OpenPosition {
    id: PositionId,
    strategy: &'static str,
    market_id: MarketId,
    token_id: TokenId,
    side: Side,
    entry_price: Decimal,
    size: Decimal,
    entry_time: DateTime<Utc>,
}

impl BacktestEngine {
    /// Create a new engine with the given configuration.
    pub fn new(config: BacktestConfig) -> Self {
        let executor =
            SimExecutor::new(config.initial_balance, config.fee_rate_bps, config.slippage_bps);
        let report_builder = ReportBuilder::new(config.initial_balance);
        Self {
            config,
            strategies: Vec::new(),
            executor,
            report_builder,
        }
    }

    /// Register a strategy for the backtest.
    pub fn add_strategy(&mut self, strategy: Box<dyn Strategy>) {
        self.strategies.push(strategy);
    }

    /// Run the backtest on recorded events from a JSONL file.
    pub fn run(&mut self, data_path: &Path) -> anyhow::Result<BacktestReport> {
        let reader = EventReader::open(data_path)?;
        let events: Vec<RecordedEvent> = reader.collect::<Result<Vec<_>, _>>()?;
        Ok(self.run_from_events(events.into_iter()))
    }

    /// Run the backtest on an iterator of events.
    ///
    /// For each event, updates the world state, evaluates all strategies,
    /// attempts to fill generated signals, and tracks positions.
    pub fn run_from_events(
        &mut self,
        events: impl Iterator<Item = RecordedEvent>,
    ) -> BacktestReport {
        let mut books: HashMap<(String, String), (Vec<Level>, Vec<Level>)> = HashMap::new();
        let mut spot_prices: HashMap<Symbol, SpotPrice> = HashMap::new();
        let mut market_snapshots: HashMap<MarketId, MarketSnapshot> = HashMap::new();
        let mut open_positions: Vec<OpenPosition> = Vec::new();
        let mut current_ts = Utc::now();

        // Record initial equity
        self.report_builder
            .add_equity_point(current_ts, self.config.initial_balance);

        for event in events {
            current_ts = event.timestamp();

            // Update state from event
            match &event {
                RecordedEvent::Book {
                    market,
                    token,
                    bids,
                    asks,
                    ts,
                    ..
                } => {
                    books.insert(
                        (market.clone(), token.clone()),
                        (bids.clone(), asks.clone()),
                    );

                    let market_id = MarketId(market.clone());
                    let token_id = TokenId(token.clone());
                    let book = OrderbookSnapshot {
                        market_id: market_id.clone(),
                        token_id,
                        bids: bids.clone(),
                        asks: asks.clone(),
                        timestamp: *ts,
                    };
                    let mid = book.mid_price();
                    let spread = book.spread();
                    let imbalance =
                        pmbot_core::math::book_imbalance(bids, asks, bids.len().max(asks.len()));

                    let snapshot = market_snapshots
                        .entry(market_id.clone())
                        .or_insert_with(|| MarketSnapshot {
                            info: MarketInfo {
                                id: market_id.clone(),
                                question: String::new(),
                                slug: String::new(),
                                outcomes: vec![],
                                token_ids: vec![],
                                condition_id: String::new(),
                                neg_risk: false,
                                active: true,
                                end_date: None,
                                liquidity: Decimal::ZERO,
                                volume: Decimal::ZERO,
                            },
                            book: Arc::new(book.clone()),
                            mid_price: mid,
                            spread,
                            imbalance,
                            price_history: vec![],
                        });

                    snapshot.book = Arc::new(book);
                    snapshot.mid_price = mid;
                    snapshot.spread = spread;
                    snapshot.imbalance = imbalance;

                    if let Some(mid_val) = mid {
                        snapshot.price_history.push(PricePoint {
                            price: mid_val,
                            timestamp: *ts,
                        });
                    }
                }
                RecordedEvent::Spot {
                    symbol,
                    price,
                    ts,
                    ..
                } => {
                    spot_prices.insert(
                        Symbol(symbol.clone()),
                        SpotPrice {
                            price: *price,
                            timestamp: *ts,
                        },
                    );
                }
            }

            // Build world state
            let positions: Vec<Position> = open_positions
                .iter()
                .map(|op| Position {
                    id: op.id,
                    market_id: op.market_id.clone(),
                    token_id: op.token_id.clone(),
                    strategy: op.strategy,
                    side: op.side,
                    entry_price: op.entry_price,
                    size: op.size,
                    tp_price: Decimal::ZERO,
                    sl_price: Decimal::ZERO,
                    opened_at: op.entry_time,
                    unrealized_pnl: Decimal::ZERO,
                })
                .collect();

            let world = WorldState {
                markets: market_snapshots.clone(),
                positions,
                open_orders: vec![],
                balance: self.executor.balance(),
                daily_pnl: Decimal::ZERO,
                external_prices: spot_prices.clone(),
                timestamp: current_ts,
            };

            // Evaluate all strategies
            let mut all_signals = Vec::new();
            for strategy in &mut self.strategies {
                let signals = strategy.evaluate(&world);
                all_signals.extend(signals);
            }

            // Process signals
            for signal in &all_signals {
                match signal {
                    pmbot_core::messages::Signal::Enter {
                        id,
                        strategy,
                        market_id,
                        token_id,
                        side,
                        ..
                    } => {
                        // Get the book for this market
                        let book_key = (market_id.0.clone(), token_id.0.clone());
                        let (bids, asks) = books
                            .get(&book_key)
                            .cloned()
                            .unwrap_or_else(|| (vec![], vec![]));

                        if let Some(fill) = self.executor.try_fill(signal, &bids, &asks) {
                            let pos = OpenPosition {
                                id: PositionId::new(),
                                strategy,
                                market_id: market_id.clone(),
                                token_id: token_id.clone(),
                                side: *side,
                                entry_price: fill.price,
                                size: fill.size,
                                entry_time: current_ts,
                            };
                            open_positions.push(pos);

                            // Notify strategy of fill
                            let fill_event = FillEvent {
                                order_id: fill.order_id,
                                signal_id: *id,
                                market_id: market_id.clone(),
                                side: *side,
                                price: fill.price,
                                size: fill.size,
                                timestamp: current_ts,
                            };
                            for strat in &mut self.strategies {
                                if strat.name() == *strategy {
                                    strat.on_fill(&fill_event);
                                }
                            }
                        }
                    }
                    pmbot_core::messages::Signal::Exit {
                        strategy,
                        position_id,
                        ..
                    } => {
                        // Find and close the position
                        if let Some(idx) = open_positions.iter().position(|p| p.id == *position_id)
                        {
                            let pos = open_positions.remove(idx);

                            // Get the book to determine exit price
                            let book_key = (pos.market_id.0.clone(), pos.token_id.0.clone());
                            let (bids, asks) = books
                                .get(&book_key)
                                .cloned()
                                .unwrap_or_else(|| (vec![], vec![]));

                            // Exit price: if we bought, we sell (use best bid);
                            // if we sold, we buy back (use best ask)
                            let exit_price = match pos.side {
                                Side::Buy => bids.first().map(|l| l.price),
                                Side::Sell => asks.first().map(|l| l.price),
                            };

                            if let Some(exit_px) = exit_price {
                                let pnl = match pos.side {
                                    Side::Buy => (exit_px - pos.entry_price) * pos.size,
                                    Side::Sell => (pos.entry_price - exit_px) * pos.size,
                                };

                                let trade = CompletedTrade {
                                    strategy: strategy.to_string(),
                                    side: format!("{}", pos.side),
                                    entry_price: pos.entry_price,
                                    exit_price: exit_px,
                                    size: pos.size,
                                    pnl,
                                    fees: Decimal::ZERO, // fees already accounted in executor
                                    entry_time: pos.entry_time,
                                    exit_time: current_ts,
                                };
                                self.report_builder.add_trade(trade);
                            }
                        }
                        let _ = strategy; // suppress unused warning
                    }
                    _ => {} // Amend/CancelAll not simulated
                }
            }

            // Record equity point
            self.report_builder
                .add_equity_point(current_ts, self.executor.balance());
        }

        self.report_builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pmbot_core::messages::{Signal, StrategyMetrics};
    use pmbot_core::types::*;
    use rust_decimal_macros::dec;

    /// A simple test strategy that buys whenever it sees a market.
    struct AlwaysBuyStrategy {
        filled: bool,
    }

    impl AlwaysBuyStrategy {
        fn new() -> Self {
            Self { filled: false }
        }
    }

    impl Strategy for AlwaysBuyStrategy {
        fn name(&self) -> &'static str {
            "always_buy"
        }

        fn evaluate(&mut self, world: &WorldState) -> Vec<Signal> {
            if self.filled {
                return vec![];
            }

            let mut signals = Vec::new();
            for (market_id, snapshot) in &world.markets {
                if snapshot.book.asks.is_empty() {
                    continue;
                }
                let token_id = snapshot
                    .book
                    .token_id
                    .clone();
                signals.push(Signal::Enter {
                    id: SignalId::new(),
                    strategy: "always_buy",
                    market_id: market_id.clone(),
                    token_id,
                    side: Side::Buy,
                    size: dec!(10),
                    price: None,
                    edge: dec!(0.05),
                    confidence: dec!(0.9),
                });
            }
            signals
        }

        fn on_fill(&mut self, _fill: &FillEvent) {
            self.filled = true;
        }

        fn on_market_change(&mut self, _old: &MarketId, _new: &MarketInfo) {}

        fn metrics(&self) -> StrategyMetrics {
            StrategyMetrics {
                name: "always_buy",
                state: "active",
                edge: None,
                signals_generated: 0,
                custom: vec![],
            }
        }
    }

    /// Strategy that does nothing (used for baseline testing).
    struct NoopStrategy;

    impl Strategy for NoopStrategy {
        fn name(&self) -> &'static str {
            "noop"
        }
        fn evaluate(&mut self, _world: &WorldState) -> Vec<Signal> {
            vec![]
        }
        fn on_fill(&mut self, _fill: &FillEvent) {}
        fn on_market_change(&mut self, _old: &MarketId, _new: &MarketInfo) {}
        fn metrics(&self) -> StrategyMetrics {
            StrategyMetrics {
                name: "noop",
                state: "active",
                edge: None,
                signals_generated: 0,
                custom: vec![],
            }
        }
    }

    fn make_book_events() -> Vec<RecordedEvent> {
        let ts1 = chrono::TimeZone::with_ymd_and_hms(&Utc, 2025, 1, 1, 10, 0, 0).unwrap();
        let ts2 = chrono::TimeZone::with_ymd_and_hms(&Utc, 2025, 1, 1, 10, 1, 0).unwrap();
        let ts3 = chrono::TimeZone::with_ymd_and_hms(&Utc, 2025, 1, 1, 10, 2, 0).unwrap();

        vec![
            RecordedEvent::Book {
                ts: ts1,
                market: "m1".into(),
                token: "t1".into(),
                bids: vec![Level {
                    price: dec!(0.48),
                    size: dec!(100),
                }],
                asks: vec![Level {
                    price: dec!(0.52),
                    size: dec!(100),
                }],
            },
            RecordedEvent::Book {
                ts: ts2,
                market: "m1".into(),
                token: "t1".into(),
                bids: vec![Level {
                    price: dec!(0.50),
                    size: dec!(100),
                }],
                asks: vec![Level {
                    price: dec!(0.54),
                    size: dec!(100),
                }],
            },
            RecordedEvent::Book {
                ts: ts3,
                market: "m1".into(),
                token: "t1".into(),
                bids: vec![Level {
                    price: dec!(0.52),
                    size: dec!(100),
                }],
                asks: vec![Level {
                    price: dec!(0.56),
                    size: dec!(100),
                }],
            },
        ]
    }

    #[test]
    fn test_engine_no_strategies() {
        let config = BacktestConfig {
            initial_balance: dec!(1000),
            fee_rate_bps: 0,
            slippage_bps: 0,
            tick_interval_ms: 1000,
        };
        let mut engine = BacktestEngine::new(config);
        let report = engine.run_from_events(make_book_events().into_iter());

        assert_eq!(report.trade_count, 0);
        assert_eq!(report.total_pnl, dec!(0));
    }

    #[test]
    fn test_engine_noop_strategy() {
        let config = BacktestConfig {
            initial_balance: dec!(1000),
            fee_rate_bps: 0,
            slippage_bps: 0,
            tick_interval_ms: 1000,
        };
        let mut engine = BacktestEngine::new(config);
        engine.add_strategy(Box::new(NoopStrategy));

        let report = engine.run_from_events(make_book_events().into_iter());
        assert_eq!(report.trade_count, 0);
    }

    #[test]
    fn test_engine_always_buy_fills() {
        let config = BacktestConfig {
            initial_balance: dec!(1000),
            fee_rate_bps: 0,
            slippage_bps: 0,
            tick_interval_ms: 1000,
        };
        let mut engine = BacktestEngine::new(config);
        engine.add_strategy(Box::new(AlwaysBuyStrategy::new()));

        let report = engine.run_from_events(make_book_events().into_iter());

        // Strategy buys on first event at 0.52, size=10 -> cost = 5.20
        // Balance should decrease
        assert!(report.equity_curve.len() >= 3);
        // The balance after first buy should be < 1000
        assert!(report.equity_curve[1].equity < dec!(1000));
    }

    #[test]
    fn test_engine_from_file() {
        use crate::recorder::EventRecorder;

        let path = std::env::temp_dir().join(format!(
            "pmbot_engine_test_{}.jsonl",
            std::process::id()
        ));

        let mut recorder = EventRecorder::new(&path).unwrap();
        for event in make_book_events() {
            recorder.record(&event).unwrap();
        }
        recorder.flush().unwrap();

        let config = BacktestConfig {
            initial_balance: dec!(1000),
            fee_rate_bps: 0,
            slippage_bps: 0,
            tick_interval_ms: 1000,
        };
        let mut engine = BacktestEngine::new(config);
        engine.add_strategy(Box::new(NoopStrategy));

        let report = engine.run(&path).unwrap();
        assert_eq!(report.trade_count, 0);

        let _ = std::fs::remove_file(&path);
    }
}
