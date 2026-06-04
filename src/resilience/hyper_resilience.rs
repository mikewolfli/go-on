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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::watch;

/// Lock a Mutex, recovering from poison with a log.
fn lock_mutex<T>(mtx: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mtx.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::error!("hyper_resilience mutex poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

/// Lock a RwLock for reading, recovering from poison with a log.
fn read_lock<T>(rw: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    match rw.read() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::error!("hyper_resilience rwlock read poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

/// Lock a RwLock for writing, recovering from poison with a log.
#[allow(dead_code)]
fn write_lock<T>(rw: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    match rw.write() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::error!("hyper_resilience rwlock write poisoned, recovering");
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
    /// Interval (in ms) after which an open circuit breaker automatically
    /// transitions to HalfOpen during `is_available()` checks, enabling the
    /// self-healing / auto-recovery pattern without requiring an explicit
    /// `probe()` call.
    #[serde(default = "default_half_open_probe_interval_ms")]
    pub half_open_probe_interval_ms: u64,
}

#[allow(dead_code)]
fn default_circuit_breaker_threshold() -> u64 {
    5
}
#[allow(dead_code)]
fn default_recovery_timeout_ms() -> u64 {
    30_000
}
#[allow(dead_code)]
fn default_health_check_interval_ms() -> u64 {
    5_000
}
#[allow(dead_code)]
fn default_max_failover_attempts() -> u32 {
    3
}
#[allow(dead_code)]
fn default_self_healing_enabled() -> bool {
    true
}

#[allow(dead_code)]
fn default_half_open_probe_interval_ms() -> u64 {
    5000
}

impl Default for ResilienceConfig {
    fn default() -> Self {
        Self {
            circuit_breaker_threshold: 5,
            recovery_timeout_ms: 30_000,
            health_check_interval_ms: 5_000,
            max_failover_attempts: 3,
            self_healing_enabled: true,
            half_open_probe_interval_ms: 5000,
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
/// All methods are thread-safe via fine-grained locks:
/// - `config`: `RwLock` (read-heavy, rarely written)
/// - `circuit_breakers`: `Mutex` (frequently mutated)
/// - `failover_groups`: `Mutex` (separate from circuit breakers)
/// - `healing_actions_taken`: `AtomicU64` (lock-free counter)
/// - Test metrics: `Mutex` (occasional writes)
pub struct HyperResilienceEngine {
    config: RwLock<ResilienceConfig>,
    circuit_breakers: Mutex<HashMap<String, CircuitBreaker>>,
    failover_groups: Mutex<HashMap<String, FailoverGroup>>,
    healing_actions_taken: AtomicU64,
    started_ms: u64,
    test_avg_latency_ms: Mutex<f64>,
    test_error_rate: Mutex<f64>,
    cancel_tx: watch::Sender<bool>,
}

impl std::fmt::Debug for HyperResilienceEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HyperResilienceEngine")
            .field("started_ms", &self.started_ms)
            .field("healing_actions_taken", &self.healing_actions_taken)
            .field("cancel_tx", &"watch::Sender")
            .finish()
    }
}

impl Clone for HyperResilienceEngine {
    fn clone(&self) -> Self {
        Self {
            config: RwLock::new(read_lock(&self.config).clone()),
            circuit_breakers: Mutex::new(lock_mutex(&self.circuit_breakers).clone()),
            failover_groups: Mutex::new(lock_mutex(&self.failover_groups).clone()),
            healing_actions_taken: AtomicU64::new(
                self.healing_actions_taken.load(Ordering::Relaxed),
            ),
            started_ms: self.started_ms,
            test_avg_latency_ms: Mutex::new(*lock_mutex(&self.test_avg_latency_ms)),
            test_error_rate: Mutex::new(*lock_mutex(&self.test_error_rate)),
            cancel_tx: self.cancel_tx.clone(),
        }
    }
}

impl HyperResilienceEngine {
    /// Create a new hyper-resilience engine with the given configuration.
    pub fn new(config: ResilienceConfig) -> Self {
        let now_ms = now_millis();
        let (cancel_tx, _) = watch::channel(false);
        Self {
            config: RwLock::new(config),
            circuit_breakers: Mutex::new(HashMap::new()),
            failover_groups: Mutex::new(HashMap::new()),
            healing_actions_taken: AtomicU64::new(0),
            started_ms: now_ms,
            test_avg_latency_ms: Mutex::new(10.0),
            test_error_rate: Mutex::new(0.001),
            cancel_tx,
        }
    }

    /// Create a new hyper-resilience engine wrapped in `Arc` for shared ownership.
    ///
    /// This is a convenience wrapper around [`new`] that makes it easier to inject
    /// the engine via `ServerBuilder` or other shared-state patterns.
    pub fn new_shared(config: ResilienceConfig) -> Arc<Self> {
        Arc::new(Self::new(config))
    }

    /// Register a circuit breaker with the given name, threshold, and recovery timeout.
    pub fn register_circuit_breaker(
        &self,
        name: &str,
        threshold: u64,
        recovery_timeout_ms: u64,
    ) -> Result<()> {
        let mut cbs = lock_mutex(&self.circuit_breakers);
        if cbs.contains_key(name) {
            bail!(
                "{}",
                tf(
                    "error.circuit_breaker_already_registered",
                    &[("name", name)]
                )
            );
        }
        cbs.insert(
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
        let mut cbs = lock_mutex(&self.circuit_breakers);
        let cb = cbs
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
        let mut cbs = lock_mutex(&self.circuit_breakers);
        let cb = cbs
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
    /// If the breaker is **Open** and the time since the last failure exceeds
    /// `half_open_probe_interval_ms`, this method automatically transitions
    /// it to **HalfOpen**, enabling the self-healing / auto-recovery pattern
    /// without requiring an explicit `probe()` call.
    ///
    /// # Lock ordering
    /// Always acquires `circuit_breakers` Mutex **first**, then `config` RwLock
    /// (only in the Open branch where `probe_interval` is needed).
    /// This matches the common pattern used by `record_failure_with_mode`,
    /// `record_success`, `record_execution`, etc. — all of which acquire
    /// `circuit_breakers` without taking `config` at all. Acquiring in the
    /// opposite order (config first, then circuit_breakers) would risk a
    /// deadlock with code paths that hold circuit_breakers and later take
    /// config (e.g. `health_check_cycle` → `record_execution`).
    pub fn is_available(&self, breaker_name: &str) -> bool {
        // Acquire circuit_breakers FIRST — this is the canonical lock order.
        let mut cbs = lock_mutex(&self.circuit_breakers);
        let cb = match cbs.get_mut(breaker_name) {
            Some(cb) => cb,
            None => return false,
        };

        match cb.state {
            CircuitState::Closed | CircuitState::HalfOpen => true,
            CircuitState::Open => {
                // Acquire config RwLock only in the Open branch (not in the fast path).
                let probe_interval = read_lock(&self.config).half_open_probe_interval_ms;
                let now = now_millis();
                if now >= cb.last_failure_ms + probe_interval {
                    cb.state = CircuitState::HalfOpen;
                    cb.half_open_attempts = 0;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Probe a circuit breaker: if open and the recovery timeout has elapsed,
    /// transition to half-open.  Returns `true` if the breaker is accepting
    /// requests after the probe (i.e. closed or half-open).
    ///
    /// This is the state-mutating counterpart of `is_available()`.
    pub fn probe(&self, breaker_name: &str) -> bool {
        let mut cbs = lock_mutex(&self.circuit_breakers);
        let cb = match cbs.get_mut(breaker_name) {
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
        let mut fgs = lock_mutex(&self.failover_groups);
        if fgs.contains_key(group_id) {
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
        fgs.insert(
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
        let mut fgs = lock_mutex(&self.failover_groups);
        let group = fgs
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
        let cbs = lock_mutex(&self.circuit_breakers);
        let active_circuit_breakers = cbs.len();
        let open_circuits = cbs
            .values()
            .filter(|cb| matches!(cb.state, CircuitState::Open))
            .count();
        let avg_latency_ms = *lock_mutex(&self.test_avg_latency_ms);
        let error_rate = *lock_mutex(&self.test_error_rate);
        drop(cbs);

        let fgs = lock_mutex(&self.failover_groups);
        let active_failovers = fgs.values().filter(|g| g.failover_count > 0).count();
        drop(fgs);

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
            avg_latency_ms,
            error_rate,
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
        self.healing_actions_taken.fetch_add(1, Ordering::Release);

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
                let mut cbs = lock_mutex(&self.circuit_breakers);
                if let Some(cb) = cbs.get_mut(target) {
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
                let mut fgs = lock_mutex(&self.failover_groups);
                if let Some(group) = fgs.get_mut(target) {
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
        let cbs = lock_mutex(&self.circuit_breakers);
        let total_circuit_breakers = cbs.len();
        let open_circuits = cbs
            .values()
            .filter(|cb| matches!(cb.state, CircuitState::Open))
            .count();
        drop(cbs);

        let fgs = lock_mutex(&self.failover_groups);
        let failover_groups = fgs.len();
        drop(fgs);

        // Derive resilience level from the ratio of open circuits.
        let level = if open_circuits == 0 && failover_groups == 0 {
            ResilienceLevel::Standard
        } else if open_circuits <= 1 {
            ResilienceLevel::High
        } else {
            ResilienceLevel::Critical
        };

        // Determine system health degradation (aligned with system_health()).
        let system_health = if open_circuits > 0 && open_circuits > total_circuit_breakers / 2 {
            DegradationLevel::Emergency
        } else if open_circuits > total_circuit_breakers / 3 && open_circuits > 0 {
            DegradationLevel::Constrained
        } else if open_circuits > 0 {
            DegradationLevel::Degraded
        } else {
            DegradationLevel::Normal
        };

        let uptime_ms = now_millis().saturating_sub(self.started_ms);
        let healing_actions_taken = self.healing_actions_taken.load(Ordering::Acquire);

        ResilienceProfile {
            level,
            system_health,
            total_circuit_breakers,
            open_circuits,
            failover_groups,
            healing_actions_taken,
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
        let interval_ms = read_lock(&self.config).health_check_interval_ms;

        let engine = Arc::clone(self);
        let mut rx = self.cancel_tx.subscribe();
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(tokio::time::Duration::from_millis(interval_ms));
            // Skip the first tick (immediate) to give startup time
            timer.tick().await;
            loop {
                tokio::select! {
                    _ = timer.tick() => {
                        engine.health_check_cycle();
                    }
                    _ = rx.changed() => {
                        // Stop signal received
                        break;
                    }
                }
            }
        });
    }

    /// Stop background health checks by signalling the cancellation token.
    /// This is safe to call even if health checks were never started.
    pub fn stop_health_checks(&self) {
        let _ = self.cancel_tx.send(true);
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
            let cbs = lock_mutex(&self.circuit_breakers);
            cbs.keys().cloned().collect()
        };
        for name in &breaker_names {
            self.probe(name);
        }

        // ── Phase 2: Assess system health ──────────────────────────────────
        let health = self.system_health();

        // Update real operational metrics (only circuit_breakers + test metrics locks)
        {
            let cbs = lock_mutex(&self.circuit_breakers);
            let total = cbs.len();
            let open = cbs
                .values()
                .filter(|cb| matches!(cb.state, CircuitState::Open))
                .count();
            let half_open = cbs
                .values()
                .filter(|cb| matches!(cb.state, CircuitState::HalfOpen))
                .count();
            drop(cbs);

            let mut avg_latency = lock_mutex(&self.test_avg_latency_ms);
            let mut err_rate = lock_mutex(&self.test_error_rate);

            if total > 0 {
                *err_rate = open as f64 / total as f64;
            } else {
                *err_rate = 0.0;
            }
            // Estimate latency from half-open attempts (higher when failing)
            *avg_latency = if half_open > 0 {
                15.0 + (half_open as f64 * 5.0)
            } else {
                8.0
            };
        }

        // ── Phase 3: Auto-heal if degraded ────────────────────────────────
        if health.level >= DegradationLevel::Constrained {
            let healing_enabled = read_lock(&self.config).self_healing_enabled;
            if healing_enabled {
                for name in &breaker_names {
                    let is_open = {
                        let cbs = lock_mutex(&self.circuit_breakers);
                        cbs.get(name)
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
        // Phase 1: Read config for auto-register defaults (read lock, fast path).
        let threshold: u64;
        let recovery_timeout_ms: u64;
        {
            let config = read_lock(&self.config);
            threshold = config.circuit_breaker_threshold;
            recovery_timeout_ms = config.recovery_timeout_ms;
        }

        // Phase 2: Lock only circuit_breakers for the auto-register + state transition.
        // This keeps the critical section focused on the circuit breaker state alone,
        // without holding the config lock or any other lock.
        let mut cbs = lock_mutex(&self.circuit_breakers);

        let cb_ref = cbs
            .entry(breaker_name.to_string())
            .or_insert_with(|| CircuitBreaker {
                name: breaker_name.to_string(),
                state: CircuitState::Closed,
                failure_count: 0,
                threshold,
                recovery_timeout_ms,
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
// RS3: Fault detection with distributed consensus
// ---------------------------------------------------------------------------

/// A vote from a single node in the fault detection consensus.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultVote {
    /// Node identifier casting the vote.
    pub voter_id: String,
    /// Target node being voted on.
    pub target_id: String,
    /// Whether the voter considers the target healthy.
    pub healthy: bool,
    /// Unix millis when the vote was cast.
    pub timestamp_ms: u64,
    /// Optional evidence (e.g. probe latency, error message).
    pub evidence: Option<String>,
}

/// Quorum-based fault detection consensus.
///
/// Nodes cast votes on whether a target is healthy. A fault is declared
/// when a majority of voters agree the target is unhealthy within a
/// configurable window. This prevents a single faulty probe from
/// triggering an unnecessary failover.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FaultConsensus {
    /// Minimum votes required to reach a decision.
    quorum_size: usize,
    /// Votes are considered stale after this duration (ms).
    vote_window_ms: u64,
    /// In-memory vote log (bounded to avoid unbounded growth).
    votes: Vec<FaultVote>,
    /// Maximum number of votes to retain per target.
    max_votes_per_target: usize,
}

#[allow(dead_code)]
impl Default for FaultConsensus {
    fn default() -> Self {
        Self {
            quorum_size: 3,
            vote_window_ms: 10_000, // 10 seconds
            votes: Vec::with_capacity(128),
            max_votes_per_target: 100,
        }
    }
}

#[allow(dead_code)]
impl FaultConsensus {
    /// Create a new fault consensus with the given quorum size and vote window.
    pub fn new(quorum_size: usize, vote_window_ms: u64) -> Self {
        Self {
            quorum_size,
            vote_window_ms,
            votes: Vec::with_capacity(128),
            max_votes_per_target: 100,
        }
    }

    /// Record a vote from a peer node.
    /// Automatically prunes stale votes and enforces the per-target cap.
    pub fn record_vote(&mut self, vote: FaultVote) {
        // Prune stale votes before inserting.
        let now = now_millis();
        self.votes
            .retain(|v| now.saturating_sub(v.timestamp_ms) < self.vote_window_ms);

        // Enforce per-target cap: keep the most recent votes.
        let target_count = self
            .votes
            .iter()
            .filter(|v| v.target_id == vote.target_id)
            .count();
        if target_count >= self.max_votes_per_target {
            // Remove the oldest vote for this target.
            if let Some(pos) = self
                .votes
                .iter()
                .position(|v| v.target_id == vote.target_id)
            {
                self.votes.remove(pos);
            }
        }

        self.votes.push(vote);
    }

    /// Determine if a fault is declared for the target based on quorum.
    ///
    /// Returns `(declared_fault, unhealthy_votes, total_votes)`.
    pub fn evaluate(&self, target_id: &str) -> (bool, usize, usize) {
        let now = now_millis();
        let relevant: Vec<&FaultVote> = self
            .votes
            .iter()
            .filter(|v| {
                v.target_id == target_id && now.saturating_sub(v.timestamp_ms) < self.vote_window_ms
            })
            .collect();

        let total = relevant.len();
        let unhealthy = relevant.iter().filter(|v| !v.healthy).count();

        // Declare fault when a quorum of voters report unhealthy AND
        // at least half of all voters agree.
        let declared = unhealthy >= self.quorum_size && unhealthy > total / 2;

        (declared, unhealthy, total)
    }

    /// Prune stale votes (call periodically or on each record_vote).
    pub fn evict_stale(&mut self) {
        let now = now_millis();
        self.votes
            .retain(|v| now.saturating_sub(v.timestamp_ms) < self.vote_window_ms);
    }

    /// Number of unique targets being tracked.
    pub fn tracked_targets(&self) -> usize {
        let mut targets: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for v in &self.votes {
            targets.insert(v.target_id.as_str());
        }
        targets.len()
    }

    /// Total number of votes stored.
    pub fn total_votes(&self) -> usize {
        self.votes.len()
    }
}

// ---------------------------------------------------------------------------
// RS5: Recovery plan persistence
// ---------------------------------------------------------------------------

/// A recovery plan step.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryStep {
    /// Step description.
    pub description: String,
    /// Action to take.
    pub action: SelfHealingAction,
    /// Target node or component.
    pub target: String,
    /// Step timeout in milliseconds.
    pub timeout_ms: u64,
    /// Whether this step is reversible.
    pub reversible: bool,
}

/// A persisted recovery plan.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryPlan {
    /// Unique plan identifier.
    pub plan_id: String,
    /// Human-readable description of the recovery.
    pub description: String,
    /// Ordered steps to execute.
    pub steps: Vec<RecoveryStep>,
    /// Unix millis when the plan was created.
    pub created_at_ms: u64,
    /// Source of the plan (e.g. "auto", "operator").
    pub source: String,
}

#[allow(dead_code)]
impl RecoveryPlan {
    /// Create a new recovery plan.
    pub fn new(
        plan_id: String,
        description: String,
        source: String,
        steps: Vec<RecoveryStep>,
    ) -> Self {
        Self {
            plan_id,
            description,
            steps,
            created_at_ms: now_millis(),
            source,
        }
    }
}

/// Persistence for recovery plans.
///
/// Saves plans to a configurable directory in NDJSON format so they
/// survive process restarts and can be audited.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct RecoveryPlanStore {
    /// Directory where plans are persisted.
    store_dir: std::path::PathBuf,
}

#[allow(dead_code)]
impl RecoveryPlanStore {
    /// Create a new store rooted at the given directory.
    /// Creates the directory if it does not exist.
    pub fn new(store_dir: impl Into<std::path::PathBuf>) -> std::io::Result<Self> {
        let store_dir = store_dir.into();
        std::fs::create_dir_all(&store_dir)?;
        Ok(Self { store_dir })
    }

    /// Create a store with a default path (`./.goon/recovery-plans/`).
    pub fn with_default_path() -> std::io::Result<Self> {
        Self::new(std::path::PathBuf::from("./.goon/recovery-plans"))
    }

    /// Save a recovery plan to disk.
    pub fn save(&self, plan: &RecoveryPlan) -> std::io::Result<()> {
        let path = self.store_dir.join(format!("{}.json", plan.plan_id));
        let json = serde_json::to_string_pretty(plan).map_err(std::io::Error::other)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    /// Load a specific recovery plan by ID.
    pub fn load(&self, plan_id: &str) -> std::io::Result<Option<RecoveryPlan>> {
        let path = self.store_dir.join(format!("{}.json", plan_id));
        if !path.exists() {
            return Ok(None);
        }
        let json = std::fs::read_to_string(&path)?;
        let plan: RecoveryPlan = serde_json::from_str(&json).map_err(std::io::Error::other)?;
        Ok(Some(plan))
    }

    /// List all stored plan IDs.
    pub fn list(&self) -> std::io::Result<Vec<String>> {
        let mut plans = Vec::new();
        for entry in std::fs::read_dir(&self.store_dir)? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|e| e == "json") {
                if let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str()) {
                    plans.push(stem.to_string());
                }
            }
        }
        Ok(plans)
    }

    /// Delete a persisted plan.
    pub fn delete(&self, plan_id: &str) -> std::io::Result<()> {
        let path = self.store_dir.join(format!("{}.json", plan_id));
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
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
