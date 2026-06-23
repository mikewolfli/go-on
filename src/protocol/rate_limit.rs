//! RateLimitMiddleware — per-tenant rate limiting based on JWT claims
//!
//! GAP-B49-11: Extends PhaseRateLimiter with tenant-level tracking.
//! Returns 429 + Retry-After when exceeded.

// F-GAP-49: Module wired into production protocol pipeline.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::warn;

use crate::shared::token_bucket::TokenBucket;

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
#[derive(Debug)]
pub struct RateLimitMiddleware {
    buckets: Arc<Mutex<HashMap<String, TokenBucket>>>,
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
            buckets: Arc::new(Mutex::new(HashMap::new())),
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
    fn lazy_evict(&self, buckets: &mut HashMap<String, TokenBucket>) {
        if buckets.len() > self.max_tenants {
            buckets.retain(|_, bucket| !bucket.is_idle(self.idle_timeout));
        }
    }

    /// Evict a specific tenant from the rate limiter.
    pub async fn evict_tenant(&self, tenant_id: &str) {
        let mut buckets = self.buckets.lock().await;
        buckets.remove(tenant_id);
    }

    /// Check if a request from the given tenant should be allowed.
    /// Returns Ok(()) if allowed, or the number of seconds to wait before retrying.
    pub async fn check(&self, tenant_id: &str) -> Result<(), u64> {
        let mut buckets = self.buckets.lock().await;

        // Lazy eviction: only evict when we're at capacity to keep common path fast.
        self.lazy_evict(&mut buckets);

        // Enforce max_tenants: reject new tenants when at capacity.
        // Existing tenants can always proceed.
        if !buckets.contains_key(tenant_id) && buckets.len() >= self.max_tenants {
            warn!(
                "rate limit tenant limit reached (max={}), rejecting tenant '{}'",
                self.max_tenants, tenant_id
            );
            return Err(60); // 60 second backoff hint
        }

        let bucket = buckets.entry(tenant_id.to_string()).or_insert_with(|| {
            TokenBucket::new(
                self.default_limit.burst as f64,
                self.default_limit.rpm as f64 / 60.0,
            )
        });

        if bucket.try_consume(1.0) {
            Ok(())
        } else {
            let retry_after = (1.0 / bucket.refill_rate).ceil() as u64;
            Err(retry_after.max(1))
        }
    }

    /// Get current rate limit state for a tenant
    pub async fn state(&self, tenant_id: &str) -> RateLimitState {
        let buckets = self.buckets.lock().await;

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
    }

    /// Compute the `Retry-After` header value (in seconds) for the given tenant.
    ///
    /// Returns the number of seconds the client should wait before making
    /// another request.  Useful for constructing a 429 response with the
    /// `Retry-After` HTTP header.
    pub async fn retry_after(&self, tenant_id: &str) -> u64 {
        let buckets = self.buckets.lock().await;

        if let Some(bucket) = buckets.get(tenant_id) {
            let wait_ms = bucket.wait_time_ms();
            (wait_ms / 1000).max(1)
        } else {
            0 // No rate limit state yet for this tenant
        }
    }

    /// Start a background task that periodically evicts idle tenant entries.
    /// This provides a TTL-based background eviction cycle, reducing the need
    /// for per-request lazy eviction checks.
    ///
    /// The task runs every `check_interval` and can be aborted via the returned
    /// `tokio::task::JoinHandle`. Safe to call multiple times — each call spawns
    /// a separate eviction task.
    pub fn start_background_eviction(
        &self,
        check_interval: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let buckets = Arc::clone(&self.buckets);
        let idle_timeout = self.idle_timeout;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(check_interval).await;
                let mut guard = buckets.lock().await;
                let before = guard.len();
                guard.retain(|_, bucket| !bucket.is_idle(idle_timeout));
                let after = guard.len();
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

    #[tokio::test]
    async fn test_basic_rate_limiting() {
        let limiter = RateLimitMiddleware::new(TenantRateLimit { rpm: 60, burst: 5 });
        // First 5 requests should pass (burst)
        for _ in 0..5 {
            assert!(limiter.check("tenant-1").await.is_ok());
        }
        // 6th should be rate limited
        assert!(limiter.check("tenant-1").await.is_err());
    }

    #[tokio::test]
    async fn test_different_tenants_independent() {
        let limiter = RateLimitMiddleware::new(TenantRateLimit { rpm: 60, burst: 2 });
        assert!(limiter.check("tenant-a").await.is_ok());
        assert!(limiter.check("tenant-a").await.is_ok());
        assert!(limiter.check("tenant-a").await.is_err());
        // Different tenant is not affected
        assert!(limiter.check("tenant-b").await.is_ok());
    }

    #[tokio::test]
    async fn test_retry_after() {
        let limiter = RateLimitMiddleware::new(TenantRateLimit {
            rpm: 60, // 1 per second
            burst: 1,
        });
        assert!(limiter.check("test").await.is_ok());
        let err = limiter.check("test").await.unwrap_err();
        assert!(err >= 1);
    }

    #[tokio::test]
    async fn test_state_reporting() {
        let limiter = RateLimitMiddleware::new(TenantRateLimit {
            rpm: 120,
            burst: 10,
        });
        let state = limiter.state("test").await;
        assert_eq!(state.capacity, 10);
        assert!((state.refill_per_second - 2.0).abs() < 0.001);
    }
}
