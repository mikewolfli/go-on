use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

static TOOL_ALLOWED_TOTAL: AtomicU64 = AtomicU64::new(0);
static TOOL_POLICY_DENIED_TOTAL: AtomicU64 = AtomicU64::new(0);
static TOOL_BUDGET_DENIED_TOTAL: AtomicU64 = AtomicU64::new(0);

pub(crate) fn record_tool_allowed() {
    TOOL_ALLOWED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_tool_policy_denied() {
    TOOL_POLICY_DENIED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_tool_budget_denied() {
    TOOL_BUDGET_DENIED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn tool_governance_counters() -> Value {
    let allowed = TOOL_ALLOWED_TOTAL.load(Ordering::Relaxed);
    let policy_denied = TOOL_POLICY_DENIED_TOTAL.load(Ordering::Relaxed);
    let budget_denied = TOOL_BUDGET_DENIED_TOTAL.load(Ordering::Relaxed);
    let total_attempts = allowed + policy_denied + budget_denied;

    json!({
        "tool_allowed_total": allowed,
        "tool_policy_denied_total": policy_denied,
        "tool_budget_denied_total": budget_denied,
        "tool_total_attempts": total_attempts,
    })
}