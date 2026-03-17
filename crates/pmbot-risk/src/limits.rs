use std::path::Path;

use rust_decimal::Decimal;

use pmbot_core::config::RiskConfig;
use pmbot_core::error::RejectReason;
use pmbot_core::messages::{Signal, WorldState};

/// Circuit breaker that enforces risk limits before allowing trades.
#[derive(Debug)]
pub struct CircuitBreaker {
    config: RiskConfig,
    daily_pnl: Decimal,
    position_count: usize,
    tripped: bool,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the given risk configuration.
    pub fn new(config: RiskConfig) -> Self {
        Self {
            config,
            daily_pnl: Decimal::ZERO,
            position_count: 0,
            tripped: false,
        }
    }

    /// Run all pre-trade checks against the given signal and world state.
    ///
    /// Checks are executed in order:
    /// 1. Kill switch (file existence check)
    /// 2. Daily loss limit
    /// 3. Max positions
    /// 4. Minimum edge
    /// 5. Insufficient balance
    pub fn check(&self, signal: &Signal, world: &WorldState) -> Result<(), RejectReason> {
        // Only enter signals need full checks
        let (edge, size, price, market_id) = match signal {
            Signal::Enter {
                edge,
                size,
                price,
                market_id,
                ..
            } => (*edge, *size, *price, market_id),
            _ => return Ok(()),
        };

        // 1. Kill switch
        if self.tripped || Path::new(&self.config.kill_switch_path).exists() {
            return Err(RejectReason::KillSwitchActive);
        }

        // 2. Daily loss limit
        let loss_limit = self.config.bankroll * self.config.daily_loss_limit_pct;
        if self.daily_pnl < -loss_limit {
            return Err(RejectReason::DailyLossExceeded);
        }

        // 3. Max positions
        if self.position_count >= self.config.max_positions {
            return Err(RejectReason::MaxPositionsReached);
        }

        // 4. Min edge
        if edge < self.config.min_edge {
            return Err(RejectReason::InsufficientEdge {
                edge: edge.to_string(),
                min: self.config.min_edge.to_string(),
            });
        }

        // 5. Insufficient balance
        let required = size * price.unwrap_or(Decimal::ONE);
        if world.balance < required {
            return Err(RejectReason::InsufficientBalance);
        }

        // 6. No-trade zone
        if let Some(market) = world.markets.get(market_id) {
            if let Some(end_date) = market.info.end_date {
                let now = chrono::Utc::now();
                if end_date > now {
                    let seconds_remaining = (end_date - now).num_seconds() as u64;
                    if seconds_remaining < self.config.no_trade_zone_secs {
                        return Err(RejectReason::NoTradeZone { seconds_remaining });
                    }
                } else {
                    return Err(RejectReason::NoTradeZone {
                        seconds_remaining: 0,
                    });
                }
            }
        }

        Ok(())
    }

    /// Add realized PnL to the daily running total.
    pub fn update_pnl(&mut self, pnl: Decimal) {
        self.daily_pnl += pnl;
    }

    /// Set the current number of open positions.
    pub fn update_position_count(&mut self, count: usize) {
        self.position_count = count;
    }

    /// Reset the daily PnL counter (call at start of day).
    pub fn reset_daily(&mut self) {
        self.daily_pnl = Decimal::ZERO;
    }

    /// Force-trip the circuit breaker. Once tripped, all signals are rejected.
    pub fn trip(&mut self) {
        self.tripped = true;
    }

    /// Returns true if the breaker has been force-tripped.
    pub fn is_tripped(&self) -> bool {
        self.tripped
    }

    /// Returns the current daily PnL.
    pub fn daily_pnl(&self) -> Decimal {
        self.daily_pnl
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use pmbot_core::types::{MarketId, Side, SignalId, TokenId};
    use rust_decimal_macros::dec;
    use std::collections::HashMap;

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
            kill_switch_path: "/tmp/pmbot-risk-test-kill-NONEXISTENT".into(),
        }
    }

    fn test_signal(edge: Decimal, size: Decimal, price: Decimal) -> Signal {
        Signal::Enter {
            id: SignalId::new(),
            strategy: "test",
            market_id: MarketId("m1".into()),
            token_id: TokenId("t1".into()),
            side: Side::Buy,
            size,
            price: Some(price),
            edge,
            confidence: dec!(0.70),
        }
    }

    fn test_world(balance: Decimal) -> WorldState {
        WorldState {
            active_market_id: None,
            markets: HashMap::new(),
            positions: vec![],
            open_orders: vec![],
            balance,
            daily_pnl: Decimal::ZERO,
            external_prices: HashMap::new(),
            network_latency: HashMap::new(),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_passes_all_checks() {
        let breaker = CircuitBreaker::new(test_config());
        let signal = test_signal(dec!(0.10), dec!(50), dec!(0.50));
        let world = test_world(dec!(1000));
        assert!(breaker.check(&signal, &world).is_ok());
    }

    #[test]
    fn test_rejects_kill_switch_tripped() {
        let mut breaker = CircuitBreaker::new(test_config());
        breaker.trip();
        let signal = test_signal(dec!(0.10), dec!(50), dec!(0.50));
        let world = test_world(dec!(1000));
        assert_eq!(
            breaker.check(&signal, &world).unwrap_err(),
            RejectReason::KillSwitchActive
        );
    }

    #[test]
    fn test_rejects_kill_switch_file() {
        let kill_path = "/tmp/pmbot-risk-test-kill-file";
        std::fs::write(kill_path, "").unwrap();

        let mut config = test_config();
        config.kill_switch_path = kill_path.into();
        let breaker = CircuitBreaker::new(config);

        let signal = test_signal(dec!(0.10), dec!(50), dec!(0.50));
        let world = test_world(dec!(1000));
        let result = breaker.check(&signal, &world);

        std::fs::remove_file(kill_path).ok();
        assert_eq!(result.unwrap_err(), RejectReason::KillSwitchActive);
    }

    #[test]
    fn test_rejects_daily_loss_exceeded() {
        let mut breaker = CircuitBreaker::new(test_config());
        // bankroll=1000, daily_loss_limit_pct=0.05 => limit=50
        // pnl of -51 should trip
        breaker.update_pnl(dec!(-51));
        let signal = test_signal(dec!(0.10), dec!(50), dec!(0.50));
        let world = test_world(dec!(1000));
        assert_eq!(
            breaker.check(&signal, &world).unwrap_err(),
            RejectReason::DailyLossExceeded
        );
    }

    #[test]
    fn test_rejects_max_positions() {
        let mut breaker = CircuitBreaker::new(test_config());
        breaker.update_position_count(3); // max_positions=3
        let signal = test_signal(dec!(0.10), dec!(50), dec!(0.50));
        let world = test_world(dec!(1000));
        assert_eq!(
            breaker.check(&signal, &world).unwrap_err(),
            RejectReason::MaxPositionsReached
        );
    }

    #[test]
    fn test_rejects_insufficient_edge() {
        let breaker = CircuitBreaker::new(test_config());
        let signal = test_signal(dec!(0.05), dec!(50), dec!(0.50)); // edge=0.05 < min_edge=0.08
        let world = test_world(dec!(1000));
        match breaker.check(&signal, &world).unwrap_err() {
            RejectReason::InsufficientEdge { edge, min } => {
                assert_eq!(edge, "0.05");
                assert_eq!(min, "0.08");
            }
            other => panic!("expected InsufficientEdge, got: {other:?}"),
        }
    }

    #[test]
    fn test_rejects_insufficient_balance() {
        let breaker = CircuitBreaker::new(test_config());
        let signal = test_signal(dec!(0.10), dec!(100), dec!(0.50)); // required=50
        let world = test_world(dec!(10)); // balance=10 < 50
        assert_eq!(
            breaker.check(&signal, &world).unwrap_err(),
            RejectReason::InsufficientBalance
        );
    }

    #[test]
    fn test_exit_signal_always_passes() {
        let mut breaker = CircuitBreaker::new(test_config());
        breaker.trip(); // tripped but exit should still pass
        let signal = Signal::Exit {
            id: SignalId::new(),
            strategy: "test",
            signal_id: SignalId::new(),
            reason: pmbot_core::types::ExitReason::StrategyExit,
        };
        let world = test_world(dec!(0));
        assert!(breaker.check(&signal, &world).is_ok());
    }

    #[test]
    fn test_cancel_all_signal_always_passes() {
        let mut breaker = CircuitBreaker::new(test_config());
        breaker.trip();
        let signal = Signal::CancelAll {
            market_id: MarketId("m1".into()),
        };
        let world = test_world(dec!(0));
        assert!(breaker.check(&signal, &world).is_ok());
    }

    #[test]
    fn test_reset_daily() {
        let mut breaker = CircuitBreaker::new(test_config());
        breaker.update_pnl(dec!(-51));
        assert_eq!(breaker.daily_pnl(), dec!(-51));
        breaker.reset_daily();
        assert_eq!(breaker.daily_pnl(), Decimal::ZERO);
    }

    #[test]
    fn test_update_pnl_accumulates() {
        let mut breaker = CircuitBreaker::new(test_config());
        breaker.update_pnl(dec!(10));
        breaker.update_pnl(dec!(-5));
        breaker.update_pnl(dec!(3));
        assert_eq!(breaker.daily_pnl(), dec!(8));
    }

    #[test]
    fn test_trip_and_is_tripped() {
        let mut breaker = CircuitBreaker::new(test_config());
        assert!(!breaker.is_tripped());
        breaker.trip();
        assert!(breaker.is_tripped());
    }
}
