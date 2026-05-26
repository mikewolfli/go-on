use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

static TOOL_ALLOWED_TOTAL: AtomicU64 = AtomicU64::new(0);
static TOOL_POLICY_DENIED_TOTAL: AtomicU64 = AtomicU64::new(0);
static TOOL_BUDGET_DENIED_TOTAL: AtomicU64 = AtomicU64::new(0);
static TOOL_RBAC_DENIED_TOTAL: AtomicU64 = AtomicU64::new(0);
static TOOL_HARNESS_SANDBOX_DENIED_TOTAL: AtomicU64 = AtomicU64::new(0);

pub(crate) fn record_tool_allowed() {
    TOOL_ALLOWED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_tool_policy_denied() {
    TOOL_POLICY_DENIED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_tool_budget_denied() {
    TOOL_BUDGET_DENIED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_tool_rbac_denied() {
    TOOL_RBAC_DENIED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_tool_harness_sandbox_denied() {
    TOOL_HARNESS_SANDBOX_DENIED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn tool_governance_counters() -> Value {
    let allowed = TOOL_ALLOWED_TOTAL.load(Ordering::Relaxed);
    let policy_denied = TOOL_POLICY_DENIED_TOTAL.load(Ordering::Relaxed);
    let budget_denied = TOOL_BUDGET_DENIED_TOTAL.load(Ordering::Relaxed);
    let rbac_denied = TOOL_RBAC_DENIED_TOTAL.load(Ordering::Relaxed);
    let harness_sandbox_denied = TOOL_HARNESS_SANDBOX_DENIED_TOTAL.load(Ordering::Relaxed);
    let total_attempts =
        allowed + policy_denied + budget_denied + rbac_denied + harness_sandbox_denied;

    json!({
        "tool_allowed_total": allowed,
        "tool_policy_denied_total": policy_denied,
        "tool_budget_denied_total": budget_denied,
        "tool_rbac_denied_total": rbac_denied,
        "tool_harness_sandbox_denied_total": harness_sandbox_denied,
        "tool_total_attempts": total_attempts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn test_record_tool_allowed_increments_counter() {
        let before = TOOL_ALLOWED_TOTAL.load(Ordering::Relaxed);
        record_tool_allowed();
        let after = TOOL_ALLOWED_TOTAL.load(Ordering::Relaxed);
        assert!(after > before, "allowed counter should increment");
    }

    #[test]
    fn test_record_tool_policy_denied_increments_counter() {
        let before = TOOL_POLICY_DENIED_TOTAL.load(Ordering::Relaxed);
        record_tool_policy_denied();
        let after = TOOL_POLICY_DENIED_TOTAL.load(Ordering::Relaxed);
        assert!(after > before, "policy denied counter should increment");
    }

    #[test]
    fn test_tool_governance_counters_returns_expected_keys() {
        let counters = tool_governance_counters();
        assert!(counters.get("tool_allowed_total").is_some());
        assert!(counters.get("tool_policy_denied_total").is_some());
        assert!(counters.get("tool_budget_denied_total").is_some());
        assert!(counters.get("tool_rbac_denied_total").is_some());
        assert!(counters.get("tool_harness_sandbox_denied_total").is_some());
        assert!(counters.get("tool_total_attempts").is_some());
    }

    #[test]
    fn test_tool_governance_counters_total_matches_sum() {
        let counters = tool_governance_counters();
        let allowed = counters["tool_allowed_total"].as_u64().unwrap_or(0);
        let policy_denied = counters["tool_policy_denied_total"].as_u64().unwrap_or(0);
        let budget_denied = counters["tool_budget_denied_total"].as_u64().unwrap_or(0);
        let rbac_denied = counters["tool_rbac_denied_total"].as_u64().unwrap_or(0);
        let harness_denied = counters["tool_harness_sandbox_denied_total"]
            .as_u64()
            .unwrap_or(0);
        let total = counters["tool_total_attempts"].as_u64().unwrap_or(0);
        assert_eq!(
            total,
            allowed + policy_denied + budget_denied + rbac_denied + harness_denied,
            "total should equal sum of all categories"
        );
    }
}
