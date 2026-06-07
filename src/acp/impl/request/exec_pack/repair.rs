//! Repair loop types and functions for workflow execution.
//! Implements autonomous iterative repair capabilities for failed subtasks.

use serde::Serialize;
use serde_json::{json, Value};

/// Auto-Repair Loop Support for Step 2 of BLUE22
#[derive(Debug, Clone)]
pub(crate) struct RepairContext {
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
pub(crate) struct RepairCycleReport {
    pub(super) iteration: u32,
    pub(super) failed_before: usize,
    pub(super) failed_after: usize,
    pub(super) actions_applied: usize,
    pub(super) result: String,
    pub(super) diagnosis: String,
    pub(super) strategy_adjustment: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RepairAction {
    pub(super) iteration: u32,
    pub(super) action_type: String,
    pub(super) target_subtask_id: String,
    pub(super) description: String,
    pub(super) applied_at: i64,
    pub(super) result: String,
    pub(super) details: Value,
}

pub(crate) fn should_trigger_auto_repair(
    failure_count: usize,
    failure_classes: &[String],
    governance_config: Option<&Value>,
) -> bool {
    if failure_count == 0 {
        return false;
    }

    let repairable_classes = [
        "execution_subtask_failed",
        "subtask_retry_eligible",
        "execution_timeout_recoverable",
        "execution_transient_error",
    ];

    let has_repairable = failure_classes
        .iter()
        .any(|cls| repairable_classes.contains(&cls.as_str()));

    if !has_repairable {
        return false;
    }

    let auto_repair_enabled = governance_config
        .and_then(|cfg| cfg.get("auto_repair_enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(true);

    auto_repair_enabled
}

pub(crate) fn build_repair_context(
    task_id: String,
    failure_classes: Vec<String>,
    governance_config: Option<&Value>,
) -> RepairContext {
    let max_iterations = governance_config
        .and_then(|cfg| cfg.get("auto_repair_max_iterations"))
        .and_then(Value::as_u64)
        .unwrap_or(2)
        .min(3) as u32;

    let budget_tokens = governance_config
        .and_then(|cfg| cfg.get("auto_repair_budget_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(4000);

    let budget_time_seconds = governance_config
        .and_then(|cfg| cfg.get("auto_repair_budget_seconds"))
        .and_then(Value::as_u64)
        .unwrap_or(180);

    let governance_mode = governance_config
        .and_then(|cfg| cfg.get("auto_repair_mode"))
        .and_then(Value::as_str)
        .unwrap_or("assisted")
        .to_string();

    RepairContext {
        iteration: 1,
        max_iterations,
        task_id,
        failure_classes,
        budget_tokens,
        budget_time_seconds,
        governance_mode,
        repair_actions: Vec::new(),
        cycle_reports: Vec::new(),
    }
}

pub(crate) fn record_repair_action(
    context: &mut RepairContext,
    action_type: &str,
    target_subtask_id: String,
    description: String,
    result: &str,
    details: Value,
) {
    context.repair_actions.push(RepairAction {
        iteration: context.iteration,
        action_type: action_type.to_string(),
        target_subtask_id,
        description,
        applied_at: crate::acp::prelude::now_ts(),
        result: result.to_string(),
        details,
    });
}

pub(crate) fn evaluate_repair_termination_criteria(
    context: &RepairContext,
    start_time_ms: u64,
) -> (bool, String) {
    if context.iteration >= context.max_iterations {
        return (
            true,
            format!("reached max iterations ({})", context.max_iterations),
        );
    }

    let elapsed_ms = crate::acp::prelude::now_ts_ms() as u64 - start_time_ms;
    let budget_ms = context.budget_time_seconds * 1000;
    if elapsed_ms > budget_ms {
        return (
            true,
            format!("exceeded time budget ({} > {}ms)", elapsed_ms, budget_ms),
        );
    }

    let estimated_tokens_per_action = 500;
    let estimated_total_tokens = context.repair_actions.len() as u64 * estimated_tokens_per_action;
    if estimated_total_tokens > context.budget_tokens {
        return (
            true,
            format!(
                "exceeded token budget ({} > {})",
                estimated_total_tokens, context.budget_tokens
            ),
        );
    }

    (false, "within budget".to_string())
}

pub(crate) fn should_continue_repair_loop(
    context: &RepairContext,
    failed_subtask_count: usize,
    start_time_ms: u64,
) -> bool {
    if failed_subtask_count == 0 {
        return false;
    }

    let (should_terminate, _reason) = evaluate_repair_termination_criteria(context, start_time_ms);
    !should_terminate
}

pub(crate) fn build_repair_loop_state(
    context: &RepairContext,
    failed_count: usize,
    start_time_ms: u64,
) -> Value {
    let (should_terminate, termination_reason) =
        evaluate_repair_termination_criteria(context, start_time_ms);

    json!({
        "iteration": context.iteration,
        "max_iterations": context.max_iterations,
        "failed_subtasks_pending": failed_count,
        "repair_actions_executed": context.repair_actions.len(),
        "should_continue": !should_terminate && failed_count > 0,
        "termination_reason": if should_terminate {
            termination_reason
        } else if failed_count == 0 {
            "all subtasks passed".to_string()
        } else {
            "continue repair loop".to_string()
        },
        "governance_mode": context.governance_mode.clone(),
        "budget_tokens_used": (context.repair_actions.len() as u64 * 500).min(context.budget_tokens),
        "budget_tokens_limit": context.budget_tokens,
    })
}

pub(crate) fn build_repair_history_response(context: &RepairContext) -> Value {
    let total_cycles = context.cycle_reports.len() as u64;
    let resolved_cycles = context
        .cycle_reports
        .iter()
        .filter(|cycle| cycle.result == "resolved")
        .count() as u64;
    let improved_cycles = context
        .cycle_reports
        .iter()
        .filter(|cycle| cycle.result == "improved")
        .count() as u64;
    let unresolved_cycles = context
        .cycle_reports
        .iter()
        .filter(|cycle| cycle.result == "unresolved")
        .count() as u64;
    let repair_effective_ratio = if total_cycles == 0 {
        0.0
    } else {
        (resolved_cycles + improved_cycles) as f64 / total_cycles as f64
    };
    let replan_required = unresolved_cycles > 0;
    crate::acp::helpers::autonomy_metrics::record_repair_replan_decision(replan_required);

    let diagnosis_summary = json!({
        "total_actions": context.repair_actions.len(),
        "successful_actions": context.repair_actions.iter().filter(|a| a.result == "success").count(),
        "failed_actions": context.repair_actions.iter().filter(|a| a.result == "failed").count(),
        "total_cycles": total_cycles,
        "resolved_cycles": resolved_cycles,
        "improved_cycles": improved_cycles,
        "unresolved_cycles": unresolved_cycles,
        "repair_effective_ratio": repair_effective_ratio,
        "replan_required": replan_required,
        "next_action_hint": if replan_required {
            "promote remaining failures from retry to reroute/replan"
        } else {
            "continue execution flow"
        },
        "top_failure_class": context.failure_classes.first().cloned().unwrap_or_else(|| "unknown".to_string()),
        "latest_result": context
            .cycle_reports
            .last()
            .map(|cycle| cycle.result.clone())
            .unwrap_or_else(|| "not_started".to_string()),
    });

    json!({
        "iteration": context.iteration,
        "max_iterations": context.max_iterations,
        "task_id": context.task_id,
        "failure_classes": context.failure_classes,
        "governance_mode": context.governance_mode,
        "actions_count": context.repair_actions.len(),
        "cycles": context.cycle_reports,
        "diagnosis_summary": diagnosis_summary,
        "actions": context.repair_actions.iter().map(|action| json!({
            "iteration": action.iteration,
            "type": action.action_type,
            "subtask_id": action.target_subtask_id,
            "description": action.description,
            "applied_at": action.applied_at,
            "result": action.result,
            "details": action.details,
        })).collect::<Vec<_>>(),
    })
}
