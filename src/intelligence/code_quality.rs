//! GAP-B53-57: Proactive code quality detection hooks.
//!
//! Provides analyzers that detect code quality issues (dead code, complexity,
//! style violations) and generate evolution triggers for the self-improvement
//! loop. Hooks run periodically and on plan completion.

use crate::orchestration::self_evolution::evolution_loop::EvolutionTrigger;
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

    /// Returns only dead code issues.
    /// Public API for consumers of `CodeQualityReport`.
    #[allow(dead_code)]
    pub fn dead_code_issues(&self) -> Vec<&CodeQualityIssue> {
        self.issues
            .iter()
            .filter(|i| matches!(i, CodeQualityIssue::DeadCode { .. }))
            .collect()
    }

    /// Converts the report into evolution triggers for self-improvement.
    /// Public API for consumers of `CodeQualityReport`.
    #[allow(dead_code)]
    pub fn to_evolution_triggers(&self) -> Vec<EvolutionTrigger> {
        let mut triggers = Vec::new();

        for issue in &self.issues {
            match issue {
                CodeQualityIssue::DeadCode { module, ratio } => {
                    triggers.push(EvolutionTrigger::DeadCodeDetected {
                        module: module.clone(),
                        ratio: *ratio,
                    });
                }
                CodeQualityIssue::HighComplexity { module, .. } => {
                    triggers.push(EvolutionTrigger::PerformanceRegression {
                        metric: format!("complexity::{module}"),
                        threshold: 0.5,
                        direction: crate::orchestration::self_evolution::evolution_loop::RegressionDirection::Increasing,
                    });
                }
                CodeQualityIssue::MissingDocs { module, count } => {
                    if *count > 5 {
                        triggers.push(EvolutionTrigger::ManualRequest {
                            instruction: format!(
                                "Add missing documentation in module '{module}' ({count} items)"
                            ),
                        });
                    }
                }
                CodeQualityIssue::UnsafePattern { module, pattern } => {
                    triggers.push(EvolutionTrigger::ConfigDrift {
                        key: format!("unsafe_pattern::{module}"),
                        expected: "no_unsafe_usage".to_string(),
                        actual: format!("unsafe pattern: {pattern}"),
                    });
                }
            }
        }

        triggers
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

/// Hook: call this after a BrainLoop plan completes to check for
/// code quality regressions introduced during the plan.
///
/// Wired via pre_patch_quality_gate() in sandbox.apply_patch();
/// post_plan variant is public API for external plan-completion hooks.
#[allow(dead_code)]
pub fn post_plan_quality_hook() -> CodeQualityReport {
    run_code_quality_scan()
}

/// Hook: call this before an EvolutionLoop patch is applied to ensure
/// the change does not degrade code quality.
pub fn pre_patch_quality_gate() -> CodeQualityReport {
    run_code_quality_scan()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_report() {
        let report = CodeQualityReport {
            issues: vec![],
            health_score: 1.0,
            modules_scanned: 10,
            scanned_at_ms: 1000,
        };
        assert!(report.is_clean());
        assert!(report.to_evolution_triggers().is_empty());
    }

    #[test]
    fn test_dead_code_issue_creates_trigger() {
        let report = CodeQualityReport {
            issues: vec![CodeQualityIssue::DeadCode {
                module: "src/foo.rs".to_string(),
                ratio: 0.3,
            }],
            health_score: 0.7,
            modules_scanned: 1,
            scanned_at_ms: 1000,
        };

        let triggers = report.to_evolution_triggers();
        assert_eq!(triggers.len(), 1);
        match &triggers[0] {
            EvolutionTrigger::DeadCodeDetected { module, ratio } => {
                assert_eq!(module, "src/foo.rs");
                assert!((ratio - 0.3).abs() < 1e-6);
            }
            _ => panic!("Expected DeadCodeDetected trigger"),
        }
    }

    #[test]
    fn test_dead_code_filter() {
        let report = CodeQualityReport {
            issues: vec![
                CodeQualityIssue::DeadCode {
                    module: "a.rs".to_string(),
                    ratio: 0.2,
                },
                CodeQualityIssue::HighComplexity {
                    module: "b.rs".to_string(),
                    score: 50,
                },
            ],
            health_score: 0.5,
            modules_scanned: 2,
            scanned_at_ms: 1000,
        };

        let dead = report.dead_code_issues();
        assert_eq!(dead.len(), 1);
    }

    #[test]
    fn test_run_code_quality_scan() {
        let report = run_code_quality_scan();
        // The scan runs cargo clippy as a subprocess. We validate the report
        // structure is valid regardless of whether clippy found issues.
        assert!(report.scanned_at_ms > 0, "scan should produce a timestamp");
        // health_score is 1.0 if no issues, lower if issues found, 0.0 if scan failed.
        assert!(
            report.health_score >= 0.0,
            "health_score must be non-negative"
        );
        assert!(
            report.health_score <= 1.0,
            "health_score must not exceed 1.0"
        );
    }

    #[test]
    fn test_post_plan_quality_hook() {
        let report = post_plan_quality_hook();
        // post_plan_quality_hook delegates to run_code_quality_scan.
        // Validate report structure.
        assert!(
            report.scanned_at_ms > 0,
            "post_plan hook should produce a timestamp"
        );
        assert!(
            report.health_score >= 0.0,
            "health_score must be non-negative"
        );
    }
}
