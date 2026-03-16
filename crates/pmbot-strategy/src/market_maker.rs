//! Market Maker strategy — two-sided quoting with inventory skew.
//!
//! Places simultaneous bid and ask orders around the mid-price,
//! adjusting the spread based on current inventory to manage risk.
//!
//! State machine:
//! ```text
//! Quoting ↔ Paused
//! ```

use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tracing::debug;

use pmbot_core::messages::{Signal, StrategyMetrics, WorldState};
use pmbot_core::types::{FillEvent, MarketId, MarketInfo, Side, SignalId};

use crate::traits::Strategy;

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MakerState {
    /// Actively quoting both sides.
    Quoting,
    /// Paused due to inventory limits.
    Paused,
}

// ---------------------------------------------------------------------------
// MarketMaker
// ---------------------------------------------------------------------------

/// Two-sided market making strategy with inventory-based skew.
///
/// Quotes around mid-price with a configurable spread. Adjusts
/// bid/ask skew based on current inventory to reduce directional risk.
pub struct MarketMaker {
    /// Base spread in basis points (e.g., 200 = 2%).
    spread_bps: u32,
    /// Maximum inventory before pausing one side.
    max_inventory: Decimal,
    /// Size per quote order.
    quote_size: Decimal,
    /// Minimum time between re-quotes.
    refresh_interval: Duration,
    /// Current net inventory (positive = long, negative = short).
    current_inventory: Decimal,
    /// Timestamp of last quote.
    last_quote_time: Option<Instant>,
    /// Internal state machine.
    state: MakerState,
    /// Number of signals generated (for metrics).
    signals_generated: u64,
}

impl MarketMaker {
    /// Create a new market maker strategy.
    ///
    /// - `spread_bps`: spread in basis points (e.g., 200 for 2%)
    /// - `max_inventory`: max net position before pausing
    /// - `quote_size`: size of each quote
    /// - `refresh_interval_ms`: milliseconds between re-quotes
    pub fn new(
        spread_bps: u32,
        max_inventory: Decimal,
        quote_size: Decimal,
        refresh_interval_ms: u64,
    ) -> Self {
        Self {
            spread_bps,
            max_inventory,
            quote_size,
            refresh_interval: Duration::from_millis(refresh_interval_ms),
            current_inventory: Decimal::ZERO,
            last_quote_time: None,
            state: MakerState::Quoting,
            signals_generated: 0,
        }
    }

    /// State name for metrics display.
    fn state_name(&self) -> &'static str {
        match self.state {
            MakerState::Quoting => "quoting",
            MakerState::Paused => "paused",
        }
    }

    /// Compute base spread as a Decimal from basis points.
    fn base_spread(&self) -> Decimal {
        Decimal::from(self.spread_bps) / dec!(10000)
    }

    /// Compute inventory skew: shifts quotes to reduce inventory.
    fn inventory_skew(&self) -> Decimal {
        if self.max_inventory.is_zero() {
            return Decimal::ZERO;
        }
        self.current_inventory / self.max_inventory * self.base_spread() / dec!(2)
    }

    /// Force a re-quote on the next evaluate (for testing).
    #[cfg(test)]
    fn force_requote(&mut self) {
        self.last_quote_time = None;
    }
}

impl Strategy for MarketMaker {
    fn name(&self) -> &'static str {
        "market_maker"
    }

    fn evaluate(&mut self, world: &WorldState) -> Vec<Signal> {
        // Get the first market.
        let (market_id, snap) = match world.markets.iter().next() {
            Some(pair) => pair,
            None => return Vec::new(),
        };

        let mid = match snap.mid_price {
            Some(p) => p,
            None => return Vec::new(),
        };

        let token_id = match snap.info.token_ids.first() {
            Some(tid) => tid.clone(),
            None => return Vec::new(),
        };

        // Check refresh interval.
        if let Some(last) = self.last_quote_time
            && last.elapsed() < self.refresh_interval
        {
            return Vec::new();
        }

        // Check inventory limits.
        let inv_abs = self.current_inventory.abs();
        if inv_abs >= self.max_inventory {
            if self.state != MakerState::Paused {
                debug!(
                    inventory = %self.current_inventory,
                    max = %self.max_inventory,
                    "market_maker: inventory limit hit, pausing"
                );
                self.state = MakerState::Paused;
            }
        } else if self.state == MakerState::Paused {
            debug!("market_maker: inventory reduced, resuming");
            self.state = MakerState::Quoting;
        }

        let half_spread = self.base_spread() / dec!(2);
        let skew = self.inventory_skew();

        // Bid is tighter when short (skew < 0), wider when long (skew > 0).
        let bid_price = mid - half_spread + skew;
        // Ask is tighter when long (skew > 0), wider when short (skew < 0).
        let ask_price = mid + half_spread + skew;

        let mut signals = Vec::new();

        // Cancel old quotes first.
        signals.push(Signal::CancelAll {
            market_id: market_id.clone(),
        });
        self.signals_generated += 1;

        match self.state {
            MakerState::Quoting => {
                // Place bid.
                signals.push(Signal::Enter {
                    id: SignalId::new(),
                    strategy: "market_maker",
                    market_id: market_id.clone(),
                    token_id: token_id.clone(),
                    side: Side::Buy,
                    size: self.quote_size,
                    price: Some(bid_price),
                    edge: half_spread,
                    confidence: dec!(0.50),
                });
                self.signals_generated += 1;

                // Place ask.
                signals.push(Signal::Enter {
                    id: SignalId::new(),
                    strategy: "market_maker",
                    market_id: market_id.clone(),
                    token_id,
                    side: Side::Sell,
                    size: self.quote_size,
                    price: Some(ask_price),
                    edge: half_spread,
                    confidence: dec!(0.50),
                });
                self.signals_generated += 1;
            }

            MakerState::Paused => {
                // Only quote the side that reduces inventory.
                if self.current_inventory > Decimal::ZERO {
                    // Long: only place ask to sell.
                    signals.push(Signal::Enter {
                        id: SignalId::new(),
                        strategy: "market_maker",
                        market_id: market_id.clone(),
                        token_id,
                        side: Side::Sell,
                        size: self.quote_size,
                        price: Some(ask_price),
                        edge: half_spread,
                        confidence: dec!(0.50),
                    });
                    self.signals_generated += 1;
                } else {
                    // Short: only place bid to buy.
                    signals.push(Signal::Enter {
                        id: SignalId::new(),
                        strategy: "market_maker",
                        market_id: market_id.clone(),
                        token_id,
                        side: Side::Buy,
                        size: self.quote_size,
                        price: Some(bid_price),
                        edge: half_spread,
                        confidence: dec!(0.50),
                    });
                    self.signals_generated += 1;
                }
            }
        }

        self.last_quote_time = Some(Instant::now());

        debug!(
            mid = %mid,
            bid = %bid_price,
            ask = %ask_price,
            inventory = %self.current_inventory,
            state = self.state_name(),
            "market_maker: quoted"
        );

        signals
    }

    fn on_fill(&mut self, fill: &FillEvent) {
        match fill.side {
            Side::Buy => self.current_inventory += fill.size,
            Side::Sell => self.current_inventory -= fill.size,
        }
        debug!(
            side = %fill.side,
            size = %fill.size,
            inventory = %self.current_inventory,
            "market_maker: fill, updated inventory"
        );
    }

    fn on_market_change(&mut self, _old: &MarketId, _new: &MarketInfo) {
        self.state = MakerState::Quoting;
        self.current_inventory = Decimal::ZERO;
        self.last_quote_time = None;
        debug!("market_maker: market changed, resetting state");
    }

    fn metrics(&self) -> StrategyMetrics {
        StrategyMetrics {
            name: "market_maker",
            state: self.state_name(),
            edge: Some(self.base_spread() / dec!(2)),
            signals_generated: self.signals_generated,
            custom: vec![
                ("spread_bps", self.spread_bps.to_string()),
                ("inventory", self.current_inventory.to_string()),
                ("max_inv", self.max_inventory.to_string()),
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

    fn make_mm_world(mid_price: Option<Decimal>) -> WorldState {
        let mut markets = HashMap::new();
        let market_id = MarketId("m-mm".into());
        let token_id = TokenId("tok-mm".into());

        let book = OrderbookSnapshot {
            market_id: market_id.clone(),
            token_id: token_id.clone(),
            bids: vec![Level {
                price: dec!(0.49),
                size: dec!(100),
            }],
            asks: vec![Level {
                price: dec!(0.51),
                size: dec!(100),
            }],
            timestamp: ts(0),
        };

        markets.insert(
            market_id.clone(),
            MarketSnapshot {
                info: MarketInfo {
                    id: market_id,
                    question: "Test MM?".into(),
                    slug: "test-mm".into(),
                    outcomes: vec!["Yes".into(), "No".into()],
                    token_ids: vec![token_id],
                    condition_id: "cond-mm".into(),
                    neg_risk: false,
                    active: true,
                    end_date: None,
                    liquidity: dec!(10000),
                    volume: dec!(50000),
                },
                book: Arc::new(book),
                mid_price,
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
            external_prices: HashMap::new(),
            timestamp: ts(0),
        }
    }

    #[test]
    fn test_no_signal_without_mid_price() {
        let mut strat = MarketMaker::new(200, dec!(100), dec!(10), 0);
        let world = make_mm_world(None);
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_no_signal_without_markets() {
        let mut strat = MarketMaker::new(200, dec!(100), dec!(10), 0);
        let mut world = make_mm_world(Some(dec!(0.50)));
        world.markets.clear();
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_quotes_both_sides() {
        let mut strat = MarketMaker::new(200, dec!(100), dec!(10), 0);
        let world = make_mm_world(Some(dec!(0.50)));
        let signals = strat.evaluate(&world);

        // CancelAll + Buy + Sell = 3 signals
        assert_eq!(signals.len(), 3);
        assert!(matches!(&signals[0], Signal::CancelAll { .. }));

        let mut has_buy = false;
        let mut has_sell = false;
        for sig in &signals[1..] {
            match sig {
                Signal::Enter { side: Side::Buy, price, .. } => {
                    has_buy = true;
                    // bid = 0.50 - 0.01 + 0 (no skew) = 0.49
                    assert_eq!(*price, Some(dec!(0.49)));
                }
                Signal::Enter { side: Side::Sell, price, .. } => {
                    has_sell = true;
                    // ask = 0.50 + 0.01 + 0 (no skew) = 0.51
                    assert_eq!(*price, Some(dec!(0.51)));
                }
                other => panic!("unexpected signal: {other:?}"),
            }
        }
        assert!(has_buy, "should have a Buy signal");
        assert!(has_sell, "should have a Sell signal");
    }

    #[test]
    fn test_inventory_skew_when_long() {
        let mut strat = MarketMaker::new(200, dec!(100), dec!(10), 0);
        // Simulate being long 50 units (half of max_inventory)
        strat.current_inventory = dec!(50);

        let world = make_mm_world(Some(dec!(0.50)));
        let signals = strat.evaluate(&world);

        assert_eq!(signals.len(), 3);

        // skew = 50/100 * 0.02/2 = 0.005
        // bid = 0.50 - 0.01 + 0.005 = 0.495 (tighter — less eager to buy)
        // ask = 0.50 + 0.01 + 0.005 = 0.515 (wider — more eager to sell? No!)
        // Actually: skew pushes both prices UP when long. bid goes up (less eager
        // to buy more), ask goes up (less attractive to buyers but we still offer).
        // The skew is: inventory_skew = inv/max * spread/2 / 2
        // Wait: skew = 50/100 * 0.02 / 2 = 0.005
        // bid = mid - half_spread + skew = 0.50 - 0.01 + 0.005 = 0.495
        // ask = mid + half_spread + skew = 0.50 + 0.01 + 0.005 = 0.515

        for sig in &signals[1..] {
            match sig {
                Signal::Enter { side: Side::Buy, price, .. } => {
                    assert_eq!(*price, Some(dec!(0.495)));
                }
                Signal::Enter { side: Side::Sell, price, .. } => {
                    assert_eq!(*price, Some(dec!(0.515)));
                }
                _ => {}
            }
        }
    }

    #[test]
    fn test_inventory_skew_when_short() {
        let mut strat = MarketMaker::new(200, dec!(100), dec!(10), 0);
        strat.current_inventory = dec!(-50);

        let world = make_mm_world(Some(dec!(0.50)));
        let signals = strat.evaluate(&world);

        // skew = -50/100 * 0.02/2 = -0.005
        // bid = 0.50 - 0.01 + (-0.005) = 0.485
        // ask = 0.50 + 0.01 + (-0.005) = 0.505
        for sig in &signals[1..] {
            match sig {
                Signal::Enter { side: Side::Buy, price, .. } => {
                    assert_eq!(*price, Some(dec!(0.485)));
                }
                Signal::Enter { side: Side::Sell, price, .. } => {
                    assert_eq!(*price, Some(dec!(0.505)));
                }
                _ => {}
            }
        }
    }

    #[test]
    fn test_pauses_at_max_inventory() {
        let mut strat = MarketMaker::new(200, dec!(100), dec!(10), 0);
        strat.current_inventory = dec!(100); // at max

        let world = make_mm_world(Some(dec!(0.50)));
        let signals = strat.evaluate(&world);

        // Should be Paused: CancelAll + only Sell (to reduce long inventory)
        assert_eq!(signals.len(), 2);
        assert!(matches!(&signals[0], Signal::CancelAll { .. }));
        match &signals[1] {
            Signal::Enter { side, .. } => {
                assert_eq!(*side, Side::Sell);
            }
            other => panic!("expected Sell Enter, got: {other:?}"),
        }
        assert_eq!(strat.state, MakerState::Paused);
    }

    #[test]
    fn test_pauses_at_max_short_inventory() {
        let mut strat = MarketMaker::new(200, dec!(100), dec!(10), 0);
        strat.current_inventory = dec!(-100); // at max short

        let world = make_mm_world(Some(dec!(0.50)));
        let signals = strat.evaluate(&world);

        // Should be Paused: CancelAll + only Buy (to reduce short inventory)
        assert_eq!(signals.len(), 2);
        assert!(matches!(&signals[0], Signal::CancelAll { .. }));
        match &signals[1] {
            Signal::Enter { side, .. } => {
                assert_eq!(*side, Side::Buy);
            }
            other => panic!("expected Buy Enter, got: {other:?}"),
        }
    }

    #[test]
    fn test_on_fill_updates_inventory() {
        let mut strat = MarketMaker::new(200, dec!(100), dec!(10), 0);
        assert_eq!(strat.current_inventory, Decimal::ZERO);

        // Buy fill
        let fill = FillEvent {
            order_id: OrderId("o1".into()),
            signal_id: SignalId::new(),
            market_id: MarketId("m-mm".into()),
            side: Side::Buy,
            price: dec!(0.49),
            size: dec!(10),
            timestamp: ts(0),
        };
        strat.on_fill(&fill);
        assert_eq!(strat.current_inventory, dec!(10));

        // Sell fill
        let fill = FillEvent {
            order_id: OrderId("o2".into()),
            signal_id: SignalId::new(),
            market_id: MarketId("m-mm".into()),
            side: Side::Sell,
            price: dec!(0.51),
            size: dec!(5),
            timestamp: ts(1),
        };
        strat.on_fill(&fill);
        assert_eq!(strat.current_inventory, dec!(5));
    }

    #[test]
    fn test_refresh_interval_throttles_quotes() {
        // 10 second refresh interval
        let mut strat = MarketMaker::new(200, dec!(100), dec!(10), 10_000);

        let world = make_mm_world(Some(dec!(0.50)));

        // First evaluate: should quote
        let signals = strat.evaluate(&world);
        assert_eq!(signals.len(), 3);

        // Second evaluate immediately: should be throttled
        let signals = strat.evaluate(&world);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_resumes_from_paused_when_inventory_reduces() {
        let mut strat = MarketMaker::new(200, dec!(100), dec!(10), 0);
        strat.current_inventory = dec!(100); // at max

        let world = make_mm_world(Some(dec!(0.50)));
        strat.evaluate(&world);
        assert_eq!(strat.state, MakerState::Paused);

        // Simulate a sell fill reducing inventory
        let fill = FillEvent {
            order_id: OrderId("o1".into()),
            signal_id: SignalId::new(),
            market_id: MarketId("m-mm".into()),
            side: Side::Sell,
            price: dec!(0.51),
            size: dec!(50),
            timestamp: ts(0),
        };
        strat.on_fill(&fill);
        assert_eq!(strat.current_inventory, dec!(50));

        // Force requote and evaluate — should resume quoting
        strat.force_requote();
        let signals = strat.evaluate(&world);
        assert_eq!(strat.state, MakerState::Quoting);
        // Should have CancelAll + Buy + Sell = 3
        assert_eq!(signals.len(), 3);
    }
}
