//! BLUE38 F-GAP-21: Self-Model Core (M5 "自模型核心")
//!
//! Structured self-representation that tracks the system's own capabilities,
//! limitations, identity, and performance. All state is guarded behind
//! `Arc<Mutex<>>` for thread-safe access.

use crate::i18n::runtime::tf;
use crate::intelligence::lock_guard;
use crate::intelligence::now_ms;
use crate::shared::execution_recorder::ExecutionRecorder;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::debug;

// Lock a Mutex, recovering from poison with a log.
// Uses shared `crate::intelligence::lock_guard`.
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

/// A known capability the system can perform, along with tracked effectiveness
/// and confidence metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfCapability {
    pub name: String,
    pub description: String,
    /// Effectiveness score in [0.0, 1.0].
    pub effectiveness: f64,
    /// Confidence score in [0.0, 1.0].
    pub confidence: f64,
    /// Number of times this capability has been used.
    pub usage_count: u64,
    /// Timestamp (ms since epoch) of the last verification.
    pub last_verified_ms: u64,
    /// Category this capability belongs to.
    pub category: String,
    /// Names of capabilities that must be present for this one to function.
    pub prerequisites: Vec<String>,
}

/// A recognised limitation of the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfLimitation {
    pub name: String,
    pub description: String,
    /// Severity level: "Low", "Medium", "High", or "Critical".
    pub severity: String,
    /// Optional description of a known workaround.
    pub workaround: Option<String>,
    /// Timestamp (ms since epoch) when this limitation was discovered.
    pub discovered_ms: u64,
    /// Whether an operator has acknowledged this limitation.
    pub is_acknowledged: bool,
}

/// A point-in-time performance measurement of the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfPerformanceSnapshot {
    pub timestamp_ms: u64,
    pub avg_latency_ms: f64,
    pub p50_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub error_rate: f64,
    pub throughput: f64,
    pub agent_count: u32,
    pub tasks_processed: u64,
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

/// A runtime summary / profile of the self-model's current state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfModelProfile {
    /// Whether the system identity has been set.
    pub identity_set: bool,
    /// Total number of registered capabilities.
    pub capabilities_count: usize,
    /// Total number of reported limitations.
    pub limitations_count: usize,
    /// Number of limitations that have been acknowledged.
    pub acknowledged_limitations: usize,
    /// Number of performance snapshots retained.
    pub performance_snapshots: usize,
    /// Timestamp (ms) of the last update to the model.
    pub last_update_ms: u64,
    /// Number of capabilities with dynamic EMA stats.
    pub capabilities_with_stats: usize,
    /// Average dynamic effectiveness across all tracked capabilities.
    pub avg_dynamic_effectiveness: f64,
    /// Average dynamic confidence across all tracked capabilities.
    pub avg_dynamic_confidence: f64,
    /// Total execution samples collected across all capabilities.
    pub total_samples: u64,
    /// Average EMA latency (ms) across all tracked capabilities.
    pub avg_latency_ms: f64,
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct Inner {
    config: SelfModelConfig,
    identity: Option<SelfIdentity>,
    capabilities: Vec<SelfCapability>,
    limitations: Vec<SelfLimitation>,
    snapshots: Vec<SelfPerformanceSnapshot>,
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
    pub fn new(config: SelfModelConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                config,
                identity: None,
                capabilities: Vec::new(),
                limitations: Vec::new(),
                snapshots: Vec::new(),
                capability_stats: HashMap::new(),
                last_update_ms: now_ms(),
                persistence_path: None,
            })),
        }
    }

    /// Set a persistence path for auto-saving the self-model state.
    /// When set, every mutation automatically writes the full state to this file
    /// as pretty-printed JSON.
    pub fn with_persistence_path(self, path: PathBuf) -> Self {
        {
            let mut inner = lock_guard(&self.inner);
            inner.persistence_path = Some(path);
        }
        self
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

    /// Load a `SelfModelCore` from a JSON file previously written by a
    /// persisted self-model. The loaded instance will have its persistence path
    /// set to the same `path` so that subsequent mutations continue to save.
    pub fn load_from_file(path: PathBuf) -> Result<Self> {
        let data = fs::read_to_string(&path)?;
        let mut inner: Inner = serde_json::from_str(&data)?;
        inner.persistence_path = Some(path);
        inner.last_update_ms = now_ms();
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    // -- Identity ---------------------------------------------------------

    /// Set (or overwrite) the system identity.
    pub fn set_identity(&self, identity: SelfIdentity) {
        let mut inner = lock_guard(&self.inner);
        inner.identity = Some(identity);
        inner.last_update_ms = now_ms();
        // Use persist_inner to avoid re-entrant lock deadlock
        Self::persist_inner(&inner);
    }

    /// Get the system identity, if one has been set.
    pub fn get_identity(&self) -> Option<SelfIdentity> {
        let inner = lock_guard(&self.inner);
        inner.identity.clone()
    }

    // -- Capabilities -----------------------------------------------------

    /// Register a new capability.
    ///
    /// Returns an error if a capability with the same name already exists.
    pub fn register_capability(&self, capability: SelfCapability) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        if inner.capabilities.iter().any(|c| c.name == capability.name) {
            bail!(
                "{}",
                tf(
                    "error.capability_already_registered",
                    &[("name", &capability.name)]
                )
            );
        }

        let max = inner.config.max_history;
        if inner.capabilities.len() >= max {
            // Evict the oldest capability (by last_verified_ms) to make room.
            inner.capabilities.sort_by_key(|a| a.last_verified_ms);
            inner.capabilities.pop();
        }

        inner.capabilities.push(capability);
        inner.last_update_ms = now_ms();
        Self::persist_inner(&inner);
        Ok(())
    }

    /// Update the effectiveness and confidence metrics for an existing capability.
    ///
    /// Returns an error if no capability with the given `name` exists.
    pub fn update_capability(&self, name: &str, effectiveness: f64, confidence: f64) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let cap = inner
            .capabilities
            .iter_mut()
            .find(|c| c.name == name)
            .ok_or_else(|| {
                anyhow::anyhow!("{}", tf("error.capability_not_found", &[("name", name)]))
            })?;

        cap.effectiveness = effectiveness.clamp(0.0, 1.0);
        cap.confidence = confidence.clamp(0.0, 1.0);
        cap.usage_count = cap.usage_count.saturating_add(1);
        cap.last_verified_ms = now_ms();

        inner.last_update_ms = now_ms();
        Self::persist_inner(&inner);
        Ok(())
    }

    /// Retrieve a capability by name.
    pub fn get_capability(&self, name: &str) -> Option<SelfCapability> {
        let inner = lock_guard(&self.inner);
        inner.capabilities.iter().find(|c| c.name == name).cloned()
    }

    /// List all capabilities, optionally filtered by category.
    ///
    /// When `category_filter` is `None`, all capabilities are returned.
    /// When `Some(cat)`, only capabilities whose `category` equals the filter are returned.
    pub fn list_capabilities(&self, category_filter: Option<&str>) -> Vec<SelfCapability> {
        let inner = lock_guard(&self.inner);
        match category_filter {
            Some(cat) => inner
                .capabilities
                .iter()
                .filter(|c| c.category == cat)
                .cloned()
                .collect(),
            None => inner.capabilities.clone(),
        }
    }

    // -- Limitations ------------------------------------------------------

    /// Add a new limitation.
    ///
    /// If the number of limitations exceeds max_history, the oldest
    /// limitation (by discovered_ms) is evicted.
    pub fn add_limitation(&self, limitation: SelfLimitation) {
        let mut inner = lock_guard(&self.inner);
        inner.limitations.push(limitation);

        // Evict oldest limitation when max_history is exceeded.
        let max = inner.config.max_history;
        while inner.limitations.len() > max {
            inner.limitations.remove(0);
        }

        inner.last_update_ms = now_ms();
        Self::persist_inner(&inner);
    }

    /// Mark an existing limitation as acknowledged.
    ///
    /// Returns an error if no limitation with the given `name` exists.
    pub fn acknowledge_limitation(&self, name: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let lim = inner
            .limitations
            .iter_mut()
            .find(|l| l.name == name)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.self_model.limitation_not_found", &[("name", name)])
                )
            })?;
        lim.is_acknowledged = true;
        inner.last_update_ms = now_ms();
        Self::persist_inner(&inner);
        Ok(())
    }

    /// Retrieve a limitation by name.
    pub fn get_limitation(&self, name: &str) -> Option<SelfLimitation> {
        let inner = lock_guard(&self.inner);
        inner.limitations.iter().find(|l| l.name == name).cloned()
    }

    /// List all limitations, optionally filtered to only acknowledged ones.
    ///
    /// When `acknowledged_only` is `false`, all limitations are returned.
    /// When `true`, only limitations where `is_acknowledged == true` are returned.
    pub fn list_limitations(&self, acknowledged_only: bool) -> Vec<SelfLimitation> {
        let inner = lock_guard(&self.inner);
        if acknowledged_only {
            inner
                .limitations
                .iter()
                .filter(|l| l.is_acknowledged)
                .cloned()
                .collect()
        } else {
            inner.limitations.clone()
        }
    }

    // -- Performance ------------------------------------------------------

    /// Record a performance snapshot.
    ///
    /// If `enable_performance_tracking` is `false` in the config, the snapshot
    /// is silently discarded.
    pub fn record_performance(&self, snapshot: SelfPerformanceSnapshot) {
        let mut inner = lock_guard(&self.inner);
        if !inner.config.enable_performance_tracking {
            return;
        }

        inner.snapshots.push(snapshot);

        // Trim to max_history.
        let max = inner.config.max_history;
        while inner.snapshots.len() > max {
            inner.snapshots.remove(0);
        }

        inner.last_update_ms = now_ms();
        Self::persist_inner(&inner);
    }

    /// Get the most recent `count` performance snapshots (newest first).
    ///
    /// If `count` is larger than the number of available snapshots, all are returned.
    pub fn performance_history(&self, count: usize) -> Vec<SelfPerformanceSnapshot> {
        let inner = lock_guard(&self.inner);
        let len = inner.snapshots.len();
        let start = len.saturating_sub(count);
        inner.snapshots[start..].iter().rev().cloned().collect()
    }

    /// Get the latest performance snapshot, if any.
    pub fn latest_performance(&self) -> Option<SelfPerformanceSnapshot> {
        let inner = lock_guard(&self.inner);
        inner.snapshots.last().cloned()
    }

    /// Convenience method to record a performance snapshot from individual metrics.
    ///
    /// This computes p50 = p95 = p99 = `latency` (single-sample approximation),
    /// sets `error_rate`, `throughput`, `agent_count`, and `tasks_processed`
    /// as provided, and records a new `SelfPerformanceSnapshot`.
    pub fn refresh_performance_metrics(
        &self,
        latency: f64,
        error_rate: f64,
        throughput: f64,
        agent_count: u32,
        tasks: u64,
    ) {
        let snapshot = SelfPerformanceSnapshot {
            timestamp_ms: now_ms(),
            avg_latency_ms: latency,
            p50_latency_ms: latency,
            p95_latency_ms: latency,
            p99_latency_ms: latency,
            error_rate,
            throughput,
            agent_count,
            tasks_processed: tasks,
        };
        self.record_performance(snapshot);
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
        let now = now_ms();

        let mut inner = lock_guard(&self.inner);
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

    /// Get the dynamic EMA stats for a specific capability, if any.
    pub fn get_capability_stats(&self, name: &str) -> Option<CapabilityStats> {
        let inner = lock_guard(&self.inner);
        inner.capability_stats.get(name).cloned()
    }

    /// Return the list of capability names whose dynamic effectiveness score
    /// is below 0.5 — these need improvement or replacement.
    pub fn capability_gaps(&self) -> Vec<String> {
        let inner = lock_guard(&self.inner);
        let mut gaps: Vec<String> = inner
            .capability_stats
            .iter()
            .filter(|(_, s)| s.effectiveness < 0.5)
            .map(|(name, _)| name.clone())
            .collect();
        gaps.sort();
        gaps
    }

    // -- Profile ----------------------------------------------------------

    /// Return a summary profile of the self-model's current state, including
    /// live dynamic EMA metrics.
    pub fn profile(&self) -> SelfModelProfile {
        let inner = lock_guard(&self.inner);

        let limitations_count = inner.limitations.len();
        let acknowledged_limitations = inner
            .limitations
            .iter()
            .filter(|l| l.is_acknowledged)
            .count();

        let capabilities_with_stats = inner.capability_stats.len();
        let total_samples: u64 = inner
            .capability_stats
            .values()
            .map(|s| s.sample_count)
            .sum();

        let (avg_eff, avg_conf, avg_lat) = if capabilities_with_stats > 0 {
            let count = capabilities_with_stats as f64;
            let sum_eff: f64 = inner
                .capability_stats
                .values()
                .map(|s| s.effectiveness)
                .sum();
            let sum_conf: f64 = inner.capability_stats.values().map(|s| s.confidence).sum();
            let sum_lat: f64 = inner
                .capability_stats
                .values()
                .map(|s| s.avg_latency_ms)
                .sum();
            (sum_eff / count, sum_conf / count, sum_lat / count)
        } else {
            (0.0, 0.0, 0.0)
        };

        SelfModelProfile {
            identity_set: inner.identity.is_some(),
            capabilities_count: inner.capabilities.len(),
            limitations_count,
            acknowledged_limitations,
            performance_snapshots: inner.snapshots.len(),
            last_update_ms: inner.last_update_ms,
            capabilities_with_stats,
            avg_dynamic_effectiveness: avg_eff,
            avg_dynamic_confidence: avg_conf,
            total_samples,
            avg_latency_ms: avg_lat,
        }
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to build a default config for tests.
    fn test_config() -> SelfModelConfig {
        SelfModelConfig {
            update_interval_ms: 60_000,
            max_history: 100,
            enable_performance_tracking: true,
        }
    }

    /// Helper to construct a simple identity.
    fn test_identity(name: &str, version: &str) -> SelfIdentity {
        SelfIdentity {
            system_name: name.to_string(),
            version: version.to_string(),
            description: "Test identity".to_string(),
            creator: "test_runner".to_string(),
            created_ms: 1_000_000,
            tags: vec!["test".to_string()],
        }
    }

    /// Helper to construct a simple capability.
    fn test_capability(
        name: &str,
        category: &str,
        effectiveness: f64,
        confidence: f64,
    ) -> SelfCapability {
        SelfCapability {
            name: name.to_string(),
            description: format!("Capability {}", name),
            effectiveness,
            confidence,
            usage_count: 0,
            last_verified_ms: 0,
            category: category.to_string(),
            prerequisites: Vec::new(),
        }
    }

    /// Helper to construct a simple limitation.
    fn test_limitation(name: &str, severity: &str) -> SelfLimitation {
        SelfLimitation {
            name: name.to_string(),
            description: format!("Limitation {}", name),
            severity: severity.to_string(),
            workaround: None,
            discovered_ms: now_ms(),
            is_acknowledged: false,
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: Newly created self-model has no identity.
    // -----------------------------------------------------------------------
    // -----------------------------------------------------------------------
    // Test 1: Setting and getting identity.
    // -----------------------------------------------------------------------
    #[test]
    fn test_set_and_get_identity() {
        let core = SelfModelCore::new(test_config());
        let identity = test_identity("TestSys", "1.0.0");
        core.set_identity(identity.clone());

        let retrieved = core.get_identity().unwrap();
        assert_eq!(retrieved.system_name, "TestSys");
        assert_eq!(retrieved.version, "1.0.0");
        assert_eq!(retrieved.tags, vec!["test"]);

        let p = core.profile();
        assert!(p.identity_set);
    }

    // -----------------------------------------------------------------------
    // Test 3: Register a capability.
    // -----------------------------------------------------------------------
    #[test]
    fn test_register_capability() {
        let core = SelfModelCore::new(test_config());
        let cap = test_capability("translate", "nlp", 0.8, 0.9);
        core.register_capability(cap).unwrap();

        let caps = core.list_capabilities(None);
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].name, "translate");
        assert_eq!(caps[0].category, "nlp");
        assert!((caps[0].effectiveness - 0.8).abs() < 1e-9);
        assert!((caps[0].confidence - 0.9).abs() < 1e-9);
    }

    // -----------------------------------------------------------------------
    // Test 4: Update an existing capability.
    // -----------------------------------------------------------------------
    #[test]
    fn test_update_capability() {
        let core = SelfModelCore::new(test_config());
        core.register_capability(test_capability("qa", "testing", 0.5, 0.5))
            .unwrap();

        core.update_capability("qa", 0.95, 0.99).unwrap();

        let cap = core.get_capability("qa").unwrap();
        assert!((cap.effectiveness - 0.95).abs() < 1e-9);
        assert!((cap.confidence - 0.99).abs() < 1e-9);
        assert_eq!(cap.usage_count, 1);
        assert!(cap.last_verified_ms > 0);
    }

    // -----------------------------------------------------------------------
    // Test 5: Getting a nonexistent capability returns None.
    // -----------------------------------------------------------------------
    #[test]
    fn test_get_nonexistent_capability_fails() {
        let core = SelfModelCore::new(test_config());
        assert!(core.get_capability("does_not_exist").is_none());
    }

    // -----------------------------------------------------------------------
    // Test 6: Add a limitation.
    // -----------------------------------------------------------------------
    #[test]
    fn test_add_limitation() {
        let core = SelfModelCore::new(test_config());
        let lim = test_limitation("no_gpu", "High");
        core.add_limitation(lim);

        let lims = core.list_limitations(false);
        assert_eq!(lims.len(), 1);
        assert_eq!(lims[0].name, "no_gpu");
        assert_eq!(lims[0].severity, "High");
        assert!(!lims[0].is_acknowledged);
    }

    // -----------------------------------------------------------------------
    // Test 7: Acknowledge a limitation.
    // -----------------------------------------------------------------------
    #[test]
    fn test_acknowledge_limitation() {
        let core = SelfModelCore::new(test_config());
        core.add_limitation(test_limitation("rate_limit", "Medium"));

        core.acknowledge_limitation("rate_limit").unwrap();

        let lim = core.get_limitation("rate_limit").unwrap();
        assert!(lim.is_acknowledged);

        // Only acknowledged should appear in the filtered list.
        let acked = core.list_limitations(true);
        assert_eq!(acked.len(), 1);

        // All limitations still includes it.
        let all = core.list_limitations(false);
        assert_eq!(all.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Test 8: Record a performance snapshot.
    // -----------------------------------------------------------------------
    #[test]
    fn test_record_performance() {
        let core = SelfModelCore::new(test_config());
        let snap = SelfPerformanceSnapshot {
            timestamp_ms: now_ms(),
            avg_latency_ms: 150.0,
            p50_latency_ms: 120.0,
            p95_latency_ms: 300.0,
            p99_latency_ms: 450.0,
            error_rate: 0.02,
            throughput: 100.0,
            agent_count: 5,
            tasks_processed: 50,
        };
        core.record_performance(snap);

        let latest = core.latest_performance().unwrap();
        assert!((latest.avg_latency_ms - 150.0).abs() < 1e-9);
        assert!((latest.p50_latency_ms - 120.0).abs() < 1e-9);
        assert!((latest.p95_latency_ms - 300.0).abs() < 1e-9);
        assert!((latest.error_rate - 0.02).abs() < 1e-9);
        assert!((latest.throughput - 100.0).abs() < 1e-9);
        assert_eq!(latest.agent_count, 5);
        assert_eq!(latest.tasks_processed, 50);
    }

    // -----------------------------------------------------------------------
    // Test 9: Performance history returns snapshots in newest-first order.
    // -----------------------------------------------------------------------
    #[test]
    fn test_performance_history() {
        let core = SelfModelCore::new(test_config());
        assert!(core.performance_history(10).is_empty());

        for i in 0..5 {
            let snap = SelfPerformanceSnapshot {
                timestamp_ms: 1000 + i,
                avg_latency_ms: 50.0 + i as f64 * 10.0,
                p50_latency_ms: 40.0 + i as f64 * 10.0,
                p95_latency_ms: 80.0 + i as f64 * 10.0,
                p99_latency_ms: 100.0 + i as f64 * 10.0,
                error_rate: 0.01,
                throughput: 200.0,
                agent_count: 3,
                tasks_processed: 10 * (i + 1),
            };
            core.record_performance(snap);
        }

        // Request the 3 most recent.
        let recent = core.performance_history(3);
        assert_eq!(recent.len(), 3);

        // Newest first — timestamps should be descending.
        assert!(recent[0].timestamp_ms > recent[1].timestamp_ms);
        assert!(recent[1].timestamp_ms > recent[2].timestamp_ms);

        // The newest should have been the last recorded (index 4).
        assert_eq!(recent[0].tasks_processed, 50);
    }

    // -----------------------------------------------------------------------
    // Test 10: Latest performance with no snapshots.
    // -----------------------------------------------------------------------
    #[test]
    fn test_latest_performance() {
        let core = SelfModelCore::new(test_config());
        assert!(core.latest_performance().is_none());
    }

    // -----------------------------------------------------------------------
    // Test 11: Refresh performance metrics convenience method.
    // -----------------------------------------------------------------------
    #[test]
    fn test_refresh_performance_metrics() {
        let core = SelfModelCore::new(test_config());

        core.refresh_performance_metrics(100.0, 0.05, 250.0, 8, 120);

        let snap = core.latest_performance().unwrap();
        assert!((snap.avg_latency_ms - 100.0).abs() < 1e-9);
        assert!((snap.p50_latency_ms - 100.0).abs() < 1e-9);
        assert!((snap.p95_latency_ms - 100.0).abs() < 1e-9);
        assert!((snap.p99_latency_ms - 100.0).abs() < 1e-9);
        assert!((snap.error_rate - 0.05).abs() < 1e-9);
        assert!((snap.throughput - 250.0).abs() < 1e-9);
        assert_eq!(snap.agent_count, 8);
        assert_eq!(snap.tasks_processed, 120);
    }

    // -----------------------------------------------------------------------
    // Test 12: Profile accurately reflects registered state.
    // -----------------------------------------------------------------------
    #[test]
    fn test_profile_reflects_state() {
        let core = SelfModelCore::new(test_config());

        // Start empty.
        let p0 = core.profile();
        assert!(!p0.identity_set);
        assert_eq!(p0.capabilities_count, 0);
        assert_eq!(p0.limitations_count, 0);
        assert_eq!(p0.acknowledged_limitations, 0);
        assert_eq!(p0.performance_snapshots, 0);

        // Set identity.
        core.set_identity(test_identity("Sys", "2.0"));

        // Register capabilities.
        core.register_capability(test_capability("a", "x", 0.9, 0.9))
            .unwrap();
        core.register_capability(test_capability("b", "y", 0.7, 0.8))
            .unwrap();

        // Add limitations, acknowledge one.
        core.add_limitation(test_limitation("l1", "Low"));
        core.add_limitation(test_limitation("l2", "Critical"));
        core.acknowledge_limitation("l1").unwrap();

        // Record performance.
        core.refresh_performance_metrics(50.0, 0.01, 500.0, 4, 200);

        let p = core.profile();
        assert!(p.identity_set);
        assert_eq!(p.capabilities_count, 2);
        assert_eq!(p.limitations_count, 2);
        assert_eq!(p.acknowledged_limitations, 1);
        assert_eq!(p.performance_snapshots, 1);
        assert!(p.last_update_ms > 0);
    }

    // -----------------------------------------------------------------------
    // Additional: Register duplicate capability fails.
    // -----------------------------------------------------------------------
    #[test]
    fn test_register_duplicate_capability_fails() {
        let core = SelfModelCore::new(test_config());
        core.register_capability(test_capability("dedup", "x", 0.5, 0.5))
            .unwrap();
        let result = core.register_capability(test_capability("dedup", "x", 0.6, 0.6));
        assert!(result.is_err());
        assert_eq!(core.list_capabilities(None).len(), 1);
    }

    // -----------------------------------------------------------------------
    // Additional: Acknowledge nonexistent limitation fails.
    // -----------------------------------------------------------------------
    #[test]
    fn test_acknowledge_nonexistent_limitation_fails() {
        let core = SelfModelCore::new(test_config());
        let result = core.acknowledge_limitation("phantom");
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Additional: Performance tracking disabled discards snapshots.
    // -----------------------------------------------------------------------
    #[test]
    fn test_performance_tracking_disabled() {
        let config = SelfModelConfig {
            enable_performance_tracking: false,
            ..test_config()
        };
        let core = SelfModelCore::new(config);

        core.refresh_performance_metrics(10.0, 0.0, 1.0, 1, 5);
        assert!(core.latest_performance().is_none());
        assert!(core.performance_history(10).is_empty());
    }

    // -----------------------------------------------------------------------
    // Additional: List capabilities by category filter.
    // -----------------------------------------------------------------------
    #[test]
    fn test_list_capabilities_by_category() {
        let core = SelfModelCore::new(test_config());
        core.register_capability(test_capability("translate", "nlp", 0.8, 0.9))
            .unwrap();
        core.register_capability(test_capability("summarize", "nlp", 0.7, 0.8))
            .unwrap();
        core.register_capability(test_capability("codegen", "code", 0.6, 0.7))
            .unwrap();

        let nlp_caps = core.list_capabilities(Some("nlp"));
        assert_eq!(nlp_caps.len(), 2);

        let code_caps = core.list_capabilities(Some("code"));
        assert_eq!(code_caps.len(), 1);

        let all = core.list_capabilities(None);
        assert_eq!(all.len(), 3);
    }

    // -----------------------------------------------------------------------
    // Additional: EMA computation for capabilities.
    // -----------------------------------------------------------------------
    #[test]
    fn test_ema_effectiveness_computation() {
        let core = SelfModelCore::new(test_config());

        // First call: should initialize with the observed value.
        core.record_execution_result("cap-a", true, 100);
        let stats = core.get_capability_stats("cap-a").unwrap();
        // First sample: effectiveness = 1.0 (direct assignment)
        assert!((stats.effectiveness - 1.0).abs() < 1e-9);
        assert_eq!(stats.sample_count, 1);

        // Second call: EMA = 0.3 * 1.0 + 0.7 * 1.0 = 1.0
        core.record_execution_result("cap-a", true, 100);
        let stats = core.get_capability_stats("cap-a").unwrap();
        assert!((stats.effectiveness - 1.0).abs() < 1e-9);
        assert_eq!(stats.sample_count, 2);

        // Third call: failure → EMA = 0.3 * 0.0 + 0.7 * 1.0 = 0.7
        core.record_execution_result("cap-a", false, 200);
        let stats = core.get_capability_stats("cap-a").unwrap();
        assert!((stats.effectiveness - 0.7).abs() < 1e-9);
        assert_eq!(stats.sample_count, 3);

        // Fourth call: failure → EMA = 0.3 * 0.0 + 0.7 * 0.7 = 0.49
        core.record_execution_result("cap-a", false, 200);
        let stats = core.get_capability_stats("cap-a").unwrap();
        assert!((stats.effectiveness - 0.49).abs() < 1e-9);
        assert_eq!(stats.sample_count, 4);
    }

    // -----------------------------------------------------------------------
    // Additional: EMA latency computation.
    // -----------------------------------------------------------------------
    #[test]
    fn test_ema_latency_computation() {
        let core = SelfModelCore::new(test_config());

        core.record_execution_result("cap-b", true, 100);
        let stats = core.get_capability_stats("cap-b").unwrap();
        assert!((stats.avg_latency_ms - 100.0).abs() < 1e-9);

        // EMA = 0.3 * 200 + 0.7 * 100 = 60 + 70 = 130
        core.record_execution_result("cap-b", true, 200);
        let stats = core.get_capability_stats("cap-b").unwrap();
        assert!((stats.avg_latency_ms - 130.0).abs() < 1e-9);

        // EMA = 0.3 * 50 + 0.7 * 130 = 15 + 91 = 106
        core.record_execution_result("cap-b", true, 50);
        let stats = core.get_capability_stats("cap-b").unwrap();
        assert!((stats.avg_latency_ms - 106.0).abs() < 1e-9);
    }

    // -----------------------------------------------------------------------
    // Additional: Confidence grows with sample count.
    // -----------------------------------------------------------------------
    #[test]
    fn test_confidence_increases_with_samples() {
        let core = SelfModelCore::new(test_config());

        // Record many successes.
        for _ in 0..50 {
            core.record_execution_result("cap-c", true, 50);
        }

        let stats = core.get_capability_stats("cap-c").unwrap();
        // After 50 samples, confidence approaches sample_count/(sample_count+10) ≈ 0.83.
        assert!(stats.confidence > 0.8, "confidence={}", stats.confidence);
        assert!(stats.confidence <= 1.0);
        assert_eq!(stats.sample_count, 50);
    }

    // -----------------------------------------------------------------------
    // Additional: capability_gaps returns underperforming capabilities.
    // -----------------------------------------------------------------------
    #[test]
    fn test_capability_gaps() {
        let core = SelfModelCore::new(test_config());

        // Record a mix.
        core.record_execution_result("good", true, 10);
        core.record_execution_result("good", true, 10);
        core.record_execution_result("good", true, 10);

        core.record_execution_result("bad", false, 500);
        core.record_execution_result("bad", false, 500);
        core.record_execution_result("bad", false, 500); // effectiveness ≈ 0.343

        core.record_execution_result("okay", true, 100);
        core.record_execution_result("okay", false, 100);
        core.record_execution_result("okay", true, 100); // effectiveness = ?

        let gaps = core.capability_gaps();
        assert!(gaps.contains(&"bad".to_string()), "gaps={:?}", gaps);
        assert!(!gaps.contains(&"good".to_string()), "gaps={:?}", gaps);

        // 'okay' effectiveness after: 1.0, then 0.3*0+0.7*1=0.7, then 0.3*1+0.7*0.7=0.79
        // Should not be a gap.
        assert!(!gaps.contains(&"okay".to_string()), "gaps={:?}", gaps);
    }

    // -----------------------------------------------------------------------
    // Additional: Dynamic profile includes live EMA metrics.
    // -----------------------------------------------------------------------
    #[test]
    fn test_dynamic_profile_includes_ema_metrics() {
        let core = SelfModelCore::new(test_config());

        // Profile starts with zero stats.
        let p0 = core.profile();
        assert_eq!(p0.capabilities_with_stats, 0);
        assert_eq!(p0.total_samples, 0);
        assert!((p0.avg_dynamic_effectiveness - 0.0).abs() < 1e-9);
        assert!((p0.avg_dynamic_confidence - 0.0).abs() < 1e-9);
        assert!((p0.avg_latency_ms - 0.0).abs() < 1e-9);

        // Record some execution results.
        core.record_execution_result("alpha", true, 100);
        core.record_execution_result("alpha", true, 100);
        core.record_execution_result("beta", false, 300);

        let p = core.profile();
        assert_eq!(p.capabilities_with_stats, 2);
        assert_eq!(p.total_samples, 3);
        // avg_dynamic_effectiveness:
        //   alpha: 2 successes → effectiveness = 1.0
        //   beta:  1 failure   → effectiveness starts at observed 0.0
        //   average = (1.0 + 0.0) / 2 = 0.5
        assert!((p.avg_dynamic_effectiveness - 0.5).abs() < 1e-9);
        assert!(p.avg_dynamic_confidence > 0.0);
        // avg_latency_ms: alpha at 100, beta at 300 → (100.0 + 300.0) / 2 = 200.0
        assert!((p.avg_latency_ms - 200.0).abs() < 1e-9);
        assert!(p.last_update_ms > 0);
    }

    // -----------------------------------------------------------------------
    // Additional: capability_gaps is empty when no stats recorded.
    // -----------------------------------------------------------------------
    #[test]
    fn test_capability_gaps_empty_when_no_stats() {
        let core = SelfModelCore::new(test_config());
        let gaps = core.capability_gaps();
        assert!(gaps.is_empty());
    }

    // -----------------------------------------------------------------------
    // Additional: Stats are independent per capability.
    // -----------------------------------------------------------------------
    #[test]
    fn test_stats_independent_per_capability() {
        let core = SelfModelCore::new(test_config());

        core.record_execution_result("foo", true, 10);
        core.record_execution_result("bar", false, 500);

        let foo = core.get_capability_stats("foo").unwrap();
        let bar = core.get_capability_stats("bar").unwrap();

        assert!((foo.effectiveness - 1.0).abs() < 1e-9);
        assert!((foo.avg_latency_ms - 10.0).abs() < 1e-9);
        assert_eq!(foo.sample_count, 1);

        assert!((bar.effectiveness - 0.0).abs() < 1e-9);
        assert!((bar.avg_latency_ms - 500.0).abs() < 1e-9);
        assert_eq!(bar.sample_count, 1);
    }

    // -----------------------------------------------------------------------
    // Persistence: save-then-load maintains data integrity.
    // -----------------------------------------------------------------------
    #[test]
    fn test_persistence_save_and_load() {
        let dir = std::env::temp_dir().join(format!(
            "go_on_self_model_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("self_model.json");

        // Ensure the parent directory exists for file I/O.
        let _ = std::fs::create_dir_all(&dir);

        // --- Save phase ---
        let core = SelfModelCore::new(test_config()).with_persistence_path(path.clone());

        let identity = SelfIdentity {
            system_name: "persist-test".into(),
            version: "1.0".into(),
            description: "Persistence integrity test".into(),
            creator: "test_runner".into(),
            created_ms: 42_000,
            tags: vec!["alpha".into(), "beta".into()],
        };
        core.set_identity(identity.clone());

        let cap = SelfCapability {
            name: "test_cap".into(),
            description: "A test capability".into(),
            effectiveness: 0.9,
            confidence: 0.8,
            usage_count: 10,
            last_verified_ms: 100,
            category: "testing".into(),
            prerequisites: vec![],
        };
        core.register_capability(cap.clone()).unwrap();

        let lim = SelfLimitation {
            name: "test_lim".into(),
            description: "A test limitation".into(),
            severity: "Low".into(),
            workaround: Some("do something else".into()),
            discovered_ms: 200,
            is_acknowledged: false,
        };
        core.add_limitation(lim.clone());

        core.record_performance(SelfPerformanceSnapshot {
            timestamp_ms: 300,
            avg_latency_ms: 12.5,
            p50_latency_ms: 10.0,
            p95_latency_ms: 20.0,
            p99_latency_ms: 30.0,
            error_rate: 0.01,
            throughput: 100.0,
            agent_count: 4,
            tasks_processed: 500,
        });

        // Record an execution result (EMA stats).
        core.record_execution_result("test_cap", true, 15);

        drop(core); // force the file to be fully written

        // --- Load phase ---
        let loaded = SelfModelCore::load_from_file(path.clone()).unwrap();

        // Verify identity
        let loaded_id = loaded.get_identity().unwrap();
        assert_eq!(loaded_id.system_name, "persist-test");
        assert_eq!(loaded_id.version, "1.0");
        assert_eq!(loaded_id.tags, vec!["alpha", "beta"]);

        // Verify capabilities
        let loaded_cap = loaded.get_capability("test_cap").unwrap();
        assert_eq!(loaded_cap.name, "test_cap");
        assert!((loaded_cap.effectiveness - 0.9).abs() < 1e-9);
        assert!((loaded_cap.confidence - 0.8).abs() < 1e-9);

        // Verify limitations
        let loaded_lim = loaded.get_limitation("test_lim").unwrap();
        assert_eq!(loaded_lim.name, "test_lim");
        assert!(!loaded_lim.is_acknowledged);
        assert_eq!(loaded_lim.workaround, Some("do something else".into()));

        // Verify performance snapshots
        let perf = loaded.performance_history(1);
        assert_eq!(perf.len(), 1);
        assert!((perf[0].avg_latency_ms - 12.5).abs() < 1e-9);

        // Verify EMA stats
        let stats = loaded.get_capability_stats("test_cap").unwrap();
        assert_eq!(stats.sample_count, 1);
        assert!((stats.effectiveness - 1.0).abs() < 1e-9);

        // Verify the persistence path is set on the loaded instance
        // by performing a mutation and checking it writes without error.
        loaded.acknowledge_limitation("test_lim").unwrap();
        let lim_after = loaded.get_limitation("test_lim").unwrap();
        assert!(lim_after.is_acknowledged);

        // Clean up
        let _ = std::fs::remove_dir_all(&dir);
    }
}
