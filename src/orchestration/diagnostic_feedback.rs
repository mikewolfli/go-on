//! Diagnostic Feedback Loop — Compiler/LSP error integration and
//! automated problem-solving enhancement for the BrainLoop.
//!
//! Captures compiler errors, linter warnings, and test failures to
//! inform the reflect/replan phases of the BrainLoop with structured
//! diagnostic data. Enables the system to self-correct based on
//! build output feedback.

// F-GAP-51: dead_code allowed on specific items below (reserved for full diagnostic integration)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// DiagnosticSeverity
// ---------------------------------------------------------------------------

/// Severity of a diagnostic message.
#[allow(dead_code)] // F-GAP-51 — reserved for full diagnostic integration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

impl DiagnosticSeverity {
    #[cfg(test)]
    pub fn label(&self) -> &str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Hint => "hint",
        }
    }
}

// ---------------------------------------------------------------------------
// DiagnosticMessage
// ---------------------------------------------------------------------------

/// A single diagnostic message parsed from compiler/LSP output.
#[allow(dead_code)] // F-GAP-51 — reserved for full diagnostic integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticMessage {
    /// File path where the diagnostic originated.
    pub file: String,
    /// Line number (1-based).
    pub line: usize,
    /// Column number (1-based).
    pub column: usize,
    /// Severity level.
    pub severity: DiagnosticSeverity,
    /// Error/warning code (e.g. "E0308").
    pub code: Option<String>,
    /// Human-readable message.
    pub message: String,
    /// Suggested fix hint, if available.
    pub suggestion: Option<String>,
    /// The source line that triggered the diagnostic.
    pub source_snippet: Option<String>,
}

// ---------------------------------------------------------------------------
// DiagnosticBatch
// ---------------------------------------------------------------------------

/// A batch of diagnostic messages from a single compilation run.
#[allow(dead_code)] // F-GAP-51 — reserved for full diagnostic integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticBatch {
    /// Unique identifier for this batch.
    pub batch_id: String,
    /// Timestamp when this batch was collected.
    pub timestamp_ms: u64,
    /// Total number of errors.
    pub error_count: usize,
    /// Total number of warnings.
    pub warning_count: usize,
    /// Total number of info/hints.
    pub info_count: usize,
    /// Individual diagnostic messages.
    pub messages: Vec<DiagnosticMessage>,
    /// Which phase of the BrainLoop this batch corresponds to.
    pub loop_phase: Option<String>,
}

#[allow(dead_code)] // F-GAP-51 — reserved for full diagnostic integration
impl DiagnosticBatch {
    pub fn new(messages: Vec<DiagnosticMessage>) -> Self {
        let error_count = messages
            .iter()
            .filter(|m| m.severity == DiagnosticSeverity::Error)
            .count();
        let warning_count = messages
            .iter()
            .filter(|m| m.severity == DiagnosticSeverity::Warning)
            .count();
        let info_count = messages.len() - error_count - warning_count;
        let mut batch_id = String::with_capacity(41); // "diag-" + 36 UUID chars
        batch_id.push_str("diag-");
        batch_id.push_str(&uuid::Uuid::new_v4().as_hyphenated().to_string());
        Self {
            batch_id,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            error_count,
            warning_count,
            info_count,
            messages,
            loop_phase: None,
        }
    }

    /// Whether this batch contains any errors (blocking issues).
    pub fn has_errors(&self) -> bool {
        self.error_count > 0
    }

    /// Generate a summary suitable for BrainLoop reflect phase.
    #[allow(dead_code)] // F-GAP-11 — reserved for BrainLoop reflect phase integration
    pub fn summary(&self) -> String {
        format!(
            "Build diagnostics: {} errors, {} warnings, {} info",
            self.error_count, self.warning_count, self.info_count
        )
    }

    /// Extract a prioritized list of files that need attention.
    #[cfg(test)]
    pub fn affected_files(&self) -> Vec<String> {
        let mut files: Vec<String> = self.messages.iter().map(|m| m.file.clone()).collect();
        files.sort();
        files.dedup();
        files
    }
}

// ---------------------------------------------------------------------------
// DiagnosticPattern
// ---------------------------------------------------------------------------

/// A known diagnostic pattern that maps to a repair strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)] // F-GAP-51 — reserved for full diagnostic integration
pub struct DiagnosticPattern {
    /// Error code pattern (e.g. "E0308", "borrowck", "unused").
    pub pattern: String,
    /// Human-readable description of what this pattern means.
    pub description: String,
    /// Suggested repair strategy name.
    pub repair_strategy: String,
    /// How many times this pattern has been matched.
    pub match_count: u64,
    /// Historical success rate of the repair strategy [0.0, 1.0].
    pub repair_success_rate: f64,
}

// ---------------------------------------------------------------------------
// DiagnosticFeedbackEngine
// ---------------------------------------------------------------------------

/// Central engine that collects diagnostics and provides feedback
/// to the BrainLoop for self-correction.
#[allow(dead_code)] // F-GAP-51 — reserved for full diagnostic integration
pub struct DiagnosticFeedbackEngine {
    /// History of diagnostic batches.
    history: Vec<DiagnosticBatch>,
    /// Known patterns and their repair strategies.
    patterns: HashMap<String, DiagnosticPattern>,
    /// Max history size before pruning.
    max_history: usize,
}

#[allow(dead_code)] // F-GAP-51 — reserved for full diagnostic integration
impl DiagnosticFeedbackEngine {
    pub fn new() -> Self {
        let mut engine = Self {
            history: Vec::with_capacity(50),
            patterns: HashMap::with_capacity(16),
            max_history: 50,
        };
        engine.register_builtin_patterns();
        engine
    }

    /// Register built-in known error patterns.
    fn register_builtin_patterns(&mut self) {
        let builtins = vec![
            ("E0308", "Type mismatch", "fix_type_mismatch"),
            ("E0599", "Method not found", "fix_method_not_found"),
            ("E0425", "Cannot find value", "fix_undefined_variable"),
            ("E0432", "Invalid import", "fix_invalid_import"),
            ("E0061", "Wrong argument count", "fix_argument_count"),
            ("E0502", "Borrow conflict", "fix_borrow_conflict"),
            ("E0507", "Cannot move out", "fix_move_error"),
            ("E0382", "Use after move", "fix_use_after_move"),
            ("E0597", "Borrow lifetime", "fix_lifetime_error"),
            ("E0277", "Trait bound not satisfied", "fix_trait_bound"),
            ("borrowck", "Borrow checker error", "fix_borrow_error"),
            ("unused_import", "Unused import", "remove_unused_import"),
            (
                "unused_variable",
                "Unused variable",
                "remove_unused_variable",
            ),
            ("dead_code", "Dead code", "remove_dead_code"),
            ("unreachable", "Unreachable code", "fix_unreachable_code"),
        ];
        for (pattern, desc, strategy) in builtins {
            self.patterns.insert(
                pattern.to_string(),
                DiagnosticPattern {
                    pattern: pattern.to_string(),
                    description: desc.to_string(),
                    repair_strategy: strategy.to_string(),
                    match_count: 0,
                    repair_success_rate: 0.0,
                },
            );
        }
    }

    /// Submit a new diagnostic batch for analysis.
    pub fn submit_batch(&mut self, batch: DiagnosticBatch) {
        // Update pattern match counts
        for msg in &batch.messages {
            if let Some(ref code) = msg.code {
                if let Some(pattern) = self.patterns.get_mut(code) {
                    pattern.match_count += 1;
                }
            }
        }

        self.history.push(batch);
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }
    }

    /// Get the most recent diagnostic batch.
    pub fn latest_batch(&self) -> Option<&DiagnosticBatch> {
        self.history.last()
    }

    /// Get a repair strategy recommendation based on the current diagnostics.
    pub fn recommend_repair(&self) -> Option<(String, String)> {
        let batch = self.latest_batch()?;
        if !batch.has_errors() {
            return None;
        }
        // Find the first error with a known pattern
        for msg in &batch.messages {
            if msg.severity == DiagnosticSeverity::Error {
                if let Some(ref code) = msg.code {
                    if let Some(pattern) = self.patterns.get(code) {
                        return Some((
                            pattern.repair_strategy.clone(),
                            pattern.description.clone(),
                        ));
                    }
                }
            }
        }
        None
    }

    /// Calculate error trend: decreasing, stable, or increasing.
    pub fn error_trend(&self) -> &str {
        if self.history.len() < 2 {
            return "stable";
        }
        let len = self.history.len();
        let recent = self.history[len - 1].error_count;
        let previous = self.history[len - 2].error_count;
        if recent < previous {
            "decreasing"
        } else if recent > previous {
            "increasing"
        } else {
            "stable"
        }
    }

    /// Total errors across all batches in history.
    #[allow(dead_code)] // F-GAP-11 — reserved for diagnostic statistics
    pub fn total_errors(&self) -> usize {
        self.history.iter().map(|b| b.error_count).sum()
    }

    /// Total warnings across all batches.
    #[allow(dead_code)] // F-GAP-11 — reserved for diagnostic statistics
    pub fn total_warnings(&self) -> usize {
        self.history.iter().map(|b| b.warning_count).sum()
    }

    /// Known patterns count.
    #[cfg(test)]
    pub fn pattern_count(&self) -> usize {
        self.patterns.len()
    }
}

impl Default for DiagnosticFeedbackEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(
        severity: DiagnosticSeverity,
        code: &str,
        file: &str,
        line: usize,
        msg: &str,
    ) -> DiagnosticMessage {
        DiagnosticMessage {
            file: file.to_string(),
            line,
            column: 1,
            severity,
            code: Some(code.to_string()),
            message: msg.to_string(),
            suggestion: None,
            source_snippet: None,
        }
    }

    #[test]
    fn test_diagnostic_batch_has_errors() {
        let msgs = vec![
            make_msg(
                DiagnosticSeverity::Error,
                "E0308",
                "src/main.rs",
                10,
                "type mismatch",
            ),
            make_msg(
                DiagnosticSeverity::Warning,
                "unused",
                "src/lib.rs",
                5,
                "unused var",
            ),
        ];
        let batch = DiagnosticBatch::new(msgs);
        assert!(batch.has_errors());
        assert_eq!(batch.error_count, 1);
        assert_eq!(batch.warning_count, 1);
    }

    #[test]
    fn test_diagnostic_batch_no_errors() {
        let msgs = vec![make_msg(
            DiagnosticSeverity::Warning,
            "unused",
            "src/lib.rs",
            5,
            "unused",
        )];
        let batch = DiagnosticBatch::new(msgs);
        assert!(!batch.has_errors());
    }

    #[test]
    fn test_affected_files() {
        let msgs = vec![
            make_msg(DiagnosticSeverity::Error, "E0308", "src/main.rs", 10, ""),
            make_msg(DiagnosticSeverity::Error, "E0308", "src/main.rs", 15, ""),
            make_msg(DiagnosticSeverity::Warning, "unused", "src/lib.rs", 5, ""),
        ];
        let batch = DiagnosticBatch::new(msgs);
        let files = batch.affected_files();
        assert_eq!(files.len(), 2);
        assert!(files.contains(&"src/main.rs".to_string()));
    }

    #[test]
    fn test_feedback_engine_builtin_patterns() {
        let engine = DiagnosticFeedbackEngine::new();
        assert!(engine.pattern_count() > 10);
    }

    #[test]
    fn test_feedback_engine_submit_and_recommend() {
        let mut engine = DiagnosticFeedbackEngine::new();
        let msgs = vec![make_msg(
            DiagnosticSeverity::Error,
            "E0308",
            "src/main.rs",
            10,
            "type mismatch",
        )];
        let batch = DiagnosticBatch::new(msgs);
        engine.submit_batch(batch);

        let rec = engine.recommend_repair();
        assert!(rec.is_some());
        assert_eq!(rec.unwrap().0, "fix_type_mismatch");
    }

    #[test]
    fn test_error_trend_decreasing() {
        let mut engine = DiagnosticFeedbackEngine::new();
        engine.submit_batch(DiagnosticBatch::new(vec![
            make_msg(DiagnosticSeverity::Error, "E1", "a.rs", 1, ""),
            make_msg(DiagnosticSeverity::Error, "E1", "a.rs", 2, ""),
        ]));
        engine.submit_batch(DiagnosticBatch::new(vec![make_msg(
            DiagnosticSeverity::Error,
            "E1",
            "a.rs",
            1,
            "",
        )]));
        assert_eq!(engine.error_trend(), "decreasing");
    }

    #[test]
    fn test_severity_labels() {
        assert_eq!(DiagnosticSeverity::Error.label(), "error");
        assert_eq!(DiagnosticSeverity::Warning.label(), "warning");
    }
}
