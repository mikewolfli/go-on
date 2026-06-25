//! Global rate limiter — token bucket per-tenant + global max concurrent.
//!
//! Provides two layers of rate limiting:
//! 1. Per-tenant token bucket (sliding window, configurable rate)
//! 2. Global max concurrent requests (semaphore-based)

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::shared::token_bucket::TokenBucket;
use tokio::sync::Semaphore;

/// Rate limiter configuration.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Max requests per second per tenant (token bucket refill rate).
    pub tenant_rps: f64,
    /// Max burst size per tenant.
    pub tenant_burst: u32,
    /// Max concurrent requests across all tenants (semaphore limit).
    pub max_concurrent: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            tenant_rps: 100.0,
            tenant_burst: 50,
            max_concurrent: 1000,
        }
    }
}

/// A permit guard that auto-releases the global semaphore slot on drop.
#[allow(
    dead_code,
    reason = "New API surface — wired from ACP HTTP request handler in subsequent PR"
)]
pub struct MaxConcurrentGuard<'a> {
    _permit: tokio::sync::SemaphorePermit<'a>,
}

/// Global rate limiter instance.
pub struct GlobalRateLimiter {
    config: RateLimitConfig,
    tenants: Mutex<HashMap<String, TokenBucket>>,
    /// Global max-concurrent semaphore.
    #[allow(
        dead_code,
        reason = "New API surface — wired from ACP HTTP request handler in subsequent PR"
    )]
    semaphore: Semaphore,
}

impl GlobalRateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        let max = config.max_concurrent;
        Self {
            config,
            tenants: Mutex::new(HashMap::new()),
            semaphore: Semaphore::new(max),
        }
    }

    /// Try to consume a token for the given tenant.
    /// Returns true if allowed, false if rate limited.
    pub fn try_consume_tenant(&self, tenant_id: &str, tokens: f64) -> bool {
        let mut tenants = self.tenants.lock().unwrap();
        let bucket = tenants.entry(tenant_id.to_string()).or_insert_with(|| {
            TokenBucket::new(self.config.tenant_burst as f64, self.config.tenant_rps)
        });
        bucket.try_consume(tokens)
    }

    /// Try to acquire the global semaphore permit.
    /// Returns `None` if the global concurrency limit is reached.
    #[allow(
        dead_code,
        reason = "New API surface — wired from ACP HTTP request handler in subsequent PR"
    )]
    pub fn try_acquire_global(&self) -> Option<MaxConcurrentGuard<'_>> {
        self.semaphore
            .try_acquire()
            .ok()
            .map(|permit| MaxConcurrentGuard { _permit: permit })
    }

    /// Manually release one global semaphore slot.
    /// Prefer using `MaxConcurrentGuard` (auto-release via Drop) instead.
    #[allow(
        dead_code,
        reason = "New API surface — wired from ACP HTTP request handler in subsequent PR"
    )]
    pub fn release_global(&self) {
        self.semaphore.add_permits(1);
    }
}

static GLOBAL_RATE_LIMITER: OnceLock<GlobalRateLimiter> = OnceLock::new();

pub fn global_rate_limiter() -> &'static GlobalRateLimiter {
    GLOBAL_RATE_LIMITER.get_or_init(|| GlobalRateLimiter::new(RateLimitConfig::default()))
}
