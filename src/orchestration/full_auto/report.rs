//! Reporting and analytics for the full-auto flow.
//!
//! Provides the audit trail types that capture what happened during
//! a [`FullAutoFlow`](super::FullAutoFlow) execution run.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// SkillMatch
// ---------------------------------------------------------------------------

/// A skill matched to the task, together with its composite score and a
/// human‑readable rationale.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMatch {
    /// Skill name (matches `SkillRegistry` key).
    pub name: String,
    /// Human-readable description of the skill.
    pub description: String,
    /// Composite match score (0.0 – 1.0).
    pub score: f64,
    /// Human-readable explanation of the score.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// ExecutionStep
// ---------------------------------------------------------------------------

/// A single step recorded in the execution audit trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionStep {
    /// Name of the skill that was executed.
    pub skill_name: String,
    /// The input value provided to the skill.
    pub input: Value,
    /// The output value returned by the skill (or `Null` on failure).
    pub output: Value,
    /// Whether the execution completed without error.
    pub success: bool,
    /// Wall-clock duration of this step in milliseconds.
    pub duration_ms: u64,
    /// Monotonic timestamp (milliseconds since flow start).
    pub timestamp_ms: u64,
    /// Error message if the step failed, or `None`.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// AutoExecutionReport
// ---------------------------------------------------------------------------

/// Full audit trail for an automatic execution run.
///
/// Contains every stage from parsing through environment preparation to
/// skill execution, enabling full traceability of what happened and why.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoExecutionReport {
    /// The structured task intent derived from the raw description.
    pub task_intent: super::intent::TaskIntent,
    /// Skills that were matched and considered for execution.
    pub matched_skills: Vec<SkillMatch>,
    /// Environment state at the time of the run.
    pub environment_status: super::environment::ExecutionEnvironment,
    /// Ordered log of every skill execution attempt.
    pub execution_log: Vec<ExecutionStep>,
    /// Final consolidated output, if any.
    pub final_output: Option<String>,
    /// Non‑fatal errors that occurred during the flow.
    pub errors: Vec<String>,
    /// Total wall‑clock duration of the entire flow in milliseconds.
    pub total_duration_ms: u64,
    /// Cache metrics snapshot from the fast-path cache.
    pub cache_metrics: Value,
}

impl AutoExecutionReport {
    /// Returns `true` when all matched skills completed successfully
    /// and no errors were recorded.
    pub fn is_success(&self) -> bool {
        self.errors.is_empty() && self.execution_log.iter().all(|s| s.success)
    }

    /// Returns the number of successful steps in the execution log.
    pub fn success_count(&self) -> usize {
        self.execution_log.iter().filter(|s| s.success).count()
    }

    /// Returns the number of failed steps in the execution log.
    pub fn failure_count(&self) -> usize {
        self.execution_log.iter().filter(|s| !s.success).count()
    }
}
