use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

static PLANNER_GUIDED_ROUTE_TOTAL: AtomicU64 = AtomicU64::new(0);
static EXPLICIT_TOOL_ROUTE_TOTAL: AtomicU64 = AtomicU64::new(0);
static REQUIREMENT_AUTO_RECOVERY_TOTAL: AtomicU64 = AtomicU64::new(0);
static REQUIREMENT_HUMAN_CONFIRMATION_TOTAL: AtomicU64 = AtomicU64::new(0);

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

pub(crate) fn autonomy_metrics_snapshot() -> Value {
    let planner_guided = PLANNER_GUIDED_ROUTE_TOTAL.load(Ordering::Relaxed);
    let explicit = EXPLICIT_TOOL_ROUTE_TOTAL.load(Ordering::Relaxed);
    let auto_recovery = REQUIREMENT_AUTO_RECOVERY_TOTAL.load(Ordering::Relaxed);
    let human_confirmation = REQUIREMENT_HUMAN_CONFIRMATION_TOTAL.load(Ordering::Relaxed);
    let route_total = planner_guided + explicit;
    let recovery_total = auto_recovery + human_confirmation;

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

    json!({
        "planner_guided_tool_route_total": planner_guided,
        "explicit_tool_route_total": explicit,
        "planner_guided_route_ratio": planner_guided_ratio,
        "requirement_auto_recovery_total": auto_recovery,
        "requirement_human_confirmation_total": human_confirmation,
        "requirement_auto_recovery_ratio": auto_recovery_ratio,
    })
}