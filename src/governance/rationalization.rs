//! Rationalization — F-GAP-22
//!
//! S4: Self-Rationalization Guard
//!
//! Detects low-confidence outputs with weak evidence and flags them for re-examination.
//! In full_auto mode, blocks and triggers a single re-question cycle (token-budget controlled).
//!
//! # Status
//! Fully wired. `SelfRationalizationGuard` is actively called from four locations:
//! `PolicyEvaluator` (P1-11), `HarnessBus`, `init_intel_voters` in the intelligence hub,
//! and `RationalizationGuardVoter` in the voting subsystem.

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
    /// Returns true if the output should be blocked (full_auto re-question).
    pub fn evaluate(
        &mut self,
        annotation: &mut RationalizationAnnotation,
        confidence: f32,
        is_full_auto: bool,
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

        if is_full_auto {
            annotation.reexamine_triggered = true;
            self.counters.reexamine_triggered_count += 1;
            self.counters.weak_evidence_blocked_count += 1;
            return true;
        }

        false
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
