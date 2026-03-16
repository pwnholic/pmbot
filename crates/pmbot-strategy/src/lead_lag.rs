//! Lead-Lag strategy — trades Polymarket markets based on Binance
//! BTC price movements that have not yet been reflected in the
//! prediction market mid-price.
//!
//! State machine:
//! ```text
//! Watching → SignalDetected → InPosition → Cooldown → Watching
//! ```

use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tracing::debug;

use pmbot_core::messages::{Signal, StrategyMetrics, WorldState};
use pmbot_core::types::{
    ExitReason, FillEvent, MarketId, MarketInfo, Side, SignalId, Symbol,
};

use crate::traits::Strategy;

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

/// Internal state of the lead-lag strategy.
#[derive(Debug)]
enum State {
    /// Watching for a significant BTC move.
    Watching,
    /// A move was detected; waiting for the entry delay to elapse.
    SignalDetected {
        direction: Side,
        detected_at: Instant,
        edge: Decimal,
    },
    /// We have an open position and are waiting for convergence.
    InPosition {
        entry_edge: Decimal,
        signal_id: SignalId,
    },
    /// Cooldown after exiting a position.
    Cooldown { until: Instant },
}

// ---------------------------------------------------------------------------
// LeadLag
// ---------------------------------------------------------------------------

/// Lead-lag strategy implementation.
///
/// Compares external BTC price (from feed) with the prediction market
/// mid-price. When BTC moves significantly, the strategy expects the
/// Polymarket price to follow with a lag.
pub struct LeadLag {
    /// Minimum relative BTC move to trigger a signal.
    lag_threshold: Decimal,
    /// Delay before entering after signal detection.
    entry_delay: Duration,
    /// Convergence threshold to trigger exit (relative gap).
    exit_convergence: Decimal,
    /// Cooldown period after exiting.
    cooldown: Duration,
    /// The BTC anchor symbol to watch in external prices.
    anchor_symbol: Symbol,
    /// Internal state machine.
    state: State,
    /// BTC price at the time the strategy started watching.
    anchor_price: Option<Decimal>,
    /// Number of signals generated (for metrics).
    signals_generated: u64,
}

impl LeadLag {
    /// Create a new lead-lag strategy.
    ///
    /// - `lag_threshold`: minimum relative BTC move (e.g., 0.02 = 2%)
    /// - `entry_delay_ms`: milliseconds to wait after detection before entering
    /// - `exit_convergence`: relative gap threshold to trigger exit
    pub fn new(lag_threshold: Decimal, entry_delay_ms: u64, exit_convergence: Decimal) -> Self {
        Self {
            lag_threshold,
            entry_delay: Duration::from_millis(entry_delay_ms),
            exit_convergence,
            cooldown: Duration::from_secs(5),
            anchor_symbol: Symbol("BTCUSDT".into()),
            state: State::Watching,
            anchor_price: None,
            signals_generated: 0,
        }
    }

    /// State name for metrics display.
    fn state_name(&self) -> &'static str {
        match &self.state {
            State::Watching => "watching",
            State::SignalDetected { .. } => "signal_detected",
            State::InPosition { .. } => "in_position",
            State::Cooldown { .. } => "cooldown",
        }
    }
}

impl Strategy for LeadLag {
    fn name(&self) -> &'static str {
        "lead_lag"
    }

    fn evaluate(&mut self, world: &WorldState) -> Vec<Signal> {
        // Get the BTC spot price from external feeds.
        let btc_price = match world.external_prices.get(&self.anchor_symbol) {
            Some(sp) => sp.price,
            None => return Vec::new(),
        };

        // Get the first market (our trading target).
        let (market_id, snap) = match world.active_market_id.as_ref().and_then(|id| world.markets.get(id).map(|snap| (id, snap))) {
            Some(pair) => pair,
            None => return Vec::new(),
        };

        let token_id = match snap.info.token_ids.first() {
            Some(tid) => tid.clone(),
            None => return Vec::new(),
        };

        // Set anchor on first observation.
        if self.anchor_price.is_none() {
            self.anchor_price = Some(btc_price);
            debug!(anchor = %btc_price, "lead_lag: set BTC anchor");
            return Vec::new();
        }

        let anchor = self.anchor_price.unwrap();
        if anchor.is_zero() {
            return Vec::new();
        }

        let btc_move = (btc_price - anchor) / anchor;

        match &self.state {
            State::Watching => {
                if btc_move.abs() > self.lag_threshold {
                    let direction = if btc_move > Decimal::ZERO {
                        Side::Buy
                    } else {
                        Side::Sell
                    };
                    let edge = btc_move.abs();
                    debug!(
                        btc_move = %btc_move,
                        direction = %direction,
                        "lead_lag: signal detected"
                    );

                    // If delay is zero, enter immediately without
                    // going through the SignalDetected state.
                    if self.entry_delay.is_zero() {
                        let signal_id = SignalId::new();
                        self.signals_generated += 1;
                        self.state = State::InPosition {
                            entry_edge: edge,
                            signal_id,
                        };
                        return vec![Signal::Enter {
                            id: signal_id,
                            strategy: "lead_lag",
                            market_id: market_id.clone(),
                            token_id,
                            side: direction,
                            size: dec!(1),
                            price: snap.mid_price,
                            edge,
                            confidence: dec!(0.70),
                        }];
                    }

                    self.state = State::SignalDetected {
                        direction,
                        detected_at: Instant::now(),
                        edge,
                    };
                }
                Vec::new()
            }

            State::SignalDetected {
                direction,
                detected_at,
                edge,
            } => {
                if detected_at.elapsed() >= self.entry_delay {
                    let direction = *direction;
                    let edge = *edge;
                    let signal_id = SignalId::new();
                    self.signals_generated += 1;

                    self.state = State::InPosition {
                        entry_edge: edge,
                        signal_id,
                    };

                    vec![Signal::Enter {
                        id: signal_id,
                        strategy: "lead_lag",
                        market_id: market_id.clone(),
                        token_id,
                        side: direction,
                        size: dec!(1), // Position sizing is the risk actor's job.
                        price: snap.mid_price,
                        edge,
                        confidence: dec!(0.70),
                    }]
                } else {
                    Vec::new()
                }
            }

            State::InPosition {
                entry_edge: _,
                signal_id,
            } => {
                // Check if the BTC move has converged back.
                if btc_move.abs() < self.exit_convergence {
                    let original_signal_id = *signal_id;
                    let signal_id = SignalId::new();
                    self.signals_generated += 1;

                    self.state = State::Cooldown {
                        until: Instant::now() + self.cooldown,
                    };

                    // Reset anchor for next cycle.
                    self.anchor_price = Some(btc_price);

                    vec![Signal::Exit {
                        id: signal_id,
                        strategy: "lead_lag",
                        signal_id: original_signal_id,
                        reason: ExitReason::StrategyExit,
                    }]
                } else {
                    Vec::new()
                }
            }

            State::Cooldown { until } => {
                if Instant::now() >= *until {
                    self.state = State::Watching;
                    // Reset anchor for next cycle.
                    self.anchor_price = Some(btc_price);
                }
                Vec::new()
            }
        }
    }

    fn on_fill(&mut self, _fill: &FillEvent) {
        // The risk actor manages position tracking; we just note
        // that our signal was executed.
        debug!("lead_lag: fill received");
    }

    fn on_market_change(&mut self, _old: &MarketId, _new: &MarketInfo) {
        // Reset state on market rotation.
        self.state = State::Watching;
        self.anchor_price = None;
        debug!("lead_lag: market changed, resetting state");
    }

    fn metrics(&self) -> StrategyMetrics {
        StrategyMetrics {
            name: "lead_lag",
            state: self.state_name(),
            edge: self.anchor_price.map(|_| {
                // Report current edge if we're in a signal state.
                match &self.state {
                    State::SignalDetected { edge, .. } => *edge,
                    State::InPosition { entry_edge, .. } => *entry_edge,
                    _ => Decimal::ZERO,
                }
            }),
            signals_generated: self.signals_generated,
            custom: vec![
                (
                    "threshold",
                    format!("{:.2}%", self.lag_threshold * dec!(100)),
                ),
                ("anchor", self.anchor_price.map_or("none".into(), |p| p.to_string())),
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

    fn make_world(
        btc_price: Option<Decimal>,
        market_mid: Option<Decimal>,
    ) -> WorldState {
        let mut external_prices = HashMap::new();
        if let Some(price) = btc_price {
            external_prices.insert(
                Symbol("BTCUSDT".into()),
                SpotPrice {
                    price,
                    timestamp: ts(0),
                },
            );
        }

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
                },
                book: Arc::new(book),
                mid_price: market_mid,
                spread: Some(dec!(0.02)),
                imbalance: Decimal::ZERO,
                price_history: Vec::new(),
            },
        );

        WorldState {
            active_market_id: Some(MarketId("m-1".into())),
            markets,
            positions: Vec::new(),
            open_orders: Vec::new(),
            balance: dec!(1000),
            daily_pnl: Decimal::ZERO,
            external_prices,
            timestamp: ts(0),
        }
    }

    #[test]
    fn test_no_signal_without_btc_price() {
        let mut strat = LeadLag::new(dec!(0.02), 0, dec!(0.005));
        let world = make_world(None, Some(dec!(0.51)));
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_no_signal_without_markets() {
        let mut strat = LeadLag::new(dec!(0.02), 0, dec!(0.005));
        let mut world = make_world(Some(dec!(50000)), Some(dec!(0.51)));
        world.markets.clear();
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_sets_anchor_on_first_tick() {
        let mut strat = LeadLag::new(dec!(0.02), 0, dec!(0.005));
        let world = make_world(Some(dec!(50000)), Some(dec!(0.51)));

        let signals = strat.evaluate(&world);
        assert!(signals.is_empty(), "first tick should set anchor only");
        assert_eq!(strat.anchor_price, Some(dec!(50000)));
    }

    #[test]
    fn test_detects_signal_on_large_move() {
        let mut strat = LeadLag::new(dec!(0.02), 0, dec!(0.005));

        // First tick: set anchor at 50000
        let world = make_world(Some(dec!(50000)), Some(dec!(0.51)));
        strat.evaluate(&world);

        // Second tick: BTC moves up 3% (> 2% threshold), entry_delay=0
        let world = make_world(Some(dec!(51500)), Some(dec!(0.51)));
        let signals = strat.evaluate(&world);

        // With 0ms delay, should immediately enter.
        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Enter {
                strategy, side, ..
            } => {
                assert_eq!(*strategy, "lead_lag");
                assert_eq!(*side, Side::Buy);
            }
            other => panic!("expected Enter, got: {other:?}"),
        }
    }

    #[test]
    fn test_no_signal_on_small_move() {
        let mut strat = LeadLag::new(dec!(0.02), 0, dec!(0.005));

        // Set anchor
        let world = make_world(Some(dec!(50000)), Some(dec!(0.51)));
        strat.evaluate(&world);

        // BTC moves 1% (< 2% threshold)
        let world = make_world(Some(dec!(50500)), Some(dec!(0.51)));
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_sell_signal_on_btc_drop() {
        let mut strat = LeadLag::new(dec!(0.02), 0, dec!(0.005));

        // Set anchor
        let world = make_world(Some(dec!(50000)), Some(dec!(0.51)));
        strat.evaluate(&world);

        // BTC drops 3%
        let world = make_world(Some(dec!(48500)), Some(dec!(0.51)));
        let signals = strat.evaluate(&world);

        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Enter { side, .. } => {
                assert_eq!(*side, Side::Sell);
            }
            other => panic!("expected Enter, got: {other:?}"),
        }
    }

    #[test]
    fn test_exit_on_convergence() {
        let mut strat = LeadLag::new(dec!(0.02), 0, dec!(0.005));

        // Set anchor
        let world = make_world(Some(dec!(50000)), Some(dec!(0.51)));
        strat.evaluate(&world);

        // Enter position (BTC +3%)
        let world = make_world(Some(dec!(51500)), Some(dec!(0.51)));
        let signals = strat.evaluate(&world);
        assert_eq!(signals.len(), 1);
        assert!(matches!(&signals[0], Signal::Enter { .. }));

        // Now BTC converges back (move < exit_convergence = 0.5%)
        let world = make_world(Some(dec!(50100)), Some(dec!(0.51)));
        let signals = strat.evaluate(&world);

        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Exit { strategy, reason, .. } => {
                assert_eq!(*strategy, "lead_lag");
                assert_eq!(*reason, ExitReason::StrategyExit);
            }
            other => panic!("expected Exit, got: {other:?}"),
        }
    }

    #[test]
    fn test_market_change_resets_state() {
        let mut strat = LeadLag::new(dec!(0.02), 0, dec!(0.005));
        strat.anchor_price = Some(dec!(50000));

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
        };

        strat.on_market_change(&MarketId("m-1".into()), &new_market);

        assert!(strat.anchor_price.is_none());
        assert!(matches!(strat.state, State::Watching));
    }

    #[test]
    fn test_metrics_report() {
        let strat = LeadLag::new(dec!(0.02), 500, dec!(0.005));
        let metrics = strat.metrics();
        assert_eq!(metrics.name, "lead_lag");
        assert_eq!(metrics.state, "watching");
        assert_eq!(metrics.signals_generated, 0);
    }

    #[test]
    fn test_entry_delay_prevents_immediate_entry() {
        let mut strat = LeadLag::new(dec!(0.02), 10_000, dec!(0.005));

        // Set anchor
        let world = make_world(Some(dec!(50000)), Some(dec!(0.51)));
        strat.evaluate(&world);

        // BTC +3% — should detect but NOT enter (10s delay)
        let world = make_world(Some(dec!(51500)), Some(dec!(0.51)));
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty(), "should not enter during delay");

        // Still in SignalDetected state
        assert!(matches!(strat.state, State::SignalDetected { .. }));
    }
}
