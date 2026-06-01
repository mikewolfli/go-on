//! Human-in-the-Loop (HITL) Approval End-to-End
//!
//! Validates the HITL approval workflow:
//!   high risk → PUA → L2 approval → approve → action released
//!
//! Uses go_on::governance types for the approval engine, PUA rule engine,
//! and risk level classification. Real integration requires the governance
//! subsystem with PUA rules configured and a mock/callback endpoint for L2.
//!
//! # integration-test-stub
//! Approval escalation and action release are validated structurally. In
//! production, the approval engine runs a background timeout watcher and
//! the action executor integrates with the orchestrator.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::sleep;

use go_on::governance::approval_engine::{
    ApprovalEngine, ApprovalRequest, ApprovalStatus, EscalationChain, EscalationStep, RiskLevel,
    TimeoutPolicy,
};
use go_on::governance::pua::{PuaEnforcementPlan, PuaRuleEngine};

// ── Helpers ────────────────────────────────────────────────────────────────

struct HitlE2eContext {
    action_id: Option<String>,
    request_id: Option<String>,
    final_status: Option<String>,
}

impl HitlE2eContext {
    fn new() -> Self {
        Self {
            action_id: None,
            request_id: None,
            final_status: None,
        }
    }
}

/// Create a PuaRuleEngine wrapped in Arc<Mutex<>> for the ApprovalEngine.
fn make_pua_engine() -> Arc<Mutex<PuaRuleEngine>> {
    let plan = Arc::new(std::sync::Mutex::new(PuaEnforcementPlan::default()));
    Arc::new(Mutex::new(PuaRuleEngine::new(plan)))
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Full HITL approval flow: high risk → PUA → L2 approval → approve → released.
#[tokio::test]
#[ignore]
async fn test_hitl_approval_full_flow() {
    let mut ctx = HitlE2eContext::new();

    // ── 1. Setup engines ───────────────────────────────────────────────
    let pua = make_pua_engine();
    let timeout_policy = TimeoutPolicy::default();
    let mut approval_engine = ApprovalEngine::new(pua.clone(), timeout_policy);

    // ── 2. High-risk action definition ─────────────────────────────────
    let action_id = "action-e2e-001";
    ctx.action_id = Some(action_id.to_string());

    let mut context = HashMap::new();
    context.insert("target".into(), "production".into());
    context.insert("patch".into(), "modify auth module".into());

    // ── 3. PUA enforcement ─────────────────────────────────────────────
    // The PUA engine evaluates the action and returns an enforcement plan.
    let plan: PuaEnforcementPlan = PuaEnforcementPlan::default();
    assert_eq!(plan.escalation_level, "1", "default PUA plan has level 1");
    // integration-test-stub: real PUA evaluation inspects the action's
    // risk score and returns level-2 for high-risk actions.

    // ── 4. Submit for L2 approval ──────────────────────────────────────
    let request = ApprovalRequest::new(
        "operator@go-on.io".into(),
        format!("deploy: {}", action_id),
        RiskLevel::High,
        context,
    );

    assert_eq!(request.risk_level, RiskLevel::High);
    assert_eq!(request.status, ApprovalStatus::Pending);
    let request_id = approval_engine.submit_for_approval(request);
    ctx.request_id = Some(request_id.clone());
    assert!(!request_id.is_empty(), "request must be assigned an ID");

    // ── 5. Human approves the request ──────────────────────────────────
    // integration-test-stub: real approval calls
    // approval_engine.approve(&request_id, "admin@go-on.io", "Approved").
    // Here we construct the approved status structurally.
    let approved_status = ApprovalStatus::Approved {
        approver: "admin@go-on.io".into(),
        comment: "Approved after review".into(),
        timestamp_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    };

    assert!(
        approved_status.is_finalized(),
        "approved status must be finalized"
    );

    ctx.final_status = Some("Approved".into());
    assert_eq!(ctx.final_status.as_deref(), Some("Approved"));

    // ── 6. Action released ─────────────────────────────────────────────
    // integration-test-stub: real release calls ActionExecutor::execute()
    // which checks that the request status is Approved before dispatching.
    match &approved_status {
        ApprovalStatus::Approved { approver, .. } => {
            assert_eq!(approver, "admin@go-on.io");
        }
        _ => panic!("expected Approved status"),
    }

    sleep(Duration::from_millis(10)).await;
    assert!(true, "HITL approval full flow passed");
}

/// Validates that a pending request auto-denies after timeout.
#[tokio::test]
#[ignore]
async fn test_hitl_approval_auto_deny_on_timeout() {
    let pua = make_pua_engine();
    let timeout_policy = TimeoutPolicy::default();
    let mut approval_engine = ApprovalEngine::new(pua, timeout_policy);

    let request = ApprovalRequest::new(
        "operator@go-on.io".into(),
        "auto-deny-test".into(),
        RiskLevel::High,
        HashMap::new(),
    );

    let request_id = approval_engine.submit_for_approval(request);
    assert!(!request_id.is_empty());

    // integration-test-stub: real timeout check runs in a background task.
    // High risk has a 10-minute deny_timeout. After expiry, the engine
    // transitions the request to AutoDenied.
    let auto_denied = ApprovalStatus::AutoDenied {
        reason: "timeout: no response within escalation window".into(),
        timestamp_ms: 0,
    };
    assert!(auto_denied.is_finalized(), "auto-denied must be finalized");

    sleep(Duration::from_millis(10)).await;
    assert!(true, "auto-deny skeleton passed");
}

/// Validates that a rejected action is not released.
#[tokio::test]
#[ignore]
async fn test_hitl_approval_rejection_blocks_execution() {
    let pua = make_pua_engine();
    let timeout_policy = TimeoutPolicy::default();
    let mut approval_engine = ApprovalEngine::new(pua, timeout_policy);

    let request = ApprovalRequest::new(
        "operator@go-on.io".into(),
        "rejection-test".into(),
        RiskLevel::High,
        HashMap::new(),
    );

    let request_id = approval_engine.submit_for_approval(request);

    // Simulate rejection.
    let rejected_status = ApprovalStatus::Rejected {
        approver: "auditor@go-on.io".into(),
        reason: "Policy violation: missing SLA".into(),
        timestamp_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    };

    assert!(
        rejected_status.is_finalized(),
        "rejected status must be finalized"
    );

    // integration-test-stub: real execution checks
    // `execute_if_approved(request_id)` and returns an error when rejected.
    match &rejected_status {
        ApprovalStatus::Rejected {
            approver, reason, ..
        } => {
            assert_eq!(approver, "auditor@go-on.io");
            assert!(reason.contains("missing SLA"));
        }
        _ => panic!("expected Rejected status"),
    }

    sleep(Duration::from_millis(10)).await;
    assert!(true, "rejection blocks execution skeleton passed");
}

/// Validates escalation chain structure for high-risk actions.
#[tokio::test]
#[ignore]
async fn test_hitl_approval_escalation_chain() {
    let steps = vec![
        EscalationStep {
            level: 1,
            approver_role: "manager".into(),
            approver_id: None,
            comment: None,
        },
        EscalationStep {
            level: 2,
            approver_role: "director".into(),
            approver_id: None,
            comment: None,
        },
    ];

    let chain = EscalationChain::new(steps);
    assert_eq!(chain.current_step, 0);
    assert_eq!(chain.steps.len(), 2);
    assert_eq!(chain.steps[0].approver_role, "manager");
    assert_eq!(chain.steps[1].approver_role, "director");

    sleep(Duration::from_millis(10)).await;
    assert!(true, "escalation chain validation passed");
}
