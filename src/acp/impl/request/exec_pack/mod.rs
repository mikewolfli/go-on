//! exec_pack -- Workflow and task execution module.
//!
//! Split from the monolithic `exec_pack.rs` into sub-modules:
//! - `workflow`: Workflow run tracking & CRUD handlers
//! - `repair`:   Auto-repair types and pure repair logic
//! - `execute`:  `handle_workflow_execute` and repair-loop integration
//! - `task`:     `handle_task_execute`, context, subtask execution, tool helpers
//! - `pua`:      PUA gate evaluation (agent availability filtering)
//! - `artifact`: Artifact creation builders
//! - `requirement`: Requirement gate namespace

pub(super) mod artifact;
pub(super) mod execute;
pub(super) mod pua;
pub(super) mod repair;
pub(super) mod requirement;
pub(super) mod task;
pub(super) mod workflow;

// ── Import everything from parent `request.rs` so sub-modules
//     can access sibling-pack items via `super::<name>`.
use super::*;

// ── Re-exports for parent module (`request.rs` `use self::exec_pack::*`) ──

#[allow(unused_imports)] // re-exports used by sibling packs via super::exec_pack::*
pub(super) use workflow::{
    complete_workflow_run, handle_workflow_run_cancel, handle_workflow_run_get,
    handle_workflow_run_list, handle_workflow_run_pause, handle_workflow_run_resume,
    start_workflow_run, workflow_run_get_payload, workflow_run_list_payload,
    workflow_run_transition_payload, WorkflowRunRecord,
};

#[allow(unused_imports)] // re-exports used by sibling packs
pub(super) use repair::{
    build_repair_context, build_repair_history_response, build_repair_loop_state,
    evaluate_repair_termination_criteria, record_repair_action, should_trigger_auto_repair,
    RepairAction, RepairContext, RepairCycleReport,
};

pub(crate) use execute::handle_workflow_execute;

#[allow(unused_imports)] // re-exports used by sibling packs and request.rs tests
pub(super) use task::{
    apply_learning_plan_feedback, handle_task_execute, infer_workflow_parallelism,
    rebalance_execution_order, AdaptiveExecutionDefaults, AdaptivePlanningReport,
    LazyLoadExecutionReport, LazyLoadPolicy, MemoryPolicyExecutionArtifact,
    RuntimeExecutionContext, RuntimeExecutionReport,
};

#[allow(unused_imports)]
pub(super) use pua::filter_unavailable_agents;

#[allow(unused_imports)] // re-exports used by sibling packs
pub(super) use artifact::{
    build_memory_graph_profile, build_multi_agent_sessions, build_replay_scoring,
    build_review_adjudication,
};
