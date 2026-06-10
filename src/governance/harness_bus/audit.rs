//! Audit trail and governance profile for HarnessBus — F-GAP-13

use crate::governance::harness_bus::types::AuditEntry;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Maximum number of audit entries retained in memory to prevent unbounded growth.
const MAX_AUDIT_ENTRIES: usize = 10_000;

/// HarnessAuditTrail — in-memory audit log for governance events.
///
/// Optionally delegates to a HashChainAuditor for tamper-evident persistence.
#[derive(Debug, Clone, Default)]
pub struct HarnessAuditTrail {
    pub entries: Vec<AuditEntry>,
    /// Optional hash-chain auditor for tamper-evident disk persistence.
    pub hash_chain: Option<Arc<Mutex<crate::security::audit_integrity::HashChainAuditor>>>,
}

impl HarnessAuditTrail {
    /// Push an entry, evicting the oldest if the cap is exceeded.
    /// Also forwards to the HashChainAuditor if configured.
    pub fn push(&mut self, entry: AuditEntry) {
        if self.entries.len() >= MAX_AUDIT_ENTRIES {
            // Evict oldest half to amortize cost.
            let keep = MAX_AUDIT_ENTRIES / 2;
            let drain_end = self.entries.len() - keep;
            self.entries.drain(0..drain_end);
        }
        self.entries.push(entry.clone());

        // Delegate to hash-chain auditor for tamper-evident persistence.
        if let Some(ref hash_chain) = self.hash_chain {
            let mut guard = hash_chain.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            });
            let payload = serde_json::json!({
                "timestamp": entry.timestamp,
                "request_id": entry.request_id,
                "stage": entry.stage,
                "verdict": entry.verdict,
                "dispatch_policy": entry.dispatch_policy,
                "execution_policy": entry.execution_policy,
                "governance_policy": entry.governance_policy,
                "violations": entry.violations,
                "context_snapshot": entry.context_snapshot,
            });
            if let Err(e) = guard.append(payload) {
                tracing::warn!(error = %e, "Failed to append to hash-chain auditor");
            }
        }
    }

    /// Set the hash-chain auditor for this trail.
    pub fn with_hash_chain(
        mut self,
        auditor: Arc<Mutex<crate::security::audit_integrity::HashChainAuditor>>,
    ) -> Self {
        self.hash_chain = Some(auditor);
        self
    }
}

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
    /// (14) Review control overrides
    pub review_overrides: u64,
    // ── Existing fields ───────────────────────────────────────────────
    pub current_active_policies: u32,
    pub current_escalation_level: String,
    pub runtime_control_mode: String,
    pub policy_violation_trend: String,
    pub last_evaluation_ms: u64,
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
            rationalization_blocks: 0,
            rbac_denials: 0,
            security_blocks: 0,
            drift_detections: 0,
            approval_requests: 0,
            learning_updates: 0,
            hardening_events: 0,
            review_overrides: 0,
            current_active_policies: 5,
            current_escalation_level: "normal".to_string(),
            runtime_control_mode: "standard".to_string(),
            policy_violation_trend: "stable".to_string(),
            last_evaluation_ms: 0,
        }
    }
}
