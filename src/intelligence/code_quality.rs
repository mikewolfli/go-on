//! GAP-B53-57: Proactive code quality detection hooks.
//!
//! Provides analyzers that detect code quality issues (dead code, complexity,
//! style violations) and generate evolution triggers for the self-improvement
//! loop. Hooks run periodically and on plan completion.

use serde::{Deserialize, Serialize};

/// Types of code quality issues that can be detected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CodeQualityIssue {
    /// Unused imports or dead code blocks.
    DeadCode { module: String, ratio: f64 },
    /// Overly complex functions or modules.
    HighComplexity { module: String, score: u64 },
    /// Missing documentation on public API items.
    MissingDocs { module: String, count: usize },
    /// Deprecated or unsafe patterns detected.
    UnsafePattern { module: String, pattern: String },
}

/// Result of a code quality scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeQualityReport {
    /// Issues found during the scan.
    pub issues: Vec<CodeQualityIssue>,
    /// Overall health score (0.0 = worst, 1.0 = best).
    pub health_score: f64,
    /// Number of modules scanned.
    pub modules_scanned: usize,
    /// Timestamp of the scan.
    pub scanned_at_ms: u64,
}

impl CodeQualityReport {
    /// Returns true if no issues were found.
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }
}

/// Hook that performs a code quality analysis and returns a report.
///
/// Runs `cargo clippy` and parses its output to produce a structured
/// `CodeQualityReport` with per-issue entries and a health score.
pub fn run_code_quality_scan() -> CodeQualityReport {
    let mut issues = Vec::new();
    let scanned_at_ms = crate::intelligence::now_ms();

    match std::process::Command::new("cargo")
        .args(["clippy", "--message-format=short", "--quiet"])
        .output()
    {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let combined = format!("{stdout}\n{stderr}");

            // Track unique file paths that appeared in clippy output
            let modules_scanned_set: std::collections::HashSet<&str> = combined
                .lines()
                .filter_map(|l| {
                    let trimmed = l.trim();
                    // Lines like "src/foo.rs:12:34: warning: ..."
                    if trimmed.contains(".rs:") || trimmed.contains(".toml:") {
                        trimmed.split(':').next()
                    } else {
                        None
                    }
                })
                .collect();

            let modules_scanned = modules_scanned_set.len();

            // Count warnings and errors
            let warning_count = combined.lines().filter(|l| l.contains("warning:")).count();
            let error_count = combined.lines().filter(|l| l.contains("error:")).count();

            // Create issues for each warning
            for line in combined.lines().filter(|l| l.contains("warning:")) {
                let module = line.split(':').next().unwrap_or("unknown").to_string();

                let issue = if line.contains("unused")
                    || line.contains("dead_code")
                    || line.contains("redundant")
                {
                    CodeQualityIssue::DeadCode {
                        module,
                        ratio: 0.1, // single-issue ratio
                    }
                } else if line.contains("complexity") || line.contains("cognitive") {
                    CodeQualityIssue::HighComplexity { module, score: 80 }
                } else if line.contains("missing_docs") || line.contains("doc") {
                    CodeQualityIssue::MissingDocs { module, count: 1 }
                } else {
                    CodeQualityIssue::UnsafePattern {
                        module,
                        pattern: line.to_string(),
                    }
                };
                issues.push(issue);
            }

            // Create issues for each error
            for line in combined.lines().filter(|l| l.contains("error:")) {
                let module = line.split(':').next().unwrap_or("unknown").to_string();
                // Errors are typically compilation or clippy errors; map as UnsafePattern
                issues.push(CodeQualityIssue::UnsafePattern {
                    module,
                    pattern: line.to_string(),
                });
            }

            // Health score: 1.0 - penalty for warnings and errors
            let total_penalty = (warning_count as f64 * 0.01) + (error_count as f64 * 0.05);
            let health_score = (1.0 - total_penalty.min(0.95)).max(0.05);

            CodeQualityReport {
                issues,
                health_score,
                modules_scanned: modules_scanned.max(1),
                scanned_at_ms,
            }
        }
        Err(e) => {
            // GAP-B58-B17: Distinguish "no issues" from "scan failed".
            // When the external command fails we return health_score 0.0 and
            // encode the error into a synthetic issue entry so callers can
            // distinguish a broken scan from a clean result.
            tracing::warn!("Failed to run cargo clippy for code quality scan: {e}");
            CodeQualityReport {
                issues: vec![CodeQualityIssue::UnsafePattern {
                    module: "cargo_clippy".to_string(),
                    pattern: format!("scan failed: {e}"),
                }],
                health_score: 0.0,
                modules_scanned: 0,
                scanned_at_ms,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_code_quality_scan_returns_valid_report() {
        let report = run_code_quality_scan();

        // Verify the report has the expected structure.
        // Issues may or may not be present depending on the project state,
        // but the metadata fields should always be populated.
        assert!(
            report.health_score >= 0.0 && report.health_score <= 1.0,
            "health_score {} out of range [0.0, 1.0]",
            report.health_score
        );
        assert!(
            report.scanned_at_ms > 0,
            "scanned_at_ms should be a positive timestamp"
        );
        // Every issue variant should have a non-empty module string
        for issue in &report.issues {
            let module = match issue {
                CodeQualityIssue::DeadCode { module, .. } => module,
                CodeQualityIssue::HighComplexity { module, .. } => module,
                CodeQualityIssue::MissingDocs { module, .. } => module,
                CodeQualityIssue::UnsafePattern { module, .. } => module,
            };
            assert!(!module.is_empty(), "issue module should not be empty");
        }
        // is_clean is consistent with issues being empty
        assert_eq!(report.is_clean(), report.issues.is_empty());
    }
}
