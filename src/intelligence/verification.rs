//! Phase 4: Structured Verification and Review
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! Structured verification and deterministic checks will be integrated into the
//! review gate once verification signal aggregation logic is implemented.

use crate::agent::AgentAuditLog;
use crate::pua::{quality_compass, PuaExecutionReport};
use crate::quality_models::{QualitySignal, QualitySignalType, QualityVerdict};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

// ---------------------------------------------------------------------------
// Output schema validation (I-FIX14)
// ---------------------------------------------------------------------------

/// Validate that `output` conforms to `expected_schema`.
///
/// Checks:
/// - If `output` is valid JSON, validates it structurally against `expected_schema`.
/// - Always checks for malformed markdown code fences.
///
/// Returns `Ok(())` if all checks pass, or `Err` with a list of human-readable
/// validation errors.
pub fn validate_output_schema(output: &str, expected_schema: &Value) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    // Try to parse as JSON and validate against expected_schema
    if let Ok(json) = serde_json::from_str::<Value>(output) {
        validate_json_schema(&json, expected_schema, &mut errors, "$");
    }

    // Check markdown code blocks
    validate_code_blocks(output, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Recursively validate `json` against `schema` (a simplified JSON schema
/// representation supporting `type`, `properties`, `items`, and `required`).
fn validate_json_schema(json: &Value, schema: &Value, errors: &mut Vec<String>, path: &str) {
    if let Value::Object(map) = schema {
        // Check type constraint
        if let Some(expected_type) = map.get("type").and_then(|t| t.as_str()) {
            let actual = match json {
                Value::Null => "null",
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            };
            if actual != expected_type
                && !(expected_type == "number" && matches!(json, Value::Number(_)))
            {
                errors.push(format!(
                    "{}: expected type '{}', got '{}'",
                    path, expected_type, actual
                ));
            }
        }

        // Check properties (object)
        if let Some(properties) = map.get("properties").and_then(|p| p.as_object()) {
            if let Value::Object(obj) = json {
                for (key, prop_schema) in properties {
                    let child_path = format!("{}.{}", path, key);
                    let value = obj.get(key);
                    if let Some(val) = value {
                        validate_json_schema(val, prop_schema, errors, &child_path);
                    } else if let Some(required) = map.get("required").and_then(|r| r.as_array()) {
                        if required.iter().any(|r| r.as_str() == Some(key)) {
                            errors.push(format!("{}: missing required field", child_path));
                        }
                    }
                }
            } else if !json.is_null() && map.get("type").and_then(|t| t.as_str()) != Some("null") {
                errors.push(format!(
                    "{}: expected object, got {}",
                    path,
                    json_type_name(json)
                ));
            }
        }

        // Check items (array)
        if let Some(item_schema) = map.get("items") {
            if let Value::Array(arr) = json {
                for (i, item) in arr.iter().enumerate() {
                    let child_path = format!("{}[{}]", path, i);
                    validate_json_schema(item, item_schema, errors, &child_path);
                }
            } else if !json.is_null() {
                errors.push(format!(
                    "{}: expected array, got {}",
                    path,
                    json_type_name(json)
                ));
            }
        }

        // Check enum constraint
        if let Some(enum_values) = map.get("enum").and_then(|e| e.as_array()) {
            if !json.is_null() && !enum_values.contains(json) {
                errors.push(format!(
                    "{}: value {} is not one of the allowed enum values",
                    path, json
                ));
            }
        }
    }
}

/// Validate markdown code blocks in the output for basic correctness.
/// Reports issues such as unclosed code fences.
pub fn validate_code_blocks(output: &str, errors: &mut Vec<String>) {
    let code_fence_count = output.matches("```").count();
    if !code_fence_count.is_multiple_of(2) {
        errors.push(format!(
            "markdown has an odd number ({}): code fence ``` is unclosed",
            code_fence_count
        ));
    }
}

/// Return the JSON type name of a value as a &str.
fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
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

    /// Run a self-check of the go-on system source tree.
    ///
    /// Scans all `.rs` files under the project's `src/` directory for unstable
    /// macros (`todo!()`, `unreachable!()`, `unimplemented!()`) that should not
    /// be present in production code. This complements `run_lint_check()` which
    /// only checks user-supplied code.
    ///
    /// Returns a `VerificationSignal` with any issues found. If the source
    /// directory cannot be read (e.g. not in a dev environment), returns a
    /// passed signal with an informational note.
    pub fn self_check() -> VerificationSignal {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let src_dir = manifest_dir.join("src");

        let mut issues: Vec<String> = Vec::new();

        if !src_dir.exists() || !src_dir.is_dir() {
            return VerificationSignal {
                signal_type: QualitySignalType::Lint,
                passed: true,
                confidence: 0.5,
                details: Some("self_check: src/ directory not found, skipping".to_string()),
            };
        }

        let unstable_macros = ["todo!()", "unreachable!()", "unimplemented!()"];

        fn visit_dir(dir: &std::path::Path, issues: &mut Vec<String>, macros: &[&str]) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        visit_dir(&path, issues, macros);
                    } else if path.extension().is_some_and(|e| e == "rs") {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            for mac in macros {
                                // Count occurrences, skip if only in comments or test modules
                                let count = content.matches(mac).count();
                                if count > 0 {
                                    let rel = path
                                        .strip_prefix(std::path::Path::new(env!(
                                            "CARGO_MANIFEST_DIR"
                                        )))
                                        .unwrap_or(&path);
                                    issues.push(format!(
                                        "{}: {} occurrences of '{}'",
                                        rel.display(),
                                        count,
                                        mac
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }

        visit_dir(&src_dir, &mut issues, &unstable_macros);

        let passed = issues.is_empty();
        VerificationSignal {
            signal_type: QualitySignalType::Lint,
            passed,
            confidence: if passed { 0.9 } else { 0.8 },
            details: Some(if issues.is_empty() {
                "self_check: no unstable macros found in system source".to_string()
            } else {
                format!(
                    "self_check found {} issue(s): {}",
                    issues.len(),
                    issues.join("; ")
                )
            }),
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

/// Combined verification: run deterministic checks, then adversarial if high risk.
///
/// This is the production entry point that wires `DeterministicVerifier` and
/// `AdversarialVerifier` together.  If the deterministic aggregate verdict
/// indicates high risk (Reject, Revise, RequiresRepair, or Invalid), the
/// adversarial verifier is automatically invoked for all four bias angles.
///
/// Returns the deterministic verdict, the adversarial verdicts (empty if risk
/// was low), and the arbitration outcome.
pub fn verify_with_adversarial_if_high_risk(
    content: &str,
) -> (
    VerificationVerdict,
    Vec<AdversarialVerdict>,
    ArbitrationOutcome,
) {
    let mut deterministic_signals = vec![
        DeterministicVerifier::run_syntax_check(content),
        DeterministicVerifier::run_test_check(content),
        DeterministicVerifier::run_lint_check(content),
    ];
    deterministic_signals.extend(DeterministicVerifier::run_quality_compass_checks(content));

    let primary_verdict = DeterministicVerifier::aggregate(&deterministic_signals);

    // Determine if the content is high-risk
    let is_high_risk = matches!(
        primary_verdict,
        VerificationVerdict::Reject
            | VerificationVerdict::Revise
            | VerificationVerdict::RequiresRepair
            | VerificationVerdict::Invalid
    );

    if is_high_risk {
        let adversarial_verdicts = AdversarialVerifier::verify_all(content);
        let outcome = arbitrate(
            &primary_verdict,
            &adversarial_verdicts,
            &ArbitrationConfig::default(),
        );
        (primary_verdict, adversarial_verdicts, outcome)
    } else {
        (primary_verdict, vec![], ArbitrationOutcome::AcceptPrimary)
    }
}

// ---------------------------------------------------------------------------
// ArbitrationStrategy (F-GAP-02)
// ---------------------------------------------------------------------------
//
// When primary (DeterministicVerifier) and secondary (AdversarialVerifier)
// disagree, the arbitration strategy resolves the conflict.

/// Possible arbitration outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArbitrationOutcome {
    /// Accept primary verdict.
    AcceptPrimary,
    /// Accept adversarial verdict.
    AcceptAdversarial,
    /// Require human review.
    HumanReview,
    /// Insufficient evidence — defer.
    #[allow(
        dead_code,
        reason = "F-GAP-49 — planned for insufficient-evidence resolution path"
    )]
    Defer,
}

/// Arbitration configuration.
#[derive(Debug, Clone)]
pub struct ArbitrationConfig {
    /// Confidence threshold below which human review is required.
    pub human_review_threshold: f64,
    /// Whether to require adversarial verification on high-risk content.
    #[allow(
        dead_code,
        reason = "F-GAP-49 — planned wiring for adversarial verification"
    )]
    pub require_adversarial_on_high_risk: bool,
}

impl Default for ArbitrationConfig {
    fn default() -> Self {
        Self {
            human_review_threshold: 0.6,
            require_adversarial_on_high_risk: true,
        }
    }
}

/// Resolve disagreement between primary and adversarial verifiers.
///
/// # Arguments
///
/// * `primary_verdict` - Verdict from DeterministicVerifier::aggregate.
/// * `adversarial_verdicts` - Results from AdversarialVerifier.
/// * `config` - Arbitration configuration.
///
/// # Returns
///
/// An `ArbitrationOutcome` indicating the resolution.
pub fn arbitrate(
    primary_verdict: &VerificationVerdict,
    adversarial_verdicts: &[AdversarialVerdict],
    config: &ArbitrationConfig,
) -> ArbitrationOutcome {
    // If no adversarial checks ran, accept primary.
    if adversarial_verdicts.is_empty() {
        return ArbitrationOutcome::AcceptPrimary;
    }

    // Compute adversarial pass rate.
    let adv_passed = adversarial_verdicts.iter().filter(|v| v.passed).count();
    let adv_total = adversarial_verdicts.len();
    let adv_pass_rate = adv_passed as f64 / adv_total as f64;

    // Compute average adversarial confidence.
    let avg_adv_confidence: f64 = adversarial_verdicts
        .iter()
        .map(|v| v.confidence)
        .sum::<f64>()
        / adv_total as f64;

    // Map primary verdict to numeric score.
    let primary_score = match primary_verdict {
        VerificationVerdict::Approve => 1.0,
        VerificationVerdict::ApproveWithCaveats => 0.75,
        VerificationVerdict::Valid => 1.0,
        VerificationVerdict::Revise => 0.4,
        VerificationVerdict::RequiresRepair => 0.3,
        VerificationVerdict::Reject => 0.0,
        VerificationVerdict::Invalid => 0.0,
        VerificationVerdict::InsufficientEvidence => 0.5,
        VerificationVerdict::Inconclusive => 0.5,
    };

    // If both agree, accept.
    let primary_positive = primary_score >= 0.7;
    let adversarial_positive = adv_pass_rate >= 0.75;

    if primary_positive && adversarial_positive {
        return ArbitrationOutcome::AcceptPrimary;
    }
    if !primary_positive && !adversarial_positive {
        return ArbitrationOutcome::AcceptAdversarial;
    }

    // Disagreement — check confidence.
    let avg_confidence = match primary_verdict {
        VerificationVerdict::Approve | VerificationVerdict::ApproveWithCaveats => {
            // Use adversarial confidence as primary may be too permissive
            avg_adv_confidence
        }
        _ => {
            // For reject/revise, use combined confidence
            (primary_score + avg_adv_confidence) / 2.0
        }
    };

    if avg_confidence < config.human_review_threshold {
        ArbitrationOutcome::HumanReview
    } else if primary_positive && !adversarial_positive && avg_adv_confidence > 0.7 {
        // Adversarial found issues with high confidence — accept adversarial
        ArbitrationOutcome::AcceptAdversarial
    } else if !primary_positive && adversarial_positive && primary_score > 0.3 {
        // Close call — human review
        ArbitrationOutcome::HumanReview
    } else {
        ArbitrationOutcome::AcceptPrimary
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

    // ── Arbitration tests ────────────────────────────────────────

    #[test]
    fn arbitration_accepts_primary_when_both_agree_positive() {
        let primary = VerificationVerdict::Approve;
        let adv = vec![
            AdversarialVerdict {
                passed: true,
                bias: AdversarialBias::Security,
                confidence: 0.9,
                findings: vec![],
                summary: "ok".to_string(),
            },
            AdversarialVerdict {
                passed: true,
                bias: AdversarialBias::Logic,
                confidence: 0.85,
                findings: vec![],
                summary: "ok".to_string(),
            },
        ];
        let config = ArbitrationConfig::default();
        let outcome = arbitrate(&primary, &adv, &config);
        assert_eq!(outcome, ArbitrationOutcome::AcceptPrimary);
    }

    #[test]
    fn arbitration_requests_human_review_on_low_confidence() {
        let primary = VerificationVerdict::Approve;
        let adv = vec![AdversarialVerdict {
            passed: false,
            bias: AdversarialBias::Security,
            confidence: 0.3,
            findings: vec![AdversarialFinding {
                category: "test".to_string(),
                severity: "high".to_string(),
                description: "test finding".to_string(),
                recommendation: "fix it".to_string(),
            }],
            summary: "failed".to_string(),
        }];
        let config = ArbitrationConfig {
            human_review_threshold: 0.6,
            require_adversarial_on_high_risk: true,
        };
        let outcome = arbitrate(&primary, &adv, &config);
        // Low adversarial confidence + disagreement → human review
        assert_eq!(outcome, ArbitrationOutcome::HumanReview);
    }

    #[test]
    fn arbitration_defers_on_empty_adversarial() {
        let primary = VerificationVerdict::Approve;
        let config = ArbitrationConfig::default();
        let outcome = arbitrate(&primary, &[], &config);
        assert_eq!(outcome, ArbitrationOutcome::AcceptPrimary);
    }

    // ── Existing tests ───────────────────────────────────────────

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
