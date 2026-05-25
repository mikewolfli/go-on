//! DAG-driven execution adapter for the autonomy loop.
//!
//! Converts `PlannerExecutionBridge` (which wraps an `ExecutionGraph` DAG) into
//! the phased execution order format that `execute_runtime_subtasks` expects.
//! This wires AUTON-07: the DAG step ordering actually drives execution.
//!
//! Rather than modifying large files (exec_pack.rs, chat.rs), this adapter
//! is called at the boundary between planning and execution.

use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};

use crate::orchestration::execution_graph::ExNodeState;
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
    let plan_step_ids: HashSet<String> = bridge
        .plan
        .steps
        .iter()
        .map(|step| step.step_id.clone())
        .collect();

    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    for step in &bridge.plan.steps {
        deps.insert(step.step_id.clone(), step.depends_on.clone());
    }

    let mut remaining: BTreeSet<String> = plan_step_ids.iter().cloned().collect();
    let mut completed: HashSet<String> = HashSet::new();
    let mut phases: Vec<Vec<String>> = Vec::new();

    while !remaining.is_empty() {
        let ready_phase: Vec<String> = remaining
            .iter()
            .filter(|id| {
                deps.get(*id)
                    .map(|requirements| {
                        requirements
                            .iter()
                            .all(|dep| !plan_step_ids.contains(dep) || completed.contains(dep))
                    })
                    .unwrap_or(true)
            })
            .cloned()
            .collect();

        if ready_phase.is_empty() {
            // Dependency cycle or malformed graph: keep deterministic fallback
            // so runtime can still execute and surface diagnostics.
            phases.push(remaining.iter().cloned().collect());
            break;
        }

        for step_id in &ready_phase {
            let _ = remaining.remove(step_id);
            completed.insert(step_id.clone());
        }
        phases.push(ready_phase);
    }

    phases
}

/// Extract progress information from the DAG as a serializable value.
/// Includes both the raw progress snapshot and a derived "next step" hint.
pub fn dag_progress_with_suggested_next(bridge: &PlannerExecutionBridge) -> Value {
    let snapshot = bridge.progress_snapshot();
    let ready = bridge.ready_nodes();
    let completed = snapshot["completed"].as_u64().unwrap_or(0);
    let failed = snapshot["failed"].as_u64().unwrap_or(0);
    let total = snapshot["total_steps"].as_u64().unwrap_or(0);

    let next_step_hint = if !ready.is_empty() {
        format!("ready to execute: {}", ready.join(", "),)
    } else if completed >= total {
        "all steps completed".to_string()
    } else if failed > 0 {
        format!("{} steps failed, repair may be needed", failed)
    } else {
        "waiting for dependencies".to_string()
    };

    serde_json::json!({
        "progress": snapshot,
        "ready_nodes": ready,
        "next_step_hint": next_step_hint,
        "is_complete": bridge.is_complete(),
    })
}

/// Check whether the bridge DAG indicates a stalled state (no progress possible).
pub fn dag_is_stalled(bridge: &PlannerExecutionBridge) -> bool {
    let completed = bridge
        .graph
        .nodes
        .iter()
        .filter(|(_, n)| matches!(n.state, ExNodeState::Completed))
        .count();
    let failed = bridge
        .graph
        .nodes
        .iter()
        .filter(|(_, n)| matches!(n.state, ExNodeState::Failed(..)))
        .count();
    let total = bridge.total_steps;

    // Stalled if: not all done, not all failed, but nothing is ready
    !bridge.is_complete() && completed + failed < total && bridge.ready_nodes().is_empty()
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
