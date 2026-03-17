//! Fair-value strategy using Black-Scholes pricing for binary options.

use std::time::Duration;

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tracing::{debug, info};

use pmbot_core::messages::{Signal, StrategyMetrics, WorldState};
use pmbot_core::types::{ExitReason, FillEvent, MarketId, MarketInfo, Side, SignalId, Symbol};

use crate::traits::Strategy;

/// State of the fair-value strategy.
#[derive(Debug, Clone)]
enum FairValueState {
    /// Watching for a mispricing opportunity.
    Watching,
    /// Waiting for an entry order to fill.
    Entering {
        signal_id: SignalId,
        entry_fv: Decimal,
    },
    /// We have an open position based on a fair-value edge.
    InPosition {
        signal_id: SignalId,
        entry_fv: Decimal,
    },
}

impl FairValueState {
    fn name(&self) -> &'static str {
        match self {
            Self::Watching => "watching",
            Self::Entering { .. } => "entering",
            Self::InPosition { .. } => "in_position",
        }
    }
}

// ---------------------------------------------------------------------------
// FairValue
// ---------------------------------------------------------------------------

/// Fair-value strategy using Black-Scholes pricing for binary options.
pub struct FairValue {
    /// Multiplier applied to the base volatility estimate.
    vol_multiplier: Decimal,
    /// Minimum remaining time to expiry before the strategy will trade.
    min_time_to_expiry: Duration,
    /// Minimum absolute edge (|fv - market|) to enter.
    min_activation_edge: Decimal,
    /// External symbol to use for spot price (e.g., BTCUSDT).
    anchor_symbol: Symbol,
    /// The cached anchor price (strike price at market entry).
    anchor_price: Option<Decimal>,
    /// Internal state machine.
    state: FairValueState,
    /// Last computed fair value for metrics.
    last_fair_value: Option<Decimal>,
    /// Number of signals generated.
    signals_generated: u64,
}

impl FairValue {
    /// Create a new fair-value strategy.
    pub fn new(
        vol_multiplier: Decimal,
        min_time_to_expiry_secs: u64,
        min_activation_edge: Decimal,
    ) -> Self {
        Self {
            vol_multiplier,
            min_time_to_expiry: Duration::from_secs(min_time_to_expiry_secs),
            min_activation_edge,
            anchor_symbol: Symbol("BTCUSDT".into()),
            anchor_price: None,
            state: FairValueState::Watching,
            last_fair_value: None,
            signals_generated: 0,
        }
    }

    /// Compute binary call fair value using the Black-Scholes d2 term.
    fn binary_call_fv(spot: f64, strike: f64, vol: f64, time_years: f64) -> f64 {
        if vol <= 0.0 || time_years <= 0.0 {
            return if spot > strike { 1.0 } else { 0.0 };
        }
        let d2 =
            (f64::ln(spot / strike) - 0.5 * vol * vol * time_years) / (vol * f64::sqrt(time_years));

        // Approximation of the cumulative normal distribution Phi(d2)
        statrs::distribution::ContinuousCDF::<f64, f64>::cdf(
            &statrs::distribution::Normal::new(0.0, 1.0).unwrap(),
            d2,
        )
    }
}

impl Strategy for FairValue {
    fn name(&self) -> &'static str {
        "fair_value"
    }

    fn evaluate(&mut self, world: &WorldState) -> Vec<Signal> {
        // 1. Get active market.
        let (market_id, snap) = match world
            .active_market_id
            .as_ref()
            .and_then(|id| world.markets.get(id).map(|snap| (id, snap)))
        {
            Some(res) => res,
            None => return Vec::new(),
        };

        // 2. Get the BTC spot price from external feeds.
        let btc_spot = match world.external_prices.get(&self.anchor_symbol) {
            Some(sp) => sp.price,
            None => return Vec::new(),
        };

        // Set anchor on first observation.
        if self.anchor_price.is_none() {
            self.anchor_price = Some(btc_spot);
        }

        // 3. Market must have an end_date for time-to-expiry calculation.
        let end_date = match snap.info.end_date {
            Some(ed) => ed,
            None => return Vec::new(),
        };

        let now = world.timestamp;
        let time_to_expiry = end_date.signed_duration_since(now);
        let secs_remaining = time_to_expiry.num_seconds();

        // If expired or below minimum time, skip.
        if secs_remaining <= 0
            || Duration::from_secs(secs_remaining as u64) < self.min_time_to_expiry
        {
            return Vec::new();
        }

        // 4. Get current market mid price.
        let market_price = match snap.mid_price {
            Some(mp) => mp,
            None => return Vec::new(),
        };

        // Get market anchor price as the implicit strike.
        let strike = match self.anchor_price {
            Some(p) => p,
            None => return Vec::new(),
        };

        // 5. Annualized vol: default 0.50 * vol_multiplier.
        let base_vol = dec!(0.50);
        let vol = base_vol * self.vol_multiplier;

        // Convert to f64 for the Black-Scholes calculation.
        let spot_f64 = btc_spot.to_f64().unwrap_or(0.0);
        let strike_f64 = strike.to_f64().unwrap_or(0.0);
        let vol_f64 = vol.to_f64().unwrap_or(0.0);
        let time_years_f64 = secs_remaining as f64 / (365.25 * 24.0 * 3600.0);

        // 6. Compute fair value (probability that S > K).
        let fv_f64 = Self::binary_call_fv(spot_f64, strike_f64, vol_f64, time_years_f64);
        let fair_value = Decimal::try_from(fv_f64).unwrap_or(dec!(0.5));
        self.last_fair_value = Some(fair_value);

        // 7. Compute edge.
        let edge = (fair_value - market_price).abs();

        debug!(
            fair_value = %fair_value,
            market_price = %market_price,
            edge = %edge,
            "fair_value: evaluate"
        );

        match &self.state {
            FairValueState::Watching => {
                // 8. Enter if edge exceeds minimum.
                if edge > self.min_activation_edge {
                    let side = if fair_value > market_price {
                        Side::Buy
                    } else {
                        Side::Sell
                    };
                    let signal_id = SignalId::new();
                    self.signals_generated += 1;

                    let token_id = match snap.info.token_ids.first() {
                        Some(tid) => tid.clone(),
                        None => return Vec::new(),
                    };

                    self.state = FairValueState::Entering {
                        signal_id,
                        entry_fv: fair_value,
                    };

                    vec![Signal::Enter {
                        id: signal_id,
                        strategy: "fair_value",
                        market_id: market_id.clone(),
                        token_id,
                        side,
                        size: dec!(1),
                        price: snap.mid_price,
                        edge,
                        confidence: dec!(0.65),
                    }]
                } else {
                    Vec::new()
                }
            }

            FairValueState::Entering { .. } => {
                // Still waiting for fill.
                Vec::new()
            }

            FairValueState::InPosition {
                signal_id,
                entry_fv: _,
            } => {
                // 9. Exit on convergence: edge drops below min_activation_edge / 2.
                let exit_threshold = self.min_activation_edge / dec!(2);
                if edge < exit_threshold {
                    let original_signal_id = *signal_id;
                    self.state = FairValueState::Watching;
                    info!(%edge, "fair value convergence reached — exiting");

                    vec![Signal::Exit {
                        id: SignalId::new(),
                        strategy: "fair_value",
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
        if let FairValueState::Entering {
            signal_id,
            entry_fv,
        } = &self.state
        {
            if *signal_id == fill.signal_id {
                info!("fair_value: entry order filled, transitioning to InPosition");
                self.state = FairValueState::InPosition {
                    signal_id: *signal_id,
                    entry_fv: *entry_fv,
                };
            }
        } else if let FairValueState::InPosition { signal_id: _, .. } = &self.state {
            // Check if this was our exit fill (we don't track the exit signal_id yet, but let's assume if it matches it's ours)
            // Wait, the exit SignalId is new.
            // Better logic: if we are InPosition and get a fill for the SAME original signal_id but opposite side?
            // Actually, RiskActor handles the position closing.
            // For simplicity, if we get ANY fill that isn't our Enter fill while InPosition, we don't necessarily reset.
            // But if our position is closed, we should go back to Watching.
            // RiskActor should probably emit a PositionClosed event.
        }
    }

    fn on_market_change(&mut self, _old: &MarketId, _new: &MarketInfo) {
        self.state = FairValueState::Watching;
        self.anchor_price = None;
        self.last_fair_value = None;
        debug!("fair_value: market changed, resetting state");
    }

    fn metrics(&self) -> StrategyMetrics {
        StrategyMetrics {
            name: "fair_value",
            state: self.state.name(),
            edge: self.last_fair_value.map(|_| {
                match &self.state {
                    FairValueState::InPosition { entry_fv, .. }
                    | FairValueState::Entering { entry_fv, .. } => {
                        // Report the entry fair-value deviation.
                        (*entry_fv - dec!(0.5)).abs()
                    }
                    _ => Decimal::ZERO,
                }
            }),
            signals_generated: self.signals_generated,
            custom: vec![
                ("vol_multiplier", format!("{:.2}", self.vol_multiplier)),
                (
                    "min_activation_edge",
                    format!("{:.2}", self.min_activation_edge),
                ),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use pmbot_core::messages::MarketSnapshot;
    use pmbot_core::types::{OrderId, OrderbookSnapshot, PricePoint, SpotPrice, TokenId};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn ts(secs_offset: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + secs_offset, 0).unwrap()
    }

    fn empty_book() -> Arc<OrderbookSnapshot> {
        Arc::new(OrderbookSnapshot {
            market_id: MarketId("m-1".into()),
            token_id: TokenId("tok-1".into()),
            bids: Vec::new(),
            asks: Vec::new(),
            timestamp: ts(0),
        })
    }

    fn make_world(
        btc_price: Option<Decimal>,
        market_mid: Option<Decimal>,
        secs_to_expiry: Option<i64>,
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
        let info = MarketInfo {
            id: MarketId("m-1".into()),
            question: "BTC Up?".into(),
            slug: "btc-up".into(),
            outcomes: vec!["Yes".into(), "No".into()],
            token_ids: vec![TokenId("tok-1".into())],
            condition_id: "cond-1".into(),
            neg_risk: false,
            active: true,
            end_date: secs_to_expiry.map(|s| ts(s)),
            liquidity: dec!(10000),
            volume: dec!(5000),
        };

        markets.insert(
            info.id.clone(),
            MarketSnapshot {
                info: info.clone(),
                book: empty_book(),
                mid_price: market_mid,
                spread: Some(dec!(0.01)),
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
            network_latency: HashMap::new(),
            timestamp: ts(0),
        }
    }

    #[test]
    fn test_binary_call_fv() {
        // Spot = Strike, vol=0.50, T=1 year => fv ~ 0.40 (Black-Scholes d2 term)
        // Note: binary call value is Phi(d2).
        let fv = FairValue::binary_call_fv(50000.0, 50000.0, 0.50, 1.0);
        assert!(fv > 0.39 && fv < 0.41);

        // Spot far above strike => fv ~ 1.0
        let fv = FairValue::binary_call_fv(100000.0, 50000.0, 0.50, 1.0);
        assert!(fv > 0.99);

        // Spot far below strike => fv ~ 0.0
        let fv = FairValue::binary_call_fv(10000.0, 50000.0, 0.50, 1.0);
        assert!(fv < 0.01);
    }

    #[test]
    fn test_enter_on_large_edge() {
        let mut strat = FairValue::new(dec!(1.0), 60, dec!(0.05));
        strat.anchor_price = Some(dec!(50000));
        let world = make_world(Some(dec!(60000)), Some(dec!(0.51)), Some(30 * 86400));
        let signals = strat.evaluate(&world);

        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Enter { strategy, side, .. } => {
                assert_eq!(*strategy, "fair_value");
                assert_eq!(*side, Side::Buy);
            }
            other => panic!("expected Enter, got: {other:?}"),
        }
    }

    #[test]
    fn test_no_signal_on_small_edge() {
        let mut strat = FairValue::new(dec!(1.0), 60, dec!(0.05));
        strat.anchor_price = Some(dec!(50000));
        let world = make_world(Some(dec!(50000)), Some(dec!(0.50)), Some(30 * 86400));
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_exit_on_convergence() {
        let mut strat = FairValue::new(dec!(1.0), 60, dec!(0.05));
        strat.anchor_price = Some(dec!(50000));

        // Enter with large edge.
        let world = make_world(Some(dec!(60000)), Some(dec!(0.51)), Some(30 * 86400));
        let signals = strat.evaluate(&world);
        assert_eq!(signals.len(), 1);
        let signal_id = match &signals[0] {
            Signal::Enter { id, .. } => *id,
            _ => panic!("Expected enter"),
        };

        // Manually trigger fill
        strat.on_fill(&FillEvent {
            order_id: OrderId("o1".into()),
            signal_id,
            market_id: MarketId("m-1".into()),
            side: Side::Buy,
            price: dec!(0.51),
            size: dec!(10),
            timestamp: ts(0),
        });

        // Now market converges.
        let world = make_world(Some(dec!(50000)), Some(dec!(0.50)), Some(120));
        let signals = strat.evaluate(&world);
        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Exit {
                strategy,
                signal_id: sid,
                ..
            } => {
                assert_eq!(*strategy, "fair_value");
                assert_eq!(*sid, signal_id);
            }
            other => panic!("expected Exit, got: {other:?}"),
        }
    }

    #[test]
    fn test_market_change_resets_state() {
        let mut strat = FairValue::new(dec!(1.0), 60, dec!(0.05));
        strat.last_fair_value = Some(dec!(0.60));
        strat.state = FairValueState::InPosition {
            signal_id: SignalId::new(),
            entry_fv: dec!(0.60),
        };

        strat.on_market_change(
            &MarketId("old".into()),
            &MarketInfo {
                id: MarketId("new".into()),
                question: "BTC Up?".into(),
                slug: "btc-up-new".into(),
                outcomes: vec!["Yes".into(), "No".into()],
                token_ids: vec![TokenId("tok-new".into())],
                condition_id: "cond-new".into(),
                neg_risk: false,
                active: true,
                end_date: None,
                liquidity: dec!(10000),
                volume: dec!(5000),
            },
        );
        assert!(matches!(strat.state, FairValueState::Watching));
        assert!(strat.last_fair_value.is_none());
    }
}
