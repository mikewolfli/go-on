//! BLUE38 F-GAP-21: Self-Model Core (M5 "自模型核心")
//!
//! Structured self-representation that tracks the system's own capabilities,
//! limitations, identity, and performance. All state is guarded behind
//! `Arc<Mutex<>>` for thread-safe access.

use crate::shared::execution_recorder::ExecutionRecorder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::debug;

// ---------------------------------------------------------------------------
// Data Structures
// ---------------------------------------------------------------------------

/// Identity of the system — who it is, who made it, and descriptive metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfIdentity {
    pub system_name: String,
    pub version: String,
    pub description: String,
    pub creator: String,
    pub created_ms: u64,
    pub tags: Vec<String>,
}

/// Dynamic EMA-based statistics for a capability, updated on each execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityStats {
    /// EMA of success rate in [0.0, 1.0].
    pub effectiveness: f64,
    /// EMA adjusted by sample count; grows toward 1.0 as more samples arrive.
    pub confidence: f64,
    /// EMA of observed latency in ms.
    pub avg_latency_ms: f64,
    /// Total number of execution samples collected.
    pub sample_count: u64,
    /// Timestamp (ms since epoch) of the last recorded execution.
    pub last_updated: u64,
}

/// Configuration for the self-model core's behaviour.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfModelConfig {
    /// How often (in ms) the self-model should be refreshed / updated.
    pub update_interval_ms: u64,
    /// Maximum number of historical snapshots and records to retain.
    pub max_history: usize,
    /// Whether to enable performance tracking (snapshot recording).
    pub enable_performance_tracking: bool,
}

impl Default for SelfModelConfig {
    fn default() -> Self {
        Self {
            update_interval_ms: 60_000, // 1 minute
            max_history: 1000,
            enable_performance_tracking: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct Inner {
    identity: Option<SelfIdentity>,
    capability_stats: HashMap<String, CapabilityStats>,
    last_update_ms: u64,
    #[serde(skip)]
    persistence_path: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Public API — SelfModelCore
// ---------------------------------------------------------------------------

/// Thread-safe self-model core that provides a structured representation of
/// the system's own identity, capabilities, limitations, and performance.
#[derive(Debug, Clone)]
pub struct SelfModelCore {
    inner: Arc<Mutex<Inner>>,
}

impl SelfModelCore {
    /// Create a new self-model core with the given configuration.
    pub fn new(_config: SelfModelConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                identity: None,
                capability_stats: HashMap::new(),
                last_update_ms: crate::shared::timestamps::now_ts_ms() as u64,
                persistence_path: None,
            })),
        }
    }

    /// Persist without locking (caller must hold the lock).
    /// Used to avoid re-entrant lock deadlocks when persist is called
    /// from methods that already hold the lock.
    fn persist_inner(inner: &Inner) {
        if let Some(path) = &inner.persistence_path {
            match serde_json::to_string_pretty(inner) {
                Ok(json) => {
                    if let Err(e) = fs::write(path, &json) {
                        debug!("SelfModel: failed to write persistence file: {e}");
                    }
                }
                Err(e) => {
                    debug!("SelfModel: failed to serialize state: {e}");
                }
            }
        }
    }

    // -- Identity ---------------------------------------------------------

    /// Set (or overwrite) the system identity.
    pub fn set_identity(&self, identity: SelfIdentity) {
        let mut inner = crate::lock_or_recover!(&self.inner, "intelligence");
        inner.identity = Some(identity);
        inner.last_update_ms = crate::shared::timestamps::now_ts_ms() as u64;
        // Use persist_inner to avoid re-entrant lock deadlock
        Self::persist_inner(&inner);
    }

    // -- Dynamic EMA Statistics -------------------------------------------

    /// Record an execution result for a capability and update EMA-based dynamic
    /// scoring.
    ///
    /// - `capability_name` — the name of the capability that was executed.
    /// - `success` — whether the execution succeeded.
    /// - `latency` — observed latency in milliseconds.
    ///
    /// EMA formula: `new_score = 0.3 * observed + 0.7 * old_score`.
    /// If no prior stats exist for the capability, the observed value is used
    /// directly as the starting point.
    pub fn record_execution_result(&self, capability_name: &str, success: bool, latency: u64) {
        const EMA_ALPHA: f64 = 0.3;
        let observed_success = if success { 1.0 } else { 0.0 };
        let now = crate::shared::timestamps::now_ts_ms() as u64;

        let mut inner = crate::lock_or_recover!(&self.inner, "intelligence");
        let stats = inner
            .capability_stats
            .entry(capability_name.to_string())
            .or_insert(CapabilityStats {
                effectiveness: observed_success,
                confidence: 0.0,
                avg_latency_ms: latency as f64,
                sample_count: 0,
                last_updated: now,
            });

        stats.sample_count = stats.sample_count.saturating_add(1);
        stats.last_updated = now;

        // Effectiveness: EMA of observed success (1.0 / 0.0).
        stats.effectiveness =
            EMA_ALPHA * observed_success + (1.0 - EMA_ALPHA) * stats.effectiveness;

        // Confidence: EMA adjusted by sample count.
        // observed_confidence approaches 1.0 as sample_count grows.
        let observed_confidence = (stats.sample_count as f64) / (stats.sample_count as f64 + 10.0);
        stats.confidence = EMA_ALPHA * observed_confidence + (1.0 - EMA_ALPHA) * stats.confidence;

        // Latency: EMA of observed latency.
        stats.avg_latency_ms =
            EMA_ALPHA * (latency as f64) + (1.0 - EMA_ALPHA) * stats.avg_latency_ms;

        debug!(
            capability = %capability_name,
            effectiveness = stats.effectiveness,
            confidence = stats.confidence,
            latency_ms = stats.avg_latency_ms,
            samples = stats.sample_count,
            "SelfModel: updated capability stats"
        );
        Self::persist_inner(&inner);
    }
}

// ---------------------------------------------------------------------------
// Trait implementations
// ---------------------------------------------------------------------------

impl ExecutionRecorder for SelfModelCore {
    fn record_execution_result(&self, capability_name: &str, success: bool, latency: u64) {
        // Delegate to the inherent method to avoid infinite recursion
        // (the trait method would otherwise call itself).
        SelfModelCore::record_execution_result(self, capability_name, success, latency);
    }
}
