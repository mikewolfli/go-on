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
