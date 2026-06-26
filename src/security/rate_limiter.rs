//! Global rate limiter — token bucket per-tenant.
//!
//! Provides per-tenant token bucket rate limiting (sliding window,
//! configurable rate).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::shared::token_bucket::TokenBucket;

/// Rate limiter configuration.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Max requests per second per tenant (token bucket refill rate).
    pub tenant_rps: f64,
    /// Max burst size per tenant.
    pub tenant_burst: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            tenant_rps: 100.0,
            tenant_burst: 50,
        }
    }
}

/// Global rate limiter instance.
pub struct GlobalRateLimiter {
    config: RateLimitConfig,
    tenants: Mutex<HashMap<String, TokenBucket>>,
}

impl GlobalRateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            tenants: Mutex::new(HashMap::new()),
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
}

static GLOBAL_RATE_LIMITER: OnceLock<GlobalRateLimiter> = OnceLock::new();

pub fn global_rate_limiter() -> &'static GlobalRateLimiter {
    GLOBAL_RATE_LIMITER.get_or_init(|| GlobalRateLimiter::new(RateLimitConfig::default()))
}
