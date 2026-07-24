//! Global rate limiter — token bucket per-tenant.
//!
//! Provides per-tenant token bucket rate limiting (sliding window,
//! configurable rate).  Delegates to `RateLimitMiddleware` internally.

use crate::protocol::rate_limit::{RateLimitMiddleware, TenantRateLimit};

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
///
/// Wraps a `RateLimitMiddleware` internally so that all per-tenant token
/// bucket logic is handled by the shared middleware implementation.
pub struct GlobalRateLimiter {
    inner: RateLimitMiddleware,
}

impl GlobalRateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            inner: RateLimitMiddleware::new(TenantRateLimit {
                rpm: config.tenant_rps.max(1.0) as u64,
                burst: config.tenant_burst as u64,
            }),
        }
    }

    /// Try to consume a token for the given tenant.
    /// Returns true if allowed, false if rate limited.
    pub fn try_consume_tenant(&self, tenant_id: &str, tokens: f64) -> bool {
        self.inner.try_consume_tenant(tenant_id, tokens)
    }
}
