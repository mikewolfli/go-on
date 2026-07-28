//! Workflow run tracking module.
//! Contains `WorkflowRunRecord`, lifecycle management, and handler functions
//! for listing, getting, transitioning, cancelling, pausing, and resuming workflow runs.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex as StdMutex, OnceLock};

use anyhow::Result;
use serde::Serialize;
use serde_json::{json, Value};
use tracing::warn;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WorkflowRunRecord {
    pub(super) run_id: String,
    pub(super) source_method: String,
    pub(super) task: String,
    pub(super) status: String,
    pub(super) phase: String,
    pub(super) created_at: i64,
    pub(super) started_at: i64,
    pub(super) ended_at: Option<i64>,
    pub(super) error: Option<String>,
    pub(super) artifacts: Vec<String>,
    pub(super) effective_options: Value,
}

static WORKFLOW_RUNS: OnceLock<StdMutex<Vec<WorkflowRunRecord>>> = OnceLock::new();
static WORKFLOW_RUN_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn workflow_runs() -> &'static StdMutex<Vec<WorkflowRunRecord>> {
    WORKFLOW_RUNS.get_or_init(|| StdMutex::new(Vec::new()))
}

fn next_workflow_run_id() -> String {
    let seq = WORKFLOW_RUN_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("run-{}-{}", crate::shared::timestamps::now_ts_ms(), seq)
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

pub(crate) fn start_workflow_run(
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

    let mut guard = workflow_runs().lock().unwrap_or_else(|poisoned| {
        warn!("Workflow runs lock poisoned in start_workflow_run, recovering");
        poisoned.into_inner()
    });
    guard.push(record.clone());
    const MAX_ENTRIES: usize = 10000;
    if guard.len() > MAX_ENTRIES {
        let overflow = guard.len() - MAX_ENTRIES;
        guard.drain(0..overflow);
    }

    record
}

pub(crate) fn complete_workflow_run(
    run_id: &str,
    status: &str,
    error: Option<String>,
    artifacts: Vec<String>,
) {
    let mut guard = workflow_runs().lock().unwrap_or_else(|poisoned| {
        warn!("Workflow runs lock poisoned in complete_workflow_run, recovering");
        poisoned.into_inner()
    });
    if let Some(item) = guard.iter_mut().find(|record| record.run_id == run_id) {
        item.status = status.to_string();
        item.error = error;
        item.artifacts = artifacts;
        item.ended_at = Some(crate::acp::prelude::now_ts());
    }
}

fn get_workflow_run_record(run_id: &str) -> Option<WorkflowRunRecord> {
    workflow_runs()
        .lock()
        .ok()
        .and_then(|guard| guard.iter().find(|record| record.run_id == run_id).cloned())
}

fn transition_workflow_run(run_id: &str, target_status: &str) -> Result<WorkflowRunRecord> {
    let mut guard = workflow_runs()
        .lock()
        .map_err(|err| anyhow::anyhow!("failed to lock workflow run store: {}", err))?;
    let record = guard
        .iter_mut()
        .find(|item| item.run_id == run_id)
        .ok_or_else(|| anyhow::anyhow!("workflow run '{}' not found", run_id))?;

    let allowed = match (record.status.as_str(), target_status) {
        ("queued", "cancelled") | ("queued", "running") => true,
        ("running", "paused") | ("running", "cancelled") | ("running", "succeeded") => true,
        ("paused", "running") | ("paused", "cancelled") => true,
        _ if record.status == target_status => true,
        _ => false,
    };

    if !allowed {
        anyhow::bail!(
            "invalid status transition: {} -> {}",
            record.status,
            target_status
        );
    }

    record.status = target_status.to_string();
    if matches!(target_status, "succeeded" | "failed" | "cancelled") {
        record.ended_at = Some(crate::acp::prelude::now_ts());
    }
    Ok(record.clone())
}

pub(crate) fn workflow_run_list_payload(params: &Value) -> Result<Value> {
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
        Many(HashSet<String>),
    }

    let status_filter = match params.get("status") {
        Some(Value::String(single)) => StatusFilter::One(single.clone()),
        Some(Value::Array(items)) => {
            let values = items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>();
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

    let (total, runs) = match workflow_runs().lock() {
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

    Ok(json!({
        "ok": true,
        "total": total,
        "offset": offset,
        "limit": limit,
        "runs": runs,
    }))
}

pub(crate) fn workflow_run_get_payload(params: &Value) -> Result<Value> {
    let run_id = params
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("run_id is required"))?;

    match get_workflow_run_record(run_id) {
        Some(run) => Ok(json!({"ok": true, "run": run})),
        None => Err(anyhow::anyhow!("workflow run '{}' not found", run_id)),
    }
}

pub(crate) fn workflow_run_transition_payload(
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
