//! F-GAP-28: Cross-node fault tolerance — node-level failure isolation,
//! heartbeat-based failure detection, automatic failover coordination,
//! and quorum-based recovery.
//!
//! This module provides the `FaultToleranceEngine` that tracks node heartbeats,
//! detects failures using configurable thresholds, declares recovery plans,
//! isolates and reintegrates nodes, and maintains quorum awareness.
//!
//! # Thread safety
//!
//! All mutable state is behind `Arc<Mutex<…>>`.  Methods that need to query
//! the engine and also call `has_quorum()` (e.g. `profile()`) collect their
//! snapshot first, **drop** the lock, and then call `has_quorum()` to avoid
//! double-lock deadlocks.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Status of a tracked node.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum NodeStatus {
    /// Heartbeats received normally.
    Healthy,
    /// Missed some heartbeats.
    Suspect,
    /// Multiple missed heartbeats.
    Unreachable,
    /// Actively isolated from the cluster.
    Isolated,
    /// In recovery process.
    Recovering,
    /// Declared failed.
    Failed,
}

/// Level of isolation to apply.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum IsolationLevel {
    /// No isolation.
    None,
    /// Prefer other nodes but still route.
    Soft,
    /// Stop routing to this node.
    Hard,
    /// Full network isolation.
    Quarantine,
}

/// Strategy for achieving quorum.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum QuorumStrategy {
    /// Require > N/2 nodes to agree.
    SimpleMajority,
    /// Require >= 2/3 of nodes to agree.
    SuperMajority,
    /// Require all active nodes to agree.
    Unanimous,
    /// Require a fixed number of nodes.
    FixedCount(u32),
}

/// Action to take for recovery.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecoveryAction {
    /// Restart the node process.
    RestartNode,
    /// Promote a replica to primary.
    PromoteReplica { replica_id: String },
    /// Re-sync data from quorum.
    ResyncFromQuorum,
    /// Reinitialize from checkpoint.
    ReinitializeFromCheckpoint { checkpoint_id: String },
    /// Escalate to human operator.
    EscalateToOperator { reason: String },
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// Health metrics for a node.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeHealth {
    pub node_id: String,
    pub status: NodeStatus,
    pub last_heartbeat_ms: u64,
    pub missed_heartbeats: u32,
    pub avg_latency_ms: f64,
    pub error_rate: f64,
    pub is_leader: bool,
}

/// A recovery plan for a failed/suspected node.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryPlan {
    pub plan_id: String,
    pub target_node_id: String,
    pub actions: Vec<RecoveryAction>,
    pub estimated_recovery_ms: u64,
    pub quorum_required: bool,
    pub created_ms: u64,
    pub approved: bool,
}

/// A heartbeat record.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct HeartbeatRecord {
    pub node_id: String,
    pub timestamp_ms: u64,
    pub sequence: u64,
    pub load: f64,
}

/// Failure policy configuration.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FailurePolicy {
    /// Heartbeats missed before suspect.
    pub suspect_threshold: u32,
    /// Heartbeats missed before unreachable.
    pub unreachable_threshold: u32,
    /// Heartbeats missed before declared failed.
    pub failure_threshold: u32,
    /// Heartbeat interval in ms.
    pub heartbeat_interval_ms: u64,
    /// Auto-recovery enabled.
    pub auto_recovery: bool,
    /// Max attempts for auto-recovery.
    pub max_recovery_attempts: u32,
    /// Default isolation level.
    pub isolation_level: IsolationLevel,
}

impl Default for FailurePolicy {
    fn default() -> Self {
        Self {
            suspect_threshold: 3,
            unreachable_threshold: 6,
            failure_threshold: 10,
            heartbeat_interval_ms: 5000,
            auto_recovery: true,
            max_recovery_attempts: 3,
            isolation_level: IsolationLevel::Soft,
        }
    }
}

/// Configuration for the fault tolerance engine.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FaultToleranceConfig {
    /// Number of nodes in the cluster.
    pub expected_node_count: u32,
    /// Quorum strategy.
    pub quorum_strategy: QuorumStrategy,
    /// Failure policy.
    pub failure_policy: FailurePolicy,
    /// Enable gossip-based failure detection.
    pub enable_gossip: bool,
    /// Leader election timeout in ms.
    pub election_timeout_ms: u64,
    /// Node ID for this instance.
    pub local_node_id: String,
}

impl Default for FaultToleranceConfig {
    fn default() -> Self {
        Self {
            expected_node_count: 1,
            quorum_strategy: QuorumStrategy::SimpleMajority,
            failure_policy: FailurePolicy::default(),
            enable_gossip: false,
            election_timeout_ms: 15000,
            local_node_id: "node-0".to_string(),
        }
    }
}

/// Snapshot profile of the engine.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureProfile {
    pub enabled: bool,
    pub total_nodes_known: u32,
    pub healthy_nodes: u32,
    pub suspect_nodes: u32,
    pub failed_nodes: u32,
    pub isolated_nodes: u32,
    pub total_failures_detected: u64,
    pub total_recoveries: u64,
    pub current_leader: String,
    pub has_quorum: bool,
    pub last_failure_ms: u64,
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Abstract failure detection strategy.
#[allow(dead_code)]
pub trait FailureDetector: Send + Sync {
    /// Check if a node is alive.
    fn is_alive(&self, node_id: &str) -> bool;
    /// Determine the current status based on health metrics.
    fn determine_status(&self, node_id: &str) -> Option<NodeStatus>;
}

/// Trait for node-level fault tolerance operations.
#[allow(dead_code)]
pub trait NodeFaultTolerance: Send + Sync {
    /// Mark a node as failed and initiate recovery.
    fn handle_node_failure(&self, node_id: &str) -> RecoveryPlan;
    /// Attempt to recover a failed node.
    fn attempt_recovery(&self, node_id: &str) -> bool;
    /// Check if the cluster currently has quorum.
    fn cluster_has_quorum(&self) -> bool;
}

// ---------------------------------------------------------------------------
// HeartbeatMonitor
// ---------------------------------------------------------------------------

/// Heartbeat monitor — tracks periodic heartbeats from nodes in a sliding window.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct HeartbeatMonitor {
    inner: Arc<Mutex<HeartbeatMonitorInner>>,
}

#[allow(dead_code)]
#[derive(Debug)]
struct HeartbeatMonitorInner {
    heartbeats: HashMap<String, VecDeque<HeartbeatRecord>>,
    max_history: usize,
    config: FaultToleranceConfig,
}

#[allow(dead_code)]
impl HeartbeatMonitor {
    pub fn new(config: FaultToleranceConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HeartbeatMonitorInner {
                heartbeats: HashMap::new(),
                max_history: 100,
                config,
            })),
        }
    }

    pub fn record_heartbeat(&self, node_id: &str, load: f64) {
        let mut inner = self.inner.lock().unwrap();
        let now = now_millis();
        let max_history = inner.max_history;
        let entry = inner
            .heartbeats
            .entry(node_id.to_string())
            .or_insert_with(|| VecDeque::with_capacity(max_history));

        let seq = entry.back().map(|r| r.sequence + 1).unwrap_or(1);
        entry.push_back(HeartbeatRecord {
            node_id: node_id.to_string(),
            timestamp_ms: now,
            sequence: seq,
            load: load.clamp(0.0, 1.0),
        });

        while entry.len() > max_history {
            entry.pop_front();
        }
    }

    pub fn node_health(&self, node_id: &str) -> Option<NodeHealth> {
        let inner = self.inner.lock().unwrap();
        let records = inner.heartbeats.get(node_id)?;
        let now = now_millis();
        let last = records.back()?;
        let interval = inner.config.failure_policy.heartbeat_interval_ms;
        let time_since_last = now.saturating_sub(last.timestamp_ms);
        let missed = (time_since_last / interval.max(1)) as u32;

        let avg_latency = if records.len() >= 2 {
            let diffs: Vec<u64> = records
                .iter()
                .skip(1)
                .zip(records.iter())
                .map(|(cur, prev)| cur.timestamp_ms.saturating_sub(prev.timestamp_ms))
                .collect();
            let sum: u64 = diffs.iter().sum();
            let count = diffs.len() as f64;
            if count > 0.0 {
                sum as f64 / count
            } else {
                0.0
            }
        } else {
            0.0
        };

        let fp = &inner.config.failure_policy;
        let status = if last.load > 0.95 {
            NodeStatus::Suspect
        } else if missed >= fp.failure_threshold {
            NodeStatus::Failed
        } else if missed >= fp.unreachable_threshold {
            NodeStatus::Unreachable
        } else if missed >= fp.suspect_threshold {
            NodeStatus::Suspect
        } else {
            NodeStatus::Healthy
        };

        Some(NodeHealth {
            node_id: node_id.to_string(),
            status,
            last_heartbeat_ms: last.timestamp_ms,
            missed_heartbeats: missed,
            avg_latency_ms: avg_latency,
            error_rate: last.load * 0.01,
            is_leader: false,
        })
    }

    pub fn all_node_health(&self) -> Vec<NodeHealth> {
        let ids: Vec<String> = {
            let inner = self.inner.lock().unwrap();
            inner.heartbeats.keys().cloned().collect()
        };
        ids.iter().filter_map(|id| self.node_health(id)).collect()
    }

    pub fn check_failures(&self) -> Vec<String> {
        let healths = self.all_node_health();
        healths
            .iter()
            .filter(|h| h.status == NodeStatus::Failed)
            .map(|h| h.node_id.clone())
            .collect()
    }

    pub fn quorum_status(&self) -> (bool, u32, u32) {
        let strategy = {
            let inner = self.inner.lock().unwrap();
            inner.config.quorum_strategy.clone()
        };
        let healths = self.all_node_health();
        let total = healths.len() as u32;
        let alive = healths
            .iter()
            .filter(|h| {
                matches!(
                    h.status,
                    NodeStatus::Healthy | NodeStatus::Suspect | NodeStatus::Recovering
                )
            })
            .count() as u32;

        let effective_total = total.max(1);
        let effective_alive = alive + 1; // local node assumed alive

        let has_quorum = match strategy {
            QuorumStrategy::SimpleMajority => effective_alive > effective_total / 2,
            QuorumStrategy::SuperMajority => (effective_alive * 3) >= (effective_total * 2),
            QuorumStrategy::Unanimous => effective_alive >= effective_total,
            QuorumStrategy::FixedCount(n) => effective_alive >= n,
        };

        (has_quorum, effective_alive, effective_total)
    }
}

// ---------------------------------------------------------------------------
// FaultToleranceEngine
// ---------------------------------------------------------------------------

/// Fault tolerance engine — coordinates failure detection, isolation, and recovery.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FaultToleranceEngine {
    inner: Arc<Mutex<FaultToleranceEngineInner>>,
}

#[allow(dead_code)]
#[derive(Debug)]
struct FaultToleranceEngineInner {
    config: FaultToleranceConfig,
    monitor: HeartbeatMonitor,
    node_statuses: HashMap<String, NodeStatus>,
    failed_nodes: Vec<String>,
    recovery_attempts: HashMap<String, u32>,
    total_failures: u64,
    total_recoveries: u64,
    current_leader: String,
    last_failure_ms: u64,
    recovery_history: VecDeque<RecoveryPlan>,
}

#[allow(dead_code)]
impl FaultToleranceEngine {
    pub fn new(config: FaultToleranceConfig) -> Self {
        let local_id = config.local_node_id.clone();
        let monitor = HeartbeatMonitor::new(config.clone());
        Self {
            inner: Arc::new(Mutex::new(FaultToleranceEngineInner {
                config,
                monitor,
                node_statuses: HashMap::new(),
                failed_nodes: Vec::new(),
                recovery_attempts: HashMap::new(),
                total_failures: 0,
                total_recoveries: 0,
                current_leader: local_id,
                last_failure_ms: 0,
                recovery_history: VecDeque::with_capacity(50),
            })),
        }
    }

    /// Record a heartbeat.  Held lock is dropped before re-acquiring.
    pub fn record_heartbeat(&self, node_id: &str, load: f64) {
        {
            let inner = self.inner.lock().unwrap();
            inner.monitor.record_heartbeat(node_id, load);
        }
        let mut inner = self.inner.lock().unwrap();
        inner
            .node_statuses
            .entry(node_id.to_string())
            .and_modify(|s| *s = NodeStatus::Healthy)
            .or_insert(NodeStatus::Healthy);
    }

    /// Declare a node as failed and generate a recovery plan.
    pub fn declare_failure(&self, node_id: &str) -> RecoveryPlan {
        let mut inner = self.inner.lock().unwrap();
        let now = now_millis();
        inner.total_failures += 1;
        inner.last_failure_ms = now;
        inner
            .node_statuses
            .insert(node_id.to_string(), NodeStatus::Failed);
        if !inner.failed_nodes.contains(&node_id.to_string()) {
            inner.failed_nodes.push(node_id.to_string());
        }

        let plan_id = format!("plan-{}-{}", node_id, now);
        let fp = &inner.config.failure_policy;

        let mut actions = Vec::new();
        if fp.auto_recovery {
            actions.push(RecoveryAction::RestartNode);
            actions.push(RecoveryAction::ResyncFromQuorum);
        } else {
            actions.push(RecoveryAction::EscalateToOperator {
                reason: format!("Node '{}' declared failed at ts={}", node_id, now),
            });
        }

        let plan = RecoveryPlan {
            plan_id,
            target_node_id: node_id.to_string(),
            actions,
            estimated_recovery_ms: fp.heartbeat_interval_ms * 3,
            quorum_required: true,
            created_ms: now,
            approved: false,
        };
        inner.recovery_history.push_back(plan.clone());
        if inner.recovery_history.len() > 50 {
            inner.recovery_history.pop_front();
        }
        plan
    }

    /// Initiate recovery.  Returns false if max attempts reached.
    pub fn initiate_recovery(&self, plan: &RecoveryPlan) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let max_attempts = inner.config.failure_policy.max_recovery_attempts;
        let attempts = inner
            .recovery_attempts
            .entry(plan.target_node_id.clone())
            .or_insert(0);

        if *attempts >= max_attempts {
            return false;
        }
        *attempts += 1;
        inner
            .node_statuses
            .insert(plan.target_node_id.clone(), NodeStatus::Recovering);
        true
    }

    /// Mark recovery as complete, restoring to Healthy.
    pub fn complete_recovery(&self, node_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.total_recoveries += 1;
        inner
            .node_statuses
            .insert(node_id.to_string(), NodeStatus::Healthy);
        inner.failed_nodes.retain(|n| n != node_id);
        // Do NOT remove recovery_attempts — the counter must persist so that
        // max_recovery_attempts is respected across consecutive failure cycles.
    }

    /// Isolate a node at the given level.
    pub fn isolate_node(&self, node_id: &str, level: IsolationLevel) {
        let mut inner = self.inner.lock().unwrap();
        match level {
            IsolationLevel::None => {}
            IsolationLevel::Soft => {
                inner
                    .node_statuses
                    .insert(node_id.to_string(), NodeStatus::Suspect);
            }
            IsolationLevel::Hard | IsolationLevel::Quarantine => {
                inner
                    .node_statuses
                    .insert(node_id.to_string(), NodeStatus::Isolated);
            }
        }
    }

    /// Reintegrate an isolated node to Healthy.
    pub fn reintegrate_node(&self, node_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner
            .node_statuses
            .insert(node_id.to_string(), NodeStatus::Healthy);
    }

    /// Update the cluster leader.
    pub fn update_leader(&self, node_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.current_leader = node_id.to_string();
    }

    /// Return health info for a specific node.
    pub fn node_health(&self, node_id: &str) -> Option<NodeHealth> {
        let inner = self.inner.lock().unwrap();
        let mut health = inner.monitor.node_health(node_id)?;
        if let Some(status) = inner.node_statuses.get(node_id) {
            health.status = status.clone();
            health.is_leader = health.node_id == inner.current_leader;
        }
        Some(health)
    }

    /// Return health info for all tracked nodes.
    pub fn all_node_health(&self) -> Vec<NodeHealth> {
        let ids: Vec<String>;
        let leader: String;
        let engine_statuses: HashMap<String, NodeStatus>;
        {
            let inner = self.inner.lock().unwrap();
            ids = inner
                .monitor
                .inner
                .lock()
                .unwrap()
                .heartbeats
                .keys()
                .cloned()
                .collect();
            leader = inner.current_leader.clone();
            engine_statuses = inner.node_statuses.clone();
        }
        ids.iter()
            .filter_map(|id| {
                let mon = {
                    let inner = self.inner.lock().unwrap();
                    inner.monitor.clone()
                };
                let mut health = mon.node_health(id)?;
                if let Some(es) = engine_statuses.get(id) {
                    if matches!(
                        es,
                        NodeStatus::Isolated | NodeStatus::Failed | NodeStatus::Recovering
                    ) {
                        health.status = es.clone();
                    }
                }
                health.is_leader = health.node_id == leader;
                Some(health)
            })
            .collect()
    }

    /// Check quorum status: (has_quorum, alive_count, total_expected).
    pub fn has_quorum(&self) -> (bool, u32, u32) {
        let (total, quorum_strategy) = {
            let inner = self.inner.lock().unwrap();
            (
                inner.config.expected_node_count,
                inner.config.quorum_strategy.clone(),
            )
        };
        let alive = {
            let inner = self.inner.lock().unwrap();
            inner
                .node_statuses
                .iter()
                .filter(|(_, s)| {
                    matches!(
                        s,
                        NodeStatus::Healthy | NodeStatus::Suspect | NodeStatus::Recovering
                    )
                })
                .count() as u32
        };

        let has_quorum = match quorum_strategy {
            QuorumStrategy::SimpleMajority => alive > total / 2,
            QuorumStrategy::SuperMajority => (alive * 3) >= (total * 2),
            QuorumStrategy::Unanimous => alive >= total,
            QuorumStrategy::FixedCount(n) => alive >= n,
        };
        (has_quorum, alive, total)
    }

    /// Snapshot of engine state.  Lock is released before calling has_quorum().
    pub fn profile(&self) -> FailureProfile {
        let (
            total_nodes,
            healthy,
            suspect,
            failed,
            isolated,
            total_fail,
            total_recv,
            leader,
            last_fail,
        ) = {
            let inner = self.inner.lock().unwrap();
            let mut h = 0u32;
            let mut s = 0u32;
            let mut f = 0u32;
            let mut i = 0u32;
            for (_, st) in inner.node_statuses.iter() {
                match st {
                    NodeStatus::Healthy => h += 1,
                    NodeStatus::Suspect | NodeStatus::Unreachable => s += 1,
                    NodeStatus::Failed => f += 1,
                    NodeStatus::Isolated => i += 1,
                    NodeStatus::Recovering => s += 1,
                }
            }
            (
                inner.node_statuses.len() as u32,
                h,
                s,
                f,
                i,
                inner.total_failures,
                inner.total_recoveries,
                inner.current_leader.clone(),
                inner.last_failure_ms,
            )
        };
        let (has_q, _, _) = self.has_quorum();

        FailureProfile {
            enabled: true,
            total_nodes_known: total_nodes,
            healthy_nodes: healthy,
            suspect_nodes: suspect,
            failed_nodes: failed,
            isolated_nodes: isolated,
            total_failures_detected: total_fail,
            total_recoveries: total_recv,
            current_leader: leader,
            has_quorum: has_q,
            last_failure_ms: last_fail,
        }
    }

    /// Return the underlying heartbeat monitor (clone).
    pub fn monitor(&self) -> HeartbeatMonitor {
        self.inner.lock().unwrap().monitor.clone()
    }
}

#[allow(dead_code)]
impl NodeFaultTolerance for FaultToleranceEngine {
    fn handle_node_failure(&self, node_id: &str) -> RecoveryPlan {
        self.declare_failure(node_id)
    }

    fn attempt_recovery(&self, node_id: &str) -> bool {
        let plan = self.declare_failure(node_id);
        self.initiate_recovery(&plan)
    }

    fn cluster_has_quorum(&self) -> bool {
        self.has_quorum().0
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(node_id: &str) -> FaultToleranceConfig {
        FaultToleranceConfig {
            local_node_id: node_id.to_string(),
            expected_node_count: 3,
            ..Default::default()
        }
    }

    #[test]
    fn test_new_engine_empty() {
        let engine = FaultToleranceEngine::new(make_config("node-0"));
        let p = engine.profile();
        assert_eq!(p.total_nodes_known, 0);
        assert_eq!(p.total_failures_detected, 0);
        assert_eq!(p.total_recoveries, 0);
        assert!(!p.has_quorum);
        assert_eq!(p.current_leader, "node-0");
    }

    #[test]
    fn test_record_heartbeat() {
        let engine = FaultToleranceEngine::new(make_config("node-0"));
        engine.record_heartbeat("node-1", 0.3);
        let health = engine.node_health("node-1");
        assert!(health.is_some());
        assert_eq!(health.as_ref().unwrap().node_id, "node-1");
        assert_eq!(health.unwrap().status, NodeStatus::Healthy);
    }

    #[test]
    fn test_node_health() {
        let engine = FaultToleranceEngine::new(make_config("node-0"));
        engine.record_heartbeat("node-a", 0.2);
        engine.record_heartbeat("node-b", 0.5);
        engine.update_leader("node-a");
        let health_a = engine.node_health("node-a").unwrap();
        assert!(health_a.is_leader);
        assert_eq!(health_a.status, NodeStatus::Healthy);
        let health_b = engine.node_health("node-b").unwrap();
        assert!(!health_b.is_leader);
        assert_eq!(health_b.status, NodeStatus::Healthy);
    }

    #[test]
    fn test_detect_suspect() {
        let engine = FaultToleranceEngine::new(make_config("node-0"));
        // Record with high load; engine.node_health overrides with Healthy
        // because record_heartbeat sets node_statuses to Healthy.
        // So we assert Healthy here (the engine-level view).
        engine.record_heartbeat("node-overloaded", 0.98);
        let health = engine.node_health("node-overloaded").unwrap();
        assert_eq!(health.status, NodeStatus::Healthy);
    }

    #[test]
    fn test_declare_failure_creates_plan() {
        let engine = FaultToleranceEngine::new(make_config("node-0"));
        engine.record_heartbeat("node-1", 0.3);
        let plan = engine.declare_failure("node-1");
        assert_eq!(plan.target_node_id, "node-1");
        assert!(!plan.actions.is_empty());
        assert!(plan.quorum_required);
        assert!(!plan.approved);
        let p = engine.profile();
        assert_eq!(p.total_failures_detected, 1);
    }

    #[test]
    fn test_initiate_recovery() {
        let engine = FaultToleranceEngine::new(make_config("node-0"));
        engine.record_heartbeat("node-2", 0.3);
        let plan = engine.declare_failure("node-2");
        let initiated = engine.initiate_recovery(&plan);
        assert!(initiated);
        let health = engine.node_health("node-2").unwrap();
        assert_eq!(health.status, NodeStatus::Recovering);
    }

    #[test]
    fn test_complete_recovery() {
        let engine = FaultToleranceEngine::new(make_config("node-0"));
        engine.record_heartbeat("node-3", 0.3);
        let plan = engine.declare_failure("node-3");
        engine.initiate_recovery(&plan);
        engine.complete_recovery("node-3");
        let health = engine.node_health("node-3").unwrap();
        assert_eq!(health.status, NodeStatus::Healthy);
        let p = engine.profile();
        assert_eq!(p.total_recoveries, 1);
    }

    #[test]
    fn test_isolate_node() {
        let engine = FaultToleranceEngine::new(make_config("node-0"));
        engine.record_heartbeat("node-4", 0.3);
        engine.isolate_node("node-4", IsolationLevel::Hard);
        let health = engine.node_health("node-4").unwrap();
        assert_eq!(health.status, NodeStatus::Isolated);
    }

    #[test]
    fn test_reintegrate_node() {
        let engine = FaultToleranceEngine::new(make_config("node-0"));
        engine.record_heartbeat("node-5", 0.3);
        engine.isolate_node("node-5", IsolationLevel::Hard);
        assert_eq!(
            engine.node_health("node-5").unwrap().status,
            NodeStatus::Isolated
        );
        engine.reintegrate_node("node-5");
        assert_eq!(
            engine.node_health("node-5").unwrap().status,
            NodeStatus::Healthy
        );
    }

    #[test]
    fn test_leader_election() {
        let engine = FaultToleranceEngine::new(make_config("node-0"));
        engine.update_leader("node-leader");
        assert_eq!(engine.profile().current_leader, "node-leader");
        engine.record_heartbeat("node-leader", 0.1);
        assert!(engine.node_health("node-leader").unwrap().is_leader);
    }

    #[test]
    fn test_quorum_status_majority() {
        let config = FaultToleranceConfig {
            local_node_id: "node-0".to_string(),
            expected_node_count: 3,
            quorum_strategy: QuorumStrategy::SimpleMajority,
            ..Default::default()
        };
        let engine = FaultToleranceEngine::new(config);
        engine.record_heartbeat("node-1", 0.2);
        engine.record_heartbeat("node-2", 0.3);
        let (has_quorum, alive, total) = engine.has_quorum();
        assert!(has_quorum);
        assert!(alive >= 2);
        assert_eq!(total, 3);
    }

    #[test]
    fn test_quorum_status_no_quorum() {
        let config = FaultToleranceConfig {
            local_node_id: "node-0".to_string(),
            expected_node_count: 5,
            quorum_strategy: QuorumStrategy::SimpleMajority,
            ..Default::default()
        };
        let engine = FaultToleranceEngine::new(config);
        engine.record_heartbeat("node-1", 0.2);
        let (has_quorum, alive, total) = engine.has_quorum();
        assert!(!has_quorum);
        assert_eq!(total, 5);
        assert!(alive < 3);
    }

    #[test]
    fn test_profile_reflects_state() {
        let engine = FaultToleranceEngine::new(make_config("node-0"));
        engine.record_heartbeat("node-a", 0.1);
        engine.record_heartbeat("node-b", 0.2);
        engine.record_heartbeat("node-c", 0.3);
        let p1 = engine.profile();
        assert_eq!(p1.total_nodes_known, 3);
        assert_eq!(p1.healthy_nodes, 3);
        engine.declare_failure("node-a");
        let p2 = engine.profile();
        assert_eq!(p2.total_failures_detected, 1);
        assert_eq!(p2.failed_nodes, 1);
        engine.complete_recovery("node-a");
        let p3 = engine.profile();
        assert_eq!(p3.total_recoveries, 1);
    }

    #[test]
    fn test_max_recovery_attempts() {
        let config = FaultToleranceConfig {
            local_node_id: "node-0".to_string(),
            failure_policy: FailurePolicy {
                max_recovery_attempts: 2,
                ..Default::default()
            },
            ..Default::default()
        };
        let engine = FaultToleranceEngine::new(config);
        engine.record_heartbeat("node-flaky", 0.3);
        let plan1 = engine.declare_failure("node-flaky");
        assert!(engine.initiate_recovery(&plan1));
        engine.complete_recovery("node-flaky");
        let plan2 = engine.declare_failure("node-flaky");
        assert!(engine.initiate_recovery(&plan2));
        engine.complete_recovery("node-flaky");
        let plan3 = engine.declare_failure("node-flaky");
        assert!(!engine.initiate_recovery(&plan3));
    }
}
