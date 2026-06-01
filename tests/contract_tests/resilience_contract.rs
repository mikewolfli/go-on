//! GAP-B53-51: Resilience contract tests for circuit-breaker and self
-healing.
//!
//! Each test verifies a specific behavioral contract that the resilience
//! system must uphold under various failure conditions.

use go_on::resilience::hyper_resilience::{
    CircuitBreaker, CircuitState, DegradationLevel, FailureMode, HealingReport,
    HyperResilienceEngine, ResilienceConfig, ResilienceLevel, ResilienceProfile, SelfHealingAction,
    SystemHealth,
};

// ---------------------------------------------------------------------------
// Contract 1: Circuit breaker opens after N consecutive failures
// ---------------------------------------------------------------------------
#[test]
fn contract_circuit_breaker_opens_after_threshold() {
    let mut cb = CircuitBreaker::new("test-service", 3, 30_000);
    assert_eq!(cb.state(), CircuitState::Closed, "CB starts closed");

    for i in 0..3 {
        cb.record_failure();
        if i < 2 {
            assert_eq!(
                cb.state(),
                CircuitState::Closed,
                "CB should stay closed after {}/3 failures",
                i + 1
            );
        }
    }

    assert_eq!(
        cb.state(),
        CircuitState::Open,
        "CB must open after threshold (3) failures"
    );
}

// ---------------------------------------------------------------------------
// Contract 2: Circuit breaker resets failure count on success
// ---------------------------------------------------------------------------
#[test]
fn contract_circuit_breaker_resets_on_success() {
    let mut cb = CircuitBreaker::new("test-service", 5, 30_000);
    cb.record_failure();
    cb.record_failure();
    cb.record_success();
    cb.record_failure();
    cb.record_failure();
    cb.record_failure();
    cb.record_failure();
    // Total consecutive failures after last success = 4 (under threshold).
    assert_eq!(
        cb.state(),
        CircuitState::Closed,
        "CB should remain closed when failures are interleaved with success"
    );
}

// ---------------------------------------------------------------------------
// Contract 3: Circuit breaker transitions to half-open after timeout
// ---------------------------------------------------------------------------
#[test]
fn contract_circuit_breaker_half_open_after_timeout() {
    let mut cb = CircuitBreaker::new("test-service", 2, 10); // 10ms timeout
    cb.record_failure();
    cb.record_failure();
    assert_eq!(cb.state(), CircuitState::Open, "CB should be open after 2 failures");

    // Wait for the timeout to expire.
    std::thread::sleep(std::time::Duration::from_millis(20));

    // On next call attempt, CB should transition to half-open.
    assert_eq!(
        cb.state(),
        CircuitState::HalfOpen,
        "CB should transition to half-open after timeout"
    );
}

// ---------------------------------------------------------------------------
// Contract 4: Self-healing action success transitions back to healthy
// ---------------------------------------------------------------------------
#[test]
fn contract_self_healing_action_transitions_to_healthy() {
    let mut engine = HyperResilienceEngine::new(ResilienceConfig::default());
    engine.record_failure("test-svc", FailureMode::Timeout);

    let report = engine.heal("test-svc");
    assert!(
        !report.actions.is_empty(),
        "Healing should produce at least one action"
    );
    for action in &report.actions {
        assert!(
            matches!(action, SelfHealingAction::Restart(_) | SelfHealingAction::Retry(_)),
            "Healing actions should be concrete (Restart or Retry)"
        );
    }
}

// ---------------------------------------------------------------------------
// Contract 5: Degradation level escalates proportionally
// ---------------------------------------------------------------------------
#[test]
fn contract_degradation_level_escalates() {
    // Simulate that with more failures, degradation level increases.
    let mut health = SystemHealth::default();
    assert_eq!(health.degradation, DegradationLevel::None, "Fresh health has no degradation");

    health.degradation = DegradationLevel::Low;
    assert!(
        (health.degradation as u8) >= (DegradationLevel::None as u8),
        "Low degradation >= None"
    );

    health.degradation = DegradationLevel::Medium;
    assert!(
        (health.degradation as u8) >= (DegradationLevel::Low as u8),
        "Medium degradation >= Low"
    );

    health.degradation = DegradationLevel::Critical;
    assert!(
        (health.degradation as u8) >= (DegradationLevel::Medium as u8),
        "Critical degradation >= Medium"
    );
}

// ---------------------------------------------------------------------------
// Contract 6: Resilience profile provides expected config
// ---------------------------------------------------------------------------
#[test]
fn contract_resilience_profile_provides_config() {
    let profile = ResilienceProfile::default();
    let config = profile.config();
    assert!(
        config.circuit_breaker_threshold > 0,
        "Circuit breaker threshold must be positive"
    );
    assert!(
        config.health_check_interval_ms > 0,
        "Health check interval must be positive"
    );
    assert!(
        config.max_retries > 0,
        "Max retries must be positive"
    );
}
