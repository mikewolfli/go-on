//! Audit trail and governance profile for HarnessBus — F-GAP-13

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Top-level HarnessBus metrics, for push into governance.status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PuaGovernanceProfile {
    pub enabled: bool,
    pub total_evaluations: u64,
    pub allow_count: u64,
    pub deny_count: u64,
    pub escalate_count: u64,
    pub review_count: u64,
    pub red_line_blocks: u64,
    pub budget_violations: u64,
    pub sandbox_denials: u64,
    pub idempotency_hits: u64,
    pub other_denials: u64,
    pub audit_entries_total: u64,
    /// Audit entries dropped by the canonical `ThreadSafeAuditLog` due to
    /// buffer overflow — a real degradation signal for the audit subsystem
    /// (synced from `audit_log.dropped_count()` on every `HarnessBus::audit`).
    pub audit_dropped_entries: u64,
    // ── Extended governance module tracking (14-module coverage) ──────
    /// (7) Rationalization module blocks
    pub rationalization_blocks: u64,
    /// (8) RBAC enforcer denials
    pub rbac_denials: u64,
    /// (9) Security governor blocks
    pub security_blocks: u64,
    /// (10) Drift detection engine detections
    pub drift_detections: u64,
    /// (11) Approval engine requests processed
    pub approval_requests: u64,
    /// (12) Approval learning updates applied
    pub learning_updates: u64,
    /// (13) Hardening events triggered
    pub hardening_events: u64,
    // ── Existing fields ───────────────────────────────────────────────
    pub current_active_policies: u32,
    pub current_escalation_level: String,
    pub runtime_control_mode: String,
    pub policy_violation_trend: String,
    pub last_evaluation_ms: u64,
}

impl PuaGovernanceProfile {
    /// Record a rationalization block.
    pub fn record_rationalization_block(&mut self) {
        self.rationalization_blocks += 1;
    }

    /// Record an RBAC denial.
    pub fn record_rbac_denial(&mut self) {
        self.rbac_denials += 1;
    }

    /// Record a security block.
    pub fn record_security_block(&mut self) {
        self.security_blocks += 1;
    }

    /// Record a drift detection.
    pub fn record_drift_detection(&mut self) {
        self.drift_detections += 1;
    }

    /// Record an approval request.
    pub fn record_approval_request(&mut self) {
        self.approval_requests += 1;
    }

    /// Record a learning update.
    pub fn record_learning_update(&mut self) {
        self.learning_updates += 1;
    }

    /// Record a hardening event.
    pub fn record_hardening_event(&mut self) {
        self.hardening_events += 1;
    }
}

impl Default for PuaGovernanceProfile {
    fn default() -> Self {
        Self {
            enabled: true,
            total_evaluations: 0,
            allow_count: 0,
            deny_count: 0,
            escalate_count: 0,
            review_count: 0,
            red_line_blocks: 0,
            budget_violations: 0,
            sandbox_denials: 0,
            idempotency_hits: 0,
            other_denials: 0,
            audit_entries_total: 0,
            audit_dropped_entries: 0,
            rationalization_blocks: 0,
            rbac_denials: 0,
            security_blocks: 0,
            drift_detections: 0,
            approval_requests: 0,
            learning_updates: 0,
            hardening_events: 0,
            current_active_policies: 5,
            current_escalation_level: "normal".to_string(),
            runtime_control_mode: "standard".to_string(),
            policy_violation_trend: "stable".to_string(),
            last_evaluation_ms: 0,
        }
    }
}
