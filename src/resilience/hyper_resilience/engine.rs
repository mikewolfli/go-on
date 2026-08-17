//! The hyper-resilience engine: circuit breakers, failover groups, health
//! monitoring, and self-healing orchestration.

use crate::i18n::runtime::tf;
use anyhow::{bail, Context, Result};
use futures_util::future::join_all;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::{Mutex, RwLock};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::state::{
    apply_breaker_outcome, degradation_from_counts, reset_breaker, transition_breaker,
};
use super::types::*;
use super::{lock_mutex, read_lock};

/// The hyper-resilience engine that orchestrates circuit breakers, failover
/// groups, health monitoring, and self-healing actions.
///
/// All methods are thread-safe via fine-grained locks:
/// - `config`: `RwLock` (read-heavy, rarely written)
/// - `circuit_breakers`: `Mutex` (frequently mutated)
/// - `failover_groups`: `Mutex` (separate from circuit breakers)
/// - `healing_actions_taken`: `AtomicU64` (lock-free counter of **executed** actions)
/// - `healing_actions_simulated`: `AtomicU64` (lock-free counter of **simulated** actions)
/// - `test_metrics`: `Mutex<TestMetrics>` (consolidated latency + error rate)
pub struct HyperResilienceEngine {
    config: RwLock<ResilienceConfig>,
    circuit_breakers: Mutex<HashMap<String, CircuitBreaker>>,
    failover_groups: Mutex<HashMap<String, FailoverGroup>>,
    /// Per-service health monitors (unified with circuit breakers — the
    /// engine is the single resilience authority for breakers + health +
    /// degradation, replacing the former `failure_prevention` state machine).
    service_health: Mutex<HashMap<String, ServiceHealth>>,
    /// Per-service outcome counters driving the health monitor.
    service_counters: Mutex<HashMap<String, ServiceCounters>>,
    healing_actions_taken: AtomicU64,
    /// Self-healing actions that only logged a simulated effect (node restart
    /// / resource scaling are infrastructure-level and meaningless in-process).
    healing_actions_simulated: AtomicU64,
    started_ms: u64,
    test_metrics: Mutex<TestMetrics>,
    cancel_tx: watch::Sender<bool>,
    /// Handle for the background health check task, used to detect panics.
    health_check_handle: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for HyperResilienceEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HyperResilienceEngine")
            .field("started_ms", &self.started_ms)
            .field("healing_actions_taken", &self.healing_actions_taken)
            .field("healing_actions_simulated", &self.healing_actions_simulated)
            .field("cancel_tx", &"watch::Sender")
            .field("health_check_handle", &"Mutex<Option<JoinHandle>>")
            .finish()
    }
}

impl HyperResilienceEngine {
    /// Create a new hyper-resilience engine with the given configuration.
    pub fn new(config: ResilienceConfig) -> Self {
        let now_ms = crate::shared::timestamps::now_ts_ms_u64();
        let (cancel_tx, _) = watch::channel(false);
        Self {
            config: RwLock::new(config),
            circuit_breakers: Mutex::new(HashMap::new()),
            failover_groups: Mutex::new(HashMap::new()),
            service_health: Mutex::new(HashMap::new()),
            service_counters: Mutex::new(HashMap::new()),
            healing_actions_taken: AtomicU64::new(0),
            healing_actions_simulated: AtomicU64::new(0),
            started_ms: now_ms,
            // No data yet: both start at 0.0 and are populated by
            // `health_check_cycle` from real breaker states + measured
            // per-service latencies (0.0 = "no data", not a measurement).
            test_metrics: Mutex::new(TestMetrics {
                avg_latency_ms: 0.0,
                error_rate: 0.0,
            }),
            cancel_tx,
            health_check_handle: Mutex::new(None),
        }
    }

    /// Register a circuit breaker with the given name, threshold, and recovery timeout.
    pub async fn register_circuit_breaker(
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
            CircuitBreaker::new(name.to_string(), threshold, recovery_timeout_ms),
        );
        Ok(())
    }

    // ── Service health monitoring (unified authority) ───────────────────
    // The former `optimization::failure_prevention` state machine was merged
    // into this engine: per-service health monitors, degradation levels and
    // recovery now live here alongside the circuit breakers, so breaker state,
    // health status and degradation all come from one source.

    /// Register a service for health monitoring + breaker tracking.
    ///
    /// Idempotent. Lock scopes are deliberately kept separate (never hold two
    /// locks at once) to preserve the documented `circuit_breakers`-first lock
    /// order used by the background health-check loop.
    pub fn register_service(&self, name: &str) {
        {
            let mut sh = lock_mutex(&self.service_health);
            sh.entry(name.to_string()).or_insert_with(|| ServiceHealth {
                service_name: name.to_string(),
                status: HealthStatus::Healthy,
                success_rate: 1.0,
                error_rate: 0.0,
                // 0.0 means "no measurement yet" (same convention as
                // TestMetrics) — a placeholder 100ms would fake a real latency.
                avg_latency_ms: 0.0,
                last_check_timestamp: crate::shared::timestamps::now_ts_ms_u64() / 1000,
            });
        }
        {
            let mut sc = lock_mutex(&self.service_counters);
            sc.entry(name.to_string()).or_default();
        }
        {
            let threshold = read_lock(&self.config).circuit_breaker_threshold;
            let recovery = read_lock(&self.config).recovery_timeout_ms;
            let mut cbs = lock_mutex(&self.circuit_breakers);
            if !cbs.contains_key(name) {
                cbs.insert(
                    name.to_string(),
                    CircuitBreaker::new(name.to_string(), threshold, recovery),
                );
            }
        }
    }

    /// Update the per-service health monitor (counters + status) for a single
    /// execution outcome.
    ///
    /// Shared by the sync `record_outcome` and the async `record_execution`
    /// paths so `health_report()` / `degradation_level()` always derive from
    /// the same counters as the breaker state. `latency_ms: None` keeps the
    /// previous average when the caller has no latency measurement.
    fn record_service_outcome(&self, name: &str, success: bool, latency_ms: Option<u64>) {
        let max_failure_threshold = read_lock(&self.config).circuit_breaker_threshold;

        let (success_rate, error_rate, avg_latency_ms, status) = {
            let mut sc = lock_mutex(&self.service_counters);
            let counters = sc.entry(name.to_string()).or_default();
            counters.total_requests += 1;
            if success {
                counters.successful_requests += 1;
                counters.consecutive_failures = 0;
            } else {
                counters.consecutive_failures += 1;
            }
            let total = counters.total_requests;
            let success_n = counters.successful_requests;
            let failure_count = counters.consecutive_failures;
            let success_rate = if total == 0 {
                1.0
            } else {
                success_n as f64 / total as f64
            };
            let error_rate = if total == 0 {
                0.0
            } else {
                (total.saturating_sub(success_n)) as f64 / total as f64
            };
            // When failures exceed the breaker threshold, blend in a severity
            // factor so the health status reflects how badly the breaker tripped.
            let error_rate = if failure_count > 0 && failure_count >= max_failure_threshold {
                let severity = (failure_count as f64 / max_failure_threshold as f64).min(1.0);
                error_rate.max(severity * 0.5)
            } else {
                error_rate
            };
            let prior_avg = lock_mutex(&self.service_health)
                .get(name)
                .map(|h| h.avg_latency_ms)
                .unwrap_or(0.0);
            let avg_latency_ms = match latency_ms {
                Some(measured) => {
                    let samples = total.max(1) as f64;
                    let previous_weight = (samples - 1.0).max(0.0);
                    if previous_weight == 0.0 {
                        measured as f64
                    } else {
                        (prior_avg * previous_weight + measured as f64) / samples
                    }
                }
                None => prior_avg,
            };
            let status = if error_rate > HEALTH_ERROR_RATE_THRESHOLD {
                HealthStatus::Unhealthy
            } else if success_rate < HEALTH_SUCCESS_RATE_THRESHOLD {
                HealthStatus::Degraded
            } else {
                HealthStatus::Healthy
            };
            (success_rate, error_rate, avg_latency_ms, status)
        };

        {
            let mut sh = lock_mutex(&self.service_health);
            let h = sh.entry(name.to_string()).or_insert_with(|| ServiceHealth {
                service_name: name.to_string(),
                status: HealthStatus::Healthy,
                success_rate: 1.0,
                error_rate: 0.0,
                // 0.0 = no measurement yet (same convention as TestMetrics).
                avg_latency_ms: 0.0,
                last_check_timestamp: crate::shared::timestamps::now_ts_ms_u64() / 1000,
            });
            h.success_rate = success_rate;
            h.error_rate = error_rate;
            h.avg_latency_ms = avg_latency_ms;
            h.status = status;
            h.last_check_timestamp = crate::shared::timestamps::now_ts_ms_u64() / 1000;
        }
    }

    /// Record a request outcome for a service.
    ///
    /// Updates the per-service health monitor (success/error rates, EMA
    /// latency, status) and drives the service's circuit breaker — one call
    /// for both, so breaker and health can never drift. Synchronous: the
    /// underlying locks are `std::Mutex` and are never held across an `.await`.
    pub fn record_outcome(&self, name: &str, success: bool, latency_ms: u64) {
        let (max_failure_threshold, recovery_timeout_ms) = {
            let config = read_lock(&self.config);
            (config.circuit_breaker_threshold, config.recovery_timeout_ms)
        };

        self.record_service_outcome(name, success, Some(latency_ms));

        // Drive the breaker from the same outcome (sync inline update).
        let mut cbs = lock_mutex(&self.circuit_breakers);
        let cb = cbs.entry(name.to_string()).or_insert_with(|| {
            CircuitBreaker::new(name.to_string(), max_failure_threshold, recovery_timeout_ms)
        });
        apply_breaker_outcome(cb, success);
    }

    /// Current health snapshot for a service (None when not registered).
    pub fn service_health(&self, name: &str) -> Option<ServiceHealth> {
        lock_mutex(&self.service_health).get(name).cloned()
    }

    /// Health snapshots for all registered services.
    pub fn health_report(&self) -> Vec<ServiceHealth> {
        lock_mutex(&self.service_health).values().cloned().collect()
    }

    /// Degradation level for a service (Normal/Degraded/Constrained/Emergency).
    pub fn degradation_level(&self, name: &str) -> DegradationLevel {
        match lock_mutex(&self.service_health).get(name) {
            Some(h) => match h.status {
                HealthStatus::Healthy => DegradationLevel::Normal,
                HealthStatus::Degraded => DegradationLevel::Degraded,
                HealthStatus::Unhealthy => {
                    if h.success_rate < 0.5 {
                        DegradationLevel::Emergency
                    } else {
                        DegradationLevel::Constrained
                    }
                }
            },
            None => DegradationLevel::Normal,
        }
    }

    /// Whether the service should be degraded (falls back to a simpler path).
    pub fn should_degrade(&self, name: &str) -> bool {
        self.degradation_level(name) >= DegradationLevel::Constrained
    }

    /// Recover one or all services back to the healthy baseline (breaker
    /// closed, counters zeroed, health reset). Returns the recovered names.
    pub fn recover_services(&self, name: Option<&str>) -> Vec<String> {
        let names: Vec<String> = match name {
            Some(n) => vec![n.to_string()],
            None => lock_mutex(&self.service_health).keys().cloned().collect(),
        };
        let mut recovered = Vec::new();
        for n in names {
            if self.recover_service(&n) {
                recovered.push(n);
            }
        }
        recovered.sort();
        recovered
    }

    fn recover_service(&self, name: &str) -> bool {
        let health = lock_mutex(&self.service_health).get(name).cloned();
        let Some(health) = health else {
            return false;
        };
        let breaker_state = lock_mutex(&self.circuit_breakers)
            .get(name)
            .map(|cb| cb.state)
            .unwrap_or(CircuitState::Closed);
        let failure_count = lock_mutex(&self.service_counters)
            .get(name)
            .map(|c| c.consecutive_failures)
            .unwrap_or(0);
        let already_healthy = matches!(health.status, HealthStatus::Healthy)
            && breaker_state == CircuitState::Closed
            && failure_count == 0;
        if already_healthy {
            return false;
        }
        {
            let mut sc = lock_mutex(&self.service_counters);
            if let Some(c) = sc.get_mut(name) {
                c.consecutive_failures = 0;
                c.total_requests = 0;
                c.successful_requests = 0;
            }
        }
        {
            let mut cbs = lock_mutex(&self.circuit_breakers);
            if let Some(cb) = cbs.get_mut(name) {
                reset_breaker(cb);
            }
        }
        {
            let mut sh = lock_mutex(&self.service_health);
            if let Some(h) = sh.get_mut(name) {
                h.status = HealthStatus::Healthy;
                h.success_rate = 1.0;
                h.error_rate = 0.0;
                h.last_check_timestamp = crate::shared::timestamps::now_ts_ms_u64() / 1000;
            }
        }
        true
    }

    /// Snapshot all circuit breakers as
    /// `(name, state, failure_count, total_requests, successful_requests, last_state_change_ms)`.
    pub fn breaker_snapshots(&self) -> Vec<(String, CircuitState, u64, u64, u64, u64)> {
        // Lock circuit_breakers first (documented order), then release before
        // touching counters so no two locks are held simultaneously.
        let snap: Vec<(String, CircuitState, u64, u64)> = lock_mutex(&self.circuit_breakers)
            .iter()
            .map(|(n, cb)| {
                (
                    n.clone(),
                    cb.state,
                    cb.failure_count,
                    cb.last_state_change_ms,
                )
            })
            .collect();
        let counters = lock_mutex(&self.service_counters);
        snap.into_iter()
            .map(|(name, state, failures, last_change)| {
                let c = counters.get(&name);
                (
                    name,
                    state,
                    failures,
                    c.map(|c| c.total_requests).unwrap_or(0),
                    c.map(|c| c.successful_requests).unwrap_or(0),
                    last_change,
                )
            })
            .collect()
    }

    /// Current breaker state for a service (Closed when unregistered).
    pub fn breaker_state(&self, name: &str) -> CircuitState {
        lock_mutex(&self.circuit_breakers)
            .get(name)
            .map(|cb| cb.state)
            .unwrap_or(CircuitState::Closed)
    }

    /// Number of currently open circuit breakers.
    pub fn open_breaker_count(&self) -> u32 {
        lock_mutex(&self.circuit_breakers)
            .values()
            .filter(|cb| cb.state == CircuitState::Open)
            .count() as u32
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
            let mut cbs = lock_mutex(&self.circuit_breakers);
            let cb = cbs.get_mut(breaker_name).with_context(|| {
                tf("error.circuit_breaker_not_found", &[("name", breaker_name)])
            })?;

            let now = crate::shared::timestamps::now_ts_ms_u64();

            // Track failure mode
            cb.last_failure_mode = Some(failure_mode);
            cb.failure_history.push(failure_mode);
            if cb.failure_history.len() > 10 {
                cb.failure_history.remove(0);
            }

            transition_breaker(cb, BreakerOutcome::Failure, now);

            state = cb.state;
        } // drop circuit_breakers lock

        Ok(state)
    }

    /// Record a success against the named circuit breaker.
    ///
    /// If the breaker is half-open, a success moves it back to closed.
    pub async fn record_success(&self, breaker_name: &str) -> Result<()> {
        {
            let mut cbs = lock_mutex(&self.circuit_breakers);
            let cb = cbs.get_mut(breaker_name).with_context(|| {
                tf("error.circuit_breaker_not_found", &[("name", breaker_name)])
            })?;

            transition_breaker(
                cb,
                BreakerOutcome::Success,
                crate::shared::timestamps::now_ts_ms_u64(),
            );
        } // drop circuit_breakers lock

        Ok(())
    }

    /// Check whether the named circuit breaker is currently available
    /// (closed or half-open).
    ///
    /// If the breaker is **Open** and the time since the last failure exceeds
    /// the breaker's `recovery_timeout_ms`, this method automatically
    /// transitions it to **HalfOpen**, enabling the self-healing /
    /// auto-recovery pattern without requiring an explicit `probe()` call.
    ///
    /// The half-open timing is unified on `recovery_timeout_ms` — the same
    /// standard `probe()` uses — so the same breaker behaves identically
    /// regardless of which entry point transitions it.
    ///
    /// # Lock ordering
    /// Acquires `circuit_breakers` Mutex **first**, and never takes the
    /// config RwLock (the recovery timeout lives on the breaker itself),
    /// matching the pattern used by `record_failure_with_mode`,
    /// `record_success`, `record_execution`, etc.
    pub async fn is_available(&self, breaker_name: &str) -> bool {
        // Acquire circuit_breakers FIRST — this is the canonical lock order.
        let mut cbs = lock_mutex(&self.circuit_breakers);
        let cb = match cbs.get_mut(breaker_name) {
            Some(cb) => cb,
            None => return false,
        };

        match cb.state {
            CircuitState::Closed | CircuitState::HalfOpen => true,
            CircuitState::Open => {
                let recovery = cb.recovery_timeout_ms;
                let now = crate::shared::timestamps::now_ts_ms_u64();
                if now >= cb.last_failure_ms + recovery {
                    cb.state = CircuitState::HalfOpen;
                    cb.last_state_change_ms = now;
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
    /// Delegates to [`Self::is_available`] — the two were byte-identical
    /// (both mutate Open → HalfOpen on timeout elapse), so keeping one
    /// implementation prevents future drift.
    pub async fn probe(&self, breaker_name: &str) -> bool {
        self.is_available(breaker_name).await
    }

    /// Register a failover group with a primary node and a list of replicas.
    pub async fn register_failover_group(
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
    pub async fn trigger_failover(&self, group_id: &str) -> Result<String> {
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
        let now = crate::shared::timestamps::now_ts_ms_u64();
        group.last_failover_ms = now;
        group.failover_count += 1;
        group.health_score = f64::max(0.0, group.health_score - 10.0);

        Ok(next_replica)
    }

    /// Return a snapshot of the current system health.
    pub async fn system_health(&self) -> SystemHealth {
        let cbs = lock_mutex(&self.circuit_breakers);
        let active_circuit_breakers = cbs.len();
        let open_circuits = cbs
            .values()
            .filter(|cb| matches!(cb.state, CircuitState::Open))
            .count();
        let metrics = lock_mutex(&self.test_metrics);
        let avg_latency_ms = metrics.avg_latency_ms;
        let error_rate = metrics.error_rate;
        drop(metrics);
        drop(cbs);

        let fgs = lock_mutex(&self.failover_groups);
        let active_failovers = fgs.values().filter(|g| g.failover_count > 0).count();
        drop(fgs);

        // Determine degradation level based on the ratio of open circuits.
        let level =
            degradation_from_counts(open_circuits, active_circuit_breakers, active_failovers);

        SystemHealth {
            level,
            active_circuit_breakers,
            open_circuits,
            active_failovers,
            avg_latency_ms,
            error_rate,
            timestamp_ms: crate::shared::timestamps::now_ts_ms_u64(),
        }
    }

    /// Attempt a real TCP health check against the target (parsed as `host:port`).
    /// Returns `true` if the connection succeeds within the timeout.
    /// This provides actual observability into whether the service is reachable
    /// rather than relying on purely in-memory simulation.
    async fn try_health_check(&self, target: &str, timeout_ms: u64) -> (bool, String) {
        // Try to parse target as host:port
        if let Some((host, port_str)) = target.split_once(':') {
            if let Ok(port) = port_str.parse::<u16>() {
                let addr = format!("{}:{}", host, port);
                match tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
                    TcpStream::connect(&addr),
                )
                .await
                {
                    Ok(Ok(_)) => {
                        return (true, format!("TCP health check PASSED for {}", addr));
                    }
                    Ok(Err(e)) => {
                        return (
                            false,
                            format!("TCP health check FAILED for {}: {}", addr, e),
                        );
                    }
                    Err(_) => {
                        return (
                            false,
                            format!(
                                "TCP health check TIMEOUT for {} after {}ms",
                                addr, timeout_ms
                            ),
                        );
                    }
                }
            }
        }
        // Target is not a host:port — log a hint but treat as non-fatal
        (
            false,
            format!(
                "target '{}' is not a host:port address — no TCP health check possible",
                target
            ),
        )
    }

    /// Execute a self-healing action and return a report.
    ///
    /// Performs a real health check where possible and distinguishes **executed**
    /// actions (which produced a real in-process state change) from **simulated**
    /// ones (logged only):
    ///
    /// - `ClearCircuitBreaker` / `PromoteReplica` / `ReinitializeComponent` —
    ///   real effects (breaker reset / leader promotion / component state reset)
    ///   and increment `healing_actions_taken`.
    /// - `RestartNode` / `ScaleResources` — infrastructure-level actions that are
    ///   meaningless inside a single process; they are logged as `SIMULATED` and
    ///   increment `healing_actions_simulated` instead, so metrics never count
    ///   simulation as execution.
    pub async fn execute_healing(
        &self,
        action: SelfHealingAction,
        target: &str,
    ) -> Result<HealingReport> {
        let started_ms = crate::shared::timestamps::now_ts_ms_u64();

        // Perform a real TCP health check to determine if the target is reachable.
        let (healthy, health_check_result) = self.try_health_check(target, 3_000).await;
        tracing::info!(
            target: "resilience",
            target = %target,
            healthy = %healthy,
            "[HEALTH_CHECK] {}",
            health_check_result
        );

        // Simulate execution duration: healing actions are inherently slow
        // (restart, scale, reinit), so honor the nominal duration with a real
        // sleep and measure the actual elapsed time for the report — previously
        // the duration was computed but never actually waited for.
        let nominal_duration_ms: u64 = match &action {
            SelfHealingAction::RestartNode => 2_000,
            SelfHealingAction::PromoteReplica => 500,
            SelfHealingAction::ClearCircuitBreaker => 100,
            SelfHealingAction::ScaleResources => 3_000,
            SelfHealingAction::ReinitializeComponent => 1_000,
        };

        let (success, result, real_effect) = match &action {
            SelfHealingAction::ClearCircuitBreaker => {
                // Only clear a breaker whose target actually passes the health
                // probe. Previously the probe result was logged and then
                // ignored: a still-unhealthy service had its breaker reset
                // every cycle, causing a reset-open oscillation that bypassed
                // half-open recovery entirely. Non-host:port targets (breaker
                // names) cannot be probed — they are left open and recover
                // naturally via half-open on the next successful call.
                if !healthy {
                    tracing::warn!(
                        target: "resilience",
                        action = "ClearCircuitBreaker",
                        target = %target,
                        healthy = %healthy,
                        "[HEALING] SKIPPED: target '{}' did not pass the health probe; leaving the breaker open",
                        target
                    );
                    return Ok(HealingReport {
                        action,
                        target: target.to_string(),
                        initiated_ms: started_ms,
                        success: false,
                        duration_ms: crate::shared::timestamps::now_ts_ms_u64()
                            .saturating_sub(started_ms),
                        result: format!(
                            "health probe failed ({health_check_result}); breaker left open"
                        ),
                    });
                }
                tracing::info!(
                    target: "resilience",
                    action = "ClearCircuitBreaker",
                    target = %target,
                    healthy = %healthy,
                    "[HEALING] EXECUTED: reset circuit breaker for '{}' — failure count reset to 0 and the breaker transitions to Closed state",
                    target
                );
                tracing::info!(
                    target: "resilience",
                    "[SUGGESTION] Actionable: verify the downstream service at '{}' is healthy before clearing the breaker. Consider adding a health check endpoint.",
                    target
                );
                // Clear the circuit breaker if it exists.
                let mut cbs = lock_mutex(&self.circuit_breakers);
                if let Some(cb) = cbs.get_mut(target) {
                    reset_breaker(cb);
                    (
                        true,
                        tf("status.hyper_resilience.breaker_reset", &[("name", target)]),
                        true,
                    )
                } else {
                    (
                        false,
                        tf("error.circuit_breaker_not_found", &[("name", target)]),
                        false,
                    )
                }
            }
            SelfHealingAction::PromoteReplica => {
                tracing::info!(
                    target: "resilience",
                    action = "PromoteReplica",
                    target = %target,
                    healthy = %healthy,
                    "[HEALING] EXECUTED: promote a replica for failover group '{}' — the failover group's current leader is updated",
                    target
                );
                tracing::info!(
                    target: "resilience",
                    "[SUGGESTION] Actionable: ensure replica nodes are pre-warmed and ready to accept traffic. Verify health of the promoted replica before rerouting.",
                );
                // Real effect: promote the first replica as the new leader.
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
                    group.last_failover_ms = crate::shared::timestamps::now_ts_ms_u64();
                    (
                        true,
                        tf(
                            "status.hyper_resilience.replica_promoted",
                            &[("replica", &new_leader), ("group", target)],
                        ),
                        true,
                    )
                } else {
                    (
                        false,
                        tf("error.failover_group_not_found", &[("name", target)]),
                        false,
                    )
                }
            }
            SelfHealingAction::RestartNode => {
                // Infrastructure-level: a single process cannot restart itself.
                // Logged as SIMULATED and counted as simulated — no in-process
                // state is mutated and `healing_actions_taken` is not bumped.
                tracing::info!(
                    target: "resilience",
                    action = "RestartNode",
                    target = %target,
                    healthy = %healthy,
                    "[HEALING] SIMULATED: restart node '{}' — in production this would send a SIGTERM, wait for graceful shutdown, then restart the process via the process manager (systemd/k8s); no in-process state was changed",
                    target
                );
                tracing::info!(
                    target: "resilience",
                    "[SUGGESTION] Actionable: implement a /healthz endpoint on '{}' and use a process supervisor that auto-restarts on failure. Configure crash loop backoff.",
                    target
                );
                (
                    true,
                    format!(
                        "{} (simulated — no in-process state changed)",
                        tf(
                            "status.hyper_resilience.node_restarted",
                            &[("node", target)]
                        )
                    ),
                    false,
                )
            }
            SelfHealingAction::ScaleResources => {
                // Infrastructure-level: scaling CPU/memory/replicas is managed
                // by the orchestrator, not this process. Logged as SIMULATED.
                tracing::info!(
                    target: "resilience",
                    action = "ScaleResources",
                    target = %target,
                    healthy = %healthy,
                    "[HEALING] SIMULATED: scale resources for '{}' — in production this would increase CPU/memory limits, scale up replica count, or adjust autoscaling thresholds; no in-process state was changed",
                    target
                );
                tracing::info!(
                    target: "resilience",
                    "[SUGGESTION] Actionable: configure horizontal pod autoscaling on '{}' with target CPU utilization at 70% and memory at 80%. Set min/max replicas.",
                    target
                );
                (
                    true,
                    format!(
                        "{} (simulated — no in-process state changed)",
                        tf(
                            "status.hyper_resilience.resources_scaled",
                            &[("target", target)]
                        )
                    ),
                    false,
                )
            }
            SelfHealingAction::ReinitializeComponent => {
                tracing::info!(
                    target: "resilience",
                    action = "ReinitializeComponent",
                    target = %target,
                    healthy = %healthy,
                    "[HEALING] EXECUTED: reinitialize component '{}' — the component's circuit breaker is reset (Closed, failure count 0)",
                    target
                );
                tracing::info!(
                    target: "resilience",
                    "[SUGGESTION] Actionable: implement a /reload endpoint on '{}' that refreshes config without full restart. Use graceful connection draining during reinit.",
                    target
                );
                {
                    let mut cbs = lock_mutex(&self.circuit_breakers);
                    if let Some(cb) = cbs.get_mut(target) {
                        reset_breaker(cb);
                        (
                            true,
                            tf(
                                "status.hyper_resilience.component_reinitialized",
                                &[("component", target)],
                            ),
                            true,
                        )
                    } else {
                        (
                            false,
                            tf("error.circuit_breaker_not_found", &[("name", target)]),
                            false,
                        )
                    }
                }
            }
        };

        // Count the action honestly: real in-process state change → executed;
        // simulated-only action (logged) → simulated; failed action → nothing.
        if real_effect {
            self.healing_actions_taken.fetch_add(1, Ordering::Release);
        } else if success {
            self.healing_actions_simulated
                .fetch_add(1, Ordering::Release);
        }

        // Wait for the simulated execution duration so the reported duration
        // is honest (the state-machine updates above are instantaneous).
        tokio::time::sleep(std::time::Duration::from_millis(nominal_duration_ms)).await;

        let completed_ms = crate::shared::timestamps::now_ts_ms_u64();
        let duration_ms = completed_ms.saturating_sub(started_ms);

        let report = HealingReport {
            action,
            target: target.to_string(),
            initiated_ms: started_ms,
            success,
            duration_ms,
            result,
        };

        Ok(report)
    }

    /// Return the current resilience profile summarising overall engine state.
    pub async fn profile(&self) -> ResilienceProfile {
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
        let system_health = degradation_from_counts(open_circuits, total_circuit_breakers, 0);

        let uptime_ms = crate::shared::timestamps::now_ts_ms_u64().saturating_sub(self.started_ms);
        let healing_actions_taken = self.healing_actions_taken.load(Ordering::Acquire);
        let healing_actions_simulated = self.healing_actions_simulated.load(Ordering::Acquire);

        ResilienceProfile {
            level,
            system_health,
            total_circuit_breakers,
            open_circuits,
            failover_groups,
            healing_actions_taken,
            healing_actions_simulated,
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
        let interval_ms = read_lock(&self.config).health_check_interval_ms;

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
        *self.health_check_handle.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("health_check_handle lock poisoned, recovering");
            poisoned.into_inner()
        }) = Some(handle);
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
            let cbs = lock_mutex(&self.circuit_breakers);
            cbs.keys().cloned().collect()
        };
        // Probe concurrently: each `probe` acquires the `circuit_breakers`
        // Mutex only briefly (state transition, no lock held across an await),
        // so parallel probes cannot deadlock or race — previously the loop
        // serialized every probe.
        let probes: Vec<_> = breaker_names.iter().map(|name| self.probe(name)).collect();
        join_all(probes).await;

        // ── Phase 2: Assess system health ──────────────────────────────────
        let health = self.system_health().await;

        // Update real operational metrics (only circuit_breakers + test metrics locks)
        {
            let cbs = lock_mutex(&self.circuit_breakers);
            let total = cbs.len();
            let open = cbs
                .values()
                .filter(|cb| matches!(cb.state, CircuitState::Open))
                .count();
            drop(cbs);

            let mut metrics = lock_mutex(&self.test_metrics);

            if total > 0 {
                metrics.error_rate = open as f64 / total as f64;
            } else {
                metrics.error_rate = 0.0;
            }
            // Real latency: average of the per-service measured latencies (EMA
            // values updated by `record_outcome` / `record_execution`). Lock
            // order mirrors `record_service_outcome` (service_counters →
            // service_health) so the two maps can't deadlock against it.
            // Services with zero recorded requests contribute nothing, and with
            // no data at all the metric stays 0.0 ("no data") instead of the
            // former fabricated 8/15ms estimate.
            let (latency_sum, latency_services) = {
                let counters = lock_mutex(&self.service_counters);
                let health = lock_mutex(&self.service_health);
                health
                    .iter()
                    .fold((0.0_f64, 0_usize), |(sum, n), (name, h)| {
                        let has_requests = counters
                            .get(name)
                            .map(|c| c.total_requests > 0)
                            .unwrap_or(false);
                        if has_requests {
                            (sum + h.avg_latency_ms, n + 1)
                        } else {
                            (sum, n)
                        }
                    })
            };
            metrics.avg_latency_ms = if latency_services > 0 {
                latency_sum / latency_services as f64
            } else {
                0.0
            };
        }

        // ── Phase 3: Auto-heal if degraded ────────────────────────────────
        if health.level >= DegradationLevel::Constrained {
            let healing_enabled = read_lock(&self.config).self_healing_enabled;
            if healing_enabled {
                // Collect the open breakers under a brief lock, then heal them
                // in parallel: each `execute_healing` performs a real TCP probe
                // (up to 3s) plus a simulated-duration sleep, so serializing
                // them would stretch one cycle by N × (3s + sleep). This
                // mirrors the Phase 1 parallel-probe pattern.
                let open_breakers: Vec<String> = {
                    let cbs = lock_mutex(&self.circuit_breakers);
                    breaker_names
                        .iter()
                        .filter(|name| {
                            cbs.get(*name)
                                .map(|cb| matches!(cb.state, CircuitState::Open))
                                .unwrap_or(false)
                        })
                        .cloned()
                        .collect()
                };
                let healing_futures: Vec<_> = open_breakers
                    .iter()
                    .map(|name| self.execute_healing(SelfHealingAction::ClearCircuitBreaker, name))
                    .collect();
                let reports = join_all(healing_futures).await;
                for (name, report) in open_breakers.iter().zip(reports) {
                    match report {
                        Ok(report) => {
                            tracing::info!(
                                "health-check: auto-healed breaker '{}': {}",
                                name,
                                report.result
                            );
                        }
                        Err(e) => {
                            tracing::warn!("health-check: auto-heal failed for '{}': {}", name, e);
                        }
                    }
                }
            }
        }
    }

    /// Record an execution outcome (success or failure) against the named
    /// circuit breaker, auto-registering if it does not exist yet.
    ///
    /// This is the preferred method for production code paths that do not
    /// need explicit registration. For retry / recovery orchestration,
    /// prefer the pair of `is_available()` → `record_failure()` / `record_success()`.
    pub async fn record_execution(&self, breaker_name: &str, success: bool) {
        // Phase 1: Read config for auto-register defaults (read lock, fast path).
        let threshold: u64;
        let recovery_timeout_ms: u64;
        {
            let config = read_lock(&self.config);
            threshold = config.circuit_breaker_threshold;
            recovery_timeout_ms = config.recovery_timeout_ms;
        }

        // Phase 2: Lock only circuit_breakers for the auto-register + state transition.
        // The block scopes the MutexGuard so it is dropped before any await.
        {
            let mut cbs = lock_mutex(&self.circuit_breakers);

            let cb_ref = cbs.entry(breaker_name.to_string()).or_insert_with(|| {
                CircuitBreaker::new(breaker_name.to_string(), threshold, recovery_timeout_ms)
            });

            let now = crate::shared::timestamps::now_ts_ms_u64();

            // No failure mode is recorded on this path: `record_execution`
            // callers only supply a success boolean, so any specific mode
            // would be fabricated. Callers with a real cause use
            // `record_failure_with_mode` (e.g. fallback.rs records
            // ResourceExhaustion explicitly).

            transition_breaker(
                cb_ref,
                if success {
                    BreakerOutcome::Success
                } else {
                    BreakerOutcome::Failure
                },
                now,
            );
        }

        // Record the same outcome into the per-service health monitor so
        // breaker state and `health_report()` / `degradation_level()` never
        // drift (previously record_execution updated only the breakers, while
        // record_outcome updated both). No latency is measured here, so the
        // previous average latency is kept.
        self.record_service_outcome(breaker_name, success, None);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Consolidated test metrics (latency + error rate under one Mutex).
#[derive(Debug, Clone)]
struct TestMetrics {
    /// Average measured latency in milliseconds (0.0 = no data yet; populated
    /// by `health_check_cycle` from per-service measured latencies).
    avg_latency_ms: f64,
    /// Error rate derived from real circuit breaker states (0.0 – 1.0).
    error_rate: f64,
}
