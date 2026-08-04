//! Task planning module — plan decomposition, execution tracking, and workflow generation.
//!
//! Extracted from the original monolithic `reinforcement.rs`.

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsStr;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::task_decomposer::{TaskDecomposer, TaskDecomposition};
use crate::task_router::{RoutingDecision, TaskCharacteristics, TaskRouter};

use super::health::now_ts;
use super::learning::WorkflowLearningBusArtifact;
use super::ArtifactLedger;

/// Load and parse the persisted learning bus artifact (`latest-learning.json`)
/// once per request. Callers that need several recommendations should load a
/// single `WorkflowLearningBusArtifact` and pass it to the `*_from_learning_bus`
/// variants instead of re-reading the file per recommendation.
pub fn load_learning_bus(ledger: &ArtifactLedger) -> Option<WorkflowLearningBusArtifact> {
    let latest_path = ledger.latest_path("spec", "latest-learning.json");
    let payload = std::fs::read_to_string(&latest_path).ok()?;
    serde_json::from_str::<WorkflowLearningBusArtifact>(&payload).ok()
}

// ── Subtask tracking ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedSubtaskRecord {
    pub id: String,
    pub description: String,
    pub status: String,
    pub phase_index: usize,
    pub retry_count: u32,
    /// Unix timestamp when subtask execution started (None = not yet executed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_ts: Option<i64>,
    /// Unix timestamp when subtask execution stopped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_ts: Option<i64>,
    /// Wall-clock execution time in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// e.g. "completed", "failed", "skipped"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Agent/role name that executed this subtask.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
}

impl PlannedSubtaskRecord {
    /// Mark this record as executed with full lifecycle telemetry (Section 5).
    pub fn mark_executed(
        &mut self,
        start_ts: i64,
        stop_ts: i64,
        duration_ms: u64,
        outcome: impl Into<String>,
        executor: impl Into<String>,
    ) {
        self.start_ts = Some(start_ts);
        self.stop_ts = Some(stop_ts);
        self.duration_ms = Some(duration_ms);
        let outcome = outcome.into();
        self.status = outcome.clone();
        self.outcome = Some(outcome);
        self.executor = Some(executor.into());
    }
}

// ── Execution metrics ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecutionMetrics {
    pub subtask_parallelism: usize,
    pub failure_strategy: String,
    pub phases_executed: usize,
    pub halted_early: bool,
    pub parallel_utilization: f64,
    pub serial_degradation_count: usize,
    pub parallel_failure_rollback_count: usize,
    pub serial_work_ms: u64,
    pub critical_path_ms: u64,
    pub parallel_efficiency: f64,
    pub parallel_speedup: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecutionSummary {
    pub generated_at: i64,
    pub task: String,
    pub subtasks_total: usize,
    pub subtasks_completed: usize,
    pub subtasks_failed: usize,
    pub subtasks_skipped: usize,
    pub executor: String,
    pub records: Vec<PlannedSubtaskRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_metrics: Option<TaskExecutionMetrics>,
    pub artifact_path: Option<String>,
}

// ── Checkpoint ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointSummaryArtifact {
    pub checkpoint_id: String,
    pub conversation_id: String,
    pub branch_id: String,
    pub parent_checkpoint_id: Option<String>,
    pub created_at: i64,
    pub note: Option<String>,
    pub message_count: usize,
    pub message_chars: usize,
    pub assistant_excerpt: Option<String>,
}

// ── Research artifact ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResearchArtifact {
    pub generated_at: i64,
    pub task: String,
    pub planner_output: String,
    pub researcher_output: String,
    pub reviewer_output: String,
    pub recommended_plan: String,
}

// ── Task plan ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlanArtifact {
    pub generated_at: i64,
    pub task: String,
    pub characteristics: TaskCharacteristics,
    pub routing: RoutingDecision,
    pub decomposition: Option<TaskDecomposition>,
    pub planned_subtasks: Vec<PlannedSubtaskRecord>,
    pub sub_agent_recommended: bool,
    pub activation_reasons: Vec<String>,
    pub action_checks_required: Vec<String>,
}

// ── Workflow graph ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowNode {
    pub id: String,
    pub description: String,
    pub phase_index: usize,
    pub dependencies: Vec<String>,
    pub role: String,
    pub timeout_seconds: u64,
    pub retry_limit: u32,
    pub priority: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowGeneratedArtifact {
    pub generated_at: i64,
    pub task: String,
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    pub execution_order: Vec<Vec<String>>,
    pub auto_gates: Vec<String>,
    pub routing_summary: Value,
}

// ── Policy & decision artifacts ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowOptimizationPolicyArtifact {
    pub generated_at: i64,
    pub task: String,
    pub source: String,
    pub policy_report: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase_parallelism_cap: Option<u64>,
    pub force_fail_fast: bool,
    #[serde(default)]
    pub runtime_healthy: bool,
    #[serde(default)]
    pub anomaly_detected: bool,
    #[serde(default)]
    pub detached_modules: Vec<String>,
    #[serde(default)]
    pub reattached_modules: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowWorkGradeArtifact {
    pub generated_at: i64,
    pub task: String,
    pub source: String,
    pub requested_grade: String,
    pub decided_grade: String,
    pub decision_action: String,
    pub reasons: Vec<String>,
    pub risk_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineUnifiedMetricsArtifact {
    pub generated_at: i64,
    pub task: String,
    pub source: String,
    pub predicted_success_rate: f64,
    pub risk_score: f64,
    pub runtime_healthy: bool,
    pub gates_ok: bool,
    pub subtasks_total: usize,
    pub subtasks_completed: usize,
    pub subtasks_failed: usize,
    pub subtasks_skipped: usize,
    pub parallelism: usize,
    pub parallel_utilization: f64,
    pub serial_degradation_count: usize,
    pub parallel_failure_rollback_count: usize,
    pub failure_strategy: String,
    pub work_grade: String,
    pub optimization_policy: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementContractArtifact {
    pub generated_at: i64,
    pub task: String,
    pub source: String,
    pub goal: String,
    pub scope: String,
    pub non_goals: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub constraints: Vec<String>,
    pub open_questions: Vec<String>,
    pub ambiguity_score: u8,
    pub user_confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernancePolicyArtifact {
    pub generated_at: i64,
    pub task: String,
    pub source: String,
    pub clarification_required: bool,
    pub confirmed: bool,
    pub blocked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub next_step: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionDecisionCandidate {
    pub agent: String,
    pub score: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionAssignmentRecord {
    pub subtask_id: String,
    pub phase_index: usize,
    pub task_index: usize,
    pub desired_role: String,
    pub selected_agent: String,
    pub selection_reason: String,
    pub candidate_scores: Vec<ExecutionDecisionCandidate>,
    pub dependency_blocked: bool,
    pub node_primary_agent: String,
    pub node_secondary_agents: Vec<String>,
    pub effective_executor: String,
    pub failover_applied: bool,
    pub failover_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelPhaseDecisionRecord {
    pub phase_index: usize,
    pub subtask_count: usize,
    pub parallelism_limit: usize,
    pub utilization_target: f64,
    pub has_dependencies: bool,
    pub execution_mode: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionDecisionArtifact {
    pub generated_at: i64,
    pub task: String,
    pub source: String,
    pub selected_agents: Vec<String>,
    pub assignment_reason: String,
    pub subtask_assignments: Vec<ExecutionAssignmentRecord>,
    pub parallel_phase_decisions: Vec<ParallelPhaseDecisionRecord>,
    pub parallelism: usize,
    pub failure_strategy: String,
    pub degrade_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrimarySecondaryPolicyArtifact {
    pub generated_at: i64,
    pub task: String,
    pub source: String,
    pub primary_agent: String,
    pub secondary_agents: Vec<String>,
    pub policy_version: String,
    pub failover_policy: String,
    pub secondary_max_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrimaryFailoverReportItem {
    pub subtask_id: String,
    pub phase_index: usize,
    pub selected_primary_agent: String,
    pub effective_executor: String,
    pub failover_applied: bool,
    pub failover_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrimarySecondaryFailoverArtifact {
    pub generated_at: i64,
    pub task: String,
    pub source: String,
    pub primary_agent: String,
    pub secondary_agents: Vec<String>,
    pub failover_policy: String,
    pub total_subtasks: usize,
    pub failover_count: usize,
    pub reports: Vec<PrimaryFailoverReportItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsultationArtifact {
    pub generated_at: i64,
    pub task: String,
    pub source: String,
    pub trigger_reason: String,
    pub participants: Vec<String>,
    pub candidate_plans: Vec<String>,
    pub consensus_plan: String,
    pub risk_matrix: Value,
    pub decision_confidence: f64,
    pub handoff_primary_agent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClarificationSessionArtifact {
    pub generated_at: i64,
    pub task: String,
    pub source: String,
    pub session_id: String,
    pub round_index: usize,
    pub lead_clarifier: String,
    pub assistant_clarifiers: Vec<String>,
    pub user_feedback: String,
    pub resolved_points: Vec<String>,
    pub open_points: Vec<String>,
    pub next_questions: Vec<String>,
    pub ready_to_confirm: bool,
}

// ── Public functions ───────────────────────────────────────────────────────

/// Build a task plan from a task description.
///
/// Analyzes the task, routes it, decomposes if complex enough, and returns
/// a complete `TaskPlanArtifact` with subtask records and activation reasons.
pub fn build_task_plan(task: &str) -> TaskPlanArtifact {
    let characteristics = TaskRouter::analyze_task(task);
    let routing = TaskRouter::route_task(&characteristics);
    let should_decompose = characteristics.complexity >= 3
        || routing.roles.len() >= 2
        || characteristics.involves_multiple_modules;
    let decomposition = should_decompose.then(|| TaskDecomposer::decompose(&characteristics));

    let planned_subtasks = decomposition
        .as_ref()
        .map(planned_subtask_records)
        .unwrap_or_default();

    let mut activation_reasons = Vec::new();
    if characteristics.complexity >= 4 {
        activation_reasons.push("complexity>=4".to_string());
    }
    if routing.roles.len() >= 3 {
        activation_reasons.push("multi_role_execution".to_string());
    }
    if !routing.can_parallelize.is_empty() {
        activation_reasons.push("parallelizable_roles_detected".to_string());
    }
    if characteristics.involves_multiple_modules {
        activation_reasons.push("cross_module_task".to_string());
    }

    TaskPlanArtifact {
        generated_at: now_ts(),
        task: task.to_string(),
        characteristics,
        routing,
        decomposition,
        planned_subtasks,
        sub_agent_recommended: !activation_reasons.is_empty(),
        activation_reasons,
        action_checks_required: vec![
            "spec".to_string(),
            "qa".to_string(),
            "retest".to_string(),
            "final".to_string(),
        ],
    }
}

/// Persist a task plan to the artifact ledger.
pub fn persist_task_plan(ledger: &ArtifactLedger, plan: &TaskPlanArtifact) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-plan.json", plan)
}

/// Build a `WorkflowGeneratedArtifact` from a task plan.
///
/// Generates the workflow DAG with nodes, edges, execution order, and auto-gates.
pub fn build_workflow_generated_artifact(plan: &TaskPlanArtifact) -> WorkflowGeneratedArtifact {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    for record in &plan.planned_subtasks {
        let (dependencies, priority, timeout_seconds, retry_limit) = plan
            .decomposition
            .as_ref()
            .and_then(|decomposition| {
                decomposition
                    .subtasks
                    .iter()
                    .find(|subtask| subtask.id == record.id)
                    .map(|subtask| {
                        let mut deps = subtask.dependencies.iter().cloned().collect::<Vec<_>>();
                        deps.sort();
                        let timeout =
                            ((subtask.estimated_duration_seconds / 2) as u64).clamp(60, 900);
                        let retry = if subtask.complexity >= 4 { 2 } else { 1 };
                        (deps, subtask.priority, timeout, retry)
                    })
            })
            .unwrap_or_else(|| (Vec::new(), 3, 120, 1));

        let role = choose_workflow_role(&record.description, &plan.routing.roles);
        for dep in &dependencies {
            edges.push(WorkflowEdge {
                from: dep.clone(),
                to: record.id.clone(),
            });
        }

        nodes.push(WorkflowNode {
            id: record.id.clone(),
            description: record.description.clone(),
            phase_index: record.phase_index,
            dependencies,
            role,
            timeout_seconds,
            retry_limit,
            priority,
        });
    }

    let execution_order = if let Some(decomposition) = plan.decomposition.as_ref() {
        decomposition.execution_phases.clone()
    } else {
        let mut phases = std::collections::BTreeMap::<usize, Vec<String>>::new();
        for record in &plan.planned_subtasks {
            phases
                .entry(record.phase_index)
                .or_default()
                .push(record.id.clone());
        }
        phases.into_values().collect::<Vec<_>>()
    };

    WorkflowGeneratedArtifact {
        generated_at: now_ts(),
        task: plan.task.clone(),
        nodes,
        edges,
        execution_order,
        auto_gates: plan.action_checks_required.clone(),
        routing_summary: json!({
            "roles": plan
                .routing
                .roles
                .iter()
                .map(|r| format!("{:?}", r))
                .collect::<Vec<_>>(),
            "predicted_success_rate": plan.routing.predicted_success_rate,
            "risk_factors": plan.routing.risk_factors,
        }),
    }
}

/// Persist a workflow-generated artifact to the ledger.
pub fn persist_workflow_generated(
    ledger: &ArtifactLedger,
    workflow: &WorkflowGeneratedArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-workflow.json", workflow)
}

/// Persist an execution summary to the ledger.
pub fn persist_task_execution_summary(
    ledger: &ArtifactLedger,
    summary: &TaskExecutionSummary,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-execution.json", summary)
}

/// Persist a workflow research artifact to the ledger.
pub fn persist_workflow_research(
    ledger: &ArtifactLedger,
    artifact: &WorkflowResearchArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-research.json", artifact)
}

/// Persist an optimization policy artifact.
pub fn persist_workflow_optimization_policy(
    ledger: &ArtifactLedger,
    artifact: &WorkflowOptimizationPolicyArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-optimization-policy.json", artifact)
}

/// Persist a work grade artifact.
pub fn persist_workflow_work_grade(
    ledger: &ArtifactLedger,
    artifact: &WorkflowWorkGradeArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-work-grade.json", artifact)
}

/// Persist pipeline unified metrics.
pub fn persist_pipeline_unified_metrics(
    ledger: &ArtifactLedger,
    artifact: &PipelineUnifiedMetricsArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-pipeline-metrics.json", artifact)
}

/// Persist a requirement contract artifact.
pub fn persist_requirement_contract(
    ledger: &ArtifactLedger,
    artifact: &RequirementContractArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-clarification.json", artifact)
}

/// Persist a governance policy artifact.
pub fn persist_governance_policy(
    ledger: &ArtifactLedger,
    artifact: &GovernancePolicyArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-governance-policy.json", artifact)
}

/// Persist an execution decision artifact.
pub fn persist_execution_decision(
    ledger: &ArtifactLedger,
    artifact: &ExecutionDecisionArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-execution-decision.json", artifact)
}

/// Persist a primary-secondary policy artifact.
pub fn persist_primary_secondary_policy_artifact(
    ledger: &ArtifactLedger,
    artifact: &PrimarySecondaryPolicyArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-primary-secondary-policy.json", artifact)
}

/// Persist a primary-secondary failover artifact.
pub fn persist_primary_secondary_failover_artifact(
    ledger: &ArtifactLedger,
    artifact: &PrimarySecondaryFailoverArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-primary-secondary-failover.json", artifact)
}

/// Persist a consultation artifact.
pub fn persist_consultation_artifact(
    ledger: &ArtifactLedger,
    artifact: &ConsultationArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-consultation.json", artifact)
}

/// Persist a clarification session artifact.
pub fn persist_clarification_session_artifact(
    ledger: &ArtifactLedger,
    artifact: &ClarificationSessionArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-clarification-session.json", artifact)
}

/// Recommend modules to reattach based on recent optimization-policy history.
///
/// Recovery rule:
/// - Require the last `required_healthy_streak` records to be healthy and anomaly-free.
/// - If satisfied, recover modules detached by the latest anomalous record before that streak.
pub fn recommend_reattach_modules_from_policy_history(
    ledger: &ArtifactLedger,
    required_healthy_streak: usize,
    max_records: usize,
) -> Vec<String> {
    let required_healthy_streak = required_healthy_streak.max(1);
    let max_records = max_records.max(required_healthy_streak);

    let spec_dir = ledger.latest_path("spec", "latest-optimization-policy.json");
    let Some(parent) = spec_dir.parent() else {
        return Vec::new();
    };

    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut events = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension() != Some(OsStr::new("json")) {
            continue;
        }
        let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        if !name.starts_with("latest-optimization-policy") {
            continue;
        }

        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(event) = serde_json::from_str::<WorkflowOptimizationPolicyArtifact>(&raw) else {
            continue;
        };
        events.push(event);
    }

    if events.len() < required_healthy_streak + 1 {
        return Vec::new();
    }

    events.sort_by_key(|event| event.generated_at);
    if events.len() > max_records {
        let drain_to = events.len() - max_records;
        events.drain(0..drain_to);
    }

    let healthy_tail = events
        .iter()
        .rev()
        .take(required_healthy_streak)
        .all(|event| event.runtime_healthy && !event.anomaly_detected);
    if !healthy_tail {
        return Vec::new();
    }

    let mut recovered = BTreeSet::new();
    let anomaly_anchor = events
        .iter()
        .rev()
        .skip(required_healthy_streak)
        .find(|event| event.anomaly_detected && !event.detached_modules.is_empty());

    if let Some(anchor) = anomaly_anchor {
        for module in &anchor.detached_modules {
            recovered.insert(module.clone());
        }
    }

    recovered.into_iter().collect()
}

/// Recommend next parallelism level from persisted learning events.
pub fn recommend_parallelism_from_learning(
    ledger: &ArtifactLedger,
    current: usize,
    min_parallelism: usize,
    max_parallelism: usize,
) -> usize {
    let Some(bus) = load_learning_bus(ledger) else {
        return current;
    };
    recommend_parallelism_from_learning_bus(&bus, current, min_parallelism, max_parallelism)
}

/// Parallelism recommendation from an already-loaded learning bus (no I/O).
pub fn recommend_parallelism_from_learning_bus(
    bus: &WorkflowLearningBusArtifact,
    current: usize,
    min_parallelism: usize,
    max_parallelism: usize,
) -> usize {
    let min_p = min_parallelism.max(1);
    let max_p = max_parallelism.max(min_p);
    let current = current.clamp(min_p, max_p);

    if bus.events.len() < 8 {
        return current;
    }

    let recent = bus.events.iter().rev().take(20).collect::<Vec<_>>();
    if recent.is_empty() {
        return current;
    }

    let avg_speedup = recent
        .iter()
        .map(|event| event.parallel_speedup)
        .sum::<f64>()
        / recent.len() as f64;

    let mut total_subtasks = 0usize;
    let mut total_failed = 0usize;
    for event in &recent {
        total_subtasks = total_subtasks.saturating_add(event.subtasks_total.max(1));
        total_failed = total_failed.saturating_add(event.subtasks_failed);
    }
    let fail_rate = total_failed as f64 / total_subtasks as f64;

    if fail_rate > 0.25 {
        current.saturating_sub(1).clamp(min_p, max_p)
    } else if avg_speedup > 1.6 && fail_rate < 0.10 {
        current.saturating_add(1).clamp(min_p, max_p)
    } else {
        current
    }
}

/// Recommend failure strategy from persisted learning events.
/// Returns "fail_fast" or "tolerant".
pub fn recommend_failure_strategy_from_learning(ledger: &ArtifactLedger, current: &str) -> String {
    let Some(bus) = load_learning_bus(ledger) else {
        return current.to_string();
    };
    recommend_failure_strategy_from_learning_bus(&bus, current)
}

/// Failure-strategy recommendation from an already-loaded learning bus (no I/O).
pub fn recommend_failure_strategy_from_learning_bus(
    bus: &WorkflowLearningBusArtifact,
    current: &str,
) -> String {
    if bus.events.len() < 8 {
        return current.to_string();
    }

    let recent = bus.events.iter().rev().take(20).collect::<Vec<_>>();
    if recent.is_empty() {
        return current.to_string();
    }

    let mut total_subtasks = 0usize;
    let mut total_failed = 0usize;
    for event in &recent {
        total_subtasks = total_subtasks.saturating_add(event.subtasks_total.max(1));
        total_failed = total_failed.saturating_add(event.subtasks_failed);
    }
    let fail_rate = total_failed as f64 / total_subtasks as f64;

    if fail_rate >= 0.35 {
        "fail_fast".to_string()
    } else if fail_rate <= 0.15 {
        "tolerant".to_string()
    } else {
        current.to_string()
    }
}

/// Recommend work grade from persisted learning events.
pub fn recommend_work_grade_from_learning(ledger: &ArtifactLedger, current: &str) -> String {
    let Some(bus) = load_learning_bus(ledger) else {
        return current.to_string();
    };
    recommend_work_grade_from_learning_bus(&bus, current)
}

/// Work-grade recommendation from an already-loaded learning bus (no I/O).
pub fn recommend_work_grade_from_learning_bus(
    bus: &WorkflowLearningBusArtifact,
    current: &str,
) -> String {
    if bus.events.len() < 8 {
        return current.to_string();
    }

    let recent = bus.events.iter().rev().take(20).collect::<Vec<_>>();
    if recent.is_empty() {
        return current.to_string();
    }

    let mut total_subtasks = 0usize;
    let mut total_failed = 0usize;
    let mut gates_ok_count = 0usize;
    let mut runtime_healthy_count = 0usize;
    let mut complexity_sum = 0usize;

    for event in &recent {
        total_subtasks = total_subtasks.saturating_add(event.subtasks_total.max(1));
        total_failed = total_failed.saturating_add(event.subtasks_failed);
        if event.gates_ok {
            gates_ok_count = gates_ok_count.saturating_add(1);
        }
        if event.runtime_healthy {
            runtime_healthy_count = runtime_healthy_count.saturating_add(1);
        }
        complexity_sum = complexity_sum.saturating_add(event.complexity as usize);
    }

    let fail_rate = total_failed as f64 / total_subtasks as f64;
    let gate_pass_rate = gates_ok_count as f64 / recent.len() as f64;
    let runtime_healthy_rate = runtime_healthy_count as f64 / recent.len() as f64;
    let avg_complexity = complexity_sum as f64 / recent.len() as f64;

    if fail_rate >= 0.30 || gate_pass_rate < 0.70 || runtime_healthy_rate < 0.80 {
        "safeguard".to_string()
    } else if avg_complexity >= 3.0 && fail_rate <= 0.12 && gate_pass_rate >= 0.90 {
        "full_auto".to_string()
    } else if fail_rate <= 0.08 && avg_complexity <= 2.0 && gate_pass_rate >= 0.92 {
        "edit".to_string()
    } else {
        "agent".to_string()
    }
}

/// Recommend a tuned predicted success rate using recent LearningBus outcomes.
pub fn recommend_predicted_success_rate_from_learning(
    ledger: &ArtifactLedger,
    current: f32,
    target_complexity: u8,
) -> f32 {
    let Some(bus) = load_learning_bus(ledger) else {
        return current;
    };
    recommend_predicted_success_rate_from_learning_bus(&bus, current, target_complexity)
}

/// Predicted-success-rate recommendation from an already-loaded learning bus (no I/O).
pub fn recommend_predicted_success_rate_from_learning_bus(
    bus: &WorkflowLearningBusArtifact,
    current: f32,
    target_complexity: u8,
) -> f32 {
    if bus.events.len() < 8 {
        return current;
    }

    let recent = bus.events.iter().rev().take(48).collect::<Vec<_>>();
    if recent.is_empty() {
        return current;
    }

    let mut weighted_success = 0.0f64;
    let mut weighted_total = 0.0f64;
    for event in &recent {
        let total = event.subtasks_total.max(1) as f64;
        let success = event.subtasks_completed as f64 / total;
        let complexity_distance = (event.complexity as i32 - target_complexity as i32).abs() as f64;
        let complexity_weight = 1.0 / (1.0 + complexity_distance);
        let gate_weight = if event.gates_ok { 1.0 } else { 0.8 };
        let runtime_weight = if event.runtime_healthy { 1.0 } else { 0.85 };
        let weight = complexity_weight * gate_weight * runtime_weight;
        weighted_success += success * weight;
        weighted_total += weight;
    }

    if weighted_total <= f64::EPSILON {
        return current;
    }

    let learned = (weighted_success / weighted_total).clamp(0.05, 0.99) as f32;
    (current * 0.35 + learned * 0.65).clamp(0.05, 0.99)
}

/// Reorder candidate execution agents from recent execution history.
///
/// Agents are ranked by Bayesian-smoothed success score derived from
/// `TaskExecutionSummary.records` executor outcomes.
pub fn recommend_agent_order_from_execution_history(
    ledger: &ArtifactLedger,
    candidates: &[String],
    max_records: usize,
) -> Vec<String> {
    if candidates.len() <= 1 {
        return candidates.to_vec();
    }

    let latest = ledger.latest_path("spec", "latest-execution.json");
    let Some(parent) = latest.parent() else {
        return candidates.to_vec();
    };

    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(_) => return candidates.to_vec(),
    };

    let mut summaries = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension() != Some(OsStr::new("json")) {
            continue;
        }
        let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        if !name.starts_with("latest-execution") {
            continue;
        }

        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(summary) = serde_json::from_str::<TaskExecutionSummary>(&raw) else {
            continue;
        };
        summaries.push(summary);
    }

    if summaries.is_empty() {
        return candidates.to_vec();
    }

    summaries.sort_by_key(|summary| summary.generated_at);
    if summaries.len() > max_records.max(1) {
        let drain_to = summaries.len() - max_records.max(1);
        summaries.drain(0..drain_to);
    }

    let mut stats: HashMap<String, (u64, u64)> = HashMap::new();
    for summary in &summaries {
        for record in &summary.records {
            let Some(executor) = record.executor.as_ref() else {
                continue;
            };
            if !candidates.iter().any(|candidate| candidate == executor) {
                continue;
            }
            let entry = stats.entry(executor.clone()).or_insert((0, 0));
            if record.status == "completed" {
                entry.0 = entry.0.saturating_add(1);
            } else if record.status == "failed" {
                entry.1 = entry.1.saturating_add(1);
            }
        }
    }

    let mut ranked = candidates
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let (completed, failed) = stats.get(name).copied().unwrap_or((0, 0));
            let score = (completed as f64 + 1.0) / (completed as f64 + failed as f64 + 2.0);
            (name.clone(), score, completed + failed, index)
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| a.3.cmp(&b.3))
    });

    ranked.into_iter().map(|(name, _, _, _)| name).collect()
}

// ── Internal helpers ───────────────────────────────────────────────────────

fn planned_subtask_records(decomposition: &TaskDecomposition) -> Vec<PlannedSubtaskRecord> {
    decomposition
        .execution_phases
        .iter()
        .enumerate()
        .flat_map(|(phase_index, subtask_ids)| {
            subtask_ids.iter().filter_map(move |subtask_id| {
                decomposition
                    .subtasks
                    .iter()
                    .find(|subtask| subtask.id == *subtask_id)
                    .map(|subtask| PlannedSubtaskRecord {
                        id: subtask.id.clone(),
                        description: subtask.description.clone(),
                        status: "planned".to_string(),
                        phase_index,
                        retry_count: 0,
                        start_ts: None,
                        stop_ts: None,
                        duration_ms: None,
                        outcome: None,
                        executor: None,
                    })
            })
        })
        .collect()
}

fn choose_workflow_role(description: &str, roles: &[crate::roles::AgentRole]) -> String {
    let lower = description.to_ascii_lowercase();
    if lower.contains("test") || lower.contains("verify") || lower.contains("regression") {
        return "tester".to_string();
    }
    if lower.contains("review") || lower.contains("audit") {
        return "reviewer".to_string();
    }
    if lower.contains("research") || lower.contains("analy") {
        return "researcher".to_string();
    }
    if lower.contains("plan") || lower.contains("design") {
        return "planner".to_string();
    }

    let has_coder = roles
        .iter()
        .any(|role| matches!(role, crate::roles::AgentRole::Coder));
    if has_coder {
        "coder".to_string()
    } else {
        roles
            .first()
            .map(|role| format!("{:?}", role).to_ascii_lowercase())
            .unwrap_or_else(|| "coder".to_string())
    }
}

// Keep module declaration for the sub-module containing tests.
// Tests are defined inline in the original reinforcement.rs and will remain there.
