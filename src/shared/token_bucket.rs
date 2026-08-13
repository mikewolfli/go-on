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
}

/// Convert a requests-per-minute limit to a tokens-per-second refill rate.
///
/// Single conversion point for all rate limiter entry points so the rpm→
/// per-second conversion is never duplicated (previously hand-written in
/// `RateLimitMiddleware` and `PhaseRateLimiter`).
pub fn rpm_to_refill_per_second(rpm: u64) -> f64 {
    rpm as f64 / 60.0
}

/// A thread-safe map of named token buckets.
///
/// Shared by `RateLimitMiddleware` (tenant-level) and `PhaseRateLimiter` (phase-level).
/// Internally uses `std::sync::Mutex` because all operations are short synchronous
/// critical sections that never hold the lock across `.await` points.
#[derive(Debug, Default)]
#[allow(clippy::len_without_is_empty)]
pub struct BucketMap {
    inner: Mutex<HashMap<String, TokenBucket>>,
}

/// Maximum number of tracked buckets. Bucket keys are caller-controlled
/// (tenant ids, phase keys, client IPs for the entry rate limiter), so an
/// unbounded map would grow without limit on a long-running service. When
/// exceeded, an arbitrary non-current bucket is evicted (a rate-limiter cache
/// eviction is harmless: the evicted key simply gets a fresh bucket on its
/// next request).
pub const MAX_BUCKETS: usize = 10_000;

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
        self.try_consume_n(key, 1.0, burst, refill_rate)
    }

    /// Try to consume `tokens` (possibly > 1) from the named bucket.
    ///
    /// Creates the bucket with the given `burst` capacity and `refill_rate`
    /// (tokens/sec) if it does not already exist, and re-creates it when the
    /// parameters change. Returns `true` if the tokens were available and
    /// consumed, `false` if rate-limited.
    pub fn try_consume_n(&self, key: &str, tokens: f64, burst: f64, refill_rate: f64) -> bool {
        let mut map = crate::lock_or_recover!(self.inner);
        // Bounded cache: evict an arbitrary bucket when over capacity (never
        // the just-accessed key) so a flood of distinct keys cannot grow the
        // map without bound.
        if map.len() >= MAX_BUCKETS && !map.contains_key(key) {
            if let Some(evict) = map.keys().next().cloned() {
                map.remove(&evict);
            }
        }
        let bucket = map
            .entry(key.to_string())
            .or_insert_with(|| TokenBucket::new(burst, refill_rate));

        // Re-create if params changed
        if (bucket.capacity - burst).abs() > f64::EPSILON
            || (bucket.refill_rate - refill_rate).abs() > f64::EPSILON
        {
            *bucket = TokenBucket::new(burst, refill_rate);
        }

        bucket.try_consume(tokens)
    }

    /// Number of tracked buckets.
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
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
