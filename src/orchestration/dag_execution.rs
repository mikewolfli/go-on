//! Deprecated wrapper around [`crate::orchestration::core_dag::CoreDag<T>`].
//!
//! Provides `dag_execution_order`, `dag_progress_with_suggested_next`, and
//! `dag_is_stalled` — legacy adapter functions that convert a
//! [`PlannerExecutionBridge`](crate::orchestration::planner_execution_graph::PlannerExecutionBridge)
//! into phased execution orders.
//! New code should use `CoreDag<T>` directly.
//!
//! # Deprecated
//! Use [`crate::orchestration::core_dag::CoreDag<T>`] for new code.

#![deprecated(note = "Use core_dag::CoreDag instead")]

use serde_json::Value;

use crate::orchestration::planner_execution_graph::PlannerExecutionBridge;

/// Build a phased execution order from the planner DAG.
///
/// Returns `Vec<Vec<String>>` where each inner vec is a phase of ready-to-run
/// node IDs. This matches the `execution_order` format in
/// `WorkflowGeneratedArtifact` used by `execute_runtime_subtasks`.
///
/// This returns the complete plan order (topological phases), not just the
/// first set of currently-ready nodes. Structural nodes (Start/End/Join/
/// Condition) are excluded because the planner bridge tracks only plan steps.
pub fn dag_execution_order(bridge: &PlannerExecutionBridge) -> Vec<Vec<String>> {
    crate::orchestration::core_dag::dag_execution_order(bridge)
}

/// Extract progress information from the DAG as a serializable value.
/// Includes both the raw progress snapshot and a derived "next step" hint.
pub fn dag_progress_with_suggested_next(bridge: &PlannerExecutionBridge) -> Value {
    crate::orchestration::core_dag::dag_progress_with_suggested_next(bridge)
}

/// Check whether the bridge DAG indicates a stalled state (no progress possible).
pub fn dag_is_stalled(bridge: &PlannerExecutionBridge) -> bool {
    crate::orchestration::core_dag::dag_is_stalled(bridge)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentTaskEnvelope;

    fn make_bridge() -> PlannerExecutionBridge {
        let envelope = AgentTaskEnvelope {
            task_id: "dag-test".to_string(),
            phase: "exec".to_string(),
            role: "tester".to_string(),
            objective: "Analyze the bug in the authentication system and implement the fix"
                .to_string(),
            constraints: None,
            evidence: None,
            input: serde_json::json!({}),
        };
        PlannerExecutionBridge::from_task(&envelope)
    }

    #[test]
    fn dag_order_returns_ready_nodes() {
        let bridge = make_bridge();
        let order = dag_execution_order(&bridge);
        // plan-1 should be ready (no dependencies)
        assert!(!order.is_empty());
        assert!(order[0].contains(&"plan-1".to_string()));
    }

    #[test]
    fn dag_order_covers_all_plan_steps() {
        let bridge = make_bridge();
        let order = dag_execution_order(&bridge);
        let flattened: Vec<String> = order.into_iter().flatten().collect();

        for step in &bridge.plan.steps {
            assert!(
                flattened.iter().any(|id| id == &step.step_id),
                "missing step in execution order: {}",
                step.step_id
            );
        }
    }

    #[test]
    fn dag_progress_includes_ready_hint() {
        let bridge = make_bridge();
        let progress = dag_progress_with_suggested_next(&bridge);
        assert!(progress["next_step_hint"]
            .as_str()
            .unwrap_or("")
            .contains("ready"));
    }

    #[test]
    fn dag_not_stalled_initially() {
        let bridge = make_bridge();
        // plan-1 is ready, so not stalled
        assert!(!dag_is_stalled(&bridge));
    }
}
