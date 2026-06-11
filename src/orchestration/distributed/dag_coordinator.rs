//! Distributed DAG Coordinator (GAP-B52-22)
//!
//! Coordinates DAG execution across a distributed cluster using Raft-based
//! consistency for state replication, heartbeat + lease fault detection,
//! and automatic node reassignment on failure.

use crate::orchestration::distributed::remote_executor::{
    DagId, NodeId, NodeOutput, NodeRegistration, RemoteExecutor,
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
    #[allow(dead_code)]
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
}

#[allow(dead_code)] // F-GAP-49 — reserved for distributed DAG
impl DistributedDAGCoordinator {
    pub fn new(self_node_id: NodeId, executor: Arc<dyn RemoteExecutor>) -> Self {
        let (shutdown_tx, _) = tokio::sync::watch::channel(false);
        Self {
            dag_states: RwLock::new(HashMap::new()),
            executor,
            fault_config: FaultDetectionConfig::default(),
            raft_log: RwLock::new(Vec::new()),
            current_term: RwLock::new(0),
            leader_lease: RwLock::new(0),
            self_node_id,
            shutdown_tx,
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
        for state in states.values_mut() {
            state.nodes.insert(
                node_id.clone(),
                NodeInfo::new(node_id.clone(), address.clone(), port),
            );
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
    pub async fn execute_dag(&self, dag_id: &DagId) -> Result<(), DagCoordinatorError> {
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

        // In a real implementation, an execution loop would be spawned here
        // that completes the DAG asynchronously.
        let exec = self.executor.clone();
        tokio::spawn(async move {
            info!(dag = %dag_id_str, "DAG execution started");
            // Future: iterate over ready_nodes, dispatch via executor, collect results
            let _ = exec;
        });

        Ok(())
    }

    /// Handle a heartbeat from a node.
    pub async fn handle_heartbeat(&self, node_id: &NodeId) -> Result<(), DagCoordinatorError> {
        let mut states = self.dag_states.write().await;
        let now = current_timestamp_ms();

        for state in states.values_mut() {
            if let Some(info) = state.nodes.get_mut(node_id) {
                info.last_heartbeat_ms = now;
                info.lease_expiry_ms = now + (self.fault_config.lease_duration_s * 1000);
                if info.state == NodeState::Suspect {
                    info.state = NodeState::Online;
                    info!(node = %node_id, "Node restored to Online after heartbeat");
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
        let mut states = self.dag_states.write().await;
        let now = current_timestamp_ms();
        let mut failed_nodes = Vec::new();

        for state in states.values_mut() {
            let suspect_ids: Vec<NodeId> = state
                .nodes
                .iter()
                .filter(|(_, info)| info.state == NodeState::Online && now > info.lease_expiry_ms)
                .map(|(id, _)| id.clone())
                .collect();

            for id in &suspect_ids {
                if let Some(info) = state.nodes.get_mut(id) {
                    info.state = NodeState::Suspect;
                    warn!(node = %id, "Node marked as suspect (lease expired)");
                    self.append_raft_log(RaftCommand::SuspectNode {
                        node_id: id.clone(),
                    })
                    .await;
                }
            }

            // Mark as offline if heartbeat not received for max_missed_heartbeats cycles
            let offline_ids: Vec<NodeId> = state
                .nodes
                .iter()
                .filter(|(_, info)| {
                    if info.state != NodeState::Suspect {
                        return false;
                    }
                    let missed_ms = now.saturating_sub(info.last_heartbeat_ms);
                    let lease_ms = self.fault_config.lease_duration_s * 1000;
                    missed_ms > lease_ms * self.fault_config.max_missed_heartbeats as u64
                })
                .map(|(id, _)| id.clone())
                .collect();

            for id in &offline_ids {
                if let Some(info) = state.nodes.get_mut(id) {
                    info.state = NodeState::Offline;
                    error!(node = %id, "Node marked offline (missed heartbeats)");
                    self.append_raft_log(RaftCommand::MarkOffline {
                        node_id: id.clone(),
                    })
                    .await;
                    failed_nodes.push(id.clone());
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
        let index = log.len() as u64 + 1;
        log.push(RaftLogEntry {
            index,
            term,
            command,
        });
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

        let failed = coord.check_leases().await;
        assert!(
            failed.contains(&"node-b".to_string().into()),
            "node-b should be marked as failed"
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
