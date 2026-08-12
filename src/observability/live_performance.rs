//! LivePerformanceFeed — EMA-smoothed real-time model performance tracking.
//!
//! Maintains exponentially-weighted moving averages for latency, success rate,
//! and request counts per model.  Used by the orchestrator to provide dynamic
//! cost and latency estimates that adapt to observed behaviour.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::debug;

/// Process-global live-performance feed shared by the orchestrator and the
/// capability bus. All readers (model cost/latency estimation in
/// `select_model_for_task`, `decide`) and writers (`fallback.rs` outcome
/// recording) observe the same instance, so dynamic estimates reflect real
/// observed behavior.
pub fn global_live_performance() -> &'static Arc<LivePerformanceFeed> {
    static GLOBAL: OnceLock<Arc<LivePerformanceFeed>> = OnceLock::new();
    GLOBAL.get_or_init(|| Arc::new(LivePerformanceFeed::new(0.3)))
}

/// Inner state wrapped in a single Mutex.
struct LivePerformanceInner {
    /// EMA-smoothed latency per model (ms).
    model_latency: HashMap<String, f64>,
    /// EMA-smoothed success rate per model (0.0–1.0).
    model_success_rate: HashMap<String, f64>,
    /// Total request count per model.
    model_requests: HashMap<String, u64>,
}

/// EMA-smoothed live performance feed for model monitoring.
pub struct LivePerformanceFeed {
    /// Inner state protected by a single mutex.
    inner: Mutex<LivePerformanceInner>,
    /// Exponential smoothing factor α (higher = more weight on recent).
    pub ema_alpha: f64,
}

impl LivePerformanceFeed {
    /// Create a new feed with the given EMA smoothing factor.
    ///
    /// Typical values: 0.1 (slow-moving), 0.3 (moderate), 0.5 (responsive).
    pub fn new(ema_alpha: f64) -> Self {
        Self {
            inner: Mutex::new(LivePerformanceInner {
                model_latency: HashMap::new(),
                model_success_rate: HashMap::new(),
                model_requests: HashMap::new(),
            }),
            ema_alpha,
        }
    }

    /// Maximum number of distinct models tracked (model names come from
    /// request options and could be arbitrary strings — unbounded tracking
    /// would grow memory forever in a long-running process).
    const MAX_MODELS_TRACKED: usize = 256;

    /// Whether a new model key may be inserted (bounded by the cap).
    fn can_track(&self, inner: &LivePerformanceInner) -> bool {
        inner.model_latency.len() < Self::MAX_MODELS_TRACKED
    }

    /// Record a successful request for `model` with observed latency.
    pub fn record_success(&self, model: &str, latency_ms: u64) {
        let alpha = self.ema_alpha;
        let mut inner = crate::observability::lock_mutex(&self.inner);

        // Unknown models beyond the cap are not tracked (bounded memory).
        if !inner.model_latency.contains_key(model) && !self.can_track(&inner) {
            return;
        }

        // Update latency EMA.
        let entry = inner
            .model_latency
            .entry(model.to_string())
            .or_insert(latency_ms as f64);
        *entry = alpha * (latency_ms as f64) + (1.0 - alpha) * *entry;

        // Update success-rate EMA.
        let entry = inner
            .model_success_rate
            .entry(model.to_string())
            .or_insert(1.0);
        *entry = alpha * 1.0 + (1.0 - alpha) * *entry;

        // Bump request count.
        *inner.model_requests.entry(model.to_string()).or_insert(0) += 1;

        debug!(
            model = %model,
            latency_ms,
            "LivePerformanceFeed: recorded success"
        );
    }

    /// Record a failed request for `model` with observed latency.
    pub fn record_failure(&self, model: &str, latency_ms: u64) {
        let alpha = self.ema_alpha;
        let mut inner = crate::observability::lock_mutex(&self.inner);

        // Unknown models beyond the cap are not tracked (bounded memory).
        if !inner.model_latency.contains_key(model) && !self.can_track(&inner) {
            return;
        }

        // Update latency EMA (still useful for detecting slow-failing models).
        let entry = inner
            .model_latency
            .entry(model.to_string())
            .or_insert(latency_ms as f64);
        *entry = alpha * (latency_ms as f64) + (1.0 - alpha) * *entry;

        // Update success-rate EMA (penalise).
        let entry = inner
            .model_success_rate
            .entry(model.to_string())
            .or_insert(1.0);
        *entry = alpha * 0.0 + (1.0 - alpha) * *entry;

        // Bump request count.
        *inner.model_requests.entry(model.to_string()).or_insert(0) += 1;

        debug!(
            model = %model,
            latency_ms,
            "LivePerformanceFeed: recorded failure"
        );
    }

    /// Estimate cost-per-request in cents based on observed latency and
    /// success rate.  Cheaper if the model is fast AND reliable.
    pub fn get_cost_estimate(&self, model: &str) -> Option<f64> {
        let inner = crate::observability::lock_mutex(&self.inner);

        let latency = inner.model_latency.get(model)?;
        let success = inner.model_success_rate.get(model).copied().unwrap_or(1.0);

        // Cost ∝ latency / success_rate: faster + reliable → cheaper.
        Some(*latency / success.max(0.01))
    }

    /// Get the EMA-smoothed latency estimate (ms) for a model.
    pub fn get_latency_estimate(&self, model: &str) -> Option<f64> {
        let inner = crate::observability::lock_mutex(&self.inner);
        inner.model_latency.get(model).copied()
    }

    /// Get the EMA-smoothed success rate (0.0–1.0) for a model.
    pub fn get_success_rate(&self, model: &str) -> Option<f64> {
        let inner = crate::observability::lock_mutex(&self.inner);
        inner.model_success_rate.get(model).copied()
    }

    /// Get the total request count for a model.
    pub fn get_request_count(&self, model: &str) -> u64 {
        let inner = crate::observability::lock_mutex(&self.inner);
        inner.model_requests.get(model).copied().unwrap_or(0)
    }
}

impl Default for LivePerformanceFeed {
    fn default() -> Self {
        Self::new(0.3)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_success_updates_all_stats() {
        let feed = LivePerformanceFeed::new(0.5);

        feed.record_success("model-a", 100);
        feed.record_success("model-a", 200);

        let latency = feed.get_latency_estimate("model-a").unwrap();
        let success = feed.get_success_rate("model-a").unwrap();
        let count = feed.get_request_count("model-a");

        // After two successes with α=0.5:
        // Latency: 0.5*200 + 0.5*(0.5*100 + 0.5*100) = 100 + 50 = 150
        assert!((latency - 150.0).abs() < 1.0, "latency={}", latency);
        // Success: 0.5*1.0 + 0.5*(0.5*1.0 + 0.5*1.0) = 0.5 + 0.5 = 1.0
        assert!((success - 1.0).abs() < 0.01);
        assert_eq!(count, 2);
    }

    #[test]
    fn record_failure_punishes_success_rate() {
        let feed = LivePerformanceFeed::new(0.5);

        feed.record_success("model-b", 50);
        feed.record_failure("model-b", 50);

        let success = feed.get_success_rate("model-b").unwrap();
        // After success then failure with α=0.5:
        // Success: 0.5*0.0 + 0.5*1.0 = 0.5
        assert!((success - 0.5).abs() < 0.01);
        assert_eq!(feed.get_request_count("model-b"), 2);
    }

    #[test]
    fn unobserved_model_returns_none() {
        let feed = LivePerformanceFeed::default();
        assert!(feed.get_latency_estimate("unknown").is_none());
        assert!(feed.get_success_rate("unknown").is_none());
    }

    #[test]
    fn cost_estimate_reflects_latency_and_reliability() {
        let feed = LivePerformanceFeed::new(1.0); // α=1 → no smoothing

        feed.record_success("fast-reliable", 100);
        feed.record_failure("slow-flaky", 2000);

        // fast-reliable: cost ≈ 100 / 1.0 = 100
        let cost_fast = feed.get_cost_estimate("fast-reliable").unwrap();
        assert!((cost_fast - 100.0).abs() < 1.0);

        // slow-flaky: cost ≈ 2000 / 0.0... capped at 0.01 → 200000
        let cost_slow = feed.get_cost_estimate("slow-flaky").unwrap();
        assert!(cost_slow > 500.0, "cost_slow={}", cost_slow);
    }

    #[test]
    fn ema_smoothing_dampens_changes() {
        // Low alpha → more smoothing.
        let feed = LivePerformanceFeed::new(0.1);

        feed.record_success("model-c", 1000);
        feed.record_success("model-c", 100);

        // Latency after two: 0.1*100 + 0.9*(0.1*1000 + 0.9*1000) = 10 + 900 = 910
        let latency = feed.get_latency_estimate("model-c").unwrap();
        assert!(latency > 500.0, "latency={}", latency); // Still close to 1000
    }

    #[test]
    fn global_feed_is_shared_singleton() {
        // The global feed must be the same instance for every caller so that
        // fallback outcome recording and model estimation observe one dataset.
        let a = crate::observability::live_performance::global_live_performance();
        let b = crate::observability::live_performance::global_live_performance();
        assert!(std::ptr::eq(a.as_ref(), b.as_ref()));

        // Writing through one handle is visible through the other.
        a.record_success("global-model", 42);
        assert_eq!(b.get_request_count("global-model"), 1);
    }
}
