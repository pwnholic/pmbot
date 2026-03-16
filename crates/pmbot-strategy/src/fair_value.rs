//! Fair-value strategy — prices binary options using Black-Scholes
//! and trades when the market price diverges from model fair value.
//!
//! State machine:
//! ```text
//! Watching → InPosition → Watching
//! ```

use std::time::Duration;

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use statrs::distribution::{ContinuousCDF, Normal};
use tracing::debug;

use pmbot_core::messages::{Signal, StrategyMetrics, WorldState};
use pmbot_core::types::{
    ExitReason, FillEvent, MarketId, MarketInfo, PositionId, Side, SignalId, Symbol,
};

use crate::traits::Strategy;

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

/// Internal state of the fair-value strategy.
enum FairValueState {
    /// Watching for a mispricing opportunity.
    Watching,
    /// We have an open position based on a fair-value edge.
    InPosition {
        position_id: PositionId,
        entry_fv: Decimal,
    },
}

// ---------------------------------------------------------------------------
// FairValue
// ---------------------------------------------------------------------------

/// Fair-value strategy using Black-Scholes pricing for binary options.
///
/// Computes fair value using Phi(d2) where:
/// - S = BTC spot price (from feed)
/// - K = strike (derived from market mid-price as implicit strike)
/// - sigma = realized vol (default 0.50 annualized)
/// - T = time to expiry
///
/// Trades when |fair_value - market_price| exceeds minimum edge.
pub struct FairValue {
    /// Multiplier applied to the base volatility estimate.
    vol_multiplier: Decimal,
    /// Minimum remaining time to expiry before the strategy will trade.
    min_time_to_expiry: Duration,
    /// Minimum absolute edge (|fv - market|) to enter.
    min_edge: Decimal,
    /// External symbol to use for spot price (e.g., BTCUSDT).
    anchor_symbol: Symbol,
    /// Internal state machine.
    state: FairValueState,
    /// Last computed fair value for metrics.
    last_fair_value: Option<Decimal>,
    /// Number of signals generated.
    signals_generated: u64,
}

impl FairValue {
    /// Create a new fair-value strategy.
    ///
    /// - `vol_multiplier`: scale factor on the base 0.50 annualized vol
    /// - `min_time_to_expiry_secs`: minimum seconds remaining before expiry
    /// - `min_edge`: minimum |fv - market| to trigger entry
    pub fn new(
        vol_multiplier: Decimal,
        min_time_to_expiry_secs: u64,
        min_edge: Decimal,
    ) -> Self {
        Self {
            vol_multiplier,
            min_time_to_expiry: Duration::from_secs(min_time_to_expiry_secs),
            min_edge,
            anchor_symbol: Symbol("BTCUSDT".into()),
            state: FairValueState::Watching,
            last_fair_value: None,
            signals_generated: 0,
        }
    }

    /// Compute binary call fair value using the Black-Scholes d2 term.
    ///
    /// Returns Phi(d2) = probability that S > K at expiry under risk-neutral
    /// measure (i.e., the price of a binary call option).
    fn binary_call_fv(spot: f64, strike: f64, vol: f64, time_years: f64) -> f64 {
        if vol <= 0.0 || time_years <= 0.0 {
            return if spot > strike { 1.0 } else { 0.0 };
        }
        let d2 = (f64::ln(spot / strike) - 0.5 * vol * vol * time_years)
            / (vol * f64::sqrt(time_years));
        let n = Normal::new(0.0, 1.0).unwrap();
        n.cdf(d2)
    }

    /// State name for metrics display.
    fn state_name(&self) -> &'static str {
        match &self.state {
            FairValueState::Watching => "watching",
            FairValueState::InPosition { .. } => "in_position",
        }
    }
}

impl Strategy for FairValue {
    fn name(&self) -> &'static str {
        "fair_value"
    }

    fn evaluate(&mut self, world: &WorldState) -> Vec<Signal> {
        // 1. Get BTC spot from external prices.
        let btc_spot = match world.external_prices.get(&self.anchor_symbol) {
            Some(sp) => sp.price,
            None => return Vec::new(),
        };

        // 2. Get the first market.
        let (market_id, snap) = match world.markets.iter().next() {
            Some(pair) => pair,
            None => return Vec::new(),
        };

        let token_id = match snap.info.token_ids.first() {
            Some(tid) => tid.clone(),
            None => return Vec::new(),
        };

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

        // Use market mid-price as the implicit strike.
        let strike = market_price;

        // 5. Annualized vol: default 0.50 * vol_multiplier.
        let base_vol = dec!(0.50);
        let vol = base_vol * self.vol_multiplier;

        // Convert to f64 for the Black-Scholes calculation.
        let spot_f64 = btc_spot.to_f64().unwrap_or(0.0);
        let strike_f64 = strike.to_f64().unwrap_or(0.0);
        let vol_f64 = vol.to_f64().unwrap_or(0.0);
        let time_years_f64 = secs_remaining as f64 / (365.25 * 24.0 * 3600.0);

        // 6. Compute fair value.
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
                if edge > self.min_edge {
                    let side = if fair_value > market_price {
                        Side::Buy
                    } else {
                        Side::Sell
                    };
                    let signal_id = SignalId::new();
                    let position_id = PositionId::new();
                    self.signals_generated += 1;

                    self.state = FairValueState::InPosition {
                        position_id,
                        entry_fv: fair_value,
                    };

                    vec![Signal::Enter {
                        id: signal_id,
                        strategy: "fair_value",
                        market_id: market_id.clone(),
                        token_id,
                        side,
                        size: dec!(10),
                        price: snap.mid_price,
                        edge,
                        confidence: dec!(0.65),
                    }]
                } else {
                    Vec::new()
                }
            }

            FairValueState::InPosition {
                position_id,
                entry_fv: _,
            } => {
                // 9. Exit on convergence: edge drops below min_edge / 2.
                let exit_threshold = self.min_edge / dec!(2);
                if edge < exit_threshold {
                    let position_id = *position_id;
                    let signal_id = SignalId::new();
                    self.signals_generated += 1;

                    self.state = FairValueState::Watching;

                    vec![Signal::Exit {
                        id: signal_id,
                        strategy: "fair_value",
                        position_id,
                        reason: ExitReason::StrategyExit,
                    }]
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn on_fill(&mut self, _fill: &FillEvent) {
        debug!("fair_value: fill received");
    }

    fn on_market_change(&mut self, _old: &MarketId, _new: &MarketInfo) {
        self.state = FairValueState::Watching;
        self.last_fair_value = None;
        debug!("fair_value: market changed, resetting state");
    }

    fn metrics(&self) -> StrategyMetrics {
        StrategyMetrics {
            name: "fair_value",
            state: self.state_name(),
            edge: self.last_fair_value.map(|_| {
                match &self.state {
                    FairValueState::InPosition { entry_fv, .. } => {
                        // Report the entry fair-value deviation.
                        (*entry_fv - dec!(0.5)).abs()
                    }
                    _ => Decimal::ZERO,
                }
            }),
            signals_generated: self.signals_generated,
            custom: vec![
                (
                    "min_edge",
                    format!("{:.4}", self.min_edge),
                ),
                (
                    "fair_value",
                    self.last_fair_value
                        .map_or("none".into(), |fv| format!("{:.4}", fv)),
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

    /// Build a WorldState with optional BTC price, market mid, and end_date offset.
    fn make_world(
        btc_price: Option<Decimal>,
        market_mid: Option<Decimal>,
        end_date_secs_from_now: Option<i64>,
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

        let now = ts(0);
        let end_date = end_date_secs_from_now.map(|offset| ts(offset));

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
            timestamp: now,
        };

        markets.insert(
            market_id.clone(),
            MarketSnapshot {
                info: MarketInfo {
                    id: market_id,
                    question: "Will BTC be above 50k?".into(),
                    slug: "btc-above-50k".into(),
                    outcomes: vec!["Yes".into(), "No".into()],
                    token_ids: vec![TokenId("tok".into())],
                    condition_id: "cond".into(),
                    neg_risk: false,
                    active: true,
                    end_date,
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
            markets,
            positions: Vec::new(),
            open_orders: Vec::new(),
            balance: dec!(1000),
            daily_pnl: Decimal::ZERO,
            external_prices,
            timestamp: now,
        }
    }

    #[test]
    fn test_no_signal_without_btc_price() {
        let mut strat = FairValue::new(dec!(1.0), 60, dec!(0.05));
        let world = make_world(None, Some(dec!(0.51)), Some(86400));
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_no_signal_without_end_date() {
        let mut strat = FairValue::new(dec!(1.0), 60, dec!(0.05));
        // No end_date → can't compute time to expiry
        let world = make_world(Some(dec!(50000)), Some(dec!(0.51)), None);
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_no_signal_when_too_close_to_expiry() {
        let mut strat = FairValue::new(dec!(1.0), 3600, dec!(0.05));
        // Only 30 seconds to expiry, min is 3600
        let world = make_world(Some(dec!(50000)), Some(dec!(0.51)), Some(30));
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_binary_call_fv_deep_itm() {
        // Spot far above strike → FV near 1.0
        let fv = FairValue::binary_call_fv(100000.0, 50000.0, 0.50, 0.1);
        assert!(fv > 0.9, "deep ITM should be near 1.0, got {fv}");
    }

    #[test]
    fn test_binary_call_fv_deep_otm() {
        // Spot far below strike → FV near 0.0
        let fv = FairValue::binary_call_fv(40000.0, 50000.0, 0.50, 0.1);
        assert!(fv < 0.1, "deep OTM should be near 0.0, got {fv}");
    }

    #[test]
    fn test_binary_call_fv_zero_vol() {
        // Zero vol, spot > strike → exactly 1.0
        let fv = FairValue::binary_call_fv(51000.0, 50000.0, 0.0, 1.0);
        assert!((fv - 1.0).abs() < 1e-10);

        // Zero vol, spot < strike → exactly 0.0
        let fv = FairValue::binary_call_fv(49000.0, 50000.0, 0.0, 1.0);
        assert!(fv.abs() < 1e-10);
    }

    #[test]
    fn test_enter_on_large_edge() {
        // BTC spot = 60000, strike (market mid) = 0.51, vol=0.50, T=30 days
        // With spot far above "strike", fv will be very high (near 1.0),
        // so edge = |fv - 0.51| will be large.
        let mut strat = FairValue::new(dec!(1.0), 60, dec!(0.05));
        let world = make_world(Some(dec!(60000)), Some(dec!(0.51)), Some(30 * 86400));
        let signals = strat.evaluate(&world);

        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Enter {
                strategy, side, ..
            } => {
                assert_eq!(*strategy, "fair_value");
                assert_eq!(*side, Side::Buy);
            }
            other => panic!("expected Enter, got: {other:?}"),
        }
    }

    #[test]
    fn test_no_signal_on_small_edge() {
        // Use spot ~ strike so fv ~ 0.5, and market_mid = 0.50 → edge near 0
        let mut strat = FairValue::new(dec!(1.0), 60, dec!(0.05));
        // spot = 0.50, strike = 0.50 → ATM → fv ~ 0.5
        let world = make_world(Some(dec!(0.50)), Some(dec!(0.50)), Some(30 * 86400));
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty(), "ATM should produce no signal");
    }

    #[test]
    fn test_exit_on_convergence() {
        let mut strat = FairValue::new(dec!(1.0), 60, dec!(0.05));

        // Enter with large edge.
        let world = make_world(Some(dec!(60000)), Some(dec!(0.51)), Some(30 * 86400));
        let signals = strat.evaluate(&world);
        assert_eq!(signals.len(), 1);
        assert!(matches!(&signals[0], Signal::Enter { .. }));

        // Now market converges: spot = strike = mid_price, and very short T.
        // binary_call_fv(0.50, 0.50, 0.50, tiny_T) ≈ 0.5 (ATM).
        // Market mid = 0.50 → edge = |0.5 - 0.50| ≈ 0, which is < exit_threshold = 0.025.
        let world = make_world(Some(dec!(0.50)), Some(dec!(0.50)), Some(120));
        let signals = strat.evaluate(&world);
        assert_eq!(signals.len(), 1);
        match &signals[0] {
            Signal::Exit {
                strategy, reason, ..
            } => {
                assert_eq!(*strategy, "fair_value");
                assert_eq!(*reason, ExitReason::StrategyExit);
            }
            other => panic!("expected Exit, got: {other:?}"),
        }
    }

    #[test]
    fn test_market_change_resets_state() {
        let mut strat = FairValue::new(dec!(1.0), 60, dec!(0.05));
        strat.last_fair_value = Some(dec!(0.60));

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
        assert!(strat.last_fair_value.is_none());
        assert!(matches!(strat.state, FairValueState::Watching));
    }
}
