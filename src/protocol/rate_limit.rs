//! RateLimitMiddleware — per-tenant rate limiting based on JWT claims
//!
//! GAP-B49-11: Extends PhaseRateLimiter with tenant-level tracking.
//! Returns 429 + Retry-After when exceeded.

// F-GAP-49: Module not yet wired into production protocol pipeline.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;
use tracing::warn;

/// Rate limit configuration for a tenant
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TenantRateLimit {
    /// Maximum requests per minute
    pub rpm: u64,
    /// Burst capacity
    pub burst: u64,
}

#[allow(dead_code)]
impl Default for TenantRateLimit {
    fn default() -> Self {
        Self { rpm: 60, burst: 10 }
    }
}

/// Rate limiter state for a single tenant
#[allow(dead_code)]
#[derive(Debug)]
struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    capacity: f64,
    refill_rate: f64, // tokens per second
}

#[allow(dead_code)]
impl TokenBucket {
    fn new(rpm: u64, burst: u64) -> Self {
        Self {
            tokens: burst as f64,
            last_refill: Instant::now(),
            capacity: burst as f64,
            refill_rate: rpm as f64 / 60.0,
        }
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
        if self.tokens >= tokens {
            self.tokens -= tokens;
            true
        } else {
            false
        }
    }
}

/// Rate limit middleware
#[allow(dead_code)]
#[derive(Debug)]
pub struct RateLimitMiddleware {
    buckets: Mutex<HashMap<String, TokenBucket>>,
    default_limit: TenantRateLimit,
}

#[allow(dead_code)]
impl Default for RateLimitMiddleware {
    fn default() -> Self {
        Self::new(TenantRateLimit::default())
    }
}

#[allow(dead_code)]
impl RateLimitMiddleware {
    pub fn new(default_limit: TenantRateLimit) -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            default_limit,
        }
    }

    /// Check if a request from the given tenant should be allowed.
    /// Returns Ok(()) if allowed, or the number of seconds to wait before retrying.
    pub fn check(&self, tenant_id: &str) -> Result<(), u64> {
        let mut buckets = self.buckets.lock().unwrap_or_else(|poisoned| {
            warn!("rate limit buckets lock poisoned, recovering");
            poisoned.into_inner()
        });

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
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct RateLimitState {
    pub remaining: u64,
    pub capacity: u64,
    pub refill_per_second: f64,
}

/// Global rate limiter instance
#[allow(dead_code)]
static RATE_LIMITER: std::sync::OnceLock<RateLimitMiddleware> = std::sync::OnceLock::new();

#[allow(dead_code)]
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
