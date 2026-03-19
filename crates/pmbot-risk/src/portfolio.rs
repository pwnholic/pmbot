use rust_decimal::Decimal;

use pmbot_core::error::RejectReason;
use pmbot_core::types::MarketId;

use crate::position::{PositionState, TrackedPosition};

/// Portfolio-level risk checks.
#[derive(Debug, Clone)]
pub struct PortfolioRisk {
    /// Maximum net exposure across all positions (in dollar terms).
    pub max_net_exposure: Decimal,
    /// Maximum number of positions in a single event/market.
    pub max_per_event: usize,
}

impl PortfolioRisk {
    /// Check that adding a new position of `new_size` does not exceed maximum net exposure.
    pub fn check_exposure(
        &self,
        positions: &[TrackedPosition],
        new_size: Decimal,
    ) -> Result<(), RejectReason> {
        let current_exposure: Decimal = positions
            .iter()
            .filter_map(|p| match &p.state {
                PositionState::Open { size, .. } => Some(*size),
                _ => None,
            })
            .sum();

        if current_exposure + new_size > self.max_net_exposure {
            return Err(RejectReason::InsufficientLiquidity {
                available: (self.max_net_exposure - current_exposure).to_string(),
                required: new_size.to_string(),
            });
        }

        Ok(())
    }

    /// Check that the number of positions in a given market does not exceed `max_per_event`.
    pub fn check_concentration(
        &self,
        positions: &[TrackedPosition],
        market_id: &MarketId,
    ) -> Result<(), RejectReason> {
        let count = positions
            .iter()
            .filter(|p| p.market_id == *market_id && p.is_open())
            .count();

        if count >= self.max_per_event {
            return Err(RejectReason::ExcessiveCorrelation);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pmbot_core::types::{OrderId, PositionId, Side, SignalId, TokenId};
    use rust_decimal_macros::dec;

    fn make_open_position(market_id: &str, size: Decimal) -> TrackedPosition {
        let mut pos = TrackedPosition {
            id: PositionId::new(),
            market_id: MarketId(market_id.into()),
            token_id: TokenId("t1".into()),
            outcome: "Yes".to_string(),
            strategy: "test",
            state: PositionState::Pending {
                order_id: OrderId("o1".into()),
            },
            signal_id: SignalId::new(),
        };
        let _ = pos.open(dec!(0.50), size, Side::Buy, dec!(0.70), dec!(0.35));
        pos
    }

    #[test]
    fn test_check_exposure_passes() {
        let risk = PortfolioRisk {
            max_net_exposure: dec!(500),
            max_per_event: 3,
        };
        let positions = vec![make_open_position("m1", dec!(100))];
        assert!(risk.check_exposure(&positions, dec!(100)).is_ok());
    }

    #[test]
    fn test_check_exposure_fails() {
        let risk = PortfolioRisk {
            max_net_exposure: dec!(500),
            max_per_event: 3,
        };
        let positions = vec![
            make_open_position("m1", dec!(300)),
            make_open_position("m2", dec!(150)),
        ];
        // current=450, new=100 => 550 > 500
        let result = risk.check_exposure(&positions, dec!(100));
        assert!(result.is_err());
        match result.unwrap_err() {
            RejectReason::InsufficientLiquidity { .. } => {}
            other => panic!("expected InsufficientLiquidity, got: {other:?}"),
        }
    }

    #[test]
    fn test_check_exposure_ignores_non_open() {
        let risk = PortfolioRisk {
            max_net_exposure: dec!(500),
            max_per_event: 3,
        };
        // Pending position should not count toward exposure
        let pending = TrackedPosition {
            id: PositionId::new(),
            market_id: MarketId("m1".into()),
            token_id: TokenId("t1".into()),
            outcome: "Yes".to_string(),
            strategy: "test",
            state: PositionState::Pending {
                order_id: OrderId("o1".into()),
            },
            signal_id: SignalId::new(),
        };
        assert!(risk.check_exposure(&[pending], dec!(400)).is_ok());
    }

    #[test]
    fn test_check_concentration_passes() {
        let risk = PortfolioRisk {
            max_net_exposure: dec!(500),
            max_per_event: 2,
        };
        let positions = vec![make_open_position("m1", dec!(100))];
        assert!(risk
            .check_concentration(&positions, &MarketId("m1".into()))
            .is_ok());
    }

    #[test]
    fn test_check_concentration_fails() {
        let risk = PortfolioRisk {
            max_net_exposure: dec!(500),
            max_per_event: 2,
        };
        let positions = vec![
            make_open_position("m1", dec!(100)),
            make_open_position("m1", dec!(100)),
        ];
        let result = risk.check_concentration(&positions, &MarketId("m1".into()));
        assert_eq!(result.unwrap_err(), RejectReason::ExcessiveCorrelation);
    }

    #[test]
    fn test_check_concentration_different_markets_ok() {
        let risk = PortfolioRisk {
            max_net_exposure: dec!(500),
            max_per_event: 1,
        };
        let positions = vec![make_open_position("m1", dec!(100))];
        // Different market should be fine
        assert!(risk
            .check_concentration(&positions, &MarketId("m2".into()))
            .is_ok());
    }
}
