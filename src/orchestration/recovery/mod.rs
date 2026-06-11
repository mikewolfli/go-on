//! BLUE43 Step 16: Automatic recovery with minimal human intervention closed loop.
//!
//! Provides a recovery strategy tree that selects appropriate actions based on
//! failure classification (retry/reroute/replan/repair/escalate/degrade),
//! tracks success rates, and escalates to human intervention only after
//! all automatic recovery attempts are exhausted.

use crate::resilience::hyper_resilience::HyperResilienceEngine;
use fastrand;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

pub mod escalation;
pub mod strategies;

pub use strategies::{default_strategies, select_strategy};

// ---------------------------------------------------------------------------
// Exponential backoff with jitter
// ---------------------------------------------------------------------------

/// Compute exponential backoff with full jitter for retry delays.
///
/// Formula: `random_between(0, base_ms * 2^(attempt-1))`
/// This spreads the retry load across competing clients and prevents
/// thundering herd problems.
///
/// # Arguments
/// * `base_ms` - Base delay in milliseconds.
/// * `attempt` - Which attempt number (1-based).
pub fn exp_backoff_ms(base_ms: u64, attempt: u32) -> u64 {
    let max_delay = base_ms.saturating_mul(1u64 << (attempt.saturating_sub(1)).min(10));
    if max_delay == 0 {
        return 0;
    }
    fastrand::u64(0..max_delay)
}

// ---------------------------------------------------------------------------
// Failure classification
// ---------------------------------------------------------------------------

/// Explicit classification of failure types for strategy matching.
///
/// Replaces fragile Levenshtein/string-similarity heuristics with a
/// deterministic keyword-based classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Timeout,
    RateLimit,
    NetworkError,
    PermissionDenied,
    ToolNotFound,
    ToolExecutionError,
    InvalidInput,
    ResourceExhausted,
    Unknown,
}

/// Classify a failure description string into a `FailureKind` using keyword
/// matching. Matching is case-insensitive.
pub fn classify_failure(error: &str) -> FailureKind {
    let e = error.to_ascii_lowercase();

    if e.contains("timeout") || e.contains("timed out") || e.contains("deadline exceeded") {
        FailureKind::Timeout
    } else if e.contains("rate limit") || e.contains("rate_limit") || e.contains("throttl") {
        FailureKind::RateLimit
    } else if e.contains("network") || e.contains("connection refused") || e.contains("dns") {
        FailureKind::NetworkError
    } else if e.contains("permission")
        || e.contains("denied")
        || e.contains("forbidden")
        || e.contains("unauthorized")
        || e.contains("auth")
    {
        FailureKind::PermissionDenied
    } else if e.contains("not found") || e.contains("no such") || e.contains("unknown tool") {
        FailureKind::ToolNotFound
    } else if e.contains("execution error")
        || e.contains("runtime error")
        || e.contains("crash")
        || e.contains("empty response")
    {
        FailureKind::ToolExecutionError
    } else if e.contains("invalid") || e.contains("bad request") || e.contains("malformed") {
        FailureKind::InvalidInput
    } else if e.contains("resource")
        || e.contains("memory")
        || e.contains("disk")
        || e.contains("exhausted")
    {
        FailureKind::ResourceExhausted
    } else {
        FailureKind::Unknown
    }
}

// ---------------------------------------------------------------------------
// Tool / agent reference
// ---------------------------------------------------------------------------

/// A reference to a tool or agent used inside recovery actions.
///
/// Replaces magic string literals (`"auto"`, `"current"`, `"fallback"`)
/// with explicit enum variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolReference {
    /// The current tool or agent (auto-detect).
    Auto,
    /// The currently active agent.
    Current,
    /// A fallback agent.
    Fallback,
    /// An explicitly named tool or agent.
    Named(String),
}

impl fmt::Display for ToolReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolReference::Auto => write!(f, "auto"),
            ToolReference::Current => write!(f, "current"),
            ToolReference::Fallback => write!(f, "fallback"),
            ToolReference::Named(name) => write!(f, "{name}"),
        }
    }
}

impl Serialize for ToolReference {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ToolReference {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "auto" => ToolReference::Auto,
            "current" => ToolReference::Current,
            "fallback" => ToolReference::Fallback,
            _ => ToolReference::Named(s),
        })
    }
}

// ---------------------------------------------------------------------------
// Recovery action
// ---------------------------------------------------------------------------

/// Recovery action in the strategy tree for task failures.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecoveryAction {
    /// Retry the same tool with backoff (transient errors).
    Retry {
        tool_name: ToolReference,
        attempt: u32,
        max_attempts: u32,
        backoff_ms: u64,
    },
    /// Reroute to a different agent (permission/mismatch errors).
    Reroute {
        from_agent: ToolReference,
        to_agent: ToolReference,
        reason: String,
    },
    /// Replan the task with different strategy (plan/validation errors).
    Replan {
        reason: String,
        new_objective: String,
    },
    /// Apply a known repair strategy to the result.
    Repair {
        tool_name: ToolReference,
        repair_strategy: String,
    },
    /// Escalate to human intervention (unresolvable).
    Escalate { reason: String, context: Value },
    /// Fall back to a simpler or alternative approach.
    Degrade {
        fallback_tool: String,
        rationale: String,
    },
}

impl RecoveryAction {
    /// Returns a human-readable label for this action.
    pub fn label(&self) -> &str {
        match self {
            RecoveryAction::Retry { .. } => "retry",
            RecoveryAction::Reroute { .. } => "reroute",
            RecoveryAction::Replan { .. } => "replan",
            RecoveryAction::Repair { .. } => "repair",
            RecoveryAction::Escalate { .. } => "escalate",
            RecoveryAction::Degrade { .. } => "degrade",
        }
    }

    /// Returns the action as a JSON value for evidence logging.
    #[allow(dead_code)]
    pub fn to_json(&self) -> Value {
        match self {
            RecoveryAction::Retry {
                tool_name,
                attempt,
                max_attempts,
                backoff_ms,
            } => json!({
                "action": "retry",
                "tool_name": tool_name.to_string(),
                "attempt": attempt,
                "max_attempts": max_attempts,
                "backoff_ms": backoff_ms,
            }),
            RecoveryAction::Reroute {
                from_agent,
                to_agent,
                reason,
            } => json!({
                "action": "reroute",
                "from_agent": from_agent.to_string(),
                "to_agent": to_agent.to_string(),
                "reason": reason,
            }),
            RecoveryAction::Replan {
                reason,
                new_objective,
            } => json!({
                "action": "replan",
                "reason": reason,
                "new_objective": new_objective,
            }),
            RecoveryAction::Repair {
                tool_name,
                repair_strategy,
            } => json!({
                "action": "repair",
                "tool_name": tool_name.to_string(),
                "repair_strategy": repair_strategy,
            }),
            RecoveryAction::Escalate { reason, context } => json!({
                "action": "escalate",
                "reason": reason,
                "context": context,
            }),
            RecoveryAction::Degrade {
                fallback_tool,
                rationale,
            } => json!({
                "action": "degrade",
                "fallback_tool": fallback_tool,
                "rationale": rationale,
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Recovery strategy
// ---------------------------------------------------------------------------

/// A recovery strategy with its name, action chain, and tracked success rate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryStrategy {
    /// Human-readable name for the strategy (e.g. "timeout_retry", "permission_reroute").
    pub name: String,
    /// Ordered list of recovery actions to attempt in sequence.
    pub actions: Vec<RecoveryAction>,
    /// Number of successful recoveries using this strategy.
    pub success_count: u64,
    /// Total number of recovery attempts using this strategy.
    pub attempt_count: u64,
}

impl RecoveryStrategy {
    /// Create a new recovery strategy.
    pub fn new(name: &str, actions: Vec<RecoveryAction>) -> Self {
        Self {
            name: name.to_string(),
            actions,
            success_count: 0,
            attempt_count: 0,
        }
    }

    /// Record a successful recovery.
    pub fn record_success(&mut self) {
        self.success_count += 1;
        self.attempt_count += 1;
    }

    /// Record a failed recovery.
    pub fn record_failure(&mut self) {
        self.attempt_count += 1;
    }

    /// Returns the success rate of this strategy (0.0–1.0).
    #[allow(dead_code)]
    pub fn success_rate(&self) -> f64 {
        if self.attempt_count == 0 {
            0.0
        } else {
            self.success_count as f64 / self.attempt_count as f64
        }
    }
}

// ---------------------------------------------------------------------------
// Recovery attempt
// ---------------------------------------------------------------------------

/// A single recovery attempt with its outcome and evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryAttempt {
    /// Unique identifier for this attempt.
    pub attempt_id: String,
    /// Description of the failure that triggered recovery.
    pub failure: String,
    /// The recovery action that was taken.
    pub action_taken: RecoveryAction,
    /// Whether the recovery was successful.
    pub success: bool,
    /// Duration of the recovery attempt in milliseconds (populated by record_outcome).
    pub duration_ms: u64,
    /// Evidence payload capturing context, error details, and outcome data.
    pub evidence: Value,
    /// Monotonic timestamp (ms) when this attempt was created.
    pub started_at_ms: u64,
}

impl RecoveryAttempt {
    /// Create a new recovery attempt record.
    pub fn new(
        failure: &str,
        action_taken: RecoveryAction,
        duration_ms: u64,
        evidence: Value,
    ) -> Self {
        Self {
            attempt_id: Uuid::new_v4().to_string(),
            failure: failure.to_string(),
            action_taken,
            success: false,
            duration_ms,
            evidence,
            started_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }
}

// ---------------------------------------------------------------------------
// Recovery orchestrator
// ---------------------------------------------------------------------------

/// Orchestrator that manages automatic recovery attempts with configurable
/// thresholds for human intervention escalation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryOrchestrator {
    /// Available recovery strategies, ordered by preference.
    strategies: Vec<RecoveryStrategy>,
    /// History of all recovery attempts.
    recovery_attempts: Vec<RecoveryAttempt>,
    /// Maximum number of automatic (non-escalate) recovery attempts before forcing escalation.
    max_auto_recovery_attempts: u32,
    /// Number of consecutive failures that trigger human escalation.
    human_intervention_threshold: u32,
    /// Tracks consecutive auto-recovery failures for escalation detection.
    consecutive_auto_failures: u32,
    /// Total number of auto recovery attempts made.
    total_auto_attempts: u32,
    /// Total number of escalation events.
    total_escalations: u32,
    /// Hyper-resilience engine for circuit breaker checks and failure recording.
    /// Skipped in serialization since `Arc` is not `Serialize`.
    #[serde(skip)]
    engine: Option<Arc<HyperResilienceEngine>>,
}

#[allow(dead_code)]
impl RecoveryOrchestrator {
    /// Create a new recovery orchestrator with default thresholds.
    ///
    /// Defaults:
    /// - `max_auto_recovery_attempts`: 3
    /// - `human_intervention_threshold`: 3
    pub fn new() -> Self {
        Self::with_thresholds(3, 3)
    }

    /// Create a recovery orchestrator with custom thresholds.
    pub fn with_thresholds(
        max_auto_recovery_attempts: u32,
        human_intervention_threshold: u32,
    ) -> Self {
        Self {
            strategies: default_strategies(),
            recovery_attempts: Vec::new(),
            max_auto_recovery_attempts,
            human_intervention_threshold,
            consecutive_auto_failures: 0,
            total_auto_attempts: 0,
            total_escalations: 0,
            engine: None,
        }
    }

    /// Wire hyper-resilience into tool execution.
    /// Called from the scheduler/executor when a tool call fails.
    /// Stores the engine in the orchestrator and uses it in `attempt_recovery()`
    /// to report failures and check circuit breaker state before retrying.
    ///
    /// This is a public API surface for external wiring (e.g. from server
    /// startup code).  It is not called internally because the engine is
    /// injected via builder pattern.
    pub fn with_resilience_engine(mut self, engine: Arc<HyperResilienceEngine>) -> Self {
        self.engine = Some(engine);
        self
    }

    /// Attempt recovery by selecting the best strategy for the given failure type.
    ///
    /// Returns the chosen `RecoveryAction` on success, or an escalate action
    /// when all auto recovery options are exhausted.
    pub fn attempt_recovery(
        &mut self,
        failure_type: &str,
        context: Value,
    ) -> Result<RecoveryAction, String> {
        let failure_lower = failure_type.to_ascii_lowercase();

        // Check circuit breaker availability before attempting recovery.
        if let Some(ref engine) = self.engine {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let available =
                    handle.block_on(async { engine.is_available("tool_execution").await });
                if !available {
                    self.total_escalations += 1;
                    return Ok(RecoveryAction::Escalate {
                        reason: "circuit breaker open: tool_execution is unavailable until recovery timeout elapses".to_string(),
                        context,
                    });
                }
            }
        }

        // Check escalation thresholds first.
        if let Some(escalate) = escalation::should_escalate(
            self.consecutive_auto_failures,
            self.human_intervention_threshold,
            self.total_auto_attempts,
            self.max_auto_recovery_attempts,
            context.clone(),
        ) {
            self.total_escalations += 1;
            return Ok(escalate);
        }

        // Select the best strategy based on failure type classification.
        let strategy = select_strategy(&self.strategies, &failure_lower)?;
        let strategy_index = self.strategies.iter().position(|s| s.name == strategy.name);

        // Clone only the first action; subsequent actions are tried on re-entry.
        let action = strategy
            .actions
            .first()
            .cloned()
            .ok_or_else(|| format!("strategy '{}' has no actions", strategy.name))?;

        // Apply exponential backoff with jitter for retry actions.
        let action = match &action {
            RecoveryAction::Retry {
                tool_name,
                attempt,
                max_attempts,
                backoff_ms: base,
            } => {
                let actual_backoff = exp_backoff_ms(*base, *attempt);
                RecoveryAction::Retry {
                    tool_name: tool_name.clone(),
                    attempt: *attempt,
                    max_attempts: *max_attempts,
                    backoff_ms: actual_backoff,
                }
            }
            other => other.clone(),
        };

        // Record the failure with the resilience engine before retrying.
        if let Some(ref engine) = self.engine {
            if matches!(action, RecoveryAction::Retry { .. }) {
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    let _ =
                        handle.block_on(async { engine.record_failure("tool_execution").await });
                }
            }
        }

        // Record the auto-recovery attempt (pre-execution, marked as pending).
        // duration_ms is left as 0 — it will be populated by record_outcome()
        // which computes elapsed time from started_at_ms.
        let attempt = RecoveryAttempt::new(failure_type, action.clone(), 0, context.clone());

        self.recovery_attempts.push(attempt);

        // Track attempt count only after we pick an action (not escalate).
        self.total_auto_attempts += 1;
        if let Some(idx) = strategy_index {
            self.strategies[idx].attempt_count += 1;
        }

        Ok(action)
    }

    /// Record the outcome of a recovery attempt.
    ///
    /// Updates strategy success tracking and consecutive failure counters.
    pub fn record_outcome(&mut self, attempt_id: &str, success: bool) {
        // Update the attempt record and compute actual execution duration.
        if let Some(attempt) = self
            .recovery_attempts
            .iter_mut()
            .find(|a| a.attempt_id == attempt_id)
        {
            attempt.success = success;
            // Compute actual duration from recorded start time to now.
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            attempt.duration_ms = now_ms.saturating_sub(attempt.started_at_ms);
        }

        // Update strategy statistics.
        let action_label = self
            .recovery_attempts
            .iter()
            .find(|a| a.attempt_id == attempt_id)
            .map(|a| a.action_taken.label())
            .unwrap_or("unknown");

        for strategy in self.strategies.iter_mut() {
            let first_action_label = strategy
                .actions
                .first()
                .map(|a| a.label())
                .unwrap_or("unknown");
            if first_action_label == action_label {
                if success {
                    strategy.record_success();
                } else {
                    strategy.record_failure();
                }
            }
        }

        // Update consecutive failure tracking.
        if success {
            self.consecutive_auto_failures = 0;
        } else {
            self.consecutive_auto_failures = self.consecutive_auto_failures.saturating_add(1);
        }
    }

    /// Returns the auto-recovery success rate (0.0–1.0).
    ///
    /// This measures how often automatic recovery attempts succeed.
    /// A low rate suggests the system should escalate to human sooner.
    pub fn auto_recovery_rate(&self) -> f64 {
        let auto_attempts: Vec<&RecoveryAttempt> = self
            .recovery_attempts
            .iter()
            .filter(|a| !matches!(a.action_taken, RecoveryAction::Escalate { .. }))
            .collect();

        if auto_attempts.is_empty() {
            return 0.0;
        }

        let successes = auto_attempts.iter().filter(|a| a.success).count();
        successes as f64 / auto_attempts.len() as f64
    }

    /// Returns the human intervention ratio (0.0–1.0).
    ///
    /// The ratio of escalation actions to all recovery attempts.
    /// A value near 1.0 means almost all failures escalate to human.
    /// A value near 0.0 means auto-recovery handles most failures.
    pub fn human_intervention_ratio(&self) -> f64 {
        let total = self.recovery_attempts.len();
        if total == 0 {
            return 0.0;
        }
        self.total_escalations as f64 / total as f64
    }

    /// Returns the ID of the most recent recovery attempt, if any.
    pub fn last_attempt_id(&self) -> Option<String> {
        self.recovery_attempts.last().map(|a| a.attempt_id.clone())
    }

    /// Returns the full evidence chain as a vector of JSON values.
    ///
    /// Each entry corresponds to one recovery attempt containing the failure,
    /// action taken, success status, duration, and evidence context.
    pub fn recovery_evidence_chain(&self) -> Vec<Value> {
        self.recovery_attempts
            .iter()
            .map(|attempt| {
                json!({
                    "attempt_id": attempt.attempt_id,
                    "failure": attempt.failure,
                    "action_taken": attempt.action_taken.to_json(),
                    "success": attempt.success,
                    "duration_ms": attempt.duration_ms,
                    "evidence": attempt.evidence,
                })
            })
            .collect()
    }
}

impl Default for RecoveryOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Helper to create a basic orchestrator for testing.
    fn test_orchestrator() -> RecoveryOrchestrator {
        RecoveryOrchestrator::with_thresholds(5, 3)
    }

    #[test]
    fn recovery_strategy_tree_selects_retry_for_timeout() {
        let mut orch = test_orchestrator();
        let action = orch
            .attempt_recovery("timeout", json!({"tool": "test_tool"}))
            .expect("should select a recovery action");

        assert!(
            matches!(action, RecoveryAction::Retry { .. }),
            "timeout failures should map to retry, got {:?}",
            action
        );
        if let RecoveryAction::Retry { backoff_ms, .. } = action {
            assert!(
                backoff_ms <= 1000,
                "timeout retry backoff with jitter should be <= 1000ms, got {}",
                backoff_ms
            );
        }
    }

    #[test]
    fn recovery_strategy_tree_selects_retry_for_empty_response() {
        let mut orch = test_orchestrator();
        let action = orch
            .attempt_recovery("empty response from agent", json!({"tool": "chat"}))
            .expect("should select a recovery action");

        assert!(
            matches!(action, RecoveryAction::Retry { .. }),
            "empty response should map to retry, got {:?}",
            action
        );
    }

    #[test]
    fn recovery_orchestrator_tracks_success_rate() {
        let mut orch = test_orchestrator();

        // Make 4 attempts, 3 successful.
        for i in 0..4 {
            let _action = orch
                .attempt_recovery("timeout", json!({"attempt": i}))
                .expect("should produce action");
            let attempt_id = orch
                .recovery_attempts
                .last()
                .map(|a| a.attempt_id.clone())
                .unwrap();
            let success = i < 3; // First 3 succeed, last fails.
            orch.record_outcome(&attempt_id, success);
        }

        let rate = orch.auto_recovery_rate();
        assert!(
            (rate - 0.75).abs() < 0.01,
            "expected 0.75 success rate, got {rate}"
        );
    }

    #[test]
    fn escalation_threshold_triggers_after_max_auto_attempts() {
        let mut orch = RecoveryOrchestrator::with_thresholds(2, 5);

        // First two attempts succeed.
        for _ in 0..2 {
            let action = orch
                .attempt_recovery("timeout", json!({}))
                .expect("should produce action");
            assert!(
                !matches!(action, RecoveryAction::Escalate { .. }),
                "should not escalate before max auto attempts"
            );
            let attempt_id = orch.recovery_attempts.last().unwrap().attempt_id.clone();
            orch.record_outcome(&attempt_id, true);
        }

        // Third attempt should escalate because max_auto_recovery_attempts (2) is reached.
        let action = orch
            .attempt_recovery("timeout", json!({}))
            .expect("should produce action");
        assert!(
            matches!(action, RecoveryAction::Escalate { .. }),
            "should escalate when max auto attempts exhausted, got {:?}",
            action
        );
    }

    #[test]
    fn escalation_threshold_triggers_after_consecutive_failures() {
        let mut orch = RecoveryOrchestrator::with_thresholds(10, 3);

        // Three consecutive failures should trigger escalation from threshold.
        for i in 0..3 {
            let _action = orch
                .attempt_recovery("timeout", json!({"attempt": i}))
                .expect("should produce action");
            let attempt_id = orch.recovery_attempts.last().unwrap().attempt_id.clone();
            // Record as failure.
            orch.record_outcome(&attempt_id, false);
        }

        // Next attempt should escalate due to consecutive_failures >= threshold.
        let action = orch
            .attempt_recovery("timeout", json!({"attempt": 3}))
            .expect("should produce action");
        assert!(
            matches!(action, RecoveryAction::Escalate { .. }),
            "should escalate after consecutive failures exceed threshold, got {:?}",
            action
        );
    }

    #[test]
    fn evidence_chain_preserves_recovery_attempts() {
        let mut orch = test_orchestrator();

        orch.attempt_recovery("timeout", json!({"tool": "search"}))
            .expect("should produce action");
        let id1 = orch.recovery_attempts.last().unwrap().attempt_id.clone();
        orch.record_outcome(&id1, true);

        orch.attempt_recovery("permission denied", json!({"tool": "write"}))
            .expect("should produce action");
        let id2 = orch.recovery_attempts.last().unwrap().attempt_id.clone();
        orch.record_outcome(&id2, false);

        let chain = orch.recovery_evidence_chain();
        assert_eq!(
            chain.len(),
            2,
            "evidence chain should have 2 attempts, got {}",
            chain.len()
        );

        // First attempt: timeout → retry, success = true.
        assert_eq!(chain[0]["failure"], "timeout");
        assert_eq!(chain[0]["action_taken"]["action"], "retry");
        assert_eq!(chain[0]["success"], true);

        // Second attempt: permission denied → reroute.
        assert_eq!(chain[1]["failure"], "permission denied");
        assert_eq!(chain[1]["success"], false);
    }

    #[test]
    fn human_intervention_ratio_calculation() {
        let mut orch = test_orchestrator();

        // Make 2 regular attempts and succeed (no escalation).
        for i in 0..2 {
            let action = orch
                .attempt_recovery("timeout", json!({"attempt": i}))
                .expect("should produce action");
            let id = orch.recovery_attempts.last().unwrap().attempt_id.clone();
            orch.record_outcome(&id, true);
            assert!(
                !matches!(action, RecoveryAction::Escalate { .. }),
                "attempt {i} should not escalate yet"
            );
        }

        // Human intervention ratio should be 0 so far (no escalations).
        let ratio_before = orch.human_intervention_ratio();
        assert!(
            (ratio_before - 0.0).abs() < 0.001,
            "expected 0.0 ratio before escalations, got {ratio_before}"
        );

        // Force escalations by exhausting consecutive failures.
        // Reset orchestrator with low threshold and force escalations.
        let mut orch2 = RecoveryOrchestrator::with_thresholds(10, 2);

        for i in 0..2 {
            let _action = orch2
                .attempt_recovery("timeout", json!({"attempt": i}))
                .expect("should produce action");
            let id = orch2.recovery_attempts.last().unwrap().attempt_id.clone();
            orch2.record_outcome(&id, false);
        }

        // Third attempt: consecutive failures = 2 >= threshold (2) -> escalate.
        let action2 = orch2
            .attempt_recovery("timeout", json!({"attempt": 2}))
            .expect("should produce action");
        assert!(
            matches!(action2, RecoveryAction::Escalate { .. }),
            "should escalate at third attempt"
        );

        let ratio_after = orch2.human_intervention_ratio();
        assert!(
            ratio_after > 0.0,
            "expected ratio > 0 after escalation, got {ratio_after}"
        );
    }

    #[test]
    fn recovery_strategy_success_rate_tracking() {
        let mut strategy = RecoveryStrategy::new(
            "test_strategy",
            vec![RecoveryAction::Retry {
                tool_name: ToolReference::Named("test".to_string()),
                attempt: 1,
                max_attempts: 3,
                backoff_ms: 100,
            }],
        );

        assert!(
            (strategy.success_rate() - 0.0).abs() < 0.001,
            "new strategy should have 0 success rate"
        );

        strategy.record_success();
        strategy.record_success();
        strategy.record_failure();

        let rate = strategy.success_rate();
        assert!(
            (rate - 2.0 / 3.0).abs() < 0.01,
            "expected 0.666 success rate, got {rate}"
        );
    }

    #[test]
    fn recovery_action_label_and_json() {
        let action = RecoveryAction::Retry {
            tool_name: ToolReference::Named("search".to_string()),
            attempt: 2,
            max_attempts: 5,
            backoff_ms: 2000,
        };
        assert_eq!(action.label(), "retry");

        let json = action.to_json();
        assert_eq!(json["action"], "retry");
        assert_eq!(json["tool_name"], "search");
        assert_eq!(json["attempt"], 2);
        assert_eq!(json["backoff_ms"], 2000);
    }

    #[test]
    fn escalate_action_includes_full_context() {
        let context = json!({
            "task": "write_file",
            "error": "disk_full",
            "attempts": 5,
        });
        let action = RecoveryAction::Escalate {
            reason: "disk full after 5 retries".to_string(),
            context: context.clone(),
        };

        assert_eq!(action.label(), "escalate");
        let json = action.to_json();
        assert_eq!(json["action"], "escalate");
        assert_eq!(json["reason"], "disk full after 5 retries");
        assert_eq!(json["context"]["error"], "disk_full");
    }
}
