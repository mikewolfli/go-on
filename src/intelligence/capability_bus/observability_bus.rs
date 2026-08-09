//! ObservabilityBus — Sub-bus for unified OTLP tracing, metrics, logs, and audit (BLUE38 §1, ARCH-13)
//!
//! The ObservabilityBus coordinates observability data so that the CapabilityBus can
//! query latency and error metrics when making routing decisions. It maintains:
//!
//! - A ring buffer of recent trace events
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

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// Per-agent latency statistics.
#[derive(Clone, Debug)]
pub struct LatencyStats {
    /// Arithmetic mean of recorded durations (ms).
    pub avg_duration_ms: f64,
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
#[derive(Clone, Debug, Serialize, Deserialize)]
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
}

// ---------------------------------------------------------------------------
// ObservabilityBus
// ---------------------------------------------------------------------------

/// Unified observability sub-bus that CapabilityBus can query for latency /
/// error metrics when making routing decisions.
pub struct ObservabilityBus {
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
    /// Maximum number of tracked agents before FIFO eviction
    max_agents: usize,
}

impl ObservabilityBus {
    /// Create a new `ObservabilityBus` with default capacity (10 000 events).
    pub fn new() -> Self {
        Self::with_capacity(10_000)
    }

    /// Create a new `ObservabilityBus` with a specific ring-buffer capacity.
    pub fn with_capacity(max_events: usize) -> Self {
        Self {
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
            max_agents: 1000,
        }
    }

    /// Record an execution trace and update all derived statistics.
    ///
    /// The signature carries only the signals the derived statistics consume
    /// (agent identity, duration, success). The former per-event trace ring
    /// (and its task_type / error / token_cost fields) was removed — nothing
    /// in production ever read it back.
    pub fn record_trace(&self, agent: &str, duration_ms: u64, success: bool) {
        let now = crate::shared::timestamps::now_ts_ms() as u64;

        // --- 1. Update per-agent latency ---
        {
            let mut windows = crate::lock_or_recover!(self.windows.as_ref(), "intelligence");
            let window = windows
                .entry(agent.to_string())
                .or_insert_with(|| DurationWindow::new(self.max_events));
            window.push(duration_ms);

            let stats = LatencyStats {
                avg_duration_ms: window.avg(),
            };

            let mut latency = crate::lock_or_recover!(self.agent_latency.as_ref(), "intelligence");
            // Evict oldest agent latency when at capacity for a new agent.
            if !latency.contains_key(agent) && latency.len() >= self.max_agents {
                if let Some(oldest) = latency.keys().next().cloned() {
                    latency.remove(&oldest);
                }
            }
            latency.insert(agent.to_string(), stats);
        }

        // --- 3. Update per-agent error rate ---
        {
            let mut error_rates =
                crate::lock_or_recover!(self.agent_error_rates.as_ref(), "intelligence");
            // Evict oldest agent error rate when at capacity for a new agent.
            if !error_rates.contains_key(agent) && error_rates.len() >= self.max_agents {
                if let Some(oldest) = error_rates.keys().next().cloned() {
                    error_rates.remove(&oldest);
                }
            }
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
            let mut profile = crate::lock_or_recover!(self.profile.as_ref(), "intelligence");
            profile.total_traces += 1;

            // Recompute system-level aggregates from latencies.
            let latency = crate::lock_or_recover!(self.agent_latency.as_ref(), "intelligence");
            profile.tracked_agents = latency.len() as u32;

            let avg_sys: f64 = if !latency.is_empty() {
                let sum: f64 = latency.values().map(|s| s.avg_duration_ms).sum();
                sum / latency.len() as f64
            } else {
                0.0
            };
            profile.avg_system_latency_ms = avg_sys;

            let error_rates =
                crate::lock_or_recover!(self.agent_error_rates.as_ref(), "intelligence");
            let (total_errors, total_calls): (u64, u64) =
                error_rates.values().fold((0, 0), |(acc_err, acc_call), s| {
                    (acc_err + s.error_count, acc_call + s.total_calls)
                });
            profile.system_error_rate = if total_calls > 0 {
                total_errors as f64 / total_calls as f64
            } else {
                0.0
            };
        }
    }

    /// Return error-rate statistics for a specific agent, or `None` if unknown.
    pub fn agent_error_rate(&self, agent: &str) -> Option<ErrorRateStats> {
        let error_rates = crate::lock_or_recover!(self.agent_error_rates.as_ref(), "intelligence");
        error_rates.get(agent).cloned()
    }

    /// Return a high-level profile snapshot of the bus.
    pub fn system_health(&self) -> ObservabilityBusProfile {
        let profile = crate::lock_or_recover!(self.profile.as_ref(), "intelligence");
        profile.clone()
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
    fn test_record_trace_basic() {
        let bus = ObservabilityBus::new();
        bus.record_trace("agent-a", 100, true);
        bus.record_trace("agent-a", 200, true);

        // Per-agent average latency is consumed by the system_health profile.
        let health = bus.system_health();
        assert_eq!(health.total_traces, 2);
        assert_eq!(health.tracked_agents, 1);
        assert!((health.avg_system_latency_ms - 150.0).abs() < 0.01);

        let err_rate = bus
            .agent_error_rate("agent-a")
            .expect("agent-a should have error rate stats");
        assert_eq!(err_rate.total_calls, 2);
        assert_eq!(err_rate.error_count, 0);
        assert!((err_rate.error_rate - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_error_rate_tracking() {
        let bus = ObservabilityBus::new();
        bus.record_trace("agent-b", 50, true);
        bus.record_trace("agent-b", 60, false);
        bus.record_trace("agent-b", 55, false);

        let err = bus
            .agent_error_rate("agent-b")
            .expect("agent-b should have error rate stats");
        assert_eq!(err.total_calls, 3);
        assert_eq!(err.error_count, 2);
        assert!((err.error_rate - 2.0 / 3.0).abs() < 0.001);
        assert_eq!(err.consecutive_failures, 2);
    }

    #[test]
    fn test_system_health() {
        let bus = ObservabilityBus::new();
        bus.record_trace("x", 100, true);
        bus.record_trace("y", 200, false);
        bus.record_trace("x", 300, true);

        let health = bus.system_health();
        assert_eq!(health.total_traces, 3);
        assert_eq!(health.tracked_agents, 2);
        // avg_system_latency = (100 + 300)/2 for x, 200 for y => (200 + 200)/2 = 200
        assert!((health.avg_system_latency_ms - 200.0).abs() < 0.01);
        // system_error_rate = 1 error / 3 calls
        assert!((health.system_error_rate - 1.0 / 3.0).abs() < 0.001);
    }
}
