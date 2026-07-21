use super::*;

pub(crate) fn infer_task_type(method: &str, params: &Option<Value>) -> TaskType {
    let text_hint = params
        .as_ref()
        .and_then(|value| {
            value
                .get("task")
                .and_then(Value::as_str)
                .map(|s| s.to_ascii_lowercase())
                .or_else(|| {
                    value
                        .get("action")
                        .and_then(Value::as_str)
                        .map(|s| s.to_ascii_lowercase())
                })
        })
        .unwrap_or_default();

    if text_hint.contains("security") || text_hint.contains("vuln") {
        return TaskType::SecurityPatch;
    }
    if matches!(method, "workflow.generate" | "task.plan") {
        return TaskType::FeatureAdd;
    }
    if matches!(method, "task.execute" | "workflow.execute") {
        return TaskType::BugFix;
    }
    if matches!(method, "mcp.tools.call" | "workflow.consult") {
        return TaskType::Refactor;
    }
    TaskType::Other
}

pub(crate) fn infer_file_count(params: &Option<Value>) -> usize {
    params
        .as_ref()
        .and_then(|value| {
            value
                .get("changed_files")
                .and_then(Value::as_array)
                .map(|items| items.len())
                .or_else(|| {
                    value
                        .get("files")
                        .and_then(Value::as_array)
                        .map(|items| items.len())
                })
        })
        .filter(|&count| count > 0)
        .unwrap_or(1)
}

pub(crate) fn infer_risk_score(method: &str, task_type: &TaskType) -> f64 {
    if *task_type == TaskType::SecurityPatch {
        return 0.9;
    }
    // Unknown/novel methods carry elevated risk (fail-closed principle)
    if !crate::protocol::acp_methods::AcpMethodNames::is_known(method) {
        // Only apply elevated baseline for non-infrastructure, non-MCP methods
        let is_mcp = method.starts_with("mcp.");
        let is_infra = matches!(
            method,
            "health" | "metrics" | "shutdown" | "chat" | "phase.status"
        );
        if !is_mcp && !is_infra {
            return 0.55;
        }
    }
    match method {
        "mcp.tools.call" => 0.7,
        "task.execute" | "workflow.execute" => 0.6,
        "workflow.generate" => 0.5,
        _ => 0.3,
    }
}

pub(crate) fn classify_request_error_kind(error: &anyhow::Error) -> &'static str {
    let message = error.to_string().to_ascii_lowercase();
    if message.contains("pua") {
        return "PuaViolation";
    }
    if message.contains("budget denied") || message.contains("budget exceeded") {
        return "BudgetExceeded";
    }
    if message.contains("hardening policy denied") || message.contains("sandbox") {
        return "SandboxBlocked";
    }
    "GeneralError"
}

pub(super) fn infer_error_contract_kind(
    code: i32,
    message: &str,
    explicit: Option<&str>,
) -> String {
    if let Some(kind) = explicit {
        if !kind.trim().is_empty() {
            return kind.to_string();
        }
    }

    let lower = message.to_ascii_lowercase();
    if lower.contains("pua") {
        return "PuaViolation".to_string();
    }
    if lower.contains("budget denied") || lower.contains("budget exceeded") {
        return "BudgetExceeded".to_string();
    }
    if lower.contains("hardening policy denied") || lower.contains("sandbox") {
        return "SandboxBlocked".to_string();
    }
    if code == -32601 {
        return "MethodNotFound".to_string();
    }
    if code == -32602 {
        return "InvalidParams".to_string();
    }
    if code == -32003 {
        return "AuthRequired".to_string();
    }
    if code == -32029 || lower.contains("rate limited") || lower.contains("too many requests") {
        return "RateLimited".to_string();
    }
    if lower.contains("timeout") {
        return "UpstreamTimeout".to_string();
    }
    if code == -32603 {
        return "InternalError".to_string();
    }
    "GeneralError".to_string()
}

pub(crate) fn build_retry_policy_for_kind(kind: &str) -> Value {
    let normalized = kind.to_ascii_lowercase();
    if matches!(normalized.as_str(), "ratelimited" | "upstreamtimeout") {
        json!({
            "retryable": true,
            "strategy": "exponential_backoff",
            "base_delay_ms": 500,
            "max_delay_ms": 10_000,
            "max_retries": 3
        })
    } else {
        json!({
            "retryable": false,
            "strategy": "none",
            "base_delay_ms": 0,
            "max_delay_ms": 0,
            "max_retries": 0
        })
    }
}

pub(super) fn with_error_contract_data(
    code: i32,
    message: &str,
    data: Option<Value>,
) -> Option<Value> {
    let mut normalized = serde_json::Map::new();
    let mut explicit_kind: Option<String> = None;

    if let Some(data_value) = data {
        match data_value {
            Value::Object(existing) => {
                explicit_kind = existing
                    .get("kind")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                normalized.extend(existing);
            }
            other => {
                normalized.insert("raw_data".to_string(), other);
            }
        }
    }

    let kind = infer_error_contract_kind(code, message, explicit_kind.as_deref());
    if !normalized.contains_key("kind") {
        normalized.insert("kind".to_string(), Value::String(kind.clone()));
    }
    if !normalized.contains_key("contract_version") {
        normalized.insert(
            "contract_version".to_string(),
            Value::String("x8-error-contract-v1".to_string()),
        );
    }
    if !normalized.contains_key("code_class") {
        normalized.insert(
            "code_class".to_string(),
            Value::String(format!("jsonrpc:{}", code)),
        );
    }
    if !normalized.contains_key("retry") {
        normalized.insert("retry".to_string(), build_retry_policy_for_kind(&kind));
    }
    Some(Value::Object(normalized))
}

pub(crate) fn attach_request_dispatch_context(error: anyhow::Error, method: &str) -> anyhow::Error {
    let kind = classify_request_error_kind(&error);
    error.context(format!(
        "acp.handle_request.dispatch method={} kind={}",
        method, kind
    ))
}

pub(crate) fn resolve_platform_mode(params: &Value) -> &'static str {
    match params
        .get("platform_mode")
        .and_then(Value::as_str)
        .unwrap_or("phase_compat")
        .to_ascii_lowercase()
        .as_str()
    {
        "universal" => "universal",
        _ => "phase_compat",
    }
}

pub(crate) fn map_phase_to_capability_profile(phase: Option<&str>, method: &str) -> Value {
    let phase_value = phase.unwrap_or("default");
    let inferred_capability = if method.contains("plan") || method.contains("generate") {
        "planning"
    } else if method.contains("research") || method.contains("consult") {
        "analysis"
    } else if method.contains("execute") {
        "execution"
    } else {
        "governance"
    };

    json!({
        "phase": phase_value,
        "capability": inferred_capability,
        "mapping_status": "mapped",
        "mapping_version": "blue23-phase-compat-v1",
    })
}

pub(crate) fn build_capability_profile(method: &str, task: &str, params: &Value) -> Value {
    let platform_mode = resolve_platform_mode(params);
    let phase = params.get("phase").and_then(Value::as_str);
    let run_mode = params
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("assisted");

    let intent = if method.contains("plan") || method.contains("generate") {
        "plan"
    } else if method.contains("research") {
        "research"
    } else if method.contains("consult") {
        "consult"
    } else if method.contains("execute") {
        "execute"
    } else {
        "analyze"
    };

    json!({
        "schema_version": "blue23-capability-profile-v1",
        "platform_mode": platform_mode,
        "intent": intent,
        "task": task,
        "constraints": {
            "requirement_confirmed": params
                .get("requirement_confirmed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            "run_mode": run_mode,
        },
        "gates": {
            "requirement_gate": "required",
            "review_gate": "governed",
        },
        "execution_cycle": {
            "model": "universal_cycle",
            "supports_auto_repair": true,
        },
        "toolchain": {
            "profile": "governed_runtime",
            "fallback_enabled": true,
        },
        "evidence": {
            "traceable": true,
            "artifact_backed": true,
        },
        "phase_compat": map_phase_to_capability_profile(phase, method),
    })
}

pub(super) fn build_sandbox_profile(
    method: &str,
    params: &Value,
    capability_profile: &Value,
) -> Value {
    let explicit = params.get("sandbox_profile").and_then(Value::as_str);
    let method_lower = method.to_ascii_lowercase();
    let selected = explicit.unwrap_or_else(|| {
        if method_lower.contains("execute") {
            "workspace_exec"
        } else if method_lower.contains("plan")
            || method_lower.contains("research")
            || method_lower.contains("consult")
            || method_lower.contains("generate")
        {
            "read_only"
        } else {
            "workspace_write"
        }
    });

    json!({
        "selected": selected,
        "reason": format!("risk-adaptive selection for {}", method),
        "allowed_profiles": ["read_only", "workspace_write", "workspace_exec", "elevated"],
        "from_capability_profile": capability_profile.get("schema_version").cloned().unwrap_or(Value::Null),
    })
}

pub(super) fn build_approval_checkpoint(
    method: &str,
    change_bundle: &Value,
    params: &Value,
) -> Value {
    let risk_level = change_bundle
        .get("risk")
        .and_then(|risk| risk.get("level"))
        .and_then(Value::as_str)
        .unwrap_or("low");
    let explicit_force = params
        .get("approval_required")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let required = explicit_force || matches!(risk_level, "high" | "critical");
    let checkpoint_id = format!(
        "approval-{}-{}",
        method.replace('.', "-"),
        crate::acp::prelude::now_ts_ms()
    );
    let resume_token = format!(
        "resume-{}-{}",
        method.replace('.', "-"),
        crate::acp::prelude::now_ts_ms()
    );
    json!({
        "required": required,
        "checkpoint_id": checkpoint_id,
        "resume_token": resume_token,
        "state": if required { "pending" } else { "not_required" },
        "reason": if required {
            format!("{} risk operation requires approval", risk_level)
        } else {
            "approval not required for current risk profile".to_string()
        },
        "risk_level": risk_level,
        "required_evidence": ["change_bundle", "gates", "trace_ref"],
        "approver_scope": if required { "human_reviewer" } else { "none" },
        "expires_at": crate::acp::prelude::now_ts() + 3600,
        "method": method,
    })
}

pub(crate) fn detect_git_branch() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("rev-parse")
        .arg("--abbrev-ref")
        .arg("HEAD")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8(output.stdout).ok()?;
    let branch = branch.trim();
    if branch.is_empty() {
        None
    } else {
        Some(branch.to_string())
    }
}

pub(super) fn build_repo_native_context(
    method: &str,
    params: &Value,
    change_bundle: &Value,
) -> Value {
    let cwd = std::env::current_dir()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| ".".to_string());
    let branch = params
        .get("branch")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(detect_git_branch)
        .unwrap_or_else(|| "unknown".to_string());
    let worktree = params
        .get("worktree")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| cwd.clone());
    let patch_set_count = change_bundle
        .get("files")
        .and_then(Value::as_array)
        .map(|files| files.len())
        .unwrap_or(0);

    json!({
        "repository": cwd,
        "branch": branch,
        "worktree": worktree,
        "method": method,
        "patch_set": {
            "count": patch_set_count,
            "source": "change_bundle.files",
        },
        "commit_bundle": change_bundle.get("commit_bundle").cloned().unwrap_or(Value::Null),
        "pr_bundle": change_bundle.get("pr_bundle").cloned().unwrap_or(Value::Null),
    })
}

pub(crate) fn build_universal_governance_profile(
    method: &str,
    capability_profile: &Value,
    params: &Value,
) -> Value {
    let intent = capability_profile
        .get("intent")
        .and_then(Value::as_str)
        .unwrap_or("analyze");
    let risk_band = if method.contains("execute") {
        "high"
    } else if intent == "consult" || intent == "research" {
        "medium"
    } else {
        "low"
    };
    let max_iterations = params
        .get("auto_repair_max_iterations")
        .and_then(Value::as_u64)
        .unwrap_or(2);
    let token_budget = params
        .get("budget_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(if risk_band == "high" { 6000 } else { 3000 });
    let time_budget_seconds = params
        .get("budget_time_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(if risk_band == "high" { 240 } else { 120 });

    json!({
        "schema_version": "blue23-governance-profile-v1",
        "risk_band": risk_band,
        "budget": {
            "token_budget": token_budget,
            "time_budget_seconds": time_budget_seconds,
            "max_iterations": max_iterations,
        },
        "policy_source": "capability_profile",
        "phase_compat_enabled": capability_profile
            .get("platform_mode")
            .and_then(Value::as_str)
            .unwrap_or("phase_compat")
            == "phase_compat",
    })
}

pub(crate) fn build_token_economy(
    method: &str,
    params: &Value,
    governance_profile: &Value,
    execution_cycle: &Value,
) -> Value {
    let token_budget = governance_profile
        .get("budget")
        .and_then(|budget| budget.get("token_budget"))
        .and_then(Value::as_u64)
        .unwrap_or(3000);
    let repair_iterations = execution_cycle
        .get("history_summary")
        .and_then(|summary| summary.get("repair_iterations"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let max_rounds = params
        .get("token_rounds")
        .and_then(Value::as_u64)
        .unwrap_or(1 + repair_iterations)
        .max(1);
    let reserve_ratio = if method.contains("execute") {
        0.35
    } else {
        0.2
    };

    // Dynamic task complexity: explicit param wins, else infer from task text length.
    let task_len = params
        .get("task")
        .and_then(Value::as_str)
        .map(str::len)
        .unwrap_or(0);
    let task_complexity =
        params
            .get("complexity")
            .and_then(Value::as_str)
            .unwrap_or(if task_len > 300 {
                "high"
            } else if task_len > 80 {
                "medium"
            } else {
                "low"
            });

    // Compression level escalates with repair history and task complexity.
    let compression_level = if repair_iterations >= 3 || task_complexity == "high" {
        "aggressive"
    } else if repair_iterations >= 1 || task_complexity == "medium" {
        "moderate"
    } else {
        "light"
    };

    let expected_saving_rate = match compression_level {
        "aggressive" => 0.38,
        "moderate" => 0.24,
        _ => {
            if method.contains("execute") {
                0.18
            } else {
                0.12
            }
        }
    };

    let per_round_budget = token_budget / max_rounds.max(1);
    let cumulative_saving_estimate = (token_budget as f64 * expected_saving_rate) as u64;

    json!({
        "schema_version": "blue24-token-economy-v2",
        "budget": {
            "request_token_budget": token_budget,
            "per_round_budget": per_round_budget,
            "reserve_ratio": reserve_ratio,
            "compression_enabled": true,
            "cache_reuse_enabled": true,
        },
        "compression": {
            "level": compression_level,
            "task_complexity": task_complexity,
            "repair_iterations_observed": repair_iterations,
        },
        "multi_round_strategy": {
            "enabled": true,
            "max_rounds": max_rounds,
            "summarize_between_rounds": true,
            "early_stop_gate": "requirement_and_quality",
            "cross_round_kv_cache": true,
        },
        "optimization": {
            "expected_saving_rate": expected_saving_rate,
            "cumulative_saving_estimate_tokens": cumulative_saving_estimate,
            "cost_alert_threshold": 0.85,
            "status": "governed",
        }
    })
}

pub(crate) fn build_gate_matrix(
    requirement_gate: Value,
    gate_status: &str,
    status2: &str,
    status3: &str,
    check: Option<(&str, &str)>,
) -> Value {
    let mut gates = json!({
        "requirement": requirement_gate,
        "gate": gate_status,
        "status2": status2,
        "status3": status3,
    });
    if let Some((check_name, check_status)) = check {
        gates[check_name] = Value::String(check_status.to_string());
    }
    gates
}

pub(crate) fn build_change_bundle(
    kind: &str,
    description: String,
    level: &str,
    status: &str,
    message: String,
    files: Vec<String>,
) -> Value {
    let file_change_summary = files
        .iter()
        .map(|path| {
            let lower = path.to_ascii_lowercase();
            let file_role = if lower.contains("artifact") || lower.contains("latest-") {
                "artifact"
            } else if lower.ends_with(".rs")
                || lower.ends_with(".ts")
                || lower.ends_with(".tsx")
                || lower.ends_with(".js")
                || lower.ends_with(".jsx")
            {
                "source"
            } else if lower.ends_with(".md") || lower.ends_with(".json") || lower.ends_with(".toml")
            {
                "metadata"
            } else {
                "unknown"
            };

            json!({
                "path": path,
                "role": file_role,
                "change_type": "updated",
            })
        })
        .collect::<Vec<_>>();

    let impact_surface = file_change_summary
        .iter()
        .filter_map(|entry| {
            entry
                .get("role")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let commit_scope = if kind.contains("execution") {
        "workflow"
    } else if kind.contains("analysis") {
        "research"
    } else {
        "runtime"
    };

    json!({
        "kind": kind,
        "description": description,
        "level": level,
        "status": status,
        "message": message,
        "files": files,
        "file_change_summary": file_change_summary,
        "risk": {
            "level": level,
            "impact_surface": impact_surface,
            "summary": format!("{} change bundle with {} linked files", kind, files.len()),
        },
        "gate_results": {
            "overall": status,
            "tests": status,
            "verification_mode": "runtime_governed",
        },
        "rollback_recommendation": {
            "recommended": status != "passed",
            "instructions": [
                "Revert files listed in file_change_summary.",
                "Re-run workflow/task execution after fixing root cause.",
            ],
        },
        "commit_suggestion": {
            "message": message,
            "scope": commit_scope,
            "style": "conventional",
        },
        "rollback": {
            "enabled": false,
            "strategy": "none"
        },
        "commit": {
            "message": message,
            "timestamp": crate::acp::prelude::now_ts(),
            "author": "workflow"
        },
        "commit_bundle": {
            "message": message,
            "scope": commit_scope,
            "ready": status == "passed",
            "files_count": files.len(),
        },
        "pr_bundle": {
            "title": message,
            "summary": description,
            "risk_level": level,
            "ready": status == "passed",
        },
        "test_coverage": {
            "overall_coverage": 0.0,
            "affected_areas": [],
            "test_plan": "standard"
        }
    })
}

pub fn build_learning_profile(method: &str, task: &str, params: &Value) -> Value {
    let learning_mode = params
        .get("learning_mode")
        .and_then(Value::as_str)
        .unwrap_or("adaptive");
    let replay_enabled = params
        .get("learning_replay_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let memory_scope = params
        .get("memory_scope")
        .and_then(Value::as_str)
        .unwrap_or("task_and_repo");

    let task_len = task.len();
    let cognitive_load = if task_len > 300 || method.contains("execute") {
        "high"
    } else if task_len > 80 || method.contains("plan") {
        "medium"
    } else {
        "low"
    };

    let current_strategy = if method.contains("research") || method.contains("consult") {
        "exploration"
    } else if method.contains("execute") {
        "execution"
    } else if method.contains("plan") || method.contains("generate") {
        "planning"
    } else {
        "reflection"
    };

    let repair_rounds = params
        .get("repair_iterations")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let adaptation_signal = if repair_rounds >= 2 {
        "needs_adjustment"
    } else if repair_rounds == 1 {
        "adjusting"
    } else {
        "stable"
    };

    json!({
        "schema_version": "blue24-learning-profile-v2",
        "learning_mode": learning_mode,
        "memory_scope": memory_scope,
        "cognition": {
            "self_reflection": true,
            "strategy_adaptation": true,
            "confidence_tracking": true,
            "phase": if method.contains("execute") { "execution" } else { "planning" },
        },
        "meta_cognition": {
            "reflection_depth": if method.contains("execute") { "deep" } else { "standard" },
            "strategy_evaluation": {
                "current_strategy": current_strategy,
                "adaptation_signal": adaptation_signal,
                "bias_correction_active": true,
                "repair_rounds_observed": repair_rounds,
            },
            "self_improvement": {
                "bottleneck_awareness": true,
                "correction_loop_active": true,
                "hypothesis_testing": method.contains("research") || method.contains("consult"),
            },
            "cognitive_load_estimate": cognitive_load,
            "awareness_level": "operational",
        },
        "learning_loop": {
            "replay_enabled": replay_enabled,
            "distillation_enabled": true,
            "feedback_to_strategy": true,
            "cross_round_compression": repair_rounds > 0,
        },
        "task_ref": {
            "method": method,
            "task": task,
        }
    })
}

pub fn build_knowledge_refinement_profile(
    method: &str,
    task: &str,
    params: &Value,
    _learning_profile: &Value,
) -> Value {
    let distill_scope = params
        .get("distill_scope")
        .and_then(Value::as_str)
        .unwrap_or("task_repo_runtime");
    let evolution_mode = params
        .get("evolution_mode")
        .and_then(Value::as_str)
        .unwrap_or("continuous");

    let repair_rounds = params
        .get("repair_iterations")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let confidence: f64 = if method.contains("execute") {
        match repair_rounds {
            0 => 0.82,
            1 => 0.78,
            2 => 0.73,
            _ => 0.68,
        }
    } else {
        match repair_rounds {
            0 => 0.75,
            1 => 0.72,
            _ => 0.68,
        }
    };

    let staleness_risk = if repair_rounds >= 3 {
        "elevated"
    } else if repair_rounds >= 1 {
        "moderate"
    } else {
        "low"
    };

    json!({
        "schema_version": "blue24-knowledge-refinement-v2",
        "distillation": {
            "enabled": true,
            "scope": distill_scope,
            "extract_strategy": "evidence_weighted",
            "writeback_targets": ["learning.summary", "knowledge.distill"],
        },
        "cross_round": {
            "distillation_enabled": repair_rounds > 0,
            "stale_knowledge_detection": true,
            "staleness_risk": staleness_risk,
            "writeback_on_convergence": true,
            "rounds_since_update": repair_rounds,
        },
        "self_evolution": {
            "mode": evolution_mode,
            "adaptive_routing": true,
            "policy_feedback_loop": true,
            "confidence": confidence,
        },
        "knowledge_quality": {
            "source_traceable": true,
            "dedup_enabled": true,
            "guardrail_enforced": true,
        },
        "task_ref": {
            "method": method,
            "task": task,
        }
    })
}

pub(crate) fn build_trace_ref(
    method: &str,
    request_id: Option<&Value>,
    artifact_path: Option<&str>,
) -> Value {
    json!({
        "method": method,
        "request_id": request_id.cloned().unwrap_or_default(),
        "artifact_path": artifact_path.unwrap_or_default(),
    })
}

/// Universal lazy-load platform profile injection: called by send_result for every response.
/// Injects `learning_profile` and `knowledge_refinement` if the handler did not already set them.
/// Handlers that explicitly build these objects retain their richer, task-specific versions.
pub(crate) fn inject_platform_profiles_if_absent(mut result: Value, method: &str) -> Value {
    // Only inject into object responses (not notifications / empty)
    let Some(obj) = result.as_object_mut() else {
        return result;
    };

    // ── Well-known platform metadata endpoints ────────────────────────────
    // These endpoints get full platform metadata (available modes, default mode,
    // capabilities list) alongside the standard governance profiles.
    let is_platform_metadata = matches!(method, "initialize" | "session/new" | "tools/list");

    if is_platform_metadata {
        if !obj.contains_key("platform_metadata") {
            let available_modes = vec![
                json!({"id": "safeguard", "name": "SafeGuard / 安全", "description": "Safety-first — escalation on high-risk operations (default)"}),
                json!({"id": "ask", "name": "Ask / 对话", "description": "Q&A assistant — general questions"}),
                json!({"id": "plan", "name": "Plan / 计划", "description": "Planning mode — structured task breakdown"}),
                json!({"id": "edit", "name": "Edit / 编辑", "description": "Edit/review mode — code changes"}),
                json!({"id": "full_auto", "name": "Full Auto / 全自动", "description": "Fully autonomous — agent runs without user confirmation"}),
            ];
            let platform_md = json!({
                "schema_version": "blue24-platform-universal-v1",
                "platform": "go-on",
                "default_mode": "safeguard",
                "available_modes": available_modes,
                "capabilities": [
                    "core.session.lifecycle",
                    "core.session.prompt",
                    "core.tools.list",
                    "core.tools.call",
                    "core.terminals",
                    "governance.profiles",
                    "governance.audit",
                    "security.sandbox",
                ],
            });
            obj.insert("platform_metadata".to_string(), platform_md);
        }
        // Also inject standard profiles for these endpoints
        let empty_params = json!({});
        if !obj.contains_key("learning_profile") {
            obj.insert(
                "learning_profile".to_string(),
                build_learning_profile(method, "", &empty_params),
            );
        }
        if !obj.contains_key("knowledge_refinement") {
            let lp = obj
                .get("learning_profile")
                .cloned()
                .unwrap_or_else(|| json!({}));
            obj.insert(
                "knowledge_refinement".to_string(),
                build_knowledge_refinement_profile(method, "", &empty_params, &lp),
            );
        }
        return result;
    }

    // Infrastructure endpoints (metrics, health, shutdown, protocol handshakes, trace, debug)
    // get a lightweight platform_context marker only — they carry no AI task semantics.
    let is_infrastructure = matches!(
        method,
        "metrics"
            | "metrics.get"
            | "metrics.prometheus"
            | "metrics.reset"
            | "debug_panel.get"
            | "debug.panel.get"
            | "trace.get"
            | "trace.metrics"
            | "shutdown"
            | "health"
            | "runtime.health"
            | "health.probes"
            | "session/load"
            | "session/prompt"
            | "session/cancel"
            | "session/list"
            | "session/set_mode"
            | "session/set_config_option"
            | "authenticate"
            | "logout"
            | "$/cancel_request"
            | "mcp.initialize"
            | "mcp.ping"
            | "mcp.resources.list"
            | "mcp.resources.read"
            | "mcp.resources.subscribe"
            | "mcp.logging.setLevel"
            | "mcp.completion.complete"
            | "mcp.sampling.createMessage"
            | "mcp.tools.list"
            | "mcp.tools.call"
            | "tools/call"
            | "resources/list"
            | "resources/read"
            | "agents/list"
            | "models/list"
            | "skill.import"
            | "skill.enable"
            | "skill.disable"
            | "skill.list_imported"
            | "skill.remove"
            | "acp.error"
            | "chat"
            | "openai.chat.completions"
            | "responses.api"
            | "mcp.parse_error"
            | "mcp.unknown_method"
            | "phase"
            | "phase.status"
    );
    if is_infrastructure {
        if !obj.contains_key("platform_context") {
            obj.insert(
                "platform_context".to_string(),
                json!({
                    "schema_version": "blue24-platform-universal-v1",
                    "platform": "go-on",
                    "ai_profiles_active": true,
                    "method": method,
                    "profile_class": "infrastructure",
                }),
            );
        }
        return result;
    }
    // All semantic endpoints get full learning_profile + knowledge_refinement if not already present.
    let empty_params = json!({});
    if !obj.contains_key("learning_profile") {
        obj.insert(
            "learning_profile".to_string(),
            build_learning_profile(method, "", &empty_params),
        );
    }
    if !obj.contains_key("knowledge_refinement") {
        let lp = obj
            .get("learning_profile")
            .cloned()
            .unwrap_or_else(|| json!({}));
        obj.insert(
            "knowledge_refinement".to_string(),
            build_knowledge_refinement_profile(method, "", &empty_params, &lp),
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── infer_task_type ───────────────────────────────────────────────

    #[test]
    fn infer_task_type_security_text() {
        let params = Some(json!({"task": "find vulnerabilities in auth"}));
        assert_eq!(infer_task_type("chat", &params), TaskType::SecurityPatch);
    }

    #[test]
    fn infer_task_type_workflow_generate() {
        assert_eq!(
            infer_task_type("workflow.generate", &None),
            TaskType::FeatureAdd
        );
    }

    #[test]
    fn infer_task_type_task_execute() {
        assert_eq!(infer_task_type("task.execute", &None), TaskType::BugFix);
    }

    #[test]
    fn infer_task_type_mcp_tools_call() {
        assert_eq!(infer_task_type("mcp.tools.call", &None), TaskType::Refactor);
    }

    #[test]
    fn infer_task_type_unknown_method() {
        assert_eq!(infer_task_type("unknown.method", &None), TaskType::Other);
    }

    // ── infer_file_count ──────────────────────────────────────────────

    #[test]
    fn infer_file_count_from_changed_files() {
        let params = Some(json!({"changed_files": ["a.rs", "b.rs"]}));
        assert_eq!(infer_file_count(&params), 2);
    }

    #[test]
    fn infer_file_count_from_files() {
        let params = Some(json!({"files": ["x.rs"]}));
        assert_eq!(infer_file_count(&params), 1);
    }

    #[test]
    fn infer_file_count_defaults_to_one() {
        assert_eq!(infer_file_count(&None), 1);
    }

    #[test]
    fn infer_file_count_empty_array_defaults_to_one() {
        let params = Some(json!({"changed_files": []}));
        assert_eq!(infer_file_count(&params), 1);
    }

    // ── infer_risk_score ──────────────────────────────────────────────

    #[test]
    fn infer_risk_score_security_patch_is_high() {
        assert!((infer_risk_score("chat", &TaskType::SecurityPatch) - 0.9).abs() < 1e-6);
    }

    #[test]
    fn infer_risk_score_mcp_tools_call() {
        assert!((infer_risk_score("mcp.tools.call", &TaskType::Refactor) - 0.7).abs() < 1e-6);
    }

    #[test]
    fn infer_risk_score_default_low() {
        assert!((infer_risk_score("session/new", &TaskType::Other) - 0.3).abs() < 1e-6);
    }

    // ── classify_request_error_kind ───────────────────────────────────

    #[test]
    fn classify_request_error_kind_pua() {
        let err = anyhow::anyhow!("PUA violation: blocked");
        assert_eq!(classify_request_error_kind(&err), "PuaViolation");
    }

    #[test]
    fn classify_request_error_kind_budget() {
        let err = anyhow::anyhow!("budget denied tool 'x' in scope 'y': budget exceeded");
        assert_eq!(classify_request_error_kind(&err), "BudgetExceeded");
    }

    #[test]
    fn classify_request_error_kind_sandbox() {
        let err = anyhow::anyhow!("hardening policy denied tool: sandbox strict");
        assert_eq!(classify_request_error_kind(&err), "SandboxBlocked");
    }

    #[test]
    fn classify_request_error_kind_general() {
        let err = anyhow::anyhow!("something went wrong");
        assert_eq!(classify_request_error_kind(&err), "GeneralError");
    }

    // ── infer_error_contract_kind ──────────────────────────────────────

    #[test]
    fn infer_error_contract_kind_explicit_overrides() {
        assert_eq!(
            infer_error_contract_kind(0, "msg", Some("CustomKind")),
            "CustomKind"
        );
    }

    #[test]
    fn infer_error_contract_kind_method_not_found() {
        assert_eq!(
            infer_error_contract_kind(-32601, "not found", None),
            "MethodNotFound"
        );
    }

    #[test]
    fn infer_error_contract_kind_invalid_params() {
        assert_eq!(
            infer_error_contract_kind(-32602, "invalid", None),
            "InvalidParams"
        );
    }

    #[test]
    fn infer_error_contract_kind_rate_limited_code() {
        assert_eq!(
            infer_error_contract_kind(-32029, "too many", None),
            "RateLimited"
        );
    }

    #[test]
    fn infer_error_contract_kind_empty_explicit_falls_through() {
        assert_eq!(
            infer_error_contract_kind(-32603, "internal", Some("")),
            "InternalError"
        );
    }

    // ── build_retry_policy_for_kind ────────────────────────────────────

    #[test]
    fn build_retry_policy_for_kind_rate_limited_is_retryable() {
        let policy = build_retry_policy_for_kind("RateLimited");
        assert_eq!(policy["retryable"], true);
        assert_eq!(policy["max_retries"], 3);
    }

    #[test]
    fn build_retry_policy_for_kind_general_not_retryable() {
        let policy = build_retry_policy_for_kind("GeneralError");
        assert_eq!(policy["retryable"], false);
        assert_eq!(policy["max_retries"], 0);
    }

    // ── resolve_platform_mode ──────────────────────────────────────────

    #[test]
    fn resolve_platform_mode_universal() {
        let params = json!({"platform_mode": "universal"});
        assert_eq!(resolve_platform_mode(&params), "universal");
    }

    #[test]
    fn resolve_platform_mode_defaults_to_phase_compat() {
        let params = json!({});
        assert_eq!(resolve_platform_mode(&params), "phase_compat");
    }

    // ── build_capability_profile ───────────────────────────────────────

    #[test]
    fn build_capability_profile_has_schema_version() {
        let profile = build_capability_profile("chat", "hello", &json!({}));
        assert_eq!(profile["schema_version"], "blue23-capability-profile-v1");
        assert_eq!(profile["intent"], "analyze");
    }

    #[test]
    fn build_capability_profile_execute_intent() {
        let profile = build_capability_profile("workflow.execute", "task", &json!({}));
        assert_eq!(profile["intent"], "execute");
    }

    // ── build_sandbox_profile ──────────────────────────────────────────

    #[test]
    fn build_sandbox_profile_execute_uses_workspace_exec() {
        let cap = build_capability_profile("execute", "x", &json!({}));
        let sandbox = build_sandbox_profile("execute", &json!({}), &cap);
        assert_eq!(sandbox["selected"], "workspace_exec");
    }

    #[test]
    fn build_sandbox_profile_research_uses_read_only() {
        let cap = build_capability_profile("research", "x", &json!({}));
        let sandbox = build_sandbox_profile("research", &json!({}), &cap);
        assert_eq!(sandbox["selected"], "read_only");
    }

    #[test]
    fn build_sandbox_profile_explicit_overrides() {
        let cap = build_capability_profile("chat", "x", &json!({}));
        let sandbox = build_sandbox_profile("chat", &json!({"sandbox_profile": "elevated"}), &cap);
        assert_eq!(sandbox["selected"], "elevated");
    }

    // ── build_approval_checkpoint ──────────────────────────────────────

    #[test]
    fn build_approval_checkpoint_critical_risk_requires_approval() {
        let bundle = json!({"risk": {"level": "critical"}});
        let checkpoint = build_approval_checkpoint("workflow.execute", &bundle, &json!({}));
        assert_eq!(checkpoint["required"], true);
        assert_eq!(checkpoint["state"], "pending");
    }

    #[test]
    fn build_approval_checkpoint_low_risk_no_approval() {
        let bundle = json!({"risk": {"level": "low"}});
        let checkpoint = build_approval_checkpoint("chat", &bundle, &json!({}));
        assert_eq!(checkpoint["required"], false);
        assert_eq!(checkpoint["state"], "not_required");
    }

    #[test]
    fn build_approval_checkpoint_explicit_force_required() {
        let bundle = json!({"risk": {"level": "low"}});
        let checkpoint =
            build_approval_checkpoint("chat", &bundle, &json!({"approval_required": true}));
        assert_eq!(checkpoint["required"], true);
    }

    // ── build_change_bundle ────────────────────────────────────────────

    #[test]
    fn build_change_bundle_includes_file_roles() {
        let bundle = build_change_bundle(
            "execution",
            "fix bug".to_string(),
            "medium",
            "passed",
            "commit msg".to_string(),
            vec![
                "src/main.rs".to_string(),
                "README.md".to_string(),
                "latest-output.json".to_string(),
            ],
        );
        assert_eq!(bundle["kind"], "execution");
        assert_eq!(bundle["risk"]["level"], "medium");
        assert_eq!(bundle["status"], "passed");
        assert!(!bundle["rollback_recommendation"]["recommended"]
            .as_bool()
            .expect("rollback_recommendation.recommended should be a bool"));
    }

    #[test]
    fn build_change_bundle_failed_status_recommends_rollback() {
        let bundle = build_change_bundle(
            "analysis",
            "failed".to_string(),
            "high",
            "failed",
            "error".to_string(),
            vec![],
        );
        assert_eq!(bundle["status"], "failed");
        assert!(bundle["rollback_recommendation"]["recommended"]
            .as_bool()
            .expect("rollback_recommendation.recommended should be a bool"));
    }

    #[test]
    fn build_change_bundle_empty_files() {
        let bundle = build_change_bundle(
            "analysis",
            "description".to_string(),
            "low",
            "passed",
            "msg".to_string(),
            vec![],
        );
        assert!(bundle["files"]
            .as_array()
            .expect("files should be an array")
            .is_empty());
    }

    // ── build_universal_governance_profile ─────────────────────────────

    #[test]
    fn build_universal_governance_profile_execute_high_risk() {
        let cap = build_capability_profile("workflow.execute", "t", &json!({}));
        let profile = build_universal_governance_profile("workflow.execute", &cap, &json!({}));
        assert_eq!(profile["risk_band"], "high");
        assert_eq!(profile["budget"]["token_budget"], 6000);
    }

    #[test]
    fn build_universal_governance_profile_low_risk_default_params() {
        let cap = build_capability_profile("chat", "t", &json!({}));
        let profile = build_universal_governance_profile("chat", &cap, &json!({}));
        assert_eq!(profile["risk_band"], "low");
        assert_eq!(profile["budget"]["token_budget"], 3000);
    }

    // ── map_phase_to_capability_profile ────────────────────────────────

    #[test]
    fn map_phase_to_capability_profile_planning() {
        let profile = map_phase_to_capability_profile(Some("coding"), "workflow.generate");
        assert_eq!(profile["capability"], "planning");
    }

    #[test]
    fn map_phase_to_capability_profile_execution() {
        let profile = map_phase_to_capability_profile(Some("coding"), "task.execute");
        assert_eq!(profile["capability"], "execution");
    }

    #[test]
    fn map_phase_to_capability_profile_default_to_governance() {
        let profile = map_phase_to_capability_profile(None, "chat");
        assert_eq!(profile["capability"], "governance");
    }

    // ── build_gate_matrix ─────────────────────────────────────────────

    #[test]
    fn build_gate_matrix_includes_check() {
        let req_gate = json!({"required": true});
        let gates = build_gate_matrix(req_gate, "open", "pass", "fail", Some(("custom", "ok")));
        assert_eq!(gates["gate"], "open");
        assert_eq!(gates["custom"], "ok");
    }

    #[test]
    fn build_gate_matrix_no_check() {
        let req_gate = json!({"required": false});
        let gates = build_gate_matrix(req_gate, "closed", "pass", "fail", None);
        assert_eq!(gates["requirement"]["required"], false);
        assert!(gates.get("custom").is_none());
    }
}
