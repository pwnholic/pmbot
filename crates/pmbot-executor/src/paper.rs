//! Paper executor — simulates order fills for backtesting and dry-run mode.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::Mutex;

use pmbot_core::types::{MarketId, OpenOrder, OrderId, OrderType, Side, TokenId};

use crate::actor::OrderExecutor;

/// Simulated balance and fill tracking.
#[derive(Debug)]
struct PaperState {
    balance: Decimal,
    open_orders: Vec<OpenOrder>,
    next_seq: u64,
}

/// Paper executor that simulates fills against a mock book.
///
/// - Market orders fill immediately at the configured mid price.
/// - Limit orders fill immediately (simplistic model).
/// - Generates deterministic fake `OrderId`s.
#[derive(Debug, Clone)]
pub struct PaperExecutor {
    state: Arc<Mutex<PaperState>>,
    mid_price: Decimal,
    /// Will be used when we wire up the SDK for order metadata.
    #[allow(dead_code)]
    market_id: MarketId,
}

impl PaperExecutor {
    /// Create a new paper executor with the given starting balance and mid price.
    pub fn new(balance: Decimal, mid_price: Decimal, market_id: MarketId) -> Self {
        Self {
            state: Arc::new(Mutex::new(PaperState {
                balance,
                open_orders: Vec::new(),
                next_seq: 1,
            })),
            mid_price,
            market_id,
        }
    }

    /// Return the current simulated balance.
    pub async fn balance(&self) -> Decimal {
        self.state.lock().await.balance
    }

    fn make_order_id(seq: u64) -> OrderId {
        OrderId(format!("paper-{seq}"))
    }
}

#[async_trait]
impl OrderExecutor for PaperExecutor {
    async fn submit_limit(
        &self,
        _token_id: &TokenId,
        side: Side,
        price: Decimal,
        size: Decimal,
        _order_type: OrderType,
        _post_only: bool,
    ) -> Result<OrderId> {
        let mut state = self.state.lock().await;
        let cost = price * size;

        if side == Side::Buy && state.balance < cost {
            anyhow::bail!("insufficient paper balance: have {}, need {}", state.balance, cost);
        }

        let order_id = Self::make_order_id(state.next_seq);
        state.next_seq += 1;

        // Immediate fill simulation — deduct/add balance.
        match side {
            Side::Buy => state.balance -= cost,
            Side::Sell => state.balance += cost,
        }

        // We don't track it as "open" since it fills immediately in this model.
        Ok(order_id)
    }

    async fn submit_market(
        &self,
        _token_id: &TokenId,
        side: Side,
        size: Decimal,
    ) -> Result<OrderId> {
        let mut state = self.state.lock().await;
        // In simulation, we deduct based on a tracked mid price.
        // If mid_price is not updated dynamically, this assumes $0.50
        let cost = self.mid_price * size;

        if side == Side::Buy && state.balance < cost {
            anyhow::bail!(
                "insufficient paper balance: have {}, need {}",
                state.balance,
                cost
            );
        }

        let order_id = Self::make_order_id(state.next_seq);
        state.next_seq += 1;

        match side {
            Side::Buy => state.balance -= cost,
            Side::Sell => state.balance += cost,
        }

        Ok(order_id)
    }

    async fn cancel(&self, order_id: &OrderId) -> Result<()> {
        let mut state = self.state.lock().await;
        state.open_orders.retain(|o| o.order_id != *order_id);
        Ok(())
    }

    async fn cancel_all(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        state.open_orders.clear();
        Ok(())
    }

    async fn get_open_orders(&self) -> Result<Vec<OpenOrder>> {
        let state = self.state.lock().await;
        Ok(state.open_orders.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn paper() -> PaperExecutor {
        PaperExecutor::new(
            dec!(1000),
            dec!(0.50),
            MarketId("test-market".into()),
        )
    }

    #[tokio::test]
    async fn market_buy_deducts_balance() {
        let exec = paper();
        let token = TokenId("tok".into());
        let oid = exec.submit_market(&token, Side::Buy, dec!(10)).await.unwrap();

        assert_eq!(exec.balance().await, dec!(995)); // 0.50 * 10 = 5
        assert!(oid.0.starts_with("paper-"));
    }

    #[tokio::test]
    async fn market_sell_adds_balance() {
        let exec = paper();
        let token = TokenId("tok".into());
        exec.submit_market(&token, Side::Sell, dec!(20)).await.unwrap();

        assert_eq!(exec.balance().await, dec!(1010)); // 0.50 * 20 = 10
    }

    #[tokio::test]
    async fn limit_buy_deducts_at_limit_price() {
        let exec = paper();
        let token = TokenId("tok".into());
        exec.submit_limit(&token, Side::Buy, dec!(0.45), dec!(100), OrderType::Gtc, false)
            .await
            .unwrap();

        assert_eq!(exec.balance().await, dec!(955)); // 0.45 * 100 = 45
    }

    #[tokio::test]
    async fn insufficient_balance_rejected() {
        let exec = paper();
        let token = TokenId("tok".into());
        let result = exec
            .submit_market(&token, Side::Buy, dec!(3000))
            .await;

        assert!(result.is_err());
        assert_eq!(exec.balance().await, dec!(1000)); // unchanged
    }

    #[tokio::test]
    async fn sequential_order_ids() {
        let exec = paper();
        let token = TokenId("tok".into());
        let id1 = exec.submit_market(&token, Side::Buy, dec!(1)).await.unwrap();
        let id2 = exec.submit_market(&token, Side::Buy, dec!(1)).await.unwrap();

        assert_eq!(id1.0, "paper-1");
        assert_eq!(id2.0, "paper-2");
    }

    #[tokio::test]
    async fn cancel_and_cancel_all() {
        let exec = paper();
        exec.cancel(&OrderId("paper-1".into())).await.unwrap();
        exec.cancel_all().await.unwrap();
        let open = exec.get_open_orders().await.unwrap();
        assert!(open.is_empty());
    }
}
