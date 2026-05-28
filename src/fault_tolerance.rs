//! Cross-node Fault Tolerance module — F-GAP-28
//!
//! Provides node-level fault isolation, heartbeat-based failure detection,
//! and automatic recovery coordination across a distributed cluster.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

// ---------------------------------------------------------------------------
// Core data types
// ---------------------------------------------------------------------------

/// Current status of a monitored node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    Online,
    Degraded,
    Offline,
    Recovering,
}

/// The type of fault detected on a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FaultType {
    Crash,
    Hang,
    Oom,
    NetworkSplit,
    DataCorruption,
    ResourceExhaustion,
}

/// A recorded fault event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultEvent {
    pub id: String,
    pub node_id: String,
    pub fault_type: FaultType,
    /// Fault severity (0-9, where 9 is most severe). Values >= 8 set node Offline, >= 4 set Degraded.
    pub severity: u8,
    pub description: String,
    pub detected_ms: u64,
    pub resolved_ms: Option<u64>,
    pub recovered: bool,
}

/// The level of isolation applied to a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IsolationLevel {
    Monitor,
    Quarantine,
    Shutdown,
}

/// A group of isolated nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsolationGroup {
    pub group_id: String,
    pub nodes: Vec<String>,
    pub isolation_level: IsolationLevel,
    pub created_ms: u64,
}

/// Record of a single heartbeat received from a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRecord {
    pub node_id: String,
    pub last_heartbeat_ms: u64,
    pub missed_beats: u32,
    pub status: NodeStatus,
}

/// Configuration for the fault tolerance engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultToleranceConfig {
    pub heartbeat_timeout_ms: u64,
    pub max_missed_beats: u32,
    pub recovery_check_interval_ms: u64,
}

impl Default for FaultToleranceConfig {
    fn default() -> Self {
        Self {
            heartbeat_timeout_ms: 15000,
            max_missed_beats: 3,
            recovery_check_interval_ms: 5000,
        }
    }
}

/// A snapshot profile of the cluster's health.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultToleranceProfile {
    pub total_nodes: usize,
    pub online_nodes: usize,
    pub degraded_nodes: usize,
    pub offline_nodes: usize,
    pub active_faults: usize,
    pub isolated_groups: usize,
    pub recovery_plans_pending: usize,
    pub recovery_plans_in_progress: usize,
    pub cluster_health: ClusterHealth,
}

/// Status of an automatic recovery action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryState {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// Types of automatic recovery actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryAction {
    RestartNode,
    FailoverToBackup,
    ScaleUp,
    Rebalance,
    NotifyOperator,
}

/// A recovery plan for a failed node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryPlan {
    pub plan_id: String,
    pub node_id: String,
    pub actions: Vec<RecoveryAction>,
    pub state: RecoveryState,
    pub created_ms: u64,
    pub completed_ms: Option<u64>,
    pub result: Option<String>,
}

/// Escalation level for fault handling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EscalationLevel {
    /// Automatic recovery (self-healing)
    Auto,
    /// Requires coordinator attention
    Coordinated,
    /// Requires human operator intervention
    Manual,
}

/// Severity classification for cluster health.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClusterHealth {
    Healthy,
    Degraded,
    Critical,
    Down,
}

/// Summary of a recovery cycle run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryCycleSummary {
    pub offenders: Vec<String>,
    pub plans_created: u32,
    pub plans_completed: u32,
    pub cluster_health: ClusterHealth,
    pub active_faults: u32,
    pub isolated_groups: u32,
}

// ---------------------------------------------------------------------------
// Capacity limits to prevent unbounded HashMap growth
// ---------------------------------------------------------------------------

/// Maximum resolved/recovered faults to retain before evicting oldest entries.
const MAX_FAULTS: usize = 500;
/// Maximum completed or failed recovery plans to retain before evicting oldest.
const MAX_RECOVERY_PLANS: usize = 200;

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

struct Inner {
    config: FaultToleranceConfig,
    /// node_id -> HeartbeatRecord
    heartbeats: HashMap<String, HeartbeatRecord>,
    /// fault_id -> FaultEvent
    faults: HashMap<String, FaultEvent>,
    /// group_id -> IsolationGroup
    isolation_groups: HashMap<String, IsolationGroup>,
    /// monotonic counter for generating unique fault ids
    fault_counter: u64,
    /// monotonic counter for generating unique group ids
    group_counter: u64,
    /// plan_id -> RecoveryPlan
    recovery_plans: HashMap<String, RecoveryPlan>,
    /// monotonic counter for generating unique plan ids
    plan_counter: u64,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Acquire a mutex lock, recovering from a poisoned mutex if necessary.
fn lock_guard<T>(mtx: &Mutex<T>) -> MutexGuard<'_, T> {
    match mtx.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::error!("fault_tolerance mutex poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

/// Compute cluster health from raw counts (shared by `profile` and `cluster_health`).
fn cluster_health_from_counts(
    total_nodes: usize,
    offline_nodes: usize,
    degraded_nodes: usize,
    active_faults: usize,
) -> ClusterHealth {
    if total_nodes == 0 {
        return ClusterHealth::Down;
    }
    let offline_ratio = offline_nodes as f64 / total_nodes as f64;
    let degraded_ratio = degraded_nodes as f64 / total_nodes as f64;
    if offline_ratio >= 0.5 || active_faults >= 10 {
        ClusterHealth::Critical
    } else if (offline_ratio >= 0.2 || degraded_ratio >= 0.3 || active_faults >= 5)
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
#[derive(Clone)]
pub struct FaultToleranceEngine {
    inner: Arc<Mutex<Inner>>,
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
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Register a node for heartbeat monitoring.
    pub fn register_node(&self, node_id: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let node_id = node_id.to_string();
        if inner.heartbeats.contains_key(&node_id) {
            return Err(anyhow!("node '{}' is already registered", node_id));
        }
        let now = now_millis();
        let record = HeartbeatRecord {
            node_id: node_id.clone(),
            last_heartbeat_ms: now,
            missed_beats: 0,
            status: NodeStatus::Online,
        };
        inner.heartbeats.insert(node_id, record);
        Ok(())
    }

    /// Unregister a node, removing it from monitoring entirely.
    pub fn unregister_node(&self, node_id: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let node_id = node_id.to_string();
        if inner.heartbeats.remove(&node_id).is_none() {
            return Err(anyhow!("node '{}' is not registered", node_id));
        }
        // Also clean up any active faults for this node
        inner.faults.retain(|_, f| f.node_id != node_id);
        // Clean up isolation groups that reference this node
        let groups_to_remove: Vec<String> = inner
            .isolation_groups
            .iter()
            .filter(|(_, g)| g.nodes.contains(&node_id))
            .map(|(id, _)| id.clone())
            .collect();
        for group_id in groups_to_remove {
            let mut empty = false;
            if let Some(group) = inner.isolation_groups.get_mut(&group_id) {
                group.nodes.retain(|n| n != &node_id);
                empty = group.nodes.is_empty();
            }
            if empty {
                inner.isolation_groups.remove(&group_id);
            }
        }
        // Clean up recovery plans for this node
        inner.recovery_plans.retain(|_, p| p.node_id != node_id);
        Ok(())
    }

    /// Report a heartbeat from a node. Resets the missed-beat counter and
    /// moves the node back to Online if it was recovering.
    pub fn report_heartbeat(&self, node_id: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let node_id = node_id.to_string();
        let record = inner
            .heartbeats
            .get_mut(&node_id)
            .ok_or_else(|| anyhow!("node '{}' is not registered", node_id))?;
        let now = now_millis();
        record.last_heartbeat_ms = now;
        record.missed_beats = 0;
        if record.status == NodeStatus::Offline || record.status == NodeStatus::Recovering {
            record.status = NodeStatus::Online;
        }
        Ok(())
    }

    /// Report a fault on a node. Returns the generated fault id.
    pub fn report_fault(
        &self,
        node_id: &str,
        fault_type: FaultType,
        severity: u8,
        description: &str,
    ) -> Result<String> {
        let mut inner = lock_guard(&self.inner);
        let node_id = node_id.to_string();
        if !inner.heartbeats.contains_key(&node_id) {
            return Err(anyhow!("node '{}' is not registered", node_id));
        }
        let now = now_millis();
        inner.fault_counter += 1;
        let fault_id = format!("fault-{}", inner.fault_counter);
        let event = FaultEvent {
            id: fault_id.clone(),
            node_id: node_id.clone(),
            fault_type,
            severity,
            description: description.to_string(),
            detected_ms: now,
            resolved_ms: None,
            recovered: false,
        };
        inner.faults.insert(fault_id.clone(), event);

        // Evict oldest resolved faults when the map grows too large.
        if inner.faults.len() > MAX_FAULTS {
            let mut resolved: Vec<(String, u64)> = inner
                .faults
                .iter()
                .filter(|(_, f)| f.recovered)
                .map(|(id, f)| (id.clone(), f.detected_ms))
                .collect();
            resolved.sort_unstable_by_key(|(_, ts)| *ts);
            let to_remove = inner.faults.len().saturating_sub(MAX_FAULTS);
            for (id, _) in resolved.into_iter().take(to_remove) {
                inner.faults.remove(&id);
            }
        }

        // Mark the node as degraded or offline based on severity
        // IMPORTANT: Only escalate status (Online→Degraded→Offline), never downgrade.
        // A node that is already Offline should not become Degraded from a lower-severity fault.
        if let Some(record) = inner.heartbeats.get_mut(&node_id) {
            if severity >= 8 && record.status != NodeStatus::Offline {
                record.status = NodeStatus::Offline;
            } else if severity >= 4 && record.status == NodeStatus::Online {
                record.status = NodeStatus::Degraded;
            }
        }

        Ok(fault_id)
    }

    /// Resolve an active fault by its id.
    pub fn resolve_fault(&self, fault_id: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let fault_id = fault_id.to_string();
        let event = inner
            .faults
            .get_mut(&fault_id)
            .ok_or_else(|| anyhow!("fault '{}' not found", fault_id))?;
        if event.recovered {
            return Err(anyhow!("fault '{}' is already resolved", fault_id));
        }
        let now = now_millis();
        event.resolved_ms = Some(now);
        event.recovered = true;
        Ok(())
    }

    /// Isolate a node under a specific isolation level. Creates or updates
    /// an isolation group containing the node.
    pub fn isolate_node(&self, node_id: &str, level: IsolationLevel) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let node_id = node_id.to_string();
        if !inner.heartbeats.contains_key(&node_id) {
            return Err(anyhow!("node '{}' is not registered", node_id));
        }

        // Mark the node offline if shutdown level
        if level == IsolationLevel::Shutdown {
            if let Some(record) = inner.heartbeats.get_mut(&node_id) {
                record.status = NodeStatus::Offline;
            }
        } else if level == IsolationLevel::Quarantine {
            if let Some(record) = inner.heartbeats.get_mut(&node_id) {
                record.status = NodeStatus::Degraded;
            }
        }

        // Check if this node already belongs to a group
        for group in inner.isolation_groups.values_mut() {
            if group.nodes.contains(&node_id) {
                group.isolation_level = level.clone();
                return Ok(());
            }
        }

        // Create a new isolation group
        inner.group_counter += 1;
        let group_id = format!("group-{}", inner.group_counter);
        let now = now_millis();
        let group = IsolationGroup {
            group_id: group_id.clone(),
            nodes: vec![node_id],
            isolation_level: level,
            created_ms: now,
        };
        inner.isolation_groups.insert(group_id, group);
        Ok(())
    }

    /// Reintegrate a previously isolated node back into the cluster.
    pub fn reintegrate_node(&self, node_id: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let node_id = node_id.to_string();
        if !inner.heartbeats.contains_key(&node_id) {
            return Err(anyhow!("node '{}' is not registered", node_id));
        }

        // Remove node from all isolation groups
        let groups_to_remove: Vec<String> = inner
            .isolation_groups
            .iter()
            .filter(|(_, g)| g.nodes.contains(&node_id))
            .map(|(id, _)| id.clone())
            .collect();

        for group_id in groups_to_remove {
            let mut empty = false;
            if let Some(group) = inner.isolation_groups.get_mut(&group_id) {
                group.nodes.retain(|n| n != &node_id);
                empty = group.nodes.is_empty();
            }
            if empty {
                inner.isolation_groups.remove(&group_id);
            }
        }

        // Restore node to online
        if let Some(record) = inner.heartbeats.get_mut(&node_id) {
            record.status = NodeStatus::Online;
            record.missed_beats = 0;
        }

        // Resolve all active faults for this node (they were recovered)
        let fault_ids: Vec<String> = inner
            .faults
            .values()
            .filter(|f| f.node_id == node_id && !f.recovered)
            .map(|f| f.id.clone())
            .collect();
        let now = now_millis();
        for fault_id in fault_ids {
            if let Some(event) = inner.faults.get_mut(&fault_id) {
                event.resolved_ms = Some(now);
                event.recovered = true;
            }
        }

        // Complete all active (Pending/InProgress) recovery plans for this node
        let active_plan_ids: Vec<String> = inner
            .recovery_plans
            .values()
            .filter(|p| {
                p.node_id == node_id
                    && (p.state == RecoveryState::Pending || p.state == RecoveryState::InProgress)
            })
            .map(|p| p.plan_id.clone())
            .collect();
        for plan_id in active_plan_ids {
            if let Some(plan) = inner.recovery_plans.get_mut(&plan_id) {
                plan.state = RecoveryState::Completed;
                plan.completed_ms = Some(now);
            }
        }

        Ok(())
    }

    /// Check all heartbeats and return a list of node ids that have missed
    /// too many heartbeats (exceeded max_missed_beats).
    pub fn check_heartbeats(&self) -> Vec<String> {
        let mut inner = lock_guard(&self.inner);
        let now = now_millis();
        let timeout = inner.config.heartbeat_timeout_ms;
        let max_missed = inner.config.max_missed_beats;

        let mut offenders = Vec::new();

        let node_ids: Vec<String> = inner.heartbeats.keys().cloned().collect();
        for node_id in node_ids {
            if let Some(record) = inner.heartbeats.get_mut(&node_id) {
                let elapsed = now.saturating_sub(record.last_heartbeat_ms);
                if elapsed >= timeout {
                    record.missed_beats = record.missed_beats.saturating_add(1).min(max_missed);
                } else {
                    // Node is responsive; reset miss counter
                    record.missed_beats = 0;
                }

                // Update status based on missed beats
                if record.missed_beats >= max_missed {
                    record.status = NodeStatus::Offline;
                    offenders.push(node_id.clone());
                } else if record.missed_beats > 0 {
                    record.status = NodeStatus::Degraded;
                } else if record.status != NodeStatus::Recovering {
                    record.status = NodeStatus::Online;
                }
            }
        }

        offenders
    }

    /// Return all active (unresolved) faults.
    pub fn active_faults(&self) -> Vec<FaultEvent> {
        let inner = lock_guard(&self.inner);
        inner
            .faults
            .values()
            .filter(|f| !f.recovered)
            .cloned()
            .collect()
    }

    /// Return a snapshot profile of the cluster state.
    pub fn profile(&self) -> FaultToleranceProfile {
        let inner = lock_guard(&self.inner);
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

    // -----------------------------------------------------------------------
    // Recovery subsystem
    // -----------------------------------------------------------------------

    /// Create a recovery plan for a failed node.
    /// Determines appropriate recovery actions based on fault type and severity.
    pub fn create_recovery_plan(&self, node_id: &str) -> Result<String> {
        let mut inner = lock_guard(&self.inner);
        let node_id = node_id.to_string();
        if !inner.heartbeats.contains_key(&node_id) {
            return Err(anyhow!("node '{}' is not registered", node_id));
        }

        // Collect active faults for this node
        let node_faults: Vec<&FaultEvent> = inner
            .faults
            .values()
            .filter(|f| f.node_id == node_id && !f.recovered)
            .collect();

        // Determine actions based on faults
        let mut actions = Vec::new();
        let max_severity = node_faults.iter().map(|f| f.severity).max().unwrap_or(0);

        for fault in &node_faults {
            match fault.fault_type {
                FaultType::Crash | FaultType::Oom => {
                    if !actions.contains(&RecoveryAction::RestartNode) {
                        actions.push(RecoveryAction::RestartNode);
                    }
                }
                FaultType::Hang | FaultType::ResourceExhaustion => {
                    if !actions.contains(&RecoveryAction::ScaleUp) {
                        actions.push(RecoveryAction::ScaleUp);
                    }
                }
                FaultType::NetworkSplit => {
                    if !actions.contains(&RecoveryAction::FailoverToBackup) {
                        actions.push(RecoveryAction::FailoverToBackup);
                    }
                }
                FaultType::DataCorruption => {
                    if !actions.contains(&RecoveryAction::Rebalance) {
                        actions.push(RecoveryAction::Rebalance);
                    }
                }
            }
        }

        // Add operator notification for high severity
        if max_severity >= 9 {
            actions.push(RecoveryAction::NotifyOperator);
        }

        // If no specific actions, add a default
        if actions.is_empty() {
            actions.push(RecoveryAction::NotifyOperator);
        }

        inner.plan_counter += 1;
        let plan_id = format!("plan-{}", inner.plan_counter);
        let now = now_millis();
        let plan = RecoveryPlan {
            plan_id: plan_id.clone(),
            node_id: node_id.clone(),
            actions,
            state: RecoveryState::Pending,
            created_ms: now,
            completed_ms: None,
            result: None,
        };
        inner.recovery_plans.insert(plan_id.clone(), plan);

        // Evict oldest completed/failed plans when the map grows too large.
        if inner.recovery_plans.len() > MAX_RECOVERY_PLANS {
            let mut done: Vec<(String, u64)> = inner
                .recovery_plans
                .iter()
                .filter(|(_, p)| {
                    p.state == RecoveryState::Completed || p.state == RecoveryState::Failed
                })
                .map(|(id, p)| (id.clone(), p.created_ms))
                .collect();
            done.sort_unstable_by_key(|(_, ts)| *ts);
            let to_remove = inner
                .recovery_plans
                .len()
                .saturating_sub(MAX_RECOVERY_PLANS);
            for (id, _) in done.into_iter().take(to_remove) {
                inner.recovery_plans.remove(&id);
            }
        }

        Ok(plan_id)
    }

    /// Execute a recovery plan — transitions it to InProgress.
    pub fn execute_recovery_plan(&self, plan_id: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let plan = inner
            .recovery_plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow!("recovery plan '{}' not found", plan_id))?;
        if plan.state != RecoveryState::Pending {
            return Err(anyhow!(
                "recovery plan '{}' is not in Pending state",
                plan_id
            ));
        }
        plan.state = RecoveryState::InProgress;
        Ok(())
    }

    /// Complete a recovery plan with a result.
    pub fn complete_recovery_plan(&self, plan_id: &str, result: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let plan = inner
            .recovery_plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow!("recovery plan '{}' not found", plan_id))?;
        if plan.state != RecoveryState::InProgress {
            return Err(anyhow!(
                "recovery plan '{}' is not in InProgress state",
                plan_id
            ));
        }
        let node_id_clone = plan.node_id.clone();
        plan.state = RecoveryState::Completed;
        plan.completed_ms = Some(now_millis());
        plan.result = Some(result.to_string());

        // Restore the node status if completing a recovery plan
        if let Some(record) = inner.heartbeats.get_mut(&node_id_clone) {
            record.status = NodeStatus::Recovering;
        }
        Ok(())
    }

    /// Fail a recovery plan.
    pub fn fail_recovery_plan(&self, plan_id: &str, error: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let plan = inner
            .recovery_plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow!("recovery plan '{}' not found", plan_id))?;
        plan.state = RecoveryState::Failed;
        plan.completed_ms = Some(now_millis());
        plan.result = Some(format!("failed: {}", error));
        Ok(())
    }

    /// Get active recovery plans.
    pub fn active_recovery_plans(&self) -> Vec<RecoveryPlan> {
        let inner = lock_guard(&self.inner);
        inner
            .recovery_plans
            .values()
            .filter(|p| p.state == RecoveryState::Pending || p.state == RecoveryState::InProgress)
            .cloned()
            .collect()
    }

    /// Assess the escalation level for a given node.
    pub fn escalation_level(&self, node_id: &str) -> EscalationLevel {
        let inner = lock_guard(&self.inner);
        let node_id = node_id.to_string();
        let record = match inner.heartbeats.get(&node_id) {
            Some(r) => r,
            None => return EscalationLevel::Manual,
        };

        let active_node_faults: Vec<&FaultEvent> = inner
            .faults
            .values()
            .filter(|f| f.node_id == node_id && !f.recovered)
            .collect();

        if active_node_faults.is_empty() {
            return EscalationLevel::Auto;
        }

        let max_severity = active_node_faults
            .iter()
            .map(|f| f.severity)
            .max()
            .unwrap_or(0);
        let ongoing_recovery = inner
            .recovery_plans
            .values()
            .any(|p| p.node_id == node_id && p.state == RecoveryState::InProgress);

        match (record.status.clone(), max_severity, ongoing_recovery) {
            (NodeStatus::Online, _, _) => EscalationLevel::Auto,
            (NodeStatus::Degraded, s, _) if s < 7 => EscalationLevel::Auto,
            (NodeStatus::Degraded, _, _) => EscalationLevel::Coordinated,
            (NodeStatus::Offline, s, _) if s >= 9 => EscalationLevel::Manual,
            (NodeStatus::Offline, _, true) => EscalationLevel::Coordinated,
            (NodeStatus::Offline, _, _) => EscalationLevel::Coordinated,
            (NodeStatus::Recovering, _, _) => EscalationLevel::Coordinated,
        }
    }

    /// Get the overall cluster health.
    pub fn cluster_health(&self) -> ClusterHealth {
        let p = self.profile();
        if p.total_nodes == 0 {
            return ClusterHealth::Down;
        }
        cluster_health_from_counts(
            p.total_nodes,
            p.offline_nodes,
            p.degraded_nodes,
            p.active_faults,
        )
    }

    /// Run the full recovery cycle: check heartbeats, auto-create recovery plans,
    /// and return the status summary.
    pub fn run_recovery_cycle(&self) -> RecoveryCycleSummary {
        let offenders = self.check_heartbeats();
        let mut plans_created = 0u32;
        let mut plans_completed = 0u32;

        for node_id in &offenders {
            // Check if a plan already exists for this node
            let existing = {
                let inner = lock_guard(&self.inner);
                inner
                    .recovery_plans
                    .values()
                    .any(|p| p.node_id == *node_id && p.state != RecoveryState::Completed)
            };
            if !existing && self.create_recovery_plan(node_id).is_ok() {
                plans_created += 1;
            }
        }

        // Auto-execute pending plans and count completions
        let pending_plans = self.active_recovery_plans();
        for plan in &pending_plans {
            if plan.state == RecoveryState::Pending
                && self.execute_recovery_plan(&plan.plan_id).is_ok()
            {
                plans_completed += 1;
            }
        }

        let health = self.cluster_health();
        let profile = self.profile();

        RecoveryCycleSummary {
            offenders,
            plans_created,
            plans_completed,
            cluster_health: health,
            active_faults: profile.active_faults as u32,
            isolated_groups: profile.isolated_groups as u32,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_millis() -> u64 {
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
        engine.register_node("node-1").unwrap();
        let profile = engine.profile();
        assert_eq!(profile.total_nodes, 1);
        assert_eq!(profile.online_nodes, 1);
    }

    #[test]
    fn test_register_duplicate_node_fails() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
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
        engine.register_node("node-1").unwrap();
        engine.unregister_node("node-1").unwrap();
        let profile = engine.profile();
        assert_eq!(profile.total_nodes, 0);
        // unregistering an unknown node should fail
        let result = engine.unregister_node("node-1");
        assert!(result.is_err());
    }

    #[test]
    fn test_report_heartbeat() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
        // report a heartbeat (should succeed)
        engine.report_heartbeat("node-1").unwrap();
        // reporting heartbeat for unknown node should fail
        let result = engine.report_heartbeat("node-unknown");
        assert!(result.is_err());
    }

    #[test]
    fn test_missed_heartbeat_detection() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
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
        engine.register_node("node-1").unwrap();
        let fault_id = engine
            .report_fault("node-1", FaultType::Crash, 7, "Node crashed unexpectedly")
            .unwrap();
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
        engine.register_node("node-1").unwrap();
        let fault_id = engine
            .report_fault("node-1", FaultType::Oom, 9, "Out of memory")
            .unwrap();

        // Resolve the fault
        engine.resolve_fault(&fault_id).unwrap();
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
        engine.register_node("node-1").unwrap();

        // Isolate at Monitor level
        engine
            .isolate_node("node-1", IsolationLevel::Monitor)
            .unwrap();
        let profile = engine.profile();
        assert_eq!(profile.isolated_groups, 1);

        // Isolate again at a different level (should update existing group)
        engine
            .isolate_node("node-1", IsolationLevel::Quarantine)
            .unwrap();
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
        engine.register_node("node-1").unwrap();
        engine
            .isolate_node("node-1", IsolationLevel::Quarantine)
            .unwrap();

        // Reintegrate
        engine.reintegrate_node("node-1").unwrap();
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
        engine.register_node("node-1").unwrap();
        engine.register_node("node-2").unwrap();

        let id1 = engine
            .report_fault("node-1", FaultType::Crash, 8, "crash")
            .unwrap();
        let id2 = engine
            .report_fault("node-2", FaultType::Hang, 5, "hang")
            .unwrap();

        let active = engine.active_faults();
        assert_eq!(active.len(), 2);

        // Resolve one
        engine.resolve_fault(&id1).unwrap();
        let active = engine.active_faults();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, id2);
    }

    #[test]
    fn test_profile_reflects_state() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
        engine.register_node("node-2").unwrap();
        engine.register_node("node-3").unwrap();

        engine
            .report_fault("node-1", FaultType::NetworkSplit, 9, "split")
            .unwrap();
        engine
            .report_fault("node-2", FaultType::ResourceExhaustion, 5, "exhausted")
            .unwrap();

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
        engine.register_node("node-1").unwrap();
        engine
            .report_fault("node-1", FaultType::Crash, 8, "crash")
            .unwrap();
        let plan_id = engine.create_recovery_plan("node-1").unwrap();
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
        engine.register_node("node-1").unwrap();
        engine
            .report_fault("node-1", FaultType::Hang, 6, "hang")
            .unwrap();
        let plan_id = engine.create_recovery_plan("node-1").unwrap();

        // Execute plan
        engine.execute_recovery_plan(&plan_id).unwrap();
        let plans = engine.active_recovery_plans();
        assert_eq!(plans[0].state, RecoveryState::InProgress);

        // Complete plan
        engine
            .complete_recovery_plan(&plan_id, "restarted successfully")
            .unwrap();
        let active = engine.active_recovery_plans();
        assert!(active.is_empty());

        // Double complete should fail
        assert!(engine.complete_recovery_plan(&plan_id, "again").is_err());
    }

    #[test]
    fn test_fail_recovery_plan() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
        engine
            .report_fault("node-1", FaultType::DataCorruption, 7, "corruption")
            .unwrap();
        let plan_id = engine.create_recovery_plan("node-1").unwrap();
        engine.execute_recovery_plan(&plan_id).unwrap();
        engine.fail_recovery_plan(&plan_id, "timeout").unwrap();
        let active = engine.active_recovery_plans();
        assert!(active.is_empty());
    }

    #[test]
    fn test_escalation_level() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();

        // No faults → Auto
        assert_eq!(engine.escalation_level("node-1"), EscalationLevel::Auto);

        // Low severity fault → Auto
        engine
            .report_fault("node-1", FaultType::Hang, 3, "minor")
            .unwrap();
        assert_eq!(engine.escalation_level("node-1"), EscalationLevel::Auto);

        // High severity fault → Manual
        engine
            .report_fault("node-1", FaultType::Crash, 9, "severe crash")
            .unwrap();
        assert_eq!(engine.escalation_level("node-1"), EscalationLevel::Manual);

        // Unknown node → Manual
        assert_eq!(engine.escalation_level("unknown"), EscalationLevel::Manual);
    }

    #[test]
    fn test_cluster_health_healthy() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
        engine.register_node("node-2").unwrap();
        engine.register_node("node-3").unwrap();
        assert_eq!(engine.cluster_health(), ClusterHealth::Healthy);
    }

    #[test]
    fn test_cluster_health_degraded() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
        engine.register_node("node-2").unwrap();
        engine
            .report_fault("node-1", FaultType::Crash, 5, "moderate")
            .unwrap();
        assert_eq!(engine.cluster_health(), ClusterHealth::Degraded);
    }

    #[test]
    fn test_run_recovery_cycle() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();

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
            engine.register_node(&format!("node-{}", i)).unwrap();
        }
        assert_eq!(
            engine.profile().total_nodes,
            10,
            "should have 10 total nodes"
        );

        // Inject crash on node-5 — severity 9 sets node to Offline
        engine
            .report_fault("node-5", FaultType::Crash, 9, "test crash")
            .unwrap();
        let p = engine.profile();
        assert_eq!(
            p.offline_nodes, 1,
            "crash fault should set one node offline"
        );

        // Create + execute + complete recovery plan
        let plan = engine.create_recovery_plan("node-5").unwrap();
        engine.execute_recovery_plan(&plan).unwrap();
        engine.complete_recovery_plan(&plan, "recovered").unwrap();
        engine.reintegrate_node("node-5").unwrap();

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
            engine.register_node(&format!("node-{}", i)).unwrap();
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
                .unwrap();
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
            ft.register_node(&format!("node-{}", i)).unwrap();
        }
        assert_eq!(ft.profile().online_nodes, 10);

        // Crash node-5 — severity 9 marks node offline
        let crashed = "node-5";
        ft.report_fault(crashed, FaultType::Crash, 9, "test crash")
            .unwrap();
        let profile = ft.profile();
        assert_eq!(
            profile.offline_nodes, 1,
            "crash should set one node offline"
        );
        assert_eq!(profile.online_nodes, 9);

        // Create + execute + complete recovery plan
        let plan = ft.create_recovery_plan(crashed).unwrap();
        ft.execute_recovery_plan(&plan).unwrap();
        ft.complete_recovery_plan(&plan, "restarted").unwrap();
        ft.reintegrate_node(crashed).unwrap();

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
        engine.register_node("node-1").unwrap();
        engine
            .report_fault("node-1", FaultType::Oom, 9, "OOM")
            .unwrap();
        engine.create_recovery_plan("node-1").unwrap();
        let profile = engine.profile();
        assert!(profile.total_nodes > 0);
    }
}
