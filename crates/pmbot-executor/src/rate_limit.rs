//! Rate limiting for API calls using token bucket algorithm.
//!
//! Provides rate limiting to prevent exceeding API quotas.

use std::time::{Duration, Instant};

/// Token bucket rate limiter.
///
/// Tokens are added at a fixed rate, and each request consumes tokens.
/// If no tokens are available, requests must wait.
#[derive(Debug)]
pub struct RateLimiter {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64,
    last_refill: Instant,
}

impl RateLimiter {
    /// Create a new rate limiter.
    ///
    /// - `max_tokens`: Maximum number of tokens in the bucket
    /// - `refill_rate`: Tokens added per second
    pub fn new(max_tokens: u32, refill_rate: f64) -> Self {
        Self {
            tokens: max_tokens as f64,
            max_tokens: max_tokens as f64,
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    /// Try to acquire tokens without blocking.
    ///
    /// Returns `true` if tokens were acquired, `false` otherwise.
    pub fn try_acquire(&mut self, tokens: u32) -> bool {
        self.refill();
        if self.tokens >= tokens as f64 {
            self.tokens -= tokens as f64;
            true
        } else {
            false
        }
    }

    /// Acquire tokens, waiting if necessary.
    ///
    /// This will block until the required tokens are available.
    pub async fn acquire(&mut self, tokens: u32) {
        loop {
            self.refill();
            if self.tokens >= tokens as f64 {
                self.tokens -= tokens as f64;
                return;
            }
            // Wait before trying again
            let needed = tokens as f64 - self.tokens;
            let wait_time = Duration::from_secs_f64(needed / self.refill_rate);
            tokio::time::sleep(wait_time).await;
        }
    }

    /// Refill tokens based on elapsed time since last refill.
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        let new_tokens = elapsed * self.refill_rate;
        self.tokens = (self.tokens + new_tokens).min(self.max_tokens);
        self.last_refill = now;
    }

    /// Get the current number of available tokens.
    pub fn available(&self) -> f64 {
        // Note: this doesn't do a refill, so it's a snapshot
        self.tokens
    }
}

/// Rate limiter for different API endpoints with different limits.
#[derive(Debug)]
pub struct ApiRateLimiter {
    /// Orders per second limit.
    pub orders: RateLimiter,
    /// Reads (GET requests) per second limit.
    pub reads: RateLimiter,
    /// WebSocket messages per second limit.
    pub ws_messages: RateLimiter,
}

impl ApiRateLimiter {
    /// Create a new API rate limiter with reasonable defaults.
    ///
    /// - Orders: 5 per second (180 per minute - typical API limit)
    /// - Reads: 20 per second
    /// - WebSocket: 100 per second
    pub fn new() -> Self {
        Self {
            orders: RateLimiter::new(10, 5.0),
            reads: RateLimiter::new(40, 20.0),
            ws_messages: RateLimiter::new(200, 100.0),
        }
    }

    /// Create with custom limits.
    pub fn with_limits(orders_per_sec: f64, reads_per_sec: f64, ws_per_sec: f64) -> Self {
        Self {
            orders: RateLimiter::new(orders_per_sec as u32 * 2, orders_per_sec),
            reads: RateLimiter::new(reads_per_sec as u32 * 2, reads_per_sec),
            ws_messages: RateLimiter::new(ws_per_sec as u32 * 2, ws_per_sec),
        }
    }
}

impl Default for ApiRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_initial() {
        let limiter = RateLimiter::new(10, 5.0);
        assert_eq!(limiter.available(), 10.0);
    }

    #[test]
    fn test_rate_limiter_try_acquire_success() {
        let mut limiter = RateLimiter::new(10, 5.0);
        assert!(limiter.try_acquire(5));
        assert_eq!(limiter.available(), 5.0);
    }

    #[test]
    fn test_rate_limiter_try_acquire_fail() {
        let mut limiter = RateLimiter::new(10, 5.0);
        assert!(!limiter.try_acquire(15)); // More than available
    }

    #[tokio::test]
    async fn test_rate_limiter_acquire() {
        let mut limiter = RateLimiter::new(10, 10.0); // 10 tokens/sec
        limiter.try_acquire(10); // Use all tokens

        // Should wait and then acquire
        limiter.acquire(5).await;
        assert!(limiter.available() < 10.0);
    }

    #[test]
    fn test_api_rate_limiter_default() {
        let limiter = ApiRateLimiter::new();
        assert!(limiter.orders.try_acquire(1));
        assert!(limiter.reads.try_acquire(1));
        assert!(limiter.ws_messages.try_acquire(1));
    }
}
