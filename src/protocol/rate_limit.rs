//! RateLimitMiddleware — per-tenant rate limiting based on JWT claims
//!
//! GAP-B49-11: Extends PhaseRateLimiter with tenant-level tracking.
//! Returns 429 + Retry-After when exceeded.

// F-GAP-49: Module wired into production protocol pipeline.

use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

use crate::shared::token_bucket::{rpm_to_refill_per_second, BucketMap};

/// Rate limit configuration for a tenant
#[derive(Debug, Clone)]
pub struct TenantRateLimit {
    /// Maximum requests per minute
    pub rpm: u64,
    /// Burst capacity
    pub burst: u64,
}

impl Default for TenantRateLimit {
    fn default() -> Self {
        Self { rpm: 60, burst: 10 }
    }
}

/// Rate limit middleware
///
/// NOTE: Uses std::sync::Mutex (not tokio::sync::Mutex) because all bucket
/// operations are short synchronous critical sections that never hold the
/// lock across .await points. std::sync::Mutex is faster for this pattern.
#[derive(Debug)]
pub struct RateLimitMiddleware {
    buckets: Arc<BucketMap>,
    default_limit: TenantRateLimit,
    idle_timeout: Duration,
    max_tenants: usize,
}

impl Default for RateLimitMiddleware {
    fn default() -> Self {
        Self::new(TenantRateLimit::default())
    }
}

impl RateLimitMiddleware {
    pub const DEFAULT_MAX_TENANTS: usize = 10_000;

    pub fn new(default_limit: TenantRateLimit) -> Self {
        Self {
            buckets: Arc::new(BucketMap::new()),
            default_limit,
            idle_timeout: Duration::from_secs(3600),
            max_tenants: Self::DEFAULT_MAX_TENANTS,
        }
    }

    /// Set the idle timeout for tenant eviction.
    pub fn with_idle_timeout(mut self, idle_timeout_seconds: u64) -> Self {
        self.idle_timeout = Duration::from_secs(idle_timeout_seconds);
        self
    }

    /// Set the maximum number of tenant entries.
    pub fn with_max_tenants(mut self, max: usize) -> Self {
        self.max_tenants = max;
        self
    }

    /// Perform lazy eviction: remove idle tenant entries.
    fn lazy_evict(&self) {
        self.buckets.with_lock(|buckets| {
            if buckets.len() >= self.max_tenants {
                buckets.retain(|_, bucket| !bucket.is_idle(self.idle_timeout));
            }
        });
    }

    /// Evict a specific tenant from the rate limiter.
    pub fn evict_tenant(&self, tenant_id: &str) {
        self.buckets.with_lock(|buckets| buckets.remove(tenant_id));
    }

    /// Shared capacity guard for new tenants: lazily evicts idle entries once
    /// the bucket map is at capacity, and reports whether the tenant must be
    /// rejected because the map is still full. Used by both [`Self::check`]
    /// and [`Self::try_consume_tenant`] so no entry path can grow the bucket
    /// map without bound (previously `try_consume_tenant` bypassed both
    /// `lazy_evict` and the `max_tenants` cap).
    fn reject_tenant_if_full(&self, tenant_id: &str) -> bool {
        self.lazy_evict();
        self.buckets.with_lock(|buckets| {
            if !buckets.contains_key(tenant_id) && buckets.len() >= self.max_tenants {
                warn!(
                    "rate limit tenant limit reached (max={}), rejecting tenant '{}'",
                    self.max_tenants, tenant_id
                );
                return true;
            }
            false
        })
    }

    /// Check if a request from the given tenant should be allowed.
    /// Returns Ok(()) if allowed, or the number of seconds to wait before retrying.
    pub fn check(&self, tenant_id: &str) -> Result<(), u64> {
        if self.reject_tenant_if_full(tenant_id) {
            return Err(60);
        }

        let burst = self.default_limit.burst as f64;
        let refill_rate = rpm_to_refill_per_second(self.default_limit.rpm);

        if self.buckets.try_consume(tenant_id, burst, refill_rate) {
            Ok(())
        } else {
            let retry_after = self.buckets.with_lock(|buckets| {
                buckets
                    .get(tenant_id)
                    .map(|b| (1.0 / b.refill_rate).ceil() as u64)
                    .unwrap_or(1)
            });
            Err(retry_after.max(1))
        }
    }

    /// Try to consume tokens for a tenant (sync, no async required).
    ///
    /// Delegates to the canonical `BucketMap::try_consume_n` primitive so the
    /// create-or-recreate + refill logic lives in exactly one place, and
    /// applies the same lazy-eviction + `max_tenants` capacity guard as
    /// [`Self::check`] (previously this path bypassed both, letting the bucket
    /// map grow without bound). Returns `false` when the tenant must be
    /// rejected because the bucket map is at capacity.
    pub fn try_consume_tenant(&self, tenant_id: &str, tokens: f64) -> bool {
        if self.reject_tenant_if_full(tenant_id) {
            return false;
        }
        let burst = self.default_limit.burst as f64;
        let refill_rate = rpm_to_refill_per_second(self.default_limit.rpm);
        self.buckets
            .try_consume_n(tenant_id, tokens, burst, refill_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_rate_limiting() {
        let limiter = RateLimitMiddleware::new(TenantRateLimit { rpm: 60, burst: 5 });
        for _ in 0..5 {
            assert!(limiter.check("tenant-1").is_ok());
        }
        assert!(limiter.check("tenant-1").is_err());
    }

    #[test]
    fn test_different_tenants_independent() {
        let limiter = RateLimitMiddleware::new(TenantRateLimit { rpm: 60, burst: 2 });
        assert!(limiter.check("tenant-a").is_ok());
        assert!(limiter.check("tenant-a").is_ok());
        assert!(limiter.check("tenant-a").is_err());
        assert!(limiter.check("tenant-b").is_ok());
    }

    #[test]
    fn test_retry_after() {
        let limiter = RateLimitMiddleware::new(TenantRateLimit { rpm: 60, burst: 1 });
        assert!(limiter.check("test").is_ok());
        let err = limiter.check("test").unwrap_err();
        assert!(err >= 1);
    }
}
