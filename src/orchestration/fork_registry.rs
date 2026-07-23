//! BLUE35 S10 + BLUE38 ARCH-05: Fork Registry — Sub-agent Process Isolation
//!
//! Tracks forked sub-agent executions and provides isolation boundaries
//! (sandbox level, resource limits, timeout policies) for each fork.
//!
//! Architectural capabilities:
//! - Process-level isolation tracking (register / find / cleanup / snapshot / restore / merge)
//! - Resource quota control (CPU / memory / wall-clock limits) for forked sub-agents
//! - Snapshot/restore serialization for checkpointing fork groups
//! - Merge logic for fan-out join semantics
//! - Thread-safe via `Arc<RwLock<...>>`
//! - Integration-ready for `AgentWorkerScheduler` fan-out (L2 scheduling)

use crate::agents::communication::budget::AgentExecutionBudget;
use crate::agents::communication::path::AgentPath;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::cmp;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

// ──────────────────────────────────────────────
// ForkSnapshot — serializable state snapshot
// ──────────────────────────────────────────────

/// A serializable snapshot of a fork's state at a point in time.
///
/// Contains the raw data payload (`Vec<u8>`), a Unix-epoch timestamp
/// (seconds with subsecond precision as `f64`), and an arbitrary set of
/// string-keyed labels for metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ForkSnapshot {
    /// Opaque payload data captured at snapshot time.
    pub data: Vec<u8>,
    /// Unix timestamp (seconds since epoch) with fractional subsecond precision.
    pub timestamp: f64,
    /// Arbitrary string-keyed metadata labels.
    pub labels: HashMap<String, String>,
}

impl ForkSnapshot {
    /// Construct a new `ForkSnapshot` with the current system time.
    pub fn new(data: Vec<u8>, labels: HashMap<String, String>) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        Self {
            data,
            timestamp,
            labels,
        }
    }

    /// Returns the timestamp as a `SystemTime` if it is representable.
    pub fn as_system_time(&self) -> Option<SystemTime> {
        let secs = self.timestamp.trunc() as u64;
        let nanos = (self.timestamp.fract() * 1_000_000_000.0) as u32;
        UNIX_EPOCH.checked_add(std::time::Duration::new(secs, nanos))
    }
}

// ──────────────────────────────────────────────
// ForkResourceQuota — CPU / memory / time limits
// ──────────────────────────────────────────────

/// Resource quota for a forked sub-agent.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ForkResourceQuota {
    /// Fractional CPU core limit (e.g. 1.5 = one and a half cores).
    pub cpu_quota: f64,
    /// Maximum resident memory in MB.
    pub memory_mb: u64,
    /// Maximum wall-clock time in seconds.
    pub time_limit_secs: u64,
}

impl Default for ForkResourceQuota {
    fn default() -> Self {
        Self {
            cpu_quota: 1.0,
            memory_mb: 512,
            time_limit_secs: 300,
        }
    }
}

impl ForkResourceQuota {
    /// Returns `true` if all limits are zero / unset.
    pub fn is_unlimited(&self) -> bool {
        self.cpu_quota <= 0.0 && self.memory_mb == 0 && self.time_limit_secs == 0
    }

    /// Merge another quota into this one, taking the stricter (minimum) of each limit.
    /// A zero/unset limit in `self` is replaced by `other`'s value (unless other is also zero).
    pub fn merge_strict(&mut self, other: &ForkResourceQuota) {
        self.cpu_quota = cap_min_f64(self.cpu_quota, other.cpu_quota);
        self.memory_mb = cap_min_u64(self.memory_mb, other.memory_mb);
        self.time_limit_secs = cap_min_u64(self.time_limit_secs, other.time_limit_secs);
    }
}

fn cap_min_f64(a: f64, b: f64) -> f64 {
    if a <= 0.0 {
        b
    } else if b <= 0.0 {
        a
    } else {
        a.min(b)
    }
}

fn cap_min_u64(a: u64, b: u64) -> u64 {
    if a == 0 {
        b
    } else if b == 0 {
        a
    } else {
        cmp::min(a, b)
    }
}

// ──────────────────────────────────────────────
// ForkJoinResult — fan-out merge outcome
// ──────────────────────────────────────────────

/// The outcome of joining (merging) multiple fork snapshots back together.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ForkJoinResult {
    /// Whether the join operation completed successfully.
    pub success: bool,
    /// The merged payload data.
    pub merged_data: Vec<u8>,
    /// Human-readable conflict descriptions (empty if no conflicts).
    pub conflicts: Vec<String>,
}

impl ForkJoinResult {
    /// Construct a successful join result with no conflicts.
    pub fn success(merged_data: Vec<u8>) -> Self {
        Self {
            success: true,
            merged_data,
            conflicts: Vec::new(),
        }
    }

    /// Construct a failed join result with the given conflict descriptions.
    pub fn failure(conflicts: Vec<String>) -> Self {
        Self {
            success: false,
            merged_data: Vec::new(),
            conflicts,
        }
    }

    /// Returns `true` if there were any conflicts during the join.
    pub fn has_conflicts(&self) -> bool {
        !self.conflicts.is_empty()
    }
}

// ──────────────────────────────────────────────
// ForkConfig — registry configuration
// ──────────────────────────────────────────────

/// Global configuration for the fork registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkConfig {
    /// Default resource quota applied when none is specified.
    pub default_quota: ForkResourceQuota,
    /// Maximum number of simultaneously tracked forks.
    pub max_forks: usize,
}

impl Default for ForkConfig {
    fn default() -> Self {
        Self {
            default_quota: ForkResourceQuota::default(),
            max_forks: 64,
        }
    }
}

// ──────────────────────────────────────────────
// ForkEntry — internal tracked fork state
// ──────────────────────────────────────────────

/// A registered fork entry stored in the registry (BLUE70 enhanced).
#[derive(Debug, Clone)]
pub struct ForkEntry {
    /// Unique identifier for this fork.
    pub fork_id: String,
    /// Identifier of the parent task that created this fork.
    pub parent_task_id: String,
    /// Resource quota assigned to this fork.
    pub quota: ForkResourceQuota,
    /// Optional snapshot recorded from this fork.
    pub snapshot: Option<ForkSnapshot>,
    /// Whether this fork has completed execution.
    pub completed: bool,

    // ── BLUE70: Agent communication fields ─────────────────────────
    /// Agent path in the CommunicationBus tree (None for non-CommunicationBus forks).
    pub agent_path: Option<AgentPath>,
    /// Parent agent path in the CommunicationBus tree.
    pub parent_agent_path: Option<AgentPath>,
    /// Execution budget (token ceiling, concurrency, etc.).
    pub budget: Option<AgentExecutionBudget>,
    /// Fork start timestamp (ms since epoch).
    pub started_at_ms: u64,
    /// Fork completion timestamp (ms since epoch).
    pub completed_at_ms: Option<u64>,
}

impl ForkEntry {
    pub fn new(fork_id: String, parent_task_id: String, quota: ForkResourceQuota) -> Self {
        Self {
            fork_id,
            parent_task_id,
            quota,
            snapshot: None,
            completed: false,
            agent_path: None,
            parent_agent_path: None,
            budget: None,
            started_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            completed_at_ms: None,
        }
    }

    /// Set the agent path (BLUE70 convenience method).
    pub fn with_agent_path(mut self, path: AgentPath) -> Self {
        self.agent_path = Some(path);
        self
    }

    /// Set the parent agent path (BLUE70 convenience method).
    pub fn with_parent_agent_path(mut self, path: AgentPath) -> Self {
        self.parent_agent_path = Some(path);
        self
    }

    /// Set the execution budget (BLUE70 convenience method).
    pub fn with_budget(mut self, budget: AgentExecutionBudget) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Mark this fork as completed.
    pub fn mark_completed(&mut self) {
        self.completed = true;
        self.completed_at_ms = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        );
    }
}

// ──────────────────────────────────────────────
// ForkRegistry — thread-safe sub-agent registry
// ──────────────────────────────────────────────

/// Thread-safe registry for tracking forked sub-agent executions.
///
/// All mutation is guarded by `Arc<RwLock<...>>`. Unique fork IDs are
/// generated from a monotonic counter combined with a timestamp prefix
/// so they are both human-readable and collision-resistant.
#[derive(Debug, Clone)]
pub struct ForkRegistry {
    inner: Arc<RwLock<ForkRegistryInner>>,
    /// Global atomic counter for unique ID generation (shared across clones).
    counter: Arc<AtomicU64>,
    /// Immutable configuration.
    config: Arc<ForkConfig>,
}

/// Inner state locked behind an RwLock.
#[derive(Debug)]
struct ForkRegistryInner {
    forks: HashMap<String, ForkEntry>,
}

impl ForkRegistry {
    /// Create a new `ForkRegistry` with the given configuration.
    pub fn new(config: ForkConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ForkRegistryInner {
                forks: HashMap::with_capacity(config.max_forks),
            })),
            counter: Arc::new(AtomicU64::new(0)),
            config: Arc::new(config),
        }
    }

    /// Create a new `ForkRegistry` with default configuration.
    pub fn default_with_max(max_forks: usize) -> Self {
        Self::new(ForkConfig {
            max_forks,
            ..ForkConfig::default()
        })
    }

    // ── ID generation ────────────────────────────────────────────

    /// Generate a unique fork ID from the current timestamp plus a
    /// monotonically increasing per-instance counter.
    fn generate_id(&self) -> String {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let seq = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("fork-{}-{}", ts, seq)
    }

    // ── Configuration ────────────────────────────────────────────

    /// Return a copy of the current configuration.
    pub fn config(&self) -> ForkConfig {
        (*self.config).clone()
    }

    /// The maximum number of forks this registry can hold.
    pub fn max_forks(&self) -> usize {
        self.config.max_forks
    }

    /// The default quota used when none is specified.
    pub fn default_quota(&self) -> ForkResourceQuota {
        self.config.default_quota
    }

    // ── Registration ────────────────────────────────────────────

    /// Register a new fork. Returns the generated unique ID, or `None`
    /// if the registry is at capacity.
    ///
    /// The default quota from `ForkConfig` is used.
    pub fn register(&self, parent_task_id: &str) -> Result<Option<String>> {
        self.register_with_quota(parent_task_id, self.config.default_quota)
    }

    /// Register a new fork with an explicit resource quota.
    /// Returns `None` if the registry is at capacity.
    pub fn register_with_quota(
        &self,
        parent_task_id: &str,
        quota: ForkResourceQuota,
    ) -> Result<Option<String>> {
        let fork_id = self.generate_id();
        let mut inner = self
            .inner
            .write()
            .map_err(|e| anyhow::anyhow!("ForkRegistry lock poisoned: {e}"))?;
        if inner.forks.len() >= self.config.max_forks {
            return Ok(None);
        }
        let entry = ForkEntry::new(fork_id.clone(), parent_task_id.to_string(), quota);
        inner.forks.insert(fork_id.clone(), entry);
        Ok(Some(fork_id))
    }

    // ── Lookup ──────────────────────────────────────────────────

    /// Find a fork by its ID.
    pub fn find(&self, id: &str) -> Result<Option<ForkEntry>> {
        let inner = self
            .inner
            .read()
            .map_err(|e| anyhow::anyhow!("ForkRegistry lock poisoned: {e}"))?;
        Ok(inner.forks.get(id).cloned())
    }

    /// List all currently tracked forks.
    pub fn list(&self) -> Result<Vec<ForkEntry>> {
        let inner = self
            .inner
            .read()
            .map_err(|e| anyhow::anyhow!("ForkRegistry lock poisoned: {e}"))?;
        Ok(inner.forks.values().cloned().collect())
    }

    // ── Removal ─────────────────────────────────────────────────

    /// Remove a fork by its ID. Returns `true` if the fork existed and
    /// was removed.
    pub fn remove(&self, id: &str) -> Result<bool> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| anyhow::anyhow!("ForkRegistry lock poisoned: {e}"))?;
        Ok(inner.forks.remove(id).is_some())
    }

    /// Remove all forks from the registry.
    pub fn clear(&self) -> Result<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| anyhow::anyhow!("ForkRegistry lock poisoned: {e}"))?;
        inner.forks.clear();
        Ok(())
    }

    // ── Size queries ────────────────────────────────────────────

    /// Number of forks currently tracked (both active and completed).
    pub fn len(&self) -> Result<usize> {
        let inner = self
            .inner
            .read()
            .map_err(|e| anyhow::anyhow!("ForkRegistry lock poisoned: {e}"))?;
        Ok(inner.forks.len())
    }

    /// Returns `true` if no forks are tracked.
    pub fn is_empty(&self) -> Result<bool> {
        let inner = self
            .inner
            .read()
            .map_err(|e| anyhow::anyhow!("ForkRegistry lock poisoned: {e}"))?;
        Ok(inner.forks.is_empty())
    }

    /// Number of forks that have not yet been marked completed.
    pub fn active_count(&self) -> Result<usize> {
        let inner = self
            .inner
            .read()
            .map_err(|e| anyhow::anyhow!("ForkRegistry lock poisoned: {e}"))?;
        Ok(inner.forks.values().filter(|e| !e.completed).count())
    }

    /// Number of forks that have been marked completed.
    pub fn completed_count(&self) -> Result<usize> {
        let inner = self
            .inner
            .read()
            .map_err(|e| anyhow::anyhow!("ForkRegistry lock poisoned: {e}"))?;
        Ok(inner.forks.values().filter(|e| e.completed).count())
    }

    // ── Status transitions ──────────────────────────────────────

    /// Mark a fork as completed.
    pub fn complete(&self, fork_id: &str) -> Result<bool> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| anyhow::anyhow!("ForkRegistry lock poisoned: {e}"))?;
        if let Some(entry) = inner.forks.get_mut(fork_id) {
            entry.completed = true;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // ── Snapshot support ────────────────────────────────────────

    /// Attach a snapshot to an existing fork. Returns `true` if the fork
    /// was found.
    pub fn attach_snapshot(&self, fork_id: &str, snapshot: ForkSnapshot) -> Result<bool> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| anyhow::anyhow!("ForkRegistry lock poisoned: {e}"))?;
        if let Some(entry) = inner.forks.get_mut(fork_id) {
            entry.snapshot = Some(snapshot);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Retrieve a clone of the snapshot attached to a fork, if any.
    pub fn get_snapshot(&self, fork_id: &str) -> Result<Option<ForkSnapshot>> {
        let inner = self
            .inner
            .read()
            .map_err(|e| anyhow::anyhow!("ForkRegistry lock poisoned: {e}"))?;
        Ok(inner.forks.get(fork_id).and_then(|e| e.snapshot.clone()))
    }

    /// Collect all snapshots from completed forks, grouped by parent task ID.
    /// Useful for fan-out merge logic.
    pub fn collect_completed_snapshots(&self) -> Result<HashMap<String, Vec<ForkSnapshot>>> {
        let inner = self
            .inner
            .read()
            .map_err(|e| anyhow::anyhow!("ForkRegistry lock poisoned: {e}"))?;
        let mut map: HashMap<String, Vec<ForkSnapshot>> = HashMap::new();
        for entry in inner.forks.values() {
            if entry.completed {
                if let Some(ref snap) = entry.snapshot {
                    map.entry(entry.parent_task_id.clone())
                        .or_default()
                        .push(snap.clone());
                }
            }
        }
        Ok(map)
    }

    /// Merge all snapshots for a given parent task ID into a single
    /// `ForkJoinResult`. The merge concatenates the data payloads of all
    /// snapshots in creation order. Conflicts are reported when more than
    /// one snapshot has overlapping label keys with differing values.
    pub fn merge_snapshots(&self, parent_task_id: &str) -> Result<ForkJoinResult> {
        let inner = self
            .inner
            .read()
            .map_err(|e| anyhow::anyhow!("ForkRegistry lock poisoned: {e}"))?;
        let mut snapshots: Vec<ForkSnapshot> = inner
            .forks
            .values()
            .filter(|e| e.parent_task_id == parent_task_id && e.completed)
            .filter_map(|e| e.snapshot.clone())
            .collect();

        if snapshots.is_empty() {
            return Ok(ForkJoinResult::success(Vec::new()));
        }

        // Sort by timestamp so the merge order is deterministic.
        snapshots.sort_by(|a, b| {
            a.timestamp
                .partial_cmp(&b.timestamp)
                .unwrap_or(cmp::Ordering::Equal)
        });

        // Concatenate data payloads.
        let total_len: usize = snapshots.iter().map(|s| s.data.len()).sum();
        let mut merged_data = Vec::with_capacity(total_len);
        let mut label_union: HashMap<String, String> = HashMap::new();
        let mut conflicts: Vec<String> = Vec::new();

        for snap in &snapshots {
            merged_data.extend_from_slice(&snap.data);
            for (k, v) in &snap.labels {
                match label_union.entry(k.clone()) {
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        if entry.get() != v {
                            conflicts.push(format!(
                                "Label '{}' conflict: '{}' vs '{}'",
                                k,
                                entry.get(),
                                v
                            ));
                            // Keep the later value (last writer wins for data).
                            entry.insert(v.clone());
                        }
                    }
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(v.clone());
                    }
                }
            }
        }

        Ok(ForkJoinResult {
            success: conflicts.is_empty(),
            merged_data,
            conflicts,
        })
    }

    // ── Cleanup ─────────────────────────────────────────────────

    /// Remove all completed forks from the registry. Returns the number
    /// of entries removed.
    pub fn reap_completed(&self) -> Result<usize> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| anyhow::anyhow!("ForkRegistry lock poisoned: {e}"))?;
        let before = inner.forks.len();
        inner.forks.retain(|_, e| !e.completed);
        Ok(before - inner.forks.len())
    }
}

// ──────────────────────────────────────────────
// IntoIterator support (consumes self)
// ──────────────────────────────────────────────

impl IntoIterator for ForkRegistry {
    type Item = (String, ForkEntry);
    type IntoIter = std::collections::hash_map::IntoIter<String, ForkEntry>;

    fn into_iter(self) -> Self::IntoIter {
        match Arc::into_inner(self.inner) {
            Some(inner) => {
                let inner = inner.into_inner().unwrap_or_else(|e| {
                    warn!("ForkRegistry lock poisoned in into_iter, recovering");
                    e.into_inner()
                });
                inner.forks.into_iter()
            }
            None => {
                warn!(
                    "ForkRegistry has multiple references in into_iter, returning empty iterator"
                );
                HashMap::new().into_iter()
            }
        }
    }
}

// ──────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn test_config() -> ForkConfig {
        ForkConfig {
            max_forks: 10,
            ..ForkConfig::default()
        }
    }

    #[test]
    fn test_register_and_find() {
        let reg = ForkRegistry::new(test_config());
        let fid = reg
            .register("parent-1")
            .expect("should register")
            .expect("should have fork id");
        assert!(!fid.is_empty());
        assert!(fid.starts_with("fork-"));

        let entry = reg
            .find(&fid)
            .expect("should find")
            .expect("should have entry");
        assert_eq!(entry.parent_task_id, "parent-1");
        assert!(!entry.completed);
    }

    #[test]
    fn test_register_with_quota() {
        let reg = ForkRegistry::new(test_config());
        let quota = ForkResourceQuota {
            cpu_quota: 2.0,
            memory_mb: 1024,
            time_limit_secs: 600,
        };
        let fid = reg
            .register_with_quota("parent-1", quota)
            .expect("should register")
            .expect("should have fork id");
        let entry = reg
            .find(&fid)
            .expect("should find")
            .expect("should have entry");
        assert_eq!(entry.quota, quota);
    }

    #[test]
    fn test_max_forks_limit() {
        let config = ForkConfig {
            max_forks: 2,
            ..ForkConfig::default()
        };
        let reg = ForkRegistry::new(config);

        assert!(reg.register("p1").expect("lock").is_some());
        assert!(reg.register("p2").expect("lock").is_some());
        assert!(reg.register("p3").expect("lock").is_none());
    }

    #[test]
    fn test_list_and_len() {
        let reg = ForkRegistry::new(test_config());
        assert!(reg.is_empty().expect("lock"));
        assert_eq!(reg.len().expect("lock"), 0);

        reg.register("p1").expect("lock");
        reg.register("p2").expect("lock");
        reg.register("p3").expect("lock");

        assert!(!reg.is_empty().expect("lock"));
        assert_eq!(reg.len().expect("lock"), 3);
        assert_eq!(reg.list().expect("lock").len(), 3);
    }

    #[test]
    fn test_remove() {
        let reg = ForkRegistry::new(test_config());
        let fid = reg
            .register("p1")
            .expect("should register")
            .expect("should have fork id");
        assert_eq!(reg.len().expect("lock"), 1);

        assert!(reg.remove(&fid).expect("lock"));
        assert_eq!(reg.len().expect("lock"), 0);

        // Removing again returns false.
        assert!(!reg.remove(&fid).expect("lock"));
    }

    #[test]
    fn test_clear() {
        let reg = ForkRegistry::new(test_config());
        reg.register("p1").expect("lock");
        reg.register("p2").expect("lock");
        assert_eq!(reg.len().expect("lock"), 2);

        reg.clear().expect("lock");
        assert_eq!(reg.len().expect("lock"), 0);
        assert!(reg.is_empty().expect("lock"));
    }

    #[test]
    fn test_complete_and_reap() {
        let reg = ForkRegistry::new(test_config());
        let fid1 = reg
            .register("p1")
            .expect("reg")
            .expect("should have fork id");
        let fid2 = reg
            .register("p2")
            .expect("reg")
            .expect("should have fork id");

        assert_eq!(reg.active_count().expect("lock"), 2);
        assert_eq!(reg.completed_count().expect("lock"), 0);

        assert!(reg.complete(&fid1).expect("lock"));
        assert_eq!(reg.active_count().expect("lock"), 1);
        assert_eq!(reg.completed_count().expect("lock"), 1);

        assert!(reg.complete(&fid2).expect("lock"));
        assert_eq!(reg.active_count().expect("lock"), 0);
        assert_eq!(reg.completed_count().expect("lock"), 2);

        // Reap removes completed forks.
        assert_eq!(reg.reap_completed().expect("lock"), 2);
        assert_eq!(reg.len().expect("lock"), 0);
    }

    #[test]
    fn test_attach_and_get_snapshot() {
        let reg = ForkRegistry::new(test_config());
        let fid = reg
            .register("p1")
            .expect("reg")
            .expect("should have fork id");

        let mut labels = HashMap::new();
        labels.insert("agent".to_string(), "worker-A".to_string());
        labels.insert("iteration".to_string(), "3".to_string());

        let snapshot = ForkSnapshot::new(vec![1, 2, 3, 4], labels.clone());
        assert!(reg.attach_snapshot(&fid, snapshot).expect("lock"));

        let retrieved = reg
            .get_snapshot(&fid)
            .expect("lock")
            .expect("should have snapshot");
        assert_eq!(retrieved.data, vec![1, 2, 3, 4]);
        assert_eq!(retrieved.labels.get("agent").unwrap(), "worker-A");
        assert!(retrieved.timestamp > 0.0);
    }

    #[test]
    fn test_collect_completed_snapshots() {
        let reg = ForkRegistry::new(ForkConfig {
            max_forks: 10,
            ..ForkConfig::default()
        });

        let fid1 = reg
            .register("parent-x")
            .expect("reg")
            .expect("should have fork id");
        let fid2 = reg
            .register("parent-x")
            .expect("reg")
            .expect("should have fork id");

        reg.attach_snapshot(&fid1, ForkSnapshot::new(vec![1], HashMap::new()))
            .expect("lock");
        reg.attach_snapshot(&fid2, ForkSnapshot::new(vec![2], HashMap::new()))
            .expect("lock");

        // Not yet completed — should not appear.
        assert!(reg.collect_completed_snapshots().expect("lock").is_empty());

        reg.complete(&fid1).expect("lock");
        reg.complete(&fid2).expect("lock");

        let map = reg.collect_completed_snapshots().expect("lock");
        assert_eq!(map.len(), 1);
        assert_eq!(map["parent-x"].len(), 2);
    }

    #[test]
    fn test_merge_snapshots_no_conflicts() {
        let reg = ForkRegistry::new(ForkConfig {
            max_forks: 10,
            ..ForkConfig::default()
        });

        let fid1 = reg
            .register("parent-m")
            .expect("reg")
            .expect("should have fork id");
        let fid2 = reg
            .register("parent-m")
            .expect("reg")
            .expect("should have fork id");

        let mut labels = HashMap::new();
        labels.insert("phase".to_string(), "test".to_string());

        reg.attach_snapshot(&fid1, ForkSnapshot::new(vec![1, 2], labels.clone()))
            .expect("lock");
        reg.attach_snapshot(&fid2, ForkSnapshot::new(vec![3, 4], labels))
            .expect("lock");

        reg.complete(&fid1).expect("lock");
        reg.complete(&fid2).expect("lock");

        let result = reg.merge_snapshots("parent-m").expect("lock");
        assert!(result.success);
        assert!(result.conflicts.is_empty());
        // Merge order is by timestamp (not fork ID), so use a set comparison.
        let mut merged = result.merged_data.clone();
        merged.sort_unstable();
        assert_eq!(merged, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_merge_snapshots_with_conflicts() {
        let reg = ForkRegistry::new(ForkConfig {
            max_forks: 10,
            ..ForkConfig::default()
        });

        let fid1 = reg
            .register("parent-c")
            .expect("reg")
            .expect("should have fork id");
        let fid2 = reg
            .register("parent-c")
            .expect("reg")
            .expect("should have fork id");

        let mut labels1 = HashMap::new();
        labels1.insert("result".to_string(), "ok".to_string());

        let mut labels2 = HashMap::new();
        labels2.insert("result".to_string(), "fail".to_string());

        reg.attach_snapshot(&fid1, ForkSnapshot::new(vec![1], labels1))
            .expect("lock");
        reg.attach_snapshot(&fid2, ForkSnapshot::new(vec![2], labels2))
            .expect("lock");

        reg.complete(&fid1).expect("lock");
        reg.complete(&fid2).expect("lock");

        let result = reg.merge_snapshots("parent-c").expect("lock");
        assert!(!result.success);
        assert!(result.has_conflicts());
        assert_eq!(result.conflicts.len(), 1);
        assert!(result.conflicts[0].contains("result"));
    }

    #[test]
    fn test_merge_empty() {
        let reg = ForkRegistry::new(test_config());
        let result = reg.merge_snapshots("nonexistent").expect("lock");
        assert!(result.success);
        assert!(result.merged_data.is_empty());
    }

    #[test]
    fn test_thread_safety() {
        let reg = ForkRegistry::new(ForkConfig {
            max_forks: 100,
            ..ForkConfig::default()
        });

        let mut handles = Vec::new();
        for i in 0..10 {
            let r = reg.clone();
            handles.push(thread::spawn(move || {
                for j in 0..10 {
                    let fid = r
                        .register(&format!("thread-{}-{}", i, j))
                        .expect("lock")
                        .expect("should register");
                    assert!(r.find(&fid).expect("lock").is_some());
                }
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }

        assert_eq!(reg.len().expect("lock"), 100);
    }

    #[test]
    fn test_default_quota_values() {
        let quota = ForkResourceQuota::default();
        assert!((quota.cpu_quota - 1.0).abs() < f64::EPSILON);
        assert_eq!(quota.memory_mb, 512);
        assert_eq!(quota.time_limit_secs, 300);
    }

    #[test]
    fn test_fork_snapshot_new() {
        let snap = ForkSnapshot::new(vec![0xAB, 0xCD], HashMap::new());
        assert_eq!(snap.data, vec![0xAB, 0xCD]);
        assert!(snap.timestamp > 1_700_000_000.0); // reasonable lower bound for 2024+
        assert!(snap.labels.is_empty());
    }

    #[test]
    fn test_fork_join_result_failure() {
        let result =
            ForkJoinResult::failure(vec!["key mismatch".to_string(), "timeout".to_string()]);
        assert!(!result.success);
        assert!(result.has_conflicts());
        assert_eq!(result.conflicts.len(), 2);
    }

    #[test]
    fn test_fork_resource_quota_merge_strict() {
        let mut a = ForkResourceQuota {
            cpu_quota: 4.0,
            memory_mb: 2048,
            time_limit_secs: 600,
        };
        let b = ForkResourceQuota {
            cpu_quota: 2.0,
            memory_mb: 1024,
            time_limit_secs: 0, // 0 means "no limit set", use other
        };
        a.merge_strict(&b);
        assert!((a.cpu_quota - 2.0).abs() < f64::EPSILON);
        assert_eq!(a.memory_mb, 1024);
        assert_eq!(a.time_limit_secs, 600); // kept from a since b is 0
    }

    #[test]
    fn test_fork_snapshot_serde_roundtrip() {
        let mut labels = HashMap::new();
        labels.insert("env".to_string(), "staging".to_string());
        let snap = ForkSnapshot::new(vec![10, 20, 30], labels);
        let json = serde_json::to_string(&snap).expect("serialize");
        let deserialized: ForkSnapshot = serde_json::from_str(&json).expect("deserialize");
        // f64 timestamps at ~1.7×10⁹ magnitude can lose the last ULP through
        // serde_json's shortest-representation encoding; compare fields
        // individually and use an epsilon for the floating-point timestamp.
        assert_eq!(snap.data, deserialized.data);
        assert_eq!(snap.labels, deserialized.labels);
        assert!(
            (snap.timestamp - deserialized.timestamp).abs() < 1e-4,
            "timestamp precision lost beyond tolerance: {} vs {}",
            snap.timestamp,
            deserialized.timestamp
        );
    }

    #[test]
    fn test_into_iter() {
        let reg = ForkRegistry::new(ForkConfig {
            max_forks: 10,
            ..ForkConfig::default()
        });
        reg.register("a").expect("lock");
        reg.register("b").expect("lock");

        let collected: Vec<_> = reg.into_iter().collect();
        assert_eq!(collected.len(), 2);
    }

    #[test]
    fn test_complete_nonexistent() {
        let reg = ForkRegistry::new(test_config());
        assert!(!reg.complete("nonexistent").expect("lock"));
    }

    #[test]
    fn test_attach_snapshot_nonexistent() {
        let reg = ForkRegistry::new(test_config());
        assert!(!reg
            .attach_snapshot("nope", ForkSnapshot::new(vec![], HashMap::new()))
            .expect("lock"));
    }
}
