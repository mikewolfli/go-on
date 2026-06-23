//! A generic token bucket rate limiter.
//!
//! Provides both `Instant`-based and ms-epoch-based refill strategies,
//! consolidating three duplicate implementations from across the project.

use std::time::{Duration, Instant};

/// A generic token bucket rate limiter.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    /// Current number of tokens available.
    pub tokens: f64,
    /// Maximum number of tokens the bucket can hold.
    pub capacity: f64,
    /// Tokens added per second (refill rate).
    pub refill_rate: f64,
    /// Last Instant-based refill timestamp.
    last_refill: Instant,
    /// Last ms-epoch-based refill timestamp.
    last_refill_ms: i64,
    /// Last time a token was consumed (used for idle detection).
    last_access: Instant,
}

impl TokenBucket {
    /// Create a new bucket using `Instant`-based timing.
    ///
    /// * `capacity`  – maximum tokens (burst).
    /// * `refill_rate` – tokens per second.
    pub fn new(capacity: f64, refill_rate: f64) -> Self {
        let now = Instant::now();
        Self {
            tokens: capacity,
            capacity,
            refill_rate,
            last_refill: now,
            last_refill_ms: 0,
            last_access: now,
        }
    }

    /// Create a new bucket using ms-epoch-based timing.
    ///
    /// * `capacity`       – maximum tokens (burst).
    /// * `refill_rate`    – tokens per second.
    /// * `now_ms`         – current time in milliseconds (epoch).
    pub fn new_ms(capacity: f64, refill_rate: f64, now_ms: i64) -> Self {
        let now = Instant::now();
        Self {
            tokens: capacity,
            capacity,
            refill_rate,
            last_refill: now,
            last_refill_ms: now_ms,
            last_access: now,
        }
    }

    /// Refill tokens based on `Instant::now()` elapsed time.
    pub fn refill(&mut self) {
        let elapsed = self.last_refill.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity);
            self.last_refill = Instant::now();
        }
    }

    /// Refill tokens based on a ms-epoch timestamp.
    pub fn refill_ms(&mut self, now_ms: i64) {
        let elapsed_ms = (now_ms - self.last_refill_ms).max(0) as f64;
        if elapsed_ms > 0.0 {
            let refill = elapsed_ms / 1000.0 * self.refill_rate;
            self.tokens = (self.tokens + refill).min(self.capacity);
            self.last_refill_ms = now_ms;
        }
    }

    /// Try to consume `tokens` using `Instant`-based refill.
    ///
    /// Returns `true` if the tokens were available and consumed.
    pub fn try_consume(&mut self, tokens: f64) -> bool {
        self.refill();
        self.last_access = Instant::now();
        if self.tokens >= tokens {
            self.tokens -= tokens;
            true
        } else {
            false
        }
    }

    /// Check whether the bucket has been idle longer than `idle_timeout`.
    pub fn is_idle(&self, idle_timeout: Duration) -> bool {
        self.last_access.elapsed() >= idle_timeout
    }

    /// Return the wait time in milliseconds until a single token is available.
    pub fn wait_time_ms(&self) -> u64 {
        if self.tokens >= 1.0 {
            return 0;
        }
        let deficit = 1.0 - self.tokens;
        let secs = (deficit / self.refill_rate).ceil();
        (secs * 1000.0) as u64
    }
}
