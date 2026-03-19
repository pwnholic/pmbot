//! NegRisk Arbitrage strategy — exploits sum-of-probabilities deviation
//! in Polymarket NegRisk markets.
//!
//! NegRisk markets have multiple outcomes whose YES prices should sum to ~1.0.
//! When they deviate beyond a threshold, there is an arbitrage opportunity.
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

#[derive(Debug)]
enum NegRiskState {
    /// Scanning for sum deviation opportunities.
    Watching,
    /// Holding a position on a mispriced outcome.
    InPosition {
        signal_id: SignalId,
        direction: ArbDirection,
        condition_id: String,
        entry_price: Decimal,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArbDirection {
    /// Sum > 1.0 + threshold: sell the most overpriced outcome.
    SellOverpriced,
    /// Sum < 1.0 - threshold: buy the most underpriced outcome.
    BuyUnderpriced,
}

// ---------------------------------------------------------------------------
// NegRiskArb
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub struct NegRiskArb {
    /// Minimum deviation from 1.0 to trigger a trade (e.g., 0.02 = 2%).
    sum_deviation_threshold: Decimal,
    /// Internal state machine.
    state: NegRiskState,
    /// Number of signals generated (for metrics).
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

impl NegRiskArb {
    /// Create a new NegRisk arbitrage strategy.
    ///
    /// - `sum_deviation_threshold`: minimum deviation from 1.0 to trade (e.g., 0.02)
    pub fn new(sum_deviation_threshold: Decimal) -> Self {
        Self {
            sum_deviation_threshold,
            state: NegRiskState::Watching,
            signals_generated: 0,
            trades: 0,
            wins: 0,
            losses: 0,
            total_pnl: Decimal::ZERO,
        }
    }

    /// State name for metrics display.
    fn state_name(&self) -> &'static str {
        match &self.state {
            NegRiskState::Watching => "watching",
            NegRiskState::InPosition { .. } => "in_position",
        }
    }
}

impl Strategy for NegRiskArb {
    fn name(&self) -> &'static str {
        "negrisk_arb"
    }

    fn evaluate(&mut self, world: &WorldState) -> Vec<Signal> {
        // Collect all neg_risk markets grouped by condition_id.
        let mut groups: std::collections::HashMap<
            &str,
            Vec<(&MarketId, &pmbot_core::messages::MarketSnapshot)>,
        > = std::collections::HashMap::new();

        for (market_id, snap) in &world.markets {
            if snap.info.neg_risk {
                groups
                    .entry(&snap.info.condition_id)
                    .or_default()
                    .push((market_id, snap));
            }
        }

        if groups.is_empty() {
            return Vec::new();
        }

        match &self.state {
            NegRiskState::Watching => {
                // Check each condition group for deviation.
                for (condition_id, markets) in &groups {
                    // Sum mid-prices; skip if any outcome lacks a mid_price.
                    let mut sum = Decimal::ZERO;
                    let mut all_have_mid = true;
                    for (_, snap) in markets {
                        match snap.mid_price {
                            Some(p) => sum += p,
                            None => {
                                all_have_mid = false;
                                break;
                            }
                        }
                    }

                    if !all_have_mid {
                        continue;
                    }

                    let deviation = sum - Decimal::ONE;

                    if deviation > self.sum_deviation_threshold {
                        // Overpriced: sell the highest-priced outcome.
                        let (target_id, target_snap) = markets
                            .iter()
                            .max_by_key(|(_, s)| s.mid_price.unwrap_or(Decimal::ZERO))
                            .unwrap();

                        let token_id = match target_snap.info.token_ids.first() {
                            Some(tid) => tid.clone(),
                            None => continue,
                        };

                        let outcome = target_snap
                            .info
                            .outcomes
                            .first()
                            .cloned()
                            .unwrap_or_default();

                        let signal_id = SignalId::new();
                        self.signals_generated += 1;
                        let entry_price = target_snap.mid_price.unwrap_or(Decimal::ZERO);

                        let edge = deviation;
                        self.state = NegRiskState::InPosition {
                            signal_id,
                            direction: ArbDirection::SellOverpriced,
                            condition_id: condition_id.to_string(),
                            entry_price,
                        };

                        debug!(
                            sum = %sum,
                            deviation = %deviation,
                            target = %target_id,
                            "negrisk_arb: overpriced, selling"
                        );

                        return vec![Signal::Enter {
                            id: signal_id,
                            strategy: "negrisk_arb",
                            market_id: (*target_id).clone(),
                            token_id,
                            outcome,
                            side: Side::Sell,
                            size: dec!(1),
                            price: target_snap.mid_price,
                            edge,
                            confidence: dec!(0.80),
                        }];
                    } else if deviation < -self.sum_deviation_threshold {
                        // Underpriced: buy the lowest-priced outcome.
                        let (target_id, target_snap) = markets
                            .iter()
                            .min_by_key(|(_, s)| s.mid_price.unwrap_or(Decimal::ONE))
                            .unwrap();

                        let token_id = match target_snap.info.token_ids.first() {
                            Some(tid) => tid.clone(),
                            None => continue,
                        };

                        let outcome = target_snap
                            .info
                            .outcomes
                            .first()
                            .cloned()
                            .unwrap_or_default();

                        let signal_id = SignalId::new();
                        self.signals_generated += 1;
                        let entry_price = target_snap.mid_price.unwrap_or(Decimal::ZERO);

                        let edge = deviation.abs();
                        self.state = NegRiskState::InPosition {
                            signal_id,
                            direction: ArbDirection::BuyUnderpriced,
                            condition_id: condition_id.to_string(),
                            entry_price,
                        };

                        debug!(
                            sum = %sum,
                            deviation = %deviation,
                            target = %target_id,
                            "negrisk_arb: underpriced, buying"
                        );

                        return vec![Signal::Enter {
                            id: signal_id,
                            strategy: "negrisk_arb",
                            market_id: (*target_id).clone(),
                            token_id,
                            outcome,
                            side: Side::Buy,
                            size: dec!(1),
                            price: target_snap.mid_price,
                            edge,
                            confidence: dec!(0.80),
                        }];
                    }
                }
                Vec::new()
            }

            NegRiskState::InPosition {
                signal_id,
                condition_id,
                entry_price: _,
                ..
            } => {
                // Check if the condition group has converged back within threshold.
                if let Some(markets) = groups.get(condition_id.as_str()) {
                    let mut sum = Decimal::ZERO;
                    let mut all_have_mid = true;
                    for (_, snap) in markets {
                        match snap.mid_price {
                            Some(p) => sum += p,
                            None => {
                                all_have_mid = false;
                                break;
                            }
                        }
                    }

                    if all_have_mid {
                        let deviation = (sum - Decimal::ONE).abs();
                        if deviation <= self.sum_deviation_threshold {
                            let original_signal_id = *signal_id;
                            let signal_id = SignalId::new();
                            self.signals_generated += 1;

                            self.state = NegRiskState::Watching;

                            debug!(
                                sum = %sum,
                                deviation = %deviation,
                                "negrisk_arb: converged, exiting"
                            );

                            return vec![Signal::Exit {
                                id: signal_id,
                                strategy: "negrisk_arb",
                                signal_id: original_signal_id,
                                reason: ExitReason::StrategyExit,
                            }];
                        }
                    }
                }
                Vec::new()
            }
        }
    }

    fn on_fill(&mut self, fill: &FillEvent) {
        match &self.state {
            NegRiskState::InPosition {
                signal_id,
                direction,
                entry_price,
                ..
            } => {
                if fill.signal_id == *signal_id {
                    // Entry fill
                    self.trades += 1;
                    info!(
                        strategy = "negrisk_arb",
                        ?fill.signal_id,
                        price = %fill.price,
                        "entry order filled"
                    );
                } else {
                    // Exit fill - calculate PnL
                    // For SellOverpriced: we sold at entry_price, buy back at exit price
                    // For BuyUnderpriced: we bought at entry_price, sell at exit price
                    let pnl = match direction {
                        ArbDirection::SellOverpriced => (entry_price - fill.price) * fill.size,
                        ArbDirection::BuyUnderpriced => (fill.price - entry_price) * fill.size,
                    };

                    if pnl >= Decimal::ZERO {
                        self.wins += 1;
                    } else {
                        self.losses += 1;
                    }
                    self.total_pnl += pnl;

                    info!(
                        strategy = "negrisk_arb",
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
        self.state = NegRiskState::Watching;
        debug!("negrisk_arb: market changed, resetting state");
    }

    fn metrics(&self) -> StrategyMetrics {
        StrategyMetrics {
            name: "negrisk_arb",
            state: self.state_name(),
            edge: match &self.state {
                NegRiskState::InPosition { direction, .. } => Some(match direction {
                    ArbDirection::SellOverpriced => dec!(0.01),
                    ArbDirection::BuyUnderpriced => dec!(0.01),
                }),
                _ => None,
            },
            signals_generated: self.signals_generated,
            trades: 0,
            wins: 0,
            losses: 0,
            total_pnl: Decimal::ZERO,
            pnl_history: Vec::new(),
            custom: vec![(
                "threshold",
                format!("{:.2}%", self.sum_deviation_threshold * dec!(100)),
            )],
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

    /// Build a WorldState with multiple neg_risk markets sharing a condition_id.
    /// `outcomes` is a vec of (market_id, mid_price).
    fn make_negrisk_world(condition_id: &str, outcomes: &[(&str, Decimal)]) -> WorldState {
        let mut markets = HashMap::new();

        for (mid, price) in outcomes {
            let market_id = MarketId(mid.to_string());
            let token_id = TokenId(format!("tok-{mid}"));
            let book = OrderbookSnapshot {
                market_id: market_id.clone(),
                token_id: token_id.clone(),
                bids: vec![Level {
                    price: price - dec!(0.01),
                    size: dec!(100),
                }],
                asks: vec![Level {
                    price: price + dec!(0.01),
                    size: dec!(100),
                }],
                timestamp: ts(0),
            };

            markets.insert(
                market_id.clone(),
                MarketSnapshot {
                    info: MarketInfo {
                        id: market_id,
                        question: format!("Outcome {mid}?"),
                        slug: mid.to_string(),
                        outcomes: vec!["Yes".into(), "No".into()],
                        token_ids: vec![token_id],
                        condition_id: condition_id.into(),
                        neg_risk: true,
                        active: true,
                        end_date: None,
                        liquidity: dec!(10000),
                        volume: dec!(50000),
            outcome_prices: std::collections::HashMap::new(),
            category: "Test".into(),
            tags: vec!["test".into()],
                    },
                    book: Arc::new(book),
                    mid_price: Some(*price),
                    spread: Some(dec!(0.02)),
                    imbalance: Decimal::ZERO,
                    price_history: Vec::new(),
                },
            );
        }

        WorldState {
            active_market_id: None,
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
    fn test_no_signal_when_no_negrisk_markets() {
        let mut strat = NegRiskArb::new(dec!(0.02));
        // Empty world
        let world = WorldState {
            active_market_id: None,
            markets: HashMap::new(),
            discovered_markets: vec![],
            positions: Vec::new(),
            open_orders: Vec::new(),
            balance: dec!(1000),
            daily_pnl: Decimal::ZERO,
            external_prices: HashMap::new(),
            network_latency: HashMap::new(),
            timestamp: ts(0),
        };
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_no_signal_when_sum_within_threshold() {
        let mut strat = NegRiskArb::new(dec!(0.02));
        // Three outcomes summing to 1.01 (within 0.02 threshold)
        let world = make_negrisk_world(
            "cond-1",
            &[
                ("m-a", dec!(0.33)),
                ("m-b", dec!(0.34)),
                ("m-c", dec!(0.34)),
            ],
        );
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_sell_signal_when_sum_above_threshold() {
        let mut strat = NegRiskArb::new(dec!(0.02));
        // Sum = 0.40 + 0.35 + 0.30 = 1.05 > 1.0 + 0.02
        let world = make_negrisk_world(
            "cond-1",
            &[
                ("m-a", dec!(0.40)),
                ("m-b", dec!(0.35)),
                ("m-c", dec!(0.30)),
            ],
        );
        let signals = strat.evaluate(&world);
        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Enter {
                strategy,
                side,
                market_id,
                ..
            } => {
                assert_eq!(*strategy, "negrisk_arb");
                assert_eq!(*side, Side::Sell);
                // Should target the most overpriced (0.40)
                assert_eq!(market_id, &MarketId("m-a".into()));
            }
            other => panic!("expected Enter, got: {other:?}"),
        }
    }

    #[test]
    fn test_buy_signal_when_sum_below_threshold() {
        let mut strat = NegRiskArb::new(dec!(0.02));
        // Sum = 0.30 + 0.30 + 0.35 = 0.95 < 1.0 - 0.02
        let world = make_negrisk_world(
            "cond-1",
            &[
                ("m-a", dec!(0.30)),
                ("m-b", dec!(0.30)),
                ("m-c", dec!(0.35)),
            ],
        );
        let signals = strat.evaluate(&world);
        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Enter {
                strategy,
                side,
                market_id,
                ..
            } => {
                assert_eq!(*strategy, "negrisk_arb");
                assert_eq!(*side, Side::Buy);
                // Should target the most underpriced (0.30) — either m-a or m-b
                let mid = &market_id.0;
                assert!(
                    mid == "m-a" || mid == "m-b",
                    "expected underpriced market, got: {mid}"
                );
            }
            other => panic!("expected Enter, got: {other:?}"),
        }
    }

    #[test]
    fn test_exit_when_sum_converges() {
        let mut strat = NegRiskArb::new(dec!(0.02));

        // First: enter on overpriced sum
        let world = make_negrisk_world(
            "cond-1",
            &[
                ("m-a", dec!(0.40)),
                ("m-b", dec!(0.35)),
                ("m-c", dec!(0.30)),
            ],
        );
        let signals = strat.evaluate(&world);
        assert_eq!(signals.len(), 1);
        assert!(matches!(&signals[0], Signal::Enter { .. }));

        // Now sum converges to 1.00 (within threshold)
        let world = make_negrisk_world(
            "cond-1",
            &[
                ("m-a", dec!(0.34)),
                ("m-b", dec!(0.33)),
                ("m-c", dec!(0.33)),
            ],
        );
        let signals = strat.evaluate(&world);
        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Exit {
                strategy, reason, ..
            } => {
                assert_eq!(*strategy, "negrisk_arb");
                assert_eq!(*reason, ExitReason::StrategyExit);
            }
            other => panic!("expected Exit, got: {other:?}"),
        }
    }

    #[test]
    fn test_no_exit_when_still_deviated() {
        let mut strat = NegRiskArb::new(dec!(0.02));

        // Enter on overpriced
        let world = make_negrisk_world(
            "cond-1",
            &[
                ("m-a", dec!(0.40)),
                ("m-b", dec!(0.35)),
                ("m-c", dec!(0.30)),
            ],
        );
        strat.evaluate(&world);

        // Still deviated (sum = 1.04)
        let world = make_negrisk_world(
            "cond-1",
            &[
                ("m-a", dec!(0.38)),
                ("m-b", dec!(0.35)),
                ("m-c", dec!(0.31)),
            ],
        );
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_market_change_resets_to_watching() {
        let mut strat = NegRiskArb::new(dec!(0.02));

        // Enter position
        let world = make_negrisk_world(
            "cond-1",
            &[
                ("m-a", dec!(0.40)),
                ("m-b", dec!(0.35)),
                ("m-c", dec!(0.30)),
            ],
        );
        strat.evaluate(&world);
        assert!(matches!(strat.state, NegRiskState::InPosition { .. }));

        // Market change resets
        let new_info = MarketInfo {
            id: MarketId("m-new".into()),
            question: "New?".into(),
            slug: "new".into(),
            outcomes: vec!["Yes".into()],
            token_ids: vec![TokenId("tok-new".into())],
            condition_id: "cond-new".into(),
            neg_risk: true,
            active: true,
            end_date: None,
            liquidity: dec!(10000),
            volume: dec!(50000),
            outcome_prices: std::collections::HashMap::new(),
            category: "Test".into(),
            tags: vec!["test".into()],
        };
        strat.on_market_change(&MarketId("m-a".into()), &new_info);
        assert!(matches!(strat.state, NegRiskState::Watching));
    }
}
