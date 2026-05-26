//! Requirement gate continuation — converts hard-blocking gates into resumable state machines.
//!
//! Instead of returning -32006 errors when requirements are unclear, this module
//! produces continuation decisions that enable the caller to:
//! - Auto-confirm low-risk tasks and proceed immediately
//! - Enter a clarification sub-flow for medium-risk tasks
//! - Escalate to human confirmation only for high-risk tasks
//!
//! This implements the AUTON-02 gap: requirement gate as resumable state machine.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::acp::helpers::requirement::{
    auto_clarification_enabled, can_auto_recover_task, evaluate_requirement_gate_facade,
    synthesize_requirement_contract, try_auto_recover_requirement_gate,
    RequirementGateAutoRecovery, RequirementGateFacadeDecision,
};
use crate::reinforcement::ArtifactLedger;

/// Continuation kind from the requirement gate evaluation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RequirementContinuationKind {
    /// Gate passed — task can proceed
    Confirmed,
    /// Gate auto-recovered with synthesized contract — task can proceed
    AutoConfirmed,
    /// Clarification is in progress — caller should run workflow.clarify sub-flow
    ClarificationInProgress,
    /// Clarification is required before execution, but human confirmation is not mandatory.
    ClarificationRequired,
    /// Human confirmation is required — caller should block
    HumanConfirmationRequired,
}

/// Full continuation decision from the requirement gate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementContinuation {
    /// The kind of continuation
    pub kind: RequirementContinuationKind,
    /// Whether the task can proceed immediately
    pub can_proceed: bool,
    /// Underlying gate decision
    pub gate: RequirementGateFacadeDecision,
    /// Auto-recovery metadata (if auto-recovered)
    pub auto_recovery: Option<RequirementGateAutoRecovery>,
    /// Next step for the caller
    pub next_step: Value,
}

/// Evaluate the requirement gate with continuation-aware logic.
///
/// Unlike `evaluate_requirement_gate_facade` which returns `blocked: bool` and
/// leaves the caller to handle the error, this function explicitly tries three
/// strategies before giving up:
/// 1. **Auto-confirm**: Synthesize missing fields and proceed (low-risk, small gaps)
/// 2. **Clarification**: Return a "proceed with clarification" signal for the
///    caller to enter a clarification sub-flow without hard-blocking
/// 3. **Human confirmation**: Only block when high risk and large gaps remain
pub fn evaluate_with_continuation(
    ledger: &ArtifactLedger,
    task: &str,
    params: &Value,
    source: &str,
) -> RequirementContinuation {
    let gate = match evaluate_requirement_gate_facade(ledger, task, params, source) {
        Ok(g) => g,
        Err(e) => {
            // Gate evaluation itself failed — treat as hard block
            return RequirementContinuation {
                kind: RequirementContinuationKind::HumanConfirmationRequired,
                can_proceed: false,
                gate: RequirementGateFacadeDecision {
                    kind: "requirement_contract".to_string(),
                    blocked: true,
                    reason: Some(format!("gate evaluation error: {e}")),
                    missing_fields: vec!["*".to_string()],
                    next_step: json!({"status": "error", "error": e.to_string()}),
                    clarification_artifact_path: None,
                    governance_artifact_path: PathBuf::new(),
                },
                auto_recovery: None,
                next_step: json!({"status": "error", "error": e.to_string()}),
            };
        }
    };

    // Case 1: Gate already passed — confirmed
    if !gate.blocked {
        return RequirementContinuation {
            kind: RequirementContinuationKind::Confirmed,
            can_proceed: true,
            gate,
            auto_recovery: None,
            next_step: json!({"status": "confirmed"}),
        };
    }

    // Case 2: Try auto-recovery
    if let Ok(Some(recovery)) =
        try_auto_recover_requirement_gate(ledger, task, params, source, &gate)
    {
        return RequirementContinuation {
            kind: RequirementContinuationKind::AutoConfirmed,
            can_proceed: true,
            gate: recovery.gate.clone(),
            auto_recovery: Some(recovery),
            next_step: json!({
                "status": "auto_confirmed",
                "auto_clarification_in_progress": true,
                "requires_human_confirmation": false,
            }),
        };
    }

    // Case 3: Gate blocked but can proceed with clarification
    let missing_field_count = gate.missing_fields.len();
    let is_low_risk = can_auto_recover_task(task, &gate);
    let has_clarification_support = auto_clarification_enabled(params);

    if is_low_risk && has_clarification_support && missing_field_count <= 3 {
        // Minimal gaps — synthesize contract on the spot and mark as clarification-in-progress
        let contract = synthesize_requirement_contract(task, params, source);
        let mut recovered_params = params.clone();
        if let Some(params_obj) = recovered_params.as_object_mut() {
            params_obj.insert(
                "requirement_contract".to_string(),
                serde_json::to_value(&contract).unwrap_or_default(),
            );
            params_obj.insert("requirement_confirmed".to_string(), Value::Bool(true));
            params_obj.insert(
                "auto_clarification_in_progress".to_string(),
                Value::Bool(true),
            );
            params_obj.insert(
                "requires_human_confirmation".to_string(),
                Value::Bool(false),
            );
        }

        // Re-evaluate with the synthesized contract
        if let Ok(recovered_gate) =
            evaluate_requirement_gate_facade(ledger, task, &recovered_params, source)
        {
            if !recovered_gate.blocked {
                return RequirementContinuation {
                    kind: RequirementContinuationKind::ClarificationInProgress,
                    can_proceed: true,
                    gate: recovered_gate,
                    auto_recovery: None,
                    next_step: json!({
                        "status": "clarification_in_progress",
                        "auto_clarification_in_progress": true,
                        "requires_human_confirmation": false,
                        "missing_fields": gate.missing_fields,
                        "note": "auto-synthesized minimal contract — results may need review",
                    }),
                };
            }
        }
    }

    // Case 4: Gate still blocked.
    // Low-risk tasks should return clarification-required (soft block) rather than
    // forcing immediate human confirmation. Only high-risk paths remain hard blocked.
    let missing_fields = gate.missing_fields.clone();
    if is_low_risk {
        return RequirementContinuation {
            kind: RequirementContinuationKind::ClarificationRequired,
            can_proceed: false,
            gate,
            auto_recovery: None,
            next_step: json!({
                "method": "workflow.clarify",
                "task": task,
                "missing_fields": missing_fields,
                "requires_human_confirmation": false,
                "reason": "requirement clarification required before execution",
            }),
        };
    }

    RequirementContinuation {
        kind: RequirementContinuationKind::HumanConfirmationRequired,
        can_proceed: false,
        gate,
        auto_recovery: None,
        next_step: json!({
            "method": "workflow.clarify",
            "task": task,
            "missing_fields": missing_fields,
            "requires_human_confirmation": true,
            "reason": "requirement gate could not be auto-resolved",
        }),
    }
}

/// Check if a continuation allows proceeding with execution.
#[inline]
pub fn can_proceed_with_continuation(continuation: &RequirementContinuation) -> bool {
    continuation.can_proceed
}

/// Extract the effective requirement gate payload for the response.
pub fn requirement_gate_payload_for_response(continuation: &RequirementContinuation) -> Value {
    match &continuation.kind {
        RequirementContinuationKind::Confirmed
        | RequirementContinuationKind::AutoConfirmed
        | RequirementContinuationKind::ClarificationInProgress => {
            continuation.gate.success_payload()
        }
        RequirementContinuationKind::ClarificationRequired
        | RequirementContinuationKind::HumanConfirmationRequired => {
            continuation.gate.blocked_payload()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ledger() -> ArtifactLedger {
        ArtifactLedger::new(None)
    }

    fn simple_params() -> Value {
        json!({
            "governance": {
                "auto_clarification_enabled": true,
            },
            "requirement_contract": {
                "task": "simple test task",
                "goal": "test goal",
                "scope": "test scope",
            },
            "requirement_confirmed": true,
        })
    }

    #[test]
    fn confirmed_task_returns_confirmed_kind() {
        let ledger = make_ledger();
        let result = evaluate_with_continuation(
            &ledger,
            "simple test task",
            &simple_params(),
            "test.source",
        );
        assert!(result.can_proceed);
        assert_eq!(result.kind, RequirementContinuationKind::Confirmed);
    }
}
