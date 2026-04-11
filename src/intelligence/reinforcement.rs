//! BLUE2 reinforcement utilities.
//!
//! This module provides a focused implementation of the BLUE2 plan without
//! imposing whole-repo overreach. It adds:
//! - durable `.goon/` artifacts
//! - runtime healthchecks
//! - executable action checks
//! - controlled task planning artifacts for sub-agent style decomposition

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    collections::{BTreeSet, HashMap},
    ffi::OsStr,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cache::ResponseCache;
use crate::config::{validate_runtime_readiness, AppConfig};
use crate::task_decomposer::{TaskDecomposer, TaskDecomposition};
use crate::task_router::{RoutingDecision, TaskCharacteristics, TaskRouter};
use crate::vector::VectorStore;

const GOON_DIR: &str = ".goon";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Healthy,
    Warn,
    Error,
    Skipped,
}

impl CheckStatus {
    fn severity(self) -> u8 {
        match self {
            Self::Healthy => 0,
            Self::Skipped => 0,
            Self::Warn => 1,
            Self::Error => 2,
        }
    }

    fn merge(self, other: Self) -> Self {
        if self.severity() >= other.severity() {
            self
        } else {
            other
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentReport {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeHealthcheckReport {
    pub generated_at: i64,
    pub overall_status: CheckStatus,
    pub components: Vec<ComponentReport>,
}

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

/// Aggregate result of executing a `TaskPlanArtifact` via `task.execute`.
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResearchArtifact {
    pub generated_at: i64,
    pub task: String,
    pub planner_output: String,
    pub researcher_output: String,
    pub reviewer_output: String,
    pub recommended_plan: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowLearningEvent {
    pub generated_at: i64,
    pub task: String,
    pub complexity: u8,
    pub predicted_success_rate: f32,
    pub subtasks_total: usize,
    pub subtasks_completed: usize,
    pub subtasks_failed: usize,
    pub subtasks_skipped: usize,
    pub serial_work_ms: u64,
    pub critical_path_ms: u64,
    pub parallel_speedup: f64,
    pub parallel_efficiency: f64,
    pub executor: String,
    pub source: String,
    #[serde(default)]
    pub runtime_healthy: bool,
    #[serde(default = "default_workflow_learning_gates_ok")]
    pub gates_ok: bool,
    #[serde(default)]
    pub work_grade: String,
    #[serde(default)]
    pub risk_score: f64,
    #[serde(default)]
    pub clarification_rounds: u32,
    #[serde(default)]
    pub clarification_quality_score: f64,
    #[serde(default)]
    pub requirement_change_count: u32,
    #[serde(default)]
    pub review_reject_root_cause: String,
    #[serde(default)]
    pub primary_stability_score: f64,
    #[serde(default)]
    pub secondary_utilization_rate: f64,
    #[serde(default)]
    pub failover_count: u32,
    #[serde(default)]
    pub failover_root_cause: String,
}

fn default_workflow_learning_gates_ok() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowLearningBusArtifact {
    pub generated_at: i64,
    pub total_events: usize,
    pub events: Vec<WorkflowLearningEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeInsightArtifact {
    pub generated_at: i64,
    pub conversation_id: String,
    pub branch_id: String,
    pub phase: String,
    pub task: String,
    pub agent: String,
    pub source: String,
    pub request_excerpt: String,
    pub response_excerpt: String,
    pub reusable_insights: Vec<String>,
    pub verification_steps: Vec<String>,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeBusArtifact {
    pub generated_at: i64,
    pub total_events: usize,
    pub events: Vec<KnowledgeInsightArtifact>,
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desired_role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_agent: Option<String>,
    pub selection_reason: String,
    pub candidate_scores: Vec<ExecutionDecisionCandidate>,
    pub dependency_blocked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_primary_agent: Option<String>,
    #[serde(default)]
    pub node_secondary_agents: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_executor: Option<String>,
    #[serde(default)]
    pub failover_applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    pub selected_primary_agent: Option<String>,
    pub effective_executor: Option<String>,
    pub failover_applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    pub round_index: u32,
    pub lead_clarifier: String,
    pub assistant_clarifiers: Vec<String>,
    pub user_feedback: String,
    pub resolved_points: Vec<String>,
    pub open_points: Vec<String>,
    pub next_questions: Vec<String>,
    pub ready_to_confirm: bool,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionCheckKind {
    All,
    Spec,
    Qa,
    Retest,
    Final,
}

impl ActionCheckKind {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "all" => Some(Self::All),
            "spec" => Some(Self::Spec),
            "qa" => Some(Self::Qa),
            "retest" => Some(Self::Retest),
            "final" => Some(Self::Final),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Spec => "spec",
            Self::Qa => "qa",
            Self::Retest => "retest",
            Self::Final => "final",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionCheckItem {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    pub artifact_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionCheckReport {
    pub generated_at: i64,
    pub kind: String,
    pub overall_status: CheckStatus,
    pub ok: bool,
    pub checks_run: Vec<ActionCheckItem>,
    pub evidence_refs: Vec<String>,
    pub retest_report_path: Option<String>,
    pub final_summary_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalSummaryArtifact {
    pub generated_at: i64,
    pub overall_status: CheckStatus,
    pub evidence_refs: Vec<String>,
    pub action_check_path: String,
    pub conclusion: String,
}

#[derive(Debug, Clone)]
pub struct ArtifactLedger {
    root: PathBuf,
}

impl ArtifactLedger {
    pub fn new(config_path: Option<&Path>) -> Self {
        let root = config_path
            .and_then(|path| path.parent().map(|parent| parent.join(GOON_DIR)))
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(GOON_DIR)
            });
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn ensure_ready(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("failed to create ledger root {}", self.root.display()))
    }

    pub fn latest_path(&self, category: &str, latest_name: &str) -> PathBuf {
        self.root.join(category).join(latest_name)
    }

    pub fn write_json<T: Serialize>(
        &self,
        category: &str,
        latest_name: &str,
        value: &T,
    ) -> Result<PathBuf> {
        self.ensure_ready()?;

        let dir = self.root.join(category);
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create ledger category {}", dir.display()))?;

        let latest_path = dir.join(latest_name);
        let stem = latest_name.strip_suffix(".json").unwrap_or(latest_name);
        let archive_path = dir.join(format!("{}-{}.json", stem, now_ts()));
        let encoded = serde_json::to_vec_pretty(value)?;

        fs::write(&archive_path, &encoded).with_context(|| {
            format!("failed to write ledger artifact {}", archive_path.display())
        })?;
        fs::write(&latest_path, &encoded).with_context(|| {
            format!(
                "failed to write latest ledger artifact {}",
                latest_path.display()
            )
        })?;

        Ok(latest_path)
    }
}

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

pub fn persist_task_plan(ledger: &ArtifactLedger, plan: &TaskPlanArtifact) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-plan.json", plan)
}

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

pub fn persist_workflow_generated(
    ledger: &ArtifactLedger,
    workflow: &WorkflowGeneratedArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-workflow.json", workflow)
}

/// Persist the execution summary produced by `task.execute` to the durable ledger.
pub fn persist_task_execution_summary(
    ledger: &ArtifactLedger,
    summary: &TaskExecutionSummary,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-execution.json", summary)
}

pub fn persist_workflow_research(
    ledger: &ArtifactLedger,
    artifact: &WorkflowResearchArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-research.json", artifact)
}

pub fn persist_workflow_learning_event(
    ledger: &ArtifactLedger,
    event: WorkflowLearningEvent,
    max_events: usize,
) -> Result<PathBuf> {
    ledger.ensure_ready()?;

    let latest_path = ledger.latest_path("spec", "latest-learning.json");
    let mut existing = if latest_path.exists() {
        fs::read_to_string(&latest_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<WorkflowLearningBusArtifact>(&raw).ok())
            .unwrap_or(WorkflowLearningBusArtifact {
                generated_at: now_ts(),
                total_events: 0,
                events: Vec::new(),
            })
    } else {
        WorkflowLearningBusArtifact {
            generated_at: now_ts(),
            total_events: 0,
            events: Vec::new(),
        }
    };

    existing.events.push(event);
    if existing.events.len() > max_events {
        let overflow = existing.events.len() - max_events;
        existing.events.drain(0..overflow);
    }
    existing.generated_at = now_ts();
    existing.total_events = existing.events.len();

    ledger.write_json("spec", "latest-learning.json", &existing)
}

pub fn persist_knowledge_insight_event(
    ledger: &ArtifactLedger,
    event: KnowledgeInsightArtifact,
    max_events: usize,
) -> Result<PathBuf> {
    ledger.ensure_ready()?;

    let max_events = max_events.max(1);
    let latest_path = ledger.latest_path("spec", "latest-knowledge.json");
    let mut existing = if latest_path.exists() {
        fs::read_to_string(&latest_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<KnowledgeBusArtifact>(&raw).ok())
            .unwrap_or(KnowledgeBusArtifact {
                generated_at: now_ts(),
                total_events: 0,
                events: Vec::new(),
            })
    } else {
        KnowledgeBusArtifact {
            generated_at: now_ts(),
            total_events: 0,
            events: Vec::new(),
        }
    };

    // Dedup + confidence arbitration:
    // For events sharing (task, phase, agent), keep whichever has the higher confidence.
    // If the incoming event has lower-or-equal confidence an existing entry for the same
    // key, discard the incoming event (the existing knowledge is already at least as
    // certain). If higher, replace the existing entry so the bus always holds the most
    // confident conclusion per (task, phase, agent) combination.
    let existing_pos = existing
        .events
        .iter()
        .position(|e| e.task == event.task && e.phase == event.phase && e.agent == event.agent);
    match existing_pos {
        Some(idx) if existing.events[idx].confidence >= event.confidence => {
            // Existing entry is at least as confident — no change needed.
        }
        Some(idx) => {
            // Incoming event supersedes the existing one.
            existing.events[idx] = event;
        }
        None => {
            // No duplicate — append normally.
            existing.events.push(event);
            if existing.events.len() > max_events {
                let overflow = existing.events.len() - max_events;
                existing.events.drain(0..overflow);
            }
        }
    }

    existing.generated_at = now_ts();
    existing.total_events = existing.events.len();

    ledger.write_json("spec", "latest-knowledge.json", &existing)
}

pub fn persist_workflow_optimization_policy(
    ledger: &ArtifactLedger,
    artifact: &WorkflowOptimizationPolicyArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-optimization-policy.json", artifact)
}

pub fn persist_workflow_work_grade(
    ledger: &ArtifactLedger,
    artifact: &WorkflowWorkGradeArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-work-grade.json", artifact)
}

pub fn persist_pipeline_unified_metrics(
    ledger: &ArtifactLedger,
    artifact: &PipelineUnifiedMetricsArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-pipeline-metrics.json", artifact)
}

pub fn persist_requirement_contract(
    ledger: &ArtifactLedger,
    artifact: &RequirementContractArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-clarification.json", artifact)
}

pub fn persist_governance_policy(
    ledger: &ArtifactLedger,
    artifact: &GovernancePolicyArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-governance-policy.json", artifact)
}

pub fn persist_execution_decision(
    ledger: &ArtifactLedger,
    artifact: &ExecutionDecisionArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-execution-decision.json", artifact)
}

pub fn persist_primary_secondary_policy_artifact(
    ledger: &ArtifactLedger,
    artifact: &PrimarySecondaryPolicyArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-primary-secondary-policy.json", artifact)
}

pub fn persist_primary_secondary_failover_artifact(
    ledger: &ArtifactLedger,
    artifact: &PrimarySecondaryFailoverArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-primary-secondary-failover.json", artifact)
}

pub fn persist_consultation_artifact(
    ledger: &ArtifactLedger,
    artifact: &ConsultationArtifact,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-consultation.json", artifact)
}

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

    let entries = match fs::read_dir(parent) {
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

        let Ok(raw) = fs::read_to_string(&path) else {
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
    let min_p = min_parallelism.max(1);
    let max_p = max_parallelism.max(min_p);
    let current = current.clamp(min_p, max_p);

    let latest_path = ledger.latest_path("spec", "latest-learning.json");
    let payload = match fs::read_to_string(&latest_path) {
        Ok(raw) => raw,
        Err(_) => return current,
    };
    let bus = match serde_json::from_str::<WorkflowLearningBusArtifact>(&payload) {
        Ok(value) => value,
        Err(_) => return current,
    };

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
    let latest_path = ledger.latest_path("spec", "latest-learning.json");
    let payload = match fs::read_to_string(&latest_path) {
        Ok(raw) => raw,
        Err(_) => return current.to_string(),
    };
    let bus = match serde_json::from_str::<WorkflowLearningBusArtifact>(&payload) {
        Ok(value) => value,
        Err(_) => return current.to_string(),
    };

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
/// Returns one of: ask/edit/agent/safeguard/full_auto.
pub fn recommend_work_grade_from_learning(ledger: &ArtifactLedger, current: &str) -> String {
    let latest_path = ledger.latest_path("spec", "latest-learning.json");
    let payload = match fs::read_to_string(&latest_path) {
        Ok(raw) => raw,
        Err(_) => return current.to_string(),
    };
    let bus = match serde_json::from_str::<WorkflowLearningBusArtifact>(&payload) {
        Ok(value) => value,
        Err(_) => return current.to_string(),
    };

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
///
/// This is a lightweight online regression: blend current heuristic score with
/// weighted historical success rates, where weights decay by complexity distance.
pub fn recommend_predicted_success_rate_from_learning(
    ledger: &ArtifactLedger,
    current: f32,
    target_complexity: u8,
) -> f32 {
    let latest_path = ledger.latest_path("spec", "latest-learning.json");
    let payload = match fs::read_to_string(&latest_path) {
        Ok(raw) => raw,
        Err(_) => return current,
    };
    let bus = match serde_json::from_str::<WorkflowLearningBusArtifact>(&payload) {
        Ok(value) => value,
        Err(_) => return current,
    };

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
    // 35% heuristic + 65% learned regression signal.
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

    let entries = match fs::read_dir(parent) {
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

        let Ok(raw) = fs::read_to_string(&path) else {
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

pub fn build_runtime_healthcheck_report(
    config_path: Option<&Path>,
    cache: Option<&ResponseCache>,
    vector_store: Option<&VectorStore>,
) -> Result<RuntimeHealthcheckReport> {
    let ledger = ArtifactLedger::new(config_path);
    let mut components = Vec::new();

    match ledger.ensure_ready() {
        Ok(()) => components.push(ComponentReport {
            name: "ledger".to_string(),
            status: CheckStatus::Healthy,
            message: "durable ledger is writable".to_string(),
            details: json!({ "root": ledger.root().display().to_string() }),
        }),
        Err(err) => components.push(ComponentReport {
            name: "ledger".to_string(),
            status: CheckStatus::Error,
            message: err.to_string(),
            details: json!({ "root": ledger.root().display().to_string() }),
        }),
    }

    if let Some(path) = config_path {
        match AppConfig::load(path).and_then(|config| validate_runtime_readiness(path, &config)) {
            Ok(report) => {
                let status = if report.critical_count > 0 {
                    CheckStatus::Error
                } else if report.warn_count > 0 || report.info_count > 0 {
                    CheckStatus::Warn
                } else {
                    CheckStatus::Healthy
                };
                components.push(ComponentReport {
                    name: "config".to_string(),
                    status,
                    message: format!(
                        "config score {}/100, profile {}",
                        report.score, report.profile_recommendation
                    ),
                    details: serde_json::to_value(&report).unwrap_or_else(|_| json!({})),
                });
            }
            Err(err) => components.push(ComponentReport {
                name: "config".to_string(),
                status: CheckStatus::Error,
                message: err.to_string(),
                details: json!({ "config_path": path.display().to_string() }),
            }),
        }
    } else {
        components.push(ComponentReport {
            name: "config".to_string(),
            status: CheckStatus::Skipped,
            message: "config path unavailable".to_string(),
            details: json!({}),
        });
    }

    if let Some(cache) = cache {
        match cache.entry_count() {
            Ok(entries) => components.push(ComponentReport {
                name: "cache".to_string(),
                status: CheckStatus::Healthy,
                message: format!("sqlite cache reachable with {} entries", entries),
                details: json!({ "entries": entries }),
            }),
            Err(err) => components.push(ComponentReport {
                name: "cache".to_string(),
                status: CheckStatus::Error,
                message: err.to_string(),
                details: json!({}),
            }),
        }
    } else {
        components.push(ComponentReport {
            name: "cache".to_string(),
            status: CheckStatus::Skipped,
            message: "cache disabled".to_string(),
            details: json!({}),
        });
    }

    if let Some(vector_store) = vector_store {
        match (
            vector_store.memory_entry_count(),
            vector_store.summary_entry_count(),
        ) {
            (Ok(memory_entries), Ok(summary_entries)) => components.push(ComponentReport {
                name: "vector".to_string(),
                status: CheckStatus::Healthy,
                message: format!(
                    "vector store reachable with {} memory entries and {} summaries",
                    memory_entries, summary_entries
                ),
                details: json!({
                    "memory_entries": memory_entries,
                    "summary_entries": summary_entries,
                }),
            }),
            (Err(err), _) | (_, Err(err)) => components.push(ComponentReport {
                name: "vector".to_string(),
                status: CheckStatus::Error,
                message: err.to_string(),
                details: json!({}),
            }),
        }
    } else {
        components.push(ComponentReport {
            name: "vector".to_string(),
            status: CheckStatus::Skipped,
            message: "vector store disabled".to_string(),
            details: json!({}),
        });
    }

    let overall_status = aggregate_status(components.iter().map(|component| component.status));
    Ok(RuntimeHealthcheckReport {
        generated_at: now_ts(),
        overall_status,
        components,
    })
}

pub fn persist_runtime_healthcheck(
    ledger: &ArtifactLedger,
    report: &RuntimeHealthcheckReport,
) -> Result<PathBuf> {
    ledger.write_json("qa", "latest-healthcheck.json", report)
}

pub fn run_action_check(
    ledger: &ArtifactLedger,
    kind: ActionCheckKind,
) -> Result<ActionCheckReport> {
    ledger.ensure_ready()?;
    let generated_at = now_ts();
    let mut checks_run = Vec::new();
    let mut evidence_refs = Vec::new();
    let mut overall_status = CheckStatus::Healthy;
    let mut retest_report_path = None;
    let mut final_summary_path = None;

    let include_spec = matches!(
        kind,
        ActionCheckKind::All | ActionCheckKind::Spec | ActionCheckKind::Retest
    );
    let include_qa = matches!(
        kind,
        ActionCheckKind::All | ActionCheckKind::Qa | ActionCheckKind::Retest
    );
    let include_checkpoint = matches!(kind, ActionCheckKind::All | ActionCheckKind::Retest);

    if include_spec {
        let item = check_json_artifact(
            ledger,
            "spec",
            "latest-plan.json",
            "spec_plan",
            &["task", "characteristics", "routing", "planned_subtasks"],
        );
        maybe_capture_evidence(&item, &mut evidence_refs);
        overall_status = overall_status.merge(item.status);
        checks_run.push(item);
    }

    if include_qa {
        let item = check_json_artifact(
            ledger,
            "qa",
            "latest-healthcheck.json",
            "qa_healthcheck",
            &["generated_at", "overall_status", "components"],
        );
        maybe_capture_evidence(&item, &mut evidence_refs);
        overall_status = overall_status.merge(item.status);
        checks_run.push(item);
    }

    if include_checkpoint {
        let item = check_json_artifact(
            ledger,
            "checkpoints",
            "latest.json",
            "recovery_checkpoint",
            &[
                "checkpoint_id",
                "conversation_id",
                "branch_id",
                "message_count",
            ],
        );
        maybe_capture_evidence(&item, &mut evidence_refs);
        overall_status = overall_status.merge(item.status);
        checks_run.push(item);
    }

    if matches!(kind, ActionCheckKind::All | ActionCheckKind::Retest) {
        let retest_report = ActionCheckReport {
            generated_at,
            kind: "retest".to_string(),
            overall_status,
            ok: overall_status != CheckStatus::Error,
            checks_run: checks_run.clone(),
            evidence_refs: evidence_refs.clone(),
            retest_report_path: None,
            final_summary_path: None,
        };
        let path = ledger.write_json("retest", "latest-action-check.json", &retest_report)?;
        retest_report_path = Some(path.display().to_string());
        evidence_refs.push(path.display().to_string());
    }

    if matches!(kind, ActionCheckKind::All | ActionCheckKind::Final) {
        let retest_item = check_json_artifact(
            ledger,
            "retest",
            "latest-action-check.json",
            "retest_report",
            &["generated_at", "checks_run", "evidence_refs", "ok"],
        );
        if kind == ActionCheckKind::Final {
            maybe_capture_evidence(&retest_item, &mut evidence_refs);
            overall_status = overall_status.merge(retest_item.status);
            checks_run.push(retest_item);
        }

        let summary = FinalSummaryArtifact {
            generated_at,
            overall_status,
            evidence_refs: evidence_refs.clone(),
            action_check_path: ledger
                .latest_path("retest", "latest-action-check.json")
                .display()
                .to_string(),
            conclusion: if overall_status == CheckStatus::Error {
                "final evidence chain is incomplete; fix failing checks before promotion"
                    .to_string()
            } else {
                "final evidence chain is complete enough for controlled promotion".to_string()
            },
        };
        let path = ledger.write_json("final", "latest-summary.json", &summary)?;
        final_summary_path = Some(path.display().to_string());
        evidence_refs.push(path.display().to_string());
    }

    let report = ActionCheckReport {
        generated_at,
        kind: kind.as_str().to_string(),
        overall_status,
        ok: overall_status != CheckStatus::Error,
        checks_run,
        evidence_refs,
        retest_report_path,
        final_summary_path,
    };

    let latest_name = format!("latest-{}.json", kind.as_str());
    let _ = ledger.write_json("action-checks", latest_name.as_str(), &report)?;
    Ok(report)
}

fn maybe_capture_evidence(item: &ActionCheckItem, evidence_refs: &mut Vec<String>) {
    if item.status != CheckStatus::Error {
        if let Some(path) = &item.artifact_path {
            evidence_refs.push(path.clone());
        }
    }
}

fn check_json_artifact(
    ledger: &ArtifactLedger,
    category: &str,
    latest_name: &str,
    label: &str,
    required_keys: &[&str],
) -> ActionCheckItem {
    let path = ledger.latest_path(category, latest_name);
    let artifact_path = path.display().to_string();

    let value = match fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str::<Value>(&content) {
            Ok(value) => value,
            Err(err) => {
                return ActionCheckItem {
                    name: label.to_string(),
                    status: CheckStatus::Error,
                    message: format!("artifact is not valid JSON: {}", err),
                    artifact_path: Some(artifact_path),
                }
            }
        },
        Err(err) => {
            return ActionCheckItem {
                name: label.to_string(),
                status: CheckStatus::Error,
                message: format!("artifact missing: {}", err),
                artifact_path: Some(artifact_path),
            }
        }
    };

    let missing = required_keys
        .iter()
        .filter(|key| value.get(**key).is_none())
        .map(|key| (*key).to_string())
        .collect::<Vec<_>>();

    if missing.is_empty() {
        ActionCheckItem {
            name: label.to_string(),
            status: CheckStatus::Healthy,
            message: "artifact structure verified".to_string(),
            artifact_path: Some(artifact_path),
        }
    } else {
        ActionCheckItem {
            name: label.to_string(),
            status: CheckStatus::Error,
            message: format!("artifact missing keys: {}", missing.join(", ")),
            artifact_path: Some(artifact_path),
        }
    }
}

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

pub fn aggregate_status<I>(statuses: I) -> CheckStatus
where
    I: IntoIterator<Item = CheckStatus>,
{
    statuses
        .into_iter()
        .fold(CheckStatus::Healthy, |acc, status| acc.merge(status))
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn assistant_excerpt(messages: &[crate::agent::Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .map(|message| trim_chars(&message.content, 240))
}

pub fn total_message_chars(messages: &[crate::agent::Message]) -> usize {
    messages
        .iter()
        .map(|message| message.content.chars().count())
        .sum()
}

fn trim_chars(text: &str, max_chars: usize) -> String {
    let mut result = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        result.push_str("...");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    #[test]
    fn task_plan_recommends_sub_agents_for_complex_work() {
        let plan =
            build_task_plan("design a complex multi-module feature with verification and review");
        assert!(plan.sub_agent_recommended);
        assert!(!plan.planned_subtasks.is_empty());
    }

    #[test]
    fn healthcheck_persists_to_ledger() {
        let temp = tempdir().expect("tempdir should be created");
        let config_path = temp.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
default_phase = "coding"
model_selection_mode = "manual"

[flow]
phases = ["coding"]

[agents.copilot]
provider = "copilot"
api_key = "env://COPILOT_API_KEY"
model = "gpt-4"

[phases.coding]
description = "coding phase"
agents = ["copilot"]
fallback = true
"#,
        )
        .expect("config file should be written");

        let ledger = ArtifactLedger::new(Some(&config_path));
        let report = build_runtime_healthcheck_report(Some(&config_path), None, None)
            .expect("healthcheck should build");
        let path =
            persist_runtime_healthcheck(&ledger, &report).expect("healthcheck should persist");
        assert!(path.exists());
    }

    #[test]
    fn action_check_generates_retest_and_final_artifacts() {
        let temp = tempdir().expect("tempdir should be created");
        let config_path = temp.path().join("config.toml");
        let ledger = ArtifactLedger::new(Some(&config_path));

        let plan = build_task_plan("fix complex bug across multiple modules with tests");
        persist_task_plan(&ledger, &plan).expect("task plan should persist");

        let health = RuntimeHealthcheckReport {
            generated_at: now_ts(),
            overall_status: CheckStatus::Healthy,
            components: vec![],
        };
        persist_runtime_healthcheck(&ledger, &health).expect("healthcheck should persist");

        let checkpoint = CheckpointSummaryArtifact {
            checkpoint_id: "cp-1".to_string(),
            conversation_id: "conv-1".to_string(),
            branch_id: "main".to_string(),
            parent_checkpoint_id: None,
            created_at: now_ts(),
            note: Some("coding/copilot".to_string()),
            message_count: 2,
            message_chars: 12,
            assistant_excerpt: Some("hello".to_string()),
        };
        ledger
            .write_json("checkpoints", "latest.json", &checkpoint)
            .expect("checkpoint should persist");

        let report =
            run_action_check(&ledger, ActionCheckKind::All).expect("action check should succeed");
        assert!(report.ok);
        assert!(ledger
            .latest_path("retest", "latest-action-check.json")
            .exists());
        assert!(ledger.latest_path("final", "latest-summary.json").exists());
    }

    #[test]
    fn knowledge_bus_dedup_discards_lower_confidence_event() {
        let temp = tempdir().expect("tempdir should be created");
        let config_path = temp.path().join("config.toml");
        let ledger = ArtifactLedger::new(Some(&config_path));

        let high = KnowledgeInsightArtifact {
            generated_at: now_ts(),
            conversation_id: "c1".to_string(),
            branch_id: "main".to_string(),
            phase: "coding".to_string(),
            task: "fix-bug".to_string(),
            agent: "copilot".to_string(),
            source: "test".to_string(),
            request_excerpt: "req".to_string(),
            response_excerpt: "resp-high".to_string(),
            reusable_insights: vec!["insight-high".to_string()],
            verification_steps: vec![],
            confidence: 0.9,
        };
        persist_knowledge_insight_event(&ledger, high, 100)
            .expect("high-confidence event should persist");

        let low = KnowledgeInsightArtifact {
            generated_at: now_ts(),
            conversation_id: "c2".to_string(),
            branch_id: "main".to_string(),
            phase: "coding".to_string(),
            task: "fix-bug".to_string(),
            agent: "copilot".to_string(),
            source: "test".to_string(),
            request_excerpt: "req".to_string(),
            response_excerpt: "resp-low".to_string(),
            reusable_insights: vec!["insight-low".to_string()],
            verification_steps: vec![],
            confidence: 0.4,
        };
        persist_knowledge_insight_event(&ledger, low, 100)
            .expect("low-confidence event should persist without error");

        let path = ledger.latest_path("spec", "latest-knowledge.json");
        let raw = fs::read_to_string(&path).expect("knowledge file should exist");
        let bus: KnowledgeBusArtifact =
            serde_json::from_str(&raw).expect("should deserialize knowledge bus");
        // Bus should have exactly 1 event and it must be the high-confidence one.
        assert_eq!(bus.events.len(), 1);
        assert!((bus.events[0].confidence - 0.9).abs() < f64::EPSILON);
        assert!(bus.events[0].reusable_insights.contains(&"insight-high".to_string()));
    }

    #[test]
    fn knowledge_bus_dedup_replaces_with_higher_confidence_event() {
        let temp = tempdir().expect("tempdir should be created");
        let config_path = temp.path().join("config.toml");
        let ledger = ArtifactLedger::new(Some(&config_path));

        let low = KnowledgeInsightArtifact {
            generated_at: now_ts(),
            conversation_id: "c1".to_string(),
            branch_id: "main".to_string(),
            phase: "coding".to_string(),
            task: "refactor".to_string(),
            agent: "copilot".to_string(),
            source: "test".to_string(),
            request_excerpt: "req".to_string(),
            response_excerpt: "resp-old".to_string(),
            reusable_insights: vec!["old-insight".to_string()],
            verification_steps: vec![],
            confidence: 0.5,
        };
        persist_knowledge_insight_event(&ledger, low, 100).expect("low event should persist");

        let high = KnowledgeInsightArtifact {
            generated_at: now_ts(),
            conversation_id: "c2".to_string(),
            branch_id: "main".to_string(),
            phase: "coding".to_string(),
            task: "refactor".to_string(),
            agent: "copilot".to_string(),
            source: "test".to_string(),
            request_excerpt: "req".to_string(),
            response_excerpt: "resp-new".to_string(),
            reusable_insights: vec!["new-insight".to_string()],
            verification_steps: vec![],
            confidence: 0.95,
        };
        persist_knowledge_insight_event(&ledger, high, 100).expect("high event should persist");

        let path = ledger.latest_path("spec", "latest-knowledge.json");
        let raw = fs::read_to_string(&path).expect("knowledge file should exist");
        let bus: KnowledgeBusArtifact =
            serde_json::from_str(&raw).expect("should deserialize knowledge bus");
        assert_eq!(bus.events.len(), 1);
        assert!((bus.events[0].confidence - 0.95).abs() < f64::EPSILON);
        assert!(bus.events[0].reusable_insights.contains(&"new-insight".to_string()));
    }
}
