//! Recovery strategies — named strategies with action chains and success tracking
//!
//! Provides `RecoveryStrategy` and `RecoveryAttempt` types, plus the default
//! set of built-in strategies used by the orchestrator.

use super::*;

/// Build the default set of recovery strategies.
pub fn default_strategies() -> Vec<RecoveryStrategy> {
    vec![
        RecoveryStrategy::new(
            "timeout_retry",
            vec![RecoveryAction::Retry {
                tool_name: ToolReference::Auto,
                attempt: 1,
                max_attempts: 3,
                backoff_ms: 1000, // base — exp_backoff_ms applies jitter at runtime
            }],
        ),
        RecoveryStrategy::new(
            "empty_response_retry",
            vec![
                RecoveryAction::Retry {
                    tool_name: ToolReference::Auto,
                    attempt: 1,
                    max_attempts: 2,
                    backoff_ms: 500, // base
                },
                RecoveryAction::Repair {
                    tool_name: ToolReference::Auto,
                    repair_strategy: "request_structured_intermediate_output".to_string(),
                },
            ],
        ),
        RecoveryStrategy::new(
            "permission_reroute",
            vec![RecoveryAction::Reroute {
                from_agent: ToolReference::Current,
                to_agent: ToolReference::Fallback,
                reason: "permission_denied".to_string(),
            }],
        ),
        RecoveryStrategy::new(
            "rate_limit_backoff",
            vec![
                RecoveryAction::Retry {
                    tool_name: ToolReference::Auto,
                    attempt: 1,
                    max_attempts: 3,
                    backoff_ms: 5000, // base — applied with exp backoff + jitter
                },
                RecoveryAction::Degrade {
                    fallback_tool: "lower_cost_mode".to_string(),
                    rationale: "rate_limit_avoidance".to_string(),
                },
            ],
        ),
        RecoveryStrategy::new(
            "generic_failure_replan",
            vec![RecoveryAction::Replan {
                reason: "generic_failure".to_string(),
                new_objective: "try_alternative_approach".to_string(),
            }],
        ),
    ]
}

/// Select the best matching strategy for a given failure type.
///
/// Uses the explicit `FailureKind` classification instead of fragile
/// string similarity scoring.
pub fn select_strategy<'a>(
    strategies: &'a [RecoveryStrategy],
    failure_lower: &str,
) -> Result<&'a RecoveryStrategy, String> {
    let kind = classify_failure(failure_lower);
    let name = match kind {
        FailureKind::Timeout => "timeout_retry",
        FailureKind::RateLimit => "rate_limit_backoff",
        FailureKind::PermissionDenied => "permission_reroute",
        // Tool execution errors (empty responses, crashes) → empty_response_retry
        FailureKind::ToolExecutionError => "empty_response_retry",
        // Everything else falls through to the generic replan strategy.
        FailureKind::NetworkError
        | FailureKind::ToolNotFound
        | FailureKind::InvalidInput
        | FailureKind::ResourceExhausted
        | FailureKind::Unknown => "generic_failure_replan",
    };
    strategies
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("no strategy matches failure: {failure_lower}"))
}
