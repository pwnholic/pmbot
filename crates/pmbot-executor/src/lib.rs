//! pmbot-executor: Order execution actor with lifecycle tracking.
//!
//! This crate provides:
//! - [`OrderExecutor`] trait abstracting the Polymarket CLOB API
//! - [`ExecutorActor`] that drives orders through their lifecycle
//! - [`PaperExecutor`] for simulated/backtest execution
//! - [`Reconciler`] for startup state reconciliation
//! - [`TrackedOrder`] / [`OrderState`] state machine
//! - [`RateLimiter`] / [`ApiRateLimiter`] for rate limiting

pub mod actor;
pub mod lifecycle;
pub mod live;
pub mod paper;
pub mod rate_limit;
pub mod reconciler;

// Re-exports for convenience.
pub use actor::{ExecutorActor, OrderExecutor};
pub use lifecycle::{OrderState, TrackedOrder, TransitionError};
pub use live::LiveExecutor;
pub use paper::PaperExecutor;
pub use rate_limit::{ApiRateLimiter, RateLimiter};
pub use reconciler::Reconciler;

#[cfg(test)]
mod tests {
    //! Integration-style tests for the actor message flow.

    use std::time::Duration;

    use rust_decimal_macros::dec;
    use tokio::sync::{broadcast, mpsc};

    use pmbot_core::messages::{ExecutableOrder, ExecutionEvent};
    use pmbot_core::types::{MarketId, OrderId, OrderType, Side, SignalId, TokenId};

    use crate::actor::ExecutorActor;
    use crate::paper::PaperExecutor;

    #[tokio::test]
    async fn actor_processes_limit_order() {
        let (orders_tx, orders_rx) = mpsc::channel(16);
        let (events_tx, mut events_rx) = broadcast::channel(16);

        let paper = PaperExecutor::new(dec!(1000), dec!(0.50), MarketId("m".into()));
        let actor = ExecutorActor::new(paper, orders_rx, events_tx);

        // Spawn the actor.
        let handle = tokio::spawn(actor.run());

        let signal_id = SignalId::new();
        orders_tx
            .send(ExecutableOrder::Limit {
                signal_id,
                market_id: MarketId("test-m".into()),
                token_id: TokenId("tok".into()),
                side: Side::Buy,
                price: dec!(0.45),
                size: dec!(10),
                order_type: OrderType::Gtc,
                post_only: false,
            })
            .await
            .unwrap();

        // Receive the OrderPlaced event.
        let event = tokio::time::timeout(Duration::from_secs(2), events_rx.recv())
            .await
            .expect("timeout waiting for event")
            .expect("channel error");

        match event {
            ExecutionEvent::OrderPlaced {
                order_id,
                signal_id: sid,
            } => {
                assert_eq!(sid, signal_id);
                assert!(order_id.0.starts_with("paper-"));
            }
            other => panic!("expected OrderPlaced, got {:?}", other),
        }

        // Drop sender to shut down actor.
        drop(orders_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn actor_processes_market_order() {
        let (orders_tx, orders_rx) = mpsc::channel(16);
        let (events_tx, mut events_rx) = broadcast::channel(16);

        let paper = PaperExecutor::new(dec!(1000), dec!(0.50), MarketId("m".into()));
        let actor = ExecutorActor::new(paper, orders_rx, events_tx);
        let handle = tokio::spawn(actor.run());

        let signal_id = SignalId::new();
        orders_tx
            .send(ExecutableOrder::Market {
                signal_id,
                market_id: MarketId("test-m".into()),
                token_id: TokenId("tok".into()),
                side: Side::Buy,
                size: dec!(10),
            })
            .await
            .unwrap();

        let event = tokio::time::timeout(Duration::from_secs(2), events_rx.recv())
            .await
            .expect("timeout")
            .expect("channel error");

        assert!(matches!(event, ExecutionEvent::OrderPlaced { .. }));

        drop(orders_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn actor_processes_cancel() {
        let (orders_tx, orders_rx) = mpsc::channel(16);
        let (events_tx, mut events_rx) = broadcast::channel(16);

        let paper = PaperExecutor::new(dec!(1000), dec!(0.50), MarketId("m".into()));
        let actor = ExecutorActor::new(paper, orders_rx, events_tx);
        let handle = tokio::spawn(actor.run());

        orders_tx
            .send(ExecutableOrder::Cancel {
                order_id: OrderId("paper-1".into()),
            })
            .await
            .unwrap();

        let event = tokio::time::timeout(Duration::from_secs(2), events_rx.recv())
            .await
            .expect("timeout")
            .expect("channel error");

        assert!(matches!(event, ExecutionEvent::OrderCancelled { .. }));

        drop(orders_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn actor_processes_cancel_all() {
        let (orders_tx, orders_rx) = mpsc::channel(16);
        let (events_tx, _events_rx) = broadcast::channel(16);

        let paper = PaperExecutor::new(dec!(1000), dec!(0.50), MarketId("m".into()));
        let actor = ExecutorActor::new(paper, orders_rx, events_tx);
        let handle = tokio::spawn(actor.run());

        orders_tx
            .send(ExecutableOrder::CancelAll)
            .await
            .unwrap();

        // Give the actor time to process, then shut down.
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(orders_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn actor_rejects_insufficient_balance() {
        let (orders_tx, orders_rx) = mpsc::channel(16);
        let (events_tx, mut events_rx) = broadcast::channel(16);

        // Only $1 balance.
        let paper = PaperExecutor::new(dec!(1), dec!(0.50), MarketId("m".into()));
        let actor = ExecutorActor::new(paper, orders_rx, events_tx);
        let handle = tokio::spawn(actor.run());

        let signal_id = SignalId::new();
        orders_tx
            .send(ExecutableOrder::Market {
                signal_id,
                market_id: MarketId("test-m".into()),
                token_id: TokenId("tok".into()),
                side: Side::Buy,
                size: dec!(100), // costs $50, but only $1 available
            })
            .await
            .unwrap();

        let event = tokio::time::timeout(Duration::from_secs(2), events_rx.recv())
            .await
            .expect("timeout")
            .expect("channel error");

        match event {
            ExecutionEvent::OrderRejected { reason, .. } => {
                assert!(reason.contains("insufficient"));
            }
            other => panic!("expected OrderRejected, got {:?}", other),
        }

        drop(orders_tx);
        handle.await.unwrap();
    }
}
