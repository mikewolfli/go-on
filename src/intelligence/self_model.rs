//! BLUE38 F-GAP-21: Self-Model Core (M5 "自模型核心")
//!
//! Structured self-representation that tracks the system's own capabilities,
//! limitations, identity, and performance. All state is guarded behind
//! `Arc<Mutex<>>` for thread-safe access.

use anyhow::{bail, Result};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Data Structures
// ---------------------------------------------------------------------------

/// Identity of the system — who it is, who made it, and descriptive metadata.
#[derive(Debug, Clone)]
pub struct SelfIdentity {
    pub system_name: String,
    pub version: String,
    pub description: String,
    pub creator: String,
    pub created_ms: u64,
    pub tags: Vec<String>,
}

/// A known capability the system can perform, along with tracked effectiveness
/// and confidence metrics.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
            update_interval_ms: 60_000,   // 1 minute
            max_history: 1000,
            enable_performance_tracking: true,
        }
    }
}

/// A runtime summary / profile of the self-model's current state.
#[derive(Debug, Clone)]
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
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Inner {
    config: SelfModelConfig,
    identity: Option<SelfIdentity>,
    capabilities: Vec<SelfCapability>,
    limitations: Vec<SelfLimitation>,
    snapshots: Vec<SelfPerformanceSnapshot>,
    last_update_ms: u64,
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
                last_update_ms: now_ms(),
            })),
        }
    }

    // -- Identity ---------------------------------------------------------

    /// Set (or overwrite) the system identity.
    pub fn set_identity(&self, identity: SelfIdentity) {
        let mut inner = self.inner.lock().unwrap();
        inner.identity = Some(identity);
        inner.last_update_ms = now_ms();
    }

    /// Get the system identity, if one has been set.
    pub fn get_identity(&self) -> Option<SelfIdentity> {
        let inner = self.inner.lock().unwrap();
        inner.identity.clone()
    }

    // -- Capabilities -----------------------------------------------------

    /// Register a new capability.
    ///
    /// Returns an error if a capability with the same name already exists.
    pub fn register_capability(&self, capability: SelfCapability) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if inner.capabilities.iter().any(|c| c.name == capability.name) {
            bail!("capability '{}' is already registered", capability.name);
        }

        let max = inner.config.max_history;
        if inner.capabilities.len() >= max {
            // Evict the oldest capability (by last_verified_ms) to make room.
            inner.capabilities.sort_by(|a, b| a.last_verified_ms.cmp(&b.last_verified_ms));
            inner.capabilities.pop();
        }

        inner.capabilities.push(capability);
        inner.last_update_ms = now_ms();
        Ok(())
    }

    /// Update the effectiveness and confidence metrics for an existing capability.
    ///
    /// Returns an error if no capability with the given `name` exists.
    pub fn update_capability(&self, name: &str, effectiveness: f64, confidence: f64) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let cap = inner
            .capabilities
            .iter_mut()
            .find(|c| c.name == name)
            .ok_or_else(|| anyhow::anyhow!("capability '{}' not found", name))?;

        cap.effectiveness = effectiveness.clamp(0.0, 1.0);
        cap.confidence = confidence.clamp(0.0, 1.0);
        cap.usage_count = cap.usage_count.saturating_add(1);
        cap.last_verified_ms = now_ms();

        inner.last_update_ms = now_ms();
        Ok(())
    }

    /// Retrieve a capability by name.
    pub fn get_capability(&self, name: &str) -> Option<SelfCapability> {
        let inner = self.inner.lock().unwrap();
        inner.capabilities.iter().find(|c| c.name == name).cloned()
    }

    /// List all capabilities, optionally filtered by category.
    ///
    /// When `category_filter` is `None`, all capabilities are returned.
    /// When `Some(cat)`, only capabilities whose `category` equals the filter are returned.
    pub fn list_capabilities(&self, category_filter: Option<&str>) -> Vec<SelfCapability> {
        let inner = self.inner.lock().unwrap();
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
    pub fn add_limitation(&self, limitation: SelfLimitation) {
        let mut inner = self.inner.lock().unwrap();
        inner.limitations.push(limitation);
        inner.last_update_ms = now_ms();
    }

    /// Mark an existing limitation as acknowledged.
    ///
    /// Returns an error if no limitation with the given `name` exists.
    pub fn acknowledge_limitation(&self, name: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let lim = inner
            .limitations
            .iter_mut()
            .find(|l| l.name == name)
            .ok_or_else(|| anyhow::anyhow!("limitation '{}' not found", name))?;
        lim.is_acknowledged = true;
        inner.last_update_ms = now_ms();
        Ok(())
    }

    /// Retrieve a limitation by name.
    pub fn get_limitation(&self, name: &str) -> Option<SelfLimitation> {
        let inner = self.inner.lock().unwrap();
        inner.limitations.iter().find(|l| l.name == name).cloned()
    }

    /// List all limitations, optionally filtered to only acknowledged ones.
    ///
    /// When `acknowledged_only` is `false`, all limitations are returned.
    /// When `true`, only limitations where `is_acknowledged == true` are returned.
    pub fn list_limitations(&self, acknowledged_only: bool) -> Vec<SelfLimitation> {
        let inner = self.inner.lock().unwrap();
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
        let mut inner = self.inner.lock().unwrap();
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
    }

    /// Get the most recent `count` performance snapshots (newest first).
    ///
    /// If `count` is larger than the number of available snapshots, all are returned.
    pub fn performance_history(&self, count: usize) -> Vec<SelfPerformanceSnapshot> {
        let inner = self.inner.lock().unwrap();
        let len = inner.snapshots.len();
        let start = if count >= len { 0 } else { len - count };
        inner.snapshots[start..]
            .iter()
            .rev()
            .cloned()
            .collect()
    }

    /// Get the latest performance snapshot, if any.
    pub fn latest_performance(&self) -> Option<SelfPerformanceSnapshot> {
        let inner = self.inner.lock().unwrap();
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

    // -- Profile ----------------------------------------------------------

    /// Return a summary profile of the self-model's current state.
    pub fn profile(&self) -> SelfModelProfile {
        let inner = self.inner.lock().unwrap();

        let limitations_count = inner.limitations.len();
        let acknowledged_limitations = inner
            .limitations
            .iter()
            .filter(|l| l.is_acknowledged)
            .count();

        SelfModelProfile {
            identity_set: inner.identity.is_some(),
            capabilities_count: inner.capabilities.len(),
            limitations_count,
            acknowledged_limitations,
            performance_snapshots: inner.snapshots.len(),
            last_update_ms: inner.last_update_ms,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns the current timestamp in milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
    fn test_capability(name: &str, category: &str, effectiveness: f64, confidence: f64) -> SelfCapability {
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
    #[test]
    fn test_new_self_model_no_identity() {
        let core = SelfModelCore::new(test_config());
        assert!(core.get_identity().is_none());
        let p = core.profile();
        assert!(!p.identity_set);
        assert_eq!(p.capabilities_count, 0);
        assert_eq!(p.limitations_count, 0);
        assert_eq!(p.performance_snapshots, 0);
    }

    // -----------------------------------------------------------------------
    // Test 2: Setting and getting identity.
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
}
