//! ACP Runtime Metrics
//!
//! Tracks request-level metrics, latency histograms, vector/summary counters,
//! agent timeouts, and review-gate outcomes.
//!
//! # Performance
//!
//! Individual counters use `AtomicU64` for lock-free reads/writes.
//! Latency histograms and aggregate fields that must be read atomically
//! together still use a `StdMutex<AggregateSnapshot>`.
//!
//! Migrated from a single `StdMutex<MetricsSnapshot>` (log-20260623-8).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex as StdMutex;

use serde::Serialize;
use tracing::warn;

// ============================================================================
// Latency bucket helper (private)
// ============================================================================

/// Latency bucket boundaries for metrics (milliseconds).
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
// Aggregate snapshot (Mutex-protected — for multi-field atomic reads)
// ============================================================================

/// Fields that must be read atomically together are in this snapshot.
/// Individual counters use `AtomicU64` and are lock-free.
#[derive(Debug, Clone, Serialize, Default)]
struct AggregateSnapshot {
    /// Request latency sum (used for avg_duration computation)
    pub request_latency_sum_ms: f64,
    /// Request latency histogram bucket counts (ms buckets +Inf)
    pub request_latency_bucket_counts: [u64; 10],
    /// Chat latency sum
    pub chat_latency_sum_ms: f64,
    /// Chat latency histogram bucket counts
    pub chat_latency_bucket_counts: [u64; 10],
    /// Review latency sum
    pub review_latency_sum_ms: f64,
    /// Review latency histogram bucket counts
    pub review_latency_bucket_counts: [u64; 10],
    /// Average request duration (computed, not stored independently)
    pub avg_request_duration_ms: f64,
}

// ============================================================================
// Metrics snapshot (public — for observability consumers)
// ============================================================================

/// Metrics snapshot — combines atomic counters + aggregate snapshot.
#[derive(Debug, Clone, Serialize, Default)]
pub struct MetricsSnapshot {
    // ── Lock-free atomic counters ──────────────────────────────────────────
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub active_requests: u32,
    pub chat_requests_total: u64,
    pub agent_timeout_failures_total: u64,
    pub runtime_probe_timeout_total: u64,
    pub vector_search_total: u64,
    pub vector_hit_total: u64,
    pub vector_store_total: u64,
    pub summary_read_total: u64,
    pub summary_hit_total: u64,
    pub summary_store_total: u64,
    pub review_gate_total: u64,
    pub review_gate_approved_total: u64,
    pub review_gate_rejected_total: u64,
    pub review_gate_timeout_total: u64,
    pub review_gate_degraded_total: u64,
    pub review_gate_invalid_response_total: u64,
    // ── Aggregate fields (read from Mutex) ─────────────────────────────────
    pub avg_request_duration_ms: f64,
    pub request_latency_sum_ms: f64,
    pub request_latency_bucket_counts: [u64; 10],
    pub chat_latency_sum_ms: f64,
    pub chat_latency_bucket_counts: [u64; 10],
    pub review_latency_sum_ms: f64,
    pub review_latency_bucket_counts: [u64; 10],
    // ── System metrics (set externally) ────────────────────────────────────
    pub cache_hit_rate: f64,
    pub circuit_breaker_open_count: u32,
    pub memory_usage_bytes: u64,
    pub cpu_usage_percent: f64,
}

// ============================================================================
// Runtime metrics (public API)
// ============================================================================

/// Runtime metrics for tracking server performance.
///
/// Individual counters use `AtomicU64` for lock-free access.
/// Latency histograms and aggregates use a single `StdMutex`.
#[derive(Debug)]
pub struct RuntimeMetrics {
    // ── Lock-free counters ─────────────────────────────────────────────────
    total_requests: AtomicU64,
    successful_requests: AtomicU64,
    failed_requests: AtomicU64,
    active_requests: AtomicU32,
    chat_requests_total: AtomicU64,
    agent_timeout_failures_total: AtomicU64,
    runtime_probe_timeout_total: AtomicU64,
    vector_search_total: AtomicU64,
    vector_hit_total: AtomicU64,
    vector_store_total: AtomicU64,
    summary_read_total: AtomicU64,
    summary_hit_total: AtomicU64,
    summary_store_total: AtomicU64,
    review_gate_total: AtomicU64,
    review_gate_approved_total: AtomicU64,
    review_gate_rejected_total: AtomicU64,
    review_gate_timeout_total: AtomicU64,
    review_gate_degraded_total: AtomicU64,
    review_gate_invalid_response_total: AtomicU64,
    // ── Aggregate fields (Mutex-protected) ─────────────────────────────────
    aggregates: StdMutex<AggregateSnapshot>,
}

impl RuntimeMetrics {
    /// Create new runtime metrics
    pub fn new() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            active_requests: AtomicU32::new(0),
            chat_requests_total: AtomicU64::new(0),
            agent_timeout_failures_total: AtomicU64::new(0),
            runtime_probe_timeout_total: AtomicU64::new(0),
            vector_search_total: AtomicU64::new(0),
            vector_hit_total: AtomicU64::new(0),
            vector_store_total: AtomicU64::new(0),
            summary_read_total: AtomicU64::new(0),
            summary_hit_total: AtomicU64::new(0),
            summary_store_total: AtomicU64::new(0),
            review_gate_total: AtomicU64::new(0),
            review_gate_approved_total: AtomicU64::new(0),
            review_gate_rejected_total: AtomicU64::new(0),
            review_gate_timeout_total: AtomicU64::new(0),
            review_gate_degraded_total: AtomicU64::new(0),
            review_gate_invalid_response_total: AtomicU64::new(0),
            aggregates: StdMutex::new(AggregateSnapshot::default()),
        }
    }

    // ── Counter increments (lock-free) ─────────────────────────────────────

    #[inline]
    pub fn inc_successful_requests(&self) {
        self.successful_requests.fetch_add(1, Ordering::Relaxed);
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_failed_requests(&self) {
        self.failed_requests.fetch_add(1, Ordering::Relaxed);
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_active_requests(&self) {
        self.active_requests.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn dec_active_requests(&self) {
        self.active_requests.fetch_sub(1, Ordering::Relaxed);
    }

    // ── Counter reads (lock-free) ──────────────────────────────────────────

    #[inline]
    pub fn successful_requests(&self) -> u64 {
        self.successful_requests.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn failed_requests(&self) -> u64 {
        self.failed_requests.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn active_requests(&self) -> u32 {
        self.active_requests.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn total_requests(&self) -> u64 {
        self.total_requests.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn avg_request_duration_ms(&self) -> f64 {
        self.aggregates
            .lock()
            .map(|g| g.avg_request_duration_ms)
            .unwrap_or(0.0)
    }

    // ── Aggregate updates (Mutex-protected) ────────────────────────────────

    pub fn update_avg_duration(&self, duration_ms: f64) {
        let total = self.total_requests.load(Ordering::Relaxed) as f64;
        let mut guard = match self.aggregates.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                warn!("metrics aggregates lock poisoned, recovering");
                poisoned.into_inner()
            }
        };
        guard.avg_request_duration_ms = if total <= 1.0 {
            duration_ms
        } else {
            (guard.avg_request_duration_ms * (total - 1.0) + duration_ms) / total
        };
        guard.request_latency_sum_ms += duration_ms;
        let bucket_idx = latency_bucket_index_ms(duration_ms);
        guard.request_latency_bucket_counts[bucket_idx] =
            guard.request_latency_bucket_counts[bucket_idx].saturating_add(1);
    }

    pub fn inc_review_gate(&self) {
        self.review_gate_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_review_gate_rejected(&self) {
        self.review_gate_rejected_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_review_gate_timeout(&self) {
        self.review_gate_timeout_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_review_gate_degraded(&self) {
        self.review_gate_degraded_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_review_gate_approved(&self) {
        self.review_gate_approved_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_review_gate_invalid_response(&self) {
        self.review_gate_invalid_response_total
            .fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_chat_requests(&self) {
        self.chat_requests_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one ACP request outcome with duration.
    pub fn record_request_outcome(&self, success: bool, duration_ms: f64) {
        if success {
            self.successful_requests.fetch_add(1, Ordering::Relaxed);
        } else {
            self.failed_requests.fetch_add(1, Ordering::Relaxed);
        }
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        let duration_ms = duration_ms.max(0.0);
        let mut guard = match self.aggregates.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                warn!("metrics aggregates lock poisoned, recovering");
                poisoned.into_inner()
            }
        };
        guard.request_latency_sum_ms += duration_ms;
        let bucket_idx = latency_bucket_index_ms(duration_ms);
        guard.request_latency_bucket_counts[bucket_idx] =
            guard.request_latency_bucket_counts[bucket_idx].saturating_add(1);
        let total = self.total_requests.load(Ordering::Relaxed) as f64;
        guard.avg_request_duration_ms = if total == 0.0 {
            0.0
        } else {
            guard.request_latency_sum_ms / total
        };
    }

    /// Record chat latency.
    pub fn record_chat_latency(&self, duration_ms: f64) {
        self.chat_requests_total.fetch_add(1, Ordering::Relaxed);
        let duration_ms = duration_ms.max(0.0);
        let mut guard = match self.aggregates.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                warn!("metrics aggregates lock poisoned, recovering");
                poisoned.into_inner()
            }
        };
        guard.chat_latency_sum_ms += duration_ms;
        let bucket_idx = latency_bucket_index_ms(duration_ms);
        guard.chat_latency_bucket_counts[bucket_idx] =
            guard.chat_latency_bucket_counts[bucket_idx].saturating_add(1);
    }

    #[inline]
    pub fn inc_agent_timeout_failure(&self) {
        self.agent_timeout_failures_total
            .fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_runtime_probe_timeout(&self) {
        self.runtime_probe_timeout_total
            .fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_vector_search(&self, hit_count: usize) {
        self.vector_search_total.fetch_add(1, Ordering::Relaxed);
        self.vector_hit_total
            .fetch_add(hit_count as u64, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_vector_store(&self) {
        self.vector_store_total.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_summary_read(&self, hit: bool) {
        self.summary_read_total.fetch_add(1, Ordering::Relaxed);
        if hit {
            self.summary_hit_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[inline]
    pub fn record_summary_store(&self) {
        self.summary_store_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record review gate latency.
    pub fn record_review_latency(&self, duration_ms: f64) {
        let duration_ms = duration_ms.max(0.0);
        let mut guard = match self.aggregates.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                warn!("metrics aggregates lock poisoned, recovering");
                poisoned.into_inner()
            }
        };
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
        let mut snapshot = self.snapshot();
        f(&mut snapshot);
    }

    /// Get the current metrics snapshot (combines atomics + aggregates).
    pub fn snapshot(&self) -> MetricsSnapshot {
        let agg = self
            .aggregates
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        MetricsSnapshot {
            total_requests: self.total_requests.load(Ordering::Relaxed),
            successful_requests: self.successful_requests.load(Ordering::Relaxed),
            failed_requests: self.failed_requests.load(Ordering::Relaxed),
            active_requests: self.active_requests.load(Ordering::Relaxed),
            chat_requests_total: self.chat_requests_total.load(Ordering::Relaxed),
            agent_timeout_failures_total: self.agent_timeout_failures_total.load(Ordering::Relaxed),
            runtime_probe_timeout_total: self.runtime_probe_timeout_total.load(Ordering::Relaxed),
            vector_search_total: self.vector_search_total.load(Ordering::Relaxed),
            vector_hit_total: self.vector_hit_total.load(Ordering::Relaxed),
            vector_store_total: self.vector_store_total.load(Ordering::Relaxed),
            summary_read_total: self.summary_read_total.load(Ordering::Relaxed),
            summary_hit_total: self.summary_hit_total.load(Ordering::Relaxed),
            summary_store_total: self.summary_store_total.load(Ordering::Relaxed),
            review_gate_total: self.review_gate_total.load(Ordering::Relaxed),
            review_gate_approved_total: self.review_gate_approved_total.load(Ordering::Relaxed),
            review_gate_rejected_total: self.review_gate_rejected_total.load(Ordering::Relaxed),
            review_gate_timeout_total: self.review_gate_timeout_total.load(Ordering::Relaxed),
            review_gate_degraded_total: self.review_gate_degraded_total.load(Ordering::Relaxed),
            review_gate_invalid_response_total: self
                .review_gate_invalid_response_total
                .load(Ordering::Relaxed),
            avg_request_duration_ms: agg.avg_request_duration_ms,
            request_latency_sum_ms: agg.request_latency_sum_ms,
            request_latency_bucket_counts: agg.request_latency_bucket_counts,
            chat_latency_sum_ms: agg.chat_latency_sum_ms,
            chat_latency_bucket_counts: agg.chat_latency_bucket_counts,
            review_latency_sum_ms: agg.review_latency_sum_ms,
            review_latency_bucket_counts: agg.review_latency_bucket_counts,
            cache_hit_rate: 0.0,
            circuit_breaker_open_count: 0,
            memory_usage_bytes: 0,
            cpu_usage_percent: 0.0,
        }
    }

    /// Reset all collected runtime metrics.
    pub fn reset_all(&self) {
        self.total_requests.store(0, Ordering::Relaxed);
        self.successful_requests.store(0, Ordering::Relaxed);
        self.failed_requests.store(0, Ordering::Relaxed);
        self.active_requests.store(0, Ordering::Relaxed);
        self.chat_requests_total.store(0, Ordering::Relaxed);
        self.agent_timeout_failures_total
            .store(0, Ordering::Relaxed);
        self.runtime_probe_timeout_total.store(0, Ordering::Relaxed);
        self.vector_search_total.store(0, Ordering::Relaxed);
        self.vector_hit_total.store(0, Ordering::Relaxed);
        self.vector_store_total.store(0, Ordering::Relaxed);
        self.summary_read_total.store(0, Ordering::Relaxed);
        self.summary_hit_total.store(0, Ordering::Relaxed);
        self.summary_store_total.store(0, Ordering::Relaxed);
        self.review_gate_total.store(0, Ordering::Relaxed);
        self.review_gate_approved_total.store(0, Ordering::Relaxed);
        self.review_gate_rejected_total.store(0, Ordering::Relaxed);
        self.review_gate_timeout_total.store(0, Ordering::Relaxed);
        self.review_gate_degraded_total.store(0, Ordering::Relaxed);
        self.review_gate_invalid_response_total
            .store(0, Ordering::Relaxed);
        if let Ok(mut guard) = self.aggregates.lock() {
            *guard = AggregateSnapshot::default();
        }
    }
}

impl Default for RuntimeMetrics {
    fn default() -> Self {
        Self::new()
    }
}
