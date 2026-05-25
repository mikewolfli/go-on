//! LivePerformanceFeed — EMA-smoothed real-time model performance tracking.
//!
//! Maintains exponentially-weighted moving averages for latency, success rate,
//! and request counts per model.  Used by the orchestrator to provide dynamic
//! cost and latency estimates that adapt to observed behaviour.

use std::collections::HashMap;
use std::sync::Mutex;

use tracing::debug;

/// EMA-smoothed live performance feed for model monitoring.
pub struct LivePerformanceFeed {
    /// EMA-smoothed latency per model (ms).
    pub model_latency: Mutex<HashMap<String, f64>>,
    /// EMA-smoothed success rate per model (0.0–1.0).
    pub model_success_rate: Mutex<HashMap<String, f64>>,
    /// Total request count per model.
    pub model_requests: Mutex<HashMap<String, u64>>,
    /// Exponential smoothing factor α (higher = more weight on recent).
    pub ema_alpha: f64,
}

impl LivePerformanceFeed {
    /// Create a new feed with the given EMA smoothing factor.
    ///
    /// Typical values: 0.1 (slow-moving), 0.3 (moderate), 0.5 (responsive).
    pub fn new(ema_alpha: f64) -> Self {
        Self {
            model_latency: Mutex::new(HashMap::new()),
            model_success_rate: Mutex::new(HashMap::new()),
            model_requests: Mutex::new(HashMap::new()),
            ema_alpha,
        }
    }

    /// Record a successful request for `model` with observed latency.
    pub fn record_success(&self, model: &str, latency_ms: u64) {
        let alpha = self.ema_alpha;

        // Update latency EMA.
        {
            let mut lat = crate::observability::lock_mutex(&self.model_latency);
            let entry = lat.entry(model.to_string()).or_insert(latency_ms as f64);
            *entry = alpha * (latency_ms as f64) + (1.0 - alpha) * *entry;
        }

        // Update success-rate EMA.
        {
            let mut sr = crate::observability::lock_mutex(&self.model_success_rate);
            let entry = sr.entry(model.to_string()).or_insert(1.0);
            *entry = alpha * 1.0 + (1.0 - alpha) * *entry;
        }

        // Bump request count.
        {
            let mut req = crate::observability::lock_mutex(&self.model_requests);
            *req.entry(model.to_string()).or_insert(0) += 1;
        }

        debug!(
            model = %model,
            latency_ms,
            "LivePerformanceFeed: recorded success"
        );
    }

    /// Record a failed request for `model` with observed latency.
    pub fn record_failure(&self, model: &str, latency_ms: u64) {
        let alpha = self.ema_alpha;

        // Update latency EMA (still useful for detecting slow-failing models).
        {
            let mut lat = crate::observability::lock_mutex(&self.model_latency);
            let entry = lat.entry(model.to_string()).or_insert(latency_ms as f64);
            *entry = alpha * (latency_ms as f64) + (1.0 - alpha) * *entry;
        }

        // Update success-rate EMA (penalise).
        {
            let mut sr = crate::observability::lock_mutex(&self.model_success_rate);
            let entry = sr.entry(model.to_string()).or_insert(1.0);
            *entry = alpha * 0.0 + (1.0 - alpha) * *entry;
        }

        // Bump request count.
        {
            let mut req = crate::observability::lock_mutex(&self.model_requests);
            *req.entry(model.to_string()).or_insert(0) += 1;
        }

        debug!(
            model = %model,
            latency_ms,
            "LivePerformanceFeed: recorded failure"
        );
    }

    /// Estimate cost-per-request in cents based on observed latency and
    /// success rate.  Cheaper if the model is fast AND reliable.
    pub fn get_cost_estimate(&self, model: &str) -> Option<f64> {
        let lat = crate::observability::lock_mutex(&self.model_latency);
        let sr = crate::observability::lock_mutex(&self.model_success_rate);

        let latency = lat.get(model)?;
        let success = sr.get(model).copied().unwrap_or(1.0);

        // Cost ∝ latency / success_rate: faster + reliable → cheaper.
        Some(*latency / success.max(0.01))
    }

    /// Get the EMA-smoothed latency estimate (ms) for a model.
    pub fn get_latency_estimate(&self, model: &str) -> Option<f64> {
        let lat = crate::observability::lock_mutex(&self.model_latency);
        lat.get(model).copied()
    }

    /// Get the EMA-smoothed success rate (0.0–1.0) for a model.
    pub fn get_success_rate(&self, model: &str) -> Option<f64> {
        let sr = crate::observability::lock_mutex(&self.model_success_rate);
        sr.get(model).copied()
    }

    /// Get the total request count for a model.
    pub fn get_request_count(&self, model: &str) -> u64 {
        let req = crate::observability::lock_mutex(&self.model_requests);
        req.get(model).copied().unwrap_or(0)
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
}
