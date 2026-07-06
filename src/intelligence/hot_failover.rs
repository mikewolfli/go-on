//! HotFailover — transparent model failover with timeouts and cooldown.
//!
//! When a primary model times out or returns an error, `HotFailover`
//! immediately retries with fallback agents in capability order, while
//! blacklisting failed models for a configurable cooldown period.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the hot-failover subsystem.
#[derive(Clone)]
pub struct HotFailoverConfig {
    /// Whether failover is enabled.
    pub enabled: bool,
    /// Primary-model timeout in milliseconds before triggering failover.
    pub timeout_ms: u64,
    /// Maximum number of models to try (including primary).
    pub max_failover_attempts: u32,
    /// Duration (ms) a failed model stays on the cooldown blacklist.
    pub cooldown_ms: u64,
}

impl Default for HotFailoverConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_ms: 5000,
            max_failover_attempts: 3,
            cooldown_ms: 30000,
        }
    }
}

/// Default maximum number of blacklisted models kept in the failed_models map.
const DEFAULT_MAX_FAILED_MODELS: usize = 1000;

// ---------------------------------------------------------------------------
// Failover metrics
// ---------------------------------------------------------------------------

/// Metrics tracked during failover operations.
#[derive(Debug, Clone, Default)]
pub struct FailoverMetrics {
    /// Total failover events triggered.
    pub failover_count: u64,
    /// Total models skipped due to cooldown.
    pub cooldown_skips: u64,
    /// Cumulative extra latency added by failovers (ms).
    pub total_failover_latency_ms: u64,
}

// ---------------------------------------------------------------------------
// HotFailover
// ---------------------------------------------------------------------------

/// Transparent model failover with cooldown-based blacklisting.
pub struct HotFailover {
    config: HotFailoverConfig,
    /// Maps model ID to the Instant when its cooldown expires.
    failed_models: HashMap<String, Instant>,
    /// Maximum number of entries in the failed_models map before evicting oldest.
    max_failed_models: usize,
    /// Cumulative failover metrics.
    metrics: FailoverMetrics,
}

impl HotFailover {
    /// Create a new `HotFailover` with the given configuration.
    pub fn new(config: HotFailoverConfig) -> Self {
        Self {
            config,
            failed_models: HashMap::new(),
            max_failed_models: DEFAULT_MAX_FAILED_MODELS,
            metrics: FailoverMetrics::default(),
        }
    }

    /// Whether the given model is currently blacklisted (in cooldown).
    ///
    /// Note: expired entries are not eagerly cleaned here — they are removed
    /// the next time `record_failure` or `execute_with_failover` is called.
    pub fn is_blacklisted(&self, model_id: &str) -> bool {
        match self.failed_models.get(model_id) {
            Some(expiry) => Instant::now() < *expiry,
            None => false,
        }
    }

    /// Mark a model as failed, placing it into cooldown.
    pub fn record_failure(&mut self, model_id: &str) {
        self.evict_expired_entries();
        let cooldown = Duration::from_millis(self.config.cooldown_ms);
        let expiry = Instant::now() + cooldown;

        // Evict the oldest entry if at capacity to prevent unbounded growth.
        if self.failed_models.len() >= self.max_failed_models
            && !self.failed_models.contains_key(model_id)
        {
            if let Some(oldest_key) = self.failed_models.keys().next().cloned() {
                self.failed_models.remove(&oldest_key);
                tracing::warn!(
                    "failed_models cap reached ({}): evicted oldest entry",
                    self.max_failed_models,
                );
            }
        }

        self.failed_models.insert(model_id.to_string(), expiry);
        warn!(
            model = %model_id,
            cooldown_ms = self.config.cooldown_ms,
            "HotFailover: model blacklisted for cooldown"
        );
    }

    /// Remove entries whose cooldown has already expired.
    fn evict_expired_entries(&mut self) {
        let now = Instant::now();
        self.failed_models.retain(|_, expiry| now < *expiry);
    }

    /// Execute a task with failover across a sequence of model functions.
    ///
    /// `attempts` is a slice of `(model_id, async_fn)` pairs.  The first
    /// attempt is the primary model; subsequent entries are fallbacks.
    /// Each async function receives the model ID and returns a result.
    ///
    /// Returns the first successful result, or an error if all models fail.
    pub async fn execute_with_failover<F, Fut, T, E>(
        &mut self,
        prompt: &str,
        attempts: &[(String, F)],
    ) -> Result<T, E>
    where
        F: Fn(String) -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
        E: std::fmt::Debug + Default,
    {
        if !self.config.enabled {
            tracing::info!(%prompt, "HotFailover: disabled — using primary only");
            if let Some((model_id, f)) = attempts.first() {
                return f(model_id.clone()).await;
            }
            return Err(E::default());
        }
        if attempts.is_empty() {
            tracing::error!(%prompt, "HotFailover: no models provided — cannot execute");
            return Err(E::default());
        }

        let max_attempts = (self.config.max_failover_attempts as usize).min(attempts.len());
        let timeout = Duration::from_millis(self.config.timeout_ms);

        let failover_start = Instant::now();
        let mut last_error: Option<E> = None;
        // GAP-B58-B16: Track which models failed so the error is informative.
        let mut failed_models: Vec<String> = Vec::new();

        for (i, (model_id, f)) in attempts.iter().enumerate().take(max_attempts) {
            // Skip blacklisted models.
            if self.is_blacklisted(model_id) {
                self.metrics.cooldown_skips += 1;
                info!(
                    model = %model_id,
                    %prompt,
                    "HotFailover: skipping blacklisted model"
                );
                continue;
            }

            let attempt_start = Instant::now();
            let future = f(model_id.clone());

            match tokio::time::timeout(timeout, future).await {
                Ok(Ok(result)) => {
                    // Success — no failover latency recorded for primary.
                    if i > 0 {
                        self.metrics.failover_count += 1;
                        let latency = attempt_start.duration_since(failover_start);
                        self.metrics.total_failover_latency_ms += latency.as_millis() as u64;
                    }
                    return Ok(result);
                }
                Ok(Err(e)) => {
                    warn!(
                        model = %model_id,
                        attempt = i + 1,
                        %prompt,
                        ?e,
                        "HotFailover: model returned error"
                    );
                    self.record_failure(model_id);
                    failed_models.push(model_id.clone());
                    last_error = Some(e);
                }
                Err(_elapsed) => {
                    warn!(
                        model = %model_id,
                        attempt = i + 1,
                        %prompt,
                        timeout_ms = self.config.timeout_ms,
                        "HotFailover: model timed out"
                    );
                    self.record_failure(model_id);
                    failed_models.push(model_id.clone());
                }
            }
        }

        self.metrics.failover_count += 1;
        let latency = Instant::now().duration_since(failover_start);
        self.metrics.total_failover_latency_ms += latency.as_millis() as u64;

        // GAP-B58-B16: Log all models that failed before returning the error.
        if !failed_models.is_empty() {
            tracing::error!(
                %prompt,
                failed_models = ?failed_models,
                "HotFailover: all {} model(s) exhausted",
                failed_models.len(),
            );
        }

        // All models exhausted — return the last error or a default error.
        match last_error {
            Some(e) => Err(e),
            None => Err(E::default()),
        }
    }

    /// Return a snapshot of current failover metrics.
    pub fn metrics(&self) -> FailoverMetrics {
        self.metrics.clone()
    }

    /// Clear the cooldown blacklist (useful after a configuration change).
    pub fn clear_blacklist(&mut self) {
        self.failed_models.clear();
    }
}

/// Global singleton HotFailover instance, shared across requests.
///
/// Constructed lazily at first access using the default configuration.
/// Use this when you need a single failover tracker per process without
/// explicitly plumbing a `&HotFailover` reference through your call chain.
/// Wired into governance status profile for observability.
pub static HOT_FAILOVER_INSTANCE: LazyLock<Mutex<HotFailover>> =
    LazyLock::new(|| Mutex::new(HotFailover::new(HotFailoverConfig::default())));

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn primary_succeeds_no_failover() {
        let config = HotFailoverConfig::default();
        let mut hf = HotFailover::new(config);

        let attempts = vec![("primary".to_string(), |id: String| async move {
            assert_eq!(id, "primary");
            Ok::<_, String>(42)
        })];

        let result = hf.execute_with_failover("test", &attempts).await;
        assert_eq!(result, Ok(42));
        let metrics = hf.metrics();
        assert_eq!(metrics.failover_count, 0);
    }

    #[tokio::test]
    async fn fallback_on_primary_error() {
        let config = HotFailoverConfig {
            timeout_ms: 1000,
            max_failover_attempts: 2,
            ..HotFailoverConfig::default()
        };
        let mut hf = HotFailover::new(config);

        let call_count = std::sync::Arc::new(AtomicU32::new(0));

        // Use a trait-object vec to avoid closure-type mismatch.
        type AttemptFn<T, E> = Box<
            dyn Fn(
                    String,
                )
                    -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, E>> + Send>>
                + Send
                + Sync,
        >;
        let cc1 = call_count.clone();
        let cc2 = call_count.clone();
        let attempts: Vec<(String, AttemptFn<i32, String>)> = vec![
            (
                "primary".to_string(),
                Box::new(move |_id: String| {
                    let cc = cc1.clone();
                    Box::pin(async move {
                        cc.fetch_add(1, Ordering::SeqCst);
                        Err("primary error".to_string())
                    })
                }),
            ),
            (
                "fallback".to_string(),
                Box::new(move |_id: String| {
                    let cc = cc2.clone();
                    Box::pin(async move {
                        cc.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, String>(99)
                    })
                }),
            ),
        ];

        let result = hf.execute_with_failover("test", &attempts).await;
        assert_eq!(result, Ok(99));
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
        let metrics = hf.metrics();
        assert!(metrics.failover_count >= 1);
        assert!(hf.is_blacklisted("primary"));
    }

    #[tokio::test]
    async fn skips_blacklisted_models() {
        let config = HotFailoverConfig {
            cooldown_ms: 60000,
            max_failover_attempts: 3,
            ..HotFailoverConfig::default()
        };
        let mut hf = HotFailover::new(config);

        // Pre-blacklist the "bad" model.
        hf.record_failure("bad");

        let attempt = |id: String| -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<&str, String>> + Send>,
        > {
            Box::pin(async move {
                match id.as_str() {
                    "bad" => panic!("should not be called"),
                    _ => Ok("ok"),
                }
            })
        };

        let attempts = vec![("bad".to_string(), attempt), ("good".to_string(), attempt)];

        let result = hf.execute_with_failover("test", &attempts).await;
        assert!(result.is_ok());
        let metrics = hf.metrics();
        assert_eq!(metrics.cooldown_skips, 1);
    }

    #[tokio::test]
    async fn timeout_triggers_failover() {
        let config = HotFailoverConfig {
            timeout_ms: 50,
            max_failover_attempts: 3,
            ..HotFailoverConfig::default()
        };
        let mut hf = HotFailover::new(config);

        let attempt = |id: String| -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<&str, String>> + Send>,
        > {
            Box::pin(async move {
                match id.as_str() {
                    "slow" => {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        Ok("too late")
                    }
                    _ => Ok("fast"),
                }
            })
        };

        let attempts = vec![("slow".to_string(), attempt), ("fast".to_string(), attempt)];

        let result = hf.execute_with_failover("test", &attempts).await;
        assert!(result.is_ok());
        assert!(hf.is_blacklisted("slow"));
    }

    #[tokio::test]
    async fn disabled_mode_uses_only_primary() {
        let config = HotFailoverConfig {
            enabled: false,
            ..HotFailoverConfig::default()
        };
        let mut hf = HotFailover::new(config);

        let attempts = vec![("only".to_string(), |_id: String| async move {
            Ok::<_, String>("only".to_string())
        })];

        let result = hf.execute_with_failover("test", &attempts).await;
        assert_eq!(result, Ok("only".to_string()));
        let metrics = hf.metrics();
        assert_eq!(metrics.failover_count, 0);
    }

    #[tokio::test]
    async fn clear_blacklist_removes_all_entries() {
        let config = HotFailoverConfig::default();
        let mut hf = HotFailover::new(config);

        hf.record_failure("model-a");
        hf.record_failure("model-b");
        assert!(hf.is_blacklisted("model-a"));
        assert!(hf.is_blacklisted("model-b"));

        hf.clear_blacklist();
        assert!(!hf.is_blacklisted("model-a"));
        assert!(!hf.is_blacklisted("model-b"));
    }
}
