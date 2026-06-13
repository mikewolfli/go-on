use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

static PLANNER_GUIDED_ROUTE_TOTAL: AtomicU64 = AtomicU64::new(0);
static EXPLICIT_TOOL_ROUTE_TOTAL: AtomicU64 = AtomicU64::new(0);
static REQUIREMENT_AUTO_RECOVERY_TOTAL: AtomicU64 = AtomicU64::new(0);
static REQUIREMENT_HUMAN_CONFIRMATION_TOTAL: AtomicU64 = AtomicU64::new(0);
static ORCHESTRATION_ALIGNMENT_HIGH_TOTAL: AtomicU64 = AtomicU64::new(0);
static ORCHESTRATION_ALIGNMENT_LOW_TOTAL: AtomicU64 = AtomicU64::new(0);
static IDEMPOTENCY_HIT_TOTAL: AtomicU64 = AtomicU64::new(0);
static REPAIR_CYCLE_RESOLVED_TOTAL: AtomicU64 = AtomicU64::new(0);
static REPAIR_CYCLE_IMPROVED_TOTAL: AtomicU64 = AtomicU64::new(0);
static REPAIR_CYCLE_UNRESOLVED_TOTAL: AtomicU64 = AtomicU64::new(0);
static REPAIR_REPLAN_DECISION_TOTAL: AtomicU64 = AtomicU64::new(0);
static REPAIR_REPLAN_REQUIRED_TOTAL: AtomicU64 = AtomicU64::new(0);
static ORCHESTRATION_NODE_MAPPED_TOTAL: AtomicU64 = AtomicU64::new(0);
static ORCHESTRATION_NODE_UNMAPPED_TOTAL: AtomicU64 = AtomicU64::new(0);
static AUTONOMY_LOOP_STOP_COMPLETE_TOTAL: AtomicU64 = AtomicU64::new(0);
static AUTONOMY_LOOP_STOP_FAILED_TOTAL: AtomicU64 = AtomicU64::new(0);
static AUTONOMY_LOOP_STOP_ESCALATED_TOTAL: AtomicU64 = AtomicU64::new(0);
static AUTONOMY_LOOP_STOP_INCOMPLETE_TOTAL: AtomicU64 = AtomicU64::new(0);
static TOOL_FOLLOWUP_ATTEMPT_TOTAL: AtomicU64 = AtomicU64::new(0);
static TOOL_FOLLOWUP_SUCCESS_TOTAL: AtomicU64 = AtomicU64::new(0);
static TOOL_FOLLOWUP_FALLBACK_TOTAL: AtomicU64 = AtomicU64::new(0);
static CACHE_BYPASS_FOR_EXECUTION_TOTAL: AtomicU64 = AtomicU64::new(0);
static CACHE_SHORTCIRCUIT_REFUSED_TOTAL: AtomicU64 = AtomicU64::new(0);
static CACHE_SHORTCIRCUIT_EXECUTION_LIKE_TOTAL: AtomicU64 = AtomicU64::new(0);
static CAPABILITY_SELECTION_APPLIED_TOTAL: AtomicU64 = AtomicU64::new(0);
static CAPABILITY_SELECTION_NO_MATCH_TOTAL: AtomicU64 = AtomicU64::new(0);
static CAPABILITY_SELECTION_NONE_TOTAL: AtomicU64 = AtomicU64::new(0);
static VOTE_WINNER_STRONG_TOTAL: AtomicU64 = AtomicU64::new(0);
static VOTE_WINNER_ESCALATION_TOTAL: AtomicU64 = AtomicU64::new(0);
static FALLBACK_UNHEALTHY_AGENT_TOTAL: AtomicU64 = AtomicU64::new(0);
static REPUTATION_ROUTING_APPLIED_TOTAL: AtomicU64 = AtomicU64::new(0);
static VOTE_REPUTATION_TIEBREAK_TOTAL: AtomicU64 = AtomicU64::new(0);
static PARALLEL_TOOL_FANOUT_CALLS_TOTAL: AtomicU64 = AtomicU64::new(0);
static PARALLEL_TOOL_FANOUT_BATCH_TOTAL: AtomicU64 = AtomicU64::new(0);
static AGENT_SWITCH_TOTAL: AtomicU64 = AtomicU64::new(0);
static AGENT_SWITCH_BY_FAILURE_TOTAL: AtomicU64 = AtomicU64::new(0);
static AGENT_SWITCH_BY_REPUTATION_TOTAL: AtomicU64 = AtomicU64::new(0);

pub(crate) fn record_planner_guided_route() {
    PLANNER_GUIDED_ROUTE_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_explicit_tool_route() {
    EXPLICIT_TOOL_ROUTE_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_requirement_auto_recovery() {
    REQUIREMENT_AUTO_RECOVERY_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_requirement_human_confirmation() {
    REQUIREMENT_HUMAN_CONFIRMATION_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_orchestration_alignment(coverage_ratio: f64) {
    if coverage_ratio >= 0.6 {
        ORCHESTRATION_ALIGNMENT_HIGH_TOTAL.fetch_add(1, Ordering::Relaxed);
    } else {
        ORCHESTRATION_ALIGNMENT_LOW_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) fn record_repair_cycle_result(result: &str) {
    match result {
        "resolved" => {
            REPAIR_CYCLE_RESOLVED_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        "improved" => {
            REPAIR_CYCLE_IMPROVED_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        _ => {
            REPAIR_CYCLE_UNRESOLVED_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub(crate) fn record_repair_replan_decision(replan_required: bool) {
    REPAIR_REPLAN_DECISION_TOTAL.fetch_add(1, Ordering::Relaxed);
    if replan_required {
        REPAIR_REPLAN_REQUIRED_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) fn record_orchestration_node_mapping(mapped_nodes: u64, unmapped_nodes: u64) {
    ORCHESTRATION_NODE_MAPPED_TOTAL.fetch_add(mapped_nodes, Ordering::Relaxed);
    ORCHESTRATION_NODE_UNMAPPED_TOTAL.fetch_add(unmapped_nodes, Ordering::Relaxed);
}

pub(crate) fn record_autonomy_loop_stop_reason(reason: &str) {
    match reason {
        "complete" => {
            AUTONOMY_LOOP_STOP_COMPLETE_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        "failed" => {
            AUTONOMY_LOOP_STOP_FAILED_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        "escalated" => {
            AUTONOMY_LOOP_STOP_ESCALATED_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        _ => {
            AUTONOMY_LOOP_STOP_INCOMPLETE_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub(crate) fn record_tool_followup_attempt() {
    TOOL_FOLLOWUP_ATTEMPT_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_tool_followup_success() {
    TOOL_FOLLOWUP_SUCCESS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_tool_followup_fallback() {
    TOOL_FOLLOWUP_FALLBACK_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_cache_bypass_for_execution() {
    CACHE_BYPASS_FOR_EXECUTION_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_capability_selection_reason(reason: &str) {
    match reason {
        "capability_bus_selected" => {
            CAPABILITY_SELECTION_APPLIED_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        "capability_bus_no_match" => {
            CAPABILITY_SELECTION_NO_MATCH_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        _ => {
            CAPABILITY_SELECTION_NONE_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub(crate) fn record_vote_winner(kind: &str) {
    match kind {
        "multi_agent_strong_model_vote" => {
            VOTE_WINNER_STRONG_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        "multi_agent_multi_model_escalation" => {
            VOTE_WINNER_ESCALATION_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }
}

pub(crate) fn record_fallback_reason(reason: &str) {
    if reason == "all_agents_unhealthy" {
        FALLBACK_UNHEALTHY_AGENT_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) fn record_reputation_routing_applied() {
    REPUTATION_ROUTING_APPLIED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_vote_reputation_tiebreak() {
    VOTE_REPUTATION_TIEBREAK_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_parallel_tool_fanout(batch_size: u64) {
    PARALLEL_TOOL_FANOUT_CALLS_TOTAL.fetch_add(1, Ordering::Relaxed);
    PARALLEL_TOOL_FANOUT_BATCH_TOTAL.fetch_add(batch_size, Ordering::Relaxed);
}

pub(crate) fn record_agent_switch(reason: &str) {
    AGENT_SWITCH_TOTAL.fetch_add(1, Ordering::Relaxed);
    match reason {
        "failure" => {
            AGENT_SWITCH_BY_FAILURE_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        "reputation" => {
            AGENT_SWITCH_BY_REPUTATION_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }
}

/// Record that a cache hit was found but refused because the request is
/// execution-like and may have side effects (AUTON-03 criterion 3).
/// The `reason` categorizes the refusal for governance.status observability.
pub(crate) fn record_cache_shortcircuit_refused(reason: &str) {
    CACHE_SHORTCIRCUIT_REFUSED_TOTAL.fetch_add(1, Ordering::Relaxed);
    if reason == "execution_like_request" {
        CACHE_SHORTCIRCUIT_EXECUTION_LIKE_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) fn autonomy_metrics_snapshot() -> Value {
    let planner_guided = PLANNER_GUIDED_ROUTE_TOTAL.load(Ordering::Relaxed);
    let explicit = EXPLICIT_TOOL_ROUTE_TOTAL.load(Ordering::Relaxed);
    let auto_recovery = REQUIREMENT_AUTO_RECOVERY_TOTAL.load(Ordering::Relaxed);
    let human_confirmation = REQUIREMENT_HUMAN_CONFIRMATION_TOTAL.load(Ordering::Relaxed);
    let alignment_high = ORCHESTRATION_ALIGNMENT_HIGH_TOTAL.load(Ordering::Relaxed);
    let alignment_low = ORCHESTRATION_ALIGNMENT_LOW_TOTAL.load(Ordering::Relaxed);
    let idempotency_hits = IDEMPOTENCY_HIT_TOTAL.load(Ordering::Relaxed);
    let repair_resolved = REPAIR_CYCLE_RESOLVED_TOTAL.load(Ordering::Relaxed);
    let repair_improved = REPAIR_CYCLE_IMPROVED_TOTAL.load(Ordering::Relaxed);
    let repair_unresolved = REPAIR_CYCLE_UNRESOLVED_TOTAL.load(Ordering::Relaxed);
    let repair_replan_decisions = REPAIR_REPLAN_DECISION_TOTAL.load(Ordering::Relaxed);
    let repair_replan_required = REPAIR_REPLAN_REQUIRED_TOTAL.load(Ordering::Relaxed);
    let orchestration_node_mapped = ORCHESTRATION_NODE_MAPPED_TOTAL.load(Ordering::Relaxed);
    let orchestration_node_unmapped = ORCHESTRATION_NODE_UNMAPPED_TOTAL.load(Ordering::Relaxed);
    let loop_stop_complete = AUTONOMY_LOOP_STOP_COMPLETE_TOTAL.load(Ordering::Relaxed);
    let loop_stop_failed = AUTONOMY_LOOP_STOP_FAILED_TOTAL.load(Ordering::Relaxed);
    let loop_stop_escalated = AUTONOMY_LOOP_STOP_ESCALATED_TOTAL.load(Ordering::Relaxed);
    let loop_stop_incomplete = AUTONOMY_LOOP_STOP_INCOMPLETE_TOTAL.load(Ordering::Relaxed);
    let tool_followup_attempt = TOOL_FOLLOWUP_ATTEMPT_TOTAL.load(Ordering::Relaxed);
    let tool_followup_success = TOOL_FOLLOWUP_SUCCESS_TOTAL.load(Ordering::Relaxed);
    let tool_followup_fallback = TOOL_FOLLOWUP_FALLBACK_TOTAL.load(Ordering::Relaxed);
    let capability_selection_applied = CAPABILITY_SELECTION_APPLIED_TOTAL.load(Ordering::Relaxed);
    let capability_selection_no_match = CAPABILITY_SELECTION_NO_MATCH_TOTAL.load(Ordering::Relaxed);
    let capability_selection_none = CAPABILITY_SELECTION_NONE_TOTAL.load(Ordering::Relaxed);
    let vote_winner_strong = VOTE_WINNER_STRONG_TOTAL.load(Ordering::Relaxed);
    let vote_winner_escalation = VOTE_WINNER_ESCALATION_TOTAL.load(Ordering::Relaxed);
    let fallback_unhealthy = FALLBACK_UNHEALTHY_AGENT_TOTAL.load(Ordering::Relaxed);
    let reputation_routing_applied = REPUTATION_ROUTING_APPLIED_TOTAL.load(Ordering::Relaxed);
    let vote_reputation_tiebreak = VOTE_REPUTATION_TIEBREAK_TOTAL.load(Ordering::Relaxed);
    let route_total = planner_guided + explicit;
    let capability_selection_total =
        capability_selection_applied + capability_selection_no_match + capability_selection_none;
    let vote_total = vote_winner_strong + vote_winner_escalation;
    let parallel_tool_fanout_calls = PARALLEL_TOOL_FANOUT_CALLS_TOTAL.load(Ordering::Relaxed);
    let parallel_tool_fanout_batch = PARALLEL_TOOL_FANOUT_BATCH_TOTAL.load(Ordering::Relaxed);
    let agent_switch_total = AGENT_SWITCH_TOTAL.load(Ordering::Relaxed);
    let agent_switch_by_failure = AGENT_SWITCH_BY_FAILURE_TOTAL.load(Ordering::Relaxed);
    let agent_switch_by_reputation = AGENT_SWITCH_BY_REPUTATION_TOTAL.load(Ordering::Relaxed);
    let recovery_total = auto_recovery + human_confirmation;
    let alignment_total = alignment_high + alignment_low;
    let repair_total = repair_resolved + repair_improved + repair_unresolved;
    let orchestration_node_total = orchestration_node_mapped + orchestration_node_unmapped;
    let loop_stop_total =
        loop_stop_complete + loop_stop_failed + loop_stop_escalated + loop_stop_incomplete;

    let planner_guided_ratio = if route_total == 0 {
        0.0
    } else {
        planner_guided as f64 / route_total as f64
    };

    let auto_recovery_ratio = if recovery_total == 0 {
        0.0
    } else {
        auto_recovery as f64 / recovery_total as f64
    };

    let orchestration_alignment_high_ratio = if alignment_total == 0 {
        0.0
    } else {
        alignment_high as f64 / alignment_total as f64
    };

    let repair_effective_ratio = if repair_total == 0 {
        0.0
    } else {
        (repair_resolved + repair_improved) as f64 / repair_total as f64
    };

    let repair_replan_required_ratio = if repair_replan_decisions == 0 {
        0.0
    } else {
        repair_replan_required as f64 / repair_replan_decisions as f64
    };

    let orchestration_node_mapping_ratio = if orchestration_node_total == 0 {
        1.0
    } else {
        orchestration_node_mapped as f64 / orchestration_node_total as f64
    };

    let autonomy_loop_completion_ratio = if loop_stop_total == 0 {
        0.0
    } else {
        loop_stop_complete as f64 / loop_stop_total as f64
    };

    let tool_followup_success_ratio = if tool_followup_attempt == 0 {
        0.0
    } else {
        tool_followup_success as f64 / tool_followup_attempt as f64
    };

    let capability_selection_applied_ratio = if capability_selection_total == 0 {
        0.0
    } else {
        capability_selection_applied as f64 / capability_selection_total as f64
    };

    let vote_escalation_ratio = if vote_total == 0 {
        0.0
    } else {
        vote_winner_escalation as f64 / vote_total as f64
    };

    let fallback_unhealthy_ratio = if route_total == 0 {
        0.0
    } else {
        fallback_unhealthy as f64 / route_total as f64
    };

    let parallel_tool_fanout_avg_batch = if parallel_tool_fanout_calls == 0 {
        0.0
    } else {
        parallel_tool_fanout_batch as f64 / parallel_tool_fanout_calls as f64
    };

    json!({
        "planner_guided_tool_route_total": planner_guided,
        "explicit_tool_route_total": explicit,
        "planner_guided_route_ratio": planner_guided_ratio,
        "requirement_auto_recovery_total": auto_recovery,
        "requirement_human_confirmation_total": human_confirmation,
        "requirement_auto_recovery_ratio": auto_recovery_ratio,
        "orchestration_alignment_high_total": alignment_high,
        "orchestration_alignment_low_total": alignment_low,
        "orchestration_alignment_high_ratio": orchestration_alignment_high_ratio,
        "idempotency_hit_total": idempotency_hits,
        "repair_cycle_resolved_total": repair_resolved,
        "repair_cycle_improved_total": repair_improved,
        "repair_cycle_unresolved_total": repair_unresolved,
        "repair_cycle_effective_ratio": repair_effective_ratio,
        "repair_replan_decision_total": repair_replan_decisions,
        "repair_replan_required_total": repair_replan_required,
        "repair_replan_required_ratio": repair_replan_required_ratio,
        "orchestration_node_mapped_total": orchestration_node_mapped,
        "orchestration_node_unmapped_total": orchestration_node_unmapped,
        "orchestration_node_mapping_ratio": orchestration_node_mapping_ratio,
        "autonomy_loop_stop_complete_total": loop_stop_complete,
        "autonomy_loop_stop_failed_total": loop_stop_failed,
        "autonomy_loop_stop_escalated_total": loop_stop_escalated,
        "autonomy_loop_stop_incomplete_total": loop_stop_incomplete,
        "autonomy_loop_completion_ratio": autonomy_loop_completion_ratio,
        "tool_followup_attempt_total": tool_followup_attempt,
        "tool_followup_success_total": tool_followup_success,
        "tool_followup_fallback_total": tool_followup_fallback,
        "tool_followup_success_ratio": tool_followup_success_ratio,
        "cache_bypass_for_execution_total": CACHE_BYPASS_FOR_EXECUTION_TOTAL.load(Ordering::Relaxed),
        "cache_shortcircuit_refused_total": CACHE_SHORTCIRCUIT_REFUSED_TOTAL.load(Ordering::Relaxed),
        "cache_shortcircuit_execution_like_total": CACHE_SHORTCIRCUIT_EXECUTION_LIKE_TOTAL.load(Ordering::Relaxed),
        "capability_selection_applied_total": capability_selection_applied,
        "capability_selection_no_match_total": capability_selection_no_match,
        "capability_selection_none_total": capability_selection_none,
        "capability_selection_applied_ratio": capability_selection_applied_ratio,
        "vote_winner_strong_total": vote_winner_strong,
        "vote_winner_escalation_total": vote_winner_escalation,
        "vote_escalation_ratio": vote_escalation_ratio,
        "fallback_unhealthy_agent_total": fallback_unhealthy,
        "fallback_unhealthy_ratio": fallback_unhealthy_ratio,
        "reputation_routing_applied_total": reputation_routing_applied,
        "vote_reputation_tiebreak_total": vote_reputation_tiebreak,
        "parallel_tool_fanout_calls_total": parallel_tool_fanout_calls,
        "parallel_tool_fanout_batch_total": parallel_tool_fanout_batch,
        "parallel_tool_fanout_avg_batch": parallel_tool_fanout_avg_batch,
        "agent_switch_total": agent_switch_total,
        "agent_switch_by_failure_total": agent_switch_by_failure,
        "agent_switch_by_reputation_total": agent_switch_by_reputation,
    })
}
