use std::collections::VecDeque;

use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use pmbot_core::types::PricePoint;

/// A detected crash event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashEvent {
    pub peak_price: Decimal,
    pub current_price: Decimal,
    pub drop_pct: Decimal,
    pub peak_time: DateTime<Utc>,
    pub detected_at: DateTime<Utc>,
}

/// Rolling price history with crash detection and volatility computation.
#[derive(Debug, Clone)]
pub struct PriceTracker {
    history: VecDeque<PricePoint>,
    max_history: usize,
}

impl PriceTracker {
    /// Create a new tracker with the given ring buffer capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            history: VecDeque::with_capacity(capacity),
            max_history: capacity,
        }
    }

    /// Record a new price point. Evicts the oldest entry if at capacity.
    pub fn record(&mut self, price: Decimal, ts: DateTime<Utc>) {
        if self.history.len() >= self.max_history {
            self.history.pop_front();
        }
        self.history.push_back(PricePoint {
            price,
            timestamp: ts,
        });
    }

    /// The most recent price, if any.
    pub fn current_price(&self) -> Option<Decimal> {
        self.history.back().map(|p| p.price)
    }

    /// Detect if price has dropped more than `threshold` percent within `window`.
    ///
    /// `threshold` is expressed as a fraction (e.g., 0.10 for 10%).
    pub fn detect_crash(
        &self,
        threshold: Decimal,
        window: Duration,
    ) -> Option<CrashEvent> {
        let current = self.history.back()?;
        let cutoff = current.timestamp - window;

        // Find the peak price within the window
        let mut peak_price = current.price;
        let mut peak_time = current.timestamp;

        for point in self.history.iter().rev() {
            if point.timestamp < cutoff {
                break;
            }
            if point.price > peak_price {
                peak_price = point.price;
                peak_time = point.timestamp;
            }
        }

        if peak_price.is_zero() {
            return None;
        }

        let drop_pct = (peak_price - current.price) / peak_price;

        if drop_pct >= threshold {
            Some(CrashEvent {
                peak_price,
                current_price: current.price,
                drop_pct,
                peak_time,
                detected_at: current.timestamp,
            })
        } else {
            None
        }
    }

    /// Rolling standard deviation of returns within the given window.
    pub fn volatility(&self, window: Duration) -> Option<Decimal> {
        let points = self.points_in_window(window);
        if points.len() < 2 {
            return None;
        }

        // Compute returns
        let returns: Vec<Decimal> = points
            .windows(2)
            .filter_map(|w| {
                if w[0].price.is_zero() {
                    None
                } else {
                    Some((w[1].price - w[0].price) / w[0].price)
                }
            })
            .collect();

        if returns.is_empty() {
            return None;
        }

        let n = Decimal::from(returns.len() as u64);
        let mean: Decimal = returns.iter().copied().sum::<Decimal>() / n;

        let variance: Decimal = returns
            .iter()
            .map(|r| {
                let diff = *r - mean;
                diff * diff
            })
            .sum::<Decimal>()
            / n;

        // Manual sqrt via Newton's method for Decimal
        Some(decimal_sqrt(variance))
    }

    /// (min, max) price within the given window.
    pub fn price_range(&self, window: Duration) -> Option<(Decimal, Decimal)> {
        let points = self.points_in_window(window);
        if points.is_empty() {
            return None;
        }

        let mut min = points[0].price;
        let mut max = points[0].price;

        for p in &points[1..] {
            if p.price < min {
                min = p.price;
            }
            if p.price > max {
                max = p.price;
            }
        }

        Some((min, max))
    }

    /// Mean price within the given window.
    pub fn mean_price(&self, window: Duration) -> Option<Decimal> {
        let points = self.points_in_window(window);
        if points.is_empty() {
            return None;
        }

        let sum: Decimal = points.iter().map(|p| p.price).sum();
        Some(sum / Decimal::from(points.len() as u64))
    }

    /// Collect points within the window (from now backwards).
    fn points_in_window(&self, window: Duration) -> Vec<&PricePoint> {
        let Some(latest) = self.history.back() else {
            return Vec::new();
        };
        let cutoff = latest.timestamp - window;

        self.history
            .iter()
            .filter(|p| p.timestamp >= cutoff)
            .collect()
    }

    /// Number of recorded points.
    pub fn len(&self) -> usize {
        self.history.len()
    }

    /// Whether the tracker has no data.
    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }
}

/// Newton's method square root for Decimal.
fn decimal_sqrt(val: Decimal) -> Decimal {
    if val.is_zero() || val == Decimal::ONE {
        return val;
    }
    // Start with val/2 as initial guess
    let two = Decimal::TWO;
    let mut guess = val / two;
    // 20 iterations is more than enough for convergence
    for _ in 0..20 {
        if guess.is_zero() {
            return Decimal::ZERO;
        }
        guess = (guess + val / guess) / two;
    }
    guess
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use rust_decimal_macros::dec;

    fn ts(secs_offset: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + secs_offset, 0).unwrap()
    }

    #[test]
    fn test_record_and_current_price() {
        let mut tracker = PriceTracker::new(10);
        assert!(tracker.current_price().is_none());

        tracker.record(dec!(0.50), ts(0));
        assert_eq!(tracker.current_price(), Some(dec!(0.50)));

        tracker.record(dec!(0.55), ts(1));
        assert_eq!(tracker.current_price(), Some(dec!(0.55)));
    }

    #[test]
    fn test_ring_buffer_eviction() {
        let mut tracker = PriceTracker::new(3);
        tracker.record(dec!(1.0), ts(0));
        tracker.record(dec!(2.0), ts(1));
        tracker.record(dec!(3.0), ts(2));
        tracker.record(dec!(4.0), ts(3));

        assert_eq!(tracker.len(), 3);
        // Oldest (1.0) should be evicted
        assert_eq!(tracker.current_price(), Some(dec!(4.0)));
    }

    #[test]
    fn test_detect_crash() {
        let mut tracker = PriceTracker::new(100);
        // Price goes up then crashes
        tracker.record(dec!(1.00), ts(0));
        tracker.record(dec!(1.10), ts(10));
        tracker.record(dec!(1.20), ts(20)); // peak
        tracker.record(dec!(1.00), ts(30));
        tracker.record(dec!(0.90), ts(40)); // 25% drop from 1.20

        let crash = tracker.detect_crash(dec!(0.20), Duration::seconds(60));
        assert!(crash.is_some());
        let event = crash.unwrap();
        assert_eq!(event.peak_price, dec!(1.20));
        assert_eq!(event.current_price, dec!(0.90));
        assert_eq!(event.drop_pct, dec!(0.25));
    }

    #[test]
    fn test_no_crash_when_within_threshold() {
        let mut tracker = PriceTracker::new(100);
        tracker.record(dec!(1.00), ts(0));
        tracker.record(dec!(0.95), ts(10)); // 5% drop

        let crash = tracker.detect_crash(dec!(0.10), Duration::seconds(60));
        assert!(crash.is_none());
    }

    #[test]
    fn test_detect_crash_respects_window() {
        let mut tracker = PriceTracker::new(100);
        tracker.record(dec!(1.20), ts(0));   // peak, but outside window
        tracker.record(dec!(1.00), ts(100));
        tracker.record(dec!(0.90), ts(110));

        // Window of 20s: only sees 1.00 and 0.90 (10% drop from 1.00)
        let crash = tracker.detect_crash(dec!(0.20), Duration::seconds(20));
        assert!(crash.is_none()); // 10% < 20% threshold
    }

    #[test]
    fn test_mean_price() {
        let mut tracker = PriceTracker::new(100);
        tracker.record(dec!(1.0), ts(0));
        tracker.record(dec!(2.0), ts(1));
        tracker.record(dec!(3.0), ts(2));

        let mean = tracker.mean_price(Duration::seconds(60)).unwrap();
        assert_eq!(mean, dec!(2.0));
    }

    #[test]
    fn test_mean_price_respects_window() {
        let mut tracker = PriceTracker::new(100);
        tracker.record(dec!(10.0), ts(0));  // outside window
        tracker.record(dec!(2.0), ts(50));
        tracker.record(dec!(4.0), ts(55));

        // Window of 10s from latest (ts=55): cutoff at ts=45
        let mean = tracker.mean_price(Duration::seconds(10)).unwrap();
        assert_eq!(mean, dec!(3.0)); // (2+4)/2
    }

    #[test]
    fn test_price_range() {
        let mut tracker = PriceTracker::new(100);
        tracker.record(dec!(1.5), ts(0));
        tracker.record(dec!(0.8), ts(1));
        tracker.record(dec!(2.2), ts(2));
        tracker.record(dec!(1.0), ts(3));

        let (min, max) = tracker.price_range(Duration::seconds(60)).unwrap();
        assert_eq!(min, dec!(0.8));
        assert_eq!(max, dec!(2.2));
    }

    #[test]
    fn test_volatility_returns_some() {
        let mut tracker = PriceTracker::new(100);
        tracker.record(dec!(100), ts(0));
        tracker.record(dec!(105), ts(1));
        tracker.record(dec!(103), ts(2));
        tracker.record(dec!(107), ts(3));

        let vol = tracker.volatility(Duration::seconds(60));
        assert!(vol.is_some());
        // Just verify it's positive and reasonable
        let v = vol.unwrap();
        assert!(v > Decimal::ZERO);
    }

    #[test]
    fn test_volatility_not_enough_data() {
        let mut tracker = PriceTracker::new(100);
        tracker.record(dec!(100), ts(0));

        assert!(tracker.volatility(Duration::seconds(60)).is_none());
    }

    #[test]
    fn test_empty_tracker() {
        let tracker = PriceTracker::new(10);
        assert!(tracker.current_price().is_none());
        assert!(tracker.mean_price(Duration::seconds(60)).is_none());
        assert!(tracker.price_range(Duration::seconds(60)).is_none());
        assert!(tracker.volatility(Duration::seconds(60)).is_none());
        assert!(tracker.detect_crash(dec!(0.10), Duration::seconds(60)).is_none());
        assert!(tracker.is_empty());
    }
}
