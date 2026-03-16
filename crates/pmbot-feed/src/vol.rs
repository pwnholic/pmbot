use std::collections::VecDeque;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

// ---------------------------------------------------------------------------
// Volatility method enum
// ---------------------------------------------------------------------------

/// Method used to compute realized volatility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolMethod {
    /// Exponentially weighted moving average (lambda = 0.94).
    Ewma,
    /// Simple rolling standard deviation of log returns.
    Rolling,
    /// Parkinson high-low estimator using price range.
    Parkinson,
}

impl VolMethod {
    /// Parse from config string (e.g. "ewma", "rolling", "parkinson").
    pub fn from_str_config(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "rolling" => Self::Rolling,
            "parkinson" => Self::Parkinson,
            _ => Self::Ewma,
        }
    }
}

// ---------------------------------------------------------------------------
// VolComputer
// ---------------------------------------------------------------------------

/// Computes realized volatility from a stream of prices.
pub struct VolComputer {
    method: VolMethod,
    window: Duration,
    prices: VecDeque<(DateTime<Utc>, Decimal)>,
}

impl VolComputer {
    /// Create a new volatility computer.
    pub fn new(method: VolMethod, window: Duration) -> Self {
        Self {
            method,
            window,
            prices: VecDeque::new(),
        }
    }

    /// Record a new price observation, evicting stale entries.
    pub fn record(&mut self, price: Decimal, ts: DateTime<Utc>) {
        // Evict entries outside the window.
        let cutoff = ts - self.window;
        while self.prices.front().is_some_and(|(t, _)| *t < cutoff) {
            self.prices.pop_front();
        }
        self.prices.push_back((ts, price));
    }

    /// Compute realized volatility based on the configured method.
    ///
    /// Returns `None` if insufficient data (need at least 2 prices for
    /// EWMA/Rolling, at least 1 for Parkinson).
    pub fn realized_vol(&self) -> Option<Decimal> {
        match self.method {
            VolMethod::Ewma => self.ewma_vol(),
            VolMethod::Rolling => self.rolling_vol(),
            VolMethod::Parkinson => self.parkinson_vol(),
        }
    }

    /// Number of price observations currently in the window.
    pub fn len(&self) -> usize {
        self.prices.len()
    }

    /// Whether there are no observations.
    pub fn is_empty(&self) -> bool {
        self.prices.is_empty()
    }

    // -----------------------------------------------------------------------
    // Private implementations
    // -----------------------------------------------------------------------

    /// EWMA volatility with lambda = 0.94.
    ///
    /// Computes exponentially weighted variance of log returns, then takes
    /// the square root.
    fn ewma_vol(&self) -> Option<Decimal> {
        let returns = self.log_returns()?;
        if returns.is_empty() {
            return None;
        }

        let lambda = dec!(0.94);
        let one_minus_lambda = Decimal::ONE - lambda;

        let mut variance = Decimal::ZERO;
        for r in &returns {
            variance = lambda * variance + one_minus_lambda * r * r;
        }

        decimal_sqrt(variance)
    }

    /// Simple rolling standard deviation of log returns.
    fn rolling_vol(&self) -> Option<Decimal> {
        let returns = self.log_returns()?;
        let n = returns.len();
        if n < 2 {
            return None;
        }

        let n_dec = Decimal::from(n as u64);
        let mean: Decimal = returns.iter().copied().sum::<Decimal>() / n_dec;

        let sum_sq: Decimal = returns.iter().map(|r| (*r - mean) * (*r - mean)).sum();
        let variance = sum_sq / Decimal::from((n - 1) as u64);

        decimal_sqrt(variance)
    }

    /// Parkinson volatility estimator.
    ///
    /// Uses the range (max - min) of prices in the window:
    /// vol = (high - low) / (2 * sqrt(ln(2)))
    ///
    /// This is a simplified version that works with tick data rather than
    /// OHLC bars.
    fn parkinson_vol(&self) -> Option<Decimal> {
        if self.prices.len() < 2 {
            return None;
        }

        let high = self.prices.iter().map(|(_, p)| *p).max()?;
        let low = self.prices.iter().map(|(_, p)| *p).min()?;

        if low <= Decimal::ZERO || high <= Decimal::ZERO {
            return None;
        }

        let range = high - low;
        // Parkinson constant: 1 / (2 * sqrt(ln(2))) ~ 1 / (2 * 0.8326) ~ 0.6006
        // We use the simplified form: vol ~ range / (2 * 0.8326)
        // But for tick data, we normalize by the midpoint price.
        let mid = (high + low) / Decimal::TWO;
        if mid.is_zero() {
            return None;
        }

        // Parkinson: vol = (1 / (2 * sqrt(ln2))) * ln(H/L)
        // Approximate ln(H/L) ~ (H - L) / mid for small moves
        let log_ratio = range / mid;

        // 1 / (2 * sqrt(ln(2))) approx 0.6006
        let parkinson_const = dec!(0.6006);
        Some(parkinson_const * log_ratio)
    }

    /// Compute log returns from the price series.
    ///
    /// Approximates ln(p1/p0) as (p1 - p0) / p0 for small returns.
    fn log_returns(&self) -> Option<Vec<Decimal>> {
        if self.prices.len() < 2 {
            return None;
        }

        let mut returns = Vec::with_capacity(self.prices.len() - 1);
        let prices: Vec<_> = self.prices.iter().collect();

        for i in 1..prices.len() {
            let prev = prices[i - 1].1;
            let curr = prices[i].1;
            if prev.is_zero() {
                continue;
            }
            // Approximate log return: (curr - prev) / prev
            returns.push((curr - prev) / prev);
        }

        if returns.is_empty() {
            None
        } else {
            Some(returns)
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Approximate square root for `Decimal` using Newton's method.
///
/// Returns `None` for negative inputs. Returns `Some(Decimal::ZERO)` for zero.
fn decimal_sqrt(val: Decimal) -> Option<Decimal> {
    if val < Decimal::ZERO {
        return None;
    }
    if val.is_zero() {
        return Some(Decimal::ZERO);
    }

    // Newton's method: x_{n+1} = (x_n + val / x_n) / 2
    let mut guess = val / Decimal::TWO;
    if guess.is_zero() {
        guess = Decimal::ONE;
    }

    for _ in 0..50 {
        let next = (guess + val / guess) / Decimal::TWO;
        let diff = (next - guess).abs();
        guess = next;
        // Converge to ~12 decimal places
        if diff < dec!(0.000000000001) {
            break;
        }
    }

    Some(guess)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    #[test]
    fn test_vol_computer_record_and_eviction() {
        let mut vc = VolComputer::new(VolMethod::Rolling, Duration::from_secs(10));

        vc.record(dec!(100), ts(0));
        vc.record(dec!(101), ts(5));
        vc.record(dec!(102), ts(10));
        assert_eq!(vc.len(), 3);

        // Recording at ts(15) should evict ts(0) since window is 10s
        vc.record(dec!(103), ts(15));
        // ts(0) is outside [15-10, 15] = [5, 15]
        assert_eq!(vc.len(), 3);
    }

    #[test]
    fn test_vol_computer_empty() {
        let vc = VolComputer::new(VolMethod::Ewma, Duration::from_secs(60));
        assert!(vc.is_empty());
        assert!(vc.realized_vol().is_none());
    }

    #[test]
    fn test_vol_computer_single_price() {
        let mut vc = VolComputer::new(VolMethod::Rolling, Duration::from_secs(60));
        vc.record(dec!(100), ts(0));
        assert_eq!(vc.len(), 1);
        assert!(vc.realized_vol().is_none());
    }

    #[test]
    fn test_rolling_vol_constant_price() {
        let mut vc = VolComputer::new(VolMethod::Rolling, Duration::from_secs(60));
        for i in 0..10 {
            vc.record(dec!(100), ts(i));
        }
        let vol = vc.realized_vol().unwrap();
        assert_eq!(vol, Decimal::ZERO);
    }

    #[test]
    fn test_rolling_vol_with_movement() {
        let mut vc = VolComputer::new(VolMethod::Rolling, Duration::from_secs(60));
        // Prices: 100, 102, 98, 101, 99
        vc.record(dec!(100), ts(0));
        vc.record(dec!(102), ts(1));
        vc.record(dec!(98), ts(2));
        vc.record(dec!(101), ts(3));
        vc.record(dec!(99), ts(4));

        let vol = vc.realized_vol().unwrap();
        assert!(vol > Decimal::ZERO, "vol should be positive: {vol}");
    }

    #[test]
    fn test_ewma_vol_with_movement() {
        let mut vc = VolComputer::new(VolMethod::Ewma, Duration::from_secs(60));
        vc.record(dec!(100), ts(0));
        vc.record(dec!(102), ts(1));
        vc.record(dec!(98), ts(2));
        vc.record(dec!(101), ts(3));
        vc.record(dec!(99), ts(4));

        let vol = vc.realized_vol().unwrap();
        assert!(vol > Decimal::ZERO, "EWMA vol should be positive: {vol}");
    }

    #[test]
    fn test_ewma_vol_constant_price() {
        let mut vc = VolComputer::new(VolMethod::Ewma, Duration::from_secs(60));
        for i in 0..10 {
            vc.record(dec!(100), ts(i));
        }
        let vol = vc.realized_vol().unwrap();
        assert_eq!(vol, Decimal::ZERO);
    }

    #[test]
    fn test_parkinson_vol_with_range() {
        let mut vc = VolComputer::new(VolMethod::Parkinson, Duration::from_secs(60));
        vc.record(dec!(100), ts(0));
        vc.record(dec!(105), ts(1));
        vc.record(dec!(95), ts(2));
        vc.record(dec!(100), ts(3));

        let vol = vc.realized_vol().unwrap();
        assert!(vol > Decimal::ZERO, "Parkinson vol should be positive: {vol}");
    }

    #[test]
    fn test_parkinson_vol_constant_price() {
        let mut vc = VolComputer::new(VolMethod::Parkinson, Duration::from_secs(60));
        for i in 0..5 {
            vc.record(dec!(100), ts(i));
        }
        let vol = vc.realized_vol().unwrap();
        assert_eq!(vol, Decimal::ZERO);
    }

    #[test]
    fn test_vol_method_from_str() {
        assert_eq!(VolMethod::from_str_config("ewma"), VolMethod::Ewma);
        assert_eq!(VolMethod::from_str_config("EWMA"), VolMethod::Ewma);
        assert_eq!(VolMethod::from_str_config("rolling"), VolMethod::Rolling);
        assert_eq!(VolMethod::from_str_config("parkinson"), VolMethod::Parkinson);
        assert_eq!(VolMethod::from_str_config("unknown"), VolMethod::Ewma);
    }

    #[test]
    fn test_decimal_sqrt() {
        let result = decimal_sqrt(dec!(4)).unwrap();
        let diff = (result - dec!(2)).abs();
        assert!(diff < dec!(0.000001), "sqrt(4) = {result}, expected ~2");

        let result = decimal_sqrt(dec!(0)).unwrap();
        assert_eq!(result, Decimal::ZERO);

        assert!(decimal_sqrt(dec!(-1)).is_none());
    }
}
