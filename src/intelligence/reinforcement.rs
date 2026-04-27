//! BLUE2 reinforcement utilities.
//!
//! This is a re-export facade for the sub-modules under `reinforcement/`.
//! All public items from the original monolithic module are re-exported here
//! to preserve backward-compatible paths like `crate::reinforcement::*`.
//!
//! The sub-modules are:
//! - [`health`](reinforcement/health/index.html) — health checks, component reports
//! - [`action_check`](reinforcement/action_check/index.html) — spec/QA/retest/final verification gates
//! - [`task_plan`](reinforcement/task_plan/index.html) — plan decomposition, execution tracking, workflow generation
//! - [`learning`](reinforcement/learning/index.html) — feedback collection, pattern analysis, Q-learning

pub mod reinforcement;

pub use reinforcement::action_check::{
    ActionCheckItem, ActionCheckKind, ActionCheckReport, FinalSummaryArtifact, run_action_check,
};
pub use reinforcement::health::{
    aggregate_status, build_runtime_healthcheck_report, persist_runtime_healthcheck, CheckStatus,
    ComponentReport, RuntimeHealthcheckReport,
};
pub use reinforcement::learning::{
    persist_knowledge_insight_event, persist_workflow_learning_event, ExperienceKnowledgeBase,
    FailurePattern, KnowledgeBusArtifact, KnowledgeInsightArtifact, LearningFeedbackSystem,
    LearningPattern, QLearningAgent, RewardFunction, RlTaskExecutionMetrics, SuccessCase,
    WorkflowLearningBusArtifact, WorkflowLearningEvent,
};
pub use reinforcement::task_plan::{
    build_task_plan, build_workflow_generated_artifact, load_task_graph_checkpoint,
    persist_clarification_session_artifact, persist_consultation_artifact,
    persist_execution_decision, persist_governance_policy, persist_pipeline_unified_metrics,
    persist_primary_secondary_failover_artifact, persist_primary_secondary_policy_artifact,
    persist_requirement_contract, persist_task_execution_summary, persist_task_graph_checkpoint,
    persist_task_plan, persist_workflow_generated, persist_workflow_optimization_policy,
    persist_workflow_research, persist_workflow_work_grade,
    recommend_agent_order_from_execution_history,
    recommend_failure_strategy_from_learning, recommend_parallelism_from_learning,
    recommend_predicted_success_rate_from_learning, recommend_reattach_modules_from_policy_history,
    recommend_work_grade_from_learning, CheckpointSummaryArtifact, ClarificationSessionArtifact,
    ConsultationArtifact, ExecutionAssignmentRecord, ExecutionDecisionArtifact,
    ExecutionDecisionCandidate, GovernancePolicyArtifact, ParallelPhaseDecisionRecord,
    PipelineUnifiedMetricsArtifact, PlannedSubtaskRecord, PrimaryFailoverReportItem,
    PrimarySecondaryFailoverArtifact, PrimarySecondaryPolicyArtifact,
    RequirementContractArtifact, TaskExecutionMetrics, TaskExecutionSummary,
    TaskGraphCheckpointArtifact, TaskPlanArtifact, WorkflowEdge, WorkflowGeneratedArtifact,
    WorkflowNode, WorkflowOptimizationPolicyArtifact, WorkflowResearchArtifact,
    WorkflowWorkGradeArtifact,
};

// ── Re-export ArtifactLedger from its canonical location ───────────────────

pub use reinforcement::ArtifactLedger;
```
Wait, I need to think about this more carefully. The `ArtifactLedger` and `now_ts` and `assistant_excerpt` and `total_message_chars` and `trim_chars` functions were originally in the root of `reinforcement.rs`. The `ArtifactLedger` is used across the sub-modules and externally.

Let me place `ArtifactLedger` in a shared place. I'll put it directly in the top-level facade with a `mod` declaration. Actually, the cleanest approach is to put common shared items directly in the top-level file.

Let me reconsider the structure. The `ArtifactLedger` is used by all submodules, and `now_ts()`, `assistant_excerpt()`, `total_message_chars()` are public utilities used by external code. I should move `ArtifactLedger`, `now_ts` into a shared module within `reinforcement/`.

Let me put the shared utilities (ArtifactLedger, now_ts, assistant_excerpt, total_message_chars) into a `shared.rs` sub-module:
