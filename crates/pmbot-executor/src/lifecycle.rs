//! Order state machine — tracks an order from creation through final state.

use std::time::Instant;

use rust_decimal::Decimal;

use pmbot_core::types::{MarketId, OrderId, Side, SignalId, TokenId};

/// States an order can be in.
#[derive(Debug, Clone)]
pub enum OrderState {
    Created,
    Submitted { submitted_at: Instant },
    Live { order_id: OrderId },
    PartialFill { filled: Decimal, remaining: Decimal },
    Filled,
    Cancelled,
    Rejected { reason: String },
    TimedOut,
}

impl OrderState {
    /// Returns `true` if the order is in a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            OrderState::Filled
                | OrderState::Cancelled
                | OrderState::Rejected { .. }
                | OrderState::TimedOut
        )
    }

    /// Returns `true` if the order is actively live on the exchange.
    pub fn is_live(&self) -> bool {
        matches!(
            self,
            OrderState::Live { .. } | OrderState::PartialFill { .. }
        )
    }
}

/// An order tracked through its lifecycle.
#[derive(Debug, Clone)]
pub struct TrackedOrder {
    pub signal_id: SignalId,
    pub market_id: MarketId,
    pub state: OrderState,
    pub token_id: TokenId,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    pub created_at: Instant,
}

/// Errors from invalid state transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionError {
    pub from: &'static str,
    pub to: &'static str,
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid transition: {} -> {}", self.from, self.to)
    }
}

impl std::error::Error for TransitionError {}

impl TrackedOrder {
    /// Create a new tracked order in the `Created` state.
    pub fn new(
        signal_id: SignalId,
        market_id: MarketId,
        token_id: TokenId,
        side: Side,
        price: Decimal,
        size: Decimal,
    ) -> Self {
        Self {
            signal_id,
            market_id,
            state: OrderState::Created,
            token_id,
            side,
            price,
            size,
            created_at: Instant::now(),
        }
    }

    /// Transition to `Submitted`.
    pub fn submit(&mut self) -> Result<(), TransitionError> {
        match &self.state {
            OrderState::Created => {
                self.state = OrderState::Submitted {
                    submitted_at: Instant::now(),
                };
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Submitted",
            }),
        }
    }

    /// Transition to `Live` with the exchange-assigned order ID.
    pub fn make_live(&mut self, order_id: OrderId) -> Result<(), TransitionError> {
        match &self.state {
            OrderState::Submitted { .. } => {
                self.state = OrderState::Live { order_id };
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Live",
            }),
        }
    }

    /// Transition to `PartialFill`.
    pub fn partial_fill(
        &mut self,
        filled: Decimal,
        remaining: Decimal,
    ) -> Result<(), TransitionError> {
        match &self.state {
            OrderState::Live { .. } | OrderState::PartialFill { .. } => {
                self.state = OrderState::PartialFill { filled, remaining };
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "PartialFill",
            }),
        }
    }

    /// Transition to `Filled`.
    pub fn fill(&mut self) -> Result<(), TransitionError> {
        match &self.state {
            OrderState::Live { .. } | OrderState::PartialFill { .. } => {
                self.state = OrderState::Filled;
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Filled",
            }),
        }
    }

    /// Transition to `Cancelled`.
    pub fn cancel(&mut self) -> Result<(), TransitionError> {
        match &self.state {
            OrderState::Created
            | OrderState::Submitted { .. }
            | OrderState::Live { .. }
            | OrderState::PartialFill { .. } => {
                self.state = OrderState::Cancelled;
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Cancelled",
            }),
        }
    }

    /// Transition to `Rejected`.
    pub fn reject(&mut self, reason: String) -> Result<(), TransitionError> {
        match &self.state {
            OrderState::Created | OrderState::Submitted { .. } => {
                self.state = OrderState::Rejected { reason };
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "Rejected",
            }),
        }
    }

    /// Transition to `TimedOut`.
    pub fn timeout(&mut self) -> Result<(), TransitionError> {
        match &self.state {
            OrderState::Submitted { .. } => {
                self.state = OrderState::TimedOut;
                Ok(())
            }
            _ => Err(TransitionError {
                from: self.state_name(),
                to: "TimedOut",
            }),
        }
    }

    fn state_name(&self) -> &'static str {
        match &self.state {
            OrderState::Created => "Created",
            OrderState::Submitted { .. } => "Submitted",
            OrderState::Live { .. } => "Live",
            OrderState::PartialFill { .. } => "PartialFill",
            OrderState::Filled => "Filled",
            OrderState::Cancelled => "Cancelled",
            OrderState::Rejected { .. } => "Rejected",
            OrderState::TimedOut => "TimedOut",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn make_order() -> TrackedOrder {
        TrackedOrder::new(
            SignalId::new(),
            MarketId("test-m".into()),
            TokenId("tok1".into()),
            Side::Buy,
            dec!(0.55),
            dec!(10),
        )
    }

    #[test]
    fn happy_path_created_to_filled() {
        let mut order = make_order();
        assert!(!order.state.is_terminal());

        order.submit().unwrap();
        order.make_live(OrderId("ord-1".into())).unwrap();
        order.partial_fill(dec!(5), dec!(5)).unwrap();
        order.fill().unwrap();

        assert!(order.state.is_terminal());
    }

    #[test]
    fn direct_fill_from_live() {
        let mut order = make_order();
        order.submit().unwrap();
        order.make_live(OrderId("ord-2".into())).unwrap();
        order.fill().unwrap();
        assert!(matches!(order.state, OrderState::Filled));
    }

    #[test]
    fn cancel_from_live() {
        let mut order = make_order();
        order.submit().unwrap();
        order.make_live(OrderId("ord-3".into())).unwrap();
        order.cancel().unwrap();
        assert!(matches!(order.state, OrderState::Cancelled));
    }

    #[test]
    fn cancel_from_created() {
        let mut order = make_order();
        order.cancel().unwrap();
        assert!(matches!(order.state, OrderState::Cancelled));
    }

    #[test]
    fn reject_from_submitted() {
        let mut order = make_order();
        order.submit().unwrap();
        order.reject("insufficient funds".into()).unwrap();
        assert!(order.state.is_terminal());
    }

    #[test]
    fn timeout_from_submitted() {
        let mut order = make_order();
        order.submit().unwrap();
        order.timeout().unwrap();
        assert!(matches!(order.state, OrderState::TimedOut));
    }

    #[test]
    fn cannot_submit_twice() {
        let mut order = make_order();
        order.submit().unwrap();
        let err = order.submit().unwrap_err();
        assert_eq!(err.from, "Submitted");
        assert_eq!(err.to, "Submitted");
    }

    #[test]
    fn cannot_go_from_filled_to_live() {
        let mut order = make_order();
        order.submit().unwrap();
        order.make_live(OrderId("ord-4".into())).unwrap();
        order.fill().unwrap();
        let err = order.make_live(OrderId("ord-5".into())).unwrap_err();
        assert_eq!(err.from, "Filled");
        assert_eq!(err.to, "Live");
    }

    #[test]
    fn cannot_fill_from_created() {
        let mut order = make_order();
        let err = order.fill().unwrap_err();
        assert_eq!(err.from, "Created");
        assert_eq!(err.to, "Filled");
    }

    #[test]
    fn cannot_cancel_terminal() {
        let mut order = make_order();
        order.submit().unwrap();
        order.reject("bad".into()).unwrap();
        let err = order.cancel().unwrap_err();
        assert_eq!(err.from, "Rejected");
    }

    #[test]
    fn cannot_timeout_from_live() {
        let mut order = make_order();
        order.submit().unwrap();
        order.make_live(OrderId("ord-6".into())).unwrap();
        let err = order.timeout().unwrap_err();
        assert_eq!(err.from, "Live");
    }
}
