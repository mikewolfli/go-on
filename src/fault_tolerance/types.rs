//! Core data types for the fault tolerance module.

use serde::{Deserialize, Serialize};

/// Current status of a monitored node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    Online,
    Degraded,
    Offline,
    Recovering,
}

/// The type of fault detected on a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FaultType {
    Crash,
    Hang,
    Oom,
    NetworkSplit,
    NetworkTimeout,
    NetworkPartition,
    FileIOError,
    ProcessCrash,
    DataCorruption,
    ResourceExhaustion,
    RateLimit,
    AuthFailure,
    LatencySpike { delay_ms: u64 },
    PartialWrite,
}

impl FaultType {
    /// Human-readable label for a fault type.
    pub fn label(&self) -> &str {
        match self {
            FaultType::Crash => "crash",
            FaultType::Hang => "hang",
            FaultType::Oom => "oom",
            FaultType::NetworkSplit => "network_split",
            FaultType::NetworkTimeout => "network_timeout",
            FaultType::NetworkPartition => "network_partition",
            FaultType::FileIOError => "file_io_error",
            FaultType::ProcessCrash => "process_crash",
            FaultType::ResourceExhaustion => "resource_exhaustion",
            FaultType::DataCorruption => "data_corruption",
            FaultType::RateLimit => "rate_limit",
            FaultType::AuthFailure => "auth_failure",
            FaultType::LatencySpike { .. } => "latency_spike",
            FaultType::PartialWrite => "partial_write",
        }
    }
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

/// Result of a single consistency check after a recovery action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsistencyCheckEvent {
    pub check_id: String,
    pub check_type: String,
    pub passed: bool,
    pub details: String,
    pub timestamp_ms: u64,
}

/// Summary of a recovery cycle run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryCycleSummary {
    pub offenders: Vec<String>,
    pub plans_created: u32,
    pub plans_activated: u32,
    pub cluster_health: ClusterHealth,
    pub active_faults: u32,
    pub isolated_groups: u32,
    pub consistency_checks: Vec<ConsistencyCheckEvent>,
}

/// Configuration for cluster health threshold calculations.
///
/// Used by `cluster_health_from_counts` to determine whether the cluster
/// is Healthy, Degraded, or Critical based on node offline/degraded ratios
/// and unresolved fault counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterHealthConfig {
    /// Ratio of offline nodes at or above which the cluster is **Critical**.
    /// Default: 0.5
    pub healthy_threshold: f64,
    /// Ratio of offline nodes at or above which the cluster is **Degraded**
    /// (unless already Critical). Default: 0.2
    pub degraded_threshold: f64,
    /// Ratio of degraded nodes at or above which the cluster is **Degraded**
    /// (unless already Critical). Default: 0.3
    pub unhealthy_threshold: f64,
}

impl Default for ClusterHealthConfig {
    fn default() -> Self {
        Self {
            healthy_threshold: 0.5,
            degraded_threshold: 0.2,
            unhealthy_threshold: 0.3,
        }
    }
}

// ---------------------------------------------------------------------------
// Capacity limits to prevent unbounded HashMap growth
// ---------------------------------------------------------------------------

/// Maximum resolved/recovered faults to retain before evicting oldest entries.
pub(crate) const MAX_FAULTS: usize = 500;
/// Maximum completed or failed recovery plans to retain before evicting oldest.
pub(crate) const MAX_RECOVERY_PLANS: usize = 200;
/// Maximum heartbeat records to track before evicting the node with the oldest heartbeat.
pub(crate) const MAX_HEARTBEATS: usize = 1000;
/// Maximum isolation groups before evicting the oldest group.
pub(crate) const MAX_GROUPS: usize = 200;
