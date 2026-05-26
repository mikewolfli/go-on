use super::*;

#[derive(Debug, Clone)]
pub(super) struct RepairContext {
    pub(super) iteration: u32,
    pub(super) max_iterations: u32,
    pub(super) task_id: String,
    pub(super) failure_classes: Vec<String>,
    pub(super) budget_tokens: u64,
    pub(super) budget_time_seconds: u64,
    pub(super) governance_mode: String,
    pub(super) repair_actions: Vec<RepairAction>,
    pub(super) cycle_reports: Vec<RepairCycleReport>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct RepairCycleReport {
    pub(super) iteration: u32,
    pub(super) failed_before: usize,
    pub(super) failed_after: usize,
    pub(super) actions_applied: usize,
    pub(super) result: String,
    pub(super) diagnosis: String,
    pub(super) strategy_adjustment: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct RepairAction {
    pub(super) iteration: u32,
    pub(super) action_type: String,
    pub(super) target_subtask_id: String,
    pub(super) description: String,
    pub(super) applied_at: i64,
    pub(super) result: String,
    pub(super) details: Value,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct WorkflowRunRecord {
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

#[derive(Clone)]
pub(super) struct RuntimeExecutionContext {
    pub(super) task_timeout_seconds: Option<u64>,
    pub(super) task_parallelism_cap: usize,
    pub(super) principles: Option<Vec<String>>,
    pub(super) base_options: HashMap<String, Value>,
    pub(super) app_config: Arc<AppConfig>,
    pub(super) primary_agent: String,
    pub(super) secondary_agents: Vec<String>,
    pub(super) candidates: Vec<(String, Arc<dyn crate::agent::Agent>)>,
    pub(super) failure_strategy: String,
    pub(super) adaptive_selector: Arc<StdMutex<crate::adaptive_selector::AdaptiveModelSelector>>,
    pub(super) online_controller: Arc<StdMutex<crate::acp::prelude::OnlineControllerState>>,
    pub(super) failure_prevention: Arc<StdMutex<crate::failure_prevention::FailurePrevention>>,
    pub(super) metrics: Arc<crate::acp::prelude::RuntimeMetrics>,
    pub(super) memory_store: Arc<StdMutex<MemoryStore>>,
    pub(super) lazy_policy: LazyLoadPolicy,
    pub(super) adaptive_defaults: AdaptiveExecutionDefaults,
    pub(super) artifact_ledger: ArtifactLedger,
    pub(super) vector_store: Option<Arc<VectorStore>>,
    pub(super) orchestration_ctx: Arc<OrchestrationContext>,
}

#[derive(Clone, Serialize)]
pub(super) struct AdaptiveExecutionDefaults {
    pub(super) recommended_failure_strategy: String,
    pub(super) applied_failure_strategy: String,
    pub(super) failure_strategy_from_learning: bool,
    pub(super) recommended_mode: String,
    pub(super) applied_mode: String,
    pub(super) mode_from_learning: bool,
    pub(super) filtered_unavailable_agents: Vec<String>,
    pub(super) hardness: HardnessProfile,
    pub(super) cost: TokenCostGovernanceProfile,
}

#[derive(Clone, Serialize)]
pub(super) struct AdaptivePlanningReport {
    pub(super) predicted_success_before: f32,
    pub(super) predicted_success_after: f32,
    pub(super) parallelism_before: usize,
    pub(super) recommended_parallelism: usize,
    pub(super) parallelism_after: usize,
}

pub(super) struct RuntimeExecutionReport {
    pub(super) assignment_records: Vec<ExecutionAssignmentRecord>,
    pub(super) subtasks_completed: usize,
    pub(super) subtasks_failed: usize,
    pub(super) subtasks_skipped: usize,
    pub(super) subtask_parallelism: usize,
    pub(super) phases_executed: usize,
    pub(super) halted_early: bool,
    pub(super) parallel_utilization: f64,
    pub(super) parallel_failure_rollback_count: usize,
    pub(super) serial_work_ms: u64,
    pub(super) critical_path_ms: u64,
    pub(super) parallel_efficiency: f64,
    pub(super) parallel_speedup: f64,
    pub(super) failure_strategy: String,
    pub(super) failover_count: usize,
    pub(super) failover_root_cause: String,
    pub(super) lazy_load: LazyLoadExecutionReport,
}

pub(super) struct SubtaskRunResult {
    pub(super) record_index: usize,
    pub(super) duration_ms: u64,
    pub(super) executor: String,
    pub(super) success: bool,
    pub(super) failover_applied: bool,
    pub(super) failover_reason: Option<String>,
    pub(super) desired_role: Option<String>,
    pub(super) candidate_scores: Vec<ExecutionDecisionCandidate>,
    pub(super) response_excerpt: String,
    pub(super) tool_loop_used: bool,
    pub(super) tool_observations: Vec<String>,
    #[allow(dead_code)] // F-GAP-17 — reserved for self-rationalization audit trail
    pub(super) audit_log_json: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct LazyLoadPolicy {
    pub(super) enable_tool_loop: bool,
    pub(super) enable_role_collaboration: bool,
    pub(super) enable_memory_policy: bool,
    pub(super) activation_reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct LazyLoadExecutionReport {
    pub(super) policy: LazyLoadPolicy,
    pub(super) tool_loop_runs: usize,
    pub(super) role_routed_subtasks: usize,
    pub(super) memory_entries_written: usize,
    pub(super) memory_entries_retained: usize,
    pub(super) memory_artifact_path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct MemoryPolicyExecutionArtifact {
    pub(super) generated_at: i64,
    pub(super) task: String,
    pub(super) policy: LazyLoadPolicy,
    pub(super) total_entries_before_gc: usize,
    pub(super) retained_entries_after_gc: usize,
    pub(super) sample_observations: Vec<String>,
}
