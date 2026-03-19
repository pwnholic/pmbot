//! Startup reconciliation — compares local tracked orders against the
//! exchange's view and emits corrective `ExecutionEvent`s.

use std::collections::HashMap;

use anyhow::Result;
use rust_decimal::Decimal;
use tracing::{info, warn};

use pmbot_core::messages::ExecutionEvent;
use pmbot_core::types::OrderId;

use crate::actor::OrderExecutor;
use crate::lifecycle::TrackedOrder;

/// Reconciles local tracked orders with the exchange on startup.
pub struct Reconciler;

impl Reconciler {
    /// Fetch open orders from the exchange and reconcile with our local state.
    ///
    /// Returns a list of `ExecutionEvent`s for any discrepancies found:
    /// - Orders we track locally but are no longer on the exchange -> `OrderCancelled`
    /// - Orders on the exchange that we don't know about locally (logged, no event)
    /// - Partial fills detected from size differences -> `OrderPartialFill`
    pub async fn reconcile(
        executor: &dyn OrderExecutor,
        tracked: &mut HashMap<OrderId, TrackedOrder>,
    ) -> Result<Vec<ExecutionEvent>> {
        let exchange_orders = executor.get_open_orders().await?;
        let mut events = Vec::new();

        // Build a set of exchange order IDs for quick lookup.
        let exchange_ids: HashMap<&OrderId, &pmbot_core::types::OpenOrder> = exchange_orders
            .iter()
            .map(|o| (&o.order_id, o))
            .collect();

        // 1. Check locally tracked orders against exchange state.
        let local_ids: Vec<OrderId> = tracked.keys().cloned().collect();
        for oid in &local_ids {
            let Some(local) = tracked.get_mut(oid) else {
                continue;
            };

            if local.state.is_terminal() {
                continue;
            }

            if let Some(exchange_order) = exchange_ids.get(oid) {
                // Order exists on exchange — check for partial fills.
                if exchange_order.filled > Decimal::ZERO {
                    let remaining = exchange_order.size - exchange_order.filled;
                    let _ = local.partial_fill(exchange_order.filled, remaining);
                    events.push(ExecutionEvent::OrderPartialFill {
                        order_id: oid.clone(),
                        signal_id: local.signal_id,
                        market_id: local.market_id.clone(),
                        side: local.side,
                        price: local.price,
                        filled: exchange_order.filled,
                        remaining,
                    });
                    info!(
                        order_id = %oid,
                        filled = %exchange_order.filled,
                        "reconciled partial fill"
                    );
                }
            } else {
                // Order missing from exchange — mark cancelled.
                let _ = local.cancel();
                events.push(ExecutionEvent::OrderCancelled {
                    order_id: oid.clone(),
                });
                warn!(order_id = %oid, "reconciled: order missing from exchange, marked cancelled");
            }
        }

        // 2. Log exchange orders we don't track locally (informational only).
        for eo in &exchange_orders {
            if !tracked.contains_key(&eo.order_id) {
                warn!(
                    order_id = %eo.order_id,
                    "unknown order found on exchange during reconciliation"
                );
            }
        }

        info!(
            events = events.len(),
            tracked = tracked.len(),
            exchange = exchange_orders.len(),
            "reconciliation complete"
        );

        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::TrackedOrder;
    use crate::paper::PaperExecutor;
    use pmbot_core::types::{MarketId, Side, SignalId, TokenId};
    use rust_decimal_macros::dec;

    #[tokio::test]
    async fn reconcile_empty_state() {
        let exec = PaperExecutor::new(dec!(1000), dec!(0.50), MarketId("m".into()));
        let mut tracked = HashMap::new();
        let events = Reconciler::reconcile(&exec, &mut tracked).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn reconcile_marks_missing_as_cancelled() {
        let exec = PaperExecutor::new(dec!(1000), dec!(0.50), MarketId("m".into()));
        let mut tracked = HashMap::new();

        // Create a tracked order that is "live" but not on exchange.
        let oid = OrderId("missing-1".into());
        let mut order = TrackedOrder::new(
            SignalId::new(),
            MarketId("test-m".into()),
            TokenId("t".into()),
            Side::Buy,
            dec!(0.5),
            dec!(10),
        );
        order.submit().unwrap();
        order.make_live(oid.clone()).unwrap();
        tracked.insert(oid.clone(), order);

        let events = Reconciler::reconcile(&exec, &mut tracked).await.unwrap();

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ExecutionEvent::OrderCancelled { order_id } if *order_id == oid
        ));
        assert!(tracked[&oid].state.is_terminal());
    }
}
