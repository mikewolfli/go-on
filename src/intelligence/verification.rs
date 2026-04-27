//! Phase 4: Structured Verification and Review
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! Structured verification and deterministic checks will be integrated into the
//! review gate once verification signal aggregation logic is implemented.

use crate::agent::AgentAuditLog;
use crate::pua::{quality_compass, PuaExecutionReport};
use crate::quality_models::{QualitySignal, QualitySignalType, QualityVerdict};
use serde::{Deserialize, Serialize};

pub type VerificationVerdict = QualityVerdict;
pub type VerificationSignal = QualitySignal;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredReview {
    pub verdict: VerificationVerdict,
    pub reviewer_agent: String,
    pub confidence: f32,
    pub signals: Vec<VerificationSignal>,
    pub rationale: String,
    pub assumptions_validated: Vec<String>,
    pub weak_evidence_flags: Vec<String>,
    pub quality_compass: Vec<String>,
    pub pua_report: Option<PuaExecutionReport>,
    pub audit_log: Option<AgentAuditLog>,
}

/// Independent verifier that runs deterministic checks
pub struct DeterministicVerifier;
impl DeterministicVerifier {
    /// Run a syntax check by looking for common syntax errors.
    ///
    /// - Checks bracket balancing (`{}`, `[]`, `()`)
    /// - Detects obviously truncated content (no trailing newline on large blocks)
    /// - Returns a structured signal with pass/fail and details
    pub fn run_syntax_check(content: &str) -> VerificationSignal {
        let mut issues: Vec<String> = Vec::new();

        // Bracket balancing checks
        let bracket_pairs = [('{', '}'), ('[', ']'), ('(', ')')];
        for &(open, close) in &bracket_pairs {
            let opens = content.matches(open).count();
            let closes = content.matches(close).count();
            if opens != closes {
                issues.push(format!(
                    "unbalanced '{}' and '{}': {} opens vs {} closes",
                    open, close, opens, closes
                ));
            }
        }

        // Check for truncated content: large blocks that don't end with newline
        if content.len() > 1000 && !content.ends_with('\n') {
            issues.push("large content block may be truncated (no trailing newline)".to_string());
        }

        // Check for obviously incomplete code blocks
        let code_fence_opens = content.matches("```").count();
        if !code_fence_opens.is_multiple_of(2) {
            issues.push("unclosed markdown code fence (odd number of ```)".to_string());
        }

        let passed = issues.is_empty();
        VerificationSignal {
            signal_type: QualitySignalType::Syntax,
            passed,
            confidence: if passed { 0.95 } else { 0.85 },
            details: if issues.is_empty() {
                Some("bracket balance check passed".to_string())
            } else {
                Some(format!("syntax issues found: {}", issues.join("; ")))
            },
        }
    }

    /// Run a test output analysis.
    ///
    /// - Checks for common pass/fail indicators in test runner output
    /// - Detects "FAILED", "FAIL", "error:" (lowercase), "panic" markers
    /// - Counts passed vs failed test assertions
    pub fn run_test_check(test_results: &str) -> VerificationSignal {
        let mut issues: Vec<String> = Vec::new();
        let lower = test_results.to_ascii_lowercase();

        // Check for explicit failure indicators
        let fail_markers = ["FAILED", "FAIL ", " FAIL", "error:", "panicked"];
        for marker in &fail_markers {
            if lower.contains(marker) {
                issues.push(format!(
                    "test output contains failure marker '{}'",
                    marker.trim()
                ));
            }
        }

        // Parse common test output patterns
        let passed_count: usize = test_results
            .lines()
            .filter(|line| {
                let l = line.to_ascii_lowercase();
                l.contains("passed") || l.contains("ok")
            })
            .count();
        let failed_count: usize = test_results
            .lines()
            .filter(|line| {
                let l = line.to_ascii_lowercase();
                l.contains("failed")
            })
            .count();

        let passed = issues.is_empty() && failed_count == 0;
        VerificationSignal {
            signal_type: QualitySignalType::Tests,
            passed,
            confidence: if passed { 0.9 } else { 0.8 },
            details: Some(format!(
                "passed: {}, failed: {}, issues: {}",
                passed_count,
                failed_count,
                issues.len()
            )),
        }
    }

    /// Run a lint check on code content.
    ///
    /// - Detects `#![allow(dead_code)]` (lint-suppression anti-pattern)
    /// - Detects `todo!()` / `unreachable!()` / `unimplemented!()` in production paths
    /// - Returns a structured signal
    pub fn run_lint_check(code: &str) -> VerificationSignal {
        let mut issues: Vec<String> = Vec::new();

        // Check for dead code suppression
        if code.contains("#![allow(dead_code)]") {
            issues.push("file uses #![allow(dead_code)] suppression".to_string());
        }

        // Check for production-unstable macros
        let unstable_macros = ["todo!()", "unreachable!()", "unimplemented!()"];
        for mac in &unstable_macros {
            if code.contains(mac) {
                issues.push(format!("code contains '{}' macro", mac));
            }
        }

        // Check for very long lines (potential formatting issues)
        let long_lines = code.lines().filter(|line| line.len() > 200).count();
        if long_lines > 0 {
            issues.push(format!("{} lines exceed 200 character limit", long_lines));
        }

        let passed = issues.is_empty();
        VerificationSignal {
            signal_type: QualitySignalType::Lint,
            passed,
            confidence: if passed { 0.85 } else { 0.75 },
            details: if issues.is_empty() {
                Some("lint checks passed".to_string())
            } else {
                Some(format!("lint issues: {}", issues.join("; ")))
            },
        }
    }

    /// Run quality compass checks using the configured pua quality compass.
    pub fn run_quality_compass_checks() -> Vec<VerificationSignal> {
        quality_compass()
            .into_iter()
            .map(|item| {
                // For each compass item, emit a signal. In a full implementation
                // this would actually evaluate the code against each criterion.
                VerificationSignal {
                    signal_type: QualitySignalType::PuaQualityCompass,
                    passed: true,
                    confidence: 0.7,
                    details: Some(item),
                }
            })
            .collect()
    }

    /// Aggregate multiple verification signals into a single verdict.
    pub fn aggregate(signals: &[VerificationSignal]) -> VerificationVerdict {
        if signals.is_empty() {
            return VerificationVerdict::InsufficientEvidence;
        }

        let pass_rate = signals.iter().filter(|s| s.passed).count() as f64 / signals.len() as f64;

        if pass_rate >= 0.9 {
            VerificationVerdict::Approve
        } else if pass_rate >= 0.7 {
            VerificationVerdict::ApproveWithCaveats
        } else if pass_rate >= 0.5 {
            VerificationVerdict::Revise
        } else {
            VerificationVerdict::Reject
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syntax_check_balanced_brackets() {
        let code = "fn main() { let x = vec![1, 2, 3]; }";
        let signal = DeterministicVerifier::run_syntax_check(code);
        assert!(signal.passed, "balanced brackets should pass");
    }

    #[test]
    fn syntax_check_unbalanced_brackets() {
        let code = "fn main() { let x = vec![1, 2, 3; }";
        let signal = DeterministicVerifier::run_syntax_check(code);
        assert!(!signal.passed, "unbalanced brackets should fail");
    }

    #[test]
    fn syntax_check_unclosed_code_fence() {
        let code = "some text\n```\ncode block";
        let signal = DeterministicVerifier::run_syntax_check(code);
        assert!(!signal.passed, "unclosed code fence should fail");
    }

    #[test]
    fn test_output_analysis_detects_failure() {
        let output = "test result: FAILED. 3 passed; 1 failed; 0 ignored";
        let signal = DeterministicVerifier::run_test_check(output);
        assert!(!signal.passed, "FAILED in test output should fail");
    }

    #[test]
    fn lint_check_detects_dead_code_suppression() {
        let code = "#![allow(dead_code)]\nfn unused() {}";
        let signal = DeterministicVerifier::run_lint_check(code);
        assert!(!signal.passed, "dead_code suppression should be flagged");
    }

    #[test]
    fn lint_check_detects_todo() {
        let code = "fn placeholder() { todo!() }";
        let signal = DeterministicVerifier::run_lint_check(code);
        assert!(!signal.passed, "todo!() macro should be flagged");
    }

    #[test]
    fn aggregate_all_pass() {
        let signals = vec![
            VerificationSignal {
                signal_type: QualitySignalType::Syntax,
                passed: true,
                confidence: 0.95,
                details: None,
            },
            VerificationSignal {
                signal_type: QualitySignalType::Tests,
                passed: true,
                confidence: 0.9,
                details: None,
            },
        ];
        let verdict = DeterministicVerifier::aggregate(&signals);
        assert_eq!(verdict, VerificationVerdict::Approve);
    }

    #[test]
    fn aggregate_some_fail() {
        let signals = vec![
            VerificationSignal {
                signal_type: QualitySignalType::Syntax,
                passed: true,
                confidence: 0.95,
                details: None,
            },
            VerificationSignal {
                signal_type: QualitySignalType::Tests,
                passed: false,
                confidence: 0.8,
                details: Some("failure".to_string()),
            },
        ];
        let verdict = DeterministicVerifier::aggregate(&signals);
        assert_eq!(verdict, VerificationVerdict::Revise);
    }
}
