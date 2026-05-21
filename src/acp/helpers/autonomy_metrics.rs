use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

static PLANNER_GUIDED_ROUTE_TOTAL: AtomicU64 = AtomicU64::new(0);
static EXPLICIT_TOOL_ROUTE_TOTAL: AtomicU64 = AtomicU64::new(0);
static REQUIREMENT_AUTO_RECOVERY_TOTAL: AtomicU64 = AtomicU64::new(0);
static REQUIREMENT_HUMAN_CONFIRMATION_TOTAL: AtomicU64 = AtomicU64::new(0);
static ORCHESTRATION_ALIGNMENT_HIGH_TOTAL: AtomicU64 = AtomicU64::new(0);
static ORCHESTRATION_ALIGNMENT_LOW_TOTAL: AtomicU64 = AtomicU64::new(0);
static IDEMPOTENCY_HIT_TOTAL: AtomicU64 = AtomicU64::new(0);
static IDEMPOTENCY_PENDING_CONTINUATION_HIT_TOTAL: AtomicU64 = AtomicU64::new(0);
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

pub(crate) fn record_idempotency_hit(pending_continuation: bool) {
    IDEMPOTENCY_HIT_TOTAL.fetch_add(1, Ordering::Relaxed);
    if pending_continuation {
        IDEMPOTENCY_PENDING_CONTINUATION_HIT_TOTAL.fetch_add(1, Ordering::Relaxed);
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

pub(crate) fn autonomy_metrics_snapshot() -> Value {
    let planner_guided = PLANNER_GUIDED_ROUTE_TOTAL.load(Ordering::Relaxed);
    let explicit = EXPLICIT_TOOL_ROUTE_TOTAL.load(Ordering::Relaxed);
    let auto_recovery = REQUIREMENT_AUTO_RECOVERY_TOTAL.load(Ordering::Relaxed);
    let human_confirmation = REQUIREMENT_HUMAN_CONFIRMATION_TOTAL.load(Ordering::Relaxed);
    let alignment_high = ORCHESTRATION_ALIGNMENT_HIGH_TOTAL.load(Ordering::Relaxed);
    let alignment_low = ORCHESTRATION_ALIGNMENT_LOW_TOTAL.load(Ordering::Relaxed);
    let idempotency_hits = IDEMPOTENCY_HIT_TOTAL.load(Ordering::Relaxed);
    let idempotency_pending_hits =
        IDEMPOTENCY_PENDING_CONTINUATION_HIT_TOTAL.load(Ordering::Relaxed);
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
    let route_total = planner_guided + explicit;
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

    let idempotency_pending_ratio = if idempotency_hits == 0 {
        0.0
    } else {
        idempotency_pending_hits as f64 / idempotency_hits as f64
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
        "idempotency_pending_continuation_hit_total": idempotency_pending_hits,
        "idempotency_pending_continuation_ratio": idempotency_pending_ratio,
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
    })
}