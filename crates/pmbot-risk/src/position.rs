use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;

use pmbot_core::types::{MarketId, OrderId, PositionId, Side, SignalId, TokenId};

/// State machine for a tracked position's lifecycle.
#[derive(Debug, Clone)]
pub enum PositionState {
    /// Order submitted but not yet filled.
    Pending { order_id: OrderId },

    /// Position is live.
    Open {
        entry_price: Decimal,
        size: Decimal,
        side: Side,
        opened_at: DateTime<Utc>,
        tp_price: Decimal,
        sl_price: Decimal,
    },

    /// Exit order submitted, awaiting fill.
    Closing {
        exit_order_id: OrderId,
        entry_price: Decimal,
        size: Decimal,
        side: Side,
        opened_at: DateTime<Utc>,
    },

    /// Position fully closed.
    Closed {
        entry_price: Decimal,
        exit_price: Decimal,
        pnl: Decimal,
        duration: Duration,
    },
}

/// A position tracked by the risk actor through its full lifecycle.
#[derive(Debug, Clone)]
pub struct TrackedPosition {
    pub id: PositionId,
    pub market_id: MarketId,
    pub token_id: TokenId,
    pub outcome: String,
    pub strategy: &'static str,
    pub state: PositionState,
    pub signal_id: SignalId,
}

#[derive(Debug)]
pub enum PositionError {
    InvalidState(&'static str),
}

impl std::fmt::Display for PositionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidState(msg) => write!(f, "invalid state transition: {}", msg),
        }
    }
}

impl std::error::Error for PositionError {}

impl TrackedPosition {
    /// Create a new tracked position in the Pending state.
    pub fn new(
        market_id: MarketId,
        token_id: TokenId,
        outcome: String,
        strategy: &'static str,
        order_id: OrderId,
        signal_id: SignalId,
    ) -> Self {
        Self {
            id: PositionId::new(),
            market_id,
            token_id,
            outcome,
            strategy,
            state: PositionState::Pending { order_id },
            signal_id,
        }
    }

    /// Transition from Pending to Open on fill.
    ///
    /// # Errors
    /// Returns `PositionError` if the position is not in the Pending state.
    pub fn open(
        &mut self,
        fill_price: Decimal,
        size: Decimal,
        side: Side,
        tp_price: Decimal,
        sl_price: Decimal,
    ) -> Result<(), PositionError> {
        if !matches!(self.state, PositionState::Pending { .. }) {
            return Err(PositionError::InvalidState(
                "can only open a Pending position",
            ));
        }
        self.state = PositionState::Open {
            entry_price: fill_price,
            size,
            side,
            opened_at: Utc::now(),
            tp_price,
            sl_price,
        };
        Ok(())
    }

    /// Transition from Open to Closing when an exit order is submitted.
    ///
    /// Returns the entry price and side for order construction.
    ///
    /// # Errors
    /// Returns `PositionError` if the position is not in the Open state.
    pub fn start_closing(
        &mut self,
        exit_order_id: OrderId,
    ) -> Result<(Decimal, Side), PositionError> {
        let (entry_price, size, side, opened_at) = match self.state {
            PositionState::Open {
                entry_price,
                size,
                side,
                opened_at,
                ..
            } => (entry_price, size, side, opened_at),
            _ => {
                return Err(PositionError::InvalidState(
                    "can only start closing an Open position",
                ));
            }
        };
        self.state = PositionState::Closing {
            exit_order_id,
            entry_price,
            size,
            side,
            opened_at,
        };
        Ok((size, side))
    }

    /// Transition from Closing to Closed when the exit fill arrives.
    ///
    /// Extracts entry data from the Closing state and computes realized PnL.
    /// Returns the realized PnL on success.
    ///
    /// # Errors
    /// Returns `PositionError` if the position is not in the Closing state.
    pub fn close(&mut self, exit_price: Decimal) -> Result<Decimal, PositionError> {
        let (entry_price, size, side, opened_at) = match &self.state {
            PositionState::Closing {
                entry_price,
                size,
                side,
                opened_at,
                ..
            } => (*entry_price, *size, *side, *opened_at),
            _ => {
                return Err(PositionError::InvalidState(
                    "can only close a Closing position",
                ));
            }
        };
        let pnl = compute_pnl(entry_price, exit_price, size, side);
        let duration = Utc::now() - opened_at;
        self.state = PositionState::Closed {
            entry_price,
            exit_price,
            pnl,
            duration,
        };
        Ok(pnl)
    }

    /// Calculate unrealized PnL for an Open position.
    ///
    /// Returns `Decimal::ZERO` if position is not Open.
    pub fn unrealized_pnl(&self, current_price: Decimal) -> Decimal {
        match &self.state {
            PositionState::Open {
                entry_price,
                size,
                side,
                ..
            } => compute_pnl(*entry_price, current_price, *size, *side),
            _ => Decimal::ZERO,
        }
    }

    /// Check if the current price has reached the take-profit level.
    ///
    /// Returns `false` if the position is not Open.
    pub fn should_take_profit(&self, current_price: Decimal) -> bool {
        match &self.state {
            PositionState::Open { side, tp_price, .. } => match side {
                Side::Buy => current_price >= *tp_price,
                Side::Sell => current_price <= *tp_price,
            },
            _ => false,
        }
    }

    /// Check if the current price has reached the stop-loss level.
    ///
    /// Returns `false` if the position is not Open.
    pub fn should_stop_loss(&self, current_price: Decimal) -> bool {
        match &self.state {
            PositionState::Open { side, sl_price, .. } => match side {
                Side::Buy => current_price <= *sl_price,
                Side::Sell => current_price >= *sl_price,
            },
            _ => false,
        }
    }

    /// Returns true if the position is in the Open state.
    pub fn is_open(&self) -> bool {
        matches!(self.state, PositionState::Open { .. })
    }

    /// Returns true if the position is in the Closed state.
    pub fn is_closed(&self) -> bool {
        matches!(self.state, PositionState::Closed { .. })
    }
}

/// Compute PnL for a binary prediction market position.
///
/// - Buy side: `pnl = (current_price - entry_price) * size`
/// - Sell side: `pnl = (entry_price - current_price) * size`
fn compute_pnl(entry_price: Decimal, current_price: Decimal, size: Decimal, side: Side) -> Decimal {
    match side {
        Side::Buy => (current_price - entry_price) * size,
        Side::Sell => (entry_price - current_price) * size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn make_pending() -> TrackedPosition {
        TrackedPosition::new(
            MarketId("market-1".into()),
            TokenId("token-1".into()),
            "Yes".to_string(),
            "test_strategy",
            OrderId("order-1".into()),
            SignalId::new(),
        )
    }

    #[test]
    fn test_pending_to_open() {
        let mut pos = make_pending();
        assert!(matches!(pos.state, PositionState::Pending { .. }));

        let _ = pos.open(dec!(0.50), dec!(100), Side::Buy, dec!(0.70), dec!(0.35));
        match &pos.state {
            PositionState::Open {
                entry_price,
                size,
                side,
                tp_price,
                sl_price,
                ..
            } => {
                assert_eq!(*entry_price, dec!(0.50));
                assert_eq!(*size, dec!(100));
                assert_eq!(*side, Side::Buy);
                assert_eq!(*tp_price, dec!(0.70));
                assert_eq!(*sl_price, dec!(0.35));
            }
            _ => panic!("expected Open state"),
        }
    }

    #[test]
    fn test_open_to_closing() {
        let mut pos = make_pending();
        let _ = pos.open(dec!(0.50), dec!(100), Side::Buy, dec!(0.70), dec!(0.35));
        let _ = pos.start_closing(OrderId("exit-1".into()));
        match &pos.state {
            PositionState::Closing { exit_order_id, .. } => {
                assert_eq!(exit_order_id.0, "exit-1");
            }
            _ => panic!("expected Closing state"),
        }
    }

    #[test]
    fn test_closing_to_closed_buy_profit() {
        let mut pos = make_pending();
        let _ = pos.open(dec!(0.50), dec!(100), Side::Buy, dec!(0.70), dec!(0.35));
        pos.start_closing(OrderId("exit-1".into()));
        let pnl = pos.close(dec!(0.70)).unwrap();

        match &pos.state {
            PositionState::Closed {
                entry_price,
                exit_price,
                pnl,
                ..
            } => {
                assert_eq!(*entry_price, dec!(0.50));
                assert_eq!(*exit_price, dec!(0.70));
                assert_eq!(*pnl, dec!(20)); // (0.70 - 0.50) * 100
            }
            _ => panic!("expected Closed state"),
        }
    }

    #[test]
    fn test_closing_to_closed_buy_loss() {
        let mut pos = make_pending();
        pos.open(dec!(0.50), dec!(100), Side::Buy, dec!(0.70), dec!(0.35));

        pos.start_closing(OrderId("exit-1".into()));
        let pnl = pos.close(dec!(0.35)).unwrap();

        match &pos.state {
            PositionState::Closed { pnl, .. } => {
                assert_eq!(*pnl, dec!(-15)); // (0.35 - 0.50) * 100
            }
            _ => panic!("expected Closed state"),
        }
    }

    #[test]
    fn test_closing_to_closed_sell_profit() {
        let mut pos = make_pending();
        pos.open(dec!(0.50), dec!(100), Side::Sell, dec!(0.30), dec!(0.65));

        pos.start_closing(OrderId("exit-1".into()));
        let pnl = pos.close(dec!(0.30)).unwrap();

        match &pos.state {
            PositionState::Closed { pnl, .. } => {
                assert_eq!(*pnl, dec!(20)); // (0.50 - 0.30) * 100
            }
            _ => panic!("expected Closed state"),
        }
    }

    #[test]
    fn test_closing_to_closed_sell_loss() {
        let mut pos = make_pending();
        pos.open(dec!(0.50), dec!(100), Side::Sell, dec!(0.30), dec!(0.65));

        pos.start_closing(OrderId("exit-1".into()));
        let pnl = pos.close(dec!(0.65)).unwrap();

        match &pos.state {
            PositionState::Closed { pnl, .. } => {
                assert_eq!(*pnl, dec!(-15)); // (0.50 - 0.65) * 100
            }
            _ => panic!("expected Closed state"),
        }
    }

    #[test]
    fn test_unrealized_pnl_buy() {
        let mut pos = make_pending();
        pos.open(dec!(0.50), dec!(100), Side::Buy, dec!(0.70), dec!(0.35));

        assert_eq!(pos.unrealized_pnl(dec!(0.60)), dec!(10)); // profit
        assert_eq!(pos.unrealized_pnl(dec!(0.40)), dec!(-10)); // loss
        assert_eq!(pos.unrealized_pnl(dec!(0.50)), dec!(0)); // flat
    }

    #[test]
    fn test_unrealized_pnl_sell() {
        let mut pos = make_pending();
        pos.open(dec!(0.50), dec!(100), Side::Sell, dec!(0.30), dec!(0.65));

        assert_eq!(pos.unrealized_pnl(dec!(0.40)), dec!(10)); // profit
        assert_eq!(pos.unrealized_pnl(dec!(0.60)), dec!(-10)); // loss
        assert_eq!(pos.unrealized_pnl(dec!(0.50)), dec!(0)); // flat
    }

    #[test]
    fn test_unrealized_pnl_non_open_returns_zero() {
        let pos = make_pending();
        assert_eq!(pos.unrealized_pnl(dec!(0.60)), Decimal::ZERO);
    }

    #[test]
    fn test_should_take_profit_buy() {
        let mut pos = make_pending();
        pos.open(dec!(0.50), dec!(100), Side::Buy, dec!(0.70), dec!(0.35));

        assert!(!pos.should_take_profit(dec!(0.60)));
        assert!(pos.should_take_profit(dec!(0.70)));
        assert!(pos.should_take_profit(dec!(0.80)));
    }

    #[test]
    fn test_should_take_profit_sell() {
        let mut pos = make_pending();
        pos.open(dec!(0.50), dec!(100), Side::Sell, dec!(0.30), dec!(0.65));

        assert!(!pos.should_take_profit(dec!(0.40)));
        assert!(pos.should_take_profit(dec!(0.30)));
        assert!(pos.should_take_profit(dec!(0.20)));
    }

    #[test]
    fn test_should_stop_loss_buy() {
        let mut pos = make_pending();
        pos.open(dec!(0.50), dec!(100), Side::Buy, dec!(0.70), dec!(0.35));

        assert!(!pos.should_stop_loss(dec!(0.45)));
        assert!(pos.should_stop_loss(dec!(0.35)));
        assert!(pos.should_stop_loss(dec!(0.20)));
    }

    #[test]
    fn test_should_stop_loss_sell() {
        let mut pos = make_pending();
        pos.open(dec!(0.50), dec!(100), Side::Sell, dec!(0.30), dec!(0.65));

        assert!(!pos.should_stop_loss(dec!(0.55)));
        assert!(pos.should_stop_loss(dec!(0.65)));
        assert!(pos.should_stop_loss(dec!(0.80)));
    }

    #[test]
    fn test_tp_sl_non_open_returns_false() {
        let pos = make_pending();
        assert!(!pos.should_take_profit(dec!(0.99)));
        assert!(!pos.should_stop_loss(dec!(0.01)));
    }

    #[test]
    fn test_open_non_pending_returns_error() {
        let mut pos = make_pending();
        let _ = pos.open(dec!(0.50), dec!(100), Side::Buy, dec!(0.70), dec!(0.35));
        // Opening an already-open position should return an error
        let result = pos.open(dec!(0.60), dec!(50), Side::Buy, dec!(0.80), dec!(0.40));
        assert!(result.is_err());
    }

    #[test]
    fn test_start_closing_non_open_returns_error() {
        let mut pos = make_pending();
        let result = pos.start_closing(OrderId("exit-1".into()));
        assert!(result.is_err());
    }
}
