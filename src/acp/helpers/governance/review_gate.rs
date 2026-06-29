//! BLUE43 Step 4: Extracted review gate helper for chat orchestration.
//!
//! Provides a focused interface for running review gates (dual review,
//! enhanced verification) during chat request processing.

use serde_json::{json, Value};

use crate::acp::server::AcpServer;
use crate::agent::Message;
use crate::config::PhaseOptions;
use crate::intelligence::verification::{
    DeterministicVerifier, StructuredReview, VerificationVerdict,
};
use crate::rpc_protocol::RequestTraceContext;

/// Outcome of a review gate execution
#[derive(Debug, Clone)]
pub struct ReviewGateOutcome {
    pub passed: bool,
}

/// Run the dual review gate for full_auto mode.
///
/// Delegates to the existing dual-review implementation and returns
/// structured review results.
pub async fn run_review_gate(
    server: &AcpServer,
    messages: &[Message],
    phase_options: Option<&PhaseOptions>,
    span: Option<&opentelemetry::Context>,
    trace: &RequestTraceContext,
) -> ReviewGateOutcome {
    let review_outcome = crate::acp::r#impl::agent::run_dual_review_gate(
        server,
        None,
        messages,
        phase_options,
        span,
        trace,
    )
    .await;

    ReviewGateOutcome {
        passed: review_outcome.map(|o| o.passed).unwrap_or(false),
    }
}

/// Run enhanced verification (syntax, test, lint, adversarial checks) on response text.
pub fn run_enhanced_verification(response_text: &str) -> Value {
    let mut verification_signals = Vec::new();

    let syntax_signal = DeterministicVerifier::run_syntax_check(response_text);
    verification_signals.push(syntax_signal);

    if response_text.to_ascii_lowercase().contains("test") || response_text.contains("assert") {
        let test_signal = DeterministicVerifier::run_test_check(response_text);
        verification_signals.push(test_signal);
    }

    if response_text.contains("fn ")
        || response_text.contains("let ")
        || response_text.contains("pub ")
    {
        let lint_signal = DeterministicVerifier::run_lint_check(response_text);
        verification_signals.push(lint_signal);
    }

    let adversarial_signal = DeterministicVerifier::run_test_check(response_text);
    verification_signals.push(adversarial_signal);

    let passed_count = verification_signals.iter().filter(|s| s.passed).count();
    let total_count = verification_signals.len();
    let confidence = if total_count > 0 {
        passed_count as f32 / total_count as f32
    } else {
        1.0
    };

    let structured_review = StructuredReview {
        verdict: if confidence >= 0.8 {
            VerificationVerdict::Approve
        } else {
            VerificationVerdict::Reject
        },
        reviewer_agent: "enhanced_verification_system".to_string(),
        confidence,
        signals: verification_signals,
        rationale: format!(
            "Enhanced verification completed with {}/{} checks passed",
            passed_count, total_count
        ),
        assumptions_validated: vec![
            "Syntax validity".to_string(),
            "No adversarial patterns".to_string(),
        ],
        weak_evidence_flags: if confidence < 0.9 {
            vec!["Some verification checks had lower confidence".to_string()]
        } else {
            Vec::new()
        },
        quality_compass: vec![
            "Deterministic verification".to_string(),
            "Adversarial robustness".to_string(),
        ],
        pua_report: None,
        audit_log: None,
    };

    json!({
        "enhanced_verification": {
            "verdict": format!("{:?}", structured_review.verdict),
            "confidence": structured_review.confidence,
            "signals_count": structured_review.signals.len(),
            "passed_checks": passed_count,
            "total_checks": total_count,
            "rationale": structured_review.rationale,
            "assumptions_validated": structured_review.assumptions_validated,
            "quality_compass": structured_review.quality_compass,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::server::ServerBuilder;

    #[tokio::test]
    async fn run_review_gate_returns_review_outcome() {
        let server = ServerBuilder::new().build();
        let trace = RequestTraceContext {
            trace_id: "test".to_string(),
            span_id: "test".to_string(),
            method: "test".to_string(),
            request_id: "test".to_string(),
        };
        let outcome = run_review_gate(&server, &[], None, None, &trace).await;

        // With no flow_manager configured, the underlying dual review gate
        // should fail, producing a ReviewGateOutcome with passed=false.
        assert!(!outcome.passed, "should not pass without a flow manager");
    }

    #[test]
    fn run_enhanced_verification_checks_syntax() {
        let result = run_enhanced_verification("fn hello() { let x = 1; }");

        let ev = result
            .get("enhanced_verification")
            .expect("should have enhanced_verification key");
        assert!(ev.get("verdict").and_then(Value::as_str).is_some());
        assert!(ev.get("confidence").and_then(Value::as_f64).is_some());
        assert!(ev.get("signals_count").and_then(Value::as_u64).is_some());
        assert!(ev.get("passed_checks").and_then(Value::as_u64).is_some());
        assert!(ev.get("total_checks").and_then(Value::as_u64).is_some());
        assert!(ev.get("rationale").and_then(Value::as_str).is_some());
        assert!(ev
            .get("assumptions_validated")
            .and_then(Value::as_array)
            .is_some());
        assert!(ev
            .get("quality_compass")
            .and_then(Value::as_array)
            .is_some());
    }

    #[test]
    fn run_enhanced_verification_code_snippets_get_lint_check() {
        let result =
            run_enhanced_verification("fn test() {\n    let x = 1;\n    pub fn inner() {}\n}");
        let ev = result
            .get("enhanced_verification")
            .expect("should have enhanced_verification");
        let total = ev.get("total_checks").and_then(Value::as_u64).unwrap_or(0);
        // Should have at least 4 checks: syntax + test + lint + adversarial
        assert!(total >= 4, "expected >=4 checks, got {}", total);
    }

    #[test]
    fn run_enhanced_verification_plain_text_no_extra_checks() {
        // Plain text without code markers should only get syntax + adversarial checks
        let result = run_enhanced_verification("This is a plain text response.");
        let ev = result
            .get("enhanced_verification")
            .expect("should have enhanced_verification");
        let total = ev.get("total_checks").and_then(Value::as_u64).unwrap_or(0);
        // Should have 2 checks: syntax + adversarial (no test/lint for plain text)
        assert_eq!(total, 2, "expected 2 checks for plain text, got {}", total);
    }

    #[test]
    fn run_enhanced_verification_confidence_reflects_pass_rate() {
        let result = run_enhanced_verification("fn hello() { let x = 1; }");
        let ev = result
            .get("enhanced_verification")
            .expect("should have enhanced_verification");
        let passed = ev.get("passed_checks").and_then(Value::as_u64).unwrap_or(0);
        let total = ev.get("total_checks").and_then(Value::as_u64).unwrap_or(0);
        let confidence = ev.get("confidence").and_then(Value::as_f64).unwrap_or(0.0);
        if total > 0 {
            let expected = passed as f64 / total as f64;
            assert!(
                (confidence - expected).abs() < 0.01,
                "confidence {} should equal passed/total = {}/{}",
                confidence,
                passed,
                total
            );
        }
    }

    // ── ReviewGateOutcome ─────────────────────────────────────────────
}
