//! Strategy actor — the tick-driven signal generation loop.
//!
//! The [`StrategyActor`] ingests [`MarketEvent`], [`FeedEvent`], and
//! [`ExecutionEvent`] streams, builds a [`WorldState`] snapshot every tick,
//! and evaluates all registered strategies to produce [`Signal`]s.

use std::time::Duration;

use rust_decimal::Decimal;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use pmbot_core::messages::{ExecutionEvent, FeedEvent, MarketEvent, Signal};

use crate::context::WorldStateBuilder;
use crate::registry::StrategyRegistry;

/// Actor that evaluates strategies on a periodic tick.
pub struct StrategyActor {
    registry: StrategyRegistry,
    world_builder: WorldStateBuilder,
    market_rx: broadcast::Receiver<MarketEvent>,
    feed_rx: broadcast::Receiver<FeedEvent>,
    execution_rx: broadcast::Receiver<ExecutionEvent>,
    signal_tx: mpsc::Sender<Signal>,
    tick_interval_ms: u64,
}

impl StrategyActor {
    /// Create a new strategy actor.
    pub fn new(
        registry: StrategyRegistry,
        initial_balance: Decimal,
        market_rx: broadcast::Receiver<MarketEvent>,
        feed_rx: broadcast::Receiver<FeedEvent>,
        execution_rx: broadcast::Receiver<ExecutionEvent>,
        signal_tx: mpsc::Sender<Signal>,
        tick_interval_ms: u64,
    ) -> Self {
        Self {
            registry,
            world_builder: WorldStateBuilder::new(initial_balance),
            market_rx,
            feed_rx,
            execution_rx,
            signal_tx,
            tick_interval_ms,
        }
    }

    /// Run the strategy actor loop.
    ///
    /// Processes incoming events and evaluates all strategies on each tick.
    /// Returns when the signal sender is closed or all event sources drop.
    pub async fn run(mut self) {
        info!(
            tick_ms = self.tick_interval_ms,
            strategies = self.registry.len(),
            "StrategyActor started"
        );

        let mut tick =
            tokio::time::interval(Duration::from_millis(self.tick_interval_ms));
        // Don't burst ticks if we fall behind.
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                result = self.market_rx.recv() => {
                    match result {
                        Ok(event) => {
                            self.world_builder.apply_market_event(&event);
                            if let pmbot_core::messages::MarketEvent::MarketRotation { old, new } = event {
                                for strategy in self.registry.iter_mut() {
                                    strategy.on_market_change(&old, &new);
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(missed = n, "market event channel lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("market event channel closed");
                            break;
                        }
                    }
                }
                result = self.feed_rx.recv() => {
                    match result {
                        Ok(event) => self.world_builder.apply_feed_event(&event),
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(missed = n, "feed event channel lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("feed event channel closed");
                            break;
                        }
                    }
                }
                result = self.execution_rx.recv() => {
                    match result {
                        Ok(event) => {
                            self.world_builder.apply_execution_event(&event);
                            match event {
                                pmbot_core::messages::ExecutionEvent::OrderFilled { order_id, signal_id, market_id, side, price, size } => {
                                    let fill_event = pmbot_core::types::FillEvent {
                                        order_id: order_id.clone(),
                                        signal_id: signal_id.clone(),
                                        market_id: market_id.clone(),
                                        side: side.clone(),
                                        price: price.clone(),
                                        size: size.clone(),
                                        timestamp: chrono::Utc::now(),
                                    };
                                    for strategy in self.registry.iter_mut() {
                                        strategy.on_fill(&fill_event);
                                    }
                                }
                                pmbot_core::messages::ExecutionEvent::OrderPartialFill { order_id, filled, remaining: _ } => {
                                    // Try to reconstruct FillEvent from open orders if possible
                                    let world = self.world_builder.snapshot();
                                    if let Some(order) = world.open_orders.iter().find(|o| o.order_id == order_id.clone()) {
                                        let fill_event = pmbot_core::types::FillEvent {
                                            order_id: order_id.clone(),
                                            signal_id: pmbot_core::types::SignalId::new(), // Partial fill doesn't carry signal_id
                                            market_id: order.market_id.clone(),
                                            side: order.side.clone(),
                                            price: order.price.clone(),
                                            size: filled.clone(), // Size is the total filled amount here
                                            timestamp: chrono::Utc::now(),
                                        };
                                        for strategy in self.registry.iter_mut() {
                                            strategy.on_fill(&fill_event);
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(missed = n, "execution event channel lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("execution event channel closed");
                            break;
                        }
                    }
                }
                _ = tick.tick() => {
                    let world = self.world_builder.snapshot();
                    for strategy in self.registry.iter_mut() {
                        let signals = strategy.evaluate(&world);
                        for signal in signals {
                            debug!(
                                strategy = signal.strategy_name(),
                                "emitting signal"
                            );
                            if self.signal_tx.send(signal).await.is_err() {
                                info!("signal channel closed, shutting down");
                                return;
                            }
                        }
                    }
                }
            }
        }

        info!("StrategyActor shutting down");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::Strategy;
    use pmbot_core::messages::{StrategyMetrics, WorldState};
    use pmbot_core::types::*;
    use rust_decimal_macros::dec;

    /// Strategy that emits an Enter signal on every tick if markets exist.
    struct AlwaysEnterStrategy {
        count: u64,
    }

    impl AlwaysEnterStrategy {
        fn new() -> Self {
            Self { count: 0 }
        }
    }

    impl Strategy for AlwaysEnterStrategy {
        fn name(&self) -> &'static str {
            "always_enter"
        }

        fn evaluate(&mut self, world: &WorldState) -> Vec<Signal> {
            if let Some((market_id, snap)) = world.active_market_id.as_ref().and_then(|id| world.markets.get(id).map(|snap| (id, snap))) {
                self.count += 1;
                let token_id = snap.info.token_ids.first().cloned()
                    .unwrap_or(TokenId(String::new()));
                vec![Signal::Enter {
                    id: SignalId::new(),
                    strategy: "always_enter",
                    market_id: market_id.clone(),
                    token_id,
                    side: Side::Buy,
                    size: dec!(1),
                    price: Some(dec!(0.50)),
                    edge: dec!(0.05),
                    confidence: dec!(0.80),
                }]
            } else {
                Vec::new()
            }
        }

        fn on_fill(&mut self, _fill: &FillEvent) {}

        fn on_market_change(&mut self, _old: &MarketId, _new: &MarketInfo) {}

        fn metrics(&self) -> StrategyMetrics {
            StrategyMetrics {
                name: "always_enter",
                state: "active",
                edge: Some(dec!(0.05)),
                signals_generated: self.count,
                custom: Vec::new(),
            }
        }
    }

    fn sample_market_info() -> MarketInfo {
        MarketInfo {
            id: MarketId("m-1".into()),
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
        }
    }

    #[tokio::test]
    async fn test_strategy_actor_emits_signals_on_tick() {
        let (market_tx, market_rx) = broadcast::channel(16);
        let (feed_tx, feed_rx) = broadcast::channel(16);
        let (exec_tx, exec_rx) = broadcast::channel(16);
        let (signal_tx, mut signal_rx) = mpsc::channel(16);

        let mut registry = StrategyRegistry::new();
        registry.register(Box::new(AlwaysEnterStrategy::new()));

        let actor = StrategyActor::new(
            registry,
            dec!(1000),
            market_rx,
            feed_rx,
            exec_rx,
            signal_tx,
            50, // 50ms tick
        );

        let handle = tokio::spawn(actor.run());

        // Inject a market so the strategy has something to trade.
        market_tx
            .send(MarketEvent::MarketRotation {
                old: MarketId("old".into()),
                new: sample_market_info(),
            })
            .unwrap();

        // Wait for at least one signal.
        let signal = tokio::time::timeout(
            Duration::from_secs(2),
            signal_rx.recv(),
        )
        .await
        .expect("timeout waiting for signal")
        .expect("signal channel closed");

        match signal {
            Signal::Enter { strategy, side, .. } => {
                assert_eq!(strategy, "always_enter");
                assert_eq!(side, Side::Buy);
            }
            other => panic!("expected Enter signal, got: {other:?}"),
        }

        // Shut down by dropping all senders.
        drop(market_tx);
        drop(feed_tx);
        drop(exec_tx);
        drop(signal_rx);

        // Give actor time to notice shutdown.
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    }

    #[tokio::test]
    async fn test_strategy_actor_no_signals_without_markets() {
        let (_market_tx, market_rx) = broadcast::channel(16);
        let (_feed_tx, feed_rx) = broadcast::channel(16);
        let (_exec_tx, exec_rx) = broadcast::channel(16);
        let (signal_tx, mut signal_rx) = mpsc::channel(16);

        let mut registry = StrategyRegistry::new();
        registry.register(Box::new(AlwaysEnterStrategy::new()));

        let actor = StrategyActor::new(
            registry,
            dec!(1000),
            market_rx,
            feed_rx,
            exec_rx,
            signal_tx,
            50,
        );

        let _handle = tokio::spawn(actor.run());

        // With no market rotation, the strategy should produce no signals.
        let result = tokio::time::timeout(
            Duration::from_millis(200),
            signal_rx.recv(),
        )
        .await;

        assert!(result.is_err(), "expected timeout (no signals)");
    }
}
