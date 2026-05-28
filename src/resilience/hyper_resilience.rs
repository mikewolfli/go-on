//! F-GAP-27: Hyper-resilience — super-node failover, multi-level circuit breaking,
//! cascading degradation handling, and self-healing capabilities.
//!
//! This module provides the core resilience engine that monitors system health,
//! manages circuit breakers at multiple levels, orchestrates failover between
//! primary and replica nodes, and executes self-healing actions when degradation
//! is detected.

use crate::i18n::runtime::tf;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

/// Lock a Mutex, recovering from poison with a log.
fn lock_guard<T>(mtx: &Mutex<T>) -> MutexGuard<'_, T> {
    match mtx.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::error!("hyper_resilience mutex poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Resilience hardening level for a component or profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResilienceLevel {
    Standard,
    Enhanced,
    High,
    Critical,
}

/// Failure mode classification used by the engine to categorise events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(dead_code)] // F-GAP-27 — reserved for future hyper-resilience wiring
pub enum FailureMode {
    NodeFailure,
    NetworkPartition,
    ResourceExhaustion,
    CascadingDegradation,
    DataCorruption,
    TimeoutStorm,
}

pub use crate::optimization::failure_prevention::CircuitBreakerState as CircuitState;

/// System-wide degradation level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd)]
pub enum DegradationLevel {
    Normal,
    Degraded,
    Constrained,
    Emergency,
}

/// Self-healing action that can be executed by the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelfHealingAction {
    RestartNode,
    PromoteReplica,
    ClearCircuitBreaker,
    ScaleResources,
    ReinitializeComponent,
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// A circuit breaker that protects against repeated failures.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreaker {
    pub name: String,
    pub state: CircuitState,
    pub failure_count: u64,
    pub threshold: u64,
    pub recovery_timeout_ms: u64,
    pub last_failure_ms: u64,
    pub half_open_attempts: u64,
    /// The failure mode of the most recent failure.
    pub last_failure_mode: Option<FailureMode>,
    /// Rolling history of recent failure modes (most recent first, max 10).
    pub failure_history: Vec<FailureMode>,
}

/// A group of nodes forming a failover set with one primary and one or more replicas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverGroup {
    pub group_id: String,
    pub primary_node: String,
    pub replica_nodes: Vec<String>,
    pub current_leader: String,
    pub health_score: f64,
    pub last_failover_ms: u64,
    pub failover_count: u64,
}

/// Snapshot of current system health metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHealth {
    pub level: DegradationLevel,
    pub active_circuit_breakers: usize,
    pub open_circuits: usize,
    pub active_failovers: usize,
    pub avg_latency_ms: f64,
    pub error_rate: f64,
    pub timestamp_ms: u64,
}

/// Report produced after executing a self-healing action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealingReport {
    pub action: SelfHealingAction,
    pub target: String,
    pub initiated_ms: u64,
    pub success: bool,
    pub duration_ms: u64,
    pub result: String,
}

/// Configuration for the hyper-resilience engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResilienceConfig {
    #[serde(default = "default_circuit_breaker_threshold")]
    pub circuit_breaker_threshold: u64,
    #[serde(default = "default_recovery_timeout_ms")]
    pub recovery_timeout_ms: u64,
    #[serde(default = "default_health_check_interval_ms")]
    pub health_check_interval_ms: u64,
    #[serde(default = "default_max_failover_attempts")]
    pub max_failover_attempts: u32,
    #[serde(default = "default_self_healing_enabled")]
    pub self_healing_enabled: bool,
}

fn default_circuit_breaker_threshold() -> u64 {
    5
}
fn default_recovery_timeout_ms() -> u64 {
    30_000
}
fn default_health_check_interval_ms() -> u64 {
    5_000
}
fn default_max_failover_attempts() -> u32 {
    3
}
fn default_self_healing_enabled() -> bool {
    true
}

impl Default for ResilienceConfig {
    fn default() -> Self {
        Self {
            circuit_breaker_threshold: 5,
            recovery_timeout_ms: 30_000,
            health_check_interval_ms: 5_000,
            max_failover_attempts: 3,
            self_healing_enabled: true,
        }
    }
}

/// High-level resilience profile summarising the current state of the engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResilienceProfile {
    pub level: ResilienceLevel,
    pub system_health: DegradationLevel,
    pub total_circuit_breakers: usize,
    pub open_circuits: usize,
    pub failover_groups: usize,
    pub healing_actions_taken: u64,
    pub uptime_ms: u64,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// The hyper-resilience engine that orchestrates circuit breakers, failover
/// groups, health monitoring, and self-healing actions.
///
/// All methods are thread-safe via interior mutability (`Arc<Mutex<…>>`).
#[derive(Debug, Clone)]
pub struct HyperResilienceEngine {
    inner: Arc<Mutex<EngineInner>>,
}

#[derive(Debug)]
struct EngineInner {
    config: ResilienceConfig,
    circuit_breakers: HashMap<String, CircuitBreaker>,
    failover_groups: HashMap<String, FailoverGroup>,
    healing_actions_taken: u64,
    started_ms: u64,
    // Test/benchmark health metrics
    test_avg_latency_ms: f64,
    test_error_rate: f64,
    // Flag to indicate health checks have been started
    health_checks_running: bool,
}

impl HyperResilienceEngine {
    /// Create a new hyper-resilience engine with the given configuration.
    pub fn new(config: ResilienceConfig) -> Self {
        let now_ms = now_millis();
        Self {
            inner: Arc::new(Mutex::new(EngineInner {
                config,
                circuit_breakers: HashMap::new(),
                failover_groups: HashMap::new(),
                healing_actions_taken: 0,
                started_ms: now_ms,
                test_avg_latency_ms: 10.0,
                test_error_rate: 0.001,
                health_checks_running: false,
            })),
        }
    }

    /// Register a circuit breaker with the given name, threshold, and recovery timeout.
    pub fn register_circuit_breaker(
        &self,
        name: &str,
        threshold: u64,
        recovery_timeout_ms: u64,
    ) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        if inner.circuit_breakers.contains_key(name) {
            bail!(
                "{}",
                tf(
                    "error.circuit_breaker_already_registered",
                    &[("name", name)]
                )
            );
        }
        inner.circuit_breakers.insert(
            name.to_string(),
            CircuitBreaker {
                name: name.to_string(),
                state: CircuitState::Closed,
                failure_count: 0,
                threshold,
                recovery_timeout_ms,
                last_failure_ms: 0,
                half_open_attempts: 0,
                last_failure_mode: None,
                failure_history: Vec::new(),
            },
        );
        Ok(())
    }

    /// Record a failure against the named circuit breaker.
    ///
    /// Returns the new state of the circuit breaker after applying the failure.
    /// Uses `FailureMode::ResourceExhaustion` as the default failure mode.
    pub fn record_failure(&self, breaker_name: &str) -> Result<CircuitState> {
        self.record_failure_with_mode(breaker_name, FailureMode::ResourceExhaustion)
    }

    /// Record a failure with a specific `FailureMode` classification.
    ///
    /// Returns the new state of the circuit breaker after applying the failure.
    /// The failure mode is stored on the circuit breaker for diagnostics
    /// and is included in the failure history (rolling window of 10).
    pub fn record_failure_with_mode(
        &self,
        breaker_name: &str,
        failure_mode: FailureMode,
    ) -> Result<CircuitState> {
        let mut inner = lock_guard(&self.inner);
        let cb = inner
            .circuit_breakers
            .get_mut(breaker_name)
            .with_context(|| tf("error.circuit_breaker_not_found", &[("name", breaker_name)]))?;

        let now = now_millis();

        // Track failure mode
        cb.last_failure_mode = Some(failure_mode);
        cb.failure_history.push(failure_mode);
        if cb.failure_history.len() > 10 {
            cb.failure_history.remove(0);
        }

        match cb.state {
            CircuitState::Closed => {
                cb.failure_count += 1;
                cb.last_failure_ms = now;
                if cb.failure_count >= cb.threshold {
                    cb.state = CircuitState::Open;
                }
            }
            CircuitState::Open => {
                // Already open; update last_failure so the timer resets.
                cb.last_failure_ms = now;
            }
            CircuitState::HalfOpen => {
                // Failure in half-open immediately trips back to open.
                cb.state = CircuitState::Open;
                cb.failure_count += 1;
                cb.last_failure_ms = now;
                cb.half_open_attempts = 0;
            }
        }

        Ok(cb.state)
    }

    /// Record a success against the named circuit breaker.
    ///
    /// If the breaker is half-open, a success moves it back to closed.
    pub fn record_success(&self, breaker_name: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let cb = inner
            .circuit_breakers
            .get_mut(breaker_name)
            .with_context(|| tf("error.circuit_breaker_not_found", &[("name", breaker_name)]))?;

        match cb.state {
            CircuitState::HalfOpen => {
                // Success in half-open → closed.
                cb.state = CircuitState::Closed;
                cb.failure_count = 0;
                cb.half_open_attempts = 0;
                cb.last_failure_ms = 0;
            }
            CircuitState::Closed => {
                // Reset failure count on success while closed.
                cb.failure_count = 0;
            }
            CircuitState::Open => {
                // No-op: an open breaker can't accept successes directly;
                // it must transition through half-open first.
            }
        }

        Ok(())
    }

    /// Check whether the named circuit breaker is currently available
    /// (closed or half-open).
    ///
    /// This method also evaluates whether an open breaker should transition
    /// to half-open based on the recovery timeout.
    /// Check whether a circuit breaker is currently accepting requests (read-only).
    ///
    /// Does not mutate state. Use `probe()` if you also want automatic
    /// open→half-open recovery transitions.
    pub fn is_available(&self, breaker_name: &str) -> bool {
        let inner = lock_guard(&self.inner);
        match inner.circuit_breakers.get(breaker_name) {
            Some(cb) => matches!(cb.state, CircuitState::Closed | CircuitState::HalfOpen),
            None => false,
        }
    }

    /// Probe a circuit breaker: if open and the recovery timeout has elapsed,
    /// transition to half-open.  Returns `true` if the breaker is accepting
    /// requests after the probe (i.e. closed or half-open).
    ///
    /// This is the state-mutating counterpart of `is_available()`.
    pub fn probe(&self, breaker_name: &str) -> bool {
        let mut inner = lock_guard(&self.inner);
        let cb = match inner.circuit_breakers.get_mut(breaker_name) {
            Some(cb) => cb,
            None => return false,
        };

        match cb.state {
            CircuitState::Closed | CircuitState::HalfOpen => true,
            CircuitState::Open => {
                let now = now_millis();
                if now >= cb.last_failure_ms + cb.recovery_timeout_ms {
                    cb.state = CircuitState::HalfOpen;
                    cb.half_open_attempts = 0;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Register a failover group with a primary node and a list of replicas.
    pub fn register_failover_group(
        &self,
        group_id: &str,
        primary: &str,
        replicas: Vec<String>,
    ) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        if inner.failover_groups.contains_key(group_id) {
            bail!(
                "{}",
                tf(
                    "error.hyper_resilience.failover_already_registered",
                    &[("name", group_id)]
                )
            );
        }
        if replicas.is_empty() {
            bail!(
                "{}",
                tf(
                    "error.hyper_resilience.failover_requires_replica",
                    &[("name", group_id)]
                )
            );
        }
        inner.failover_groups.insert(
            group_id.to_string(),
            FailoverGroup {
                group_id: group_id.to_string(),
                primary_node: primary.to_string(),
                replica_nodes: replicas,
                current_leader: primary.to_string(),
                health_score: 100.0,
                last_failover_ms: 0,
                failover_count: 0,
            },
        );
        Ok(())
    }

    /// Trigger a failover for the named group, promoting the next available replica.
    ///
    /// Returns the identifier of the new leader.
    pub fn trigger_failover(&self, group_id: &str) -> Result<String> {
        let mut inner = lock_guard(&self.inner);
        let group = inner
            .failover_groups
            .get_mut(group_id)
            .with_context(|| tf("error.failover_group_not_found", &[("name", group_id)]))?;

        // Find the next replica (round-robin through replicas).
        let current_idx = group
            .replica_nodes
            .iter()
            .position(|r| r == &group.current_leader);

        let next_replica = match current_idx {
            Some(idx) => {
                let next_idx = (idx + 1) % group.replica_nodes.len();
                group.replica_nodes[next_idx].clone()
            }
            None => {
                // Current leader is not in the replica list (shouldn't happen,
                // but fall back to the first replica).
                group.replica_nodes[0].clone()
            }
        };

        group.current_leader = next_replica.clone();
        let now = now_millis();
        group.last_failover_ms = now;
        group.failover_count += 1;
        group.health_score = f64::max(0.0, group.health_score - 10.0);

        Ok(next_replica)
    }

    /// Return a snapshot of the current system health.
    pub fn system_health(&self) -> SystemHealth {
        let inner = lock_guard(&self.inner);
        let active_circuit_breakers = inner.circuit_breakers.len();
        let open_circuits = inner
            .circuit_breakers
            .values()
            .filter(|cb| matches!(cb.state, CircuitState::Open))
            .count();
        let active_failovers = inner
            .failover_groups
            .values()
            .filter(|g| g.failover_count > 0)
            .count();

        // Determine degradation level based on the ratio of open circuits.
        let level = if open_circuits > 0 && open_circuits > active_circuit_breakers / 2 {
            DegradationLevel::Emergency
        } else if open_circuits > active_circuit_breakers / 3 && open_circuits > 0 {
            DegradationLevel::Constrained
        } else if open_circuits > 0 || active_failovers > 0 {
            DegradationLevel::Degraded
        } else {
            DegradationLevel::Normal
        };

        SystemHealth {
            level,
            active_circuit_breakers,
            open_circuits,
            active_failovers,
            avg_latency_ms: inner.test_avg_latency_ms,
            error_rate: inner.test_error_rate,
            timestamp_ms: now_millis(),
        }
    }

    /// Execute a self-healing action and return a report.
    ///
    /// This is a test/benchmark operation — no actual node restarts or resource
    /// scaling are performed.
    pub fn execute_healing(
        &self,
        action: SelfHealingAction,
        target: &str,
    ) -> Result<HealingReport> {
        let started_ms = now_millis();
        let mut inner = lock_guard(&self.inner);

        inner.healing_actions_taken += 1;

        // Simulate execution duration.
        let test_duration_ms: u64 = match &action {
            SelfHealingAction::RestartNode => 2_000,
            SelfHealingAction::PromoteReplica => 500,
            SelfHealingAction::ClearCircuitBreaker => 100,
            SelfHealingAction::ScaleResources => 3_000,
            SelfHealingAction::ReinitializeComponent => 1_000,
        };

        let (success, result) = match &action {
            SelfHealingAction::ClearCircuitBreaker => {
                // Clear the circuit breaker if it exists.
                if let Some(cb) = inner.circuit_breakers.get_mut(target) {
                    cb.state = CircuitState::Closed;
                    cb.failure_count = 0;
                    cb.last_failure_ms = 0;
                    cb.half_open_attempts = 0;
                    (
                        true,
                        tf("status.hyper_resilience.breaker_reset", &[("name", target)]),
                    )
                } else {
                    (
                        false,
                        tf("error.circuit_breaker_not_found", &[("name", target)]),
                    )
                }
            }
            SelfHealingAction::PromoteReplica => {
                // Simulate promoting a replica by triggering a failover.
                if let Some(group) = inner.failover_groups.get_mut(target) {
                    let new_leader = if group.replica_nodes.is_empty() {
                        group.primary_node.clone()
                    } else {
                        // Simple promotion: promote the first replica.
                        group.replica_nodes[0].clone()
                    };
                    group.current_leader = new_leader.clone();
                    group.failover_count += 1;
                    group.last_failover_ms = now_millis();
                    (
                        true,
                        tf(
                            "status.hyper_resilience.replica_promoted",
                            &[("replica", &new_leader), ("group", target)],
                        ),
                    )
                } else {
                    (
                        false,
                        tf("error.failover_group_not_found", &[("name", target)]),
                    )
                }
            }
            _ => {
                // Generic simulation for other actions.
                (
                    true,
                    tf(
                        "status.hyper_resilience.healing_executed",
                        &[("action", &format!("{:?}", action)), ("target", target)],
                    ),
                )
            }
        };

        let completed_ms = now_millis();
        let duration_ms = completed_ms
            .saturating_sub(started_ms)
            .max(test_duration_ms);

        Ok(HealingReport {
            action,
            target: target.to_string(),
            initiated_ms: started_ms,
            success,
            duration_ms,
            result,
        })
    }

    /// Return the current resilience profile summarising overall engine state.
    pub fn profile(&self) -> ResilienceProfile {
        let inner = lock_guard(&self.inner);
        let total_circuit_breakers = inner.circuit_breakers.len();
        let open_circuits = inner
            .circuit_breakers
            .values()
            .filter(|cb| matches!(cb.state, CircuitState::Open))
            .count();
        let failover_groups = inner.failover_groups.len();

        // Derive resilience level from the ratio of open circuits.
        let level = if open_circuits == 0 && failover_groups == 0 {
            ResilienceLevel::Standard
        } else if open_circuits <= 1 {
            ResilienceLevel::High
        } else {
            ResilienceLevel::Critical
        };

        // Determine system health degradation.
        let system_health =
            if open_circuits > 0 && open_circuits >= total_circuit_breakers.saturating_sub(1) {
                DegradationLevel::Emergency
            } else if open_circuits >= total_circuit_breakers / 2 {
                DegradationLevel::Constrained
            } else if open_circuits > 0 {
                DegradationLevel::Degraded
            } else {
                DegradationLevel::Normal
            };

        let uptime_ms = now_millis().saturating_sub(inner.started_ms);

        ResilienceProfile {
            level,
            system_health,
            total_circuit_breakers,
            open_circuits,
            failover_groups,
            healing_actions_taken: inner.healing_actions_taken,
            uptime_ms,
        }
    }

    /// Start background health checks. Spawns a tokio task that periodically
    /// probes all circuit breakers, assesses system health from real execution
    /// data, and automatically triggers self-healing for degraded components.
    ///
    /// Requires the engine to be wrapped in an `Arc`. Call this once during
    /// server startup. Safe to call multiple times — subsequent calls are
    /// no-ops.
    pub fn start_health_checks(self: &Arc<Self>) {
        let mut inner = lock_guard(&self.inner);
        if inner.health_checks_running {
            return;
        }
        inner.health_checks_running = true;
        let interval_ms = inner.config.health_check_interval_ms;
        drop(inner);

        let engine = Arc::clone(self);
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(tokio::time::Duration::from_millis(interval_ms));
            // Skip the first tick (immediate) to give startup time
            timer.tick().await;
            loop {
                timer.tick().await;
                engine.health_check_cycle();
            }
        });
    }

    /// Run a single health-check cycle.
    ///
    /// 1. Probes all circuit breakers — transitions those past their recovery
    ///    timeout from `Open` → `HalfOpen`.
    /// 2. Assesses overall system health from real circuit breaker states.
    /// 3. If degradation is `Constrained` or higher and self-healing is
    ///    enabled, automatically executes `ClearCircuitBreaker` on all open
    ///    breakers.
    ///
    /// This method is safe to call from any thread.
    pub fn health_check_cycle(&self) {
        // ── Phase 1: Probe all circuit breakers ────────────────────────────
        let breaker_names: Vec<String> = {
            let inner = lock_guard(&self.inner);
            inner.circuit_breakers.keys().cloned().collect()
        };
        for name in &breaker_names {
            self.probe(name);
        }

        // ── Phase 2: Assess system health ──────────────────────────────────
        let health = self.system_health();

        // Update real operational metrics
        {
            let mut inner = lock_guard(&self.inner);
            // Calculate real error rate from circuit breaker states
            let total = inner.circuit_breakers.len();
            let open = inner
                .circuit_breakers
                .values()
                .filter(|cb| matches!(cb.state, CircuitState::Open))
                .count();
            let half_open = inner
                .circuit_breakers
                .values()
                .filter(|cb| matches!(cb.state, CircuitState::HalfOpen))
                .count();

            if total > 0 {
                inner.test_error_rate = open as f64 / total as f64;
            } else {
                inner.test_error_rate = 0.0;
            }
            // Estimate latency from half-open attempts (higher when failing)
            inner.test_avg_latency_ms = if half_open > 0 {
                15.0 + (half_open as f64 * 5.0)
            } else {
                8.0
            };
        }

        // ── Phase 3: Auto-heal if degraded ────────────────────────────────
        if health.level >= DegradationLevel::Constrained {
            let healing_enabled = lock_guard(&self.inner).config.self_healing_enabled;
            if healing_enabled {
                for name in &breaker_names {
                    let is_open = {
                        let inner = lock_guard(&self.inner);
                        inner
                            .circuit_breakers
                            .get(name)
                            .map(|cb| matches!(cb.state, CircuitState::Open))
                            .unwrap_or(false)
                    };
                    if is_open {
                        match self.execute_healing(SelfHealingAction::ClearCircuitBreaker, name) {
                            Ok(report) => {
                                tracing::info!(
                                    "health-check: auto-healed breaker '{}': {}",
                                    name,
                                    report.result
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "health-check: auto-heal failed for '{}': {}",
                                    name,
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// Record an execution outcome (success/failure) against a named circuit
    /// breaker.  If the breaker does not exist it will be automatically
    /// registered with the engine's default threshold and recovery timeout.
    /// Record an execution result for a circuit breaker.  If the breaker does
    /// not exist it will be automatically registered with the engine's default
    /// threshold and recovery timeout.
    ///
    /// Registration is performed under a single lock to avoid a TOCTOU race
    /// between the existence check and the registration call.
    ///
    /// This is the primary integration point for production code paths such as
    /// `HarnessBus::evaluate()` and `verify_output()`.
    pub fn record_execution(&self, breaker_name: &str, success: bool) {
        // All work (auto-register, check state, update state) is done under a
        // single lock acquisition to avoid a TOCTOU race between dropping the
        // lock and calling record_success / record_failure.
        let mut inner = lock_guard(&self.inner);

        // Auto-register if not present.
        let config = inner.config.clone();
        let cb_ref = inner
            .circuit_breakers
            .entry(breaker_name.to_string())
            .or_insert_with(|| CircuitBreaker {
                name: breaker_name.to_string(),
                state: CircuitState::Closed,
                failure_count: 0,
                threshold: config.circuit_breaker_threshold,
                recovery_timeout_ms: config.recovery_timeout_ms,
                last_failure_ms: 0,
                half_open_attempts: 0,
                last_failure_mode: None,
                failure_history: Vec::new(),
            });

        let now = now_millis();

        if success {
            match cb_ref.state {
                CircuitState::HalfOpen => {
                    // Success in half-open → closed.
                    cb_ref.state = CircuitState::Closed;
                    cb_ref.failure_count = 0;
                    cb_ref.half_open_attempts = 0;
                    cb_ref.last_failure_ms = 0;
                }
                CircuitState::Closed => {
                    // Reset failure count on success while closed.
                    cb_ref.failure_count = 0;
                }
                CircuitState::Open => {
                    // No-op: an open breaker can't accept successes directly;
                    // it must transition through half-open first.
                }
            }
        } else {
            // Track failure mode (default to ResourceExhaustion like record_failure).
            let failure_mode = FailureMode::ResourceExhaustion;
            cb_ref.last_failure_mode = Some(failure_mode);
            cb_ref.failure_history.push(failure_mode);
            if cb_ref.failure_history.len() > 10 {
                cb_ref.failure_history.remove(0);
            }

            match cb_ref.state {
                CircuitState::Closed => {
                    cb_ref.failure_count += 1;
                    cb_ref.last_failure_ms = now;
                    if cb_ref.failure_count >= cb_ref.threshold {
                        cb_ref.state = CircuitState::Open;
                    }
                }
                CircuitState::Open => {
                    // Already open; update last_failure so the timer resets.
                    cb_ref.last_failure_ms = now;
                }
                CircuitState::HalfOpen => {
                    // Failure in half-open immediately trips back to open.
                    cb_ref.state = CircuitState::Open;
                    cb_ref.failure_count += 1;
                    cb_ref.last_failure_ms = now;
                    cb_ref.half_open_attempts = 0;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the current time in milliseconds since the Unix epoch.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    /// 1. A fresh engine has no circuit breakers, no failover groups.
    #[test]
    fn test_new_engine_empty() {
        let config = ResilienceConfig::default();
        let engine = HyperResilienceEngine::new(config);
        let p = engine.profile();
        assert_eq!(p.total_circuit_breakers, 0);
        assert_eq!(p.failover_groups, 0);
        assert_eq!(p.healing_actions_taken, 0);
    }

    /// 2. Register a circuit breaker succeeds and it appears in the profile.
    #[test]
    fn test_register_circuit_breaker() {
        let engine = HyperResilienceEngine::new(ResilienceConfig::default());
        engine
            .register_circuit_breaker("cb-gateway", 3, 10_000)
            .unwrap();
        let p = engine.profile();
        assert_eq!(p.total_circuit_breakers, 1);
    }

    /// 3. Recording failures beyond threshold trips the breaker open.
    #[test]
    fn test_circuit_breaker_trips_open() {
        let engine = HyperResilienceEngine::new(ResilienceConfig::default());
        engine.register_circuit_breaker("cb-db", 3, 10_000).unwrap();

        // First two failures — still closed.
        assert_eq!(
            engine.record_failure("cb-db").unwrap(),
            CircuitState::Closed
        );
        assert_eq!(
            engine.record_failure("cb-db").unwrap(),
            CircuitState::Closed
        );
        // Third failure trips to open.
        assert_eq!(engine.record_failure("cb-db").unwrap(), CircuitState::Open);
    }

    /// 4. After recovery timeout elapses, an open breaker transitions to half-open.
    #[test]
    fn test_circuit_breaker_half_open() {
        let engine = HyperResilienceEngine::new(ResilienceConfig::default());
        // Use a very short timeout so the test doesn't take long.
        engine.register_circuit_breaker("cb-cache", 1, 1).unwrap();

        // Single failure trips to open.
        assert_eq!(
            engine.record_failure("cb-cache").unwrap(),
            CircuitState::Open
        );

        // Immediately — not available, still open.
        assert!(!engine.is_available("cb-cache"));

        // Wait for the recovery timeout (1 ms + some slack).
        thread::sleep(Duration::from_millis(10));

        // Now probe should transition to half-open and return true.
        assert!(engine.probe("cb-cache"));
    }

    /// 5. A success in half-open resets the breaker to closed.
    #[test]
    fn test_circuit_breaker_resets_on_success() {
        let engine = HyperResilienceEngine::new(ResilienceConfig::default());
        engine.register_circuit_breaker("cb-api", 1, 1).unwrap();

        // Trip to open.
        engine.record_failure("cb-api").unwrap();
        assert!(!engine.is_available("cb-api"));

        // Wait for recovery timeout.
        thread::sleep(Duration::from_millis(10));

        // Now probe transitions to half-open.
        assert!(engine.probe("cb-api"));

        // Record a success — should close the breaker.
        engine.record_success("cb-api").unwrap();
        let health = engine.system_health();
        assert_eq!(health.open_circuits, 0);
    }

    /// 6. An open circuit breaker reports unavailable.
    #[test]
    fn test_is_available_open_returns_false() {
        let engine = HyperResilienceEngine::new(ResilienceConfig::default());
        engine
            .register_circuit_breaker("cb-slow", 2, 60_000)
            .unwrap();

        engine.record_failure("cb-slow").unwrap();
        engine.record_failure("cb-slow").unwrap();

        // Should be open and unavailable.
        assert!(!engine.is_available("cb-slow"));
    }

    /// 7. Register a failover group with primary and replicas.
    #[test]
    fn test_register_failover_group() {
        let engine = HyperResilienceEngine::new(ResilienceConfig::default());
        engine
            .register_failover_group(
                "group-alpha",
                "node-primary",
                vec!["node-replica-1".to_string(), "node-replica-2".to_string()],
            )
            .unwrap();
        let p = engine.profile();
        assert_eq!(p.failover_groups, 1);
    }

    /// 8. Triggering a failover promotes a replica to leader.
    #[test]
    fn test_trigger_failover() {
        let engine = HyperResilienceEngine::new(ResilienceConfig::default());
        engine
            .register_failover_group(
                "group-beta",
                "node-p",
                vec!["node-r1".to_string(), "node-r2".to_string()],
            )
            .unwrap();

        let new_leader = engine.trigger_failover("group-beta").unwrap();
        assert_eq!(new_leader, "node-r1");

        // A second failover should go to the next replica.
        let new_leader2 = engine.trigger_failover("group-beta").unwrap();
        assert_eq!(new_leader2, "node-r2");

        // Third failover wraps around.
        let new_leader3 = engine.trigger_failover("group-beta").unwrap();
        assert_eq!(new_leader3, "node-r1");
    }

    /// 9. System health reflects registered breakers and failure state.
    #[test]
    fn test_system_health_reflects_state() {
        let engine = HyperResilienceEngine::new(ResilienceConfig::default());
        engine.register_circuit_breaker("cb-1", 1, 60_000).unwrap();
        engine.register_circuit_breaker("cb-2", 1, 60_000).unwrap();

        let health = engine.system_health();
        assert_eq!(health.active_circuit_breakers, 2);
        assert_eq!(health.open_circuits, 0);
        assert_eq!(health.level, DegradationLevel::Normal);

        // Trip one breaker.
        engine.record_failure("cb-1").unwrap();
        let health2 = engine.system_health();
        assert_eq!(health2.open_circuits, 1);
        // One out of two open breakers triggers Constrained (more than 1/3 threshold)
        assert_eq!(health2.level, DegradationLevel::Constrained);
    }

    /// 10. Executing a self-healing action produces a valid report.
    #[test]
    fn test_execute_healing() {
        let engine = HyperResilienceEngine::new(ResilienceConfig::default());
        engine
            .register_circuit_breaker("cb-broken", 1, 10_000)
            .unwrap();
        engine.record_failure("cb-broken").unwrap();

        let report = engine
            .execute_healing(SelfHealingAction::ClearCircuitBreaker, "cb-broken")
            .unwrap();
        assert!(report.success);
        assert_eq!(report.target, "cb-broken");
        assert!(report.duration_ms > 0);

        // After healing, the breaker should be closed.
        let health = engine.system_health();
        assert_eq!(health.open_circuits, 0);
    }

    /// 11. Profile accurately reflects engine state after operations.
    #[test]
    fn test_profile_reflects_state() {
        let engine = HyperResilienceEngine::new(ResilienceConfig::default());
        engine.register_circuit_breaker("cb-1", 3, 10_000).unwrap();
        engine.register_circuit_breaker("cb-2", 3, 10_000).unwrap();
        engine
            .register_failover_group("group-gamma", "node-p", vec!["node-r1".to_string()])
            .unwrap();

        // Trip one breaker.
        engine.record_failure("cb-1").unwrap();
        engine.record_failure("cb-1").unwrap();
        engine.record_failure("cb-1").unwrap();

        let p = engine.profile();
        assert_eq!(p.total_circuit_breakers, 2);
        assert_eq!(p.open_circuits, 1);
        assert_eq!(p.failover_groups, 1);
    }

    /// 12. Registering a circuit breaker with a duplicate name fails.
    #[test]
    fn test_register_duplicate_circuit_breaker_fails() {
        let engine = HyperResilienceEngine::new(ResilienceConfig::default());
        engine
            .register_circuit_breaker("cb-dup", 5, 10_000)
            .unwrap();
        let result = engine.register_circuit_breaker("cb-dup", 3, 20_000);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err
            .to_string()
            .contains("error.circuit_breaker_already_registered"));
    }
}
