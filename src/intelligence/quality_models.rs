//! Quality assessment types, verdicts, signals.
//!
//! ## Cross-references
//!
//! - [`QualityVerdict`] is the canonical verdict enum. The [`verification`](crate::intelligence::verification)
//!   module aliases it as `VerificationVerdict` for semantic clarity in the verification pipeline.
//! - [`QualitySignal`] is similarly aliased as `VerificationSignal` in the verification module.
//!
//! The former `KnowledgeDistiller` / `aggregate_verdict` / bigram `similarity`
//! helpers were removed: they had zero production callers (only self-tests).
//! Verdict aggregation lives in [`verification::aggregate`](crate::intelligence::verification).

use serde::{Deserialize, Serialize};

/// Canonical quality verdict enum.
///
/// This is the single source of truth for categorical quality verdicts.
/// The [`verification`](crate::intelligence::verification) module re-exports
/// this as `VerificationVerdict` via a type alias.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum QualityVerdict {
    Approve,
    ApproveWithCaveats,
    Reject,
    Revise,
    InsufficientEvidence,
    Valid,
    Invalid,
    RequiresRepair,
    Inconclusive,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum QualitySignalType {
    Syntax,
    Tests,
    Lint,
    Policy,
    Logic,
    PuaQualityCompass,
    RuntimeVerification,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualitySignal {
    pub signal_type: QualitySignalType,
    pub passed: bool,
    pub confidence: f32,
    pub details: Option<String>,
}

impl QualitySignal {
    pub fn is_sufficient_for_distillation(&self) -> bool {
        self.passed && self.confidence >= 0.7
    }
}
