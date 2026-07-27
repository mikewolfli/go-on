//! RateLimitMiddleware — per-tenant rate limiting based on JWT claims
//!
//! GAP-B49-11: Extends PhaseRateLimiter with tenant-level tracking.
//! Returns 429 + Retry-After when exceeded.

// F-GAP-49: Module wired into production protocol pipeline.

use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

use crate::shared::token_bucket::{BucketMap, TokenBucket};

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

    /// Check if a request from the given tenant should be allowed.
    /// Returns Ok(()) if allowed, or the number of seconds to wait before retrying.
    pub fn check(&self, tenant_id: &str) -> Result<(), u64> {
        self.lazy_evict();

        let should_reject = self.buckets.with_lock(|buckets| {
            if !buckets.contains_key(tenant_id) && buckets.len() >= self.max_tenants {
                warn!(
                    "rate limit tenant limit reached (max={}), rejecting tenant '{}'",
                    self.max_tenants, tenant_id
                );
                return true;
            }
            false
        });

        if should_reject {
            return Err(60);
        }

        let burst = self.default_limit.burst as f64;
        let refill_rate = self.default_limit.rpm as f64 / 60.0;

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

    /// Get current rate limit state for a tenant
    pub fn state(&self, tenant_id: &str) -> RateLimitState {
        self.buckets.with_lock(|buckets| {
            if let Some(bucket) = buckets.get(tenant_id) {
                RateLimitState {
                    remaining: bucket.tokens as u64,
                    capacity: bucket.capacity as u64,
                    refill_per_second: bucket.refill_rate,
                }
            } else {
                RateLimitState {
                    remaining: self.default_limit.burst,
                    capacity: self.default_limit.burst,
                    refill_per_second: self.default_limit.rpm as f64 / 60.0,
                }
            }
        })
    }

    /// Compute the Retry-After header value in seconds for the given tenant.
    pub fn retry_after(&self, tenant_id: &str) -> u64 {
        self.buckets.with_lock(|buckets| {
            if let Some(bucket) = buckets.get(tenant_id) {
                let wait_ms = bucket.wait_time_ms();
                (wait_ms / 1000).max(1)
            } else {
                0
            }
        })
    }

    /// Start a background eviction task for idle tenants.
    pub fn start_background_eviction(
        &self,
        check_interval: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let buckets = Arc::clone(&self.buckets);
        let idle_timeout = self.idle_timeout;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(check_interval).await;
                let before = buckets.len();
                buckets.retain(|_, bucket| !bucket.is_idle(idle_timeout));
                let after = buckets.len();
                if before != after {
                    warn!(
                        "rate limit eviction: removed {} idle tenants ({} remaining)",
                        before - after,
                        after
                    );
                }
            }
        })
    }

    /// Start background eviction with a default 5-minute check interval.
    pub fn start_background_eviction_default(&self) -> tokio::task::JoinHandle<()> {
        self.start_background_eviction(Duration::from_secs(300))
    }

    /// Try to consume tokens for a tenant (sync, no async required).
    /// Looks up or creates a token bucket and attempts to consume tokens.
    pub fn try_consume_tenant(&self, tenant_id: &str, tokens: f64) -> bool {
        let burst = self.default_limit.burst as f64;
        let refill_rate = self.default_limit.rpm as f64;
        // For multi-token consumption we bypass the single-token try_consume
        // and operate directly on the bucket.
        self.buckets.with_lock(|buckets| {
            let bucket = buckets
                .entry(tenant_id.to_string())
                .or_insert_with(|| TokenBucket::new(burst, refill_rate));
            bucket.try_consume(tokens)
        })
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitState {
    pub remaining: u64,
    pub capacity: u64,
    pub refill_per_second: f64,
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

    #[test]
    fn test_state_reporting() {
        let limiter = RateLimitMiddleware::new(TenantRateLimit {
            rpm: 120,
            burst: 10,
        });
        let state = limiter.state("test");
        assert_eq!(state.capacity, 10);
        assert!((state.refill_per_second - 2.0).abs() < 0.001);
    }
}
