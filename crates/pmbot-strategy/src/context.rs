//! WorldState builder — accumulates events into immutable snapshots.
//!
//! The [`WorldStateBuilder`] is the strategy actor's local state. It
//! ingests [`MarketEvent`], [`FeedEvent`], and [`ExecutionEvent`] messages
//! and produces a frozen [`WorldState`] snapshot on each tick.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use rust_decimal::Decimal;

use pmbot_core::messages::{
    ExecutionEvent, FeedEvent, MarketEvent, MarketSnapshot, Position, WorldState,
};
use pmbot_core::types::{MarketId, OpenOrder, PricePoint, SpotPrice, Symbol};
use std::time::Duration;

/// Accumulates events and builds immutable [`WorldState`] snapshots.
pub struct WorldStateBuilder {
    active_market_id: Option<MarketId>,
    markets: HashMap<MarketId, MarketSnapshot>,
    positions: Vec<Position>,
    open_orders: Vec<OpenOrder>,
    balance: Decimal,
    daily_pnl: Decimal,
    external_prices: HashMap<Symbol, SpotPrice>,
    network_latency: HashMap<&'static str, Duration>,
}

impl WorldStateBuilder {
    /// Create a new builder with the given initial USDC balance.
    pub fn new(initial_balance: Decimal) -> Self {
        Self {
            active_market_id: None,
            markets: HashMap::new(),
            positions: Vec::new(),
            open_orders: Vec::new(),
            balance: initial_balance,
            daily_pnl: Decimal::ZERO,
            external_prices: HashMap::new(),
            network_latency: HashMap::new(),
        }
    }

    /// Apply a market event (book update, price change, rotation).
    pub fn apply_market_event(&mut self, event: &MarketEvent) {
        match event {
            MarketEvent::BookUpdate { market_id, book } => {
                if let Some(snap) = self.markets.get_mut(market_id) {
                    snap.mid_price = book.mid_price();
                    snap.spread = book.spread();
                    snap.book = Arc::clone(book);

                    // Append to price history if we have a mid price.
                    if let Some(mid) = snap.mid_price {
                        snap.price_history.push(PricePoint {
                            price: mid,
                            timestamp: book.timestamp,
                        });
                        // Cap price history to prevent unbounded memory growth
                        if snap.price_history.len() > 3000 {
                            snap.price_history.drain(0..500);
                        }
                    }
                }
            }
            MarketEvent::PriceChange {
                market_id,
                price,
                ..
            } => {
                if let Some(snap) = self.markets.get_mut(market_id) {
                    snap.mid_price = Some(*price);
                }
            }
            MarketEvent::MarketRotation { old, new } => {
                // Remove the old market snapshot.
                self.markets.remove(old);

                self.active_market_id = Some(new.id.clone());
                // Insert a fresh snapshot for the new market.
                let snap = MarketSnapshot {
                    info: new.clone(),
                    book: Arc::new(pmbot_core::types::OrderbookSnapshot {
                        market_id: new.id.clone(),
                        token_id: new
                            .token_ids
                            .first()
                            .cloned()
                            .unwrap_or(pmbot_core::types::TokenId(String::new())),
                        bids: Vec::new(),
                        asks: Vec::new(),
                        timestamp: Utc::now(),
                    }),
                    mid_price: None,
                    spread: None,
                    imbalance: Decimal::ZERO,
                    price_history: Vec::new(),
                };
                self.markets.insert(new.id.clone(), snap);
            }
            MarketEvent::LatencyUpdate { latency } => {
                self.network_latency.insert("polymarket", *latency);
            }
            MarketEvent::Connected | MarketEvent::Disconnected => {
                // No state changes needed for connection events.
            }
        }
    }

    /// Apply a feed event (external spot price or vol update).
    pub fn apply_feed_event(&mut self, event: &FeedEvent) {
        match event {
            FeedEvent::SpotPrice {
                symbol,
                price,
                timestamp,
            } => {
                self.external_prices.insert(
                    symbol.clone(),
                    SpotPrice {
                        price: *price,
                        timestamp: *timestamp,
                    },
                );
            }
            FeedEvent::VolUpdate { .. } => {
                // Vol updates are informational; strategies can query
                // the feed directly if needed. No world state change.
            }
            FeedEvent::LatencyUpdate { latency } => {
                self.network_latency.insert("binance", *latency);
            }
        }
    }

    /// Apply an execution event (order placed, filled, cancelled, etc.).
    pub fn apply_execution_event(&mut self, event: &ExecutionEvent) {
        match event {
            ExecutionEvent::OrderPlaced { order_id, .. } => {
                // Order tracking is handled externally; we just note it.
                tracing::debug!(order_id = %order_id, "order placed recorded in world state");
            }
            ExecutionEvent::OrderFilled {
                order_id,
                price,
                size,
                side,
                ..
            } => {
                // Remove the order from open orders.
                self.open_orders.retain(|o| o.order_id != *order_id);

                // Adjust balance based on fill.
                let cost = *price * *size;
                match side {
                    pmbot_core::types::Side::Buy => {
                        self.balance -= cost;
                    }
                    pmbot_core::types::Side::Sell => {
                        self.balance += cost;
                    }
                }
            }
            ExecutionEvent::OrderPartialFill {
                order_id, filled, ..
            } => {
                if let Some(order) = self.open_orders.iter_mut().find(|o| o.order_id == *order_id)
                {
                    order.filled = *filled;
                }
            }
            ExecutionEvent::OrderCancelled { order_id } => {
                self.open_orders.retain(|o| o.order_id != *order_id);
            }
            ExecutionEvent::OrderRejected { .. } => {
                // Nothing to update; the order never existed.
            }
        }
    }

    /// Replace the tracked positions.
    pub fn set_positions(&mut self, positions: Vec<Position>) {
        self.positions = positions;
    }

    /// Replace the tracked open orders.
    pub fn set_open_orders(&mut self, orders: Vec<OpenOrder>) {
        self.open_orders = orders;
    }

    /// Set the current USDC balance.
    pub fn set_balance(&mut self, balance: Decimal) {
        self.balance = balance;
    }

    /// Set the daily P&L figure.
    pub fn set_daily_pnl(&mut self, pnl: Decimal) {
        self.daily_pnl = pnl;
    }

    /// Build an immutable snapshot of the current world state.
    pub fn snapshot(&self) -> WorldState {
        WorldState {
            active_market_id: self.active_market_id.clone(),
            markets: self.markets.clone(),
            positions: self.positions.clone(),
            open_orders: self.open_orders.clone(),
            balance: self.balance,
            daily_pnl: self.daily_pnl,
            external_prices: self.external_prices.clone(),
            network_latency: self.network_latency.clone(),
            timestamp: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use pmbot_core::types::*;
    use rust_decimal_macros::dec;

    fn ts(secs: i64) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn sample_market_info() -> MarketInfo {
        MarketInfo {
            id: MarketId("market-1".into()),
            question: "Will BTC > 100k?".into(),
            slug: "btc-100k".into(),
            outcomes: vec!["Yes".into(), "No".into()],
            token_ids: vec![TokenId("tok-yes".into()), TokenId("tok-no".into())],
            condition_id: "cond-1".into(),
            neg_risk: false,
            active: true,
            end_date: None,
            liquidity: dec!(50000),
            volume: dec!(100000),
        }
    }

    fn sample_book(market_id: &MarketId) -> OrderbookSnapshot {
        OrderbookSnapshot {
            market_id: market_id.clone(),
            token_id: TokenId("tok-yes".into()),
            bids: vec![
                Level {
                    price: dec!(0.55),
                    size: dec!(100),
                },
                Level {
                    price: dec!(0.54),
                    size: dec!(200),
                },
            ],
            asks: vec![
                Level {
                    price: dec!(0.56),
                    size: dec!(100),
                },
                Level {
                    price: dec!(0.57),
                    size: dec!(200),
                },
            ],
            timestamp: ts(0),
        }
    }

    #[test]
    fn test_new_builder_has_correct_balance() {
        let builder = WorldStateBuilder::new(dec!(1000));
        let snap = builder.snapshot();
        assert_eq!(snap.balance, dec!(1000));
        assert_eq!(snap.daily_pnl, Decimal::ZERO);
        assert!(snap.markets.is_empty());
        assert!(snap.positions.is_empty());
    }

    #[test]
    fn test_apply_market_rotation_adds_market() {
        let mut builder = WorldStateBuilder::new(dec!(1000));
        let info = sample_market_info();

        builder.apply_market_event(&MarketEvent::MarketRotation {
            old: MarketId("old".into()),
            new: info.clone(),
        });

        let snap = builder.snapshot();
        assert_eq!(snap.markets.len(), 1);
        assert!(snap.markets.contains_key(&info.id));
        assert_eq!(snap.markets[&info.id].info.question, "Will BTC > 100k?");
    }

    #[test]
    fn test_apply_book_update_sets_mid_and_spread() {
        let mut builder = WorldStateBuilder::new(dec!(1000));
        let info = sample_market_info();

        // First add the market via rotation.
        builder.apply_market_event(&MarketEvent::MarketRotation {
            old: MarketId("old".into()),
            new: info.clone(),
        });

        // Then apply a book update.
        let book = Arc::new(sample_book(&info.id));
        builder.apply_market_event(&MarketEvent::BookUpdate {
            market_id: info.id.clone(),
            book,
        });

        let snap = builder.snapshot();
        let ms = &snap.markets[&info.id];
        assert_eq!(ms.mid_price, Some(dec!(0.555)));
        assert_eq!(ms.spread, Some(dec!(0.01)));
        assert_eq!(ms.price_history.len(), 1);
    }

    #[test]
    fn test_apply_feed_event_spot_price() {
        let mut builder = WorldStateBuilder::new(dec!(1000));
        let sym = Symbol("BTCUSDT".into());

        builder.apply_feed_event(&FeedEvent::SpotPrice {
            symbol: sym.clone(),
            price: dec!(50000),
            timestamp: ts(0),
        });

        let snap = builder.snapshot();
        assert!(snap.external_prices.contains_key(&sym));
        assert_eq!(snap.external_prices[&sym].price, dec!(50000));
    }

    #[test]
    fn test_apply_execution_event_fill_adjusts_balance() {
        let mut builder = WorldStateBuilder::new(dec!(1000));

        builder.apply_execution_event(&ExecutionEvent::OrderFilled {
            order_id: OrderId("o-1".into()),
            signal_id: SignalId::new(),
            market_id: MarketId("m-1".into()),
            side: pmbot_core::types::Side::Buy,
            price: dec!(0.50),
            size: dec!(100),
        });

        let snap = builder.snapshot();
        // Bought 100 @ 0.50 = cost 50
        assert_eq!(snap.balance, dec!(950));
    }

    #[test]
    fn test_apply_execution_event_sell_increases_balance() {
        let mut builder = WorldStateBuilder::new(dec!(1000));

        builder.apply_execution_event(&ExecutionEvent::OrderFilled {
            order_id: OrderId("o-1".into()),
            signal_id: SignalId::new(),
            market_id: MarketId("m-1".into()),
            side: pmbot_core::types::Side::Sell,
            price: dec!(0.60),
            size: dec!(100),
        });

        let snap = builder.snapshot();
        // Sold 100 @ 0.60 = +60
        assert_eq!(snap.balance, dec!(1060));
    }

    #[test]
    fn test_apply_execution_cancel_removes_order() {
        let mut builder = WorldStateBuilder::new(dec!(1000));
        builder.set_open_orders(vec![OpenOrder {
            order_id: OrderId("o-1".into()),
            market_id: MarketId("m-1".into()),
            token_id: TokenId("tok".into()),
            side: pmbot_core::types::Side::Buy,
            price: dec!(0.50),
            size: dec!(10),
            filled: Decimal::ZERO,
            order_type: OrderType::Gtc,
        }]);

        assert_eq!(builder.snapshot().open_orders.len(), 1);

        builder.apply_execution_event(&ExecutionEvent::OrderCancelled {
            order_id: OrderId("o-1".into()),
        });

        assert!(builder.snapshot().open_orders.is_empty());
    }

    #[test]
    fn test_set_positions_and_pnl() {
        let mut builder = WorldStateBuilder::new(dec!(1000));

        builder.set_positions(vec![Position {
            id: PositionId::new(),
            market_id: MarketId("m-1".into()),
            token_id: TokenId("tok".into()),
            strategy: "lead_lag",
            side: pmbot_core::types::Side::Buy,
            entry_price: dec!(0.50),
            size: dec!(10),
            tp_price: dec!(0.60),
            sl_price: dec!(0.40),
            opened_at: ts(0),
            unrealized_pnl: dec!(5),
        }]);

        builder.set_daily_pnl(dec!(42));

        let snap = builder.snapshot();
        assert_eq!(snap.positions.len(), 1);
        assert_eq!(snap.daily_pnl, dec!(42));
    }

    #[test]
    fn test_price_change_updates_mid_price() {
        let mut builder = WorldStateBuilder::new(dec!(1000));
        let info = sample_market_info();

        builder.apply_market_event(&MarketEvent::MarketRotation {
            old: MarketId("old".into()),
            new: info.clone(),
        });

        builder.apply_market_event(&MarketEvent::PriceChange {
            market_id: info.id.clone(),
            token_id: TokenId("tok-yes".into()),
            price: dec!(0.62),
        });

        let snap = builder.snapshot();
        assert_eq!(snap.markets[&info.id].mid_price, Some(dec!(0.62)));
    }

    #[test]
    fn test_connected_disconnected_no_crash() {
        let mut builder = WorldStateBuilder::new(dec!(1000));
        builder.apply_market_event(&MarketEvent::Connected);
        builder.apply_market_event(&MarketEvent::Disconnected);
        // Should not panic or change state.
        let snap = builder.snapshot();
        assert!(snap.markets.is_empty());
    }
}
