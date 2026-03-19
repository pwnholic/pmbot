use std::time::Duration;

use chrono::Utc;

use pmbot_core::types::MarketInfo;

/// Manages market window timing and rotation decisions.
#[derive(Debug, Clone)]
pub struct MarketRotator {
    market_type: String,
    no_trade_zone_secs: u64,
    lookahead_secs: u64,
    current_market: Option<MarketInfo>,
}

impl MarketRotator {
    /// Create a new rotator with the given configuration.
    pub fn new(market_type: String, no_trade_zone_secs: u64, lookahead_secs: u64) -> Self {
        Self {
            market_type,
            no_trade_zone_secs,
            lookahead_secs,
            current_market: None,
        }
    }

    /// Set the current active market.
    pub fn set_current(&mut self, market: MarketInfo) {
        self.current_market = Some(market);
    }

    /// Get a reference to the current market, if any.
    pub fn current_market(&self) -> Option<&MarketInfo> {
        self.current_market.as_ref()
    }

    /// The configured market type (e.g., "15min", "hourly").
    pub fn market_type(&self) -> &str {
        &self.market_type
    }

    /// Time remaining until the current market expires.
    /// Returns `None` if no market is set or market has no end date.
    pub fn time_to_expiry(&self) -> Option<Duration> {
        let market = self.current_market.as_ref()?;
        let end_date = market.end_date?;
        let now = Utc::now();

        if end_date <= now {
            Some(Duration::ZERO)
        } else {
            let diff = end_date - now;
            // chrono::Duration -> std::time::Duration
            diff.to_std().ok()
        }
    }

    /// True if the current market is within the no-trade zone (close to expiry).
    pub fn in_no_trade_zone(&self) -> bool {
        match self.time_to_expiry() {
            Some(tte) => tte.as_secs() < self.no_trade_zone_secs,
            None => false,
        }
    }

    /// True if the current market has expired or is ending soon enough to rotate.
    /// Also returns true if no market is set, or if the current market has no
    /// end_date (unsafe for time-bounded trading).
    pub fn should_rotate(&self) -> bool {
        match self.time_to_expiry() {
            Some(tte) => tte.is_zero() || tte.as_secs() < self.no_trade_zone_secs,
            // No market set OR market has no end_date => should rotate
            None => true,
        }
    }

    /// True if we should pre-subscribe to the next market
    /// (time to expiry is less than the lookahead threshold).
    pub fn needs_presubscribe(&self) -> bool {
        match self.time_to_expiry() {
            Some(tte) => tte.as_secs() < self.lookahead_secs,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, Utc};
    use pmbot_core::types::{MarketId, TokenId};
    use rust_decimal_macros::dec;
    use std::collections::HashMap;

    fn make_market(end_offset_secs: i64) -> MarketInfo {
        let mut outcome_prices = HashMap::new();
        outcome_prices.insert("Yes".to_string(), dec!(0.5));
        outcome_prices.insert("No".to_string(), dec!(0.5));

        MarketInfo {
            id: MarketId("test-market".into()),
            question: "Test?".into(),
            slug: "test".into(),
            outcomes: vec!["Yes".into(), "No".into()],
            token_ids: vec![TokenId("token-yes".into()), TokenId("token-no".into())],
            outcome_prices,
            condition_id: "cond-123".into(),
            neg_risk: false,
            active: true,
            end_date: Some(Utc::now() + ChronoDuration::seconds(end_offset_secs)),
            liquidity: dec!(10000),
            volume: dec!(50000),
            category: "Test".into(),
            tags: vec!["test".into()],
        }
    }

    #[test]
    fn test_no_market_set() {
        let rotator = MarketRotator::new("15min".into(), 60, 30);
        assert!(rotator.time_to_expiry().is_none());
        assert!(!rotator.in_no_trade_zone());
        assert!(rotator.should_rotate()); // no market => should rotate
        assert!(!rotator.needs_presubscribe());
    }

    #[test]
    fn test_time_to_expiry_future() {
        let mut rotator = MarketRotator::new("15min".into(), 60, 30);
        rotator.set_current(make_market(300)); // 5 minutes out

        let tte = rotator.time_to_expiry().unwrap();
        // Should be roughly 300 seconds (allow some tolerance for test execution)
        assert!(tte.as_secs() >= 298 && tte.as_secs() <= 302);
    }

    #[test]
    fn test_time_to_expiry_expired() {
        let mut rotator = MarketRotator::new("15min".into(), 60, 30);
        rotator.set_current(make_market(-10)); // 10 seconds ago

        let tte = rotator.time_to_expiry().unwrap();
        assert!(tte.is_zero());
    }

    #[test]
    fn test_in_no_trade_zone() {
        let mut rotator = MarketRotator::new("15min".into(), 60, 30);

        // 30 seconds to expiry, no-trade zone is 60s => in zone
        rotator.set_current(make_market(30));
        assert!(rotator.in_no_trade_zone());

        // 120 seconds to expiry => not in zone
        rotator.set_current(make_market(120));
        assert!(!rotator.in_no_trade_zone());
    }

    #[test]
    fn test_should_rotate() {
        let mut rotator = MarketRotator::new("15min".into(), 60, 30);

        // Expired market
        rotator.set_current(make_market(-5));
        assert!(rotator.should_rotate());

        // In no-trade zone
        rotator.set_current(make_market(30));
        assert!(rotator.should_rotate());

        // Plenty of time
        rotator.set_current(make_market(300));
        assert!(!rotator.should_rotate());
    }

    #[test]
    fn test_needs_presubscribe() {
        let mut rotator = MarketRotator::new("15min".into(), 60, 30);

        // 20 seconds to expiry, lookahead is 30s => needs presubscribe
        rotator.set_current(make_market(20));
        assert!(rotator.needs_presubscribe());

        // 120 seconds to expiry => no presubscribe needed
        rotator.set_current(make_market(120));
        assert!(!rotator.needs_presubscribe());
    }

    #[test]
    fn test_market_without_end_date() {
        let mut rotator = MarketRotator::new("15min".into(), 60, 30);
        let mut market = make_market(300);
        market.end_date = None;
        rotator.set_current(market);

        assert!(rotator.time_to_expiry().is_none());
        assert!(!rotator.in_no_trade_zone());
        // has a market but no end_date => unsafe for time-bounded trading, should rotate
        assert!(rotator.should_rotate());
        assert!(!rotator.needs_presubscribe());
    }
}
