//! ObservabilityBus — Sub-bus for unified OTLP tracing, metrics, logs, and audit (BLUE38 §1, ARCH-13)
//!
//! The ObservabilityBus coordinates observability data so that the CapabilityBus can
//! query latency and error metrics when making routing decisions. It maintains:
//!
//! - A ring buffer of recent [`TraceEvent`] entries
//! - Per-agent latency statistics with percentile calculations (P50, P95, P99)
//! - Per-agent error rate tracking
//! - A summary profile for system-level health queries
//!
//! # Architecture
//!
//! ```text
//!                 CapabilityBus (scheduling coordinator)
//!                         │
//!          ┌──────────────┼──────────────┐
//!          │              │              │
//!     ┌────▼────┐   ┌────▼────┐   ┌─────▼─────┐
//!     │  Work   │   │Observab.│   │  Other    │
//!     │  flow   │   │  Bus    │   │  Sub-     │
//!     │  Learn  │   │(metrics,│   │  buses    │
//!     │  Bus    │   │ traces, │   │           │
//!     │         │   │  audit) │   │           │
//!     └─────────┘   └─────────┘   └───────────┘
//! ```
//!
//! ## Thread safety
//!
//! All internal state is protected by `Arc<Mutex<…>>`, making `ObservabilityBus`
//! safe to share across asynchronous boundaries.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// A single recorded trace event.
#[derive(Clone, Debug)]
pub struct TraceEvent {
    /// Monotonic timestamp in milliseconds (e.g. from `std::time::Instant`).
    pub timestamp_ms: u64,
    /// Agent that handled (or attempted to handle) the task.
    pub agent: String,
    /// High-level category of the task (e.g. "code_gen", "chat", "embedding").
    pub task_type: String,
    /// Wall-clock duration of the operation in milliseconds.
    pub duration_ms: u64,
    /// Whether the operation completed without error.
    pub success: bool,
    /// If `success` is `false`, the error message (if available).
    pub error: Option<String>,
    /// Approximate token cost incurred by the operation.
    pub token_cost: u64,
}

/// Per-agent latency statistics.
#[derive(Clone, Debug)]
pub struct LatencyStats {
    /// Arithmetic mean of recorded durations (ms).
    pub avg_duration_ms: f64,
    /// 50th percentile (median) duration (ms).
    pub p50_ms: f64,
    /// 95th percentile duration (ms).
    pub p95_ms: f64,
    /// 99th percentile duration (ms).
    pub p99_ms: f64,
    /// Number of samples used to compute the above statistics.
    pub sample_count: u64,
    /// Timestamp (ms) of the most recent sample that contributed.
    pub last_updated_ms: u64,
}

/// Per-agent error rate statistics.
#[derive(Clone, Debug)]
pub struct ErrorRateStats {
    /// Total number of recorded calls **or** attempts.
    pub total_calls: u64,
    /// Number of calls that ended in error.
    pub error_count: u64,
    /// Ratio `error_count / total_calls` in the range `[0.0, 1.0]`.
    pub error_rate: f64,
    /// Timestamp (ms) of the most recent error.
    pub last_error_ms: u64,
    /// Consecutive failures since the last success.
    pub consecutive_failures: u64,
}

/// High-level observability profile returned by [`ObservabilityBus::system_health`].
#[derive(Clone, Debug)]
pub struct ObservabilityBusProfile {
    /// Whether the bus is active (default `true` on construction).
    pub enabled: bool,
    /// Total number of traces recorded over the lifetime of the bus.
    pub total_traces: u64,
    /// Number of distinct agents currently being tracked.
    pub tracked_agents: u32,
    /// System-wide arithmetic mean of all recorded durations (ms).
    pub avg_system_latency_ms: f64,
    /// System-wide error rate (errors / total calls).
    pub system_error_rate: f64,
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Sorted container of recent durations used for percentile computation.
///
/// Every call to `record_trace` inserts the latest duration and, when the
/// capacity is exceeded, evicts the oldest duration. Durations are stored in a
/// `Vec<u64>` kept in ascending order so that percentiles can be read in O(1).
#[derive(Clone, Debug)]
struct DurationWindow {
    max_len: usize,
    /// Durations in insertion order (the ring).
    ring: VecDeque<u64>,
    /// Durations in sorted order (kept in sync with `ring`).
    sorted: Vec<u64>,
}

impl DurationWindow {
    fn new(max_len: usize) -> Self {
        Self {
            max_len,
            ring: VecDeque::with_capacity(max_len),
            sorted: Vec::with_capacity(max_len),
        }
    }

    /// Push a new duration, evicting the oldest if the window is full.
    fn push(&mut self, duration_ms: u64) {
        if self.ring.len() == self.max_len {
            // Evict the oldest.
            if let Some(oldest) = self.ring.pop_front() {
                if let Ok(pos) = self.sorted.binary_search(&oldest) {
                    self.sorted.remove(pos);
                }
            }
        }
        self.ring.push_back(duration_ms);
        let pos = self
            .sorted
            .binary_search(&duration_ms)
            .unwrap_or_else(|e| e);
        self.sorted.insert(pos, duration_ms);
    }

    fn len(&self) -> usize {
        self.sorted.len()
    }

    fn avg(&self) -> f64 {
        let n = self.len();
        if n == 0 {
            return 0.0;
        }
        let sum: u64 = self.sorted.iter().copied().sum();
        sum as f64 / n as f64
    }

    /// Returns the p-th percentile (p in [0.0, 100.0]).
    ///
    /// Uses the **nearest-rank** method: the value at the smallest index
    /// whose rank is ≥ the desired percentile.
    fn percentile(&self, p: f64) -> f64 {
        let n = self.len();
        if n == 0 {
            return 0.0;
        }
        // rank = ceil(p / 100.0 * n), 1-based
        let rank = (p / 100.0 * n as f64).ceil().max(1.0).min(n as f64) as usize;
        self.sorted[rank - 1] as f64
    }
}

// ---------------------------------------------------------------------------
// ObservabilityBus
// ---------------------------------------------------------------------------

/// Unified observability sub-bus that CapabilityBus can query for latency /
/// error metrics when making routing decisions.
pub struct ObservabilityBus {
    /// Recent trace events in insertion order (ring buffer).
    trace_events: Arc<Mutex<VecDeque<TraceEvent>>>,
    /// Per-agent latency statistics.
    agent_latency: Arc<Mutex<HashMap<String, LatencyStats>>>,
    /// Per-agent error rate statistics.
    agent_error_rates: Arc<Mutex<HashMap<String, ErrorRateStats>>>,
    /// Maximum number of trace events retained in the ring buffer.
    max_events: usize,
    /// Profile and system-level metrics.
    profile: Arc<Mutex<ObservabilityBusProfile>>,
    /// Per-agent sorted-duration windows for percentile computation.
    ///
    /// Keep separate from `agent_latency` so that `LatencyStats` remains
    /// cheaply clonable and we don't leak the sliding-window internals.
    windows: Arc<Mutex<HashMap<String, DurationWindow>>>,
}

impl ObservabilityBus {
    /// Create a new `ObservabilityBus` with default capacity (10 000 events).
    pub fn new() -> Self {
        Self::with_capacity(10_000)
    }

    /// Create a new `ObservabilityBus` with a specific ring-buffer capacity.
    pub fn with_capacity(max_events: usize) -> Self {
        Self {
            trace_events: Arc::new(Mutex::new(VecDeque::with_capacity(max_events))),
            agent_latency: Arc::new(Mutex::new(HashMap::new())),
            agent_error_rates: Arc::new(Mutex::new(HashMap::new())),
            max_events,
            profile: Arc::new(Mutex::new(ObservabilityBusProfile {
                enabled: true,
                total_traces: 0,
                tracked_agents: 0,
                avg_system_latency_ms: 0.0,
                system_error_rate: 0.0,
            })),
            windows: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Record an execution trace and update all derived statistics.
    pub fn record_trace(
        &self,
        agent: &str,
        task_type: &str,
        duration_ms: u64,
        success: bool,
        error: Option<String>,
        token_cost: u64,
    ) {
        let now = Self::now_ms();

        // --- 1. Push trace event into the ring buffer ---
        {
            let mut events = self
                .trace_events
                .lock()
                .expect("ObservabilityBus trace_events lock poisoned");
            if events.len() == self.max_events {
                events.pop_front();
            }
            events.push_back(TraceEvent {
                timestamp_ms: now,
                agent: agent.to_string(),
                task_type: task_type.to_string(),
                duration_ms,
                success,
                error: error.clone(),
                token_cost,
            });
        }

        // --- 2. Update per-agent latency ---
        {
            let mut windows = self
                .windows
                .lock()
                .expect("ObservabilityBus windows lock poisoned");
            let window = windows
                .entry(agent.to_string())
                .or_insert_with(|| DurationWindow::new(self.max_events));
            window.push(duration_ms);
            let n = window.len();

            let stats = LatencyStats {
                avg_duration_ms: window.avg(),
                p50_ms: window.percentile(50.0),
                p95_ms: window.percentile(95.0),
                p99_ms: window.percentile(99.0),
                sample_count: n as u64,
                last_updated_ms: now,
            };

            let mut latency = self
                .agent_latency
                .lock()
                .expect("ObservabilityBus agent_latency lock poisoned");
            latency.insert(agent.to_string(), stats);
        }

        // --- 3. Update per-agent error rate ---
        {
            let mut error_rates = self
                .agent_error_rates
                .lock()
                .expect("ObservabilityBus agent_error_rates lock poisoned");
            let ers = error_rates
                .entry(agent.to_string())
                .or_insert(ErrorRateStats {
                    total_calls: 0,
                    error_count: 0,
                    error_rate: 0.0,
                    last_error_ms: 0,
                    consecutive_failures: 0,
                });
            ers.total_calls += 1;
            if success {
                ers.consecutive_failures = 0;
            } else {
                ers.error_count += 1;
                ers.last_error_ms = now;
                ers.consecutive_failures += 1;
            }
            ers.error_rate = if ers.total_calls > 0 {
                ers.error_count as f64 / ers.total_calls as f64
            } else {
                0.0
            };
        }

        // --- 4. Update profile ---
        {
            let mut profile = self
                .profile
                .lock()
                .expect("ObservabilityBus profile lock poisoned");
            profile.total_traces += 1;

            // Recompute system-level aggregates from latencies.
            let latency = self
                .agent_latency
                .lock()
                .expect("ObservabilityBus agent_latency lock poisoned (inner)");
            profile.tracked_agents = latency.len() as u32;

            let avg_sys: f64 = if !latency.is_empty() {
                let sum: f64 = latency.values().map(|s| s.avg_duration_ms).sum();
                sum / latency.len() as f64
            } else {
                0.0
            };
            profile.avg_system_latency_ms = avg_sys;

            let error_rates = self
                .agent_error_rates
                .lock()
                .expect("ObservabilityBus agent_error_rates lock poisoned (inner)");
            let (total_errors, total_calls): (u64, u64) = error_rates.values().fold(
                (0, 0),
                |(acc_err, acc_call), s| (acc_err + s.error_count, acc_call + s.total_calls),
            );
            profile.system_error_rate = if total_calls > 0 {
                total_errors as f64 / total_calls as f64
            } else {
                0.0
            };
        }
    }

    /// Return latency statistics for a specific agent, or `None` if unknown.
    pub fn agent_latency(&self, agent: &str) -> Option<LatencyStats> {
        let latency = self
            .agent_latency
            .lock()
            .expect("ObservabilityBus agent_latency lock poisoned");
        latency.get(agent).cloned()
    }

    /// Return error-rate statistics for a specific agent, or `None` if unknown.
    pub fn agent_error_rate(&self, agent: &str) -> Option<ErrorRateStats> {
        let error_rates = self
            .agent_error_rates
            .lock()
            .expect("ObservabilityBus agent_error_rates lock poisoned");
        error_rates.get(agent).cloned()
    }

    /// Return the names of all agents whose error rate is strictly **below**
    /// `max_error_rate`.
    ///
    /// Agents with zero recorded calls are **excluded** since there is
    /// insufficient data for a routing decision.
    pub fn healthy_agents(&self, max_error_rate: f64) -> Vec<String> {
        let error_rates = self
            .agent_error_rates
            .lock()
            .expect("ObservabilityBus agent_error_rates lock poisoned");
        error_rates
            .iter()
            .filter(|(_, s)| s.total_calls > 0 && s.error_rate < max_error_rate)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Return the names of all agents whose **average** latency exceeds
    /// `threshold_ms`.
    ///
    /// Agents with zero recorded calls are excluded.
    pub fn slow_agents(&self, threshold_ms: f64) -> Vec<String> {
        let latency = self
            .agent_latency
            .lock()
            .expect("ObservabilityBus agent_latency lock poisoned");
        latency
            .iter()
            .filter(|(_, s)| s.sample_count > 0 && s.avg_duration_ms > threshold_ms)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Return a high-level profile snapshot of the bus.
    pub fn system_health(&self) -> ObservabilityBusProfile {
        let profile = self
            .profile
            .lock()
            .expect("ObservabilityBus profile lock poisoned");
        profile.clone()
    }

    /// Return the `count` most recent trace events.
    ///
    /// If fewer than `count` events have been recorded, all available events
    /// are returned.
    pub fn recent_traces(&self, count: usize) -> Vec<TraceEvent> {
        let events = self
            .trace_events
            .lock()
            .expect("ObservabilityBus trace_events lock poisoned");
        let len = events.len();
        let start = if len > count { len - count } else { 0 };
        events.range(start..).cloned().collect()
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Monotonic timestamp in milliseconds (epoch-based for human readability).
    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

impl Default for ObservabilityBus {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_bus_is_empty() {
        let bus = ObservabilityBus::new();
        let health = bus.system_health();
        assert!(health.enabled);
        assert_eq!(health.total_traces, 0);
        assert_eq!(health.tracked_agents, 0);
        assert!(bus.recent_traces(10).is_empty());
    }

    #[test]
    fn test_record_trace_basic() {
        let bus = ObservabilityBus::new();
        bus.record_trace("agent-a", "chat", 100, true, None, 50);
        bus.record_trace("agent-a", "chat", 200, true, None, 30);

        let traces = bus.recent_traces(5);
        assert_eq!(traces.len(), 2);
        assert_eq!(traces[0].agent, "agent-a");
        assert_eq!(traces[0].duration_ms, 100);
        assert!(traces[0].success);

        let latency = bus.agent_latency("agent-a").unwrap();
        assert_eq!(latency.sample_count, 2);
        assert!((latency.avg_duration_ms - 150.0).abs() < 0.01);
        // With only 2 samples all percentiles land on actual values.
        assert!((latency.p50_ms - 150.0).abs() < 0.01 || (latency.p50_ms - 100.0).abs() < 0.01 || (latency.p50_ms - 200.0).abs() < 0.01);

        let err_rate = bus.agent_error_rate("agent-a").unwrap();
        assert_eq!(err_rate.total_calls, 2);
        assert_eq!(err_rate.error_count, 0);
        assert!((err_rate.error_rate - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_error_rate_tracking() {
        let bus = ObservabilityBus::new();
        bus.record_trace("agent-b", "embed", 50, true, None, 10);
        bus.record_trace("agent-b", "embed", 60, false, Some("timeout".to_string()), 10);
        bus.record_trace("agent-b", "embed", 55, false, Some("panic".to_string()), 10);

        let err = bus.agent_error_rate("agent-b").unwrap();
        assert_eq!(err.total_calls, 3);
        assert_eq!(err.error_count, 2);
        assert!((err.error_rate - 2.0 / 3.0).abs() < 0.001);
        assert_eq!(err.consecutive_failures, 2);
    }

    #[test]
    fn test_healthy_and_slow_agents() {
        let bus = ObservabilityBus::new();
        bus.record_trace("fast", "chat", 10, true, None, 1);
        bus.record_trace("fast", "chat", 12, true, None, 1);
        bus.record_trace("slow", "chat", 500, true, None, 1);
        bus.record_trace("erratic", "chat", 30, false, Some("err".to_string()), 1);

        let healthy = bus.healthy_agents(0.5);
        assert!(healthy.contains(&"fast".to_string()));
        assert!(healthy.contains(&"slow".to_string()));
        assert!(!healthy.contains(&"erratic".to_string()));

        let slow = bus.slow_agents(100.0);
        assert!(slow.contains(&"slow".to_string()));
        assert!(!slow.contains(&"fast".to_string()));
        assert!(!slow.contains(&"erratic".to_string()));
    }

    #[test]
    fn test_ring_buffer_eviction() {
        let bus = ObservabilityBus::with_capacity(5);
        for i in 0..10 {
            bus.record_trace("a", "test", i * 10, true, None, i);
        }
        let traces = bus.recent_traces(10);
        assert_eq!(traces.len(), 5);
        // The oldest retained duration should be 50 (index 5 * 10).
        assert_eq!(traces[0].duration_ms, 50);
        assert_eq!(traces[4].duration_ms, 90);
    }

    #[test]
    fn test_system_health() {
        let bus = ObservabilityBus::new();
        bus.record_trace("x", "chat", 100, true, None, 5);
        bus.record_trace("y", "embed", 200, false, Some("err".to_string()), 10);
        bus.record_trace("x", "chat", 300, true, None, 5);

        let health = bus.system_health();
        assert_eq!(health.total_traces, 3);
        assert_eq!(health.tracked_agents, 2);
        // avg_system_latency = (100 + 300)/2 for x, 200 for y => (200 + 200)/2 = 200
        assert!((health.avg_system_latency_ms - 200.0).abs() < 0.01);
        // system_error_rate = 1 error / 3 calls
        assert!((health.system_error_rate - 1.0 / 3.0).abs() < 0.001);
    }
}
