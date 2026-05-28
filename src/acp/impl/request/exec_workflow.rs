//! Workflow run execution orchestration.
//!
//! Provides the full lifecycle management for workflow runs:
//! creating, tracking, listing, and transitioning workflow execution states.
//! All functions are accessible via `pub(super)` from the request module.

use super::*;

// ── Helpers ────────────────────────────────────────────────────────────────

fn workflow_runs() -> &'static StdMutex<Vec<WorkflowRunRecord>> {
    WORKFLOW_RUNS.get_or_init(|| StdMutex::new(Vec::new()))
}

fn workflow_runs_lock_guard() -> Result<std::sync::MutexGuard<'static, Vec<WorkflowRunRecord>>> {
    workflow_runs()
        .lock()
        .map_err(|poisoned| {
            let recovered = poisoned.into_inner();
            warn!(target: "acp::exec_workflow", "workflow_runs Mutex poisoned – recovered data");
            recovered
        })
}

fn next_workflow_run_id() -> String {
    let seq = WORKFLOW_RUN_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("run-{}-{}", crate::acp::prelude::now_ts_ms(), seq)
}

fn merge_effective_option_from_root(params: &Value, key: &str, out: &mut HashMap<String, Value>) {
    if let Some(value) = params.get(key) {
        out.insert(key.to_string(), value.clone());
    }
}

fn extract_effective_options(params: &Value) -> HashMap<String, Value> {
    let mut options = HashMap::new();
    let whitelist = ["temperature", "top_p", "max_tokens", "model"];

    if let Some(extra) = params
        .get("options")
        .and_then(|value| value.get("extra"))
        .and_then(Value::as_object)
    {
        for key in whitelist {
            if let Some(value) = extra.get(key) {
                options.insert(key.to_string(), value.clone());
            }
        }
    }

    for key in ["temperature", "top_p", "max_tokens", "model"] {
        merge_effective_option_from_root(params, key, &mut options);
    }

    options
}

fn effective_options_value(params: &Value) -> Value {
    Value::Object(extract_effective_options(params).into_iter().collect())
}

fn run_id_from_params(params: &Value) -> Option<String> {
    params
        .get("run_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

// ── Workflow state machine ─────────────────────────────────────────────────

/// Validates that `target_status` is a legal transition from `current_status`.
fn is_valid_transition(current_status: &str, target_status: &str) -> bool {
    match (current_status, target_status) {
        ("queued", "cancelled") | ("queued", "running") => true,
        ("running", "paused") | ("running", "cancelled") => true,
        ("running", "succeeded") | ("running", "failed") => true,
        ("paused", "running") | ("paused", "cancelled") => true,
        _ if current_status == target_status => true,
        _ => false,
    }
}

/// All terminal status values that set `ended_at`.
fn is_terminal_status(status: &str) -> bool {
    matches!(status, "succeeded" | "failed" | "cancelled")
}

// ── Public API ─────────────────────────────────────────────────────────────

pub(super) fn start_workflow_run(
    source_method: &str,
    task: &str,
    phase: Option<&str>,
    params: &Value,
) -> WorkflowRunRecord {
    let now = crate::acp::prelude::now_ts();
    let record = WorkflowRunRecord {
        run_id: run_id_from_params(params).unwrap_or_else(next_workflow_run_id),
        source_method: source_method.to_string(),
        task: task.to_string(),
        status: "running".to_string(),
        phase: phase.unwrap_or("default").to_string(),
        created_at: now,
        started_at: now,
        ended_at: None,
        error: None,
        artifacts: Vec::new(),
        effective_options: effective_options_value(params),
    };

    if let Ok(mut guard) = workflow_runs_lock_guard() {
        guard.push(record.clone());
        if guard.len() > 2000 {
            let overflow = guard.len() - 2000;
            guard.drain(0..overflow);
        }
    }

    record
}

pub(super) fn complete_workflow_run(
    run_id: &str,
    status: &str,
    error: Option<String>,
    artifacts: Vec<String>,
) {
    if let Ok(mut guard) = workflow_runs_lock_guard() {
        if let Some(item) = guard.iter_mut().find(|record| record.run_id == run_id) {
            item.status = status.to_string();
            item.error = error;
            item.artifacts = artifacts;
            if is_terminal_status(status) {
                item.ended_at = Some(crate::acp::prelude::now_ts());
            }
        }
    }
}

pub(super) fn get_workflow_run_record(run_id: &str) -> Option<WorkflowRunRecord> {
    workflow_runs_lock_guard()
        .ok()
        .and_then(|guard| guard.iter().find(|record| record.run_id == run_id).cloned())
}

pub(super) fn transition_workflow_run(run_id: &str, target_status: &str) -> Result<WorkflowRunRecord> {
    let mut guard = workflow_runs_lock_guard()?;
    let record = guard
        .iter_mut()
        .find(|item| item.run_id == run_id)
        .ok_or_else(|| anyhow::anyhow!("workflow run '{}' not found", run_id))?;

    if !is_valid_transition(record.status.as_str(), target_status) {
        anyhow::bail!(
            "invalid status transition: {} -> {}",
            record.status,
            target_status
        );
    }

    record.status = target_status.to_string();
    if is_terminal_status(target_status) {
        record.ended_at = Some(crate::acp::prelude::now_ts());
    }
    Ok(record.clone())
}

pub(super) fn execution_option_overrides(params: &Value) -> HashMap<String, Value> {
    extract_effective_options(params)
}

// ── Query payloads ─────────────────────────────────────────────────────────

pub(super) fn workflow_run_list_payload(params: &Value) -> Value {
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(50)
        .min(500);
    let offset = params
        .get("offset")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(0);

    enum StatusFilter {
        Any,
        One(String),
        Many(std::collections::HashSet<String>),
    }

    let status_filter = match params.get("status") {
        Some(Value::String(single)) => StatusFilter::One(single.clone()),
        Some(Value::Array(items)) => {
            let values: Vec<_> = items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect();
            if values.is_empty() {
                StatusFilter::Any
            } else {
                StatusFilter::Many(values.into_iter().collect())
            }
        }
        _ => StatusFilter::Any,
    };

    let matches_status = |record: &WorkflowRunRecord| match &status_filter {
        StatusFilter::Any => true,
        StatusFilter::One(single) => record.status == *single,
        StatusFilter::Many(items) => items.contains(&record.status),
    };

    let (total, runs) = match workflow_runs_lock_guard() {
        Ok(guard) => {
            let mut total = 0usize;
            let mut runs = Vec::new();
            for record in guard.iter().rev() {
                if !matches_status(record) {
                    continue;
                }
                if total >= offset && runs.len() < limit {
                    runs.push(record.clone());
                }
                total += 1;
            }
            (total, runs)
        }
        Err(_) => (0usize, Vec::new()),
    };

    json!({
        "ok": true,
        "total": total,
        "offset": offset,
        "limit": limit,
        "runs": runs,
    })
}

pub(super) fn workflow_run_get_payload(params: &Value) -> Result<Value> {
    let run_id = params
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("run_id is required"))?;

    match get_workflow_run_record(run_id) {
        Some(run) => Ok(json!({"ok": true, "run": run})),
        None => Err(anyhow::anyhow!("workflow run '{}' not found", run_id)),
    }
}

pub(super) fn workflow_run_transition_payload(
    params: &Value,
    target_status: &str,
) -> Result<Value> {
    let run_id = params
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("run_id is required"))?;

    let run = transition_workflow_run(run_id, target_status)?;
    Ok(json!({"ok": true, "run": run, "action": target_status}))
}

/// Cancel a workflow run – convenience shorthand for transition.
pub(super) fn workflow_run_cancel_payload(params: &Value) -> Result<Value> {
    workflow_run_transition_payload(params, "cancelled")
}

/// Transition workflow run to "running" – convenience for queued items.
pub(super) fn workflow_run_start_payload(params: &Value) -> Result<Value> {
    workflow_run_transition_payload(params, "running")
}

/// Set workflow run to "succeeded" with optional artifact list.
pub(super) fn workflow_run_succeed_payload(params: &Value, artifacts: Vec<String>) -> Result<Value> {
    let run_id = params
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("run_id is required"))?;
    let mut guard = workflow_runs_lock_guard()?;
    let record = guard
        .iter_mut()
        .find(|item| item.run_id == run_id)
        .ok_or_else(|| anyhow::anyhow!("workflow run '{}' not found", run_id))?;

    if !is_valid_transition(record.status.as_str(), "succeeded") {
        anyhow::bail!("invalid status transition: {} -> succeeded", record.status);
    }

    record.status = "succeeded".to_string();
    record.ended_at = Some(crate::acp::prelude::now_ts());
    record.artifacts = artifacts;
    Ok(json!({"ok": true, "run": record.clone()}))
}

// ── Async handlers ─────────────────────────────────────────────────────────

pub(super) async fn handle_workflow_run_list(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, workflow_run_list_payload(&params)).await
}

pub(super) async fn handle_workflow_run_get(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    match workflow_run_get_payload(&params) {
        Ok(payload) => send_result(server, request_id, payload).await,
        Err(err) => send_error(server, request_id, -32602, err.to_string(), None).await,
    }
}

pub(super) async fn handle_workflow_run_cancel(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    match workflow_run_cancel_payload(&params) {
        Ok(payload) => send_result(server, request_id, payload).await,
        Err(err) => send_error(server, request_id, -32602, err.to_string(), None).await,
    }
}

pub(super) async fn handle_workflow_run_start(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    match workflow_run_start_payload(&params) {
        Ok(payload) => send_result(server, request_id, payload).await,
        Err(err) => send_error(server, request_id, -32602, err.to_string(), None).await,
    }
}

pub(super) async fn handle_workflow_run_complete(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let run_id = params.get("run_id").and_then(Value::as_str).unwrap_or("");
    let status = params.get("status").and_then(Value::as_str).unwrap_or("succeeded");
    let error = params.get("error").and_then(Value::as_str).map(ToString::to_string);
    let artifacts = params
        .get("artifacts")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).map(ToString::to_string).collect())
        .unwrap_or_default();

    complete_workflow_run(run_id, status, error, artifacts);
    send_result(server, request_id, json!({"ok": true, "run_id": run_id, "status": status})).await
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_next_workflow_run_id_format() {
        let id = next_workflow_run_id();
        assert!(id.starts_with("run-"), "run id should start with 'run-'");
        assert!(id.len() > 10, "run id should contain timestamp + seq");
    }

    #[test]
    fn test_extract_effective_options_from_extra() {
        let params = json!({
            "options": {
                "extra": {
                    "temperature": 0.7,
                    "top_p": 0.9,
                    "max_tokens": 2048,
                    "model": "gpt-4"
                }
            }
        });
        let opts = extract_effective_options(&params);
        assert_eq!(opts.get("temperature").and_then(Value::as_f64), Some(0.7));
        assert_eq!(opts.get("model").and_then(Value::as_str), Some("gpt-4"));
    }

    #[test]
    fn test_extract_effective_options_from_root_overrides_extra() {
        let params = json!({
            "temperature": 0.5,
            "options": { "extra": { "temperature": 0.9 } }
        });
        let opts = extract_effective_options(&params);
        assert_eq!(
            opts.get("temperature").and_then(Value::as_f64),
            Some(0.5),
            "root-level temperature should override extra"
        );
    }

    #[test]
    fn test_run_id_from_params() {
        assert_eq!(run_id_from_params(&json!({"run_id": "abc"})), Some("abc".into()));
        assert_eq!(run_id_from_params(&json!({"run_id": "  "})), None);
        assert_eq!(run_id_from_params(&json!({"run_id": ""})), None);
        assert_eq!(run_id_from_params(&json!({})), None);
    }

    #[test]
    fn test_start_and_complete_workflow_run() {
        // start
        let record = start_workflow_run("test_method", "test task", Some("test"), &json!({}));
        assert_eq!(record.status, "running");
        assert_eq!(record.task, "test task");
        assert!(record.ended_at.is_none());

        // complete with success
        complete_workflow_run(&record.run_id, "succeeded", None, vec!["result.txt".into()]);
        let stored = get_workflow_run_record(&record.run_id).expect("should exist");
        assert_eq!(stored.status, "succeeded");
        assert!(stored.ended_at.is_some());
        assert_eq!(stored.artifacts, vec!["result.txt"]);

        // complete with failure
        let record2 = start_workflow_run("test_method", "failing task", None, &json!({}));
        complete_workflow_run(&record2.run_id, "failed", Some("timeout".into()), vec![]);
        let stored2 = get_workflow_run_record(&record2.run_id).expect("should exist");
        assert_eq!(stored2.status, "failed");
        assert_eq!(stored2.error.as_deref(), Some("timeout"));
    }

    #[test]
    fn test_transition_workflow_run_valid() {
        let record = start_workflow_run("test", "test", None, &json!({}));
        // running -> succeeded
        let updated = transition_workflow_run(&record.run_id, "succeeded").unwrap();
        assert_eq!(updated.status, "succeeded");
        assert!(updated.ended_at.is_some());
    }

    #[test]
    fn test_transition_workflow_run_invalid() {
        let record = start_workflow_run("test", "test", None, &json!({}));
        // running -> queued is invalid (going backwards)
        let err = transition_workflow_run(&record.run_id, "queued").unwrap_err();
        assert!(err.to_string().contains("invalid status transition"));
    }

    #[test]
    fn test_transition_workflow_run_idempotent() {
        let record = start_workflow_run("test", "test", None, &json!({}));
        transition_workflow_run(&record.run_id, "running").unwrap(); // same status no-op
        let stored = get_workflow_run_record(&record.run_id).unwrap();
        assert_eq!(stored.status, "running");
    }

    #[test]
    fn test_workflow_run_list_pagination() {
        for i in 0..10 {
            start_workflow_run("test", &format!("task {}", i), None, &json!({}));
        }
        let params = json!({"limit": 3, "offset": 0});
        let result = workflow_run_list_payload(&params);
        assert_eq!(result["ok"].as_bool(), Some(true));
        assert_eq!(result["runs"].as_array().map(|a| a.len()), Some(3));
        assert!(result["total"].as_u64().unwrap_or(0) >= 10);
    }

    #[test]
    fn test_workflow_run_list_by_status() {
        let r1 = start_workflow_run("test", "task1", None, &json!({}));
        let r2 = start_workflow_run("test", "task2", None, &json!({}));
        transition_workflow_run(&r1.run_id, "succeeded").ok();

        let params = json!({"status": "succeeded"});
        let result = workflow_run_list_payload(&params);
        let runs = result["runs"].as_array().unwrap();
        assert!(runs.iter().all(|r| r["status"] == "succeeded"));

        let params2 = json!({"status": ["running", "succeeded"]});
        let result2 = workflow_run_list_payload(&params2);
        let runs2 = result2["runs"].as_array().unwrap();
        assert!(runs2.iter().any(|r| r["run_id"] == r2.run_id));
    }

    #[test]
    fn test_workflow_run_get() {
        let record = start_workflow_run("test", "get me", None, &json!({}));
        let params = json!({"run_id": &record.run_id});
        let result = workflow_run_get_payload(&params).unwrap();
        assert_eq!(result["run"]["task"], "get me");
    }

    #[test]
    fn test_workflow_run_get_not_found() {
        let result = workflow_run_get_payload(&json!({"run_id": "nonexistent"}));
        assert!(result.is_err());
    }

    #[test]
    fn test_workflow_run_succeed() {
        let record = start_workflow_run("test", "succeed me", None, &json!({}));
        let params = json!({"run_id": &record.run_id});
        let result = workflow_run_succeed_payload(&params, vec!["output.json".into()]).unwrap();
        assert_eq!(result["run"]["status"], "succeeded");
        assert!(result["run"]["ended_at"].is_number());
        assert_eq!(
            result["run"]["artifacts"].as_array().map(|a| a.len()),
            Some(1)
        );
    }

    #[test]
    fn test_is_valid_transition_covers_failed() {
        assert!(is_valid_transition("running", "failed"));
        assert!(!is_valid_transition("queued", "failed"));
        assert!(is_terminal_status("failed"));
        assert!(is_terminal_status("succeeded"));
        assert!(is_terminal_status("cancelled"));
        assert!(!is_terminal_status("running"));
    }

    #[test]
    fn test_execution_option_overrides_respects_whitelist() {
        let params = json!({
            "temperature": 0.3,
            "top_p": 0.95,
            "max_tokens": 4096,
            "model": "gpt-4",
            "extra_field": "ignored"
        });
        let overrides = execution_option_overrides(&params);
        assert_eq!(overrides.len(), 4);
        assert!(!overrides.contains_key("extra_field"));
    }

    #[test]
    fn test_run_id_generation_increasing() {
        let id1 = next_workflow_run_id();
        let id2 = next_workflow_run_id();
        assert_ne!(id1, id2, "each run id should be unique");
    }

    #[test]
    fn test_complete_workflow_run_unknown_id() {
        // Should not panic for unknown run_id
        complete_workflow_run("unknown-id", "succeeded", None, vec![]);
    }
}
