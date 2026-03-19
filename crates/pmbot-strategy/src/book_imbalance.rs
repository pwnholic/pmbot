//! Book-imbalance strategy — trades based on order-flow imbalance
//! confirmed by short-term price momentum.
//!
//! State machine:
//! ```text
//! Watching → InPosition → Watching
//! ```

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tracing::{debug, info};

use pmbot_core::messages::{Signal, StrategyMetrics, WorldState};
use pmbot_core::types::{ExitReason, FillEvent, MarketId, MarketInfo, Side, SignalId};

use crate::traits::Strategy;

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

/// Internal state of the book-imbalance strategy.
enum BookImbalanceState {
    /// Watching for a strong imbalance with momentum confirmation.
    Watching,
    /// We have an open position triggered by imbalance.
    InPosition {
        signal_id: SignalId,
        entry_imbalance: Decimal,
        entry_price: Decimal,
        side: Side,
    },
}

// ---------------------------------------------------------------------------
// BookImbalance
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub struct BookImbalance {
    /// Absolute imbalance threshold to trigger entry (e.g., 0.6).
    threshold: Decimal,
    /// Number of top order-book levels to consider (for metrics; the
    /// snapshot pre-computes imbalance).
    levels: usize,
    /// Number of recent price points to check for momentum confirmation.
    momentum_window: usize,
    /// Minimum edge to emit in the signal.
    min_activation_edge: Decimal,
    /// Internal state machine.
    state: BookImbalanceState,
    /// Number of signals generated.
    signals_generated: u64,
    /// Number of trades executed.
    trades: u64,
    /// Number of winning trades.
    wins: u64,
    /// Number of losing trades.
    losses: u64,
    /// Total PnL.
    total_pnl: Decimal,
}

impl BookImbalance {
    /// Create a new book-imbalance strategy.
    ///
    /// - `threshold`: minimum absolute imbalance to trigger (e.g., 0.6)
    /// - `levels`: number of book levels used for imbalance calc
    /// - `momentum_window`: number of price points to confirm trend
    /// - `min_activation_edge`: minimum edge value for signal emission
    pub fn new(
        threshold: Decimal,
        levels: usize,
        momentum_window: usize,
        min_activation_edge: Decimal,
    ) -> Self {
        Self {
            threshold,
            levels,
            momentum_window,
            min_activation_edge,
            state: BookImbalanceState::Watching,
            signals_generated: 0,
            trades: 0,
            wins: 0,
            losses: 0,
            total_pnl: Decimal::ZERO,
        }
    }

    /// Check if recent price history confirms momentum in `expected_direction`.
    ///
    /// Returns `true` if the last `window` price points trend in the
    /// expected direction (positive for Buy, negative for Sell).
    fn momentum_confirms(
        history: &[pmbot_core::types::PricePoint],
        window: usize,
        expected_direction: Side,
    ) -> bool {
        if history.len() < 2 || window < 2 {
            return false;
        }

        let n = window.min(history.len());
        let recent = &history[history.len() - n..];

        // Count how many consecutive steps move in the expected direction.
        let mut confirming = 0u32;
        for pair in recent.windows(2) {
            let delta = pair[1].price - pair[0].price;
            let confirms = match expected_direction {
                Side::Buy => delta > Decimal::ZERO,
                Side::Sell => delta < Decimal::ZERO,
            };
            if confirms {
                confirming += 1;
            }
        }

        // Require a majority of steps to confirm.
        confirming > (n as u32 - 1) / 2
    }

    /// State name for metrics display.
    fn state_name(&self) -> &'static str {
        match &self.state {
            BookImbalanceState::Watching => "watching",
            BookImbalanceState::InPosition { .. } => "in_position",
        }
    }
}

impl Strategy for BookImbalance {
    fn name(&self) -> &'static str {
        "book_imbalance"
    }

    fn evaluate(&mut self, world: &WorldState) -> Vec<Signal> {
        // 1. Get the first market.
        let (market_id, snap) = match world
            .active_market_id
            .as_ref()
            .and_then(|id| world.markets.get(id).map(|snap| (id, snap)))
        {
            Some(pair) => pair,
            None => return Vec::new(),
        };

        let token_id = match snap.info.token_ids.first() {
            Some(tid) => tid.clone(),
            None => return Vec::new(),
        };

        let outcome = snap.info.outcomes.first().cloned().unwrap_or_default();

        // 2. Read imbalance from the pre-computed snapshot.
        let imbalance = snap.imbalance;

        debug!(
            imbalance = %imbalance,
            threshold = %self.threshold,
            "book_imbalance: evaluate"
        );

        match &self.state {
            BookImbalanceState::Watching => {
                // 3. Check if imbalance exceeds threshold in either direction.
                let (exceeds, direction) = if imbalance > self.threshold {
                    (true, Side::Buy)
                } else if imbalance < -self.threshold {
                    (true, Side::Sell)
                } else {
                    (false, Side::Buy) // unused
                };

                if !exceeds {
                    return Vec::new();
                }

                // 4. Confirm momentum from price history.
                if !Self::momentum_confirms(&snap.price_history, self.momentum_window, direction) {
                    debug!("book_imbalance: imbalance detected but momentum does not confirm");
                    return Vec::new();
                }

                let edge = imbalance.abs().max(self.min_activation_edge);
                let signal_id = SignalId::new();
                self.signals_generated += 1;
                let entry_price = snap.mid_price.unwrap_or(Decimal::ZERO);

                self.state = BookImbalanceState::InPosition {
                    signal_id,
                    entry_imbalance: imbalance,
                    entry_price,
                    side: direction,
                };

                vec![Signal::Enter {
                    id: signal_id,
                    strategy: "book_imbalance",
                    market_id: market_id.clone(),
                    token_id,
                    outcome,
                    side: direction,
                    size: dec!(1),
                    price: snap.mid_price,
                    edge,
                    confidence: dec!(0.55),
                }]
            }

            BookImbalanceState::InPosition {
                signal_id,
                entry_imbalance,
                ..
            } => {
                // 5. Exit when imbalance reverses (crosses zero).
                let reversed = if *entry_imbalance > Decimal::ZERO {
                    imbalance <= Decimal::ZERO
                } else {
                    imbalance >= Decimal::ZERO
                };

                if reversed {
                    let original_signal_id = *signal_id;
                    let signal_id = SignalId::new();
                    self.signals_generated += 1;

                    self.state = BookImbalanceState::Watching;

                    vec![Signal::Exit {
                        id: signal_id,
                        strategy: "book_imbalance",
                        signal_id: original_signal_id,
                        reason: ExitReason::StrategyExit,
                    }]
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn on_fill(&mut self, fill: &FillEvent) {
        match &self.state {
            BookImbalanceState::InPosition {
                signal_id,
                entry_price,
                side,
                ..
            } => {
                if fill.signal_id == *signal_id {
                    // Entry fill
                    self.trades += 1;
                    info!(
                        strategy = "book_imbalance",
                        ?fill.signal_id,
                        price = %fill.price,
                        "entry order filled"
                    );
                } else {
                    // Exit fill - calculate PnL
                    let pnl = match side {
                        Side::Buy => (fill.price - entry_price) * fill.size,
                        Side::Sell => (entry_price - fill.price) * fill.size,
                    };

                    if pnl >= Decimal::ZERO {
                        self.wins += 1;
                    } else {
                        self.losses += 1;
                    }
                    self.total_pnl += pnl;

                    info!(
                        strategy = "book_imbalance",
                        ?fill.signal_id,
                        pnl = %pnl,
                        total_trades = self.trades,
                        wins = self.wins,
                        losses = self.losses,
                        total_pnl = %self.total_pnl,
                        "position closed"
                    );
                }
            }
            _ => {}
        }
    }

    fn on_market_change(&mut self, _old: &MarketId, _new: &MarketInfo) {
        self.state = BookImbalanceState::Watching;
        debug!("book_imbalance: market changed, resetting state");
    }

    fn metrics(&self) -> StrategyMetrics {
        StrategyMetrics {
            name: "book_imbalance",
            state: self.state_name(),
            edge: match &self.state {
                BookImbalanceState::InPosition {
                    entry_imbalance, ..
                } => Some(entry_imbalance.abs()),
                _ => None,
            },
            signals_generated: self.signals_generated,
            trades: 0,
            wins: 0,
            losses: 0,
            total_pnl: Decimal::ZERO,
            pnl_history: Vec::new(),
            custom: vec![
                ("threshold", format!("{:.2}", self.threshold)),
                ("levels", self.levels.to_string()),
                ("momentum_window", self.momentum_window.to_string()),
            ],
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use pmbot_core::messages::MarketSnapshot;
    use pmbot_core::types::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn ts(secs: i64) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    /// Build a WorldState with configurable imbalance and price history.
    fn make_world(
        mid_price: Option<Decimal>,
        imbalance: Decimal,
        price_history: Vec<(i64, Decimal)>,
    ) -> WorldState {
        let mut markets = HashMap::new();
        let market_id = MarketId("m-1".into());
        let book = OrderbookSnapshot {
            market_id: market_id.clone(),
            token_id: TokenId("tok".into()),
            bids: vec![Level {
                price: dec!(0.50),
                size: dec!(100),
            }],
            asks: vec![Level {
                price: dec!(0.52),
                size: dec!(100),
            }],
            timestamp: ts(0),
        };

        let history: Vec<PricePoint> = price_history
            .into_iter()
            .map(|(secs, price)| PricePoint {
                price,
                timestamp: ts(secs),
            })
            .collect();

        markets.insert(
            market_id.clone(),
            MarketSnapshot {
                info: MarketInfo {
                    id: market_id,
                    question: "Test?".into(),
                    slug: "test".into(),
                    outcomes: vec!["Yes".into(), "No".into()],
                    token_ids: vec![TokenId("tok".into())],
                    condition_id: "cond".into(),
                    neg_risk: false,
                    active: true,
                    end_date: None,
                    liquidity: dec!(10000),
                    volume: dec!(50000),
            outcome_prices: std::collections::HashMap::new(),
            category: "Test".into(),
            tags: vec!["test".into()],
                },
                book: Arc::new(book),
                mid_price,
                spread: Some(dec!(0.02)),
                imbalance,
                price_history: history,
            },
        );

        WorldState {
            active_market_id: Some(MarketId("m-1".into())),
            markets,
            positions: Vec::new(),
            open_orders: Vec::new(),
            balance: dec!(1000),
            daily_pnl: Decimal::ZERO,
            external_prices: HashMap::new(),
            network_latency: HashMap::new(),
            timestamp: ts(0),
        }
    }

    #[test]
    fn test_no_signal_below_threshold() {
        let mut strat = BookImbalance::new(dec!(0.6), 5, 3, dec!(0.01));
        // Imbalance = 0.3 < 0.6 threshold
        let history = vec![(-3, dec!(0.50)), (-2, dec!(0.51)), (-1, dec!(0.52))];
        let world = make_world(Some(dec!(0.51)), dec!(0.3), history);
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_no_signal_without_momentum() {
        let mut strat = BookImbalance::new(dec!(0.6), 5, 3, dec!(0.01));
        // High imbalance (buy signal) but prices are falling (no momentum confirmation)
        let history = vec![(-3, dec!(0.55)), (-2, dec!(0.53)), (-1, dec!(0.50))];
        let world = make_world(Some(dec!(0.51)), dec!(0.7), history);
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_enter_buy_on_positive_imbalance_with_momentum() {
        let mut strat = BookImbalance::new(dec!(0.6), 5, 3, dec!(0.01));
        // High positive imbalance + rising prices
        let history = vec![(-3, dec!(0.49)), (-2, dec!(0.51)), (-1, dec!(0.53))];
        let world = make_world(Some(dec!(0.51)), dec!(0.7), history);
        let signals = strat.evaluate(&world);

        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Enter { strategy, side, .. } => {
                assert_eq!(*strategy, "book_imbalance");
                assert_eq!(*side, Side::Buy);
            }
            other => panic!("expected Enter, got: {other:?}"),
        }
    }

    #[test]
    fn test_enter_sell_on_negative_imbalance_with_momentum() {
        let mut strat = BookImbalance::new(dec!(0.6), 5, 3, dec!(0.01));
        // High negative imbalance + falling prices
        let history = vec![(-3, dec!(0.55)), (-2, dec!(0.53)), (-1, dec!(0.50))];
        let world = make_world(Some(dec!(0.51)), dec!(-0.7), history);
        let signals = strat.evaluate(&world);

        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Enter { strategy, side, .. } => {
                assert_eq!(*strategy, "book_imbalance");
                assert_eq!(*side, Side::Sell);
            }
            other => panic!("expected Enter, got: {other:?}"),
        }
    }

    #[test]
    fn test_exit_on_imbalance_reversal() {
        let mut strat = BookImbalance::new(dec!(0.6), 5, 3, dec!(0.01));

        // Enter: positive imbalance + rising momentum
        let history = vec![(-3, dec!(0.49)), (-2, dec!(0.51)), (-1, dec!(0.53))];
        let world = make_world(Some(dec!(0.51)), dec!(0.7), history.clone());
        let signals = strat.evaluate(&world);
        assert_eq!(signals.len(), 1);
        assert!(matches!(&signals[0], Signal::Enter { .. }));

        // Imbalance reverses to negative → exit
        let world = make_world(Some(dec!(0.51)), dec!(-0.2), history);
        let signals = strat.evaluate(&world);

        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Exit {
                strategy, reason, ..
            } => {
                assert_eq!(*strategy, "book_imbalance");
                assert_eq!(*reason, ExitReason::StrategyExit);
            }
            other => panic!("expected Exit, got: {other:?}"),
        }
    }

    #[test]
    fn test_no_exit_while_imbalance_holds() {
        let mut strat = BookImbalance::new(dec!(0.6), 5, 3, dec!(0.01));

        // Enter
        let history = vec![(-3, dec!(0.49)), (-2, dec!(0.51)), (-1, dec!(0.53))];
        let world = make_world(Some(dec!(0.51)), dec!(0.7), history.clone());
        strat.evaluate(&world);

        // Imbalance still positive → no exit
        let world = make_world(Some(dec!(0.51)), dec!(0.4), history);
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_market_change_resets_state() {
        let mut strat = BookImbalance::new(dec!(0.6), 5, 3, dec!(0.01));

        // Enter
        let history = vec![(-3, dec!(0.49)), (-2, dec!(0.51)), (-1, dec!(0.53))];
        let world = make_world(Some(dec!(0.51)), dec!(0.7), history);
        strat.evaluate(&world);
        assert!(matches!(strat.state, BookImbalanceState::InPosition { .. }));

        let new_market = MarketInfo {
            id: MarketId("m-2".into()),
            question: "New?".into(),
            slug: "new".into(),
            outcomes: vec!["Yes".into(), "No".into()],
            token_ids: vec![TokenId("tok2".into())],
            condition_id: "cond2".into(),
            neg_risk: false,
            active: true,
            end_date: None,
            liquidity: dec!(10000),
            volume: dec!(50000),
            outcome_prices: std::collections::HashMap::new(),
            category: "Test".into(),
            tags: vec!["test".into()],
        };

        strat.on_market_change(&MarketId("m-1".into()), &new_market);
        assert!(matches!(strat.state, BookImbalanceState::Watching));
    }
}
