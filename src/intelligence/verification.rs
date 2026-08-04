//! Phase 4: Structured Verification and Review
//!
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! Structured verification and deterministic checks will be integrated into the
//! review gate once verification signal aggregation logic is implemented.
//!
//! ## Relationship to `quality_models`
//!
//! - [`VerificationVerdict`] is a type alias for [`QualityVerdict`](crate::quality_models::QualityVerdict).
//! - [`VerificationSignal`] is a type alias for [`QualitySignal`](crate::quality_models::QualitySignal).
//! - This ensures a single source of truth for categorical verdicts; the aliases
//!   exist only for semantic naming within the verification domain.

use crate::agent::AgentAuditLog;
use crate::pua::{quality_compass, PuaExecutionReport};
use crate::quality_models::{QualitySignal, QualitySignalType, QualityVerdict};
use serde::{Deserialize, Serialize};

/// Type alias for [`QualityVerdict`](crate::quality_models::QualityVerdict).
///
/// The verification pipeline uses this alias for semantic clarity — a
/// `VerificationVerdict` is the outcome of running structured checks.
/// It delegates to the canonical [`QualityVerdict`](crate::quality_models::QualityVerdict)
/// enum defined in [`quality_models`](crate::quality_models).
pub type VerificationVerdict = QualityVerdict;

/// Type alias for [`QualitySignal`](crate::quality_models::QualitySignal).
///
/// Provides a semantic name within the verification context for the canonical
/// [`QualitySignal`](crate::quality_models::QualitySignal) type from
/// [`quality_models`](crate::quality_models).
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
    ///
    /// Performs basic content sanity checks — if the content contains obvious issues
    /// such as unclosed brackets or suspicious patterns, the compass check is marked
    /// as failed.
    pub fn run_quality_compass_checks(content: &str) -> Vec<VerificationSignal> {
        // Basic content sanity: detect unclosed brackets / suspicious patterns.
        let content_has_issues = {
            let open_curly = content.matches('{').count();
            let close_curly = content.matches('}').count();
            let open_paren = content.matches('(').count();
            let close_paren = content.matches(')').count();
            let open_bracket = content.matches('[').count();
            let close_bracket = content.matches(']').count();

            open_curly != close_curly || open_paren != close_paren || open_bracket != close_bracket
        };

        quality_compass()
            .into_iter()
            .map(|item| {
                // For each compass item, emit a signal. In a full implementation
                // this would actually evaluate the code against each criterion.
                VerificationSignal {
                    signal_type: QualitySignalType::PuaQualityCompass,
                    passed: !content_has_issues,
                    confidence: if content_has_issues { 0.3 } else { 0.7 },
                    details: Some(item),
                }
            })
            .collect()
    }

    /// Run an adversarial verification check by invoking all four
    /// `AdversarialVerifier` bias probes (Security, Logic, Completeness,
    /// Performance) and aggregating the results into a single signal.
    ///
    /// The signal passes only when ALL adversarial biases pass, indicating
    /// no security, logic, completeness, or performance weaknesses were found.
    pub fn run_adversarial_check(content: &str) -> VerificationSignal {
        let verdicts = AdversarialVerifier::verify_all(content);
        let passed_count = verdicts.iter().filter(|v| v.passed).count();
        let total = verdicts.len();
        let all_passed = passed_count == total;
        let confidence = if total > 0 {
            passed_count as f32 / total as f32
        } else {
            1.0
        };
        let details = if all_passed {
            "all adversarial checks passed".to_string()
        } else {
            let failures: Vec<&str> = verdicts
                .iter()
                .filter(|v| !v.passed)
                .map(|v| match v.bias {
                    AdversarialBias::Security => "security",
                    AdversarialBias::Logic => "logic",
                    AdversarialBias::Completeness => "completeness",
                    AdversarialBias::Performance => "performance",
                })
                .collect();
            format!(
                "adversarial checks failed for: {} (passed {}/{})",
                failures.join(", "),
                passed_count,
                total
            )
        };
        VerificationSignal {
            signal_type: QualitySignalType::RuntimeVerification,
            passed: all_passed,
            confidence,
            details: Some(details),
        }
    }
}

// ---------------------------------------------------------------------------
// AdversarialVerifier — independent verification channel (F-GAP-02)
// ---------------------------------------------------------------------------
//
// Runs a second, independent verification pass from an "adversarial"
// perspective.  The goal is to find weaknesses that the primary
// DeterministicVerifier might miss — logical contradictions, edge cases,
// security implications, and requirement drift.

/// Bias direction for adversarial probing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdversarialBias {
    /// Probe for security vulnerabilities.
    Security,
    /// Probe for logical correctness.
    Logic,
    /// Probe for completeness against requirements.
    Completeness,
    /// Probe for performance / scalability.
    Performance,
}

/// A single adversarial finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdversarialFinding {
    pub category: String,
    pub severity: String, // "low" | "medium" | "high" | "critical"
    pub description: String,
    pub recommendation: String,
}

/// Outcome of an adversarial verification pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdversarialVerdict {
    pub passed: bool,
    pub bias: AdversarialBias,
    pub confidence: f64,
    pub findings: Vec<AdversarialFinding>,
    pub summary: String,
}

/// Independent adversarial verifier that probes for weaknesses.
pub struct AdversarialVerifier;

impl AdversarialVerifier {
    /// Run an adversarial verification pass with the given bias.
    ///
    /// The verifier examines content from the specified angle and returns
    /// findings along with a pass/fail verdict.
    pub fn verify(content: &str, bias: AdversarialBias) -> AdversarialVerdict {
        let mut findings: Vec<AdversarialFinding> = Vec::new();
        let lower = content.to_ascii_lowercase();

        match bias {
            AdversarialBias::Security => {
                // Check for common security anti-patterns
                if lower.contains("unsafe") {
                    findings.push(AdversarialFinding {
                        category: "memory_safety".to_string(),
                        severity: "high".to_string(),
                        description: "code contains 'unsafe' block".to_string(),
                        recommendation:
                            "review unsafe block for soundness; prefer safe abstractions"
                                .to_string(),
                    });
                }
                if lower.contains("eval(") || lower.contains("exec(") {
                    findings.push(AdversarialFinding {
                        category: "code_injection".to_string(),
                        severity: "critical".to_string(),
                        description: "dynamic code execution detected".to_string(),
                        recommendation: "avoid eval/exec; use sandboxed alternatives".to_string(),
                    });
                }
                if lower.contains("password") || lower.contains("secret") {
                    findings.push(AdversarialFinding {
                        category: "credential_exposure".to_string(),
                        severity: "high".to_string(),
                        description: "potential credential in code".to_string(),
                        recommendation: "use environment variables or a secret store".to_string(),
                    });
                }
            }
            AdversarialBias::Logic => {
                // Check for logical contradictions
                if content.matches("true").count() > 0 && content.matches("false").count() > 0 {
                    let true_count = content.matches("true").count();
                    let false_count = content.matches("false").count();
                    if true_count > 10 && false_count > 10 {
                        findings.push(AdversarialFinding {
                            category: "logic_complexity".to_string(),
                            severity: "medium".to_string(),
                            description: format!(
                                "high boolean density: {} true vs {} false literals",
                                true_count, false_count
                            ),
                            recommendation: "consider simplifying boolean logic".to_string(),
                        });
                    }
                }
                // Check for hardcoded magic numbers
                let magic_count = content
                    .lines()
                    .filter(|line| {
                        let trimmed = line.trim();
                        trimmed.starts_with("const") || trimmed.starts_with("let")
                    })
                    .filter(|line| line.chars().filter(|c| c.is_ascii_digit()).count() > 20)
                    .count();
                if magic_count > 3 {
                    findings.push(AdversarialFinding {
                        category: "magic_numbers".to_string(),
                        severity: "low".to_string(),
                        description: format!(
                            "{} lines contain heavy numeric literals",
                            magic_count
                        ),
                        recommendation: "extract magic numbers into named constants".to_string(),
                    });
                }
            }
            AdversarialBias::Completeness => {
                // Check for placeholder / incomplete code
                if lower.contains("todo") || lower.contains("fixme") || lower.contains("xxx") {
                    findings.push(AdversarialFinding {
                        category: "incomplete".to_string(),
                        severity: "medium".to_string(),
                        description: "code contains TODO/FIXME markers".to_string(),
                        recommendation: "resolve all TODO/FIXME before release".to_string(),
                    });
                }
                // Check for missing error handling
                if lower.contains("unwrap(") || lower.contains("expect(") {
                    findings.push(AdversarialFinding {
                        category: "error_handling".to_string(),
                        severity: "medium".to_string(),
                        description: "code uses unwrap/expect which may panic".to_string(),
                        recommendation: "replace with proper error propagation".to_string(),
                    });
                }
            }
            AdversarialBias::Performance => {
                // Check for obvious performance issues
                if lower.contains("clone(") {
                    let clone_count = content.matches("clone(").count();
                    if clone_count > 5 {
                        findings.push(AdversarialFinding {
                            category: "excessive_cloning".to_string(),
                            severity: "low".to_string(),
                            description: format!("{} clone() calls detected", clone_count),
                            recommendation: "prefer references or Arc where possible".to_string(),
                        });
                    }
                }
                // Check for O(n²) patterns
                let nested_loops = content
                    .lines()
                    .filter(|line| line.trim().starts_with("for "))
                    .count();
                if nested_loops > 3 {
                    findings.push(AdversarialFinding {
                        category: "nested_iteration".to_string(),
                        severity: "low".to_string(),
                        description: format!("{} for-loops detected", nested_loops),
                        recommendation: "consider iterator combinators or early exit".to_string(),
                    });
                }
            }
        }

        let passed = findings.is_empty();
        let confidence = if passed {
            0.9
        } else {
            let severity_weights: f64 = findings
                .iter()
                .map(|f| match f.severity.as_str() {
                    "critical" => 0.4,
                    "high" => 0.3,
                    "medium" => 0.2,
                    _ => 0.1,
                })
                .sum::<f64>()
                .min(0.8);
            1.0 - severity_weights
        };

        let summary = if passed {
            format!("{:?} adversarial check passed", bias)
        } else {
            format!(
                "{:?} adversarial check found {} issue(s)",
                bias,
                findings.len()
            )
        };

        AdversarialVerdict {
            passed,
            bias,
            confidence: confidence.max(0.1),
            findings,
            summary,
        }
    }

    /// Run multi-bias adversarial verification.
    ///
    /// Executes all four biases and returns the combined result.
    pub fn verify_all(content: &str) -> Vec<AdversarialVerdict> {
        vec![
            Self::verify(content, AdversarialBias::Security),
            Self::verify(content, AdversarialBias::Logic),
            Self::verify(content, AdversarialBias::Completeness),
            Self::verify(content, AdversarialBias::Performance),
        ]
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

    // ── AdversarialVerifier tests ────────────────────────────────
    #[test]
    fn adversarial_security_detects_unsafe() {
        let code = "unsafe { std::ptr::read(ptr) }";
        let verdict = AdversarialVerifier::verify(code, AdversarialBias::Security);
        assert!(!verdict.passed, "unsafe block should be flagged");
        assert!(verdict
            .findings
            .iter()
            .any(|f| f.category == "memory_safety"));
    }

    #[test]
    fn adversarial_logic_high_boolean_density() {
        let code = (0..20)
            .map(|i| format!("let x{} = true;\nlet y{} = false;\n", i, i))
            .collect::<String>();
        let verdict = AdversarialVerifier::verify(&code, AdversarialBias::Logic);
        assert!(!verdict.passed, "high boolean density should be flagged");
    }

    #[test]
    fn adversarial_completeness_detects_todo() {
        let code = "fn temp() { todo!() }";
        let verdict = AdversarialVerifier::verify(code, AdversarialBias::Completeness);
        assert!(!verdict.passed, "TODO markers should be flagged");
    }

    #[test]
    fn adversarial_performance_detects_excessive_cloning() {
        let code = (0..10)
            .map(|i| format!("let v{} = data.clone();\n", i))
            .collect::<String>();
        let verdict = AdversarialVerifier::verify(&code, AdversarialBias::Performance);
        assert!(!verdict.passed, "excessive cloning should be flagged");
    }

    #[test]
    fn adversarial_all_biases_run_independently() {
        let code = "unsafe { eval(password) }; todo!();";
        let verdicts = AdversarialVerifier::verify_all(code);
        assert_eq!(verdicts.len(), 4, "all four biases should run");
        assert!(
            verdicts.iter().any(|v| !v.passed),
            "at least one bias should find issues"
        );
    }
}
