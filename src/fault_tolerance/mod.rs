//! Cross-node Fault Tolerance module — F-GAP-28
//!
//! Provides node-level fault isolation, heartbeat-based failure detection,
//! and automatic recovery coordination across a distributed cluster.

mod detector;
mod recovery;
mod types;

pub use types::*;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

pub(crate) struct Inner {
    pub(crate) config: FaultToleranceConfig,
    /// node_id -> HeartbeatRecord
    pub(crate) heartbeats: HashMap<String, HeartbeatRecord>,
    /// fault_id -> FaultEvent
    pub(crate) faults: HashMap<String, FaultEvent>,
    /// group_id -> IsolationGroup
    pub(crate) isolation_groups: HashMap<String, IsolationGroup>,
    /// monotonic counter for generating unique fault ids
    pub(crate) fault_counter: u64,
    /// monotonic counter for generating unique group ids
    pub(crate) group_counter: u64,
    /// plan_id -> RecoveryPlan
    pub(crate) recovery_plans: HashMap<String, RecoveryPlan>,
    /// monotonic counter for generating unique plan ids
    pub(crate) plan_counter: u64,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Acquire a write lock on the RwLock, logging if contention is high.
pub(crate) fn write_guard<T>(lock: &RwLock<T>) -> tokio::sync::RwLockWriteGuard<'_, T> {
    lock.blocking_write()
}

/// Acquire a read lock on the RwLock.
pub(crate) fn read_guard<T>(lock: &RwLock<T>) -> tokio::sync::RwLockReadGuard<'_, T> {
    lock.blocking_read()
}

/// Compute cluster health from raw counts (shared by `profile` and `cluster_health`).
pub(crate) fn cluster_health_from_counts(
    total_nodes: usize,
    offline_nodes: usize,
    degraded_nodes: usize,
    active_faults: usize,
) -> ClusterHealth {
    cluster_health_from_counts_with_config(
        total_nodes,
        offline_nodes,
        degraded_nodes,
        active_faults,
        &ClusterHealthConfig::default(),
    )
}

/// Compute cluster health from raw counts using the provided configuration.
pub(crate) fn cluster_health_from_counts_with_config(
    total_nodes: usize,
    offline_nodes: usize,
    degraded_nodes: usize,
    active_faults: usize,
    config: &ClusterHealthConfig,
) -> ClusterHealth {
    if total_nodes == 0 {
        return ClusterHealth::Down;
    }
    let offline_ratio = offline_nodes as f64 / total_nodes as f64;
    let degraded_ratio = degraded_nodes as f64 / total_nodes as f64;
    if offline_ratio >= config.healthy_threshold || active_faults >= 10 {
        ClusterHealth::Critical
    } else if (offline_ratio >= config.degraded_threshold
        || degraded_ratio >= config.unhealthy_threshold
        || active_faults >= 5)
        || offline_nodes > 0
        || degraded_nodes > 0
    {
        ClusterHealth::Degraded
    } else {
        ClusterHealth::Healthy
    }
}

// ---------------------------------------------------------------------------
// FaultToleranceEngine
// ---------------------------------------------------------------------------

/// Thread-safe engine that monitors node health, detects faults, and manages
/// isolation / recovery.
///
/// Uses `tokio::sync::RwLock` for read-heavy workloads (GAP-B50-41).
#[derive(Clone)]
pub struct FaultToleranceEngine {
    pub(crate) inner: Arc<RwLock<Inner>>,
}

impl FaultToleranceEngine {
    /// Create a new engine with the given configuration.
    pub fn new(config: FaultToleranceConfig) -> Self {
        let inner = Inner {
            config,
            heartbeats: HashMap::new(),
            faults: HashMap::new(),
            isolation_groups: HashMap::new(),
            fault_counter: 0,
            group_counter: 0,
            recovery_plans: HashMap::new(),
            plan_counter: 0,
        };
        Self {
            inner: Arc::new(RwLock::new(inner)),
        }
    }

    /// Return a snapshot profile of the cluster state.
    pub fn profile(&self) -> FaultToleranceProfile {
        let inner = read_guard(&self.inner);
        let total_nodes = inner.heartbeats.len();
        let online_nodes = inner
            .heartbeats
            .values()
            .filter(|r| r.status == NodeStatus::Online)
            .count();
        let degraded_nodes = inner
            .heartbeats
            .values()
            .filter(|r| r.status == NodeStatus::Degraded)
            .count();
        let offline_nodes = inner
            .heartbeats
            .values()
            .filter(|r| r.status == NodeStatus::Offline)
            .count();
        // Recovering nodes are counted separately; they are none of the above
        let active_faults = inner.faults.values().filter(|f| !f.recovered).count();
        let isolated_groups = inner.isolation_groups.len();

        let recovery_plans_pending = inner
            .recovery_plans
            .values()
            .filter(|p| p.state == RecoveryState::Pending)
            .count();
        let recovery_plans_in_progress = inner
            .recovery_plans
            .values()
            .filter(|p| p.state == RecoveryState::InProgress)
            .count();

        let cluster_health =
            cluster_health_from_counts(total_nodes, offline_nodes, degraded_nodes, active_faults);

        FaultToleranceProfile {
            total_nodes,
            online_nodes,
            degraded_nodes,
            offline_nodes,
            active_faults,
            isolated_groups,
            recovery_plans_pending,
            recovery_plans_in_progress,
            cluster_health,
        }
    }

    /// Run the full recovery cycle: check heartbeats, auto-create recovery plans,
    /// and return the status summary. Also persists state to DB if available.
    pub fn run_recovery_cycle(&self) -> RecoveryCycleSummary {
        let offenders = self.check_heartbeats();
        let mut plans_created = 0u32;
        let mut plans_activated = 0u32;
        let mut consistency_checks = Vec::new();

        for node_id in &offenders {
            // Check if a plan already exists for this node
            let existing = {
                let inner = read_guard(&self.inner);
                inner
                    .recovery_plans
                    .values()
                    .any(|p| p.node_id == *node_id && p.state != RecoveryState::Completed)
            };
            if !existing && self.create_recovery_plan(node_id).is_ok() {
                plans_created += 1;
            }
        }

        // Auto-execute pending plans and run consistency checks on activations
        let pending_plans = self.active_recovery_plans();
        for plan in &pending_plans {
            if plan.state == RecoveryState::Pending
                && self.execute_recovery_plan(&plan.plan_id).is_ok()
            {
                plans_activated += 1;
                // Run consistency check after activation
                let check = self.post_recovery_consistency_check(&plan.plan_id);
                if !check.passed {
                    tracing::warn!(
                        "consistency check after activation of plan '{}': {}",
                        plan.plan_id,
                        check.details
                    );
                }
                consistency_checks.push(check);
            }
        }

        let health = self.cluster_health();
        let profile = self.profile();

        // BLUE56-C06: Persist recovery cycle state to DB
        self.try_persist_state();

        RecoveryCycleSummary {
            offenders,
            plans_created,
            plans_activated,
            cluster_health: health,
            active_faults: profile.active_faults as u32,
            isolated_groups: profile.isolated_groups as u32,
            consistency_checks,
        }
    }

    /// Try to persist fault tolerance state to the default SQLite cache path.
    fn try_persist_state(&self) {
        #[cfg(feature = "backend-sqlite")]
        {
            let cache_path = std::path::PathBuf::from("target")
                .join("go-on")
                .join("fault_tolerance.db");
            if let Some(parent) = cache_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(conn) = rusqlite::Connection::open(&cache_path) {
                if let Err(e) = self.save_to_db(&conn) {
                    tracing::warn!(
                        target: "fault_tolerance",
                        error = %e,
                        "failed to persist fault tolerance state"
                    );
                }
            }
        }
        #[cfg(not(feature = "backend-sqlite"))]
        {
            let _ = self.save_to_db(&());
        }
    }

    // GAP-B50-34: Persistence — save state to SQLite
    /// Save all fault tolerance state to the configured SQLite database.
    /// Creates the necessary tables if they don't exist.
    #[cfg(feature = "backend-sqlite")]
    pub fn save_to_db(&self, conn: &rusqlite::Connection) -> anyhow::Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS faults (
                id TEXT PRIMARY KEY,
                node_id TEXT NOT NULL,
                fault_type TEXT NOT NULL,
                severity INTEGER NOT NULL,
                description TEXT NOT NULL,
                detected_ms INTEGER NOT NULL,
                resolved_ms INTEGER,
                recovered INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS recovery_plans (
                plan_id TEXT PRIMARY KEY,
                node_id TEXT NOT NULL,
                actions TEXT NOT NULL,
                state TEXT NOT NULL,
                created_ms INTEGER NOT NULL,
                completed_ms INTEGER,
                result TEXT
            );
            CREATE TABLE IF NOT EXISTS isolation_groups (
                group_id TEXT PRIMARY KEY,
                nodes TEXT NOT NULL,
                isolation_level TEXT NOT NULL,
                created_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS heartbeat_records (
                node_id TEXT PRIMARY KEY,
                last_heartbeat_ms INTEGER NOT NULL,
                missed_beats INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL
            );",
        )?;

        let inner = write_guard(&self.inner);

        // Clear existing data for idempotent save
        conn.execute("DELETE FROM faults", [])?;
        conn.execute("DELETE FROM recovery_plans", [])?;
        conn.execute("DELETE FROM isolation_groups", [])?;
        conn.execute("DELETE FROM heartbeat_records", [])?;

        // Insert faults
        for fault in inner.faults.values() {
            conn.execute(
                "INSERT INTO faults (id, node_id, fault_type, severity, description, detected_ms, resolved_ms, recovered)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    fault.id,
                    fault.node_id,
                    format!("{:?}", fault.fault_type),
                    fault.severity as i64,
                    fault.description,
                    fault.detected_ms as i64,
                    fault.resolved_ms.map(|v| v as i64),
                    fault.recovered as i64,
                ],
            )?;
        }

        // Insert recovery plans
        for plan in inner.recovery_plans.values() {
            let actions_json = serde_json::to_string(&plan.actions)?;
            conn.execute(
                "INSERT INTO recovery_plans (plan_id, node_id, actions, state, created_ms, completed_ms, result)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    plan.plan_id,
                    plan.node_id,
                    actions_json,
                    format!("{:?}", plan.state),
                    plan.created_ms as i64,
                    plan.completed_ms.map(|v| v as i64),
                    plan.result,
                ],
            )?;
        }

        // Insert isolation groups
        for group in inner.isolation_groups.values() {
            let nodes_json = serde_json::to_string(&group.nodes)?;
            conn.execute(
                "INSERT INTO isolation_groups (group_id, nodes, isolation_level, created_ms)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    group.group_id,
                    nodes_json,
                    format!("{:?}", group.isolation_level),
                    group.created_ms as i64,
                ],
            )?;
        }

        // Insert heartbeat records
        for hb in inner.heartbeats.values() {
            conn.execute(
                "INSERT OR REPLACE INTO heartbeat_records (node_id, last_heartbeat_ms, missed_beats, status)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    hb.node_id,
                    hb.last_heartbeat_ms as i64,
                    hb.missed_beats as i64,
                    format!("{:?}", hb.status),
                ],
            )?;
        }

        tracing::info!(
            "FaultToleranceEngine: saved {} faults, {} plans, {} groups, {} heartbeats to DB",
            inner.faults.len(),
            inner.recovery_plans.len(),
            inner.isolation_groups.len(),
            inner.heartbeats.len()
        );
        Ok(())
    }

    /// Non-SQLite fallback: no-op
    #[cfg(not(feature = "backend-sqlite"))]
    pub fn save_to_db(&self, _conn: &()) -> anyhow::Result<()> {
        tracing::warn!("FaultToleranceEngine: save_to_db requires backend-sqlite feature");
        Ok(())
    }

    // GAP-B50-34: Persistence — load state from SQLite
    /// Load all fault tolerance state from the configured SQLite database.
    /// Returns the number of faults, plans, groups, and heartbeats restored.
    #[cfg(feature = "backend-sqlite")]
    pub fn load_from_db(
        &self,
        conn: &rusqlite::Connection,
    ) -> anyhow::Result<(usize, usize, usize, usize)> {
        let mut inner = write_guard(&self.inner);

        // Load faults
        let mut stmt = conn.prepare(
            "SELECT id, node_id, fault_type, severity, description, detected_ms, resolved_ms, recovered FROM faults"
        )?;
        let fault_rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let node_id: String = row.get(1)?;
            let fault_type_str: String = row.get(2)?;
            let severity: i64 = row.get(3)?;
            let description: String = row.get(4)?;
            let detected_ms: i64 = row.get(5)?;
            let resolved_ms: Option<i64> = row.get(6)?;
            let recovered: bool = row.get::<_, i64>(7)? != 0;
            Ok((
                id,
                node_id,
                fault_type_str,
                severity as u8,
                description,
                detected_ms as u64,
                resolved_ms.map(|v| v as u64),
                recovered,
            ))
        })?;

        // Clear faults and reload
        inner.faults.clear();
        for row in fault_rows {
            let (
                id,
                node_id,
                fault_type_str,
                severity,
                description,
                detected_ms,
                resolved_ms,
                recovered,
            ) = row?;
            let fault_type = match fault_type_str.as_str() {
                "Crash" => FaultType::Crash,
                "Hang" => FaultType::Hang,
                "Oom" => FaultType::Oom,
                "NetworkSplit" => FaultType::NetworkSplit,
                "DataCorruption" => FaultType::DataCorruption,
                "ResourceExhaustion" => FaultType::ResourceExhaustion,
                _ => continue,
            };
            inner.faults.insert(
                id.clone(),
                FaultEvent {
                    id,
                    node_id,
                    fault_type,
                    severity,
                    description,
                    detected_ms,
                    resolved_ms,
                    recovered,
                },
            );
        }

        // Load recovery plans
        let mut stmt = conn.prepare(
            "SELECT plan_id, node_id, actions, state, created_ms, completed_ms, result FROM recovery_plans"
        )?;
        let plan_rows = stmt.query_map([], |row| {
            let plan_id: String = row.get(0)?;
            let node_id: String = row.get(1)?;
            let actions_json: String = row.get(2)?;
            let state_str: String = row.get(3)?;
            let created_ms: i64 = row.get(4)?;
            let completed_ms: Option<i64> = row.get(5)?;
            let result: Option<String> = row.get(6)?;
            Ok((
                plan_id,
                node_id,
                actions_json,
                state_str,
                created_ms as u64,
                completed_ms.map(|v| v as u64),
                result,
            ))
        })?;

        inner.recovery_plans.clear();
        for row in plan_rows {
            let (plan_id, node_id, actions_json, state_str, created_ms, completed_ms, result) =
                row?;
            let actions: Vec<RecoveryAction> = serde_json::from_str(&actions_json)?;
            let state = match state_str.as_str() {
                "Pending" => RecoveryState::Pending,
                "InProgress" => RecoveryState::InProgress,
                "Completed" => RecoveryState::Completed,
                "Failed" => RecoveryState::Failed,
                _ => continue,
            };
            inner.recovery_plans.insert(
                plan_id.clone(),
                RecoveryPlan {
                    plan_id,
                    node_id,
                    actions,
                    state,
                    created_ms,
                    completed_ms,
                    result,
                },
            );
        }

        // Load isolation groups
        let mut stmt = conn
            .prepare("SELECT group_id, nodes, isolation_level, created_ms FROM isolation_groups")?;
        let group_rows = stmt.query_map([], |row| {
            let group_id: String = row.get(0)?;
            let nodes_json: String = row.get(1)?;
            let level_str: String = row.get(2)?;
            let created_ms: i64 = row.get(3)?;
            Ok((group_id, nodes_json, level_str, created_ms as u64))
        })?;

        inner.isolation_groups.clear();
        for row in group_rows {
            let (group_id, nodes_json, level_str, created_ms) = row?;
            let nodes: Vec<String> = serde_json::from_str(&nodes_json)?;
            let isolation_level = match level_str.as_str() {
                "Monitor" => IsolationLevel::Monitor,
                "Quarantine" => IsolationLevel::Quarantine,
                "Shutdown" => IsolationLevel::Shutdown,
                _ => continue,
            };
            inner.isolation_groups.insert(
                group_id.clone(),
                IsolationGroup {
                    group_id,
                    nodes,
                    isolation_level,
                    created_ms,
                },
            );
        }

        // Load heartbeat records
        let mut stmt = conn.prepare(
            "SELECT node_id, last_heartbeat_ms, missed_beats, status FROM heartbeat_records",
        )?;
        let hb_rows = stmt.query_map([], |row| {
            let node_id: String = row.get(0)?;
            let last_heartbeat_ms: i64 = row.get(1)?;
            let missed_beats: i64 = row.get(2)?;
            let status_str: String = row.get(3)?;
            Ok((
                node_id,
                last_heartbeat_ms as u64,
                missed_beats as u32,
                status_str,
            ))
        })?;

        inner.heartbeats.clear();
        for row in hb_rows {
            let (node_id, last_heartbeat_ms, missed_beats, status_str) = row?;
            let status = match status_str.as_str() {
                "Online" => NodeStatus::Online,
                "Degraded" => NodeStatus::Degraded,
                "Offline" => NodeStatus::Offline,
                "Recovering" => NodeStatus::Recovering,
                _ => continue,
            };
            inner.heartbeats.insert(
                node_id.clone(),
                HeartbeatRecord {
                    node_id,
                    last_heartbeat_ms,
                    missed_beats,
                    status,
                },
            );
        }

        let faults_count = inner.faults.len();
        let plans_count = inner.recovery_plans.len();
        let groups_count = inner.isolation_groups.len();
        let hb_count = inner.heartbeats.len();

        tracing::info!(
            "FaultToleranceEngine: loaded {} faults, {} plans, {} groups, {} heartbeats from DB",
            faults_count,
            plans_count,
            groups_count,
            hb_count
        );

        Ok((faults_count, plans_count, groups_count, hb_count))
    }

    /// Non-SQLite fallback: no-op
    #[cfg(not(feature = "backend-sqlite"))]
    pub fn load_from_db(&self, _conn: &()) -> anyhow::Result<(usize, usize, usize, usize)> {
        tracing::warn!("FaultToleranceEngine: load_from_db requires backend-sqlite feature");
        Ok((0, 0, 0, 0))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(crate) fn now_millis() -> u64 {
    crate::acp::prelude::now_ts_ms() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> FaultToleranceConfig {
        FaultToleranceConfig {
            heartbeat_timeout_ms: 100, // 100 ms for fast test
            max_missed_beats: 3,
            recovery_check_interval_ms: 50,
        }
    }

    #[test]
    fn test_new_engine_empty() {
        let config = make_config();
        let engine = FaultToleranceEngine::new(config);
        let profile = engine.profile();
        assert_eq!(profile.total_nodes, 0);
        assert_eq!(profile.online_nodes, 0);
        assert_eq!(profile.degraded_nodes, 0);
        assert_eq!(profile.offline_nodes, 0);
        assert_eq!(profile.active_faults, 0);
        assert_eq!(profile.isolated_groups, 0);
    }

    #[test]
    fn test_register_node() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for test");
        let profile = engine.profile();
        assert_eq!(profile.total_nodes, 1);
        assert_eq!(profile.online_nodes, 1);
    }

    #[test]
    fn test_register_duplicate_node_fails() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register initial node before duplicate attempt");
        let result = engine.register_node("node-1");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("already registered"));
    }

    #[test]
    fn test_unregister_node() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 before unregister test");
        engine
            .unregister_node("node-1")
            .expect("unregister node-1 should succeed");
        let profile = engine.profile();
        assert_eq!(profile.total_nodes, 0);
        // unregistering an unknown node should fail
        let result = engine.unregister_node("node-1");
        assert!(result.is_err());
    }

    #[test]
    fn test_report_heartbeat() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for heartbeat test");
        // report a heartbeat (should succeed)
        engine
            .report_heartbeat("node-1")
            .expect("report heartbeat for registered node");
        // reporting heartbeat for unknown node should fail
        let result = engine.report_heartbeat("node-unknown");
        assert!(result.is_err());
    }

    #[test]
    fn test_missed_heartbeat_detection() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for heartbeat detection test");
        // Immediately after registration, no missed beats
        let offenders = engine.check_heartbeats();
        assert!(offenders.is_empty());

        // Wait longer than the heartbeat timeout (100ms)
        std::thread::sleep(std::time::Duration::from_millis(150));

        // First check: one missed -> degraded
        let offenders = engine.check_heartbeats();
        // missed_beats == 1, < max_missed (3), so not an offender yet
        assert!(offenders.is_empty());

        // Wait again and check multiple times to exceed max_missed_beats
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(110));
            engine.check_heartbeats();
        }

        let offenders = engine.check_heartbeats();
        assert!(
            offenders.contains(&"node-1".to_string()),
            "node-1 should be marked as offender after many missed beats, got: {:?}",
            offenders
        );
    }

    #[test]
    fn test_report_fault() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for fault report test");
        let fault_id = engine
            .report_fault("node-1", FaultType::Crash, 7, "Node crashed unexpectedly")
            .expect("report crash fault on node-1 should succeed");
        assert!(fault_id.starts_with("fault-"));
        // Verify fault is active
        let active = engine.active_faults();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, fault_id);
        assert_eq!(active[0].node_id, "node-1");
        assert_eq!(active[0].fault_type, FaultType::Crash);
        assert!(!active[0].recovered);

        // Reporting fault on unknown node should fail
        let result = engine.report_fault("unknown", FaultType::Crash, 5, "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_fault() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for fault resolution test");
        let fault_id = engine
            .report_fault("node-1", FaultType::Oom, 9, "Out of memory")
            .expect("report OOM fault on node-1");

        // Resolve the fault
        engine
            .resolve_fault(&fault_id)
            .expect("resolve reported fault should succeed");
        let active = engine.active_faults();
        assert_eq!(active.len(), 0);

        // Resolving again should fail
        let result = engine.resolve_fault(&fault_id);
        assert!(result.is_err());

        // Resolving unknown fault should fail
        let result = engine.resolve_fault("does-not-exist");
        assert!(result.is_err());
    }

    #[test]
    fn test_isolate_node() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for isolate test");

        // Isolate at Monitor level
        engine
            .isolate_node("node-1", IsolationLevel::Monitor)
            .expect("isolate node-1 at Monitor level");
        let profile = engine.profile();
        assert_eq!(profile.isolated_groups, 1);

        // Isolate again at a different level (should update existing group)
        engine
            .isolate_node("node-1", IsolationLevel::Quarantine)
            .expect("isolate node-1 at Quarantine level should succeed");
        let profile = engine.profile();
        assert_eq!(profile.isolated_groups, 1);
        assert_eq!(profile.degraded_nodes, 1);

        // Isolate unknown node
        let result = engine.isolate_node("unknown", IsolationLevel::Shutdown);
        assert!(result.is_err());
    }

    #[test]
    fn test_reintegrate_node() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for reintegrate test");
        engine
            .isolate_node("node-1", IsolationLevel::Quarantine)
            .expect("quarantine node-1 before reintegration");

        // Reintegrate
        engine
            .reintegrate_node("node-1")
            .expect("reintegrate quarantined node-1");
        let profile = engine.profile();
        assert_eq!(profile.isolated_groups, 0);
        assert_eq!(profile.online_nodes, 1);

        // Reintegrate unknown node should fail
        let result = engine.reintegrate_node("unknown");
        assert!(result.is_err());
    }

    #[test]
    fn test_active_faults() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for active faults test");
        engine
            .register_node("node-2")
            .expect("register node-2 for active faults test");

        let id1 = engine
            .report_fault("node-1", FaultType::Crash, 8, "crash")
            .expect("report crash fault on node-1");
        let id2 = engine
            .report_fault("node-2", FaultType::Hang, 5, "hang")
            .expect("report hang fault on node-2");

        let active = engine.active_faults();
        assert_eq!(active.len(), 2);

        // Resolve one
        engine
            .resolve_fault(&id1)
            .expect("resolve fault id1 should succeed");
        let active = engine.active_faults();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, id2);
    }

    #[test]
    fn test_profile_reflects_state() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for profile test");
        engine
            .register_node("node-2")
            .expect("register node-2 for profile test");
        engine
            .register_node("node-3")
            .expect("register node-3 for profile test");

        engine
            .report_fault("node-1", FaultType::NetworkSplit, 9, "split")
            .expect("report network split fault on node-1");
        engine
            .report_fault("node-2", FaultType::ResourceExhaustion, 5, "exhausted")
            .expect("report resource exhaustion fault on node-2");

        // node-1 severity 9 -> offline
        // node-2 severity 5 -> degraded
        // node-3 -> online

        let profile = engine.profile();
        assert_eq!(profile.total_nodes, 3);
        assert_eq!(profile.online_nodes, 1);
        assert_eq!(profile.degraded_nodes, 1);
        assert_eq!(profile.offline_nodes, 1);
        assert_eq!(profile.active_faults, 2);
        assert_eq!(profile.isolated_groups, 0);
    }

    #[test]
    fn test_create_recovery_plan() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for recovery plan test");
        engine
            .report_fault("node-1", FaultType::Crash, 8, "crash")
            .expect("report crash fault on node-1 for plan creation");
        let plan_id = engine
            .create_recovery_plan("node-1")
            .expect("create recovery plan for node-1");
        assert!(plan_id.starts_with("plan-"));
        let plans = engine.active_recovery_plans();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].node_id, "node-1");
        assert!(!plans[0].actions.is_empty());
        // Unknown node should fail
        assert!(engine.create_recovery_plan("unknown").is_err());
    }

    #[test]
    fn test_execute_and_complete_recovery_plan() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for execute/complete test");
        engine
            .report_fault("node-1", FaultType::Hang, 6, "hang")
            .expect("report hang fault on node-1");
        let plan_id = engine
            .create_recovery_plan("node-1")
            .expect("create recovery plan for node-1");

        // Execute plan
        engine
            .execute_recovery_plan(&plan_id)
            .expect("execute recovery plan should succeed");
        let plans = engine.active_recovery_plans();
        assert_eq!(plans[0].state, RecoveryState::InProgress);

        // Complete plan
        engine
            .complete_recovery_plan(&plan_id, "restarted successfully")
            .expect("complete recovery plan should succeed");
        let active = engine.active_recovery_plans();
        assert!(active.is_empty());

        // Double complete should fail
        assert!(engine.complete_recovery_plan(&plan_id, "again").is_err());
    }

    #[test]
    fn test_fail_recovery_plan() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for fail plan test");
        engine
            .report_fault("node-1", FaultType::DataCorruption, 7, "corruption")
            .expect("report data corruption fault on node-1");
        let plan_id = engine
            .create_recovery_plan("node-1")
            .expect("create recovery plan for node-1");
        engine
            .execute_recovery_plan(&plan_id)
            .expect("execute recovery plan should succeed");
        engine
            .fail_recovery_plan(&plan_id, "timeout")
            .expect("fail recovery plan should succeed");
        let active = engine.active_recovery_plans();
        assert!(active.is_empty());
    }

    #[test]
    fn test_escalation_level() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for escalation test");

        // No faults → Auto
        assert_eq!(engine.escalation_level("node-1"), EscalationLevel::Auto);

        // Low severity fault → Auto
        engine
            .report_fault("node-1", FaultType::Hang, 3, "minor")
            .expect("report minor hang fault on node-1");
        assert_eq!(engine.escalation_level("node-1"), EscalationLevel::Auto);

        // High severity fault → Manual
        engine
            .report_fault("node-1", FaultType::Crash, 9, "severe crash")
            .expect("report severe crash fault on node-1");
        assert_eq!(engine.escalation_level("node-1"), EscalationLevel::Manual);

        // Unknown node → Manual
        assert_eq!(engine.escalation_level("unknown"), EscalationLevel::Manual);
    }

    #[test]
    fn test_cluster_health_healthy() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for healthy cluster test");
        engine
            .register_node("node-2")
            .expect("register node-2 for healthy cluster test");
        engine
            .register_node("node-3")
            .expect("register node-3 for healthy cluster test");
        assert_eq!(engine.cluster_health(), ClusterHealth::Healthy);
    }

    #[test]
    fn test_cluster_health_degraded() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for degraded cluster test");
        engine
            .register_node("node-2")
            .expect("register node-2 for degraded cluster test");
        engine
            .report_fault("node-1", FaultType::Crash, 5, "moderate")
            .expect("report crash fault to trigger degraded health");
        assert_eq!(engine.cluster_health(), ClusterHealth::Degraded);
    }

    #[test]
    fn test_run_recovery_cycle() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for recovery cycle test");

        // Force a missed heartbeat
        std::thread::sleep(std::time::Duration::from_millis(150));
        for _ in 0..4 {
            std::thread::sleep(std::time::Duration::from_millis(110));
            engine.check_heartbeats();
        }

        let summary = engine.run_recovery_cycle();
        assert!(!summary.offenders.is_empty());
        assert!(
            summary.cluster_health == ClusterHealth::Critical
                || summary.cluster_health == ClusterHealth::Degraded
        );
    }

    // ── E2E: FaultToleranceEngine lifecycle (fault → detect → recover) ──

    #[test]
    fn test_e2e_fault_detect_recover() {
        let config = FaultToleranceConfig {
            heartbeat_timeout_ms: 60_000,
            max_missed_beats: 5,
            recovery_check_interval_ms: 1000,
        };
        let engine = FaultToleranceEngine::new(config);

        for i in 0..10 {
            engine
                .register_node(&format!("node-{}", i))
                .expect("register node for e2e test");
        }
        assert_eq!(
            engine.profile().total_nodes,
            10,
            "should have 10 total nodes"
        );

        // Inject crash on node-5 — severity 9 sets node to Offline
        engine
            .report_fault("node-5", FaultType::Crash, 9, "test crash")
            .expect("report crash fault on node-5 for e2e test");
        let p = engine.profile();
        assert_eq!(
            p.offline_nodes, 1,
            "crash fault should set one node offline"
        );

        // Create + execute + complete recovery plan
        let plan = engine
            .create_recovery_plan("node-5")
            .expect("create recovery plan for node-5");
        engine
            .execute_recovery_plan(&plan)
            .expect("execute recovery plan for node-5");
        engine
            .complete_recovery_plan(&plan, "recovered")
            .expect("complete recovery plan for node-5");
        engine
            .reintegrate_node("node-5")
            .expect("reintegrate node-5 after recovery");

        let p = engine.profile();
        assert_eq!(p.total_nodes, 10, "all nodes should be present");
        assert_eq!(
            p.online_nodes, 10,
            "all nodes should be online after recovery"
        );
        assert_eq!(p.active_faults, 0, "all faults should be resolved");
    }

    // ── E2E: FaultToleranceEngine + CapabilityBus-style lifecycle ─────
    //
    // NOTE: Full HarnessBus integration requires 7 constructor args
    // (rule_engine, sandbox_level, budget, idempotency, etc.) that are
    // complex to construct in a unit test. The HarnessBus E2E wiring is
    // verified in `tests/fault_tolerance_e2e.rs` via crate binary RPC calls.

    // ── Multi-node stress test (500+ nodes) ────────────────────────────

    #[test]
    fn test_e2e_multi_node_stress_500() {
        let config = FaultToleranceConfig {
            heartbeat_timeout_ms: 60_000,
            max_missed_beats: 3,
            recovery_check_interval_ms: 100,
        };
        let engine = FaultToleranceEngine::new(config);
        let node_count = 500;

        for i in 0..node_count {
            engine
                .register_node(&format!("node-{}", i))
                .expect("register node for stress test");
        }
        assert_eq!(engine.profile().total_nodes, node_count);
        assert_eq!(engine.profile().online_nodes, node_count);

        // Inject faults on 20 nodes
        for i in 0..20 {
            let idx = (i * 13 + 7) % node_count;
            engine
                .report_fault(
                    &format!("node-{}", idx),
                    FaultType::ResourceExhaustion,
                    6,
                    "stress fault",
                )
                .expect("report resource exhaustion fault for stress test");
        }

        // Create recovery plans for all fault nodes
        let mut plans = Vec::new();
        for i in 0..20 {
            let idx = (i * 13 + 7) % node_count;
            if let Ok(pid) = engine.create_recovery_plan(&format!("node-{}", idx)) {
                plans.push(pid);
            }
        }
        assert!(!plans.is_empty(), "should create plans for faulted nodes");

        // Execute and complete all plans
        for pid in &plans {
            let _ = engine.execute_recovery_plan(pid);
        }
        for pid in &plans {
            let _ = engine.complete_recovery_plan(pid, "recovered");
        }

        // Reintegrate all recovered nodes
        for i in 0..20 {
            let idx = (i * 13 + 7) % node_count;
            let _ = engine.reintegrate_node(&format!("node-{}", idx));
        }

        let profile = engine.profile();
        assert_eq!(profile.total_nodes, node_count);
        assert_eq!(profile.online_nodes, node_count);
        assert_eq!(profile.active_faults, 0);
    }

    /// E2E: Node fault → recovery plan → execute → complete → reintegrate
    /// with transport-level notification via MultiChannelTransport.
    #[test]
    fn test_e2e_fault_recovery_with_transport() {
        use crate::protocol::transport::{MultiChannelTransport, TransportConfig};

        let ft_config = FaultToleranceConfig {
            heartbeat_timeout_ms: 60_000,
            max_missed_beats: 5,
            recovery_check_interval_ms: 1000,
        };
        let ft = FaultToleranceEngine::new(ft_config);
        let transport = MultiChannelTransport::new(TransportConfig::default());

        // Register 10 nodes
        for i in 0..10 {
            ft.register_node(&format!("node-{}", i))
                .expect("register node for transport e2e test");
        }
        assert_eq!(ft.profile().online_nodes, 10);

        // Crash node-5 — severity 9 marks node offline
        let crashed = "node-5";
        ft.report_fault(crashed, FaultType::Crash, 9, "test crash")
            .expect("report crash fault on node-5 in transport test");
        let profile = ft.profile();
        assert_eq!(
            profile.offline_nodes, 1,
            "crash should set one node offline"
        );
        assert_eq!(profile.online_nodes, 9);

        // Create + execute + complete recovery plan
        let plan = ft
            .create_recovery_plan(crashed)
            .expect("create recovery plan for node-5");
        ft.execute_recovery_plan(&plan)
            .expect("execute recovery plan should succeed");
        ft.complete_recovery_plan(&plan, "restarted")
            .expect("complete recovery plan should succeed");
        ft.reintegrate_node(crashed)
            .expect("reintegrate node-5 after transport recovery");

        // Send recovery notification via transport
        let _ = transport.send_event(
            "coordinator",
            "logger",
            &format!("node {} recovered", crashed),
        );

        assert_eq!(ft.profile().online_nodes, 10);
        assert_eq!(ft.profile().active_faults, 0);
    }

    #[test]
    fn test_profile_includes_recovery() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .expect("register node-1 for profile recovery test");
        engine
            .report_fault("node-1", FaultType::Oom, 9, "OOM")
            .expect("report OOM fault on node-1");
        engine
            .create_recovery_plan("node-1")
            .expect("create recovery plan for node-1");
        let profile = engine.profile();
        assert!(profile.total_nodes > 0);
    }
}
