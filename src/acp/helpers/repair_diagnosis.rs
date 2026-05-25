//! Repair diagnosis for the auto-repair loop (AUTON-09).
//!
//! Moves repair from "retry the same thing" toward "diagnose the failure,
//! choose a strategy, then act". Each diagnosis classifies the failure and
//! suggests a strategy (retry / reroute / replan / repair).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Classification of a repair diagnosis
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DiagnosisKind {
    /// Transient error — retry with same inputs
    Retry,
    /// Tool/agent mismatch — reroute to a different tool/agent
    Reroute,
    /// Plan was wrong — replan the approach
    Replan,
    /// Known fix available — apply specific repair
    Repair,
    /// Cannot diagnose — escalate
    Escalate,
}

/// A single diagnosis result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairDiagnosis {
    /// What kind of diagnosis
    pub kind: DiagnosisKind,
    /// Confidence in this diagnosis (0.0–1.0)
    pub confidence: f64,
    /// Human-readable explanation
    pub explanation: String,
    /// Suggested strategy
    pub suggested_strategy: String,
    /// Whether this is a rerunnable diagnosis (true = same result expected next time)
    pub deterministic: bool,
}

/// Diagnose a failure based on its outcome and context.
///
/// This is a heuristic classifier — it uses the error message, tool name,
/// and subtask outcome to classify the failure. In a full implementation,
/// this would feed into an LLM for richer diagnosis.
pub fn diagnose_repair(
    subtask_id: &str,
    _outcome: &str,
    error_message: Option<&str>,
    previous_attempts: usize,
) -> RepairDiagnosis {
    let error_lower = error_message.unwrap_or("").to_ascii_lowercase();

    // Timeout errors → retry (transient)
    if error_lower.contains("timeout") || error_lower.contains("timed out") {
        return RepairDiagnosis {
            kind: DiagnosisKind::Retry,
            confidence: 0.7,
            explanation: format!("subtask '{subtask_id}' timed out — likely transient"),
            suggested_strategy: "retry with increased timeout or fewer parallel tasks".to_string(),
            deterministic: false,
        };
    }

    // Rate limit / quota errors → retry with backoff
    if error_lower.contains("rate limit")
        || error_lower.contains("quota")
        || error_lower.contains("429")
        || error_lower.contains("throttl")
    {
        return RepairDiagnosis {
            kind: DiagnosisKind::Retry,
            confidence: 0.85,
            explanation: format!("subtask '{subtask_id}' hit rate limit — retry with backoff"),
            suggested_strategy: "retry with exponential backoff".to_string(),
            deterministic: false,
        };
    }

    // Permission errors → reroute
    if error_lower.contains("permission")
        || error_lower.contains("denied")
        || error_lower.contains("forbidden")
        || error_lower.contains("not allowed")
        || error_lower.contains("unauthorized")
    {
        return RepairDiagnosis {
            kind: DiagnosisKind::Reroute,
            confidence: 0.8,
            explanation: format!(
                "subtask '{subtask_id}' denied by permission gate — reroute to allowed path"
            ),
            suggested_strategy: "reroute to alternative tool/agent with correct permissions"
                .to_string(),
            deterministic: true,
        };
    }

    // Tool not found / not available → reroute
    if error_lower.contains("not found")
        || error_lower.contains("no such")
        || error_lower.contains("unavailable")
        || error_lower.contains("not available")
    {
        return RepairDiagnosis {
            kind: DiagnosisKind::Reroute,
            confidence: 0.75,
            explanation: format!("subtask '{subtask_id}' resource not found — reroute"),
            suggested_strategy: "reroute to alternative tool or check resource availability"
                .to_string(),
            deterministic: true,
        };
    }

    // Parse/validation errors → replan (the approach was wrong)
    if error_lower.contains("parse")
        || error_lower.contains("invalid")
        || error_lower.contains("validation")
        || error_lower.contains("malformed")
    {
        return RepairDiagnosis {
            kind: DiagnosisKind::Replan,
            confidence: 0.65,
            explanation: format!(
                "subtask '{subtask_id}' failed with parse/validation error — plan may be wrong"
            ),
            suggested_strategy: "replan the subtask approach and try again".to_string(),
            deterministic: true,
        };
    }

    // Multiple previous attempts → escalate
    if previous_attempts >= 2 {
        return RepairDiagnosis {
            kind: DiagnosisKind::Escalate,
            confidence: 0.6,
            explanation: format!(
                "subtask '{subtask_id}' failed after {previous_attempts} attempts — escalate"
            ),
            suggested_strategy: "escalate to human review or comprehensive replan".to_string(),
            deterministic: true,
        };
    }

    // Generic failure → retry once, then escalate
    if previous_attempts == 0 {
        RepairDiagnosis {
            kind: DiagnosisKind::Retry,
            confidence: 0.4,
            explanation: format!("subtask '{subtask_id}' failed with generic error — retry"),
            suggested_strategy: "retry with same configuration".to_string(),
            deterministic: false,
        }
    } else {
        RepairDiagnosis {
            kind: DiagnosisKind::Escalate,
            confidence: 0.5,
            explanation: format!(
                "subtask '{subtask_id}' failed again after {previous_attempts} retries — escalate"
            ),
            suggested_strategy: "escalate: previous retry did not resolve".to_string(),
            deterministic: true,
        }
    }
}

/// Convert a repair diagnosis to a strategy adjustment hint for the repair context.
pub fn diagnosis_to_strategy_adjustment(diagnosis: &RepairDiagnosis) -> Value {
    let strategy = match diagnosis.kind {
        DiagnosisKind::Retry => "continue targeted retry with context-preserving adjustments",
        DiagnosisKind::Reroute => "switch tool/agent assignment for this subtask",
        DiagnosisKind::Replan => "replan the subtask approach from scratch",
        DiagnosisKind::Repair => "apply specific repair action",
        DiagnosisKind::Escalate => "escalate to human or comprehensive replan",
    };

    json!({
        "diagnosis": format!("{:?}", diagnosis.kind),
        "confidence": diagnosis.confidence,
        "strategy": strategy,
        "explanation": diagnosis.explanation,
    })
}

/// Summarize diagnoses into a readable snapshot for the repair history.
pub fn diagnose_and_summarize(
    subtask_id: &str,
    outcome: &str,
    error_message: Option<&str>,
    previous_attempts: usize,
) -> Value {
    let diagnosis = diagnose_repair(subtask_id, outcome, error_message, previous_attempts);
    diagnosis_to_strategy_adjustment(&diagnosis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_diagnoses_retry() {
        let d = diagnose_repair("t1", "failed", Some("timed out after 30s"), 0);
        assert_eq!(d.kind, DiagnosisKind::Retry);
    }

    #[test]
    fn rate_limit_diagnoses_retry() {
        let d = diagnose_repair("t2", "failed", Some("rate limit exceeded"), 0);
        assert_eq!(d.kind, DiagnosisKind::Retry);
    }

    #[test]
    fn permission_diagnoses_reroute() {
        let d = diagnose_repair("t3", "failed", Some("permission denied"), 0);
        assert_eq!(d.kind, DiagnosisKind::Reroute);
    }

    #[test]
    fn not_found_diagnoses_reroute() {
        let d = diagnose_repair("t4", "failed", Some("tool not found"), 0);
        assert_eq!(d.kind, DiagnosisKind::Reroute);
    }

    #[test]
    fn parse_error_diagnoses_replan() {
        let d = diagnose_repair("t5", "failed", Some("parse error: invalid json"), 0);
        assert_eq!(d.kind, DiagnosisKind::Replan);
    }

    #[test]
    fn multiple_retries_diagnose_escalate() {
        let d = diagnose_repair("t6", "failed", Some("generic error"), 3);
        assert_eq!(d.kind, DiagnosisKind::Escalate);
    }

    #[test]
    fn first_generic_failure_retries() {
        let d = diagnose_repair("t7", "failed", Some("something went wrong"), 0);
        assert_eq!(d.kind, DiagnosisKind::Retry);
    }

    #[test]
    fn second_generic_failure_escalates() {
        let d = diagnose_repair("t8", "failed", Some("something went wrong"), 1);
        assert_eq!(d.kind, DiagnosisKind::Escalate);
    }
}
