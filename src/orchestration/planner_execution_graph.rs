//! Bridge between `Planner::plan()` output and the `ExecutionGraph` DAG.
//!
//! This module converts a Planner `ExecutionPlan` into an `ExecutionGraph` DAG,
//! enabling the ACP execution path to use `ExecutionGraph::get_ready_nodes()` for
//! real DAG-driven step execution instead of sequential step walking.
//!
//! Implements AUTON-07: multi-agent orchestration wiring.

use serde_json::Value;

use crate::agent::AgentTaskEnvelope;
use crate::orchestration::execution_graph::{ExNode, ExNodeKind, ExNodeState, ExecutionGraph};
use crate::orchestration::planner_executor::{ExecutionPlan, Planner};

/// Build an `ExecutionGraph` DAG from a `Planner::plan()` output.
///
/// Each `PlanStep` becomes a node in the DAG with its dependency edges.
#[allow(dead_code)]
pub fn build_execution_graph_from_plan(plan: &ExecutionPlan) -> ExecutionGraph {
    let mut graph = ExecutionGraph::new(&format!("plan-{}", plan.plan_id));

    // Add a task node for each plan step
    for step in &plan.steps {
        let node = ExNode {
            id: step.step_id.clone(),
            kind: ExNodeKind::Task,
            name: step.description.clone(),
            state: ExNodeState::Pending,
            input: serde_json::json!({
                "mode": format!("{:?}", step.mode),
                "agent": step.agent,
                "timeout_seconds": step.timeout_seconds,
            }),
            output: None,
            cost_estimate: 1.0,
            error: None,
            duration_ms: None,
        };
        graph.add_node(node);
    }

    // Add edges for dependency relationships
    for step in &plan.steps {
        for dep in &step.depends_on {
            graph.add_edge(dep.as_str(), step.step_id.as_str(), Some("depends"));
        }
    }

    // Note: parallel group fan-out wiring is a future enhancement
    // when Planner produces structured parallel_groups.
    // See ExecutionGraph::add_fan_out for the DAG wiring API.

    graph
}

/// Bridge result containing both the DAG and derived metadata
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PlannerExecutionBridge {
    /// The execution plan from Planner
    pub plan: ExecutionPlan,
    /// The execution graph DAG
    pub graph: ExecutionGraph,
    /// Total expected steps
    pub total_steps: usize,
}

#[allow(dead_code)]
impl PlannerExecutionBridge {
    /// Create a new bridge from a task envelope.
    #[allow(dead_code)]
    pub fn from_task(task: &AgentTaskEnvelope) -> Self {
        let plan = Planner::plan(task);
        let graph = build_execution_graph_from_plan(&plan);
        let total_steps = plan.steps.len();
        Self {
            plan,
            graph,
            total_steps,
        }
    }

    /// Get ready node IDs (nodes whose dependencies are satisfied) from the DAG.
    pub fn ready_nodes(&self) -> Vec<String> {
        self.graph.get_ready_nodes()
    }

    /// Mark a step as completed in the DAG.
    pub fn complete_step(&mut self, step_id: &str, output: Value) {
        let _ = self.graph.complete_task(step_id, output);
    }

    /// Mark a step as failed in the DAG.
    pub fn fail_step(&mut self, step_id: &str, error: String) {
        let _ = self
            .graph
            .set_node_state(step_id, ExNodeState::Failed(error.clone()));
        if let Some((_id, node)) = self.graph.nodes.iter_mut().find(|(id, _)| *id == step_id) {
            node.error = Some(error);
        }
    }

    /// Check if the entire DAG execution is complete.
    pub fn is_complete(&self) -> bool {
        self.graph.is_complete()
    }

    /// Get the current execution progress as a serializable value.
    pub fn progress_snapshot(&self) -> Value {
        let completed = self
            .graph
            .nodes
            .iter()
            .filter(|(_, n)| matches!(n.state, ExNodeState::Completed))
            .count();
        let failed = self
            .graph
            .nodes
            .iter()
            .filter(|(_, n)| matches!(n.state, ExNodeState::Failed(..)))
            .count();
        let pending = self
            .graph
            .nodes
            .iter()
            .filter(|(_, n)| matches!(n.state, ExNodeState::Pending))
            .count();

        serde_json::json!({
            "plan_id": self.plan.plan_id,
            "total_steps": self.total_steps,
            "completed": completed,
            "failed": failed,
            "pending": pending,
            "ready": self.graph.get_ready_nodes().len(),
            "parallel_groups": self.plan.parallel_groups.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_envelope() -> AgentTaskEnvelope {
        AgentTaskEnvelope {
            task_id: "test-bridge".to_string(),
            phase: "execution".to_string(),
            role: "tester".to_string(),
            objective: "Test the planner-execution-graph bridge".to_string(),
            constraints: None,
            evidence: None,
            input: serde_json::json!({}),
        }
    }

    #[test]
    fn bridge_creates_dag_with_correct_node_count() {
        let bridge = PlannerExecutionBridge::from_task(&make_test_envelope());
        // Planner creates 3 steps: plan-1, exec-1, review-1
        assert_eq!(bridge.total_steps, 3);
        // Plus Start + End nodes from ExecutionGraph
        assert!(bridge.graph.nodes.len() >= 3);
    }

    #[test]
    fn bridge_ready_nodes_starts_with_plan_step() {
        let bridge = PlannerExecutionBridge::from_task(&make_test_envelope());
        let ready = bridge.ready_nodes();
        // plan-1 should be ready (no dependencies)
        assert!(ready.iter().any(|id| id == "plan-1"));
    }

    #[test]
    fn bridge_tracks_progress_correctly() {
        let mut bridge = PlannerExecutionBridge::from_task(&make_test_envelope());
        bridge.complete_step("plan-1", serde_json::Value::Null);
        let snapshot = bridge.progress_snapshot();
        assert_eq!(snapshot["completed"].as_u64(), Some(2)); // Start node + completed step
    }

    #[test]
    fn bridge_fail_propagation() {
        let mut bridge = PlannerExecutionBridge::from_task(&make_test_envelope());
        bridge.fail_step("plan-1", "test failure".to_string());
        // exec-1 depends on plan-1, so it should NOT be ready
        let ready = bridge.ready_nodes();
        assert!(!ready.iter().any(|id| id == "exec-1"));
    }
}
