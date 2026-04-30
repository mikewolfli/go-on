//! Requirement helper functions for ACP server
//!
//! This module provides utility functions for managing requirement contracts,
//! requirement gates, and clarification metrics.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::{
    orchestration::task_router::TaskRouter,
    reinforcement::{
        persist_governance_policy, persist_requirement_contract, ArtifactLedger,
        GovernancePolicyArtifact, RequirementContractArtifact,
    },
};

/// Requirement gate decision
#[derive(Debug, Clone)]
#[allow(dead_code)] // F-GAP-02 — preserved for legacy gate decision compatibility
pub struct RequirementGateDecision {
    /// Whether the gate is blocked
    pub blocked: bool,
    /// Reason for blocking (if any)
    pub reason: Option<String>,
    /// Missing requirement fields
    pub missing_fields: Vec<String>,
    /// Path to clarification artifact (if any)
    pub clarification_artifact_path: Option<PathBuf>,
    /// Path to governance artifact
    pub governance_artifact_path: PathBuf,
}

/// Unified gate facade decision used by workflow/task main paths.
#[derive(Debug, Clone)]
pub struct RequirementGateFacadeDecision {
    /// Stable kind for cross-surface consumers.
    pub kind: String,
    /// Whether the gate blocks execution.
    pub blocked: bool,
    /// Gate reason.
    pub reason: Option<String>,
    /// Missing requirement fields.
    pub missing_fields: Vec<String>,
    /// Suggested next step.
    pub next_step: Value,
    /// Path to clarification artifact (if any).
    pub clarification_artifact_path: Option<PathBuf>,
    /// Path to governance artifact.
    pub governance_artifact_path: PathBuf,
}

impl RequirementGateFacadeDecision {
    /// Build the canonical JSON payload for blocked gate responses.
    pub fn blocked_payload(&self) -> Value {
        json!({
            "kind": self.kind,
            "blocked": self.blocked,
            "reason": self.reason,
            "missing_fields": self.missing_fields,
            "next_step": self.next_step,
            "governance_artifact_path": self.governance_artifact_path.display().to_string(),
            "clarification_artifact_path": self
                .clarification_artifact_path
                .as_ref()
                .map(|path| path.display().to_string()),
        })
    }

    /// Build the canonical JSON payload for successful gate checks.
    pub fn success_payload(&self) -> Value {
        json!({
            "kind": self.kind,
            "blocked": self.blocked,
            "confirmed": !self.blocked,
            "missing_fields": self.missing_fields,
            "next_step": self.next_step,
            "governance_artifact_path": self.governance_artifact_path.display().to_string(),
            "clarification_artifact_path": self
                .clarification_artifact_path
                .as_ref()
                .map(|path| path.display().to_string()),
        })
    }
}

/// Learning clarification metrics
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // F-GAP-02 — reserved for clarification learning feedback loop
pub struct LearningClarificationMetrics {
    /// Number of clarification rounds
    pub rounds: u32,
    /// Quality score (0.0 to 1.0)
    pub quality_score: f64,
    /// Number of requirement changes
    pub requirement_change_count: u32,
}

/// Parse string list from JSON Value
#[allow(dead_code)] // F-GAP-02 — reserved for contract parsing utility
fn parse_string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str())
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// Get current timestamp
#[allow(dead_code)] // F-GAP-02 — reserved for timestamp contract generation
fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Parse requirement contract from request parameters
pub fn parse_requirement_contract_from_params(
    params: &Value,
    task: &str,
) -> Option<RequirementContractArtifact> {
    let contract = params.get("requirement_contract")?;
    let goal = contract
        .get("goal")
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    let scope = contract
        .get("scope")
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .unwrap_or_default();

    Some(RequirementContractArtifact {
        generated_at: now_ts(),
        task: task.to_string(),
        source: "request.params.requirement_contract".to_string(),
        goal,
        scope,
        non_goals: parse_string_list(contract.get("non_goals")),
        acceptance_criteria: parse_string_list(contract.get("acceptance_criteria")),
        constraints: parse_string_list(contract.get("constraints")),
        open_questions: parse_string_list(contract.get("open_questions")),
        ambiguity_score: contract
            .get("ambiguity_score")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .min(5) as u8,
        user_confirmed: contract
            .get("user_confirmed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

/// Create default requirement contract
pub fn default_requirement_contract(task: &str, source: &str) -> RequirementContractArtifact {
    RequirementContractArtifact {
        generated_at: now_ts(),
        task: task.to_string(),
        source: source.to_string(),
        goal: String::new(),
        scope: String::new(),
        non_goals: Vec::new(),
        acceptance_criteria: Vec::new(),
        constraints: Vec::new(),
        open_questions: Vec::new(),
        ambiguity_score: 0,
        user_confirmed: false,
    }
}

/// Identify missing requirement fields
pub fn requirement_missing_fields(contract: &RequirementContractArtifact) -> Vec<String> {
    let mut missing = Vec::new();
    if contract.goal.trim().is_empty() {
        missing.push("goal".to_string());
    }
    if contract.scope.trim().is_empty() {
        missing.push("scope".to_string());
    }
    if contract.acceptance_criteria.is_empty() {
        missing.push("acceptance_criteria".to_string());
    }
    if contract.constraints.is_empty() {
        missing.push("constraints".to_string());
    }
    missing
}

/// Generate clarification questions from missing fields
#[allow(dead_code)] // F-GAP-02 — reserved for interactive clarification flow
pub fn requirement_questions_from_missing(missing_fields: &[String]) -> Vec<String> {
    missing_fields
        .iter()
        .map(|field| match field.as_str() {
            "goal" => "这个任务最终想达成的业务目标是什么？".to_string(),
            "scope" => "本次改动边界是什么？哪些模块必须包含？".to_string(),
            "acceptance_criteria" => "验收标准是什么？如何证明完成？".to_string(),
            "constraints" => "有哪些硬约束（时间、兼容性、性能、安全）？".to_string(),
            other => format!("请补充字段: {}", other),
        })
        .collect::<Vec<_>>()
}

/// Estimate requirement ambiguity score
pub fn estimate_requirement_ambiguity(task: &str, contract: &RequirementContractArtifact) -> u8 {
    let characteristics = TaskRouter::analyze_task(task);
    let mut score = characteristics.complexity.min(5);
    let missing = requirement_missing_fields(contract).len() as u8;
    score = score.saturating_add(missing.min(2));
    score.min(5)
}

/// Load latest requirement contract for a task
pub fn load_latest_requirement_contract(
    ledger: &ArtifactLedger,
    task: &str,
) -> Option<RequirementContractArtifact> {
    let latest = ledger.latest_path("spec", "latest-clarification.json");
    let raw = std::fs::read_to_string(latest).ok()?;
    let artifact = serde_json::from_str::<RequirementContractArtifact>(&raw).ok()?;
    if artifact.task.trim() == task.trim() {
        Some(artifact)
    } else {
        None
    }
}

/// Evaluate requirement gate
pub fn evaluate_requirement_gate(
    ledger: &ArtifactLedger,
    task: &str,
    params: &Value,
    source: &str,
) -> anyhow::Result<RequirementGateDecision> {
    let characteristics = TaskRouter::analyze_task(task);
    let clarification_required = characteristics.complexity >= 3
        || characteristics.involves_multiple_modules
        || characteristics.needs_verification
        || characteristics.has_safety_concerns;

    let mut contract = parse_requirement_contract_from_params(params, task)
        .or_else(|| load_latest_requirement_contract(ledger, task))
        .unwrap_or_else(|| default_requirement_contract(task, source));
    contract.generated_at = now_ts();
    contract.source = source.to_string();
    contract.ambiguity_score = estimate_requirement_ambiguity(task, &contract);
    if let Some(v) = params
        .get("requirement_confirmed")
        .and_then(|v| v.as_bool())
    {
        contract.user_confirmed = v;
    }

    let missing_fields = requirement_missing_fields(&contract);
    let confirmed = contract.user_confirmed && missing_fields.is_empty();
    let blocked = clarification_required && !confirmed;

    let clarification_artifact_path =
        if parse_requirement_contract_from_params(params, task).is_some() {
            Some(persist_requirement_contract(ledger, &contract)?)
        } else {
            None
        };

    let reason = if blocked {
        Some(
            "requirement clarification/confirmation is required before planning or execution"
                .to_string(),
        )
    } else {
        None
    };
    let governance = GovernancePolicyArtifact {
        generated_at: now_ts(),
        task: task.to_string(),
        source: source.to_string(),
        clarification_required,
        confirmed,
        blocked,
        reason: reason.clone(),
        next_step: if blocked {
            serde_json::json!({
                "method": "workflow.clarify",
                "task": task,
                "missing_fields": missing_fields,
                "suggested_followup": "call workflow.confirm with completed requirement_contract and user_confirmed=true"
            })
        } else {
            serde_json::json!({"status": "confirmed"})
        },
    };
    let governance_artifact_path = persist_governance_policy(ledger, &governance)?;

    Ok(RequirementGateDecision {
        blocked,
        reason,
        missing_fields,
        clarification_artifact_path,
        governance_artifact_path,
    })
}

/// Evaluate unified requirement gate facade for workflow/task main paths.
pub fn evaluate_requirement_gate_facade(
    ledger: &ArtifactLedger,
    task: &str,
    params: &Value,
    source: &str,
) -> anyhow::Result<RequirementGateFacadeDecision> {
    let gate = evaluate_requirement_gate(ledger, task, params, source)?;
    let next_step = if gate.blocked {
        json!({
            "method": "workflow.clarify",
            "task": task,
            "missing_fields": gate.missing_fields,
        })
    } else {
        json!({"status": "confirmed"})
    };

    Ok(RequirementGateFacadeDecision {
        kind: "requirement_contract".to_string(),
        blocked: gate.blocked,
        reason: gate.reason,
        missing_fields: gate.missing_fields,
        next_step,
        clarification_artifact_path: gate.clarification_artifact_path,
        governance_artifact_path: gate.governance_artifact_path,
    })
}

/// Derive clarification quality score from contract
pub fn derive_clarification_quality_score(contract: &RequirementContractArtifact) -> f64 {
    let missing_count = requirement_missing_fields(contract).len() as f64;
    let completeness_score = ((4.0 - missing_count).max(0.0) / 4.0).clamp(0.0, 1.0);
    let ambiguity_penalty = (contract.ambiguity_score as f64 / 5.0).clamp(0.0, 1.0);
    let quality = 0.7 * completeness_score + 0.3 * (1.0 - ambiguity_penalty);
    quality.clamp(0.0, 1.0)
}

/// Resolve learning clarification metrics
pub fn resolve_learning_clarification_metrics(
    ledger: &ArtifactLedger,
    task: &str,
    params: &Value,
) -> LearningClarificationMetrics {
    let provided_contract = parse_requirement_contract_from_params(params, task);
    let latest_contract = load_latest_requirement_contract(ledger, task);
    let active_contract = provided_contract.as_ref().or(latest_contract.as_ref());

    let rounds = params
        .get("clarification_rounds")
        .and_then(|v| v.as_u64())
        .map(|v| v.min(64) as u32)
        .unwrap_or_else(|| {
            if let Some(contract) = active_contract {
                let has_questions = !contract.open_questions.is_empty();
                let base_rounds = if has_questions { 1 } else { 0 };
                let confirm_round = if contract.user_confirmed { 1 } else { 0 };
                (base_rounds + confirm_round).max(1)
            } else {
                0
            }
        });

    let quality_score = params
        .get("clarification_quality_score")
        .and_then(|v| v.as_f64())
        .map(|v| v.clamp(0.0, 1.0))
        .unwrap_or_else(|| {
            active_contract
                .map(derive_clarification_quality_score)
                .unwrap_or(0.0)
        });

    let requirement_change_count = params
        .get("requirement_change_count")
        .and_then(|v| v.as_u64())
        .map(|v| v.min(4096) as u32)
        .or_else(|| {
            params
                .get("requirement_contract_revision")
                .and_then(|v| v.as_u64())
                .map(|revision| revision.saturating_sub(1).min(4096) as u32)
        })
        .unwrap_or_else(|| {
            if let (Some(current), Some(previous)) =
                (provided_contract.as_ref(), latest_contract.as_ref())
            {
                let changed = current.goal != previous.goal
                    || current.scope != previous.scope
                    || current.non_goals != previous.non_goals
                    || current.acceptance_criteria != previous.acceptance_criteria
                    || current.constraints != previous.constraints;
                if changed {
                    1
                } else {
                    0
                }
            } else if provided_contract.is_some() {
                1
            } else {
                0
            }
        });

    LearningClarificationMetrics {
        rounds,
        quality_score,
        requirement_change_count,
    }
}
