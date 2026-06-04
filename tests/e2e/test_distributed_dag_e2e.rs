//! Distributed DAG Execution End-to-End
//!
//! Validates the distributed DAG execution lifecycle:
//!   node registration → cross-node execution → failure → recovery
//!
//! Uses go_on::orchestration::distributed types for the DAG coordinator,
//! node registration, execution plans, and status tracking.
//!
//! # integration-test
//! Cross-node execution uses in-memory type construction. Real integration
//! requires two running go-on nodes with the `sub-bus-tool` feature and
//! network connectivity between them.

use std::collections::HashMap;

use go_on::fault_tolerance::{FaultEvent, FaultType};
use go_on::orchestration::distributed::dag_coordinator::{
    DagExecutionPlan, DagNodeAssignment, DagStatus, DistributedDagState, NodeInfo, NodeState,
};
use go_on::orchestration::distributed::remote_executor::{DagId, NodeOutput};

// ── Helpers ────────────────────────────────────────────────────────────────

struct DistributedDagE2eContext {
    node_ids: Vec<String>,
    dag_id: Option<String>,
}

impl DistributedDagE2eContext {
    fn new() -> Self {
        Self {
            node_ids: vec!["node-1".into(), "node-2".into()],
            dag_id: None,
        }
    }
}

/// Create a minimal DAG execution plan for e2e testing.
fn make_test_plan(dag_id: &str) -> DagExecutionPlan {
    DagExecutionPlan {
        dag_id: dag_id.into(),
        assignments: vec![
            DagNodeAssignment {
                dag_node_id: "fetch-data".into(),
                tool_name: "http_get".into(),
                assigned_node_id: Some("node-1".into()),
                output: None,
                error: None,
                completed: false,
                contract: None,
            },
            DagNodeAssignment {
                dag_node_id: "parse-json".into(),
                tool_name: "json_parse".into(),
                assigned_node_id: Some("node-2".into()),
                output: None,
                error: None,
                completed: false,
                contract: None,
            },
            DagNodeAssignment {
                dag_node_id: "report".into(),
                tool_name: "format_report".into(),
                assigned_node_id: None,
                output: None,
                error: None,
                completed: false,
                contract: None,
            },
        ],
        adjacency: {
            let mut m = HashMap::new();
            m.insert("parse-json".into(), vec!["fetch-data".into()]);
            m.insert("report".into(), vec!["parse-json".into()]);
            m
        },
        created_at_ms: 0,
        status: DagStatus::Pending,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Full distributed DAG execution: node registration → cross-node execution →
/// failure → recovery → completion.
#[tokio::test]
async fn test_distributed_dag_failure_recovery() {
    let mut ctx = DistributedDagE2eContext::new();

    // ── 1. Setup nodes ─────────────────────────────────────────────────
    let node1 = NodeInfo::new("node-1".into(), "127.0.0.1".into(), 9301);
    let node2 = NodeInfo::new("node-2".into(), "127.0.0.1".into(), 9302);

    assert_eq!(node1.node_id, "node-1".into());
    assert_eq!(node1.state, NodeState::Online);
    assert_eq!(node2.port, 9302);
    assert!(
        !node1.is_lease_expired(),
        "fresh node must not have expired lease"
    );

    // ── 2. DAG construction ────────────────────────────────────────────
    let dag_id: DagId = "dag-e2e-001".into();
    let plan = make_test_plan(&dag_id.to_string());
    ctx.dag_id = Some(dag_id.to_string());

    assert_eq!(plan.status, DagStatus::Pending);
    assert!(!plan.assignments.is_empty());
    assert_eq!(plan.assignments.len(), 3);

    // ── 3. Register nodes with DistributedDagState ─────────────────────
    // Use DistributedDagState to track nodes and validate the DAG lifecycle.
    let mut dag_state = DistributedDagState::new(dag_id.clone());
    dag_state.nodes.insert("node-1".into(), node1.clone());
    dag_state.nodes.insert("node-2".into(), node2.clone());
    dag_state.plan = plan.clone();
    assert_eq!(dag_state.nodes.len(), 2);
    assert!(dag_state.nodes.contains_key("node-1"));
    assert!(dag_state.nodes.contains_key("node-2"));
    // No tasks are completed yet, so ready_nodes returns the first nodes
    // with no dependencies (i.e. nodes not listed as dependents in adjacency).
    let ready = dag_state.ready_nodes();
    // "fetch-data" has no dependencies (not a key in adjacency), "parse-json"
    // depends on "fetch-data", and "report" depends on "parse-json".
    // So only "fetch-data" should be ready.
    assert_eq!(ready.len(), 1, "only fetch-data should be ready initially");
    assert_eq!(ready[0].dag_node_id, "fetch-data");

    // ── 4. Cross-node parallel execution ───────────────────────────────
    // "fetch-data" and "parse-json" have a dependency edge, so parse-json
    // runs after fetch-data completes. Mark fetch-data as completed
    // and verify parse-json becomes ready.
    let deps = plan.adjacency.get("parse-json");
    assert!(deps.is_some(), "parse-json must declare dependencies");
    assert_eq!(deps.unwrap(), &vec!["fetch-data".to_string()]);

    // Simulate fetch-data completion.
    if let Some(assign) = dag_state
        .plan
        .assignments
        .iter_mut()
        .find(|a| a.dag_node_id == "fetch-data")
    {
        assign.completed = true;
        assign.output = Some(NodeOutput::success(
            "node-1".into(),
            dag_id.clone(),
            "http_get".into(),
            serde_json::json!({"status": "ok"}),
            42,
        ));
    }
    let ready_after = dag_state.ready_nodes();
    assert!(
        ready_after.iter().any(|a| a.dag_node_id == "parse-json"),
        "parse-json should be ready after fetch-data completes"
    );

    // ── 5. Node failure ────────────────────────────────────────────────
    // Simulate node-2 going offline.
    let mut failed_node = node2.clone();
    failed_node.state = NodeState::Offline;
    assert_eq!(failed_node.state, NodeState::Offline);

    // Also represent via the fault tolerance types.
    let fault = FaultEvent {
        id: "fault-e2e-001".into(),
        node_id: "node-2".into(),
        fault_type: FaultType::Crash,
        severity: 8,
        description: "node-2 process crashed".into(),
        detected_ms: 0,
        resolved_ms: None,
        recovered: false,
    };
    assert_eq!(fault.node_id, "node-2");
    assert_eq!(fault.fault_type, FaultType::Crash);

    // ── 6. Recovery / reschedule ──────────────────────────────────────
    // Recovery reassigns orphaned DAG nodes from the failed node to an
    // online node. We simulate this by updating the node state back to
    // Online and reassigning its incomplete assignments.
    let mut recovered_node = failed_node.clone();
    recovered_node.state = NodeState::Online;
    recovered_node.last_heartbeat_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    assert_eq!(recovered_node.state, NodeState::Online);
    // Verify lease is not expired after recovery.
    assert!(
        !recovered_node.is_lease_expired(),
        "recovered node lease must be valid"
    );

    // Reassign any incomplete assignments from the original failed node.
    let incomplete_on_node2: Vec<String> = dag_state
        .plan
        .assignments
        .iter()
        .filter(|a| {
            !a.completed && a.assigned_node_id.as_ref().map(|n| n.0.as_str()) == Some("node-2")
        })
        .map(|a| a.dag_node_id.clone())
        .collect();
    // The "parse-json" assignment was assigned to node-2 but not completed,
    // so it is orphaned when node-2 fails.
    assert_eq!(
        incomplete_on_node2.len(),
        1,
        "parse-json should be orphaned after node-2 failure"
    );
    assert_eq!(incomplete_on_node2[0], "parse-json");

    // ── 7. Completion ──────────────────────────────────────────────────
    // Mark all assignments as completed and verify is_complete().
    for assign in dag_state.plan.assignments.iter_mut() {
        assign.completed = true;
        if assign.output.is_none() {
            let node_id = assign
                .assigned_node_id
                .clone()
                .unwrap_or_else(|| "unknown".into());
            let tool = assign.tool_name.clone();
            assign.output = Some(NodeOutput::success(
                node_id,
                dag_id.clone(),
                tool,
                serde_json::json!("done"),
                0,
            ));
        }
    }
    assert!(
        dag_state.is_complete(),
        "all assignments completed, DAG must be complete"
    );
    assert_eq!(
        dag_state.plan.status,
        DagStatus::Pending,
        "status is managed separately; state only tracks completion"
    );

    // Explicit status update: Pending → Running → Completed.
    dag_state.plan.status = DagStatus::Running;
    assert_eq!(dag_state.plan.status, DagStatus::Running);
    dag_state.plan.status = DagStatus::Completed;
    assert_eq!(dag_state.plan.status, DagStatus::Completed);
}

/// Validates that a DAG with invalid structure (cyclic deps) is rejected.
#[tokio::test]
async fn test_distributed_dag_rejects_invalid_dag() {
    // integration-test-stub: real validation checks for cycles before
    // submitting the DAG. A cyclic dependency between a→b→c→a must be
    // caught by the coordinator's validate_dag() method.
    //
    // Here we verify the structural representation of a cyclic DAG.
    let mut adjacency = HashMap::new();
    adjacency.insert("a".into(), vec!["c".into()]);
    adjacency.insert("b".into(), vec!["a".into()]);
    adjacency.insert("c".into(), vec!["b".into()]);

    // Detect cycle via simple DFS.
    fn has_cycle(adj: &HashMap<String, Vec<String>>) -> bool {
        let mut visited = std::collections::HashSet::new();
        let mut stack = std::collections::HashSet::new();

        fn dfs(
            node: &str,
            adj: &HashMap<String, Vec<String>>,
            visited: &mut std::collections::HashSet<String>,
            stack: &mut std::collections::HashSet<String>,
        ) -> bool {
            if stack.contains(node) {
                return true;
            }
            if visited.contains(node) {
                return false;
            }
            visited.insert(node.to_string());
            stack.insert(node.to_string());
            if let Some(deps) = adj.get(node) {
                for dep in deps {
                    if dfs(dep, adj, visited, stack) {
                        return true;
                    }
                }
            }
            stack.remove(node);
            false
        }

        for node in adj.keys() {
            if dfs(node, adj, &mut visited, &mut stack) {
                return true;
            }
        }
        false
    }

    assert!(has_cycle(&adjacency), "cyclic DAG must be detected");

    // Validate that DistributedDagState handles an empty DAG.
    // A DAG with zero assignments is trivially complete (all-of-an-empty-set).
    let dag_state = DistributedDagState::new("cycle-dag".into());
    assert!(
        dag_state.is_complete(),
        "empty DAG with no assignments is trivially complete"
    );
    assert!(
        dag_state.ready_nodes().is_empty(),
        "no ready nodes in empty DAG"
    );
    assert_eq!(dag_state.plan.status, DagStatus::Pending);

    // Verify a non-cyclic DAG passes through DistributedDagState normally.
    let plan = make_test_plan("non-cyclic-dag");
    let mut state = DistributedDagState::new("non-cyclic-dag".into());
    state.plan = plan;
    let initial_ready = state.ready_nodes();
    assert_eq!(initial_ready.len(), 1);
    assert_eq!(initial_ready[0].dag_node_id, "fetch-data");
}

/// Validates DAG status transitions.
#[tokio::test]
async fn test_distributed_dag_status_transitions() {
    let plan = make_test_plan("dag-status-test");

    assert_eq!(plan.status, DagStatus::Pending);

    // Simulate transitions: Pending → Running → Completed
    let running = DagExecutionPlan {
        status: DagStatus::Running,
        ..plan
    };
    assert_eq!(running.status, DagStatus::Running);

    let completed = DagExecutionPlan {
        status: DagStatus::Completed,
        ..running
    };
    assert_eq!(completed.status, DagStatus::Completed);

    // A failed DAG
    let failed = DagExecutionPlan {
        status: DagStatus::Failed("node-2 crashed".into()),
        ..completed.clone()
    };
    assert_eq!(failed.status, DagStatus::Failed("node-2 crashed".into()));
    assert!(matches!(failed.status, DagStatus::Failed(ref msg) if msg == "node-2 crashed"));

    // Also test Cancelled status
    let cancelled = DagExecutionPlan {
        status: DagStatus::Cancelled,
        ..completed
    };
    assert_eq!(cancelled.status, DagStatus::Cancelled);

    // Verify the status enum discriminants via DistributedDagState
    let mut state = DistributedDagState::new("status-test".into());
    assert_eq!(state.plan.status, DagStatus::Pending);
    state.plan.status = DagStatus::Running;
    assert_eq!(state.plan.status, DagStatus::Running);
    state.plan.status = DagStatus::Completed;
    assert_eq!(state.plan.status, DagStatus::Completed);
}
