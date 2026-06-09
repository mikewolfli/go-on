//! ACP Runtime Metrics
//!
//! Tracks request-level metrics, latency histograms, vector/summary counters,
//! agent timeouts, and review-gate outcomes.

use std::sync::Mutex as StdMutex;

use serde::Serialize;
use tracing::warn;

// ============================================================================
// Latency bucket helper (private)
// ============================================================================

/// Latency bucket boundaries for metrics (milliseconds).
/// 9 boundaries → 10 buckets (the +Inf bucket is implicit via `len()`).
const METRIC_LATENCY_BUCKETS_MS: [f64; 9] =
    [1.0, 5.0, 10.0, 50.0, 100.0, 500.0, 1000.0, 5000.0, 10000.0];

fn latency_bucket_index_ms(duration_ms: f64) -> usize {
    for (idx, boundary) in METRIC_LATENCY_BUCKETS_MS.iter().enumerate() {
        if duration_ms <= *boundary {
            return idx;
        }
    }
    METRIC_LATENCY_BUCKETS_MS.len()
}

// ============================================================================
// Metrics snapshot (public)
// ============================================================================

/// Metrics snapshot
#[derive(Debug, Clone, Serialize, Default)]
pub struct MetricsSnapshot {
    /// Total requests processed
    pub total_requests: u64,
    /// Successful requests
    pub successful_requests: u64,
    /// Failed requests
    pub failed_requests: u64,
    /// Average request duration in milliseconds
    pub avg_request_duration_ms: f64,
    /// Cumulative request duration in milliseconds
    pub request_latency_sum_ms: f64,
    /// Request latency histogram bucket counts (ms buckets +Inf)
    pub request_latency_bucket_counts: [u64; 10],
    /// Current active requests
    pub active_requests: u32,
    /// Cache hit rate (0.0 to 1.0)
    pub cache_hit_rate: f64,
    /// Circuit breaker open count
    pub circuit_breaker_open_count: u32,
    /// Memory usage in bytes
    pub memory_usage_bytes: u64,
    /// CPU usage percentage (0.0 to 100.0)
    pub cpu_usage_percent: f64,
    /// Total chat requests
    pub chat_requests_total: u64,
    /// Agent request timeout count across chat / execution paths
    pub agent_timeout_failures_total: u64,
    /// Local runtime probe timeout count for agent readiness checks
    pub runtime_probe_timeout_total: u64,
    /// Vector search requests executed
    pub vector_search_total: u64,
    /// Vector hits returned across searches
    pub vector_hit_total: u64,
    /// Vector entries stored
    pub vector_store_total: u64,
    /// Summary lookups executed
    pub summary_read_total: u64,
    /// Summary cache hits
    pub summary_hit_total: u64,
    /// Summary entries stored
    pub summary_store_total: u64,
    /// Cumulative chat duration in milliseconds
    pub chat_latency_sum_ms: f64,
    /// Chat latency histogram bucket counts (ms buckets +Inf)
    pub chat_latency_bucket_counts: [u64; 10],
    /// Review gate invocations
    pub review_gate_total: u64,
    /// Cumulative review-gate duration in milliseconds
    pub review_latency_sum_ms: f64,
    /// Review latency histogram bucket counts (ms buckets +Inf)
    pub review_latency_bucket_counts: [u64; 10],
    /// Review gate approved count
    pub review_gate_approved_total: u64,
    /// Review gate rejected count
    pub review_gate_rejected_total: u64,
    /// Review gate timeout count
    pub review_gate_timeout_total: u64,
    /// Review gate degraded count
    pub review_gate_degraded_total: u64,
    /// Review gate invalid response count
    pub review_gate_invalid_response_total: u64,
}

// ============================================================================
// Runtime metrics (public API)
// ============================================================================

/// Runtime metrics for tracking server performance
#[derive(Debug)]
pub struct RuntimeMetrics {
    inner: StdMutex<MetricsSnapshot>,
}

impl RuntimeMetrics {
    /// Create new runtime metrics
    pub fn new() -> Self {
        Self {
            inner: StdMutex::new(MetricsSnapshot::default()),
        }
    }

    /// Increment successful requests
    pub fn inc_successful_requests(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.successful_requests += 1;
        guard.total_requests += 1;
    }

    /// Increment failed requests
    pub fn inc_failed_requests(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.failed_requests += 1;
        guard.total_requests += 1;
    }

    /// Increment active requests
    pub fn inc_active_requests(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.active_requests += 1;
    }

    /// Decrement active requests
    pub fn dec_active_requests(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.active_requests = guard.active_requests.saturating_sub(1);
    }

    /// Get successful requests count
    pub fn successful_requests(&self) -> u64 {
        let guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.successful_requests
    }

    /// Get failed requests count
    pub fn failed_requests(&self) -> u64 {
        let guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.failed_requests
    }

    /// Get active requests count
    pub fn active_requests(&self) -> u32 {
        let guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.active_requests
    }

    /// Get total requests count
    pub fn total_requests(&self) -> u64 {
        let guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.total_requests
    }

    /// Get average request duration in milliseconds
    pub fn avg_request_duration_ms(&self) -> f64 {
        let guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.avg_request_duration_ms
    }

    /// Update average request duration
    pub fn update_avg_duration(&self, duration_ms: f64) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        let total = guard.total_requests as f64;
        guard.avg_request_duration_ms = if total <= 1.0 {
            duration_ms
        } else {
            (guard.avg_request_duration_ms * (total - 1.0) + duration_ms) / total
        };
    }

    /// Increment review gate count
    pub fn inc_review_gate(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.review_gate_total += 1;
    }

    /// Increment review gate rejected count
    pub fn inc_review_gate_rejected(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.review_gate_rejected_total += 1;
    }

    /// Increment review gate timeout count
    pub fn inc_review_gate_timeout(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.review_gate_timeout_total += 1;
    }

    /// Increment review gate degraded count
    pub fn inc_review_gate_degraded(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.review_gate_degraded_total += 1;
    }

    /// Increment review gate approved count
    pub fn inc_review_gate_approved(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.review_gate_approved_total += 1;
    }

    /// Increment review gate invalid response count
    pub fn inc_review_gate_invalid_response(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.review_gate_invalid_response_total += 1;
    }

    /// Increment chat requests count
    pub fn inc_chat_requests(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.chat_requests_total += 1;
    }

    /// Record one ACP request outcome with duration.
    pub fn record_request_outcome(&self, success: bool, duration_ms: f64) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        if success {
            guard.successful_requests += 1;
        } else {
            guard.failed_requests += 1;
        }
        guard.total_requests += 1;

        let duration_ms = duration_ms.max(0.0);
        guard.request_latency_sum_ms += duration_ms;
        let bucket_idx = latency_bucket_index_ms(duration_ms);
        guard.request_latency_bucket_counts[bucket_idx] =
            guard.request_latency_bucket_counts[bucket_idx].saturating_add(1);
        guard.avg_request_duration_ms = if guard.total_requests == 0 {
            0.0
        } else {
            guard.request_latency_sum_ms / guard.total_requests as f64
        };
    }

    /// Record chat latency.
    pub fn record_chat_latency(&self, duration_ms: f64) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        let duration_ms = duration_ms.max(0.0);
        guard.chat_requests_total += 1;
        guard.chat_latency_sum_ms += duration_ms;
        let bucket_idx = latency_bucket_index_ms(duration_ms);
        guard.chat_latency_bucket_counts[bucket_idx] =
            guard.chat_latency_bucket_counts[bucket_idx].saturating_add(1);
    }

    /// Increment agent timeout failure counter.
    pub fn inc_agent_timeout_failure(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.agent_timeout_failures_total = guard.agent_timeout_failures_total.saturating_add(1);
    }

    /// Increment runtime probe timeout counter.
    pub fn inc_runtime_probe_timeout(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.runtime_probe_timeout_total = guard.runtime_probe_timeout_total.saturating_add(1);
    }

    /// Record a vector search with hit count.
    pub fn record_vector_search(&self, hit_count: usize) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.vector_search_total = guard.vector_search_total.saturating_add(1);
        guard.vector_hit_total = guard.vector_hit_total.saturating_add(hit_count as u64);
    }

    /// Record a vector store operation.
    pub fn record_vector_store(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.vector_store_total = guard.vector_store_total.saturating_add(1);
    }

    /// Record a summary read, with hit indicator.
    pub fn record_summary_read(&self, hit: bool) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.summary_read_total = guard.summary_read_total.saturating_add(1);
        if hit {
            guard.summary_hit_total = guard.summary_hit_total.saturating_add(1);
        }
    }

    /// Record a summary store operation.
    pub fn record_summary_store(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.summary_store_total = guard.summary_store_total.saturating_add(1);
    }

    /// Record review gate latency.
    pub fn record_review_latency(&self, duration_ms: f64) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        let duration_ms = duration_ms.max(0.0);
        guard.review_latency_sum_ms += duration_ms;
        let bucket_idx = latency_bucket_index_ms(duration_ms);
        guard.review_latency_bucket_counts[bucket_idx] =
            guard.review_latency_bucket_counts[bucket_idx].saturating_add(1);
    }

    /// Update multiple snapshot fields atomically via a closure.
    pub fn update_snapshot<F>(&self, f: F)
    where
        F: FnOnce(&mut MetricsSnapshot),
    {
        if let Ok(mut guard) = self.inner.lock() {
            f(&mut guard);
        }
    }

    /// Get the current metrics snapshot.
    pub fn snapshot(&self) -> MetricsSnapshot {
        self.inner
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Reset all collected runtime metrics.
    pub fn reset_all(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        *guard = MetricsSnapshot::default();
    }
}

impl Default for RuntimeMetrics {
    fn default() -> Self {
        Self::new()
    }
}
