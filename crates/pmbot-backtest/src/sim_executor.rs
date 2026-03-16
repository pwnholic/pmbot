//! Simulated order execution for backtesting.
//!
//! [`SimExecutor`] fills signals against orderbook snapshots
//! with configurable fees and slippage.

use rust_decimal::Decimal;

use pmbot_core::math::taker_fee;
use pmbot_core::messages::Signal;
use pmbot_core::types::{Level, OrderId, Side, SignalId};

/// Result of a simulated fill.
#[derive(Debug, Clone)]
pub struct SimFill {
    /// Assigned order ID.
    pub order_id: OrderId,
    /// Signal that triggered this fill.
    pub signal_id: SignalId,
    /// Trade side.
    pub side: Side,
    /// Fill price (after slippage).
    pub price: Decimal,
    /// Fill size.
    pub size: Decimal,
    /// Fee charged.
    pub fee: Decimal,
}

/// Simulated executor for backtesting.
///
/// Tracks balance and fees, fills signals against orderbook levels
/// with configurable slippage.
pub struct SimExecutor {
    fee_rate_bps: u32,
    slippage_bps: u32,
    balance: Decimal,
    total_fees: Decimal,
    next_order_id: u64,
}

impl SimExecutor {
    /// Create a new executor with the given initial balance and fee/slippage settings.
    pub fn new(initial_balance: Decimal, fee_rate_bps: u32, slippage_bps: u32) -> Self {
        Self {
            fee_rate_bps,
            slippage_bps,
            balance: initial_balance,
            total_fees: Decimal::ZERO,
            next_order_id: 1,
        }
    }

    /// Try to fill a signal against the current book.
    ///
    /// Only `Signal::Enter` variants are filled. Returns `None` for other
    /// signal types, empty book sides, or insufficient balance.
    pub fn try_fill(
        &mut self,
        signal: &Signal,
        bids: &[Level],
        asks: &[Level],
    ) -> Option<SimFill> {
        let (id, side, size, _market_id, _token_id) = match signal {
            Signal::Enter {
                id,
                side,
                size,
                market_id,
                token_id,
                ..
            } => (*id, *side, *size, market_id, token_id),
            _ => return None,
        };

        let fill_price = match side {
            Side::Buy => {
                let best_ask = asks.first()?;
                self.apply_slippage(best_ask.price, Side::Buy)
            }
            Side::Sell => {
                let best_bid = bids.first()?;
                self.apply_slippage(best_bid.price, Side::Sell)
            }
        };

        let fee = taker_fee(fill_price, size, self.fee_rate_bps);
        let cost = fill_price * size + fee;

        // For buys, check balance covers the cost
        if side == Side::Buy && self.balance < cost {
            return None;
        }

        // Update balance
        match side {
            Side::Buy => self.balance -= cost,
            Side::Sell => self.balance += fill_price * size - fee,
        }

        self.total_fees += fee;

        let order_id = OrderId(format!("sim-{}", self.next_order_id));
        self.next_order_id += 1;

        Some(SimFill {
            order_id,
            signal_id: id,
            side,
            price: fill_price,
            size,
            fee,
        })
    }

    /// Apply slippage to a price.
    fn apply_slippage(&self, price: Decimal, side: Side) -> Decimal {
        let slippage =
            price * Decimal::new(self.slippage_bps.into(), 0) / Decimal::new(10_000, 0);
        match side {
            Side::Buy => price + slippage,  // worse for buyer
            Side::Sell => price - slippage, // worse for seller
        }
    }

    /// Current available balance.
    pub fn balance(&self) -> Decimal {
        self.balance
    }

    /// Total fees accumulated.
    pub fn total_fees(&self) -> Decimal {
        self.total_fees
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pmbot_core::messages::Signal;
    use pmbot_core::types::*;
    use rust_decimal_macros::dec;

    fn make_bids() -> Vec<Level> {
        vec![
            Level {
                price: dec!(0.50),
                size: dec!(100),
            },
            Level {
                price: dec!(0.49),
                size: dec!(200),
            },
        ]
    }

    fn make_asks() -> Vec<Level> {
        vec![
            Level {
                price: dec!(0.55),
                size: dec!(100),
            },
            Level {
                price: dec!(0.56),
                size: dec!(200),
            },
        ]
    }

    fn buy_signal(size: Decimal) -> Signal {
        Signal::Enter {
            id: SignalId::new(),
            strategy: "test",
            market_id: MarketId("m1".into()),
            token_id: TokenId("t1".into()),
            side: Side::Buy,
            size,
            price: Some(dec!(0.55)),
            edge: dec!(0.05),
            confidence: dec!(0.8),
        }
    }

    fn sell_signal(size: Decimal) -> Signal {
        Signal::Enter {
            id: SignalId::new(),
            strategy: "test",
            market_id: MarketId("m1".into()),
            token_id: TokenId("t1".into()),
            side: Side::Sell,
            size,
            price: Some(dec!(0.50)),
            edge: dec!(0.05),
            confidence: dec!(0.8),
        }
    }

    #[test]
    fn test_buy_fill_at_best_ask_plus_slippage() {
        // 0 slippage, 0 fees for simplicity
        let mut exec = SimExecutor::new(dec!(1000), 0, 0);
        let signal = buy_signal(dec!(10));
        let fill = exec.try_fill(&signal, &make_bids(), &make_asks()).unwrap();

        assert_eq!(fill.side, Side::Buy);
        assert_eq!(fill.price, dec!(0.55)); // best ask, no slippage
        assert_eq!(fill.size, dec!(10));
        assert_eq!(fill.fee, dec!(0));
        // balance = 1000 - 0.55*10 = 994.5
        assert_eq!(exec.balance(), dec!(994.5));
    }

    #[test]
    fn test_sell_fill_at_best_bid_minus_slippage() {
        let mut exec = SimExecutor::new(dec!(1000), 0, 0);
        let signal = sell_signal(dec!(10));
        let fill = exec.try_fill(&signal, &make_bids(), &make_asks()).unwrap();

        assert_eq!(fill.side, Side::Sell);
        assert_eq!(fill.price, dec!(0.50)); // best bid, no slippage
        assert_eq!(fill.size, dec!(10));
        // balance = 1000 + 0.50*10 = 1005
        assert_eq!(exec.balance(), dec!(1005));
    }

    #[test]
    fn test_slippage_applied_correctly() {
        // 100 bps = 1% slippage
        let mut exec = SimExecutor::new(dec!(1000), 0, 100);
        let signal = buy_signal(dec!(10));
        let fill = exec.try_fill(&signal, &make_bids(), &make_asks()).unwrap();

        // best_ask = 0.55, slippage = 0.55 * 100/10000 = 0.0055
        // fill_price = 0.55 + 0.0055 = 0.5555
        assert_eq!(fill.price, dec!(0.5555));
    }

    #[test]
    fn test_fees_deducted() {
        // 200 bps fee, 0 slippage
        let mut exec = SimExecutor::new(dec!(1000), 200, 0);
        let signal = buy_signal(dec!(100));
        let fill = exec.try_fill(&signal, &make_bids(), &make_asks()).unwrap();

        // fee = taker_fee(0.55, 100, 200) = 100 * min(0.55, 0.45) * 200/10000
        //      = 100 * 0.45 * 0.02 = 0.90
        assert_eq!(fill.fee, dec!(0.90));
        assert_eq!(exec.total_fees(), dec!(0.90));
    }

    #[test]
    fn test_insufficient_balance_returns_none() {
        let mut exec = SimExecutor::new(dec!(1), 0, 0);
        let signal = buy_signal(dec!(100));
        let result = exec.try_fill(&signal, &make_bids(), &make_asks());
        assert!(result.is_none());
    }

    #[test]
    fn test_non_enter_signal_returns_none() {
        let mut exec = SimExecutor::new(dec!(1000), 0, 0);
        let signal = Signal::CancelAll {
            market_id: MarketId("m1".into()),
        };
        let result = exec.try_fill(&signal, &make_bids(), &make_asks());
        assert!(result.is_none());
    }
}
