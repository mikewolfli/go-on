//! Planner bridge helpers for AUTON execution chain.
//!
//! Keeps Planner -> ExecutionGraph wiring and DAG-order injection out of large
//! request handlers.

use serde_json::{json, Value};

use crate::agent::AgentTaskEnvelope;
use crate::orchestration::dag_execution::{
    dag_execution_order, dag_is_stalled, dag_progress_with_suggested_next,
};
use crate::orchestration::planner_execution_graph::PlannerExecutionBridge;
use crate::reinforcement::WorkflowGeneratedArtifact;

/// Build PlannerExecutionBridge from request context.
pub fn build_planner_bridge(
    task_id: impl Into<String>,
    phase: impl Into<String>,
    objective: impl Into<String>,
    params: &Value,
) -> PlannerExecutionBridge {
    let envelope = AgentTaskEnvelope {
        task_id: task_id.into(),
        phase: phase.into(),
        role: "planner".to_string(),
        objective: objective.into(),
        constraints: None,
        evidence: None,
        input: json!({"params": params}),
    };

    PlannerExecutionBridge::from_task(&envelope)
}

/// Apply DAG phased execution order to workflow artifact.
/// Returns true if workflow execution order was updated.
pub fn apply_dag_order_to_workflow(
    workflow: &mut WorkflowGeneratedArtifact,
    bridge: &PlannerExecutionBridge,
) -> bool {
    let order = dag_execution_order(bridge);
    if order.is_empty() {
        return false;
    }
    workflow.execution_order = order;
    true
}

/// Build rich planner execution graph payload for response observability.
pub fn planner_execution_graph_payload(bridge: &PlannerExecutionBridge) -> Value {
    let mut payload = dag_progress_with_suggested_next(bridge);
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("stalled".to_string(), Value::Bool(dag_is_stalled(bridge)));
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task_envelope() -> AgentTaskEnvelope {
        AgentTaskEnvelope {
            task_id: "bridge-test".to_string(),
            phase: "exec".to_string(),
            role: "tester".to_string(),
            objective: "Analyze and fix the authentication bug in the login system".to_string(),
            constraints: None,
            evidence: None,
            input: json!({}),
        }
    }

    #[test]
    fn test_build_planner_bridge_returns_valid_bridge() {
        let bridge = build_planner_bridge("task-1", "execution", "Fix the bug", &json!({}));
        // Bridge should be constructed without panicking
        assert!(bridge.total_steps > 0, "planner should produce steps");
    }

    #[test]
    fn test_apply_dag_order_to_empty_workflow() {
        let bridge = PlannerExecutionBridge::from_task(&make_task_envelope());
        let mut workflow = crate::reinforcement::WorkflowGeneratedArtifact {
            generated_at: 0,
            task: "test".to_string(),
            nodes: vec![],
            edges: vec![],
            execution_order: vec![],
            auto_gates: vec![],
            routing_summary: json!({}),
        };
        let updated = apply_dag_order_to_workflow(&mut workflow, &bridge);
        // Depending on the planner output, it may or may not update
        // Just verify it doesn't panic
        let _ = updated;
    }

    #[test]
    fn test_planner_execution_graph_payload_returns_expected_keys() {
        let bridge = PlannerExecutionBridge::from_task(&make_task_envelope());
        let payload = planner_execution_graph_payload(&bridge);

        assert!(
            payload.get("progress").is_some(),
            "payload should have progress"
        );
        assert!(
            payload.get("ready_nodes").is_some(),
            "payload should have ready_nodes"
        );
        assert!(
            payload.get("stalled").is_some(),
            "payload should have stalled flag"
        );
        assert!(
            payload["stalled"].as_bool().is_some(),
            "stalled should be a boolean"
        );
    }
}
