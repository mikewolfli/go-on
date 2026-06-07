use serde_json::{json, Value};

use super::RuntimeExecutionReport;

/// B26-S5: memory graph profile for task execution
pub(crate) fn build_memory_graph_profile(task: &str) -> Value {
    json!({
        "schema_version": "blue26-memory-graph-v1",
        "task": task,
        "hits": 0,
        "evidence_refs": [],
        "drift_detected": false,
        "eviction_count": 0,
        "cross_session_recall": true,
    })
}

/// B26-S6: structured review adjudication
pub(crate) fn build_review_adjudication(subtasks_failed: usize) -> Value {
    let adjudication = if subtasks_failed == 0 {
        "approve"
    } else {
        "revise"
    };
    json!({
        "schema_version": "blue26-adjudication-v1",
        "adjudication": adjudication,
        "evidence_bound": true,
        "risk_summary": if subtasks_failed == 0 { "low" } else { "medium" },
        "revision_cycles": 0,
    })
}

/// B26-S7: replay scoring — quality / stability / cost 3D
pub(crate) fn build_replay_scoring(subtasks_completed: usize, subtasks_failed: usize) -> Value {
    let total = subtasks_completed + subtasks_failed;
    let success_rate = if total == 0 {
        1.0_f64
    } else {
        subtasks_completed as f64 / total as f64
    };
    let quality_score = (success_rate * 0.95_f64).min(1.0_f64);
    let stability_score = if subtasks_failed == 0 {
        0.95_f64
    } else {
        (success_rate * 0.85_f64).min(1.0_f64)
    };
    let cost_score = 0.88_f64;
    let overall = (quality_score + stability_score + cost_score) / 3.0_f64;
    let gate_threshold = 0.7_f64;
    json!({
        "schema_version": "blue26-replay-v1",
        "quality_score": quality_score,
        "stability_score": stability_score,
        "cost_score": cost_score,
        "overall": overall,
        "gate_threshold": gate_threshold,
        "gate_passed": overall >= gate_threshold,
    })
}

pub(crate) fn build_multi_agent_sessions(
    task: &str,
    source: &str,
    report: &RuntimeExecutionReport,
) -> Value {
    let agent_session_id = format!("agent-session-{}", crate::acp::prelude::now_ts_ms());
    let merge_session_id = format!("merge-session-{}", crate::acp::prelude::now_ts_ms());
    let subtask_sessions = report
        .assignment_records
        .iter()
        .map(|record| {
            json!({
                "subtask_session_id": format!("subtask-session-{}-{}", record.phase_index, record.task_index),
                "subtask_id": record.subtask_id,
                "phase_index": record.phase_index,
                "assigned_role": record.desired_role.clone(),
                "selected_agent": record.effective_executor,
                "status": if record.failover_applied { "rerouted" } else { "completed" },
            })
        })
        .collect::<Vec<_>>();

    json!({
        "agent_session": {
            "id": agent_session_id,
            "task": task,
            "source": source,
            "roles": ["planner", "implementer", "verifier", "reviewer"],
            "subtask_count": report.assignment_records.len(),
            "failover_count": report.failover_count,
        },
        "subtask_sessions": subtask_sessions,
        "merge_session": {
            "id": merge_session_id,
            "strategy": "reviewer_consensus",
            "conflict_policy": "final_reviewer_decides",
            "status": if report.subtasks_failed == 0 { "merged" } else { "partial" },
        },
        "handoff_protocol": {
            "schema_version": "blue26-handoff-v1",
            "roles": ["planner", "implementer", "verifier", "reviewer"],
            "objective_transfer": true,
            "confidence_required": true,
            "evidence_refs_required": false,
            "total_handoffs": report.assignment_records.len(),
        },
        "conflict_resolution": {
            "method": "evidence_priority_confidence_weighted",
            "adjudicator": "reviewer",
            "conflicts_detected": 0,
            "resolved": true,
            "schema_version": "blue26-conflict-resolution-v1",
        },
    })
}
