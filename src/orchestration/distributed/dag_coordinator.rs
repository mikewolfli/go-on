//! Distributed DAG Coordinator (GAP-B52-22)
//!
//! Coordinates DAG execution across a distributed cluster using Raft-based
//! consistency for state replication, heartbeat + lease fault detection,
//! and automatic node reassignment on failure.

use crate::orchestration::distributed::remote_executor::{
    DagId, NodeId, NodeOutput, NodeRegistration, RemoteExecutor, TaskPacket,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio::time;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
#[derive(Debug, Error)]
pub enum DagCoordinatorError {
    #[error("DAG not found: {0}")]
    DagNotFound(DagId),

    #[error("node not found: {0}")]
    NodeNotFound(NodeId),

    #[error("node {0} is offline")]
    NodeOffline(NodeId),

    #[error("execution error: {0}")]
    ExecutionError(String),

    #[error("consensus error: {0}")]
    ConsensusError(String),

    #[error("state replication failed: {0}")]
    StateReplicationFailed(String),

    #[error("invalid state transition")]
    InvalidStateTransition,
}

// ---------------------------------------------------------------------------
// NodeState
// ---------------------------------------------------------------------------

#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum NodeState {
    Online,
    Offline,
    Suspect,
    Draining,
}

// ---------------------------------------------------------------------------
// DistributedDagState
// ---------------------------------------------------------------------------

#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub node_id: NodeId,
    pub address: String,
    pub port: u16,
    pub state: NodeState,
    pub last_heartbeat_ms: u64,
    pub lease_expiry_ms: u64,
}

#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
impl NodeInfo {
    pub fn new(node_id: NodeId, address: String, port: u16) -> Self {
        let now = current_timestamp_ms();
        Self {
            node_id,
            address,
            port,
            state: NodeState::Online,
            last_heartbeat_ms: now,
            lease_expiry_ms: now + 30_000, // 30s default lease
        }
    }

    pub fn is_lease_expired(&self) -> bool {
        current_timestamp_ms() > self.lease_expiry_ms
    }
}

// ---------------------------------------------------------------------------
// SchemaContract — JSON Schema contract validation for DAG node I/O
// ---------------------------------------------------------------------------

/// A schema contract that constrains the shape of a DAG node's input or
/// output payloads.  Contracts are expressed as JSON Schema documents
/// (draft-07 subset).  When set on a [`DagNodeAssignment`], the
/// orchestrator can validate at runtime that the actual data flowing
/// through the node conforms to the declared shape, preventing implicit
/// type drift across a multi-agent pipeline.
///
/// Both fields are optional — when `None` no validation is performed
/// for that direction.
#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaContract {
    /// JSON Schema document describing the expected input shape.
    pub input_schema: Option<serde_json::Value>,
    /// JSON Schema document describing the expected output shape.
    pub output_schema: Option<serde_json::Value>,
}

/// Validate `data` against the non-`None` schemas carried by `contract`.
///
/// Returns `Ok(())` when every present schema passes.  Returns
/// `Err(reason)` with a human-readable description of the first
/// constraint violation found.
#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
pub fn validate_contract(
    data: &serde_json::Value,
    contract: &SchemaContract,
) -> Result<(), String> {
    // ── helper: check a single optional JSON Schema ────────────────
    fn check(
        data: &serde_json::Value,
        schema: &Option<serde_json::Value>,
        label: &str,
    ) -> Result<(), String> {
        let Some(schema) = schema else { return Ok(()) };

        // Type constraint
        if let Some(ty) = schema.get("type").and_then(|v| v.as_str()) {
            let matches = match ty {
                "string" => data.is_string(),
                "number" | "integer" => data.is_number(),
                "boolean" => data.is_boolean(),
                "array" => data.is_array(),
                "object" => data.is_object(),
                "null" => data.is_null(),
                _ => true, // unknown type keyword — be lenient
            };
            if !matches {
                return Err(format!(
                    "contract violation ({label}): expected type '{ty}', got {}",
                    type_name_of(data)
                ));
            }
        }

        // Enum constraint
        if let Some(enum_vals) = schema.get("enum").and_then(|v| v.as_array()) {
            if !enum_vals.iter().any(|ev| ev == data) {
                return Err(format!(
                    "contract violation ({label}): value {} not in allowed enum",
                    data
                ));
            }
        }

        // Required properties (top-level object only)
        if data.is_object() {
            if let Some(required) = schema.get("required").and_then(|v| v.as_array()) {
                let obj = data.as_object().unwrap();
                for req in required {
                    if let Some(key) = req.as_str() {
                        if !obj.contains_key(key) {
                            return Err(format!(
                                "contract violation ({label}): missing required property '{}'",
                                key
                            ));
                        }
                    }
                }
            }
        }

        // Minimum / maximum (numbers)
        if let Some(n) = data.as_f64() {
            if let Some(min) = schema.get("minimum").and_then(|v| v.as_f64()) {
                if n < min {
                    return Err(format!(
                        "contract violation ({label}): value {n} < minimum {min}"
                    ));
                }
            }
            if let Some(max) = schema.get("maximum").and_then(|v| v.as_f64()) {
                if n > max {
                    return Err(format!(
                        "contract violation ({label}): value {n} > maximum {max}"
                    ));
                }
            }
        }

        // Min-length / max-length (strings)
        if let Some(s) = data.as_str() {
            if let Some(min) = schema.get("minLength").and_then(|v| v.as_u64()) {
                if (s.len() as u64) < min {
                    return Err(format!(
                        "contract violation ({label}): string length {} < minLength {min}",
                        s.len()
                    ));
                }
            }
            if let Some(max) = schema.get("maxLength").and_then(|v| v.as_u64()) {
                if (s.len() as u64) > max {
                    return Err(format!(
                        "contract violation ({label}): string length {} > maxLength {max}",
                        s.len()
                    ));
                }
            }
        }

        // Min-items / max-items (arrays)
        if let Some(arr) = data.as_array() {
            if let Some(min) = schema.get("minItems").and_then(|v| v.as_u64()) {
                if (arr.len() as u64) < min {
                    return Err(format!(
                        "contract violation ({label}): array length {} < minItems {min}",
                        arr.len()
                    ));
                }
            }
            if let Some(max) = schema.get("maxItems").and_then(|v| v.as_u64()) {
                if (arr.len() as u64) > max {
                    return Err(format!(
                        "contract violation ({label}): array length {} > maxItems {max}",
                        arr.len()
                    ));
                }
            }
        }

        Ok(())
    }

    check(data, &contract.input_schema, "input")?;
    check(data, &contract.output_schema, "output")?;
    Ok(())
}

/// Return a human-readable type name for a JSON value.
fn type_name_of(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

// ---------------------------------------------------------------------------
// DagNodeAssignment
// ---------------------------------------------------------------------------

#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNodeAssignment {
    pub dag_node_id: String, // Logical node ID within the DAG
    pub tool_name: String,
    pub assigned_node_id: Option<NodeId>,
    pub output: Option<NodeOutput>,
    pub error: Option<String>,
    pub completed: bool,
    /// Optional schema contract for validating this node's input and output.
    pub contract: Option<SchemaContract>,
}

#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagExecutionPlan {
    pub dag_id: DagId,
    pub assignments: Vec<DagNodeAssignment>,
    pub adjacency: HashMap<String, Vec<String>>, // dag_node_id -> dependency IDs
    pub created_at_ms: u64,
    pub status: DagStatus,
}

impl DagExecutionPlan {
    /// Validate **all** node input/output contracts in this plan.
    ///
    /// Returns `Ok(())` when every node satisfies its declared contract.
    /// Returns the first `Err(reason)` otherwise, describing which node
    /// failed and why.
    ///
    /// *Input validation* checks the node's output (the result of its
    /// execution) against the **output_schema** of its contract.
    ///
    /// If a node has no contract (`None`), it is skipped.
    #[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG contract validation
    pub fn validate_all_contracts(&self) -> Result<(), String> {
        for assign in &self.assignments {
            let Some(ref contract) = assign.contract else {
                continue;
            };

            // Validate output against output_schema
            if let Some(ref output) = assign.output {
                if let Some(ref payload) = output.output {
                    if let Some(ref out_schema) = contract.output_schema {
                        let mini = SchemaContract {
                            input_schema: None,
                            output_schema: Some(out_schema.clone()),
                        };
                        validate_contract(payload, &mini)
                            .map_err(|e| format!("node '{}' output: {}", assign.dag_node_id, e))?;
                    }
                }
            }

            // Note: input validation would require the input payload to be
            // stored on the assignment.  The current data model does not
            // retain the input after execution, so we only validate output
            // here.  Input validation can be added when the input is
            // persisted alongside the assignment.
        }
        Ok(())
    }
}

#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DagStatus {
    Pending,
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

// ---------------------------------------------------------------------------
// DistributedDagState (state machine)
// ---------------------------------------------------------------------------

#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributedDagState {
    pub plan: DagExecutionPlan,
    pub nodes: HashMap<NodeId, NodeInfo>,
    pub term: u64, // Raft term
    pub voted_for: Option<NodeId>,
    pub leader_id: Option<NodeId>,
}

#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
impl DistributedDagState {
    pub fn new(dag_id: DagId) -> Self {
        Self {
            plan: DagExecutionPlan {
                dag_id,
                assignments: Vec::new(),
                adjacency: HashMap::new(),
                created_at_ms: current_timestamp_ms(),
                status: DagStatus::Pending,
            },
            nodes: HashMap::new(),
            term: 0,
            voted_for: None,
            leader_id: None,
        }
    }

    /// Get all ready nodes (dependencies satisfied, not yet completed).
    pub fn ready_nodes(&self) -> Vec<&DagNodeAssignment> {
        let completed: HashSet<&str> = self
            .plan
            .assignments
            .iter()
            .filter(|a| a.completed)
            .map(|a| a.dag_node_id.as_str())
            .collect();

        self.plan
            .assignments
            .iter()
            .filter(|a| {
                if a.completed {
                    return false;
                }
                // All dependencies must be completed
                if let Some(deps) = self.plan.adjacency.get(&a.dag_node_id) {
                    deps.iter().all(|d| completed.contains(d.as_str()))
                } else {
                    true // no dependencies
                }
            })
            .collect()
    }

    /// Check if the entire DAG is complete.
    pub fn is_complete(&self) -> bool {
        self.plan.assignments.iter().all(|a| a.completed)
    }
}

// ---------------------------------------------------------------------------
// RaftLogEntry
// ---------------------------------------------------------------------------

#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RaftCommand {
    AssignNode {
        dag_node_id: String,
        node_id: NodeId,
    },
    CompleteNode {
        dag_node_id: String,
        output: NodeOutput,
    },
    FailNode {
        dag_node_id: String,
        error: String,
    },
    Heartbeat {
        node_id: NodeId,
    },
    SuspectNode {
        node_id: NodeId,
    },
    MarkOffline {
        node_id: NodeId,
    },
    ReassignNode {
        dag_node_id: String,
        from: NodeId,
        to: NodeId,
    },
    UpdateDagStatus(DagStatus),
}

#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaftLogEntry {
    pub index: u64,
    pub term: u64,
    pub command: RaftCommand,
}

/// A snapshot of the Raft state machine at a given log index.
/// Used for log compaction and install-snapshot RPCs during leader election.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaftSnapshot {
    /// The index of the last log entry included in this snapshot.
    pub last_included_index: u64,
    /// The term of `last_included_index`.
    pub last_included_term: u64,
    /// The serialised state machine at the snapshot point.
    pub state: HashMap<DagId, DistributedDagState>,
}

// ---------------------------------------------------------------------------
// FaultDetectionConfig
// ---------------------------------------------------------------------------

#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
#[derive(Debug, Clone)]
pub struct FaultDetectionConfig {
    /// Heartbeat interval in seconds.
    pub heartbeat_interval_s: u64,
    /// Lease duration in seconds. Nodes not heard from within this window are suspect.
    pub lease_duration_s: u64,
    /// Number of missed heartbeats before marking a node offline.
    pub max_missed_heartbeats: u32,
    /// Interval at which the coordinator checks for lease expiry.
    pub check_interval_s: u64,
}

impl Default for FaultDetectionConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval_s: 5,
            lease_duration_s: 15,
            max_missed_heartbeats: 3,
            check_interval_s: 10,
        }
    }
}

// ---------------------------------------------------------------------------
// DistributedDAGCoordinator
// ---------------------------------------------------------------------------

#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
pub struct DistributedDAGCoordinator {
    /// DAG state map: dag_id -> DistributedDagState
    dag_states: RwLock<HashMap<DagId, DistributedDagState>>,
    /// Reverse index: node_id -> DAGs the node participates in.
    /// Avoids O(dags × nodes) scanning in heartbeat / lease checks.
    node_to_dags: RwLock<HashMap<NodeId, Vec<DagId>>>,
    /// The remote executor used to dispatch tasks.
    executor: Arc<dyn RemoteExecutor>,
    /// Fault detection configuration.
    fault_config: FaultDetectionConfig,
    /// Raft log (for state machine replication).
    raft_log: RwLock<Vec<RaftLogEntry>>,
    /// Current Raft term (leader-tracked).
    current_term: RwLock<u64>,
    /// Leader lease expiry.
    leader_lease: RwLock<u64>,
    /// Our own node ID.
    self_node_id: NodeId,
    /// Shutdown signal for fault detection loop.
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Index of the last entry included in the most recent snapshot.
    /// All entries with index <= last_snapshot_index have been compacted.
    last_snapshot_index: RwLock<u64>,
    /// Raft log compaction threshold: when the log exceeds this many entries
    /// (beyond the last snapshot), a new snapshot is automatically triggered.
    snapshot_threshold: u64,
}

#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
impl DistributedDAGCoordinator {
    pub fn new(self_node_id: NodeId, executor: Arc<dyn RemoteExecutor>) -> Self {
        let (shutdown_tx, _) = tokio::sync::watch::channel(false);
        Self {
            dag_states: RwLock::new(HashMap::new()),
            node_to_dags: RwLock::new(HashMap::new()),
            executor,
            fault_config: FaultDetectionConfig::default(),
            raft_log: RwLock::new(Vec::new()),
            current_term: RwLock::new(0),
            leader_lease: RwLock::new(0),
            self_node_id,
            shutdown_tx,
            last_snapshot_index: RwLock::new(0),
            snapshot_threshold: 1000,
        }
    }

    /// Create a new DAG in the coordinator.
    pub async fn create_dag(&self, dag_id: DagId) -> Result<(), DagCoordinatorError> {
        let mut states = self.dag_states.write().await;
        if states.contains_key(&dag_id) {
            return Ok(()); // idempotent
        }
        let cloned = dag_id.clone();
        info!(dag = %dag_id, "DAG created");
        states.insert(dag_id, DistributedDagState::new(cloned));
        Ok(())
    }

    /// Register a node for DAG execution.
    pub async fn register_node(
        &self,
        node_id: NodeId,
        address: String,
        port: u16,
    ) -> Result<(), DagCoordinatorError> {
        let mut states = self.dag_states.write().await;
        let dag_ids: Vec<DagId> = states.keys().cloned().collect();
        for state in states.values_mut() {
            state.nodes.insert(
                node_id.clone(),
                NodeInfo::new(node_id.clone(), address.clone(), port),
            );
        }
        // Update the reverse index: node participates in all existing DAGs
        if !dag_ids.is_empty() {
            let mut idx = self.node_to_dags.write().await;
            idx.insert(node_id.clone(), dag_ids);
        }

        // Also register with the underlying executor
        let caps = crate::orchestration::distributed::remote_executor::NodeCapabilities::new(
            node_id.clone(),
            vec![],
        );
        let reg = NodeRegistration::new(node_id.clone(), address, port, caps);
        self.executor
            .register_node(reg)
            .await
            .map_err(|e| DagCoordinatorError::ExecutionError(e.to_string()))?;

        info!(node = %node_id, "Node registered in DAG coordinator");
        Ok(())
    }

    /// Assign a DAG node to a specific worker node.
    pub async fn assign_node(
        &self,
        dag_id: &str,
        dag_node_id: &str,
        node_id: &str,
    ) -> Result<(), DagCoordinatorError> {
        let mut states = self.dag_states.write().await;
        let state = states
            .get_mut(dag_id)
            .ok_or_else(|| DagCoordinatorError::DagNotFound(DagId(dag_id.to_string())))?;

        if !state.nodes.contains_key(node_id) {
            return Err(DagCoordinatorError::NodeNotFound(NodeId(
                node_id.to_string(),
            )));
        }

        let node_state = &state.nodes[node_id].state;
        if *node_state != NodeState::Online {
            return Err(DagCoordinatorError::NodeOffline(NodeId(
                node_id.to_string(),
            )));
        }

        if let Some(assign) = state
            .plan
            .assignments
            .iter_mut()
            .find(|a| a.dag_node_id == dag_node_id)
        {
            assign.assigned_node_id = Some(NodeId(node_id.to_string()));
            debug!(dag = %dag_id, dag_node = %dag_node_id, node = %node_id, "Node assigned");
        }

        // Append to Raft log
        self.append_raft_log(RaftCommand::AssignNode {
            dag_node_id: dag_node_id.to_string(),
            node_id: NodeId(node_id.to_string()),
        })
        .await;

        Ok(())
    }

    /// Execute the DAG by dispatching ready nodes to their assigned executors.
    pub async fn execute_dag(self: &Arc<Self>, dag_id: &DagId) -> Result<(), DagCoordinatorError> {
        let mut states = self.dag_states.write().await;
        let state = states
            .get_mut(dag_id)
            .ok_or_else(|| DagCoordinatorError::DagNotFound(dag_id.clone()))?;

        if state.plan.status != DagStatus::Pending {
            return Err(DagCoordinatorError::ExecutionError(
                "DAG already started or completed".into(),
            ));
        }

        state.plan.status = DagStatus::Running;
        let dag_id_str = dag_id.clone();
        drop(states);

        let coord = self.clone();
        tokio::spawn(async move {
            info!(dag = %dag_id_str, "DAG execution started");

            loop {
                // --- Phase 1: Identify ready nodes under a read lock ---
                let ready: Vec<(
                    String,                             // dag_node_id
                    String,                             // tool_name
                    NodeId,                             // assigned_node_id
                    HashMap<NodeId, serde_json::Value>, // dep_outputs
                )> = {
                    let states = coord.dag_states.read().await;
                    let Some(state) = states.get(&dag_id_str) else {
                        error!(dag = %dag_id_str, "DAG not found during execution");
                        return;
                    };

                    // Honour cancellation.
                    if state.plan.status == DagStatus::Cancelled {
                        info!(dag = %dag_id_str, "DAG execution cancelled");
                        return;
                    }

                    // Check if every node has been completed.
                    if state.is_complete() {
                        drop(states);
                        let mut states_w = coord.dag_states.write().await;
                        if let Some(s) = states_w.get_mut(&dag_id_str) {
                            s.plan.status = DagStatus::Completed;
                        }
                        info!(dag = %dag_id_str, "DAG execution completed");
                        return;
                    }

                    let ready_nodes = state.ready_nodes();
                    if ready_nodes.is_empty() {
                        // No nodes ready and DAG is not complete – deadlocked or
                        // nodes are still being assigned.
                        error!(
                            dag = %dag_id_str,
                            "DAG stalled – no ready nodes but DAG is not complete"
                        );
                        return;
                    }

                    ready_nodes
                        .iter()
                        .filter_map(|n| {
                            let node_id = n.assigned_node_id.clone()?;

                            // Collect outputs from dependency nodes to pass as
                            // dep_outputs.
                            let mut dep_outputs = HashMap::new();
                            if let Some(deps) = state.plan.adjacency.get(&n.dag_node_id) {
                                for dep_id in deps {
                                    if let Some(dep_assign) = state
                                        .plan
                                        .assignments
                                        .iter()
                                        .find(|a| a.dag_node_id == *dep_id)
                                    {
                                        if let Some(ref output) = dep_assign.output {
                                            if let Some(ref val) = output.output {
                                                dep_outputs
                                                    .insert(NodeId(dep_id.clone()), val.clone());
                                            }
                                        }
                                    }
                                }
                            }

                            Some((
                                n.dag_node_id.clone(),
                                n.tool_name.clone(),
                                node_id,
                                dep_outputs,
                            ))
                        })
                        .collect()
                };

                // If every ready node lacked an assignment skip back to the
                // top of the loop so the next iteration re-checks.
                if ready.is_empty() {
                    continue;
                }

                // --- Phase 2: Dispatch ready nodes via the remote executor ---
                let mut completed: Vec<(String, NodeOutput)> = Vec::new();
                let mut failed = false;
                let mut fail_reason = String::new();

                for (dag_node_id, tool_name, node_id, dep_outputs) in ready {
                    let packet = TaskPacket {
                        node_id,
                        dag_id: dag_id_str.clone(),
                        tool_name,
                        input: serde_json::Value::Null,
                        dep_outputs,
                        retry_count: 0,
                        max_retries: 3,
                    };

                    match coord.executor.execute_remote(packet).await {
                        Ok(output) => {
                            completed.push((dag_node_id, output));
                        }
                        Err(e) => {
                            error!(
                                dag = %dag_id_str,
                                dag_node = %dag_node_id,
                                error = %e,
                                "Node execution failed"
                            );
                            failed = true;
                            fail_reason = e.to_string();
                            break;
                        }
                    }
                }

                // --- Phase 3: Persist results (or mark as failed) ---
                if failed {
                    let mut states_w = coord.dag_states.write().await;
                    if let Some(s) = states_w.get_mut(&dag_id_str) {
                        s.plan.status = DagStatus::Failed(fail_reason);
                    }
                    error!(dag = %dag_id_str, "DAG execution failed");
                    return;
                }

                {
                    let mut states_w = coord.dag_states.write().await;
                    let Some(state) = states_w.get_mut(&dag_id_str) else {
                        error!(dag = %dag_id_str, "DAG not found for state update");
                        return;
                    };
                    for (dag_node_id, output) in &completed {
                        if let Some(assign) = state
                            .plan
                            .assignments
                            .iter_mut()
                            .find(|a| a.dag_node_id == *dag_node_id)
                        {
                            assign.output = Some(output.clone());
                            assign.completed = true;
                        }
                    }
                }

                // Loop back to Phase 1 so newly-ready nodes are discovered.
            }
        });

        Ok(())
    }

    /// Handle a heartbeat from a node.
    pub async fn handle_heartbeat(&self, node_id: &NodeId) -> Result<(), DagCoordinatorError> {
        let now = current_timestamp_ms();

        // Use the reverse index to only touch DAGs the node participates in
        let dag_ids: Vec<DagId> = {
            let idx = self.node_to_dags.read().await;
            idx.get(node_id).cloned().unwrap_or_default()
        };

        if dag_ids.is_empty() {
            return Err(DagCoordinatorError::NodeNotFound(node_id.clone()));
        }

        let mut states = self.dag_states.write().await;
        for dag_id in &dag_ids {
            if let Some(state) = states.get_mut(dag_id) {
                if let Some(info) = state.nodes.get_mut(node_id) {
                    info.last_heartbeat_ms = now;
                    info.lease_expiry_ms = now + (self.fault_config.lease_duration_s * 1000);
                    if info.state == NodeState::Suspect {
                        info.state = NodeState::Online;
                        info!(node = %node_id, "Node restored to Online after heartbeat");
                    }
                }
            }
        }

        self.append_raft_log(RaftCommand::Heartbeat {
            node_id: node_id.clone(),
        })
        .await;
        Ok(())
    }

    /// Check for lease expiry and mark suspect/offline nodes.
    pub async fn check_leases(&self) -> Vec<NodeId> {
        let now = current_timestamp_ms();
        let mut failed_nodes = Vec::new();

        // Collect all unique nodes from the reverse index, then iterate
        // only the DAG states each node actually participates in.
        let node_dag_list: Vec<(NodeId, Vec<DagId>)> = {
            let idx = self.node_to_dags.read().await;
            idx.iter()
                .map(|(n, dags)| (n.clone(), dags.clone()))
                .collect()
        };

        if node_dag_list.is_empty() {
            return failed_nodes;
        }

        let mut states = self.dag_states.write().await;

        for (node_id, dag_ids) in &node_dag_list {
            // Peek at the node's info from the first DAG it belongs to.
            let mut first_info = None;
            for dag_id in dag_ids {
                if let Some(state) = states.get(dag_id) {
                    if let Some(info) = state.nodes.get(node_id) {
                        first_info = Some(info.clone());
                        break;
                    }
                }
            }

            let Some(info) = first_info else {
                continue;
            };

            // Determine what state transition to apply (same logic as before)
            let is_suspect = info.state == NodeState::Online && now > info.lease_expiry_ms;
            let is_offline = info.state == NodeState::Suspect
                && now.saturating_sub(info.last_heartbeat_ms)
                    > (self.fault_config.lease_duration_s * 1000)
                        * self.fault_config.max_missed_heartbeats as u64;

            if is_suspect || is_offline {
                let new_state = if is_offline {
                    NodeState::Offline
                } else {
                    NodeState::Suspect
                };

                // Apply the transition to all DAGs the node participates in
                for dag_id in dag_ids {
                    if let Some(state) = states.get_mut(dag_id) {
                        if let Some(ninfo) = state.nodes.get_mut(node_id) {
                            ninfo.state = new_state.clone();
                        }
                    }
                }

                if is_suspect {
                    warn!(node = %node_id, "Node marked as suspect (lease expired)");
                    self.append_raft_log(RaftCommand::SuspectNode {
                        node_id: node_id.clone(),
                    })
                    .await;
                } else {
                    error!(node = %node_id, "Node marked offline (missed heartbeats)");
                    self.append_raft_log(RaftCommand::MarkOffline {
                        node_id: node_id.clone(),
                    })
                    .await;
                    failed_nodes.push(node_id.clone());
                }
            }
        }

        failed_nodes
    }

    /// Reassign a DAG node from a failed node to a healthy one.
    pub async fn reassign_node(
        &self,
        dag_id: &str,
        dag_node_id: &str,
        from_node: &str,
        to_node: &str,
    ) -> Result<(), DagCoordinatorError> {
        let mut states = self.dag_states.write().await;
        let state = states
            .get_mut(dag_id)
            .ok_or_else(|| DagCoordinatorError::DagNotFound(DagId(dag_id.to_string())))?;

        let to_node_str = to_node.to_string();
        if !state.nodes.contains_key(&to_node_str) {
            return Err(DagCoordinatorError::NodeNotFound(NodeId(
                to_node.to_string(),
            )));
        }

        if state.nodes[&to_node_str].state != NodeState::Online {
            return Err(DagCoordinatorError::NodeOffline(NodeId(
                to_node.to_string(),
            )));
        }

        if let Some(assign) = state
            .plan
            .assignments
            .iter_mut()
            .find(|a| a.dag_node_id == dag_node_id)
        {
            assign.assigned_node_id = Some(NodeId(to_node.to_string()));
            assign.completed = false;
            assign.output = None;
            assign.error = None;
            info!(
                dag = %dag_id, dag_node = %dag_node_id,
                from = %from_node, to = %to_node,
                "Node reassigned"
            );
        }

        self.append_raft_log(RaftCommand::ReassignNode {
            dag_node_id: dag_node_id.to_string(),
            from: NodeId(from_node.to_string()),
            to: NodeId(to_node.to_string()),
        })
        .await;

        Ok(())
    }

    /// Query the status of a DAG.
    pub async fn get_dag_status(&self, dag_id: &str) -> Result<DagStatus, DagCoordinatorError> {
        let states = self.dag_states.read().await;
        let state = states
            .get(dag_id)
            .ok_or_else(|| DagCoordinatorError::DagNotFound(DagId(dag_id.to_string())))?;
        Ok(state.plan.status.clone())
    }

    /// List all known DAGs.
    pub async fn list_dags(&self) -> Vec<DagId> {
        self.dag_states.read().await.keys().cloned().collect()
    }

    /// Get node info for all nodes in a DAG.
    pub async fn get_nodes(&self, dag_id: &str) -> Result<Vec<NodeInfo>, DagCoordinatorError> {
        let states = self.dag_states.read().await;
        let state = states
            .get(dag_id)
            .ok_or_else(|| DagCoordinatorError::DagNotFound(DagId(dag_id.to_string())))?;
        Ok(state.nodes.values().cloned().collect())
    }

    // ── Raft helpers ──────────────────────────────────────────────────────────

    async fn append_raft_log(&self, command: RaftCommand) {
        let mut log = self.raft_log.write().await;
        let term = *self.current_term.read().await;
        let last_snapshot = *self.last_snapshot_index.read().await;
        let index = log.len() as u64 + 1 + last_snapshot;
        log.push(RaftLogEntry {
            index,
            term,
            command,
        });

        // Trigger compaction if log exceeds threshold.
        let log_len = log.len() as u64;
        if log_len >= self.snapshot_threshold {
            // Snapshot index is the last index we snapshotted; new entries are beyond it.
            let new_snapshot_index = index;
            // Compact: keep only the most recent entries (configurable tail size).
            let tail_keep = 64u64.min(log_len / 4);
            let truncate_at = (log_len - tail_keep) as usize;
            if truncate_at > 0 {
                let remaining: Vec<RaftLogEntry> = log.drain(truncate_at..).collect();
                *log = remaining;
            }
            *self.last_snapshot_index.write().await = new_snapshot_index;
            debug!(
                log_entries_before = log_len,
                snapshot_index = new_snapshot_index,
                remaining = log.len(),
                "Raft log snapshot + compacted"
            );
        }
    }

    /// Serialise the current state machine as a Raft snapshot.
    /// Used by log compaction and for install-snapshot RPCs.
    pub async fn take_snapshot(&self) -> RaftSnapshot {
        let states = self.dag_states.read().await;
        let last_index = *self.last_snapshot_index.read().await;
        let term = *self.current_term.read().await;
        RaftSnapshot {
            last_included_index: last_index,
            last_included_term: term,
            state: states.clone(),
        }
    }

    /// Install a snapshot received from a leader.
    /// Replaces the current state and truncates the log.
    pub async fn install_snapshot(&self, snapshot: RaftSnapshot) {
        let mut states = self.dag_states.write().await;
        *states = snapshot.state;
        let mut last_idx = self.last_snapshot_index.write().await;
        *last_idx = snapshot.last_included_index;
        let mut log = self.raft_log.write().await;
        log.clear();
        let mut term = self.current_term.write().await;
        *term = snapshot.last_included_term;
        info!(
            snapshot_index = snapshot.last_included_index,
            snapshot_term = snapshot.last_included_term,
            "installed Raft snapshot"
        );
    }

    /// Start the background fault detection loop.
    /// Spawn this as a tokio task during initialization.
    pub fn start_fault_detection(self: &Arc<Self>) {
        let coord = self.clone();
        let config = self.fault_config.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let handle = tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(config.check_interval_s));
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = shutdown_rx.changed() => {
                        info!("fault detection loop shutting down");
                        return;
                    }
                }
                let failed = coord.check_leases().await;
                if !failed.is_empty() {
                    warn!(nodes = ?failed, "Fault detection: nodes marked offline");

                    let dags = coord.list_dags().await;
                    for dag_id in dags {
                        let states = coord.dag_states.read().await;
                        let Some(state) = states.get(&dag_id) else {
                            continue;
                        };

                        let reassignments: Vec<(String, NodeId)> = state
                            .plan
                            .assignments
                            .iter()
                            .filter(|a| {
                                !a.completed
                                    && a.assigned_node_id
                                        .as_ref()
                                        .is_some_and(|n| failed.contains(n))
                            })
                            .map(|a| (a.dag_node_id.clone(), a.assigned_node_id.clone().unwrap()))
                            .collect();
                        drop(states);

                        for (dag_node_id, from_node) in reassignments {
                            let nodes =
                                coord.get_nodes(dag_id.0.as_str()).await.unwrap_or_default();
                            let healthy: Vec<&NodeInfo> = nodes
                                .iter()
                                .filter(|n| n.state == NodeState::Online && n.node_id != from_node)
                                .collect();

                            if let Some(target) = healthy.first() {
                                let _ = coord
                                    .reassign_node(
                                        dag_id.0.as_str(),
                                        &dag_node_id,
                                        &from_node.to_string(),
                                        &target.node_id.to_string(),
                                    )
                                    .await;
                            }
                        }
                    }
                }
            }
        });
        // Handle is intentionally detached — the fault detection loop
        // terminates via the shutdown_tx signal.
        drop(handle);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::distributed::remote_executor::{
        InProcessRemoteExecutor, NodeRegistry,
    };
    use crate::orchestration::tool::ToolRegistry;

    fn make_coordinator() -> Arc<DistributedDAGCoordinator> {
        let registry = Arc::new(NodeRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new_empty());
        let executor = Arc::new(InProcessRemoteExecutor::new(registry, tool_registry));
        Arc::new(DistributedDAGCoordinator::new("coord-1".into(), executor))
    }

    #[tokio::test]
    async fn test_create_dag() {
        let coord = make_coordinator();
        coord.create_dag("dag-test-1".into()).await.unwrap();
        let dags = coord.list_dags().await;
        assert!(dags.contains(&"dag-test-1".to_string().into()));
    }

    #[tokio::test]
    async fn test_register_and_heartbeat() {
        let coord = make_coordinator();
        coord.create_dag("dag-hb".into()).await.unwrap();
        coord
            .register_node("node-a".into(), "10.0.0.1".into(), 9000)
            .await
            .unwrap();

        let nodes = coord.get_nodes("dag-hb").await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_id, "node-a".into());
        assert_eq!(nodes[0].state, NodeState::Online);

        // Heartbeat should refresh lease
        coord.handle_heartbeat(&"node-a".into()).await.unwrap();
        let nodes = coord.get_nodes("dag-hb").await.unwrap();
        assert_eq!(nodes[0].state, NodeState::Online);
    }

    #[tokio::test]
    async fn test_lease_expiry() {
        let coord = make_coordinator();
        coord.create_dag("dag-lease".into()).await.unwrap();
        coord
            .register_node("node-b".into(), "10.0.0.2".into(), 9001)
            .await
            .unwrap();

        // Manually set the node's lease to the past to force expiry
        tokio::time::sleep(Duration::from_millis(10)).await;
        {
            let mut states = coord.dag_states.write().await;
            if let Some(state) = states.get_mut("dag-lease") {
                if let Some(info) = state.nodes.get_mut("node-b") {
                    info.lease_expiry_ms = 1; // well in the past
                    info.last_heartbeat_ms = 1; // also past
                }
            }
        }

        // First check transitions node from Online → Suspect (lease expired).
        // Second check transitions from Suspect → Offline (heartbeat too old).
        let _ = coord.check_leases().await;
        // Small sleep to ensure enough wall-clock time elapses for the
        // Suspect → Offline threshold check (which uses current_timestamp).
        tokio::time::sleep(Duration::from_millis(50)).await;
        let failed = coord.check_leases().await;
        assert!(
            failed.contains(&"node-b".to_string().into()),
            "node-b should be marked as failed after lease expiry"
        );
    }

    #[tokio::test]
    async fn test_reassign() {
        let coord = make_coordinator();
        coord.create_dag("dag-reassign".into()).await.unwrap();
        coord
            .register_node("node-1".into(), "10.0.0.1".into(), 9000)
            .await
            .unwrap();
        coord
            .register_node("node-2".into(), "10.0.0.2".into(), 9001)
            .await
            .unwrap();

        let _ = coord.assign_node("dag-reassign", "task-1", "node-1").await;

        coord
            .reassign_node("dag-reassign", "task-1", "node-1", "node-2")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_dag_status() {
        let coord = make_coordinator();
        coord.create_dag("dag-status".into()).await.unwrap();
        let status = coord.get_dag_status("dag-status").await.unwrap();
        assert_eq!(status, DagStatus::Pending);
    }

    // ── Schema contract validation tests ────────────────────────────────

    #[test]
    fn test_schema_contract_type_pass() {
        let contract = SchemaContract {
            input_schema: None,
            output_schema: Some(serde_json::json!({"type": "object"})),
        };
        let data = serde_json::json!({"key": "value"});
        assert!(validate_contract(&data, &contract).is_ok());
    }

    #[test]
    fn test_schema_contract_type_fail() {
        let contract = SchemaContract {
            input_schema: None,
            output_schema: Some(serde_json::json!({"type": "string"})),
        };
        let data = serde_json::json!(42);
        let err = validate_contract(&data, &contract).unwrap_err();
        assert!(err.contains("expected type 'string'"), "got: {err}");
    }

    #[test]
    fn test_schema_contract_required_properties() {
        let contract = SchemaContract {
            input_schema: Some(serde_json::json!({
                "type": "object",
                "required": ["name", "version"]
            })),
            output_schema: None,
        };
        // Missing "version"
        let data = serde_json::json!({"name": "foo"});
        let err = validate_contract(&data, &contract).unwrap_err();
        assert!(
            err.contains("missing required property 'version'"),
            "got: {err}"
        );

        // Now with all required fields
        let ok_data = serde_json::json!({"name": "foo", "version": 1});
        assert!(validate_contract(&ok_data, &contract).is_ok());
    }

    #[test]
    fn test_schema_contract_enum() {
        let contract = SchemaContract {
            input_schema: None,
            output_schema: Some(serde_json::json!({
                "enum": ["pending", "running", "completed"]
            })),
        };
        assert!(validate_contract(&serde_json::json!("running"), &contract).is_ok());
        let err = validate_contract(&serde_json::json!("unknown"), &contract).unwrap_err();
        assert!(err.contains("not in allowed enum"), "got: {err}");
    }

    #[test]
    fn test_schema_contract_numeric_bounds() {
        let contract = SchemaContract {
            input_schema: Some(serde_json::json!({
                "type": "number",
                "minimum": 0.0,
                "maximum": 100.0
            })),
            output_schema: None,
        };
        assert!(validate_contract(&serde_json::json!(50.0), &contract).is_ok());
        let err = validate_contract(&serde_json::json!(-1.0), &contract).unwrap_err();
        assert!(err.contains("< minimum"), "got: {err}");
        let err = validate_contract(&serde_json::json!(101.0), &contract).unwrap_err();
        assert!(err.contains("> maximum"), "got: {err}");
    }

    #[test]
    fn test_schema_contract_string_length() {
        let contract = SchemaContract {
            input_schema: None,
            output_schema: Some(serde_json::json!({
                "type": "string",
                "minLength": 1,
                "maxLength": 10
            })),
        };
        assert!(validate_contract(&serde_json::json!("hello"), &contract).is_ok());
        let err = validate_contract(&serde_json::json!(""), &contract).unwrap_err();
        assert!(err.contains("< minLength"), "got: {err}");
        let err = validate_contract(&serde_json::json!("toolongstring"), &contract).unwrap_err();
        assert!(err.contains("> maxLength"), "got: {err}");
    }

    #[test]
    fn test_schema_contract_array_items() {
        let contract = SchemaContract {
            input_schema: None,
            output_schema: Some(serde_json::json!({
                "type": "array",
                "minItems": 1,
                "maxItems": 3
            })),
        };
        assert!(validate_contract(&serde_json::json!([1, 2]), &contract).is_ok());
        let err = validate_contract(&serde_json::json!([]), &contract).unwrap_err();
        assert!(err.contains("< minItems"), "got: {err}");
    }

    #[test]
    fn test_validate_all_contracts_ok() {
        let plan = DagExecutionPlan {
            dag_id: "test-ok".into(),
            assignments: vec![
                DagNodeAssignment {
                    dag_node_id: "node-a".into(),
                    tool_name: "tool_a".into(),
                    assigned_node_id: None,
                    output: Some(NodeOutput::success(
                        "n1".into(),
                        "test-ok".into(),
                        "tool_a".into(),
                        serde_json::json!({"result": "ok"}),
                        10,
                    )),
                    error: None,
                    completed: true,
                    contract: Some(SchemaContract {
                        input_schema: None,
                        output_schema: Some(serde_json::json!({
                            "type": "object",
                            "required": ["result"]
                        })),
                    }),
                },
                DagNodeAssignment {
                    dag_node_id: "node-b".into(),
                    tool_name: "tool_b".into(),
                    assigned_node_id: None,
                    output: None,
                    error: None,
                    completed: false,
                    contract: None, // no contract — skipped
                },
            ],
            adjacency: HashMap::new(),
            created_at_ms: 0,
            status: DagStatus::Running,
        };
        assert!(plan.validate_all_contracts().is_ok());
    }

    #[test]
    fn test_validate_all_contracts_fail() {
        let plan = DagExecutionPlan {
            dag_id: "test-fail".into(),
            assignments: vec![DagNodeAssignment {
                dag_node_id: "node-bad".into(),
                tool_name: "bad_tool".into(),
                assigned_node_id: None,
                output: Some(NodeOutput::success(
                    "n1".into(),
                    "test-fail".into(),
                    "bad_tool".into(),
                    serde_json::json!("this is a string, not an object"),
                    5,
                )),
                error: None,
                completed: true,
                contract: Some(SchemaContract {
                    input_schema: None,
                    output_schema: Some(serde_json::json!({"type": "object"})),
                }),
            }],
            adjacency: HashMap::new(),
            created_at_ms: 0,
            status: DagStatus::Running,
        };
        let err = plan.validate_all_contracts().unwrap_err();
        assert!(
            err.contains("node-bad") && err.contains("expected type 'object'"),
            "got: {err}"
        );
    }

    #[test]
    fn test_schema_contract_input_output_independent() {
        // input_schema applies to input, output_schema applies to output
        let contract = SchemaContract {
            input_schema: Some(serde_json::json!({"type": "number"})),
            output_schema: Some(serde_json::json!({"type": "string"})),
        };
        // Data passed here is checked against *both* schemas, so this
        // should fail because a string is not a number.
        let err = validate_contract(&serde_json::json!("hello"), &contract).unwrap_err();
        assert!(err.contains("(input)"), "got: {err}");

        // A number passes input_schema but fails output_schema
        let err = validate_contract(&serde_json::json!(42), &contract).unwrap_err();
        assert!(err.contains("(output)"), "got: {err}");
    }

    #[test]
    fn test_schema_contract_none_always_ok() {
        let contract = SchemaContract {
            input_schema: None,
            output_schema: None,
        };
        assert!(validate_contract(&serde_json::json!("anything"), &contract).is_ok());
        assert!(validate_contract(&serde_json::json!(null), &contract).is_ok());
    }
}
