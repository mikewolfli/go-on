//! Global rate limiter — token bucket per-tenant + global max concurrent.
//!
//! Provides two layers of rate limiting:
//! 1. Per-tenant token bucket (sliding window, configurable rate)
//! 2. Global max concurrent requests (semaphore-based)

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Instant;

/// Rate limiter configuration.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Max requests per second per tenant (token bucket refill rate).
    pub tenant_rps: f64,
    /// Max burst size per tenant.
    pub tenant_burst: u32,
    /// Global max concurrent requests.
    pub global_max_concurrent: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            tenant_rps: 100.0,
            tenant_burst: 50,
            global_max_concurrent: 1000,
        }
    }
}

struct TenantBucket {
    tokens: f64,
    last_refill: Instant,
    capacity: f64,
    refill_rate: f64,
}

impl TenantBucket {
    fn new(capacity: f64, refill_rate: f64) -> Self {
        Self {
            tokens: capacity,
            last_refill: Instant::now(),
            capacity,
            refill_rate,
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

    fn refill(&mut self) {
        let elapsed = self.last_refill.elapsed().as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity);
        self.last_refill = Instant::now();
    }
}

/// Global rate limiter instance.
pub struct GlobalRateLimiter {
    /// Maximum number of concurrent requests allowed globally.
    pub global_max: usize,
    config: RateLimitConfig,
    tenants: Mutex<HashMap<String, TenantBucket>>,
    /// Semaphore for global max concurrent requests.
    pub global_semaphore: tokio::sync::Semaphore,
}

impl GlobalRateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        let global_max = config.global_max_concurrent;
        let semaphore = tokio::sync::Semaphore::new(config.global_max_concurrent);
        Self {
            global_max,
            config,
            tenants: Mutex::new(HashMap::new()),
            global_semaphore: semaphore,
        }
    }

    /// Try to consume a token for the given tenant.
    /// Returns true if allowed, false if rate limited.
    pub fn try_consume_tenant(&self, tenant_id: &str, tokens: f64) -> bool {
        let mut tenants = self.tenants.lock().unwrap();
        let bucket = tenants.entry(tenant_id.to_string()).or_insert_with(|| {
            TenantBucket::new(self.config.tenant_burst as f64, self.config.tenant_rps)
        });
        bucket.try_consume(tokens)
    }

    pub fn config(&self) -> &RateLimitConfig {
        &self.config
    }
}

static GLOBAL_RATE_LIMITER: OnceLock<GlobalRateLimiter> = OnceLock::new();

pub fn global_rate_limiter() -> &'static GlobalRateLimiter {
    GLOBAL_RATE_LIMITER.get_or_init(|| GlobalRateLimiter::new(RateLimitConfig::default()))
}

pub fn init_rate_limiter(config: RateLimitConfig) {
    let _ = GLOBAL_RATE_LIMITER.set(GlobalRateLimiter::new(config));
}
