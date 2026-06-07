use serde_json::{json, Value};

#[allow(dead_code)] // helper for derive_idempotency_continuation (currently dead)
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

#[allow(dead_code)] // helper for derive_idempotency_continuation (currently dead)
fn string_at_path(payload: &Value, path: &[&str]) -> Option<String> {
    let mut current = payload;
    for segment in path {
        let next = current.get(*segment)?;
        current = next;
    }
    current.as_str().map(|value| value.to_string())
}

#[allow(dead_code)] // helper for derive_idempotency_continuation (currently dead)
fn u64_at_path(payload: &Value, path: &[&str]) -> Option<u64> {
    let mut current = payload;
    for segment in path {
        let next = current.get(*segment)?;
        current = next;
    }
    current.as_u64()
}

#[allow(dead_code)] // F-GAP: reserved for idempotency resume pipeline
pub(crate) fn derive_idempotency_continuation(payload: &Value) -> Value {
    let run_status = payload
        .get("run_status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let failed_subtasks = u64_at_path(payload, &["summary", "subtasks_failed"]).unwrap_or(0);
    let pending_repair_iterations =
        u64_at_path(payload, &["execution_cycle", "pending_repair_iterations"]).unwrap_or(0);
    let resume_eligible = bool_at_path(
        payload,
        &[
            "execution_cycle",
            "task_graph_checkpoint",
            "resume_eligible",
        ],
    ) || bool_at_path(payload, &["execution_cycle", "resume_eligible"]);

    let pending_execution = run_status.eq_ignore_ascii_case("failed")
        || run_status.eq_ignore_ascii_case("waiting_clarification")
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

#[allow(dead_code)] // F-GAP: reserved for idempotency resume pipeline
pub(crate) fn annotate_idempotency_hit(
    mut cached_response: Value,
    idempotency_key: &str,
    bypassed_for_execution: bool,
) -> Value {
    let continuation = derive_idempotency_continuation(&cached_response);
    let continuation_pending = continuation
        .get("pending_execution")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if let Some(obj) = cached_response.as_object_mut() {
        let original_run_status = obj
            .get("run_status")
            .and_then(Value::as_str)
            .map(str::to_string);

        if continuation_pending {
            if let Some(previous_status) = original_run_status {
                obj.insert(
                    "idempotency_previous_run_status".to_string(),
                    Value::String(previous_status),
                );
            }

            obj.insert(
                "run_status".to_string(),
                Value::String("continuation_pending".to_string()),
            );

            if let Some(next_step) = continuation.get("next_step").cloned() {
                obj.insert("next_step".to_string(), next_step);
            }
        }

        obj.insert(
            "idempotency".to_string(),
            json!({
                "hit": true,
                "key": idempotency_key,
                "bypassed_for_execution": bypassed_for_execution,
                "continuation_pending": continuation_pending,
            }),
        );
        obj.insert("continuation".to_string(), continuation);
    }

    cached_response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotate_idempotency_hit_marks_pending_execution() {
        let cached = json!({
            "run_status": "failed",
            "summary": { "subtasks_failed": 1 },
            "execution_cycle": {
                "task_graph_checkpoint": {
                    "resume_eligible": true,
                    "checkpoint_id": "ckpt-123"
                }
            }
        });

        let annotated = annotate_idempotency_hit(cached, "idempotency-key-1", false);

        assert_eq!(
            annotated.get("run_status").and_then(Value::as_str),
            Some("continuation_pending")
        );
        assert_eq!(
            annotated
                .get("idempotency_previous_run_status")
                .and_then(Value::as_str),
            Some("failed")
        );
        assert_eq!(
            annotated
                .get("idempotency")
                .and_then(|value| value.get("continuation_pending"))
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            annotated
                .get("next_step")
                .and_then(|value| value.get("checkpoint_id"))
                .and_then(Value::as_str),
            Some("ckpt-123")
        );
    }

    #[test]
    fn annotate_idempotency_hit_keeps_finalized_response_stable() {
        let cached = json!({
            "run_status": "succeeded",
            "summary": { "subtasks_failed": 0 },
            "execution_cycle": {
                "task_graph_checkpoint": {
                    "resume_eligible": false
                }
            }
        });

        let annotated = annotate_idempotency_hit(cached, "idempotency-key-2", false);

        assert_eq!(
            annotated.get("run_status").and_then(Value::as_str),
            Some("succeeded")
        );
        assert_eq!(
            annotated
                .get("idempotency")
                .and_then(|value| value.get("continuation_pending"))
                .and_then(Value::as_bool),
            Some(false)
        );
        assert!(annotated.get("idempotency_previous_run_status").is_none());
    }

    #[test]
    fn annotate_idempotency_hit_marks_waiting_clarification_as_pending() {
        let cached = json!({
            "run_status": "waiting_clarification",
            "summary": { "subtasks_failed": 0 },
            "execution_cycle": {
                "task_graph_checkpoint": {
                    "resume_eligible": false,
                    "checkpoint_id": "ckpt-clarify"
                }
            }
        });

        let annotated = annotate_idempotency_hit(cached, "idempotency-key-3", false);

        assert_eq!(
            annotated.get("run_status").and_then(Value::as_str),
            Some("continuation_pending")
        );
        assert_eq!(
            annotated
                .get("idempotency_previous_run_status")
                .and_then(Value::as_str),
            Some("waiting_clarification")
        );
        assert_eq!(
            annotated
                .get("idempotency")
                .and_then(|value| value.get("continuation_pending"))
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            annotated
                .get("idempotency")
                .and_then(|value| value.get("key"))
                .and_then(Value::as_str),
            Some("idempotency-key-3")
        );
    }
}
