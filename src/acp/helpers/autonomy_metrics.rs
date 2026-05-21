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
    let route_total = planner_guided + explicit;
    let recovery_total = auto_recovery + human_confirmation;
    let alignment_total = alignment_high + alignment_low;
    let repair_total = repair_resolved + repair_improved + repair_unresolved;

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
    })
}