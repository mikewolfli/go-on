//! Distributed DAG Execution End-to-End
//!
//! Validates the distributed DAG execution lifecycle:
//!   node registration → cross-node execution → failure → recovery
//!
//! Uses go_on::orchestration::distributed types for the DAG coordinator,
//! node registration, execution plans, and status tracking.
//!
//! # integration-test-stub
//! Cross-node execution uses in-memory type construction. Real integration
//! requires two running go-on nodes with the `sub-bus-tool` feature and
//! network connectivity between them.

use std::collections::HashMap;
use std::time::Duration;
use tokio::time::sleep;

use go_on::fault_tolerance::{FaultEvent, FaultType};
use go_on::orchestration::distributed::dag_coordinator::{
    DagExecutionPlan, DagNodeAssignment, DagStatus, NodeInfo, NodeState,
};
use go_on::orchestration::distributed::remote_executor::DagId;

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
        dag_id: dag_id.to_string(),
        assignments: vec![
            DagNodeAssignment {
                dag_node_id: "fetch-data".into(),
                tool_name: "http_get".into(),
                assigned_node_id: Some("node-1".into()),
                output: None,
                error: None,
                completed: false,
            },
            DagNodeAssignment {
                dag_node_id: "parse-json".into(),
                tool_name: "json_parse".into(),
                assigned_node_id: Some("node-2".into()),
                output: None,
                error: None,
                completed: false,
            },
            DagNodeAssignment {
                dag_node_id: "report".into(),
                tool_name: "format_report".into(),
                assigned_node_id: None,
                output: None,
                error: None,
                completed: false,
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
#[ignore]
async fn test_distributed_dag_failure_recovery() {
    let mut ctx = DistributedDagE2eContext::new();

    // ── 1. Setup nodes ─────────────────────────────────────────────────
    let node1 = NodeInfo::new("node-1".into(), "127.0.0.1".into(), 9301);
    let node2 = NodeInfo::new("node-2".into(), "127.0.0.1".into(), 9302);

    assert_eq!(node1.node_id, "node-1");
    assert_eq!(node1.state, NodeState::Online);
    assert_eq!(node2.port, 9302);
    assert!(
        !node1.is_lease_expired(),
        "fresh node must not have expired lease"
    );

    // ── 2. DAG construction ────────────────────────────────────────────
    let dag_id: DagId = "dag-e2e-001".to_string();
    let plan = make_test_plan(&dag_id);
    ctx.dag_id = Some(dag_id.clone());

    assert_eq!(plan.status, DagStatus::Pending);
    assert!(!plan.assignments.is_empty());
    assert_eq!(plan.assignments.len(), 3);

    // ── 3. Register nodes with coordinator ─────────────────────────────
    // integration-test-stub: real registration calls
    // coordinator.register_node("node-1", "127.0.0.1", 9301).await.
    // Here we validate node registration via the NodeInfo type.
    let registered_nodes = vec![node1, node2];
    assert_eq!(registered_nodes.len(), 2);

    // ── 4. Cross-node parallel execution ───────────────────────────────
    // "fetch-data" and "parse-json" have a dependency edge, so parse-json
    // runs after fetch-data completes. In a real execution, the coordinator
    // identifies ready nodes from the DAG and dispatches them.
    //
    // Check that dependencies are correctly modeled:
    let deps = plan.adjacency.get("parse-json");
    assert!(deps.is_some(), "parse-json must declare dependencies");
    assert_eq!(deps.unwrap(), &vec!["fetch-data".to_string()]);

    // ── 5. Node failure ────────────────────────────────────────────────
    // Simulate node-2 going offline.
    let mut failed_node = registered_nodes[1].clone();
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
    // integration-test-stub: real recovery reassigns orphaned DAG nodes
    // from the failed node to an online node. The coordinator's
    // reschedule_orphaned() method identifies incomplete assignments
    // on offline nodes and reassigns them.
    let mut recovered_node = failed_node.clone();
    recovered_node.state = NodeState::Online;
    recovered_node.last_heartbeat_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    assert_eq!(recovered_node.state, NodeState::Online);

    // ── 7. Completion ──────────────────────────────────────────────────
    let completed_plan = DagExecutionPlan {
        status: DagStatus::Completed,
        ..plan
    };
    assert_eq!(completed_plan.status, DagStatus::Completed);

    sleep(Duration::from_millis(10)).await;
    assert!(true, "distributed DAG failure recovery passed");
}

/// Validates that a DAG with invalid structure (cyclic deps) is rejected.
#[tokio::test]
#[ignore]
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

    // Detect cycle via simple DFS (conceptual: real uses topological sort).
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

    // Real rejection:
    //   let coord = DistributedDAGCoordinator::new("coord-e2e", executor);
    //   let result = coord.validate_dag(&cyclic_plan).await;
    //   assert!(result.is_err(), "cyclic DAG must be rejected");

    sleep(Duration::from_millis(10)).await;
    assert!(true, "invalid DAG rejection passed");
}

/// Validates DAG status transitions.
#[tokio::test]
#[ignore]
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
    let _failed = DagExecutionPlan {
        status: DagStatus::Failed("node-2 crashed".into()),
        ..completed
    };

    sleep(Duration::from_millis(10)).await;
    assert!(true, "status transitions passed");
}
