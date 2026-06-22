//! Escalation strategies — human intervention escalation logic
//!
//! Provides the escalation action type and threshold management for
//! the recovery orchestrator. When auto-recovery is exhausted or a
//! circuit breaker is open, escalation is triggered.

use super::*;

/// Build an escalation action with the given reason and context.
#[allow(
    dead_code,
    reason = "Public API surface for escalation strategy consumers"
)]
pub fn build_escalation(reason: String, context: Value) -> RecoveryAction {
    RecoveryAction::Escalate { reason, context }
}

/// Check whether the orchestrator should escalate based on current state.
///
/// Returns `Some(Escalate)` if escalation is needed, `None` otherwise.
pub fn should_escalate(
    consecutive_auto_failures: u32,
    human_intervention_threshold: u32,
    total_auto_attempts: u32,
    max_auto_recovery_attempts: u32,
    context: Value,
) -> Option<RecoveryAction> {
    if consecutive_auto_failures >= human_intervention_threshold {
        return Some(RecoveryAction::Escalate {
            reason: format!(
                "{} consecutive auto-recovery failures exceeded threshold of {}",
                consecutive_auto_failures, human_intervention_threshold
            ),
            context,
        });
    }

    if total_auto_attempts >= max_auto_recovery_attempts {
        return Some(RecoveryAction::Escalate {
            reason: format!(
                "max auto recovery attempts ({}) exhausted",
                max_auto_recovery_attempts
            ),
            context,
        });
    }

    None
}
