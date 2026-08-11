//! Rationalization — F-GAP-22
//!
//! S4: Self-Rationalization Guard
//!
//! Detects low-confidence outputs with weak evidence and flags them for
//! re-examination. Weak-evidence detection: when the caller-supplied confidence
//! is below [`SelfRationalizationGuard::confidence_threshold`] and the
//! annotation carries no evidence references, the guard flags the assumptions
//! and returns `true` — the caller then decides whether to block the output or
//! weight it down.
//!
//! # Status
//! Fully wired. `SelfRationalizationGuard::evaluate()` is actively called from
//! three locations: `PolicyEvaluator::evaluate` (step 6 of the pre-route
//! composite evaluation) and `PolicyEvaluator::verify_output` (P1-11), both in
//! `src/governance/harness_bus/evaluator.rs`, plus `rationalize_decision` in
//! the intelligence hub (`src/intelligence/hub.rs`).
//! (`RationalizationGuardVoter` in the voting subsystem only reads
//! `confidence_threshold` and does not call `evaluate`.)

use serde::{Deserialize, Serialize};

/// Per-result assumptions and evidence tracking
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RationalizationAnnotation {
    /// Assumptions declared by this agent turn
    pub assumptions: Vec<String>,
    /// Evidence references (file path, URL, tool output id) per assumption
    pub evidence_refs: Vec<String>,
    /// Assumption indices that have weak or absent evidence
    pub weak_evidence_flags: Vec<String>,
    /// Whether a re-examination cycle was triggered
    pub reexamine_triggered: bool,
}

/// Runtime counters for governance.status
#[derive(Debug, Default)]
pub struct RationalizationCounters {
    pub reexamine_triggered_count: u64,
    pub weak_evidence_blocked_count: u64,
}

/// SelfRationalizationGuard evaluates an annotation and mutates it in-place.
pub struct SelfRationalizationGuard {
    /// Confidence threshold below which weak evidence is flagged (default 0.6)
    pub confidence_threshold: f32,
    pub counters: RationalizationCounters,
}

impl Default for SelfRationalizationGuard {
    fn default() -> Self {
        Self {
            confidence_threshold: 0.6,
            counters: RationalizationCounters::default(),
        }
    }
}

impl SelfRationalizationGuard {
    pub fn new(confidence_threshold: f32) -> Self {
        Self {
            confidence_threshold,
            counters: RationalizationCounters::default(),
        }
    }

    /// Evaluate annotation at the given confidence level.
    ///
    /// Returns `true` when weak evidence was detected (confidence below the
    /// threshold with no evidence refs); the caller decides whether to block
    /// the output or weight it down. Mutates the annotation in place:
    /// weak-evidence flags are populated and `reexamine_triggered` is set.
    pub fn evaluate(
        &mut self,
        annotation: &mut RationalizationAnnotation,
        confidence: f32,
    ) -> bool {
        // Flag assumptions without evidence support
        if confidence < self.confidence_threshold && annotation.evidence_refs.is_empty() {
            for assumption in &annotation.assumptions {
                if !annotation.weak_evidence_flags.contains(assumption) {
                    annotation.weak_evidence_flags.push(assumption.clone());
                }
            }
            // If no assumptions declared, add a synthetic flag
            if annotation.assumptions.is_empty() && annotation.weak_evidence_flags.is_empty() {
                annotation
                    .weak_evidence_flags
                    .push("low_confidence_no_evidence".to_string());
            }
        }

        if annotation.weak_evidence_flags.is_empty() {
            return false;
        }

        // Weak evidence detected: record the trigger and tell the caller the
        // output should be intercepted / weighted down. (The former
        // `is_full_auto` parameter was removed — every production call site
        // passed `false`, so the branch was dead and evaluate() always
        // returned false. Blocking vs. weighting is now the caller's call.)
        //
        // NOTE on the counters: both count *weak-evidence trigger events*, not
        // hard blocks — the caller may only weight the output down (e.g.
        // `verify_output` risk +0.2) instead of blocking it.
        annotation.reexamine_triggered = true;
        self.counters.reexamine_triggered_count += 1;
        self.counters.weak_evidence_blocked_count += 1;
        true
    }

    pub fn governance_profile(&self, enabled: bool) -> serde_json::Value {
        serde_json::json!({
            "enabled": enabled,
            "confidence_threshold": self.confidence_threshold,
            "reexamine_triggered_count": self.counters.reexamine_triggered_count,
            "weak_evidence_blocked_count": self.counters.weak_evidence_blocked_count,
        })
    }
}
