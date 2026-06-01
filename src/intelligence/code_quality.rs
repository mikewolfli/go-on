//! GAP-B53-57: Proactive code quality detection hooks.
//!
//! Provides analyzers that detect code quality issues (dead code, complexity,
//! style violations) and generate evolution triggers for the self-improvement
//! loop. Hooks run periodically and on plan completion.

#![allow(unused)]

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
    pub fn dead_code_issues(&self) -> Vec<&CodeQualityIssue> {
        self.issues
            .iter()
            .filter(|i| matches!(i, CodeQualityIssue::DeadCode { .. }))
            .collect()
    }

    /// Converts the report into evolution triggers for self-improvement.
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
/// In production, this integrates with `cargo clippy` and static analysis
/// tools. For now, it provides a stub that can be extended.
pub fn run_code_quality_scan() -> CodeQualityReport {
    // Stub: in production, run `cargo clippy` and parse output.
    // For now, return a clean report indicating no issues found.
    CodeQualityReport {
        issues: Vec::new(),
        health_score: 1.0,
        modules_scanned: 0,
        scanned_at_ms: crate::intelligence::now_ms(),
    }
}

/// Hook: call this after a BrainLoop plan completes to check for
/// code quality regressions introduced during the plan.
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
        // Stub returns clean.
        assert!(report.is_clean());
        assert!(report.scanned_at_ms > 0);
    }

    #[test]
    fn test_post_plan_quality_hook() {
        let report = post_plan_quality_hook();
        assert!(report.is_clean());
    }
}
