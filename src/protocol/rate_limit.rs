//! RateLimitMiddleware — per-tenant rate limiting based on JWT claims
//!
//! GAP-B49-11: Extends PhaseRateLimiter with tenant-level tracking.
//! Returns 429 + Retry-After when exceeded.

// F-GAP-49: Module wired into production protocol pipeline.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::warn;

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

/// Rate limiter state for a single tenant
#[derive(Debug)]
struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    last_access: Instant,
    capacity: f64,
    refill_rate: f64, // tokens per second
}

impl TokenBucket {
    fn new(rpm: u64, burst: u64) -> Self {
        Self {
            tokens: burst as f64,
            last_refill: Instant::now(),
            last_access: Instant::now(),
            capacity: burst as f64,
            refill_rate: rpm as f64 / 60.0,
        }
    }

    fn is_idle(&self, idle_timeout: Duration) -> bool {
        self.last_access.elapsed() >= idle_timeout
    }

    fn refill(&mut self) {
        let elapsed = self.last_refill.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity);
            self.last_refill = Instant::now();
        }
    }

    fn try_consume(&mut self, tokens: f64) -> bool {
        self.refill();
        self.last_access = Instant::now();
        if self.tokens >= tokens {
            self.tokens -= tokens;
            true
        } else {
            false
        }
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

#[allow(dead_code)] // F-GAP-49 — reserved for generic construction
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
    pub fn evict_tenant(&self, tenant_id: &str) {
        let mut buckets = self.buckets.lock().unwrap_or_else(|poisoned| {
            warn!("rate limit evict lock poisoned, recovering");
            poisoned.into_inner()
        });
        buckets.remove(tenant_id);
    }

    /// Check if a request from the given tenant should be allowed.
    /// Returns Ok(()) if allowed, or the number of seconds to wait before retrying.
    pub fn check(&self, tenant_id: &str) -> Result<(), u64> {
        let mut buckets = self.buckets.lock().unwrap_or_else(|poisoned| {
            warn!("rate limit buckets lock poisoned, recovering");
            poisoned.into_inner()
        });

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

        let bucket = buckets
            .entry(tenant_id.to_string())
            .or_insert_with(|| TokenBucket::new(self.default_limit.rpm, self.default_limit.burst));

        if bucket.try_consume(1.0) {
            Ok(())
        } else {
            let retry_after = (1.0 / bucket.refill_rate).ceil() as u64;
            Err(retry_after.max(1))
        }
    }

    /// Get current rate limit state for a tenant
    #[allow(dead_code)] // F-GAP-49 — reserved for observability/metrics integration
    pub fn state(&self, tenant_id: &str) -> RateLimitState {
        let buckets = self.buckets.lock().unwrap_or_else(|poisoned| {
            warn!("rate limit state lock poisoned, recovering");
            poisoned.into_inner()
        });

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
                let mut guard = match buckets.lock() {
                    Ok(g) => g,
                    Err(poisoned) => {
                        warn!("rate limit eviction task lock poisoned, recovering");
                        poisoned.into_inner()
                    }
                };
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

#[allow(dead_code)] // F-GAP-49 — reserved for observability/metrics integration
#[derive(Debug, Clone)]
pub struct RateLimitState {
    pub remaining: u64,
    pub capacity: u64,
    pub refill_per_second: f64,
}

/// Global rate limiter instance
#[allow(dead_code)] // F-GAP-49 — reserved for standalone server usage
static RATE_LIMITER: std::sync::OnceLock<RateLimitMiddleware> = std::sync::OnceLock::new();

#[allow(dead_code)] // F-GAP-49 — reserved for standalone server usage
pub fn rate_limiter() -> &'static RateLimitMiddleware {
    RATE_LIMITER.get_or_init(|| RateLimitMiddleware::new(TenantRateLimit::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_rate_limiting() {
        let limiter = RateLimitMiddleware::new(TenantRateLimit { rpm: 60, burst: 5 });
        // First 5 requests should pass (burst)
        for _ in 0..5 {
            assert!(limiter.check("tenant-1").is_ok());
        }
        // 6th should be rate limited
        assert!(limiter.check("tenant-1").is_err());
    }

    #[test]
    fn test_different_tenants_independent() {
        let limiter = RateLimitMiddleware::new(TenantRateLimit { rpm: 60, burst: 2 });
        assert!(limiter.check("tenant-a").is_ok());
        assert!(limiter.check("tenant-a").is_ok());
        assert!(limiter.check("tenant-a").is_err());
        // Different tenant is not affected
        assert!(limiter.check("tenant-b").is_ok());
    }

    #[test]
    fn test_retry_after() {
        let limiter = RateLimitMiddleware::new(TenantRateLimit {
            rpm: 60, // 1 per second
            burst: 1,
        });
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
