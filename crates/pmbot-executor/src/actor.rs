//! Executor actor — receives `ExecutableOrder`s and drives them through the
//! order lifecycle, emitting `ExecutionEvent`s to the rest of the system.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

use pmbot_core::messages::{ExecutableOrder, ExecutionEvent};
use pmbot_core::types::{OpenOrder, OrderId, OrderType, Side, TokenId};
use pmbot_db::Database;

use crate::lifecycle::TrackedOrder;

// ---------------------------------------------------------------------------
// OrderExecutor trait — abstracts the actual SDK / API calls
// ---------------------------------------------------------------------------

/// Abstraction over the Polymarket CLOB API so we can swap in a paper executor.
#[async_trait]
pub trait OrderExecutor: Send + Sync {
    async fn submit_limit(
        &self,
        token_id: &TokenId,
        side: Side,
        price: Decimal,
        size: Decimal,
        order_type: OrderType,
        post_only: bool,
    ) -> Result<OrderId>;

    async fn submit_market(&self, token_id: &TokenId, side: Side, size: Decimal)
    -> Result<OrderId>;

    async fn cancel(&self, order_id: &OrderId) -> Result<()>;

    async fn cancel_all(&self) -> Result<()>;

    async fn get_open_orders(&self) -> Result<Vec<OpenOrder>>;
}

// ---------------------------------------------------------------------------
// ExecutorActor
// ---------------------------------------------------------------------------

/// The executor actor owns the channel endpoints and drives order execution.
pub struct ExecutorActor<E: OrderExecutor> {
    executor: E,
    orders_rx: mpsc::Receiver<ExecutableOrder>,
    events_tx: broadcast::Sender<ExecutionEvent>,
    tracked_orders: HashMap<OrderId, TrackedOrder>,
    /// How long to wait for an order ack before emitting `TimedOut`.
    ack_timeout: Duration,
    /// Optional database for order persistence.
    db: Option<Arc<Database>>,
}

impl<E: OrderExecutor + 'static> ExecutorActor<E> {
    /// Create a new `ExecutorActor`.
    pub fn new(
        executor: E,
        orders_rx: mpsc::Receiver<ExecutableOrder>,
        events_tx: broadcast::Sender<ExecutionEvent>,
    ) -> Self {
        Self {
            executor,
            orders_rx,
            events_tx,
            tracked_orders: HashMap::new(),
            ack_timeout: Duration::from_secs(5),
            db: None,
        }
    }

    /// Set the database for order persistence.
    pub fn with_db(mut self, db: Arc<Database>) -> Self {
        self.db = Some(db);
        self
    }

    /// Provide access to tracked orders (e.g. for reconciliation).
    pub fn tracked_orders(&self) -> &HashMap<OrderId, TrackedOrder> {
        &self.tracked_orders
    }

    /// Mutable access to tracked orders (e.g. for reconciliation).
    pub fn tracked_orders_mut(&mut self) -> &mut HashMap<OrderId, TrackedOrder> {
        &mut self.tracked_orders
    }

    /// Run startup reconciliation against exchange state.
    ///
    /// This should be called BEFORE starting the actor loop to ensure
    /// reconciliation events are broadcast before other actors start processing.
    pub async fn reconcile(&mut self) -> Result<Vec<ExecutionEvent>> {
        crate::reconciler::Reconciler::reconcile(&self.executor, &mut self.tracked_orders).await
    }

    /// Run the actor loop until the orders channel is closed.
    pub async fn run(mut self) {
        info!("executor actor started");

        while let Some(order) = self.orders_rx.recv().await {
            self.handle_order(order).await;
        }
        info!("executor actor stopped (orders channel closed)");
    }

    async fn handle_order(&mut self, order: ExecutableOrder) {
        match order {
            ExecutableOrder::Limit {
                signal_id,
                market_id,
                token_id,
                side,
                price,
                size,
                order_type,
                post_only,
            } => {
                let mut tracked = TrackedOrder::new(
                    signal_id,
                    market_id.clone(),
                    token_id.clone(),
                    side,
                    price,
                    size,
                );

                if tracked.submit().is_err() {
                    error!("failed to transition order to Submitted");
                    return;
                }

                let result = tokio::time::timeout(
                    self.ack_timeout,
                    self.executor
                        .submit_limit(&token_id, side, price, size, order_type, post_only),
                )
                .await;

                match result {
                    Ok(Ok(order_id)) => {
                        if tracked.make_live(order_id.clone()).is_err() {
                            error!("failed to transition order to Live");
                            return;
                        }
                        info!(%order_id, ?signal_id, "limit order placed");
                        self.emit(ExecutionEvent::OrderPlaced {
                            order_id: order_id.clone(),
                            signal_id,
                        });

                        // For paper mode simulation, fill immediately.
                        // In live mode, this would be handled by a websocket/polling task.
                        let _ = tracked.fill();
                        self.emit(ExecutionEvent::OrderFilled {
                            order_id: order_id.clone(),
                            signal_id,
                            market_id: market_id.clone(),
                            side,
                            price,
                            size,
                        });

                        self.tracked_orders.insert(order_id.clone(), tracked);
                        if let Some(tracked) = self.tracked_orders.get(&order_id) {
                            self.persist_order(tracked, &order_id).await;
                        }
                    }
                    Ok(Err(e)) => {
                        let reason = e.to_string();
                        let _ = tracked.reject(reason.clone());
                        self.emit(ExecutionEvent::OrderRejected {
                            order_id: None,
                            signal_id,
                            reason,
                        });
                    }
                    Err(_elapsed) => {
                        let _ = tracked.timeout();
                        self.emit(ExecutionEvent::OrderRejected {
                            order_id: None,
                            signal_id,
                            reason: "submission timed out".into(),
                        });
                    }
                }
            }

            ExecutableOrder::Market {
                signal_id,
                market_id,
                token_id,
                side,
                size,
            } => {
                let mut tracked = TrackedOrder::new(
                    signal_id,
                    market_id.clone(),
                    token_id.clone(),
                    side,
                    Decimal::ZERO,
                    size,
                );

                if tracked.submit().is_err() {
                    error!("failed to transition market order to Submitted");
                    return;
                }

                let result = tokio::time::timeout(
                    self.ack_timeout,
                    self.executor.submit_market(&token_id, side, size),
                )
                .await;

                match result {
                    Ok(Ok(order_id)) => {
                        if tracked.make_live(order_id.clone()).is_err() {
                            error!("failed to transition market order to Live");
                            return;
                        }
                        // Market orders fill immediately in most cases.
                        let _ = tracked.fill();
                        info!(%order_id, ?signal_id, %size, ?side, "market order placed and filled");
                        self.emit(ExecutionEvent::OrderPlaced {
                            order_id: order_id.clone(),
                            signal_id,
                        });
                        self.emit(ExecutionEvent::OrderFilled {
                            order_id: order_id.clone(),
                            signal_id,
                            market_id: market_id.clone(),
                            side,
                            price: Decimal::ZERO, // Market orders don't have a pre-known fill price here
                            size,
                        });
                        self.tracked_orders.insert(order_id.clone(), tracked);
                        if let Some(tracked) = self.tracked_orders.get(&order_id) {
                            self.persist_order(tracked, &order_id).await;
                        }
                    }
                    Ok(Err(e)) => {
                        let reason = e.to_string();
                        let _ = tracked.reject(reason.clone());
                        self.emit(ExecutionEvent::OrderRejected {
                            order_id: None,
                            signal_id,
                            reason,
                        });
                    }
                    Err(_elapsed) => {
                        let _ = tracked.timeout();
                        self.emit(ExecutionEvent::OrderRejected {
                            order_id: None,
                            signal_id,
                            reason: "submission timed out".into(),
                        });
                    }
                }
            }

            ExecutableOrder::Cancel { order_id } => match self.executor.cancel(&order_id).await {
                Ok(()) => {
                    if let Some(tracked) = self.tracked_orders.get_mut(&order_id) {
                        let _ = tracked.cancel();
                    }
                    self.emit(ExecutionEvent::OrderCancelled { order_id: order_id.clone() });
                    self.remove_order(&order_id).await;
                }
                Err(e) => {
                    warn!("cancel failed for {}: {}", order_id, e);
                }
            },

            ExecutableOrder::CancelAll => match self.executor.cancel_all().await {
                Ok(()) => {
                    let order_ids: Vec<OrderId> = self.tracked_orders.keys().cloned().collect();
                    for (oid, tracked) in &mut self.tracked_orders {
                        if !tracked.state.is_terminal() {
                            let _ = tracked.cancel();
                            self.events_tx
                                .send(ExecutionEvent::OrderCancelled {
                                    order_id: oid.clone(),
                                })
                                .ok();
                        }
                    }
                    for oid in order_ids {
                        self.remove_order(&oid).await;
                    }
                }
                Err(e) => {
                    warn!("cancel_all failed: {}", e);
                }
            },
        }
    }

    fn emit(&self, event: ExecutionEvent) {
        // Ignore send errors — no receivers is fine during shutdown.
        let _ = self.events_tx.send(event);
    }

    /// Persist an order to the database (if db is configured).
    /// Only persists orders that have an order_id (Live state).
    async fn persist_order(&self, tracked: &TrackedOrder, order_id: &OrderId) {
        if let Some(ref db) = self.db {
            use pmbot_db::orders::OrderStatus;
            use chrono::Utc;
            
            // Map lifecycle state to db status
            let status = match &tracked.state {
                crate::lifecycle::OrderState::Created => return, // Don't persist until submitted
                crate::lifecycle::OrderState::Submitted { .. } => return, // Don't persist until live
                crate::lifecycle::OrderState::Live { .. } => OrderStatus::Open,
                crate::lifecycle::OrderState::PartialFill { filled, .. } => {
                    if *filled > Decimal::ZERO {
                        OrderStatus::PartiallyFilled
                    } else {
                        OrderStatus::Open
                    }
                }
                crate::lifecycle::OrderState::Filled => OrderStatus::Filled,
                crate::lifecycle::OrderState::Cancelled => OrderStatus::Cancelled,
                crate::lifecycle::OrderState::Rejected { .. } => return, // Don't persist rejected orders
                crate::lifecycle::OrderState::TimedOut => return, // Don't persist timed out orders
            };
            
            // Convert Instant to DateTime<Utc>
            let created_at = Utc::now() - tracked.created_at.elapsed();
            
            let db_order = pmbot_db::orders::TrackedOrder {
                order_id: order_id.clone(),
                signal_id: tracked.signal_id,
                market_id: tracked.market_id.clone(),
                token_id: tracked.token_id.clone(),
                outcome: String::new(), // Placeholder for multi-option support
                side: tracked.side,
                price: tracked.price,
                size: tracked.size,
                filled: match &tracked.state {
                    crate::lifecycle::OrderState::PartialFill { filled, .. } => *filled,
                    _ => Decimal::ZERO,
                },
                status,
                created_at,
                updated_at: Utc::now(),
            };
            
            if let Err(e) = db.orders().insert(&db_order).await {
                warn!(error = %e, order_id = %order_id, "failed to persist order to database");
            }
        }
    }

    /// Remove an order from the database (if db is configured).
    async fn remove_order(&self, order_id: &OrderId) {
        if let Some(ref db) = self.db {
            if let Err(e) = db.orders().remove(order_id).await {
                warn!(%order_id, error = %e, "failed to remove order from database");
            }
        }
    }

    /// Persist a completed trade to the database (if db is configured).
    #[allow(dead_code)]
    async fn persist_trade(
        &self,
        order_id: &OrderId,
        signal_id: pmbot_core::types::SignalId,
        market_id: pmbot_core::types::MarketId,
        side: Side,
        entry_price: Decimal,
        size: Decimal,
        filled_size: Decimal,
    ) {
        if let Some(ref db) = self.db {
            let trade = pmbot_db::trades::Trade {
                id: 0, // Auto-generated
                order_id: order_id.clone(),
                signal_id,
                market_id,
                token_id: pmbot_core::types::TokenId(String::new()), // Would need to track
                outcome: String::new(), // Would need to track
                strategy: String::new(), // Would need to track
                side,
                entry_price,
                exit_price: None,
                size,
                filled_size,
                pnl: None,
                opened_at: chrono::Utc::now(),
                closed_at: None,
            };
            
            if let Err(e) = db.trades().insert(&trade).await {
                warn!(%order_id, error = %e, "failed to persist trade to database");
            }
        }
    }
}

/// State captured during executor shutdown for persistence or recovery.
#[derive(Debug, Default)]
pub struct ShutdownState {
    pub orders_cancelled: usize,
    pub tracked_orders: Vec<(OrderId, TrackedOrder)>,
}

impl<E: OrderExecutor + 'static> ExecutorActor<E> {
    /// Gracefully shut down the executor.
    ///
    /// In live mode, cancels all open orders.
    /// Returns the final state for persistence.
    pub async fn shutdown(mut self) -> Result<ShutdownState> {
        info!(tracked = self.tracked_orders.len(), "executor shutting down");
        
        let mut state = ShutdownState::default();
        
        // Cancel all live orders
        for (order_id, tracked) in &self.tracked_orders {
            if tracked.state.is_live() {
                match self.executor.cancel(order_id).await {
                    Ok(()) => {
                        state.orders_cancelled += 1;
                        info!(%order_id, "cancelled order on shutdown");
                    }
                    Err(e) => {
                        warn!(%order_id, error = %e, "failed to cancel order on shutdown");
                    }
                }
            }
        }
        
        // Capture final state
        state.tracked_orders = self.tracked_orders.drain().collect();
        
        info!(
            cancelled = state.orders_cancelled,
            remaining = state.tracked_orders.len(),
            "executor shutdown complete"
        );
        
        Ok(state)
    }
}
