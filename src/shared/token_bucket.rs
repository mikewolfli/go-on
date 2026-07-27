//! A generic token bucket rate limiter.
//!
//! Provides both `Instant`-based and ms-epoch-based refill strategies,
//! consolidating three duplicate implementations from across the project.

use std::collections::HashMap;
use std::sync::Mutex;
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
    /// Kept for API compatibility; unused internally after BucketMap migration.
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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

/// A thread-safe map of named token buckets.
///
/// Shared by `RateLimitMiddleware` (tenant-level) and `PhaseRateLimiter` (phase-level).
/// Internally uses `std::sync::Mutex` because all operations are short synchronous
/// critical sections that never hold the lock across `.await` points.
#[derive(Debug, Default)]
pub struct BucketMap {
    inner: Mutex<HashMap<String, TokenBucket>>,
}

impl BucketMap {
    /// Create an empty `BucketMap`.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Try to consume a single token from the named bucket.
    ///
    /// Creates the bucket with the given `burst` capacity and `refill_rate` (tokens/sec)
    /// if it does not already exist. If the bucket exists with different parameters,
    /// it is re-created with the new parameters.
    ///
    /// Returns `true` if the token was consumed, `false` if rate-limited.
    pub fn try_consume(&self, key: &str, burst: f64, refill_rate: f64) -> bool {
        let mut map = crate::lock_or_recover!(self.inner);
        let bucket = map
            .entry(key.to_string())
            .or_insert_with(|| TokenBucket::new(burst, refill_rate));

        // Re-create if params changed
        if (bucket.capacity - burst).abs() > f64::EPSILON
            || (bucket.refill_rate - refill_rate).abs() > f64::EPSILON
        {
            *bucket = TokenBucket::new(burst, refill_rate);
        }

        bucket.try_consume(1.0)
    }

    /// Number of tracked buckets.
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Returns `true` if the map contains no buckets.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Snapshot of current tokens and capacity per key.
    pub fn snapshot(&self) -> HashMap<String, (f64, f64)> {
        self.inner
            .lock()
            .map(|guard| {
                guard
                    .iter()
                    .map(|(k, v)| (k.clone(), (v.tokens, v.capacity)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Retain only buckets for which the predicate returns `true`.
    pub fn retain<F>(&self, mut f: F)
    where
        F: FnMut(&str, &mut TokenBucket) -> bool,
    {
        let mut map = crate::lock_or_recover!(self.inner);
        map.retain(|k, v| f(k.as_str(), v));
    }

    /// Provide temporary access to the locked inner map.
    /// Used by `RateLimitMiddleware` for operations that inspect/modify
    /// specific buckets beyond what the high-level API provides.
    pub fn with_lock<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&mut HashMap<String, TokenBucket>) -> T,
    {
        let mut map = crate::lock_or_recover!(self.inner);
        f(&mut map)
    }
}
