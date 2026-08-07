//! Shared DetectionSeverity enum used across security sub-modules.
//!
//! Both `content_safety` and `prompt_injection` previously defined identical
//! private enums (`SafetySeverity` / `InjectionSeverity`).  This module
//! provides a single canonical type that each sub-module re-exports under its
//! own alias.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum DetectionSeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl DetectionSeverity {
    /// Canonical detection-confidence score for this severity level.
    ///
    /// Used by detectors (e.g. prompt-injection) to compare against a
    /// configurable detection threshold.
    pub fn to_score(&self) -> f64 {
        match self {
            Self::Low => 0.3,
            Self::Medium => 0.5,
            Self::High => 0.7,
            Self::Critical => 0.9,
        }
    }
}

/// Shared base for [`SafetyViolation`](crate::security::prompt_injection::SafetyViolation)
/// in [`prompt_injection`](crate::security::prompt_injection).
///
/// The type-specific fields (e.g. `category`, `suggested_action`, `pattern_id`)
/// are defined in each module's own `SafetyViolation` wrapper.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SafetyViolationBase {
    pub severity: DetectionSeverity,
    pub match_text: String,
    pub start_pos: usize,
    pub end_pos: usize,
    pub description: String,
}
