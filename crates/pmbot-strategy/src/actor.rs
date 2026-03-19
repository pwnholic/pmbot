//! Strategy actor — the tick-driven signal generation loop.
//!
//! The [`StrategyActor`] ingests [`MarketEvent`], [`FeedEvent`], and
//! [`ExecutionEvent`] streams, builds a [`WorldState`] snapshot every tick,
//! and evaluates all registered strategies to produce [`Signal`]s.

use std::time::Duration;

use rust_decimal::Decimal;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use pmbot_core::messages::{ExecutionEvent, FeedEvent, MarketEvent, PositionSnapshot, Signal};

use crate::context::WorldStateBuilder;
use crate::registry::StrategyRegistry;

/// Actor that evaluates strategies on a periodic tick.
pub struct StrategyActor {
    registry: StrategyRegistry,
    world_builder: WorldStateBuilder,
    market_rx: broadcast::Receiver<MarketEvent>,
    feed_rx: broadcast::Receiver<FeedEvent>,
    execution_rx: broadcast::Receiver<ExecutionEvent>,
    position_rx: tokio::sync::watch::Receiver<PositionSnapshot>,
    signal_tx: mpsc::Sender<Signal>,
    world_tx: tokio::sync::watch::Sender<pmbot_core::messages::WorldState>,
    tui_tx: Option<tokio::sync::watch::Sender<Option<(pmbot_core::messages::WorldState, Vec<pmbot_core::messages::StrategyMetrics>)>>>,
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
        position_rx: tokio::sync::watch::Receiver<PositionSnapshot>,
        signal_tx: mpsc::Sender<Signal>,
        world_tx: tokio::sync::watch::Sender<pmbot_core::messages::WorldState>,
        tui_tx: Option<tokio::sync::watch::Sender<Option<(pmbot_core::messages::WorldState, Vec<pmbot_core::messages::StrategyMetrics>)>>>,
        tick_interval_ms: u64,
    ) -> Self {
        Self {
            registry,
            world_builder: WorldStateBuilder::new(initial_balance),
            market_rx,
            feed_rx,
            execution_rx,
            position_rx,
            signal_tx,
            world_tx,
            tui_tx,
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
                            
                            // Re-evaluate strategies whenever the orderbook/price updates
                            // or on market rotation
                            let should_evaluate = match &event {
                                pmbot_core::messages::MarketEvent::BookUpdate { .. } => true,
                                pmbot_core::messages::MarketEvent::PriceChange { .. } => true,
                                pmbot_core::messages::MarketEvent::MarketRotation { old, new } => {
                                    // Cancel all open orders for the old market
                                    let _ = self.signal_tx.send(pmbot_core::messages::Signal::CancelAll {
                                        market_id: old.clone(),
                                    }).await;
                                    info!(%old, new_market = %new.id, "market rotation — sent CancelAll for old market");
                                    for strategy in self.registry.iter_mut() {
                                        strategy.on_market_change(&old, &new);
                                    }
                                    true
                                }
                                _ => false,
                            };

                            if should_evaluate {
                                if !self.evaluate_and_send().await {
                                    return;
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
                        Ok(event) => {
                            self.world_builder.apply_feed_event(&event);
                            // Feed updates (like Binance price) should trigger evaluation 
                            // especially for LeadLag / FairValue
                            if !self.evaluate_and_send().await {
                                return;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(missed = n, "feed event channel lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("feed event channel closed");
                            break;
                        }
                    }
                }
                result = self.position_rx.changed() => {
                    match result {
                        Ok(()) => {
                            let snapshot = self.position_rx.borrow_and_update().clone();
                            self.world_builder.set_positions(snapshot.positions);
                            self.world_builder.set_daily_pnl(snapshot.daily_pnl);
                            // Push updated world to TUI immediately
                            if let Some(tx) = &self.tui_tx {
                                let world = self.world_builder.snapshot();
                                let metrics: Vec<_> = self.registry.iter_mut().map(|s| s.metrics()).collect();
                                let _ = tx.send(Some((world, metrics)));
                            }
                        }
                        Err(_) => {
                            info!("position channel closed");
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
                                pmbot_core::messages::ExecutionEvent::OrderPartialFill {
                                    order_id,
                                    signal_id,
                                    market_id,
                                    side,
                                    price,
                                    filled,
                                    ..
                                } => {
                                    let world = self.world_builder.snapshot();
                                    if let Some(order) = world.open_orders.iter().find(|o| o.order_id == order_id) {
                                        let delta = filled.saturating_sub(order.filled);
                                        if delta > Decimal::ZERO {
                                            let fill_event = pmbot_core::types::FillEvent {
                                                order_id,
                                                signal_id,
                                                market_id,
                                                side,
                                                price,
                                                size: delta,
                                                timestamp: chrono::Utc::now(),
                                            };
                                            for strategy in self.registry.iter_mut() {
                                                strategy.on_fill(&fill_event);
                                            }
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
            }
        }

        info!("StrategyActor shutting down");
    }

    /// Evaluates strategies and sends signals. Returns `true` if it should continue, `false` if the channel is closed.
    async fn evaluate_and_send(&mut self) -> bool {
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
                    return false;
                }
            }
        }

        // Send world state to RiskActor for unrealized PnL computation
        let _ = self.world_tx.send(world.clone());
        
        if let Some(tx) = &self.tui_tx {
            let metrics: Vec<_> = self.registry.iter_mut().map(|s| s.metrics()).collect();
            let _ = tx.send(Some((world, metrics)));
        }

        true
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
                let outcome = snap.info.outcomes.first().cloned().unwrap_or_default();
                vec![Signal::Enter {
                    id: SignalId::new(),
                    strategy: "always_enter",
                    market_id: market_id.clone(),
                    token_id,
                    outcome,
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
                trades: 0,
                wins: 0,
                losses: 0,
                total_pnl: Decimal::ZERO,
                pnl_history: Vec::new(),
                custom: Vec::new(),
            }
        }
    }

    fn sample_market_info() -> MarketInfo {
        use std::collections::HashMap;
        let mut outcome_prices = HashMap::new();
        outcome_prices.insert("Yes".to_string(), dec!(0.50));
        outcome_prices.insert("No".to_string(), dec!(0.50));
        MarketInfo {
            id: MarketId("m-1".into()),
            question: "Test?".into(),
            slug: "test".into(),
            outcomes: vec!["Yes".into(), "No".into()],
            token_ids: vec![TokenId("tok".into())],
            outcome_prices,
            condition_id: "cond".into(),
            neg_risk: false,
            active: true,
            end_date: None,
            liquidity: dec!(10000),
            volume: dec!(50000),
            category: "Test".into(),
            tags: vec!["test".into()],
        }
    }

    #[tokio::test]
    async fn test_strategy_actor_emits_signals_on_tick() {
        let (market_tx, market_rx) = broadcast::channel(16);
        let (feed_tx, feed_rx) = broadcast::channel(16);
        let (exec_tx, exec_rx) = broadcast::channel(16);
        let (signal_tx, mut signal_rx) = mpsc::channel(16);
        let (_pos_tx, pos_rx) = tokio::sync::watch::channel(PositionSnapshot { positions: vec![], daily_pnl: Decimal::ZERO });
        let (world_tx, _world_rx) = tokio::sync::watch::channel(pmbot_core::messages::WorldState::default());

        let mut registry = StrategyRegistry::new();
        registry.register(Box::new(AlwaysEnterStrategy::new()));

        let actor = StrategyActor::new(
            registry,
            dec!(1000),
            market_rx,
            feed_rx,
            exec_rx,
            pos_rx,
            signal_tx,
            world_tx,
            None,
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

        // Wait for signals — first may be CancelAll from rotation, then Enter
        let mut found_enter = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout_at(deadline, signal_rx.recv()).await {
                Ok(Some(Signal::Enter { strategy, side, .. })) => {
                    assert_eq!(strategy, "always_enter");
                    assert_eq!(side, Side::Buy);
                    found_enter = true;
                    break;
                }
                Ok(Some(Signal::CancelAll { .. })) => {
                    // Expected on market rotation — skip and wait for Enter
                    continue;
                }
                Ok(Some(other)) => panic!("unexpected signal: {other:?}"),
                Ok(None) => panic!("signal channel closed"),
                Err(_) => panic!("timeout waiting for Enter signal"),
            }
        }
        assert!(found_enter, "never received Enter signal");

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
        let (_pos_tx, pos_rx) = tokio::sync::watch::channel(PositionSnapshot { positions: vec![], daily_pnl: Decimal::ZERO });
        let (world_tx, _world_rx) = tokio::sync::watch::channel(pmbot_core::messages::WorldState::default());

        let mut registry = StrategyRegistry::new();
        registry.register(Box::new(AlwaysEnterStrategy::new()));

        let actor = StrategyActor::new(
            registry,
            dec!(1000),
            market_rx,
            feed_rx,
            exec_rx,
            pos_rx,
            signal_tx,
            world_tx,
            None,
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
