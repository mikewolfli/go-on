//! Cross-node Fault Tolerance module — F-GAP-28
//!
//! **Scope — Node-level fault detection:** heartbeats, isolation groups,
//! recovery plans, and cluster-health computation.
//!
//! This module owns the node-level lifecycle: detecting when a peer goes
//! silent, isolating it from traffic, and coordinating automated recovery
//! plans (escalation, reintegration).  It does **not** implement service-level
//! patterns such as circuit breaking, failover routing, or self-healing
//! retry logic — those belong to the `resilience` module.
//!
//! Both modules are complementary: `fault_tolerance` answers *"is the node
//! alive?"* and *"how do we bring it back?"* while `resilience` answers
//! *"how do we keep serving through transient failures?"*

mod detector;
mod recovery;
mod types;

pub use types::*;

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
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

/// Snapshot of fault tolerance state for persistence operations.
/// Cloned under the async lock before entering `spawn_blocking` to avoid
/// holding the lock across blocking I/O.
#[derive(Clone, Serialize, Deserialize)]
struct FaultToleranceSnapshot {
    faults: HashMap<String, FaultEvent>,
    recovery_plans: HashMap<String, RecoveryPlan>,
    isolation_groups: HashMap<String, IsolationGroup>,
    heartbeats: HashMap<String, HeartbeatRecord>,
}

/// Persist snapshot to SQLite. Runs inside `spawn_blocking` — never call from
/// async context directly.
#[cfg(feature = "backend-sqlite")]
fn persist_sqlite(path: &std::path::Path, snapshot: &FaultToleranceSnapshot) {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(
                "FaultToleranceEngine: failed to create parent dir {}: {}",
                parent.display(),
                e
            );
        }
    }

    let conn = match rusqlite::Connection::open(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(
                "FaultToleranceEngine: failed to open SQLite DB at {}: {}",
                path.display(),
                e
            );
            return;
        }
    };
    let mut conn = conn;

    if let Err(e) = conn.execute_batch(
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
    ) {
        tracing::warn!("FaultToleranceEngine: failed to create tables: {}", e);
        return;
    }

    // Wrap the full clear+rewrite in one transaction: previously every DELETE
    // and INSERT autocommitted individually (N fsyncs per 10s cycle). A single
    // transaction batches the writes into one commit.
    let tx = match conn.transaction() {
        Ok(tx) => tx,
        Err(e) => {
            tracing::warn!("FaultToleranceEngine: failed to begin transaction: {}", e);
            return;
        }
    };

    // Clear existing data for idempotent save
    for table in &[
        "faults",
        "recovery_plans",
        "isolation_groups",
        "heartbeat_records",
    ] {
        if let Err(e) = tx.execute(&format!("DELETE FROM {}", table), []) {
            tracing::warn!(
                "FaultToleranceEngine: failed to clear table {}: {}",
                table,
                e
            );
        }
    }

    // Insert faults
    for fault in snapshot.faults.values() {
        if let Err(e) = tx.execute(
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
        ) {
            tracing::warn!(
                "FaultToleranceEngine: failed to insert fault {}: {}", fault.id, e
            );
        }
    }

    // Insert recovery plans
    for plan in snapshot.recovery_plans.values() {
        let actions_json = match serde_json::to_string(&plan.actions) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(
                    "FaultToleranceEngine: failed to serialize plan {} actions: {}",
                    plan.plan_id,
                    e
                );
                continue;
            }
        };
        if let Err(e) = tx.execute(
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
        ) {
            tracing::warn!(
                "FaultToleranceEngine: failed to insert plan {}: {}", plan.plan_id, e
            );
        }
    }

    // Insert isolation groups
    for group in snapshot.isolation_groups.values() {
        let nodes_json = match serde_json::to_string(&group.nodes) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(
                    "FaultToleranceEngine: failed to serialize group {} nodes: {}",
                    group.group_id,
                    e
                );
                continue;
            }
        };
        if let Err(e) = tx.execute(
            "INSERT INTO isolation_groups (group_id, nodes, isolation_level, created_ms)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                group.group_id,
                nodes_json,
                format!("{:?}", group.isolation_level),
                group.created_ms as i64,
            ],
        ) {
            tracing::warn!(
                "FaultToleranceEngine: failed to insert group {}: {}",
                group.group_id,
                e
            );
        }
    }

    // Insert heartbeat records
    for hb in snapshot.heartbeats.values() {
        if let Err(e) = tx.execute(
            "INSERT OR REPLACE INTO heartbeat_records (node_id, last_heartbeat_ms, missed_beats, status)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                hb.node_id,
                hb.last_heartbeat_ms as i64,
                hb.missed_beats as i64,
                format!("{:?}", hb.status),
            ],
        ) {
            tracing::warn!(
                "FaultToleranceEngine: failed to insert heartbeat {}: {}", hb.node_id, e
            );
        }
    }

    if let Err(e) = tx.commit() {
        tracing::warn!("FaultToleranceEngine: failed to commit state: {}", e);
    }

    tracing::info!(
        "FaultToleranceEngine: saved {} faults, {} plans, {} groups, {} heartbeats to DB",
        snapshot.faults.len(),
        snapshot.recovery_plans.len(),
        snapshot.isolation_groups.len(),
        snapshot.heartbeats.len()
    );
}

/// Persist snapshot as JSON. Runs inside `spawn_blocking` — never call from
/// async context directly.
#[cfg(not(feature = "backend-sqlite"))]
fn persist_json(path: &std::path::Path, snapshot: &FaultToleranceSnapshot) {
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(
                "FaultToleranceEngine: failed to create parent dir {}: {}",
                parent.display(),
                e
            );
        }
    }

    match serde_json::to_string_pretty(snapshot) {
        Ok(json) => {
            if let Err(e) = std::fs::write(path, json) {
                tracing::warn!(
                    "FaultToleranceEngine: failed to write JSON to {}: {}",
                    path.display(),
                    e
                );
            } else {
                tracing::info!(
                    target: "fault_tolerance",
                    faults = snapshot.faults.len(),
                    plans = snapshot.recovery_plans.len(),
                    groups = snapshot.isolation_groups.len(),
                    heartbeats = snapshot.heartbeats.len(),
                    "persisted fault tolerance state to JSON file"
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                "FaultToleranceEngine: failed to serialize state as JSON: {}",
                e
            );
        }
    }
}

/// Parse an enum variant from its Debug string representation (e.g. "Crash" -> FaultType::Crash).
/// The persist functions store enums via `format!("{:?}", value)` which produces unquoted variant names.
#[cfg(feature = "backend-sqlite")]
fn parse_enum_from_debug<T>(s: &str) -> Result<T, serde_json::Error>
where
    T: serde::de::DeserializeOwned,
{
    // Wrap in JSON quotes so serde can parse it as a JSON string
    serde_json::from_str(&format!("\"{}\"", s))
}

/// Load snapshot from SQLite. Runs inside `spawn_blocking` — never call from
/// async context directly.
#[cfg(feature = "backend-sqlite")]
fn load_sqlite(path: &std::path::Path) -> Option<FaultToleranceSnapshot> {
    let conn = rusqlite::Connection::open(path).ok()?;

    // Load faults
    let mut faults = HashMap::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT id, node_id, fault_type, severity, description, detected_ms, resolved_ms, recovered FROM faults"
    ) {
        if let Ok(rows) = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let node_id: String = row.get(1)?;
            let fault_type_str: String = row.get(2)?;
            let severity: i64 = row.get(3)?;
            let description: String = row.get(4)?;
            let detected_ms: i64 = row.get(5)?;
            let resolved_ms: Option<i64> = row.get(6)?;
            let recovered: i64 = row.get(7)?;
            Ok((id, node_id, fault_type_str, severity, description, detected_ms, resolved_ms, recovered))
        }) {
            for row in rows.flatten() {
                let fault_type: FaultType = parse_enum_from_debug(&row.2).unwrap_or(FaultType::Crash);
                let fault = FaultEvent {
                    id: row.0,
                    node_id: row.1,
                    fault_type,
                    severity: row.3 as u8,
                    description: row.4,
                    detected_ms: row.5 as u64,
                    resolved_ms: row.6.map(|v| v as u64),
                    recovered: row.7 != 0,
                };
                faults.insert(fault.id.clone(), fault);
            }
        }
    }

    // Load recovery plans
    let mut recovery_plans = HashMap::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT plan_id, node_id, actions, state, created_ms, completed_ms, result FROM recovery_plans"
    ) {
        if let Ok(rows) = stmt.query_map([], |row| {
            let plan_id: String = row.get(0)?;
            let node_id: String = row.get(1)?;
            let actions_json: String = row.get(2)?;
            let state_str: String = row.get(3)?;
            let created_ms: i64 = row.get(4)?;
            let completed_ms: Option<i64> = row.get(5)?;
            let result: Option<String> = row.get(6)?;
            Ok((plan_id, node_id, actions_json, state_str, created_ms, completed_ms, result))
        }) {
            for row in rows.flatten() {
                let actions: Vec<RecoveryAction> =
                    serde_json::from_str(&row.2).unwrap_or_default();
                let state: RecoveryState =
                    parse_enum_from_debug(&row.3).unwrap_or(RecoveryState::Pending);
                let plan = RecoveryPlan {
                    plan_id: row.0,
                    node_id: row.1,
                    actions,
                    state,
                    created_ms: row.4 as u64,
                    completed_ms: row.5.map(|v| v as u64),
                    result: row.6,
                };
                recovery_plans.insert(plan.plan_id.clone(), plan);
            }
        }
    }

    // Load isolation groups
    let mut isolation_groups = HashMap::new();
    if let Ok(mut stmt) =
        conn.prepare("SELECT group_id, nodes, isolation_level, created_ms FROM isolation_groups")
    {
        if let Ok(rows) = stmt.query_map([], |row| {
            let group_id: String = row.get(0)?;
            let nodes_json: String = row.get(1)?;
            let isolation_level_str: String = row.get(2)?;
            let created_ms: i64 = row.get(3)?;
            Ok((group_id, nodes_json, isolation_level_str, created_ms))
        }) {
            for row in rows.flatten() {
                let nodes: Vec<String> = serde_json::from_str(&row.1).unwrap_or_default();
                let isolation_level: IsolationLevel =
                    parse_enum_from_debug(&row.2).unwrap_or(IsolationLevel::Monitor);
                let group = IsolationGroup {
                    group_id: row.0,
                    nodes,
                    isolation_level,
                    created_ms: row.3 as u64,
                };
                isolation_groups.insert(group.group_id.clone(), group);
            }
        }
    }

    // Load heartbeat records
    let mut heartbeats = HashMap::new();
    if let Ok(mut stmt) = conn
        .prepare("SELECT node_id, last_heartbeat_ms, missed_beats, status FROM heartbeat_records")
    {
        if let Ok(rows) = stmt.query_map([], |row| {
            let node_id: String = row.get(0)?;
            let last_heartbeat_ms: i64 = row.get(1)?;
            let missed_beats: i64 = row.get(2)?;
            let status_str: String = row.get(3)?;
            Ok((node_id, last_heartbeat_ms, missed_beats, status_str))
        }) {
            for row in rows.flatten() {
                let status: NodeStatus =
                    parse_enum_from_debug(&row.3).unwrap_or(NodeStatus::Online);
                let hb = HeartbeatRecord {
                    node_id: row.0,
                    last_heartbeat_ms: row.1 as u64,
                    missed_beats: row.2 as u32,
                    status,
                    // Restored records have not reported in this process yet;
                    // liveness monitoring resumes on the next real heartbeat.
                    has_reported: false,
                };
                heartbeats.insert(hb.node_id.clone(), hb);
            }
        }
    }

    if faults.is_empty()
        && recovery_plans.is_empty()
        && isolation_groups.is_empty()
        && heartbeats.is_empty()
    {
        return None;
    }

    Some(FaultToleranceSnapshot {
        faults,
        recovery_plans,
        isolation_groups,
        heartbeats,
    })
}

/// Load snapshot from JSON file. Runs inside `spawn_blocking` — never call from
/// async context directly.
#[cfg(not(feature = "backend-sqlite"))]
fn load_json(path: &std::path::Path) -> Option<FaultToleranceSnapshot> {
    let data = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<FaultToleranceSnapshot>(&data) {
        Ok(snapshot) => Some(snapshot),
        Err(e) => {
            tracing::warn!(
                "FaultToleranceEngine: failed to deserialize state from JSON: {}",
                e
            );
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
    if offline_ratio >= config.healthy_threshold || active_faults >= config.critical_fault_count {
        ClusterHealth::Critical
    } else if offline_ratio >= config.degraded_threshold
        || degraded_ratio >= config.unhealthy_threshold
        || active_faults > 0
    {
        // The threshold comparisons above are the real signal; the
        // previously-present `offline_nodes > 0 || degraded_nodes > 0` fallback
        // made them vacuous (any single offline node forced Degraded regardless
        // of ratio). Unresolved faults remain an explicit degradation signal.
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

    /// Create a new engine and immediately restore persisted state from
    /// disk/DB. Falls back to an empty engine if no persisted state exists.
    ///
    /// Synchronous: restore runs once at engine construction (startup), and
    /// the underlying load is a small bounded read. The periodic write path
    /// (`try_persist_state`) remains async and offloads I/O via
    /// `spawn_blocking`.
    pub fn new_with_restore(config: FaultToleranceConfig) -> Self {
        let engine = Self::new(config);
        engine.restore_state();
        engine
    }

    /// Load persisted state into the engine's in-memory maps.
    ///
    /// Uses the same storage path as `try_persist_state` so that state
    /// survives process restarts. Counter fields (fault_counter,
    /// group_counter, plan_counter) are derived from the loaded data.
    ///
    /// Runs synchronously at startup; `try_write` succeeds because a
    /// freshly-created engine has no concurrent lock holders.
    pub fn restore_state(&self) {
        #[cfg(feature = "backend-sqlite")]
        {
            let cache_path = crate::shared::goon_paths::goon_subdir("fault_tolerance")
                .join("fault_tolerance.db");

            if !cache_path.exists() {
                tracing::info!(
                    "FaultToleranceEngine: no existing state DB at {} — starting fresh",
                    cache_path.display()
                );
                return;
            }

            if let Some(snapshot) = load_sqlite(&cache_path) {
                let Ok(mut inner) = self.inner.try_write() else {
                    tracing::warn!(
                        "FaultToleranceEngine: state lock busy at startup — skipping restore"
                    );
                    return;
                };
                inner.faults = snapshot.faults;
                inner.recovery_plans = snapshot.recovery_plans;
                inner.isolation_groups = snapshot.isolation_groups;
                inner.heartbeats = snapshot.heartbeats;
                // Derive counters from loaded data
                inner.fault_counter = inner.faults.len() as u64;
                inner.group_counter = inner.isolation_groups.len() as u64;
                inner.plan_counter = inner.recovery_plans.len() as u64;
                tracing::info!(
                    faults = inner.faults.len(),
                    plans = inner.recovery_plans.len(),
                    groups = inner.isolation_groups.len(),
                    heartbeats = inner.heartbeats.len(),
                    "FaultToleranceEngine: restored state from SQLite"
                );
            } else {
                tracing::info!("FaultToleranceEngine: no data in SQLite DB — starting fresh");
            }
        }
        #[cfg(not(feature = "backend-sqlite"))]
        {
            let cache_path = crate::shared::goon_paths::goon_subdir("fault_tolerance")
                .join("fault_tolerance.json");

            if !cache_path.exists() {
                tracing::info!(
                    "FaultToleranceEngine: no existing state file at {} — starting fresh",
                    cache_path.display()
                );
                return;
            }

            if let Some(snapshot) = load_json(&cache_path) {
                let Ok(mut inner) = self.inner.try_write() else {
                    tracing::warn!(
                        "FaultToleranceEngine: state lock busy at startup — skipping restore"
                    );
                    return;
                };
                inner.faults = snapshot.faults;
                inner.recovery_plans = snapshot.recovery_plans;
                inner.isolation_groups = snapshot.isolation_groups;
                inner.heartbeats = snapshot.heartbeats;
                // Derive counters from loaded data
                inner.fault_counter = inner.faults.len() as u64;
                inner.group_counter = inner.isolation_groups.len() as u64;
                inner.plan_counter = inner.recovery_plans.len() as u64;
                tracing::info!(
                    faults = inner.faults.len(),
                    plans = inner.recovery_plans.len(),
                    groups = inner.isolation_groups.len(),
                    heartbeats = inner.heartbeats.len(),
                    "FaultToleranceEngine: restored state from JSON"
                );
            } else {
                tracing::info!("FaultToleranceEngine: no data in JSON file — starting fresh");
            }
        }
    }

    /// Return a snapshot profile of the cluster state.
    pub async fn profile(&self) -> FaultToleranceProfile {
        let inner = self.inner.read().await;
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
    ///
    /// Plan creation and execution are parallelized with `buffer_unordered(8)`
    /// — nodes are independent, and the previous strictly-serial loop made
    /// large clusters (and every plan's blocking work) wait for each other
    /// (same pattern as the memory layer's `auto_migrate`).
    pub async fn run_recovery_cycle(&self) -> RecoveryCycleSummary {
        let offenders = self.check_heartbeats().await;
        let mut plans_created = 0u32;
        let mut plans_activated = 0u32;
        let mut consistency_checks = Vec::new();

        // Auto-create recovery plans for all offenders in parallel. Each item
        // reports whether a plan was created for that node.
        let mut create_stream = stream::iter(offenders.clone())
            .map(|node_id| {
                async move {
                    // Check if a plan already exists for this node.
                    let existing = {
                        let inner = self.inner.read().await;
                        inner
                            .recovery_plans
                            .values()
                            .any(|p| p.node_id == node_id && p.state != RecoveryState::Completed)
                    };
                    if existing {
                        false
                    } else {
                        self.create_recovery_plan(&node_id).await.is_ok()
                    }
                }
            })
            .buffer_unordered(8);
        while let Some(created) = create_stream.next().await {
            if created {
                plans_created += 1;
            }
        }

        // Auto-execute pending plans and run consistency checks on activations.
        // A plan that passes its post-recovery consistency check is completed;
        // a failing plan is marked failed so `recovery_plans_in_progress` can
        // never grow unbounded. Each item returns the consistency check when
        // the plan was activated (executed), else `None`.
        let pending_plans = self.active_recovery_plans().await;
        let mut execute_stream = stream::iter(pending_plans)
            .map(|plan| {
                async move {
                    if plan.state != RecoveryState::Pending {
                        return None;
                    }
                    if self.execute_recovery_plan(&plan.plan_id).await.is_err() {
                        return None;
                    }
                    // Run consistency check after activation
                    let check = self.post_recovery_consistency_check(&plan.plan_id).await;
                    if check.passed {
                        if let Err(e) = self
                            .complete_recovery_plan(&plan.plan_id, "recovery_actions_completed")
                            .await
                        {
                            tracing::warn!(
                                target: "fault_tolerance",
                                "failed to complete recovery plan '{}': {}",
                                plan.plan_id,
                                e
                            );
                        }
                    } else {
                        tracing::warn!(
                            "consistency check after activation of plan '{}': {}",
                            plan.plan_id,
                            check.details
                        );
                        if let Err(e) = self.fail_recovery_plan(&plan.plan_id, &check.details).await
                        {
                            tracing::warn!(
                                target: "fault_tolerance",
                                "failed to mark recovery plan '{}' failed: {}",
                                plan.plan_id,
                                e
                            );
                        }
                    }
                    Some(check)
                }
            })
            .buffer_unordered(8);
        while let Some(check) = execute_stream.next().await {
            if let Some(check) = check {
                plans_activated += 1;
                consistency_checks.push(check);
            }
        }

        let health = self.cluster_health().await;
        let profile = self.profile().await;

        // BLUE56-C06: Persist recovery cycle state to DB
        self.try_persist_state().await;

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

    /// Try to persist fault tolerance state. Uses SQLite when the
    /// `backend-sqlite` feature is enabled, otherwise falls back to a JSON file.
    ///
    /// All synchronous I/O (rusqlite, filesystem) is wrapped in `spawn_blocking`
    /// to avoid blocking the async runtime.
    async fn try_persist_state(&self) {
        // Skip persistence entirely when the engine holds no state — the 30s
        // recovery cycle previously wrote an empty snapshot to disk on every
        // tick (full clone + spawn_blocking) even when no node was ever
        // registered. With no state there is nothing to restore, so the
        // write is pure overhead.
        let empty = {
            let inner = self.inner.read().await;
            inner.faults.is_empty()
                && inner.recovery_plans.is_empty()
                && inner.isolation_groups.is_empty()
                && inner.heartbeats.is_empty()
        };
        if empty {
            return;
        }

        #[cfg(feature = "backend-sqlite")]
        {
            let cache_path = crate::shared::goon_paths::goon_subdir("fault_tolerance")
                .join("fault_tolerance.db");

            // Clone data under the async lock, then release it before blocking I/O
            let snapshot = {
                let inner = self.inner.read().await;
                FaultToleranceSnapshot {
                    faults: inner.faults.clone(),
                    recovery_plans: inner.recovery_plans.clone(),
                    isolation_groups: inner.isolation_groups.clone(),
                    heartbeats: inner.heartbeats.clone(),
                }
            };

            if let Err(e) =
                tokio::task::spawn_blocking(move || persist_sqlite(&cache_path, &snapshot)).await
            {
                tracing::error!("FaultToleranceEngine: persist task panicked: {}", e);
            }
        }
        #[cfg(not(feature = "backend-sqlite"))]
        {
            let cache_path = crate::shared::goon_paths::goon_subdir("fault_tolerance")
                .join("fault_tolerance.json");

            // Clone data under the async lock, then release it before blocking I/O
            let snapshot = {
                let inner = self.inner.read().await;
                FaultToleranceSnapshot {
                    faults: inner.faults.clone(),
                    recovery_plans: inner.recovery_plans.clone(),
                    isolation_groups: inner.isolation_groups.clone(),
                    heartbeats: inner.heartbeats.clone(),
                }
            };

            if let Err(e) =
                tokio::task::spawn_blocking(move || persist_json(&cache_path, &snapshot)).await
            {
                tracing::error!("FaultToleranceEngine: persist task panicked: {}", e);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

    #[tokio::test]
    async fn test_new_engine_empty() {
        let config = make_config();
        let engine = FaultToleranceEngine::new(config);
        let profile = engine.profile().await;
        assert_eq!(profile.total_nodes, 0);
        assert_eq!(profile.online_nodes, 0);
        assert_eq!(profile.degraded_nodes, 0);
        assert_eq!(profile.offline_nodes, 0);
        assert_eq!(profile.active_faults, 0);
        assert_eq!(profile.isolated_groups, 0);
    }

    #[tokio::test]
    async fn test_register_node() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for test");
        let profile = engine.profile().await;
        assert_eq!(profile.total_nodes, 1);
        assert_eq!(profile.online_nodes, 1);
    }

    #[tokio::test]
    async fn test_register_duplicate_node_fails() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register initial node before duplicate attempt");
        let result = engine.register_node("node-1").await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("already registered"));
    }

    #[tokio::test]
    async fn test_unregister_node() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 before unregister test");
        engine
            .unregister_node("node-1")
            .await
            .expect("unregister node-1 should succeed");
        let profile = engine.profile().await;
        assert_eq!(profile.total_nodes, 0);
        // unregistering an unknown node should fail
        let result = engine.unregister_node("node-1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_report_heartbeat() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for heartbeat test");
        // report a heartbeat (should succeed)
        engine
            .report_heartbeat("node-1")
            .await
            .expect("report heartbeat for registered node");
        // reporting heartbeat for unknown node should fail
        let result = engine.report_heartbeat("node-unknown").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_missed_heartbeat_detection() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for heartbeat detection test");
        // A registered node must report at least once before liveness is
        // monitored (registration alone is not a liveness signal).
        engine
            .report_heartbeat("node-1")
            .await
            .expect("report first heartbeat");
        // Immediately after reporting, no missed beats
        let offenders = engine.check_heartbeats().await;
        assert!(offenders.is_empty());

        // Wait longer than the heartbeat timeout (100ms)
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        // First check: one missed -> degraded
        let offenders = engine.check_heartbeats().await;
        // missed_beats == 1, < max_missed (3), so not an offender yet
        assert!(offenders.is_empty());

        // Wait again and check multiple times to exceed max_missed_beats
        for _ in 0..3 {
            tokio::time::sleep(std::time::Duration::from_millis(110)).await;
            engine.check_heartbeats().await;
        }

        let offenders = engine.check_heartbeats().await;
        assert!(
            offenders.contains(&"node-1".to_string()),
            "node-1 should be marked as offender after many missed beats, got: {:?}",
            offenders
        );
    }

    #[tokio::test]
    async fn test_registered_but_never_reported_node_is_not_flagged() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("idle-node")
            .await
            .expect("register idle node");
        // Wait well past the heartbeat timeout + max_missed window; the idle
        // node (registered but never reporting) must never be flagged Offline.
        for _ in 0..10 {
            tokio::time::sleep(std::time::Duration::from_millis(110)).await;
            let offenders = engine.check_heartbeats().await;
            assert!(
                offenders.is_empty(),
                "idle node must not be flagged as offender, got: {:?}",
                offenders
            );
        }
        let profile = engine.profile().await;
        assert_eq!(profile.offline_nodes, 0, "idle node must stay Online");
    }

    #[tokio::test]
    async fn test_report_fault() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for fault report test");
        let fault_id = engine
            .report_fault("node-1", FaultType::Crash, 7, "Node crashed unexpectedly")
            .await
            .expect("report crash fault on node-1 should succeed");
        assert!(fault_id.starts_with("fault-"));
        // Verify fault is active
        let active = engine.active_faults().await;
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, fault_id);
        assert_eq!(active[0].node_id, "node-1");
        assert_eq!(active[0].fault_type, FaultType::Crash);
        assert!(!active[0].recovered);

        // Reporting fault on unknown node should fail
        let result = engine
            .report_fault("unknown", FaultType::Crash, 5, "test")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_resolve_fault() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for fault resolution test");
        let fault_id = engine
            .report_fault("node-1", FaultType::Oom, 9, "Out of memory")
            .await
            .expect("report OOM fault on node-1");

        // Resolve the fault
        engine
            .resolve_fault(&fault_id)
            .await
            .expect("resolve reported fault should succeed");
        let active = engine.active_faults().await;
        assert_eq!(active.len(), 0);

        // Resolving again should fail
        let result = engine.resolve_fault(&fault_id).await;
        assert!(result.is_err());

        // Resolving unknown fault should fail
        let result = engine.resolve_fault("does-not-exist").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_isolate_node() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for isolate test");

        // Isolate at Monitor level
        engine
            .isolate_node("node-1", IsolationLevel::Monitor)
            .await
            .expect("isolate node-1 at Monitor level");
        let profile = engine.profile().await;
        assert_eq!(profile.isolated_groups, 1);

        // Isolate again at a different level (should update existing group)
        engine
            .isolate_node("node-1", IsolationLevel::Quarantine)
            .await
            .expect("isolate node-1 at Quarantine level should succeed");
        let profile = engine.profile().await;
        assert_eq!(profile.isolated_groups, 1);
        assert_eq!(profile.degraded_nodes, 1);

        // Isolate unknown node
        let result = engine
            .isolate_node("unknown", IsolationLevel::Shutdown)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_reintegrate_node() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for reintegrate test");
        engine
            .isolate_node("node-1", IsolationLevel::Quarantine)
            .await
            .expect("quarantine node-1 before reintegration");

        // Reintegrate
        engine
            .reintegrate_node("node-1")
            .await
            .expect("reintegrate quarantined node-1");
        let profile = engine.profile().await;
        assert_eq!(profile.isolated_groups, 0);
        assert_eq!(profile.online_nodes, 1);

        // Reintegrate unknown node should fail
        let result = engine.reintegrate_node("unknown").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_active_faults() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for active faults test");
        engine
            .register_node("node-2")
            .await
            .expect("register node-2 for active faults test");

        let id1 = engine
            .report_fault("node-1", FaultType::Crash, 8, "crash")
            .await
            .expect("report crash fault on node-1");
        let id2 = engine
            .report_fault("node-2", FaultType::Hang, 5, "hang")
            .await
            .expect("report hang fault on node-2");

        let active = engine.active_faults().await;
        assert_eq!(active.len(), 2);

        // Resolve one
        engine
            .resolve_fault(&id1)
            .await
            .expect("resolve fault id1 should succeed");
        let active = engine.active_faults().await;
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, id2);
    }

    #[tokio::test]
    async fn test_profile_reflects_state() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for profile test");
        engine
            .register_node("node-2")
            .await
            .expect("register node-2 for profile test");
        engine
            .register_node("node-3")
            .await
            .expect("register node-3 for profile test");

        engine
            .report_fault("node-1", FaultType::NetworkSplit, 9, "split")
            .await
            .expect("report network split fault on node-1");
        engine
            .report_fault("node-2", FaultType::ResourceExhaustion, 5, "exhausted")
            .await
            .expect("report resource exhaustion fault on node-2");

        // node-1 severity 9 -> offline
        // node-2 severity 5 -> degraded
        // node-3 -> online

        let profile = engine.profile().await;
        assert_eq!(profile.total_nodes, 3);
        assert_eq!(profile.online_nodes, 1);
        assert_eq!(profile.degraded_nodes, 1);
        assert_eq!(profile.offline_nodes, 1);
        assert_eq!(profile.active_faults, 2);
        assert_eq!(profile.isolated_groups, 0);
    }

    #[tokio::test]
    async fn test_create_recovery_plan() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for recovery plan test");
        engine
            .report_fault("node-1", FaultType::Crash, 8, "crash")
            .await
            .expect("report crash fault on node-1 for plan creation");
        let plan_id = engine
            .create_recovery_plan("node-1")
            .await
            .expect("create recovery plan for node-1");
        assert!(plan_id.starts_with("plan-"));
        let plans = engine.active_recovery_plans().await;
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].node_id, "node-1");
        assert!(!plans[0].actions.is_empty());
        // Unknown node should fail
        assert!(engine.create_recovery_plan("unknown").await.is_err());
    }

    #[tokio::test]
    async fn test_execute_and_complete_recovery_plan() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for execute/complete test");
        engine
            .report_fault("node-1", FaultType::Hang, 6, "hang")
            .await
            .expect("report hang fault on node-1");
        let plan_id = engine
            .create_recovery_plan("node-1")
            .await
            .expect("create recovery plan for node-1");

        // Execute plan
        engine
            .execute_recovery_plan(&plan_id)
            .await
            .expect("execute recovery plan should succeed");
        let plans = engine.active_recovery_plans().await;
        assert_eq!(plans[0].state, RecoveryState::InProgress);

        // Complete plan
        engine
            .complete_recovery_plan(&plan_id, "restarted successfully")
            .await
            .expect("complete recovery plan should succeed");
        let active = engine.active_recovery_plans().await;
        assert!(active.is_empty());

        // Double complete should fail
        assert!(engine
            .complete_recovery_plan(&plan_id, "again")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_recovery_plan_dispatch_resolves_faults_and_passes_consistency() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for dispatch test");
        // Crash fault (severity 9) → plan actions include RestartNode, which
        // must resolve the fault so the consistency check can pass.
        engine
            .report_fault("node-1", FaultType::Crash, 9, "crash")
            .await
            .expect("report crash fault on node-1");
        let plan_id = engine
            .create_recovery_plan("node-1")
            .await
            .expect("create recovery plan for node-1");

        let plans = engine.active_recovery_plans().await;
        assert!(
            plans[0].actions.contains(&RecoveryAction::RestartNode),
            "crash fault should yield a RestartNode action"
        );

        // Execute the plan: dispatch runs the actions, resolving the fault.
        engine
            .execute_recovery_plan(&plan_id)
            .await
            .expect("execute recovery plan should succeed");
        let plans = engine.active_recovery_plans().await;
        assert_eq!(plans[0].state, RecoveryState::InProgress);
        assert!(
            plans[0]
                .result
                .as_deref()
                .unwrap_or("")
                .contains("RestartNode"),
            "plan result should record the executed action"
        );

        // Observable state: the node's fault is resolved and the heartbeat
        // was reset by the restart dispatch.
        let profile = engine.profile().await;
        assert_eq!(profile.active_faults, 0, "fault should be resolved");

        // The post-recovery consistency check now passes for real.
        let check = engine.post_recovery_consistency_check(&plan_id).await;
        assert!(
            check.passed,
            "consistency check should pass after real dispatch: {}",
            check.details
        );

        // The full cycle path completes the plan.
        engine
            .complete_recovery_plan(&plan_id, "recovery_actions_completed")
            .await
            .expect("complete recovery plan should succeed");
        assert!(engine.active_recovery_plans().await.is_empty());
    }

    #[tokio::test]
    async fn test_fail_recovery_plan() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for fail plan test");
        engine
            .report_fault("node-1", FaultType::DataCorruption, 7, "corruption")
            .await
            .expect("report data corruption fault on node-1");
        let plan_id = engine
            .create_recovery_plan("node-1")
            .await
            .expect("create recovery plan for node-1");
        engine
            .execute_recovery_plan(&plan_id)
            .await
            .expect("execute recovery plan should succeed");
        engine
            .fail_recovery_plan(&plan_id, "timeout")
            .await
            .expect("fail recovery plan should succeed");
        let active = engine.active_recovery_plans().await;
        assert!(active.is_empty());
    }

    #[tokio::test]
    async fn test_escalation_level() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for escalation test");

        // No faults → Auto
        assert_eq!(
            engine.escalation_level("node-1").await,
            EscalationLevel::Auto
        );

        // Low severity fault → Auto
        engine
            .report_fault("node-1", FaultType::Hang, 3, "minor")
            .await
            .expect("report minor hang fault on node-1");
        assert_eq!(
            engine.escalation_level("node-1").await,
            EscalationLevel::Auto
        );

        // High severity fault → Manual
        engine
            .report_fault("node-1", FaultType::Crash, 9, "severe crash")
            .await
            .expect("report severe crash fault on node-1");
        assert_eq!(
            engine.escalation_level("node-1").await,
            EscalationLevel::Manual
        );

        // Unknown node → Manual
        assert_eq!(
            engine.escalation_level("unknown").await,
            EscalationLevel::Manual
        );
    }

    #[tokio::test]
    async fn test_cluster_health_healthy() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for healthy cluster test");
        engine
            .register_node("node-2")
            .await
            .expect("register node-2 for healthy cluster test");
        engine
            .register_node("node-3")
            .await
            .expect("register node-3 for healthy cluster test");
        assert_eq!(engine.cluster_health().await, ClusterHealth::Healthy);
    }

    #[tokio::test]
    async fn test_cluster_health_degraded() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for degraded cluster test");
        engine
            .register_node("node-2")
            .await
            .expect("register node-2 for degraded cluster test");
        engine
            .report_fault("node-1", FaultType::Crash, 5, "moderate")
            .await
            .expect("report crash fault to trigger degraded health");
        assert_eq!(engine.cluster_health().await, ClusterHealth::Degraded);
    }

    #[tokio::test]
    async fn test_run_recovery_cycle() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for recovery cycle test");
        // The node must have reported at least once before liveness is
        // monitored (registration alone is not a liveness signal).
        engine
            .report_heartbeat("node-1")
            .await
            .expect("report first heartbeat");

        // Force a missed heartbeat
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        for _ in 0..4 {
            tokio::time::sleep(std::time::Duration::from_millis(110)).await;
            engine.check_heartbeats().await;
        }

        let summary = engine.run_recovery_cycle().await;
        assert!(!summary.offenders.is_empty());
        // The recovery cycle creates, activates, and (on a passing consistency
        // check) completes the recovery plan — the offender moves from offline
        // to Recovering, so cluster health may recover. The contract under
        // test is that offenders are detected and a plan actually ran.
        assert!(
            summary.plans_created >= 1,
            "expected a recovery plan to be created"
        );
        assert!(
            summary.plans_activated >= 1,
            "expected the recovery plan to be activated"
        );
    }

    // ── E2E: FaultToleranceEngine lifecycle (fault → detect → recover) ──

    #[tokio::test]
    async fn test_e2e_fault_detect_recover() {
        let config = FaultToleranceConfig {
            heartbeat_timeout_ms: 60_000,
            max_missed_beats: 5,
            recovery_check_interval_ms: 1000,
        };
        let engine = FaultToleranceEngine::new(config);

        for i in 0..10 {
            engine
                .register_node(&format!("node-{}", i))
                .await
                .expect("register node for e2e test");
        }
        assert_eq!(
            engine.profile().await.total_nodes,
            10,
            "should have 10 total nodes"
        );

        // Inject crash on node-5 — severity 9 sets node to Offline
        engine
            .report_fault("node-5", FaultType::Crash, 9, "test crash")
            .await
            .expect("report crash fault on node-5 for e2e test");
        let p = engine.profile().await;
        assert_eq!(
            p.offline_nodes, 1,
            "crash fault should set one node offline"
        );

        // Create + execute + complete recovery plan
        let plan = engine
            .create_recovery_plan("node-5")
            .await
            .expect("create recovery plan for node-5");
        engine
            .execute_recovery_plan(&plan)
            .await
            .expect("execute recovery plan for node-5");
        engine
            .complete_recovery_plan(&plan, "recovered")
            .await
            .expect("complete recovery plan for node-5");
        engine
            .reintegrate_node("node-5")
            .await
            .expect("reintegrate node-5 after recovery");

        let p = engine.profile().await;
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

    #[tokio::test]
    async fn test_e2e_multi_node_stress_500() {
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
                .await
                .expect("register node for stress test");
        }
        assert_eq!(engine.profile().await.total_nodes, node_count);
        assert_eq!(engine.profile().await.online_nodes, node_count);

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
                .await
                .expect("report resource exhaustion fault for stress test");
        }

        // Create recovery plans for all fault nodes
        let mut plans = Vec::new();
        for i in 0..20 {
            let idx = (i * 13 + 7) % node_count;
            if let Ok(pid) = engine.create_recovery_plan(&format!("node-{}", idx)).await {
                plans.push(pid);
            }
        }
        assert!(!plans.is_empty(), "should create plans for faulted nodes");

        // Execute and complete all plans
        for pid in &plans {
            let _ = engine.execute_recovery_plan(pid).await;
        }
        for pid in &plans {
            let _ = engine.complete_recovery_plan(pid, "recovered").await;
        }

        // Reintegrate all recovered nodes
        for i in 0..20 {
            let idx = (i * 13 + 7) % node_count;
            let _ = engine.reintegrate_node(&format!("node-{}", idx)).await;
        }

        let profile = engine.profile().await;
        assert_eq!(profile.total_nodes, node_count);
        assert_eq!(profile.online_nodes, node_count);
        assert_eq!(profile.active_faults, 0);
    }

    /// E2E: Node fault → recovery plan → execute → complete → reintegrate
    /// (MultiChannelTransport notification removed — transport eliminated as dead code)
    #[tokio::test]
    async fn test_e2e_fault_recovery_with_transport() {
        let ft_config = FaultToleranceConfig {
            heartbeat_timeout_ms: 60_000,
            max_missed_beats: 5,
            recovery_check_interval_ms: 1000,
        };
        let ft = FaultToleranceEngine::new(ft_config);

        // Register 10 nodes
        for i in 0..10 {
            ft.register_node(&format!("node-{}", i))
                .await
                .expect("register node for transport e2e test");
        }
        assert_eq!(ft.profile().await.online_nodes, 10);

        // Crash node-5 — severity 9 marks node offline
        let crashed = "node-5";
        ft.report_fault(crashed, FaultType::Crash, 9, "test crash")
            .await
            .expect("report crash fault on node-5 in transport test");
        let profile = ft.profile().await;
        assert_eq!(
            profile.offline_nodes, 1,
            "crash should set one node offline"
        );
        assert_eq!(profile.online_nodes, 9);

        // Create + execute + complete recovery plan
        let plan = ft
            .create_recovery_plan(crashed)
            .await
            .expect("create recovery plan for node-5");
        ft.execute_recovery_plan(&plan)
            .await
            .expect("execute recovery plan should succeed");
        ft.complete_recovery_plan(&plan, "restarted")
            .await
            .expect("complete recovery plan should succeed");
        ft.reintegrate_node(crashed)
            .await
            .expect("reintegrate node-5 after transport recovery");

        assert_eq!(ft.profile().await.online_nodes, 10);
        assert_eq!(ft.profile().await.active_faults, 0);
    }

    #[tokio::test]
    async fn test_profile_includes_recovery() {
        let engine = FaultToleranceEngine::new(make_config());
        engine
            .register_node("node-1")
            .await
            .expect("register node-1 for profile recovery test");
        engine
            .report_fault("node-1", FaultType::Oom, 9, "OOM")
            .await
            .expect("report OOM fault on node-1");
        engine
            .create_recovery_plan("node-1")
            .await
            .expect("create recovery plan for node-1");
        let profile = engine.profile().await;
        assert!(profile.total_nodes > 0);
    }
}
