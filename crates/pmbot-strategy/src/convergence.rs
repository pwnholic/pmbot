//! Convergence strategy — near-expiry convergence to terminal values.
//!
//! Markets near expiry with high probability tend to converge to 1.0.
//! This strategy buys deep-ITM outcomes for small guaranteed returns.
//!
//! State machine:
//! ```text
//! Scanning → InPosition → Scanning
//! ```

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tracing::debug;

use pmbot_core::messages::{Signal, StrategyMetrics, WorldState};
use pmbot_core::types::{
    ExitReason, FillEvent, MarketId, MarketInfo, Side, SignalId,
};

use crate::traits::Strategy;

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum ConvergenceState {
    /// Scanning for near-expiry high-probability markets.
    Scanning,
    /// Holding a convergence position.
    InPosition {
        signal_id: SignalId,
        market_id: MarketId,
    },
}

// ---------------------------------------------------------------------------
// Convergence
// ---------------------------------------------------------------------------

/// Near-expiry convergence strategy.
///
/// Buys outcomes with high probability when close to expiry, expecting
/// them to converge to 1.0.
pub struct Convergence {
    /// Minimum mid-price to consider (e.g., 0.90).
    min_probability: Decimal,
    /// Maximum seconds to expiry to trade (e.g., 300).
    max_time_to_expiry_secs: i64,
    /// Minimum edge (1.0 - price) required to enter.
    min_activation_edge: Decimal,
    /// Internal state machine.
    state: ConvergenceState,
    /// Number of signals generated (for metrics).
    signals_generated: u64,
}

impl Convergence {
    /// Create a new convergence strategy.
    ///
    /// - `min_probability`: minimum mid-price threshold (e.g., 0.90)
    /// - `max_time_to_expiry_secs`: max seconds before expiry to trade
    /// - `min_activation_edge`: minimum edge (1.0 - price) to enter
    pub fn new(
        min_probability: Decimal,
        max_time_to_expiry_secs: i64,
        min_activation_edge: Decimal,
    ) -> Self {
        Self {
            min_probability,
            max_time_to_expiry_secs,
            min_activation_edge,
            state: ConvergenceState::Scanning,
            signals_generated: 0,
        }
    }

    /// State name for metrics display.
    fn state_name(&self) -> &'static str {
        match &self.state {
            ConvergenceState::Scanning => "scanning",
            ConvergenceState::InPosition { .. } => "in_position",
        }
    }

    /// Compute time to expiry in seconds. Returns None if no end_date.
    fn time_to_expiry(
        end_date: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Option<i64> {
        end_date.map(|end| (end - now).num_seconds())
    }
}

impl Strategy for Convergence {
    fn name(&self) -> &'static str {
        "convergence"
    }

    fn evaluate(&mut self, world: &WorldState) -> Vec<Signal> {
        let now = world.timestamp;

        match &self.state {
            ConvergenceState::Scanning => {
                // Scan all markets for convergence opportunities.
                for (market_id, snap) in &world.markets {
                    let mid = match snap.mid_price {
                        Some(p) => p,
                        None => continue,
                    };

                    let tte = match Self::time_to_expiry(snap.info.end_date, now) {
                        Some(s) if s > 0 => s,
                        _ => continue, // no end_date or already expired
                    };

                    if tte > self.max_time_to_expiry_secs {
                        continue;
                    }

                    if mid < self.min_probability {
                        continue;
                    }

                    let edge = Decimal::ONE - mid;
                    if edge < self.min_activation_edge {
                        continue;
                    }

                    let token_id = match snap.info.token_ids.first() {
                        Some(tid) => tid.clone(),
                        None => continue,
                    };

                    let signal_id = SignalId::new();
                    self.signals_generated += 1;

                    self.state = ConvergenceState::InPosition {
                        signal_id,
                        market_id: market_id.clone(),
                    };

                    debug!(
                        market = %market_id,
                        mid = %mid,
                        tte_secs = tte,
                        edge = %edge,
                        "convergence: entering near-expiry trade"
                    );

                    return vec![Signal::Enter {
                        id: signal_id,
                        strategy: "convergence",
                        market_id: market_id.clone(),
                        token_id,
                        side: Side::Buy,
                        size: dec!(1),
                        price: Some(mid),
                        edge,
                        confidence: dec!(0.90),
                    }];
                }
                Vec::new()
            }

            ConvergenceState::InPosition {
                signal_id,
                market_id,
            } => {
                // Check if thesis is broken: mid_price dropped significantly.
                if let Some(snap) = world.markets.get(market_id)
                    && let Some(mid) = snap.mid_price
                    && mid < self.min_probability * dec!(0.95)
                {
                    let original_signal_id = *signal_id;
                    let signal_id = SignalId::new();
                    self.signals_generated += 1;

                    self.state = ConvergenceState::Scanning;

                    debug!(
                        mid = %mid,
                        "convergence: thesis broken, exiting"
                    );

                    return vec![Signal::Exit {
                        id: signal_id,
                        strategy: "convergence",
                        signal_id: original_signal_id,
                        reason: ExitReason::StopLoss,
                    }];
                }
                // Otherwise hold: let it expire or converge to 1.0.
                Vec::new()
            }
        }
    }

    fn on_fill(&mut self, _fill: &FillEvent) {
        debug!("convergence: fill received");
    }

    fn on_market_change(&mut self, _old: &MarketId, _new: &MarketInfo) {
        self.state = ConvergenceState::Scanning;
        debug!("convergence: market changed, resetting state");
    }

    fn metrics(&self) -> StrategyMetrics {
        StrategyMetrics {
            name: "convergence",
            state: self.state_name(),
            edge: match &self.state {
                ConvergenceState::InPosition { .. } => Some(dec!(0.05)),
                _ => None,
            },
            signals_generated: self.signals_generated,
            custom: vec![
                (
                    "min_prob",
                    format!("{:.0}%", self.min_probability * dec!(100)),
                ),
                (
                    "max_tte",
                    format!("{}s", self.max_time_to_expiry_secs),
                ),
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

    /// Build a WorldState with a single market at the given mid_price
    /// and end_date offset from `now`.
    fn make_convergence_world(
        mid_price: Decimal,
        end_date_offset_secs: Option<i64>,
        now_secs: i64,
    ) -> WorldState {
        let mut markets = HashMap::new();
        let market_id = MarketId("m-conv".into());
        let token_id = TokenId("tok-conv".into());

        let end_date = end_date_offset_secs
            .map(|offset| ts(now_secs + offset));

        let book = OrderbookSnapshot {
            market_id: market_id.clone(),
            token_id: token_id.clone(),
            bids: vec![Level {
                price: mid_price - dec!(0.01),
                size: dec!(100),
            }],
            asks: vec![Level {
                price: mid_price + dec!(0.01),
                size: dec!(100),
            }],
            timestamp: ts(now_secs),
        };

        markets.insert(
            market_id.clone(),
            MarketSnapshot {
                info: MarketInfo {
                    id: market_id,
                    question: "Will X happen?".into(),
                    slug: "will-x-happen".into(),
                    outcomes: vec!["Yes".into(), "No".into()],
                    token_ids: vec![token_id],
                    condition_id: "cond-conv".into(),
                    neg_risk: false,
                    active: true,
                    end_date,
                    liquidity: dec!(10000),
                    volume: dec!(50000),
                },
                book: Arc::new(book),
                mid_price: Some(mid_price),
                spread: Some(dec!(0.02)),
                imbalance: Decimal::ZERO,
                price_history: Vec::new(),
            },
        );

        WorldState {
            active_market_id: None,
            markets,
            positions: Vec::new(),
            open_orders: Vec::new(),
            balance: dec!(1000),
            daily_pnl: Decimal::ZERO,
            external_prices: HashMap::new(),
            network_latency: HashMap::new(),
            timestamp: ts(now_secs),
        }
    }

    #[test]
    fn test_no_signal_without_end_date() {
        let mut strat = Convergence::new(dec!(0.90), 300, dec!(0.02));
        // No end_date set
        let world = make_convergence_world(dec!(0.95), None, 0);
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_no_signal_when_too_far_from_expiry() {
        let mut strat = Convergence::new(dec!(0.90), 300, dec!(0.02));
        // Expires in 600s (> 300s max)
        let world = make_convergence_world(dec!(0.95), Some(600), 0);
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_no_signal_when_probability_too_low() {
        let mut strat = Convergence::new(dec!(0.90), 300, dec!(0.02));
        // Mid = 0.80 < 0.90 min
        let world = make_convergence_world(dec!(0.80), Some(200), 0);
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_enter_on_high_prob_near_expiry() {
        let mut strat = Convergence::new(dec!(0.90), 300, dec!(0.02));
        // Mid = 0.95, expires in 200s, edge = 0.05 > 0.02 min_activation_edge
        let world = make_convergence_world(dec!(0.95), Some(200), 0);
        let signals = strat.evaluate(&world);

        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Enter {
                strategy,
                side,
                edge,
                ..
            } => {
                assert_eq!(*strategy, "convergence");
                assert_eq!(*side, Side::Buy);
                assert_eq!(*edge, dec!(0.05));
            }
            other => panic!("expected Enter, got: {other:?}"),
        }
    }

    #[test]
    fn test_no_signal_when_edge_too_small() {
        let mut strat = Convergence::new(dec!(0.90), 300, dec!(0.02));
        // Mid = 0.99, edge = 0.01 < 0.02 min_activation_edge
        let world = make_convergence_world(dec!(0.99), Some(200), 0);
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_exit_on_thesis_broken() {
        let mut strat = Convergence::new(dec!(0.90), 300, dec!(0.02));

        // Enter position
        let world = make_convergence_world(dec!(0.95), Some(200), 0);
        let signals = strat.evaluate(&world);
        assert_eq!(signals.len(), 1);
        assert!(matches!(&signals[0], Signal::Enter { .. }));

        // Price drops below 0.90 * 0.95 = 0.855
        let world = make_convergence_world(dec!(0.84), Some(150), 50);
        let signals = strat.evaluate(&world);

        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Exit {
                strategy, reason, ..
            } => {
                assert_eq!(*strategy, "convergence");
                assert_eq!(*reason, ExitReason::StopLoss);
            }
            other => panic!("expected Exit, got: {other:?}"),
        }
    }

    #[test]
    fn test_hold_when_price_still_high() {
        let mut strat = Convergence::new(dec!(0.90), 300, dec!(0.02));

        // Enter position
        let world = make_convergence_world(dec!(0.95), Some(200), 0);
        strat.evaluate(&world);

        // Price stays high — should hold
        let world = make_convergence_world(dec!(0.96), Some(170), 30);
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_market_change_resets_to_scanning() {
        let mut strat = Convergence::new(dec!(0.90), 300, dec!(0.02));

        // Enter position
        let world = make_convergence_world(dec!(0.95), Some(200), 0);
        strat.evaluate(&world);
        assert!(matches!(strat.state, ConvergenceState::InPosition { .. }));

        let new_info = MarketInfo {
            id: MarketId("m-new".into()),
            question: "New?".into(),
            slug: "new".into(),
            outcomes: vec!["Yes".into()],
            token_ids: vec![TokenId("tok-new".into())],
            condition_id: "cond-new".into(),
            neg_risk: false,
            active: true,
            end_date: None,
            liquidity: dec!(10000),
            volume: dec!(50000),
        };
        strat.on_market_change(&MarketId("m-conv".into()), &new_info);
        assert!(matches!(strat.state, ConvergenceState::Scanning));
    }
}
