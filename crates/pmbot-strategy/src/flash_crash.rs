//! Flash-crash strategy — mean reversion on sharp price drops.
//!
//! State machine:
//! ```text
//! Watching → InPosition → Watching
//! ```

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tracing::debug;

use pmbot_core::messages::{Signal, StrategyMetrics, WorldState};
use pmbot_core::types::{
    ExitReason, FillEvent, MarketId, MarketInfo, PositionId, Side, SignalId,
};

use crate::traits::Strategy;

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

/// Internal state of the flash-crash strategy.
enum FlashCrashState {
    /// Watching for a sharp drop.
    Watching,
    /// Bought the dip; waiting for mean reversion.
    InPosition {
        signal_id: SignalId,
        entry_price: Decimal,
        mean_price: Decimal,
    },
}

// ---------------------------------------------------------------------------
// FlashCrash
// ---------------------------------------------------------------------------

/// Flash-crash mean-reversion strategy.
///
/// Detects sharp drops relative to a rolling mean and enters a Buy
/// position expecting a reversion to the mean. Exits when price
/// recovers by a configurable fraction of the drop.
pub struct FlashCrash {
    /// Minimum relative drop from mean to trigger entry (e.g., 0.15 = 15%).
    drop_threshold: Decimal,
    /// Lookback window in seconds for computing the rolling mean.
    lookback_secs: u64,
    /// Target reversion fraction (e.g., 0.5 = exit at 50% recovery).
    reversion_target: Decimal,
    /// Minimum order-book imbalance to confirm recovery signal.
    min_recovery_imbalance: Decimal,
    /// Internal state machine.
    state: FlashCrashState,
    /// Number of signals generated.
    signals_generated: u64,
}

impl FlashCrash {
    /// Create a new flash-crash strategy.
    ///
    /// - `drop_threshold`: relative drop from mean to trigger (e.g., 0.15)
    /// - `lookback_secs`: seconds of price history for mean calculation
    /// - `reversion_target`: fraction of drop to recover before exit
    /// - `min_recovery_imbalance`: minimum book imbalance for entry confirmation
    pub fn new(
        drop_threshold: Decimal,
        lookback_secs: u64,
        reversion_target: Decimal,
        min_recovery_imbalance: Decimal,
    ) -> Self {
        Self {
            drop_threshold,
            lookback_secs,
            reversion_target,
            min_recovery_imbalance,
            state: FlashCrashState::Watching,
            signals_generated: 0,
        }
    }

    /// State name for metrics display.
    fn state_name(&self) -> &'static str {
        match &self.state {
            FlashCrashState::Watching => "watching",
            FlashCrashState::InPosition { .. } => "in_position",
        }
    }
}

impl Strategy for FlashCrash {
    fn name(&self) -> &'static str {
        "flash_crash"
    }

    fn evaluate(&mut self, world: &WorldState) -> Vec<Signal> {
        // 1. Get the first market.
        let (market_id, snap) = match world.active_market_id.as_ref().and_then(|id| world.markets.get(id).map(|snap| (id, snap))) {
            Some(pair) => pair,
            None => return Vec::new(),
        };

        let token_id = match snap.info.token_ids.first() {
            Some(tid) => tid.clone(),
            None => return Vec::new(),
        };

        // 2. Need price history.
        if snap.price_history.is_empty() {
            return Vec::new();
        }

        // 3. Compute mean from price_history within the lookback window.
        let now = world.timestamp;
        let lookback_start = now
            - chrono::Duration::seconds(self.lookback_secs as i64);

        let points_in_window: Vec<Decimal> = snap
            .price_history
            .iter()
            .filter(|pp| pp.timestamp >= lookback_start)
            .map(|pp| pp.price)
            .collect();

        if points_in_window.is_empty() {
            return Vec::new();
        }

        let count = Decimal::from(points_in_window.len() as u64);
        let sum: Decimal = points_in_window.iter().copied().sum();
        let mean_price = sum / count;

        // 4. Get current mid price.
        let current_mid = match snap.mid_price {
            Some(mp) => mp,
            None => return Vec::new(),
        };

        match &self.state {
            FlashCrashState::Watching => {
                // 5. Compute relative drop from mean.
                if mean_price.is_zero() {
                    return Vec::new();
                }
                let drop = (mean_price - current_mid) / mean_price;

                debug!(
                    drop = %drop,
                    mean = %mean_price,
                    mid = %current_mid,
                    imbalance = %snap.imbalance,
                    "flash_crash: evaluate"
                );

                // Enter if drop exceeds threshold and book imbalance confirms recovery.
                if drop > self.drop_threshold && snap.imbalance > self.min_recovery_imbalance
                {
                    let signal_id = SignalId::new();
                    self.signals_generated += 1;

                    let edge = drop;

                    self.state = FlashCrashState::InPosition {
                        signal_id,
                        entry_price: current_mid,
                        mean_price,
                    };

                    vec![Signal::Enter {
                        id: signal_id,
                        strategy: "flash_crash",
                        market_id: market_id.clone(),
                        token_id,
                        side: Side::Buy,
                        size: dec!(1),
                        price: snap.mid_price,
                        edge,
                        confidence: dec!(0.60),
                    }]
                } else {
                    Vec::new()
                }
            }

            FlashCrashState::InPosition {
                signal_id,
                entry_price,
                mean_price,
            } => {
                // 6. Compute reversion target.
                let target =
                    *entry_price + (*mean_price - *entry_price) * self.reversion_target;

                debug!(
                    current = %current_mid,
                    target = %target,
                    "flash_crash: checking exit"
                );

                if current_mid >= target {
                    let original_signal_id = *signal_id;
                    let signal_id = SignalId::new();
                    self.signals_generated += 1;

                    self.state = FlashCrashState::Watching;

                    vec![Signal::Exit {
                        id: signal_id,
                        strategy: "flash_crash",
                        signal_id: original_signal_id,
                        reason: ExitReason::StrategyExit,
                    }]
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn on_fill(&mut self, _fill: &FillEvent) {
        debug!("flash_crash: fill received");
    }

    fn on_market_change(&mut self, _old: &MarketId, _new: &MarketInfo) {
        self.state = FlashCrashState::Watching;
        debug!("flash_crash: market changed, resetting state");
    }

    fn metrics(&self) -> StrategyMetrics {
        StrategyMetrics {
            name: "flash_crash",
            state: self.state_name(),
            edge: match &self.state {
                FlashCrashState::InPosition {
                    entry_price,
                    mean_price,
                    ..
                } => Some(*mean_price - *entry_price),
                _ => None,
            },
            signals_generated: self.signals_generated,
            custom: vec![
                (
                    "drop_threshold",
                    format!("{:.1}%", self.drop_threshold * dec!(100)),
                ),
                (
                    "reversion_target",
                    format!("{:.0}%", self.reversion_target * dec!(100)),
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

    /// Build a WorldState with price history, imbalance, and mid_price.
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
                price: dec!(0.40),
                size: dec!(100),
            }],
            asks: vec![Level {
                price: dec!(0.42),
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
            timestamp: ts(0),
        }
    }

    #[test]
    fn test_no_signal_without_price_history() {
        let mut strat = FlashCrash::new(dec!(0.15), 300, dec!(0.5), dec!(0.1));
        let world = make_world(Some(dec!(0.50)), dec!(0.3), vec![]);
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_no_signal_on_small_drop() {
        let mut strat = FlashCrash::new(dec!(0.15), 300, dec!(0.5), dec!(0.1));
        // Mean ~ 0.50, current mid = 0.48 → drop = 4% (< 15%)
        let history = vec![(-100, dec!(0.50)), (-50, dec!(0.50))];
        let world = make_world(Some(dec!(0.48)), dec!(0.3), history);
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_no_signal_without_imbalance_confirmation() {
        let mut strat = FlashCrash::new(dec!(0.15), 300, dec!(0.5), dec!(0.3));
        // Mean ~ 0.60, current mid = 0.45 → drop = 25% (> 15%)
        // But imbalance = 0.1 < 0.3 min_recovery_imbalance
        let history = vec![(-100, dec!(0.60)), (-50, dec!(0.60))];
        let world = make_world(Some(dec!(0.45)), dec!(0.1), history);
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_enter_on_flash_crash() {
        let mut strat = FlashCrash::new(dec!(0.15), 300, dec!(0.5), dec!(0.1));
        // Mean ~ 0.60, current mid = 0.45 → drop = 25% (> 15%)
        // Imbalance = 0.3 (> 0.1 min)
        let history = vec![(-100, dec!(0.60)), (-50, dec!(0.60))];
        let world = make_world(Some(dec!(0.45)), dec!(0.3), history);
        let signals = strat.evaluate(&world);

        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Enter {
                strategy, side, ..
            } => {
                assert_eq!(*strategy, "flash_crash");
                assert_eq!(*side, Side::Buy);
            }
            other => panic!("expected Enter, got: {other:?}"),
        }
    }

    #[test]
    fn test_exit_on_reversion() {
        let mut strat = FlashCrash::new(dec!(0.15), 300, dec!(0.5), dec!(0.1));

        // Enter: mean = 0.60, current = 0.45
        let history = vec![(-100, dec!(0.60)), (-50, dec!(0.60))];
        let world = make_world(Some(dec!(0.45)), dec!(0.3), history.clone());
        let signals = strat.evaluate(&world);
        assert_eq!(signals.len(), 1);
        assert!(matches!(&signals[0], Signal::Enter { .. }));

        // Target = 0.45 + (0.60 - 0.45) * 0.5 = 0.525
        // Current mid = 0.53 > 0.525 → exit
        let world = make_world(Some(dec!(0.53)), dec!(0.0), history);
        let signals = strat.evaluate(&world);

        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Exit {
                strategy, reason, ..
            } => {
                assert_eq!(*strategy, "flash_crash");
                assert_eq!(*reason, ExitReason::StrategyExit);
            }
            other => panic!("expected Exit, got: {other:?}"),
        }
    }

    #[test]
    fn test_no_exit_below_target() {
        let mut strat = FlashCrash::new(dec!(0.15), 300, dec!(0.5), dec!(0.1));

        // Enter: mean = 0.60, current = 0.45
        let history = vec![(-100, dec!(0.60)), (-50, dec!(0.60))];
        let world = make_world(Some(dec!(0.45)), dec!(0.3), history.clone());
        strat.evaluate(&world);

        // Target = 0.525, current = 0.50 < target → no exit
        let world = make_world(Some(dec!(0.50)), dec!(0.0), history);
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_market_change_resets_state() {
        let mut strat = FlashCrash::new(dec!(0.15), 300, dec!(0.5), dec!(0.1));

        // Enter a position first.
        let history = vec![(-100, dec!(0.60)), (-50, dec!(0.60))];
        let world = make_world(Some(dec!(0.45)), dec!(0.3), history);
        strat.evaluate(&world);
        assert!(matches!(strat.state, FlashCrashState::InPosition { .. }));

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
        assert!(matches!(strat.state, FlashCrashState::Watching));
    }
}
