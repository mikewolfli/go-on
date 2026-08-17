//! Hyper-resilience engine tests, moved verbatim from the former single-file
//! `hyper_resilience.rs`. Declared via `mod tests;` in `mod.rs`; `super::*`
//! resolves against the module root re-exports.

use super::*;
use std::time::Duration;

/// 1. A fresh engine has no circuit breakers, no failover groups.
#[tokio::test]
async fn test_new_engine_empty() {
    let config = ResilienceConfig::default();
    let engine = HyperResilienceEngine::new(config);
    let p = engine.profile().await;
    assert_eq!(p.total_circuit_breakers, 0);
    assert_eq!(p.failover_groups, 0);
    assert_eq!(p.healing_actions_taken, 0);
}

/// 2. Register a circuit breaker succeeds and it appears in the profile.
#[tokio::test]
async fn test_register_circuit_breaker() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    engine
        .register_circuit_breaker("cb-gateway", 3, 10_000)
        .await
        .expect("register_circuit_breaker should succeed");
    let p = engine.profile().await;
    assert_eq!(p.total_circuit_breakers, 1);
}

/// 3. Recording failures beyond threshold trips the breaker open.
#[tokio::test]
async fn test_circuit_breaker_trips_open() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    engine
        .register_circuit_breaker("cb-db", 3, 10_000)
        .await
        .expect("register_circuit_breaker should succeed");

    // First two failures — still closed.
    assert_eq!(
        engine
            .record_failure("cb-db")
            .await
            .expect("record_failure should return a state"),
        CircuitState::Closed
    );
    assert_eq!(
        engine
            .record_failure("cb-db")
            .await
            .expect("record_failure should return a state"),
        CircuitState::Closed
    );
    // Third failure trips to open.
    assert_eq!(
        engine
            .record_failure("cb-db")
            .await
            .expect("record_failure should trip breaker to Open"),
        CircuitState::Open
    );
}

/// 4. After recovery timeout elapses, an open breaker transitions to half-open.
#[tokio::test]
async fn test_circuit_breaker_half_open() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    // Recovery timeout must be large enough that the "immediately after
    // trip" assertion cannot race the timeout under parallel-test load
    // (a 1 ms timeout let scheduler delay flip Open→HalfOpen early and
    // flaked the test).
    engine
        .register_circuit_breaker("cb-cache", 1, 1000)
        .await
        .expect("register_circuit_breaker should succeed");

    // Single failure trips to open.
    assert_eq!(
        engine
            .record_failure("cb-cache")
            .await
            .expect("record_failure should return a state"),
        CircuitState::Open
    );

    // Immediately — not available, still open.
    assert!(!engine.is_available("cb-cache").await);

    // Wait for the recovery timeout (1000 ms + some slack).
    tokio::time::sleep(Duration::from_millis(1100)).await;

    // Now probe should transition to half-open and return true.
    assert!(engine.probe("cb-cache").await);
}

/// 5. A success in half-open resets the breaker to closed.
#[tokio::test]
async fn test_circuit_breaker_resets_on_success() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    // 1000 ms recovery timeout: large enough that the "still open right
    // after the trip" assertion cannot race the timeout under parallel
    // test load (a 1 ms timeout flaked under scheduler delay).
    engine
        .register_circuit_breaker("cb-api", 1, 1000)
        .await
        .expect("register_circuit_breaker should succeed");

    // Trip to open.
    engine
        .record_failure("cb-api")
        .await
        .expect("record_failure should not fail");
    assert!(!engine.is_available("cb-api").await);

    // Wait for recovery timeout.
    tokio::time::sleep(Duration::from_millis(1100)).await;

    // Now probe transitions to half-open.
    assert!(engine.probe("cb-api").await);

    // Record a success — should close the breaker.
    engine
        .record_success("cb-api")
        .await
        .expect("record_success should not fail");
    let health = engine.system_health().await;
    assert_eq!(health.open_circuits, 0);
}

/// 6. An open circuit breaker reports unavailable.
#[tokio::test]
async fn test_is_available_open_returns_false() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    engine
        .register_circuit_breaker("cb-slow", 2, 60_000)
        .await
        .expect("register_circuit_breaker should succeed");

    engine
        .record_failure("cb-slow")
        .await
        .expect("record_failure should not fail");
    engine
        .record_failure("cb-slow")
        .await
        .expect("record_failure should not fail");

    // Should be open and unavailable.
    assert!(!engine.is_available("cb-slow").await);
}

/// 7. Register a failover group with primary and replicas.
#[tokio::test]
async fn test_register_failover_group() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    engine
        .register_failover_group(
            "group-alpha",
            "node-primary",
            vec!["node-replica-1".to_string(), "node-replica-2".to_string()],
        )
        .await
        .expect("register_failover_group should succeed");
    let p = engine.profile().await;
    assert_eq!(p.failover_groups, 1);
}

/// 8. Triggering a failover promotes a replica to leader.
#[tokio::test]
async fn test_trigger_failover() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    engine
        .register_failover_group(
            "group-beta",
            "node-p",
            vec!["node-r1".to_string(), "node-r2".to_string()],
        )
        .await
        .expect("register_failover_group should succeed");

    let new_leader = engine
        .trigger_failover("group-beta")
        .await
        .expect("trigger_failover should succeed");
    assert_eq!(new_leader, "node-r1");

    // A second failover should go to the next replica.
    let new_leader2 = engine
        .trigger_failover("group-beta")
        .await
        .expect("trigger_failover should succeed");
    assert_eq!(new_leader2, "node-r2");

    // Third failover wraps around.
    let new_leader3 = engine
        .trigger_failover("group-beta")
        .await
        .expect("trigger_failover should succeed");
    assert_eq!(new_leader3, "node-r1");
}

/// 9. System health reflects registered breakers and failure state.
#[tokio::test]
async fn test_system_health_reflects_state() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    engine
        .register_circuit_breaker("cb-1", 1, 60_000)
        .await
        .expect("register_circuit_breaker should succeed");
    engine
        .register_circuit_breaker("cb-2", 1, 60_000)
        .await
        .expect("register_circuit_breaker should succeed");

    let health = engine.system_health().await;
    assert_eq!(health.active_circuit_breakers, 2);
    assert_eq!(health.open_circuits, 0);
    assert_eq!(health.level, DegradationLevel::Normal);

    // Trip one breaker.
    engine
        .record_failure("cb-1")
        .await
        .expect("record_failure should not fail");
    let health2 = engine.system_health().await;
    assert_eq!(health2.open_circuits, 1);
    // One out of two open breakers triggers Constrained (more than 1/3 threshold)
    assert_eq!(health2.level, DegradationLevel::Constrained);
}

/// 10. Executing a self-healing action produces a valid report.
#[tokio::test]
async fn test_execute_healing() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    // Register the breaker under a reachable host:port so the health probe
    // passes (healing only clears breakers whose target passes the probe).
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let target = listener.local_addr().expect("local addr").to_string();
    engine
        .register_circuit_breaker(&target, 1, 10_000)
        .await
        .expect("register_circuit_breaker should succeed");
    engine
        .record_failure(&target)
        .await
        .expect("record_failure should not fail");

    let report = engine
        .execute_healing(SelfHealingAction::ClearCircuitBreaker, &target)
        .await
        .expect("execute_healing should succeed");
    assert!(report.success);
    assert_eq!(report.target, target);
    assert!(report.duration_ms > 0);

    // After healing, the breaker should be closed.
    let health = engine.system_health().await;
    assert_eq!(health.open_circuits, 0);
}

/// 10a. A breaker whose target does NOT pass the health probe is left
/// open: the auto-heal must not reset an unhealthy service (previously the
/// probe result was ignored, causing reset-open oscillation).
#[tokio::test]
async fn test_execute_healing_skips_unhealthy_target() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    // A breaker name (not a host:port) cannot be probed — healing skips it.
    engine
        .register_circuit_breaker("cb-broken", 1, 10_000)
        .await
        .expect("register_circuit_breaker should succeed");
    engine
        .record_failure("cb-broken")
        .await
        .expect("record_failure should not fail");

    let report = engine
        .execute_healing(SelfHealingAction::ClearCircuitBreaker, "cb-broken")
        .await
        .expect("execute_healing should succeed");
    assert!(!report.success, "unprobeable target must not be healed");

    // Breaker stays open.
    let health = engine.system_health().await;
    assert_eq!(health.open_circuits, 1);
}

/// 10b. Healing counters distinguish executed from simulated actions:
/// infrastructure-level actions (RestartNode) never bump the executed
/// counter, only the simulated one.
#[tokio::test]
async fn test_execute_healing_counts_executed_vs_simulated() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let target = listener.local_addr().expect("local addr").to_string();
    engine
        .register_circuit_breaker(&target, 1, 10_000)
        .await
        .expect("register_circuit_breaker should succeed");
    engine
        .register_failover_group("grp", "primary", vec!["replica-1".to_string()])
        .await
        .expect("register_failover_group should succeed");

    // Real effects: ClearCircuitBreaker (reachable target) and PromoteReplica
    // (group exists) both count as executed.
    engine
        .execute_healing(SelfHealingAction::ClearCircuitBreaker, &target)
        .await
        .expect("execute_healing should succeed");
    engine
        .execute_healing(SelfHealingAction::PromoteReplica, "grp")
        .await
        .expect("execute_healing should succeed");

    // Simulated effect: RestartNode is infrastructure-level and must not
    // count as executed.
    engine
        .execute_healing(SelfHealingAction::RestartNode, "some-node")
        .await
        .expect("execute_healing should succeed");

    let p = engine.profile().await;
    assert_eq!(
        p.healing_actions_taken, 2,
        "only real effects count as executed"
    );
    assert_eq!(
        p.healing_actions_simulated, 1,
        "RestartNode counts as simulated, not executed"
    );

    // A failed action (unknown breaker) counts nowhere.
    engine
        .execute_healing(SelfHealingAction::ClearCircuitBreaker, "no-such-breaker")
        .await
        .expect("execute_healing should succeed");
    let p = engine.profile().await;
    assert_eq!(p.healing_actions_taken, 2);
    assert_eq!(p.healing_actions_simulated, 1);
}

/// 11. Profile accurately reflects engine state after operations.
#[tokio::test]
async fn test_profile_reflects_state() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    engine
        .register_circuit_breaker("cb-1", 3, 10_000)
        .await
        .expect("register_circuit_breaker should succeed");
    engine
        .register_circuit_breaker("cb-2", 3, 10_000)
        .await
        .expect("register_circuit_breaker should succeed");
    engine
        .register_failover_group("group-gamma", "node-p", vec!["node-r1".to_string()])
        .await
        .expect("register_failover_group should succeed");

    // Trip one breaker.
    engine
        .record_failure("cb-1")
        .await
        .expect("record_failure should not fail");
    engine
        .record_failure("cb-1")
        .await
        .expect("record_failure should not fail");
    engine
        .record_failure("cb-1")
        .await
        .expect("record_failure should not fail");

    let p = engine.profile().await;
    assert_eq!(p.total_circuit_breakers, 2);
    assert_eq!(p.open_circuits, 1);
    assert_eq!(p.failover_groups, 1);
}

/// 12. Registering a circuit breaker with a duplicate name fails.
#[tokio::test]
async fn test_register_duplicate_circuit_breaker_fails() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    engine
        .register_circuit_breaker("cb-dup", 5, 10_000)
        .await
        .expect("register_circuit_breaker should succeed");
    let result = engine.register_circuit_breaker("cb-dup", 3, 20_000).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err
        .to_string()
        .contains("error.circuit_breaker_already_registered"));
}

// ── Unified health-monitoring (ported from the former failure_prevention) ──

/// Record five failures → the service breaker trips open and health goes
/// Unhealthy (failure_prevention parity: threshold 5, error rate blended).
#[test]
fn test_record_outcome_trips_breaker_and_marks_unhealthy() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    engine.register_service("service1");
    for _ in 0..5 {
        engine.record_outcome("service1", false, 900);
    }
    assert_eq!(engine.breaker_state("service1"), CircuitState::Open);
    let health = engine.service_health("service1").unwrap();
    assert_eq!(health.status, HealthStatus::Unhealthy);
    assert!(health.error_rate > 0.1);
}

/// A success while closed resets the failure streak (parity: breaker
/// failure count resets, so it cannot trip; health is rate-based and may
/// still show degradation until more successes accumulate).
#[test]
fn test_success_resets_failure_count() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    engine.register_service("api");
    for _ in 0..4 {
        engine.record_outcome("api", false, 900);
    }
    assert_eq!(engine.breaker_state("api"), CircuitState::Closed);
    engine.record_outcome("api", true, 100);
    // The breaker failure count reset — 5 consecutive failures would have
    // opened it, so this proves the streak was broken.
    assert_eq!(engine.breaker_state("api"), CircuitState::Closed);
    // Health is rate-based: 4/5 errors is still Unhealthy (parity with
    // the former failure_prevention).
    assert_eq!(
        engine.service_health("api").unwrap().status,
        HealthStatus::Unhealthy
    );
}

/// register_service → update_service_health semantics preserved via
/// record_outcome: a healthy run keeps status Healthy.
#[test]
fn test_health_monitoring_healthy_run() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    engine.register_service("api");
    engine.record_outcome("api", true, 100);
    let health = engine.service_health("api").unwrap();
    assert_eq!(health.status, HealthStatus::Healthy);
    assert!(health.success_rate > 0.9);
}

/// Degraded → Degraded level; Unhealthy with low success rate → Emergency.
#[test]
fn test_degradation_strategy() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    engine.register_service("api");
    // Simulate a degraded service: mix successes and failures so the
    // success rate drops below 0.8 but the error rate stays low.
    for i in 0..20 {
        engine.record_outcome("api", i % 5 != 0, 100);
    }
    let level = engine.degradation_level("api");
    assert!(
        matches!(
            level,
            DegradationLevel::Degraded | DegradationLevel::Constrained
        ),
        "expected degraded-level degradation, got {level:?}"
    );
    assert!(engine.should_degrade("api") || level == DegradationLevel::Degraded);
}

/// should_degrade is true once a service is Unhealthy (Constrained+).
#[test]
fn test_should_degrade() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    engine.register_service("api");
    for _ in 0..5 {
        engine.record_outcome("api", false, 900);
    }
    assert!(engine.should_degrade("api"));
}

/// recover_services resets an unhealthy service back to healthy baseline.
#[test]
fn test_recover_services_resets_unhealthy_service() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    engine.register_service("api");
    for _ in 0..5 {
        engine.record_outcome("api", false, 900);
    }
    assert!(engine.should_degrade("api"));

    let recovered = engine.recover_services(Some("api"));
    assert_eq!(recovered, vec!["api".to_string()]);
    assert_eq!(engine.breaker_state("api"), CircuitState::Closed);
    assert_eq!(
        engine.service_health("api").unwrap().status,
        HealthStatus::Healthy
    );
    assert!(!engine.should_degrade("api"));
}

/// breaker_snapshots report per-service totals (name, state, failures,
/// total, successes) for the health/observability consumers.
#[test]
fn test_breaker_snapshots_report_totals() {
    let engine = HyperResilienceEngine::new(ResilienceConfig::default());
    engine.register_service("api");
    engine.record_outcome("api", true, 100);
    // Trip the breaker open so a real Closed→Open transition is recorded.
    for _ in 0..10 {
        engine.record_outcome("api", false, 900);
    }
    assert_eq!(engine.breaker_state("api"), CircuitState::Open);
    let snapshots = engine.breaker_snapshots();
    let (name, state, _failures, total, successes, last_change) = snapshots
        .iter()
        .find(|(n, ..)| n == "api")
        .expect("api snapshot");
    assert_eq!(name, "api");
    assert_eq!(*total, 11);
    assert_eq!(*successes, 1);
    assert!(matches!(state, CircuitState::Open));
    // A real transition timestamp must have been recorded when the
    // breaker tripped open (the former snapshot always reported "now").
    assert!(*last_change > 0);
}
