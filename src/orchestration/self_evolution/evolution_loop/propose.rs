//! Proposal phase — structured analysis of a trigger.
//!
//! Contains the [`Analysis`] type that represents the output of analysing
//! an [`EvolutionTrigger`] before generating a patch.

use serde::{Deserialize, Serialize};

use super::observe::EvolutionTrigger;

// ---------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------

/// Structured analysis of a trigger, produced before generating a patch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Analysis {
    /// Unique analysis ID.
    pub analysis_id: uuid::Uuid,
    /// The trigger that prompted this analysis.
    pub trigger: EvolutionTrigger,
    /// Root cause hypothesis.
    pub root_cause: String,
    /// Suggested approach for the fix.
    pub suggested_approach: String,
    /// Files that are likely relevant to the issue.
    pub relevant_files: Vec<String>,
    /// Risk assessment: "low", "medium", "high".
    pub risk_level: String,
    /// Confidence score (0.0 – 1.0).
    pub confidence: f64,
    /// Timestamp in milliseconds.
    pub timestamp_ms: u64,
}

impl Analysis {
    /// Create a new Analysis from a trigger.
    pub fn new(
        trigger: EvolutionTrigger,
        root_cause: String,
        suggested_approach: String,
        relevant_files: Vec<String>,
        risk_level: String,
        confidence: f64,
    ) -> Self {
        Self {
            analysis_id: uuid::Uuid::new_v4(),
            trigger,
            root_cause,
            suggested_approach,
            relevant_files,
            risk_level,
            confidence,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analysis_new() {
        let trigger = EvolutionTrigger::ManualRequest {
            instruction: "optimize".to_string(),
        };
        let analysis = Analysis::new(
            trigger.clone(),
            "root cause".to_string(),
            "approach".to_string(),
            vec!["src/lib.rs".to_string()],
            "low".to_string(),
            0.85,
        );
        assert_eq!(analysis.trigger.label(), "manual_request");
        assert!(analysis.confidence > 0.8);
    }
}
