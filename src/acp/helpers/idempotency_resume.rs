use serde_json::{json, Value};

fn bool_at_path(payload: &Value, path: &[&str]) -> bool {
    let mut current = payload;
    for segment in path {
        let Some(next) = current.get(*segment) else {
            return false;
        };
        current = next;
    }
    current.as_bool().unwrap_or(false)
}

fn string_at_path(payload: &Value, path: &[&str]) -> Option<String> {
    let mut current = payload;
    for segment in path {
        let next = current.get(*segment)?;
        current = next;
    }
    current.as_str().map(|value| value.to_string())
}

fn u64_at_path(payload: &Value, path: &[&str]) -> Option<u64> {
    let mut current = payload;
    for segment in path {
        let next = current.get(*segment)?;
        current = next;
    }
    current.as_u64()
}

pub(crate) fn derive_idempotency_continuation(payload: &Value) -> Value {
    let run_status = payload
        .get("run_status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let failed_subtasks = u64_at_path(payload, &["summary", "subtasks_failed"]).unwrap_or(0);
    let pending_repair_iterations =
        u64_at_path(payload, &["execution_cycle", "pending_repair_iterations"]).unwrap_or(0);
    let resume_eligible = bool_at_path(payload, &["execution_cycle", "task_graph_checkpoint", "resume_eligible"])
        || bool_at_path(payload, &["execution_cycle", "resume_eligible"]);

    let pending_execution = run_status.eq_ignore_ascii_case("failed")
        || failed_subtasks > 0
        || pending_repair_iterations > 0
        || resume_eligible;

    let checkpoint_id = string_at_path(
        payload,
        &["execution_cycle", "task_graph_checkpoint", "checkpoint_id"],
    );

    let reason = if pending_execution {
        if pending_repair_iterations > 0 {
            "cached_result_has_pending_repair_iterations"
        } else if failed_subtasks > 0 {
            "cached_result_contains_failed_subtasks"
        } else if resume_eligible {
            "cached_result_is_resume_eligible"
        } else {
            "cached_result_requires_continuation"
        }
    } else {
        "cached_result_finalized"
    };

    let next_step = if pending_execution {
        json!({
            "method": "task.execute",
            "resume_eligible": true,
            "checkpoint_id": checkpoint_id,
            "idempotency_allow_stale": false,
        })
    } else {
        json!({"status": "completed"})
    };

    json!({
        "pending_execution": pending_execution,
        "resume_eligible": resume_eligible,
        "failed_subtasks": failed_subtasks,
        "pending_repair_iterations": pending_repair_iterations,
        "checkpoint_id": checkpoint_id,
        "reason": reason,
        "next_step": next_step,
    })
}