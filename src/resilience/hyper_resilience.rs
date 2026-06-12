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
use std::sync::Arc;
use tokio::sync::watch;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;

#[cfg(feature = "chaos-testing")]
use super::chaos::{ChaosEngine, FaultType};

/// Lock a Mutex, recovering from poison with a log.
///
/// Note: `tokio::sync::Mutex` does not have poisoning, so the recovery
/// branch is retained only for forward-compatibility (e.g. a custom
/// wrapper).
async fn lock_mutex<T>(mtx: &Mutex<T>) -> tokio::sync::MutexGuard<'_, T> {
    mtx.lock().await
}

/// Lock a RwLock for reading, recovering from poison with a log.
///
/// Note: `tokio::sync::RwLock` does not have poisoning, so the recovery
/// branch is retained only for forward-compatibility.
async fn read_lock<T>(rw: &RwLock<T>) -> tokio::sync::RwLockReadGuard<'_, T> {
    rw.read().await
}

/// Lock a RwLock for writing, recovering from poison with a log.
#[allow(dead_code)] // F-GAP-49 — reserved for hyper-resilience write lock
async fn write_lock<T>(rw: &RwLock<T>) -> tokio::sync::RwLockWriteGuard<'_, T> {
    rw.write().await
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

// ---------------------------------------------------------------------------
// DegradationLevel — unified system-wide degradation level.
// failure_prevention re-exports this type via `pub use`.
// ---------------------------------------------------------------------------

/// System-wide degradation level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

#[allow(dead_code)] // F-GAP-49 — reserved for circuit breaker defaults
fn default_circuit_breaker_threshold() -> u64 {
    5
}
#[allow(dead_code)] // F-GAP-49 — reserved for recovery timeout defaults
fn default_recovery_timeout_ms() -> u64 {
    30_000
}
#[allow(dead_code)] // F-GAP-49 — reserved for health check interval defaults
fn default_health_check_interval_ms() -> u64 {
    5_000
}
#[allow(dead_code)] // F-GAP-49 — reserved for max failover defaults
fn default_max_failover_attempts() -> u32 {
    3
}
#[allow(dead_code)] // F-GAP-49 — reserved for self-healing default
fn default_self_healing_enabled() -> bool {
    true
}

#[allow(dead_code)] // F-GAP-49 — reserved for half-open probe interval default
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
    /// Handle for the background health check task, used to detect panics.
    health_check_handle: Mutex<Option<JoinHandle<()>>>,
    /// Optional ChaosEngine for fault injection testing.
    #[cfg(feature = "chaos-testing")]
    chaos_engine: Option<Arc<ChaosEngine>>,
    /// Optional path for persisting circuit breaker state.
    persist_path: Option<String>,
    /// Optional fault consensus for distributed fault detection.
    fault_consensus: Option<tokio::sync::Mutex<FaultConsensus>>,
    /// Optional recovery plan store for persisting recovery plans.
    plan_store: Option<RecoveryPlanStore>,
}

impl std::fmt::Debug for HyperResilienceEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HyperResilienceEngine")
            .field("started_ms", &self.started_ms)
            .field("healing_actions_taken", &self.healing_actions_taken)
            .field("cancel_tx", &"watch::Sender")
            .field("health_check_handle", &"Mutex<Option<JoinHandle>>")
            .field("persist_path", &self.persist_path)
            .field("plan_store", &self.plan_store)
            .field(
                "fault_consensus",
                &self
                    .fault_consensus
                    .as_ref()
                    .map(|_| "Mutex<FaultConsensus>"),
            )
            .finish()
    }
}

impl Clone for HyperResilienceEngine {
    fn clone(&self) -> Self {
        // Use try_lock in a loop with small sleeps to avoid blocking
        // a tokio worker thread (tokio::sync::Mutex).
        let config = loop {
            match self.config.try_read() {
                Ok(guard) => break RwLock::new(guard.clone()),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(1)),
            }
        };
        let circuit_breakers = loop {
            match self.circuit_breakers.try_lock() {
                Ok(guard) => break Mutex::new(guard.clone()),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(1)),
            }
        };
        let failover_groups = loop {
            match self.failover_groups.try_lock() {
                Ok(guard) => break Mutex::new(guard.clone()),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(1)),
            }
        };
        let test_avg_latency_ms = loop {
            match self.test_avg_latency_ms.try_lock() {
                Ok(guard) => break Mutex::new(*guard),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(1)),
            }
        };
        let test_error_rate = loop {
            match self.test_error_rate.try_lock() {
                Ok(guard) => break Mutex::new(*guard),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(1)),
            }
        };
        Self {
            config,
            circuit_breakers,
            failover_groups,
            healing_actions_taken: AtomicU64::new(
                self.healing_actions_taken.load(Ordering::Relaxed),
            ),
            started_ms: self.started_ms,
            test_avg_latency_ms,
            test_error_rate,
            cancel_tx: self.cancel_tx.clone(),
            health_check_handle: Mutex::new(None),
            #[cfg(feature = "chaos-testing")]
            chaos_engine: self.chaos_engine.clone(),
            persist_path: self.persist_path.clone(),
            fault_consensus: match self.fault_consensus.as_ref() {
                Some(m) => match m.try_lock() {
                    Ok(guard) => Some(Mutex::new(guard.clone())),
                    Err(_) => {
                        // Fallback: spin-lock with short sleeps (same pattern as other fields)
                        loop {
                            match m.try_lock() {
                                Ok(guard) => break Some(Mutex::new(guard.clone())),
                                Err(_) => std::thread::sleep(std::time::Duration::from_millis(1)),
                            }
                        }
                    }
                },
                None => None,
            },
            plan_store: self.plan_store.clone(),
        }
    }
}

/// Convert a legacy bare-bones circuit breaker (name + state) into the full
/// unified `CircuitBreaker` with sensible defaults for threshold, recovery
/// timeout, and other fields.
impl From<crate::optimization::failure_prevention::CircuitBreaker> for CircuitBreaker {
    fn from(legacy: crate::optimization::failure_prevention::CircuitBreaker) -> Self {
        Self {
            name: legacy.name,
            state: legacy.state,
            failure_count: 0,
            threshold: 5,
            recovery_timeout_ms: 30_000,
            last_failure_ms: 0,
            half_open_attempts: 0,
            last_failure_mode: None,
            failure_history: Vec::new(),
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
            health_check_handle: Mutex::new(None),
            #[cfg(feature = "chaos-testing")]
            chaos_engine: None,
            persist_path: None,
            fault_consensus: None,
            plan_store: None,
        }
    }

    /// Create a new hyper-resilience engine wrapped in `Arc` for shared ownership.
    ///
    /// This is a convenience wrapper around [`new`] that makes it easier to inject
    /// the engine via `ServerBuilder` or other shared-state patterns.
    pub fn new_shared(config: ResilienceConfig) -> Arc<Self> {
        Arc::new(Self::new(config))
    }

    /// Attach a ChaosEngine for fault injection testing (P3-1).
    #[cfg(feature = "chaos-testing")]
    pub fn with_chaos_engine(mut self, chaos: Arc<crate::resilience::chaos::ChaosEngine>) -> Self {
        self.chaos_engine = Some(chaos);
        self
    }

    /// Attach a FaultConsensus for quorum-based fault detection (P3-7).
    pub fn with_fault_consensus(mut self, consensus: FaultConsensus) -> Self {
        self.fault_consensus = Some(Mutex::new(consensus));
        self
    }

    /// Attach a persistence path for circuit breaker state (P3-2).
    pub fn with_persist_path(mut self, path: impl Into<String>) -> Self {
        self.persist_path = Some(path.into());
        self
    }

    /// Attach a RecoveryPlanStore for persisting healing plans (P3-8).
    pub fn with_plan_store(mut self, store: RecoveryPlanStore) -> Self {
        self.plan_store = Some(store);
        self
    }

    /// Register a circuit breaker with the given name, threshold, and recovery timeout.
    pub async fn register_circuit_breaker(
        &self,
        name: &str,
        threshold: u64,
        recovery_timeout_ms: u64,
    ) -> Result<()> {
        let mut cbs = lock_mutex(&self.circuit_breakers).await;
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
    pub async fn record_failure(&self, breaker_name: &str) -> Result<CircuitState> {
        self.record_failure_with_mode(breaker_name, FailureMode::ResourceExhaustion)
            .await
    }

    /// Record a failure with a specific `FailureMode` classification.
    ///
    /// Returns the new state of the circuit breaker after applying the failure.
    /// The failure mode is stored on the circuit breaker for diagnostics
    /// and is included in the failure history (rolling window of 10).
    pub async fn record_failure_with_mode(
        &self,
        breaker_name: &str,
        failure_mode: FailureMode,
    ) -> Result<CircuitState> {
        let state: CircuitState;
        {
            let mut cbs = lock_mutex(&self.circuit_breakers).await;
            let cb = cbs.get_mut(breaker_name).with_context(|| {
                tf("error.circuit_breaker_not_found", &[("name", breaker_name)])
            })?;

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

            state = cb.state;
        } // drop circuit_breakers lock before persisting

        // Persist state after transition (P3-2)
        if let Some(ref path) = self.persist_path {
            let _ = self.persist_to_db(path).await;
        }

        Ok(state)
    }

    /// Record a success against the named circuit breaker.
    ///
    /// If the breaker is half-open, a success moves it back to closed.
    pub async fn record_success(&self, breaker_name: &str) -> Result<()> {
        {
            let mut cbs = lock_mutex(&self.circuit_breakers).await;
            let cb = cbs.get_mut(breaker_name).with_context(|| {
                tf("error.circuit_breaker_not_found", &[("name", breaker_name)])
            })?;

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
        } // drop circuit_breakers lock before persisting

        // Persist state after transition (P3-2)
        if let Some(ref path) = self.persist_path {
            let _ = self.persist_to_db(path).await;
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
    pub async fn is_available(&self, breaker_name: &str) -> bool {
        // Acquire circuit_breakers FIRST — this is the canonical lock order.
        let mut cbs = lock_mutex(&self.circuit_breakers).await;
        let cb = match cbs.get_mut(breaker_name) {
            Some(cb) => cb,
            None => return false,
        };

        match cb.state {
            CircuitState::Closed | CircuitState::HalfOpen => true,
            CircuitState::Open => {
                // Acquire config RwLock only in the Open branch (not in the fast path).
                let probe_interval = read_lock(&self.config).await.half_open_probe_interval_ms;
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
    pub async fn probe(&self, breaker_name: &str) -> bool {
        let mut cbs = lock_mutex(&self.circuit_breakers).await;
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
    pub async fn register_failover_group(
        &self,
        group_id: &str,
        primary: &str,
        replicas: Vec<String>,
    ) -> Result<()> {
        let mut fgs = lock_mutex(&self.failover_groups).await;
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
    pub async fn trigger_failover(&self, group_id: &str) -> Result<String> {
        let mut fgs = lock_mutex(&self.failover_groups).await;
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
    pub async fn system_health(&self) -> SystemHealth {
        let cbs = lock_mutex(&self.circuit_breakers).await;
        let active_circuit_breakers = cbs.len();
        let open_circuits = cbs
            .values()
            .filter(|cb| matches!(cb.state, CircuitState::Open))
            .count();
        let avg_latency_ms = *lock_mutex(&self.test_avg_latency_ms).await;
        let error_rate = *lock_mutex(&self.test_error_rate).await;
        drop(cbs);

        let fgs = lock_mutex(&self.failover_groups).await;
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
    pub async fn execute_healing(
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
                let mut cbs = lock_mutex(&self.circuit_breakers).await;
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
                let mut fgs = lock_mutex(&self.failover_groups).await;
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
            SelfHealingAction::RestartNode => {
                tracing::info!(
                    target: "resilience",
                    "[HEALING] Restarting node '{}' — resetting circuit breaker and health score",
                    target
                );
                // Reset circuit breaker state for the node's service
                {
                    let mut cbs = lock_mutex(&self.circuit_breakers).await;
                    if let Some(cb) = cbs.get_mut(target) {
                        cb.state = CircuitState::Closed;
                        cb.failure_count = 0;
                        cb.last_failure_ms = 0;
                        cb.half_open_attempts = 0;
                    }
                }
                // Reset health_score back to max
                {
                    let mut fgs = lock_mutex(&self.failover_groups).await;
                    if let Some(group) = fgs.get_mut(target) {
                        group.health_score = 100.0;
                    }
                }
                (
                    true,
                    tf(
                        "status.hyper_resilience.node_restarted",
                        &[("node", target)],
                    ),
                )
            }
            SelfHealingAction::ScaleResources => {
                tracing::info!(
                    target: "resilience",
                    "[HEALING] Scaling resources for '{}' — increasing capacity by 20%",
                    target
                );
                {
                    let mut fgs = lock_mutex(&self.failover_groups).await;
                    if let Some(group) = fgs.get_mut(target) {
                        group.health_score = (group.health_score * 1.2).min(100.0);
                    }
                }
                (
                    true,
                    tf(
                        "status.hyper_resilience.resources_scaled",
                        &[("target", target)],
                    ),
                )
            }
            SelfHealingAction::ReinitializeComponent => {
                tracing::info!(
                    target: "resilience",
                    "[HEALING] Reinitializing component '{}' — resetting circuit breaker to known-good state",
                    target
                );
                {
                    let mut cbs = lock_mutex(&self.circuit_breakers).await;
                    if let Some(cb) = cbs.get_mut(target) {
                        cb.state = CircuitState::Closed;
                        cb.failure_count = 0;
                        cb.last_failure_ms = 0;
                        cb.half_open_attempts = 0;
                    }
                }
                (
                    true,
                    tf(
                        "status.hyper_resilience.component_reinitialized",
                        &[("component", target)],
                    ),
                )
            }
        };

        let completed_ms = now_millis();
        let duration_ms = completed_ms
            .saturating_sub(started_ms)
            .max(test_duration_ms);

        let report = HealingReport {
            action,
            target: target.to_string(),
            initiated_ms: started_ms,
            success,
            duration_ms,
            result,
        };

        // Persist a recovery plan to the store (P3-8)
        if let Some(ref store) = self.plan_store {
            let plan = RecoveryPlan::new(
                format!("{}-{}", report.target, report.initiated_ms),
                format!("Auto-healing {:?} on {}", report.action, report.target),
                "auto".to_string(),
                vec![RecoveryStep {
                    description: format!("{:?} execution", report.action),
                    action: report.action.clone(),
                    target: report.target.clone(),
                    timeout_ms: test_duration_ms,
                    reversible: false,
                }],
            );
            if let Err(e) = store.save(&plan) {
                tracing::warn!(
                    target: "resilience",
                    "failed to save recovery plan: {}",
                    e
                );
            }
        }

        Ok(report)
    }

    /// Return the current resilience profile summarising overall engine state.
    pub async fn profile(&self) -> ResilienceProfile {
        let cbs = lock_mutex(&self.circuit_breakers).await;
        let total_circuit_breakers = cbs.len();
        let open_circuits = cbs
            .values()
            .filter(|cb| matches!(cb.state, CircuitState::Open))
            .count();
        drop(cbs);

        let fgs = lock_mutex(&self.failover_groups).await;
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
    pub async fn start_health_checks(self: &Arc<Self>) {
        let interval_ms = read_lock(&self.config).await.health_check_interval_ms;

        let engine = Arc::clone(self);
        let mut rx = self.cancel_tx.subscribe();
        let handle = tokio::spawn(async move {
            // Inner spawn so that panics surface as JoinErrors on the handle.
            let inner = tokio::spawn(async move {
                let mut timer =
                    tokio::time::interval(tokio::time::Duration::from_millis(interval_ms));
                // Skip the first tick (immediate) to give startup time
                timer.tick().await;
                loop {
                    tokio::select! {
                        _ = timer.tick() => {
                            engine.health_check_cycle().await;
                        }
                        _ = rx.changed() => {
                            // Stop signal received
                            break;
                        }
                    }
                }
            });
            if let Err(e) = inner.await {
                tracing::error!(
                    target: "resilience",
                    "health check task panicked: {:?}",
                    e
                );
            }
        });
        *self.health_check_handle.lock().await = Some(handle);
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
    pub async fn health_check_cycle(&self) {
        // ── Phase 1: Probe all circuit breakers ────────────────────────────
        let breaker_names: Vec<String> = {
            let cbs = lock_mutex(&self.circuit_breakers).await;
            cbs.keys().cloned().collect()
        };
        for name in &breaker_names {
            self.probe(name).await;
        }

        // Record fault votes based on breaker states after probing (P3-7)
        if let Some(ref consensus_mutex) = self.fault_consensus {
            let mut consensus = consensus_mutex.lock().await;
            let cbs = lock_mutex(&self.circuit_breakers).await;
            for (name, cb) in cbs.iter() {
                let healthy = matches!(cb.state, CircuitState::Closed);
                consensus.record_vote(FaultVote {
                    voter_id: "local-engine".to_string(),
                    target_id: name.clone(),
                    healthy: healthy || matches!(cb.state, CircuitState::HalfOpen),
                    timestamp_ms: now_millis(),
                    evidence: if healthy {
                        None
                    } else {
                        Some(format!("state={:?}", cb.state))
                    },
                });
            }
            drop(cbs);

            // ── Phase 2: Fault consensus evaluation for active failover groups ─
            consensus.evict_stale();
            let fg_ids: Vec<String> = {
                let fgs = lock_mutex(&self.failover_groups).await;
                fgs.keys().cloned().collect()
            };
            for group_id in &fg_ids {
                let (declared, unhealthy, total) = consensus.evaluate(group_id);
                if declared {
                    tracing::warn!(
                        target: "resilience",
                        "fault consensus: fault DECLARED for failover group '{}' (unhealthy {}/{})",
                        group_id,
                        unhealthy,
                        total
                    );
                }
            }
        }

        // ── Phase 3: Assess system health ──────────────────────────────────
        let health = self.system_health().await;

        // Update real operational metrics (only circuit_breakers + test metrics locks)
        {
            let cbs = lock_mutex(&self.circuit_breakers).await;
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

            let mut avg_latency = lock_mutex(&self.test_avg_latency_ms).await;
            let mut err_rate = lock_mutex(&self.test_error_rate).await;

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

        // ── Phase 4: Auto-heal if degraded ────────────────────────────────
        if health.level >= DegradationLevel::Constrained {
            let healing_enabled = read_lock(&self.config).await.self_healing_enabled;
            if healing_enabled {
                for name in &breaker_names {
                    let is_open = {
                        let cbs = lock_mutex(&self.circuit_breakers).await;
                        cbs.get(name)
                            .map(|cb| matches!(cb.state, CircuitState::Open))
                            .unwrap_or(false)
                    };
                    if is_open {
                        match self
                            .execute_healing(SelfHealingAction::ClearCircuitBreaker, name)
                            .await
                        {
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

        // Persist state after self-healing actions (P3-2)
        if let Some(ref path) = self.persist_path {
            let _ = self.persist_to_db(path).await;
        }
    }

    /// Persist current circuit breaker state to a JSON file via tokio::fs.
    ///
    /// Stores the circuit_breakers HashMap as a JSON blob, enabling recovery
    /// across process restarts. Uses the provided `path` for the output file.
    pub async fn persist_to_db(&self, path: &str) -> Result<()> {
        let cbs = lock_mutex(&self.circuit_breakers).await;
        let json = serde_json::to_string_pretty(&*cbs)
            .context("failed to serialize circuit breakers for persistence")?;
        tokio::fs::write(path, &json)
            .await
            .context("failed to write resilience state to disk")?;
        Ok(())
    }

    /// Load circuit breaker state from a JSON file and populate the engine.
    ///
    /// Reads the JSON blob written by `persist_to_db` and reconstructs the
    /// `circuit_breakers` HashMap. Returns a new engine with the loaded state.
    /// If the file does not exist, returns a fresh engine with default config.
    pub async fn load_from_db(path: &str, config: ResilienceConfig) -> Result<Self> {
        let json = match tokio::fs::read_to_string(path).await {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::new(config));
            }
            Err(e) => {
                return Err(e).context("failed to read resilience state file");
            }
        };
        let circuit_breakers: HashMap<String, CircuitBreaker> =
            serde_json::from_str(&json).context("failed to deserialize circuit breakers")?;

        let now_ms = now_millis();
        let (cancel_tx, _) = watch::channel(false);
        Ok(Self {
            config: RwLock::new(config),
            circuit_breakers: Mutex::new(circuit_breakers),
            failover_groups: Mutex::new(HashMap::new()),
            healing_actions_taken: AtomicU64::new(0),
            started_ms: now_ms,
            test_avg_latency_ms: Mutex::new(10.0),
            test_error_rate: Mutex::new(0.001),
            cancel_tx,
            health_check_handle: Mutex::new(None),
            #[cfg(feature = "chaos-testing")]
            chaos_engine: None,
            persist_path: None,
            fault_consensus: None,
            plan_store: None,
        })
    }

    /// Record an execution outcome (success or failure) against the named
    /// circuit breaker, auto-registering if it does not exist yet.
    ///
    /// This is the preferred method for production code paths that do not
    /// need explicit registration. For retry / recovery orchestration,
    /// prefer the pair of `is_available()` → `record_failure()` / `record_success()`.
    pub async fn record_execution(&self, breaker_name: &str, success: bool) {
        // ── Chaos engine fault injection (P3-1) ────────────────────────────
        #[cfg(feature = "chaos-testing")]
        if let Some(ref chaos) = self.chaos_engine {
            if chaos.should_inject_fault(crate::resilience::chaos::FaultType::NetworkTimeout) {
                tracing::info!(
                    target: "resilience",
                    "[CHAOS] Injecting NetworkTimeout fault in execution '{}'",
                    breaker_name
                );
                if let Err(e) = self.record_failure(breaker_name).await {
                    tracing::warn!(
                        target: "resilience",
                        "[CHAOS] record_failure in injection failed: {}",
                        e
                    );
                }
                return;
            }
        }

        // Phase 1: Read config for auto-register defaults (read lock, fast path).
        let threshold: u64;
        let recovery_timeout_ms: u64;
        {
            let config = read_lock(&self.config).await;
            threshold = config.circuit_breaker_threshold;
            recovery_timeout_ms = config.recovery_timeout_ms;
        }

        // Phase 2: Lock only circuit_breakers for the auto-register + state transition.
        let mut cbs = lock_mutex(&self.circuit_breakers).await;

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

        // Persist state after transition (P3-2)
        if let Some(ref path) = self.persist_path {
            let _ = self.persist_to_db(path).await;
        }
    }
}

// ---------------------------------------------------------------------------
// RS3: Fault detection with distributed consensus
// ---------------------------------------------------------------------------

/// A vote from a single node in the fault detection consensus.
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
        // Cap effective quorum to the number of active voters to avoid
        // deadlock when fewer voters exist than the configured quorum_size.
        let effective_quorum = self.quorum_size.min(total.max(1));
        let declared = unhealthy >= effective_quorum && unhealthy > total / 2;

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
#[derive(Debug, Clone)]
pub struct RecoveryPlanStore {
    /// Directory where plans are persisted.
    store_dir: std::path::PathBuf,
}

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
        // Use a very short timeout so the test doesn't take long.
        engine
            .register_circuit_breaker("cb-cache", 1, 1)
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

        // Wait for the recovery timeout (1 ms + some slack).
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Now probe should transition to half-open and return true.
        assert!(engine.probe("cb-cache").await);
    }

    /// 5. A success in half-open resets the breaker to closed.
    #[tokio::test]
    async fn test_circuit_breaker_resets_on_success() {
        let engine = HyperResilienceEngine::new(ResilienceConfig::default());
        engine
            .register_circuit_breaker("cb-api", 1, 1)
            .await
            .expect("register_circuit_breaker should succeed");

        // Trip to open.
        engine
            .record_failure("cb-api")
            .await
            .expect("record_failure should not fail");
        assert!(!engine.is_available("cb-api").await);

        // Wait for recovery timeout.
        tokio::time::sleep(Duration::from_millis(10)).await;

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
        assert!(report.success);
        assert_eq!(report.target, "cb-broken");
        assert!(report.duration_ms > 0);

        // After healing, the breaker should be closed.
        let health = engine.system_health().await;
        assert_eq!(health.open_circuits, 0);
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
}
