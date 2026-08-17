//! Shared enums and structs for the hyper-resilience engine.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Resilience hardening level for a component or profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResilienceLevel {
    Standard,
    High,
    Critical,
}

/// Failure mode classification used by the engine to categorise events.
/// Production currently constructs only `ResourceExhaustion` (fallback.rs and
/// the `record_failure` default); the enum is the diagnostic vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureMode {
    ResourceExhaustion,
}

/// Circuit breaker state (single source of truth — previously defined in
/// `optimization::failure_prevention`, moved here so the hyper-resilience
/// engine owns the unified breaker state machine).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CircuitBreakerState {
    Closed,
    Open,
    HalfOpen,
}

/// Short alias used throughout the engine.
pub use self::CircuitBreakerState as CircuitState;

/// Health status of a monitored service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Per-service health snapshot (success/error rates, latency, status).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHealth {
    pub service_name: String,
    pub status: HealthStatus,
    pub success_rate: f64,
    pub error_rate: f64,
    pub avg_latency_ms: f64,
    pub last_check_timestamp: u64,
}

/// Per-service outcome counters driving the health monitor.
#[derive(Debug, Clone, Default)]
pub(crate) struct ServiceCounters {
    pub(crate) total_requests: u64,
    pub(crate) successful_requests: u64,
    pub(crate) consecutive_failures: u64,
}

/// Error-rate threshold above which a service is classified Unhealthy.
pub(crate) const HEALTH_ERROR_RATE_THRESHOLD: f64 = 0.1;
/// Success-rate threshold below which a service is classified Degraded.
pub(crate) const HEALTH_SUCCESS_RATE_THRESHOLD: f64 = 0.8;

/// Outcome of a single execution applied to a circuit breaker state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BreakerOutcome {
    Success,
    Failure,
}

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
    /// Timestamp (ms since epoch) of the last state transition.
    pub last_state_change_ms: u64,
    /// The failure mode of the most recent failure.
    pub last_failure_mode: Option<FailureMode>,
    /// Rolling history of recent failure modes (most recent first, max 10).
    pub failure_history: Vec<FailureMode>,
}

impl CircuitBreaker {
    /// Create a fresh closed breaker with the given threshold and recovery
    /// timeout. Single construction point — new fields need only one edit.
    pub(crate) fn new(name: String, threshold: u64, recovery_timeout_ms: u64) -> Self {
        Self {
            name,
            state: CircuitState::Closed,
            failure_count: 0,
            threshold,
            recovery_timeout_ms,
            last_failure_ms: 0,
            last_state_change_ms: 0,
            last_failure_mode: None,
            failure_history: Vec::new(),
        }
    }
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
    #[serde(default)]
    pub circuit_breaker_threshold: u64,
    #[serde(default)]
    pub recovery_timeout_ms: u64,
    #[serde(default)]
    pub health_check_interval_ms: u64,
    #[serde(default)]
    pub self_healing_enabled: bool,
}

impl Default for ResilienceConfig {
    fn default() -> Self {
        Self {
            circuit_breaker_threshold: 5,
            recovery_timeout_ms: 30_000,
            health_check_interval_ms: 5_000,
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
    /// Self-healing actions that produced a **real** in-process state change
    /// (breaker reset, leader promotion, component reinit).
    pub healing_actions_taken: u64,
    /// Self-healing actions that are infrastructure-level and cannot be
    /// executed inside this process (node restart, resource scaling) — they
    /// are simulated (logged only) and counted separately so metrics never
    /// conflate simulation with execution.
    #[serde(default)]
    pub healing_actions_simulated: u64,
    pub uptime_ms: u64,
}
