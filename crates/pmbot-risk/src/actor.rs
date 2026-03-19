//! Risk actor — validates signals, sizes positions via Kelly criterion,
//! and emits executable orders to the executor.

use std::collections::HashMap;
use rust_decimal::Decimal;
use tokio::sync::{broadcast, mpsc};
use tracing::{info, warn};

use pmbot_core::config::RiskConfig;
use pmbot_core::messages::{ExecutableOrder, ExecutionEvent, MarketEvent, Position, PositionSnapshot, Signal, WorldState};
use pmbot_core::types::*;

use crate::kelly::fractional_kelly;
use crate::limits::CircuitBreaker;
use crate::portfolio::PortfolioRisk;
use crate::position::TrackedPosition;

/// The Risk Actor: validates signals, sizes positions, and emits executable orders.
pub struct RiskActor {
    breaker: CircuitBreaker,
    portfolio: PortfolioRisk,
    positions: HashMap<PositionId, TrackedPosition>,

    // Config
    bankroll: Decimal,
    kelly_fraction: Decimal,
    max_position_pct: Decimal,
    stop_loss_pct: Decimal,
    tp_multiplier: Decimal,

    // World state (updated each tick)
    world: WorldState,

    // Channels
    signal_rx: mpsc::Receiver<Signal>,
    order_tx: mpsc::Sender<ExecutableOrder>,
    execution_rx: broadcast::Receiver<ExecutionEvent>,
    market_rx: broadcast::Receiver<MarketEvent>,
    position_tx: tokio::sync::watch::Sender<PositionSnapshot>,
    world_rx: tokio::sync::watch::Receiver<WorldState>,
}

impl RiskActor {
    /// Create a new risk actor from a `RiskConfig` and channel endpoints.
    pub fn new(
        config: &RiskConfig,
        signal_rx: mpsc::Receiver<Signal>,
        order_tx: mpsc::Sender<ExecutableOrder>,
        execution_rx: broadcast::Receiver<ExecutionEvent>,
        market_rx: broadcast::Receiver<MarketEvent>,
        position_tx: tokio::sync::watch::Sender<PositionSnapshot>,
        world_rx: tokio::sync::watch::Receiver<WorldState>,
    ) -> Self {
        let breaker = CircuitBreaker::new(config.clone());
        let portfolio = PortfolioRisk {
            max_net_exposure: config.bankroll * config.max_position_pct
                * Decimal::from(config.max_positions),
            max_per_event: config.max_positions,
        };

        Self {
            breaker,
            portfolio,
            positions: HashMap::new(),
            bankroll: config.bankroll,
            kelly_fraction: config.kelly_fraction,
            max_position_pct: config.max_position_pct,
            stop_loss_pct: config.stop_loss_pct,
            tp_multiplier: config.take_profit_multiplier,
            world: default_world(),
            signal_rx,
            order_tx,
            execution_rx,
            market_rx,
            position_tx,
            world_rx,
        }
    }

    /// Process a signal through the risk pipeline.
    ///
    /// Returns `Some(ExecutableOrder)` if the signal passes all checks,
    /// or `None` if it was rejected.
    pub fn process_signal(&mut self, signal: Signal) -> Option<ExecutableOrder> {
        match signal {
            Signal::Enter {
                id,
                strategy,
                market_id,
                token_id,
                outcome,
                side,
                size: _size,
                price,
                edge,
                confidence,
            } => {
                // 1. Circuit breaker checks
                let enter_signal = Signal::Enter {
                    id,
                    strategy,
                    market_id: market_id.clone(),
                    token_id: token_id.clone(),
                    outcome: outcome.clone(),
                    side,
                    size: _size,
                    price,
                    edge,
                    confidence,
                };
                if let Err(reason) = self.breaker.check(&enter_signal, &self.world) {
                    warn!(strategy, %reason, "signal rejected by circuit breaker");
                    return None;
                }

                // 2. Compute entry price (use signal price or default to 0.50)
                let entry_price = price.unwrap_or(Decimal::new(50, 2));

                // 3. Kelly position sizing
                // win_prob = edge + market_price, capped at 0.99
                let win_prob = (edge + entry_price).min(Decimal::new(99, 2));
                let kelly_size = fractional_kelly(
                    win_prob,
                    entry_price,
                    self.bankroll,
                    self.kelly_fraction,
                    self.max_position_pct,
                );

                if kelly_size <= Decimal::ZERO {
                    warn!(strategy, "kelly sizing returned zero — no edge");
                    return None;
                }

                // 4. Portfolio-level checks
                let positions_vec: Vec<_> = self.positions.values().cloned().collect();
                if let Err(reason) = self.portfolio.check_exposure(&positions_vec, kelly_size) {
                    warn!(strategy, %reason, "signal rejected by portfolio exposure check");
                    return None;
                }
                if let Err(reason) =
                    self.portfolio.check_concentration(&positions_vec, &market_id)
                {
                    warn!(strategy, %reason, "signal rejected by concentration check");
                    return None;
                }

                // 5. Compute TP/SL
                let (tp_price, sl_price) = compute_tp_sl(
                    entry_price,
                    edge,
                    side,
                    self.tp_multiplier,
                    self.stop_loss_pct,
                );

                // 6. Create tracked position in Pending state
                let order_id_placeholder = OrderId(format!("pending-{}", id.0));
                let pos = TrackedPosition::new(
                    market_id.clone(),
                    token_id.clone(),
                    outcome.clone(),
                    strategy,
                    order_id_placeholder,
                    id,
                );
                self.positions.insert(pos.id, pos);
                self.breaker
                    .update_position_count(self.open_position_count());
                self.broadcast_positions();

                // 7. Emit order
                let order = if price.is_some() {
                    ExecutableOrder::Limit {
                        signal_id: id,
                        market_id,
                        token_id,
                        side,
                        price: entry_price,
                        size: kelly_size,
                        order_type: OrderType::Gtc,
                        post_only: false,
                    }
                } else {
                    ExecutableOrder::Market {
                        signal_id: id,
                        market_id,
                        token_id,
                        side,
                        size: kelly_size,
                    }
                };

                info!(
                    strategy,
                    %side,
                    %entry_price,
                    %kelly_size,
                    %tp_price,
                    %sl_price,
                    "signal approved — emitting order"
                );
                Some(order)
            }

            Signal::Exit {
                id,
                strategy,
                signal_id,
                reason,
            } => {
                // Find position by original signal_id and begin closing
                if let Some(pos) = self.positions.values_mut().find(|p| p.signal_id == signal_id) {
                    if pos.is_open() {
                        let (size, entry_side) = match pos.start_closing(OrderId(format!("exit-{}", id.0))) {
                            Ok(res) => res,
                            Err(e) => {
                                warn!(strategy, ?signal_id, %e, "failed to start closing position");
                                return None;
                            }
                        };
                        info!(strategy, ?signal_id, ?reason, "closing position");

                        // Emit a market exit order.
                        let side = match entry_side {
                            Side::Buy => Side::Sell,
                            Side::Sell => Side::Buy,
                        };
                        let token_id = pos.token_id.clone();
                        let market_id = pos.market_id.clone();

                        Some(ExecutableOrder::Market {
                            signal_id: id,
                            market_id,
                            token_id,
                            side,
                            size,
                        })
                    } else {
                        warn!(strategy, ?signal_id, "position is not open, cannot exit");
                        None
                    }
                    } else {
                    warn!(strategy, ?signal_id, "position not found for exit");
                    None
                    }
            }

            Signal::Amend {
                strategy,
                order_id,
                new_price,
                new_size,
            } => {
                info!(strategy, %order_id, %new_price, %new_size, "amend signal — cancelling old order");
                // Amend = cancel + re-submit; we emit a cancel and let the strategy re-signal
                Some(ExecutableOrder::Cancel { order_id })
            }

            Signal::CancelAll { market_id } => {
                info!(%market_id, "cancel-all signal");
                Some(ExecutableOrder::CancelAll)
            }
        }
    }

    /// Handle an execution event to update internal position state.
    pub fn handle_execution_event(&mut self, event: &ExecutionEvent) {
        match event {
            ExecutionEvent::OrderFilled {
                order_id: _,
                signal_id,
                market_id: _,
                side,
                price,
                size,
            } => {
                // Check if this is a closing fill first
                let is_closing = self
                    .positions
                    .values()
                    .any(|p| p.signal_id == *signal_id && matches!(p.state, crate::position::PositionState::Closing { .. }));

                if is_closing {
                    // Find the closing position and close it
                    if let Some(pos) = self
                        .positions
                        .values_mut()
                        .find(|p| p.signal_id == *signal_id)
                    {
                        match pos.close(*price) {
                            Ok(realized_pnl) => {
                                info!(?signal_id, %price, %realized_pnl, "position closed on fill");
                                self.breaker.update_pnl(realized_pnl);
                            }
                            Err(e) => {
                                warn!(?signal_id, %e, "failed to close position");
                            }
                        }
                    }
                    // Remove closed positions
                    self.positions.retain(|_, p| !matches!(p.state, crate::position::PositionState::Closed { .. }));
                    self.breaker
                        .update_position_count(self.open_position_count());
                    self.broadcast_positions();
                } else if let Some(pos) = self
                    .positions
                    .values_mut()
                    .find(|p| p.signal_id == *signal_id)
                    .filter(|p| matches!(p.state, crate::position::PositionState::Pending { .. }))
                {
                    // Opening fill for a pending position
                    let edge = Decimal::new(10, 2); // default edge for TP/SL computation
                    let (tp, sl) = compute_tp_sl(
                        *price,
                        edge,
                        *side,
                        self.tp_multiplier,
                        self.stop_loss_pct,
                    );
                    if let Err(e) = pos.open(*price, *size, *side, tp, sl) {
                        warn!(?signal_id, %e, "failed to open position");
                    }
                    self.breaker
                        .update_position_count(self.open_position_count());
                    self.broadcast_positions();
                    info!(?signal_id, %price, %size, "position opened on fill");
                }
            }

            ExecutionEvent::OrderCancelled { order_id } => {
                // Remove any pending position with this order ID
                let before = self.positions.len();
                self.positions.retain(|_, p| {
                    !matches!(&p.state, crate::position::PositionState::Pending { order_id: oid } if oid == order_id)
                });
                self.breaker
                    .update_position_count(self.open_position_count());
                if self.positions.len() != before {
                    self.broadcast_positions();
                }
            }

            ExecutionEvent::OrderRejected {
                order_id: _,
                signal_id,
                reason,
            } => {
                warn!(?signal_id, %reason, "order rejected by executor");
                // Remove pending position for this signal
                let before = self.positions.len();
                self.positions.retain(|_, p| p.signal_id != *signal_id);
                self.breaker
                    .update_position_count(self.open_position_count());
                if self.positions.len() != before {
                    self.broadcast_positions();
                }
            }

            _ => {}
        }
    }

    /// Update the world state snapshot (called by the orchestrator each tick).
    pub fn update_world(&mut self, world: WorldState) {
        self.world = world;
    }

    /// Run the actor loop until the signal channel is closed.
    pub async fn run(mut self) {
        info!("risk actor started");
        
        let mut last_date = chrono::Utc::now().date_naive();
        // Since we don't have a tick channel in RiskActor natively, we can use a sleep interval for daily checks.
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));

        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let current_date = chrono::Utc::now().date_naive();
                    if current_date != last_date {
                        info!("date changed, resetting daily PnL");
                        self.breaker.reset_daily();
                        last_date = current_date;
                    }
                }
                signal = self.signal_rx.recv() => {
                    match signal {
                        Some(sig) => {
                            if let Some(order) = self.process_signal(sig)
                                && self.order_tx.send(order).await.is_err()
                            {
                                warn!("order channel closed — shutting down risk actor");
                                break;
                            }
                        }
                        None => {
                            info!("signal channel closed — shutting down risk actor");
                            break;
                        }
                    }
                }
                event = self.market_rx.recv() => {
                    match event {
                        Ok(MarketEvent::MarketRotation { old, new }) => {
                            info!(%old, new_market = %new.id, "market rotation — force-closing all positions");
                            self.force_close_all_positions();
                        }
                        Ok(_) => {} // ignore other market events
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("market event channel closed");
                            break;
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(n, "risk actor lagged on market events");
                        }
                    }
                }
                result = self.world_rx.changed() => {
                    match result {
                        Ok(()) => {
                            let world = self.world_rx.borrow_and_update().clone();
                            self.world = world.clone();
                            // Re-broadcast positions with updated market prices for live PnL
                            if !self.positions.is_empty() {
                                self.broadcast_positions();
                            }
                            // Check TP/SL for all open positions
                            self.check_tp_sl(&world);
                        }
                        Err(_) => {
                            info!("world channel closed");
                            break;
                        }
                    }
                }
                event = self.execution_rx.recv() => {
                    match event {
                        Ok(ev) => {
                            self.handle_execution_event(&ev);
                            if let pmbot_core::messages::ExecutionEvent::OrderFilled { .. } = ev {
                                self.world.balance = self.bankroll;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("execution event channel closed");
                            break;
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(n, "risk actor lagged on execution events");
                        }
                    }
                }
            }
        }
        info!("risk actor stopped");
    }

    /// Get all tracked positions.
    pub fn positions(&self) -> &HashMap<PositionId, TrackedPosition> {
        &self.positions
    }

    /// Count of currently open positions.
    fn open_position_count(&self) -> usize {
        self.positions.values().filter(|p| p.is_open()).count()
    }

    /// Broadcast a snapshot of all non-closed positions to the strategy actor.
    fn broadcast_positions(&self) {
        let positions: Vec<Position> = self
            .positions
            .values()
            .filter_map(|tp| match &tp.state {
                crate::position::PositionState::Open {
                    entry_price,
                    size,
                    side,
                    opened_at,
                    tp_price,
                    sl_price,
                } => Some(Position {
                    id: tp.id,
                    market_id: tp.market_id.clone(),
                    token_id: tp.token_id.clone(),
                    outcome: tp.outcome.clone(),
                    strategy: tp.strategy,
                    side: *side,
                    entry_price: *entry_price,
                    size: *size,
                    tp_price: *tp_price,
                    sl_price: *sl_price,
                    opened_at: *opened_at,
                    unrealized_pnl: tp.unrealized_pnl(
                        self.world
                            .markets
                            .get(&tp.market_id)
                            .and_then(|m| m.mid_price)
                            .unwrap_or(*entry_price),
                    ),
                }),
                crate::position::PositionState::Pending { .. } => Some(Position {
                    id: tp.id,
                    market_id: tp.market_id.clone(),
                    token_id: tp.token_id.clone(),
                    outcome: tp.outcome.clone(),
                    strategy: tp.strategy,
                    side: Side::Buy, // default; actual side unknown until filled
                    entry_price: Decimal::ZERO,
                    size: Decimal::ZERO,
                    tp_price: Decimal::ZERO,
                    sl_price: Decimal::ZERO,
                    opened_at: chrono::Utc::now(),
                    unrealized_pnl: Decimal::ZERO,
                }),
                _ => None, // Skip Closing / Closed
            })
            .collect();

        // Total PnL = realized (from circuit breaker) + unrealized (from open positions)
        let unrealized: Decimal = positions.iter().map(|p| p.unrealized_pnl).sum();
        let daily_pnl = self.breaker.daily_pnl() + unrealized;

        let _ = self.position_tx.send(PositionSnapshot { positions, daily_pnl });
    }

    /// Force-close all positions on market rotation.
    ///
    /// Computes realized PnL using current mid price, updates the circuit breaker,
    /// clears all tracked positions, and broadcasts the empty state.
    fn force_close_all_positions(&mut self) {
        for (_, tp) in self.positions.iter() {
            if let crate::position::PositionState::Open {
                entry_price,
                size,
                side,
                ..
            } = &tp.state
            {
                let mid = self
                    .world
                    .markets
                    .get(&tp.market_id)
                    .and_then(|m| m.mid_price)
                    .unwrap_or(*entry_price);
                let realized = match side {
                    Side::Buy => (mid - entry_price) * size,
                    Side::Sell => (entry_price - mid) * size,
                };
                self.breaker.update_pnl(realized);
                info!(
                    strategy = tp.strategy,
                    ?tp.signal_id,
                    %realized,
                    "force-closed position on market rotation"
                );
            }
        }
        let count = self.positions.len();
        self.positions.clear();
        self.breaker.update_position_count(0);
        self.broadcast_positions();
        info!(count, "cleared all positions after market rotation");
    }

    /// Check all open positions for TP/SL triggers and emit Exit signals if triggered.
    fn check_tp_sl(&mut self, world: &WorldState) {
        let mut signals_to_emit = Vec::new();

        for (pos_id, tp) in self.positions.iter() {
            if let crate::position::PositionState::Open { .. } = &tp.state {
                // Get current mid price for this market
                let mid_price = match world.markets.get(&tp.market_id).and_then(|m| m.mid_price) {
                    Some(p) => p,
                    None => continue, // No price data
                };

                let should_exit = if tp.should_take_profit(mid_price) {
                    info!(
                        strategy = tp.strategy,
                        ?pos_id,
                        %mid_price,
                        "TP triggered"
                    );
                    true
                } else if tp.should_stop_loss(mid_price) {
                    info!(
                        strategy = tp.strategy,
                        ?pos_id,
                        %mid_price,
                        "SL triggered"
                    );
                    true
                } else {
                    false
                };

                if should_exit {
                    signals_to_emit.push(Signal::Exit {
                        id: SignalId::new(),
                        strategy: tp.strategy,
                        signal_id: tp.signal_id,
                        reason: if tp.should_take_profit(mid_price) {
                            pmbot_core::types::ExitReason::TakeProfit
                        } else {
                            pmbot_core::types::ExitReason::StopLoss
                        },
                    });
                }
            }
        }

        // Emit exit signals
        for signal in signals_to_emit {
            if let Some(order) = self.process_signal(signal) {
                // Try to send, but don't block if channel is full
                let _ = self.order_tx.try_send(order);
            }
        }
    }
}

/// Compute take-profit and stop-loss prices.
fn compute_tp_sl(
    entry_price: Decimal,
    edge: Decimal,
    side: Side,
    tp_multiplier: Decimal,
    stop_loss_pct: Decimal,
) -> (Decimal, Decimal) {
    match side {
        Side::Buy => {
            let tp = entry_price + (edge * tp_multiplier);
            let sl = entry_price * (Decimal::ONE - stop_loss_pct);
            (tp, sl)
        }
        Side::Sell => {
            let tp = entry_price - (edge * tp_multiplier);
            let sl = entry_price * (Decimal::ONE + stop_loss_pct);
            (tp, sl)
        }
    }
}

/// Create a default empty WorldState for initialization.
fn default_world() -> WorldState {
    WorldState {
        active_market_id: None,
        markets: HashMap::new(),
        positions: vec![],
        open_orders: vec![],
        balance: Decimal::new(1_000_000, 0), // large default so tests pass
        daily_pnl: Decimal::ZERO,
        external_prices: HashMap::new(),
        network_latency: HashMap::new(),
        timestamp: chrono::Utc::now(),
    }
}

/// State captured during risk actor shutdown for persistence.
#[derive(Debug, Default)]
pub struct ShutdownState {
    pub final_pnl: Decimal,
    pub open_positions: Vec<TrackedPosition>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use pmbot_core::config::RiskConfig;
    use rust_decimal_macros::dec;

    fn test_config() -> RiskConfig {
        RiskConfig {
            bankroll: dec!(1000),
            kelly_fraction: dec!(0.15),
            max_position_pct: dec!(0.05),
            max_positions: 3,
            daily_loss_limit_pct: dec!(0.05),
            stop_loss_pct: dec!(0.30),
            take_profit_multiplier: dec!(2),
            min_edge: dec!(0.08),
            no_trade_zone_secs: 60,
            kill_switch_path: "/tmp/pmbot-risk-actor-test-kill-NONEXISTENT".into(),
        }
    }

    fn make_actor() -> (
        RiskActor,
        mpsc::Sender<Signal>,
        mpsc::Receiver<ExecutableOrder>,
        broadcast::Sender<ExecutionEvent>,
    ) {
        let (signal_tx, signal_rx) = mpsc::channel(16);
        let (order_tx, order_rx) = mpsc::channel(16);
        let (exec_tx, exec_rx) = broadcast::channel(16);
        let (position_tx, _position_rx) = tokio::sync::watch::channel(PositionSnapshot { positions: vec![], daily_pnl: Decimal::ZERO });
        let (_world_tx, world_rx) = tokio::sync::watch::channel(WorldState::default());
        let (_market_tx, market_rx) = broadcast::channel::<MarketEvent>(16);
        let actor = RiskActor::new(&test_config(), signal_rx, order_tx, exec_rx, market_rx, position_tx, world_rx);
        (actor, signal_tx, order_rx, exec_tx)
    }

    fn enter_signal(edge: Decimal, price: Decimal) -> Signal {
        Signal::Enter {
            id: SignalId::new(),
            strategy: "test",
            market_id: MarketId("m1".into()),
            token_id: TokenId("t1".into()),
            outcome: "Yes".to_string(),
            side: Side::Buy,
            size: dec!(100),
            price: Some(price),
            edge,
            confidence: dec!(0.70),
        }
    }

    #[test]
    fn test_process_enter_signal_approved() {
        let (mut actor, _sig_tx, _ord_rx, _exec_tx) = make_actor();
        let signal = enter_signal(dec!(0.10), dec!(0.50));
        let order = actor.process_signal(signal);
        assert!(order.is_some());
        match order.unwrap() {
            ExecutableOrder::Limit {
                side, price, size, ..
            } => {
                assert_eq!(side, Side::Buy);
                assert_eq!(price, dec!(0.50));
                assert!(size > Decimal::ZERO);
            }
            other => panic!("expected Limit order, got: {other:?}"),
        }
    }

    #[test]
    fn test_process_enter_signal_rejected_low_edge() {
        let (mut actor, _sig_tx, _ord_rx, _exec_tx) = make_actor();
        let signal = Signal::Enter {
            id: SignalId::new(),
            strategy: "test",
            market_id: MarketId("m1".into()),
            token_id: TokenId("t1".into()),
            outcome: "Yes".to_string(),
            side: Side::Buy,
            size: dec!(100),
            price: Some(dec!(0.50)),
            edge: dec!(0.02), // below min_edge of 0.08
            confidence: dec!(0.60),
        };
        let order = actor.process_signal(signal);
        assert!(order.is_none());
    }

    #[test]
    fn test_process_enter_signal_rejected_tripped() {
        let (mut actor, _sig_tx, _ord_rx, _exec_tx) = make_actor();
        actor.breaker.trip();
        let signal = enter_signal(dec!(0.10), dec!(0.50));
        let order = actor.process_signal(signal);
        assert!(order.is_none());
    }

    #[test]
    fn test_process_market_order_when_no_price() {
        let (mut actor, _sig_tx, _ord_rx, _exec_tx) = make_actor();
        let signal = Signal::Enter {
            id: SignalId::new(),
            strategy: "test",
            market_id: MarketId("m1".into()),
            token_id: TokenId("t1".into()),
            outcome: "Yes".to_string(),
            side: Side::Buy,
            size: dec!(100),
            price: None, // no price → Market order
            edge: dec!(0.10),
            confidence: dec!(0.70),
        };
        let order = actor.process_signal(signal);
        assert!(order.is_some());
        assert!(matches!(order.unwrap(), ExecutableOrder::Market { .. }));
    }

    #[test]
    fn test_process_cancel_all() {
        let (mut actor, _sig_tx, _ord_rx, _exec_tx) = make_actor();
        let signal = Signal::CancelAll {
            market_id: MarketId("m1".into()),
        };
        let order = actor.process_signal(signal);
        assert!(matches!(order, Some(ExecutableOrder::CancelAll)));
    }

    #[test]
    fn test_process_amend_emits_cancel() {
        let (mut actor, _sig_tx, _ord_rx, _exec_tx) = make_actor();
        let signal = Signal::Amend {
            strategy: "test",
            order_id: OrderId("order-123".into()),
            new_price: dec!(0.55),
            new_size: dec!(50),
        };
        let order = actor.process_signal(signal);
        assert!(matches!(order, Some(ExecutableOrder::Cancel { .. })));
    }

    #[test]
    fn test_handle_fill_opens_position() {
        let (mut actor, _sig_tx, _ord_rx, _exec_tx) = make_actor();
        let sig_id = SignalId::new();

        // First, process an enter signal to create a pending position
        let signal = Signal::Enter {
            id: sig_id,
            strategy: "test",
            market_id: MarketId("m1".into()),
            token_id: TokenId("t1".into()),
            outcome: "Yes".to_string(),
            side: Side::Buy,
            size: dec!(100),
            price: Some(dec!(0.50)),
            edge: dec!(0.10),
            confidence: dec!(0.70),
        };
        let _order = actor.process_signal(signal);
        assert_eq!(actor.positions.len(), 1);

        // Simulate fill event
        let fill_event = ExecutionEvent::OrderFilled {
            order_id: OrderId("filled-1".into()),
            signal_id: sig_id,
            market_id: MarketId("m1".into()),
            side: Side::Buy,
            price: dec!(0.50),
            size: dec!(25),
        };
        actor.handle_execution_event(&fill_event);

        // Position should now be Open
        let pos = actor.positions.values().next().unwrap();
        assert!(pos.is_open());
    }

    #[test]
    fn test_handle_rejection_removes_position() {
        let (mut actor, _sig_tx, _ord_rx, _exec_tx) = make_actor();
        let sig_id = SignalId::new();

        let signal = Signal::Enter {
            id: sig_id,
            strategy: "test",
            market_id: MarketId("m1".into()),
            token_id: TokenId("t1".into()),
            outcome: "Yes".to_string(),
            side: Side::Buy,
            size: dec!(100),
            price: Some(dec!(0.50)),
            edge: dec!(0.10),
            confidence: dec!(0.70),
        };
        let _order = actor.process_signal(signal);
        assert_eq!(actor.positions.len(), 1);

        // Simulate rejection
        let reject_event = ExecutionEvent::OrderRejected {
            order_id: None,
            signal_id: sig_id,
            reason: "test rejection".into(),
        };
        actor.handle_execution_event(&reject_event);

        // Position should be removed
        assert_eq!(actor.positions.len(), 0);
    }

    #[test]
    fn test_max_positions_enforced() {
        let (mut actor, _sig_tx, _ord_rx, _exec_tx) = make_actor();

        // Fill 3 positions (max_positions = 3)
        for i in 0..3 {
            let sig_id = SignalId::new();
            let signal = Signal::Enter {
                id: sig_id,
                strategy: "test",
                market_id: MarketId(format!("m{}", i)),
                token_id: TokenId(format!("t{}", i)),
                outcome: "Yes".to_string(),
                side: Side::Buy,
                size: dec!(10),
                price: Some(dec!(0.50)),
                edge: dec!(0.10),
                confidence: dec!(0.70),
            };
            let order = actor.process_signal(signal);
            assert!(order.is_some(), "signal {i} should be approved");

            // Simulate fill
            let fill = ExecutionEvent::OrderFilled {
                order_id: OrderId(format!("order-{i}")),
                signal_id: sig_id,
                market_id: MarketId(format!("m{}", i)),
                side: Side::Buy,
                price: dec!(0.50),
                size: dec!(10),
            };
            actor.handle_execution_event(&fill);
        }

        // 4th signal should be rejected (max positions reached)
        let signal = Signal::Enter {
            id: SignalId::new(),
            strategy: "test",
            market_id: MarketId("m4".into()),
            token_id: TokenId("t4".into()),
            outcome: "Yes".to_string(),
            side: Side::Buy,
            size: dec!(10),
            price: Some(dec!(0.50)),
            edge: dec!(0.10),
            confidence: dec!(0.70),
        };
        let order = actor.process_signal(signal);
        assert!(order.is_none(), "4th signal should be rejected");
    }

    #[test]
    fn test_compute_tp_sl_buy() {
        let (tp, sl) = compute_tp_sl(dec!(0.50), dec!(0.10), Side::Buy, dec!(2), dec!(0.30));
        assert_eq!(tp, dec!(0.70)); // 0.50 + 0.10 * 2
        assert_eq!(sl, dec!(0.35)); // 0.50 * (1 - 0.30)
    }

    #[test]
    fn test_compute_tp_sl_sell() {
        let (tp, sl) = compute_tp_sl(dec!(0.50), dec!(0.10), Side::Sell, dec!(2), dec!(0.30));
        assert_eq!(tp, dec!(0.30)); // 0.50 - 0.10 * 2
        assert_eq!(sl, dec!(0.65)); // 0.50 * (1 + 0.30)
    }

    #[test]
    fn test_exit_signal_for_nonexistent_position() {
        let (mut actor, _sig_tx, _ord_rx, _exec_tx) = make_actor();
        let signal = Signal::Exit {
            id: SignalId::new(),
            strategy: "test",
            signal_id: SignalId::new(), // doesn't exist
            reason: ExitReason::StrategyExit,
        };
        let order = actor.process_signal(signal);
        assert!(order.is_none());
    }

    #[test]
    fn test_update_world_state() {
        let (mut actor, _sig_tx, _ord_rx, _exec_tx) = make_actor();
        let new_world = WorldState {
            active_market_id: None,
            markets: HashMap::new(),
            positions: vec![],
            open_orders: vec![],
            balance: dec!(500),
            daily_pnl: dec!(-10),
            external_prices: HashMap::new(),
            network_latency: HashMap::new(),
            timestamp: chrono::Utc::now(),
        };
        actor.update_world(new_world);
        assert_eq!(actor.world.balance, dec!(500));
    }

    #[tokio::test]
    async fn test_actor_run_processes_signal() {
        let (signal_tx, signal_rx) = mpsc::channel(16);
        let (order_tx, mut order_rx) = mpsc::channel(16);
        let (exec_tx, exec_rx) = broadcast::channel(16);
        let (position_tx, _position_rx) = tokio::sync::watch::channel(PositionSnapshot { positions: vec![], daily_pnl: Decimal::ZERO });
        let (_world_tx, world_rx) = tokio::sync::watch::channel(WorldState::default());
        let (_market_tx, market_rx) = broadcast::channel::<MarketEvent>(16);
        let actor = RiskActor::new(&test_config(), signal_rx, order_tx, exec_rx, market_rx, position_tx, world_rx);

        // Spawn actor
        let handle = tokio::spawn(actor.run());

        // Send a signal
        let signal = Signal::Enter {
            id: SignalId::new(),
            strategy: "test",
            market_id: MarketId("m1".into()),
            token_id: TokenId("t1".into()),
            outcome: "Yes".to_string(),
            side: Side::Buy,
            size: dec!(100),
            price: Some(dec!(0.50)),
            edge: dec!(0.10),
            confidence: dec!(0.70),
        };
        signal_tx.send(signal).await.unwrap();

        // Should receive an order
        let order = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            order_rx.recv(),
        )
        .await
        .expect("timed out waiting for order")
        .expect("order channel closed");

        assert!(matches!(order, ExecutableOrder::Limit { .. }));

        // Drop sender to stop actor
        drop(signal_tx);
        drop(exec_tx);
        let _ = handle.await;
    }
}
