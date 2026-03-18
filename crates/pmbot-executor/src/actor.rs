//! Executor actor — receives `ExecutableOrder`s and drives them through the
//! order lifecycle, emitting `ExecutionEvent`s to the rest of the system.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

use pmbot_core::messages::{ExecutableOrder, ExecutionEvent};
use pmbot_core::types::{OpenOrder, OrderId, OrderType, Side, TokenId};

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
        }
    }

    /// Provide access to tracked orders (e.g. for reconciliation).
    pub fn tracked_orders(&self) -> &HashMap<OrderId, TrackedOrder> {
        &self.tracked_orders
    }

    /// Mutable access to tracked orders (e.g. for reconciliation).
    pub fn tracked_orders_mut(&mut self) -> &mut HashMap<OrderId, TrackedOrder> {
        &mut self.tracked_orders
    }

    /// Run the actor loop until the orders channel is closed.
    pub async fn run(mut self) {
        info!("executor actor started");

        // Run reconciliation
        match crate::reconciler::Reconciler::reconcile(&self.executor, &mut self.tracked_orders)
            .await
        {
            Ok(events) => {
                for event in events {
                    let _ = self.events_tx.send(event);
                }
            }
            Err(e) => {
                error!("fatal error: reconciliation failed: {}", e);
                return; // Fatal: do not start the executor loop if reconciliation fails
            }
        }

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
                            market_id,
                            side,
                            price,
                            size,
                        });

                        self.tracked_orders.insert(order_id, tracked);
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
                            market_id,
                            side,
                            price: Decimal::ZERO, // Market orders don't have a pre-known fill price here
                            size,
                        });
                        self.tracked_orders.insert(order_id, tracked);
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
                    self.emit(ExecutionEvent::OrderCancelled { order_id });
                }
                Err(e) => {
                    warn!("cancel failed for {}: {}", order_id, e);
                }
            },

            ExecutableOrder::CancelAll => match self.executor.cancel_all().await {
                Ok(()) => {
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
}
