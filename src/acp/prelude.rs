//! ACP Prelude - Type definitions and constants
//!
//! This module contains type definitions, constants, and basic structures
//! used throughout the ACP system. It serves as the foundation for the
//! modular ACP implementation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

use crate::agent::Message;
use crate::config::PhaseOptions;
use crate::reinforcement::{RequirementContractArtifact, TaskPlanArtifact};
use crate::roles::AgentRole;

// Re-export commonly used types
// Message type is already available through crate::agent::Message

// ============================================================================
// Constants
// ============================================================================

/// Default circuit breaker failure threshold
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub const DEFAULT_BREAKER_FAILURE_THRESHOLD: u32 = 3;
/// Default circuit breaker open time in seconds
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub const DEFAULT_BREAKER_OPEN_SECONDS: i64 = 60;
/// Maximum conversation ID length
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub const MAX_CONVERSATION_ID_LEN: usize = 128;
/// Maximum branch ID length
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub const MAX_BRANCH_ID_LEN: usize = 64;
/// Maximum checkpoint ID length
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub const MAX_CHECKPOINT_ID_LEN: usize = 128;
/// Maximum checkpoints per conversation
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub const MAX_CHECKPOINTS_PER_CONVERSATION: usize = 256;
/// Maximum checkpoint message characters
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub const MAX_CHECKPOINT_MESSAGE_CHARS: usize = 64_000;
/// Maximum conversations tracked
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub const MAX_CONVERSATIONS_TRACKED: usize = 512;
/// Maximum stream chunks
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub const MAX_STREAM_CHUNKS: usize = 4_096;
/// Maximum stream characters
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub const MAX_STREAM_CHARS: usize = 256_000;

pub const ACP_LOCK_RUNTIME_CONFIG: &str = "runtime_config";
pub const ACP_LOCK_MEMORY_CACHE: &str = "memory_cache";
pub const ACP_LOCK_MEMORY_STORE: &str = "memory_store";
pub const ACP_LOCK_RESPONSE_CACHE: &str = "response_cache";
pub const ACP_LOCK_VECTOR_STORE: &str = "vector_store";
pub const ACP_LOCK_MAINTENANCE: &str = "maintenance_tracker";
pub const ACP_LOCK_LIFECYCLE: &str = "lifecycle_state";
pub const ACP_LOCK_CIRCUIT_BREAKERS: &str = "circuit_breakers";
pub const ACP_LOCK_PHASE_RATE_LIMITER: &str = "phase_rate_limiter";
pub const ACP_LOCK_INFLIGHT_LIMITER: &str = "inflight_limiter";

const ACP_LOCK_SLOW_WAIT_THRESHOLD: Duration = Duration::from_millis(5);

/// Histogram buckets for latency measurements (seconds)
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub const HISTOGRAM_BUCKETS_SECONDS: [f64; 10] = [
    0.001, // 1ms
    0.005, // 5ms
    0.01,  // 10ms
    0.05,  // 50ms
    0.1,   // 100ms
    0.5,   // 500ms
    1.0,   // 1s
    5.0,   // 5s
    10.0,  // 10s
    60.0,  // 60s
];

// ============================================================================
// Type Definitions
// ============================================================================

/// Conversation checkpoint structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationCheckpoint {
    /// Unique checkpoint ID
    pub checkpoint_id: String,
    /// Conversation ID
    pub conversation_id: String,
    /// Branch ID
    pub branch_id: String,
    /// Parent checkpoint ID (for branching)
    pub parent_checkpoint_id: Option<String>,
    /// Creation timestamp
    pub created_at: i64,
    /// Optional note
    pub note: Option<String>,
    /// Persisted meta-cognition state for save/restore continuity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metacognitive_loop: Option<Value>,
    /// Messages in this checkpoint
    pub messages: Vec<Message>,
}

/// Conversation state structure
#[derive(Debug, Clone, Default)]
pub struct ConversationState {
    /// Checkpoints in this conversation
    pub checkpoints: Vec<ConversationCheckpoint>,
    /// Branch heads mapping
    pub branch_heads: HashMap<String, String>,
    /// Last touched timestamp
    pub last_touched_at: i64,
}

/// Conversation prune result
#[derive(Debug, Clone, Serialize, Default)]
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub struct ConversationPruneResult {
    /// Number of conversations removed
    pub removed: usize,
    /// Number of branch heads repaired
    pub repaired_heads: usize,
}

#[derive(Debug, Default)]
struct AcpLockCounters {
    acquisitions: AtomicU64,
    poisoned_total: AtomicU64,
    recovered_total: AtomicU64,
    slow_wait_total: AtomicU64,
    total_wait_nanos: AtomicU64,
    max_wait_nanos: AtomicU64,
}

impl AcpLockCounters {
    fn record_wait(&self, wait: Duration) {
        let wait_nanos = wait.as_nanos().min(u64::MAX as u128) as u64;
        self.acquisitions.fetch_add(1, Ordering::Relaxed);
        self.total_wait_nanos
            .fetch_add(wait_nanos, Ordering::Relaxed);
        if wait >= ACP_LOCK_SLOW_WAIT_THRESHOLD {
            self.slow_wait_total.fetch_add(1, Ordering::Relaxed);
        }

        let mut current = self.max_wait_nanos.load(Ordering::Relaxed);
        while wait_nanos > current {
            match self.max_wait_nanos.compare_exchange(
                current,
                wait_nanos,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }

    fn record_poison(&self) {
        self.poisoned_total.fetch_add(1, Ordering::Relaxed);
    }

    fn record_recovery(&self) {
        self.recovered_total.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self, name: &'static str) -> AcpLockSnapshot {
        let acquisitions = self.acquisitions.load(Ordering::Relaxed);
        let total_wait_nanos = self.total_wait_nanos.load(Ordering::Relaxed);
        let max_wait_nanos = self.max_wait_nanos.load(Ordering::Relaxed);
        let avg_wait_ms = if acquisitions > 0 {
            total_wait_nanos as f64 / acquisitions as f64 / 1_000_000.0
        } else {
            0.0
        };

        AcpLockSnapshot {
            name: name.to_string(),
            acquisitions,
            poisoned_total: self.poisoned_total.load(Ordering::Relaxed),
            recovered_total: self.recovered_total.load(Ordering::Relaxed),
            slow_wait_total: self.slow_wait_total.load(Ordering::Relaxed),
            avg_wait_ms,
            max_wait_ms: max_wait_nanos as f64 / 1_000_000.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct AcpLockSnapshot {
    pub name: String,
    pub acquisitions: u64,
    pub poisoned_total: u64,
    pub recovered_total: u64,
    pub slow_wait_total: u64,
    pub avg_wait_ms: f64,
    pub max_wait_ms: f64,
}

#[derive(Debug, Default)]
pub struct AcpLockMonitor {
    runtime_config: AcpLockCounters,
    memory_cache: AcpLockCounters,
    memory_store: AcpLockCounters,
    response_cache: AcpLockCounters,
    vector_store: AcpLockCounters,
    maintenance: AcpLockCounters,
    lifecycle: AcpLockCounters,
    circuit_breakers: AcpLockCounters,
    phase_rate_limiter: AcpLockCounters,
    inflight_limiter: AcpLockCounters,
}

impl AcpLockMonitor {
    fn counters(&self, name: &'static str) -> &AcpLockCounters {
        match name {
            ACP_LOCK_RUNTIME_CONFIG => &self.runtime_config,
            ACP_LOCK_MEMORY_CACHE => &self.memory_cache,
            ACP_LOCK_MEMORY_STORE => &self.memory_store,
            ACP_LOCK_RESPONSE_CACHE => &self.response_cache,
            ACP_LOCK_VECTOR_STORE => &self.vector_store,
            ACP_LOCK_MAINTENANCE => &self.maintenance,
            ACP_LOCK_LIFECYCLE => &self.lifecycle,
            ACP_LOCK_CIRCUIT_BREAKERS => &self.circuit_breakers,
            ACP_LOCK_PHASE_RATE_LIMITER => &self.phase_rate_limiter,
            ACP_LOCK_INFLIGHT_LIMITER => &self.inflight_limiter,
            _ => {
                warn!("Unknown ACP lock monitor component: {name}, using fallback mutex");
                static FALLBACK: AcpLockCounters = AcpLockCounters {
                    acquisitions: AtomicU64::new(0),
                    poisoned_total: AtomicU64::new(0),
                    recovered_total: AtomicU64::new(0),
                    slow_wait_total: AtomicU64::new(0),
                    total_wait_nanos: AtomicU64::new(0),
                    max_wait_nanos: AtomicU64::new(0),
                };
                &FALLBACK
            }
        }
    }

    pub fn snapshot(&self) -> Vec<AcpLockSnapshot> {
        [
            ACP_LOCK_RUNTIME_CONFIG,
            ACP_LOCK_MEMORY_CACHE,
            ACP_LOCK_MEMORY_STORE,
            ACP_LOCK_RESPONSE_CACHE,
            ACP_LOCK_VECTOR_STORE,
            ACP_LOCK_MAINTENANCE,
            ACP_LOCK_LIFECYCLE,
            ACP_LOCK_CIRCUIT_BREAKERS,
            ACP_LOCK_PHASE_RATE_LIMITER,
            ACP_LOCK_INFLIGHT_LIMITER,
        ]
        .into_iter()
        .map(|name| self.counters(name).snapshot(name))
        .collect()
    }

    fn record_wait(&self, name: &'static str, wait: Duration) {
        self.counters(name).record_wait(wait);
    }

    fn record_poison(&self, name: &'static str) {
        self.counters(name).record_poison();
    }

    fn record_recovery(&self, name: &'static str) {
        self.counters(name).record_recovery();
    }
}

pub fn with_acp_lock<T, R, F>(
    monitor: &AcpLockMonitor,
    name: &'static str,
    mutex: &StdMutex<T>,
    operation: F,
) -> R
where
    F: FnOnce(&mut T) -> R,
{
    let wait_started = Instant::now();
    match mutex.lock() {
        Ok(mut guard) => {
            monitor.record_wait(name, wait_started.elapsed());
            operation(&mut guard)
        }
        Err(poisoned) => {
            monitor.record_wait(name, wait_started.elapsed());
            monitor.record_poison(name);
            monitor.record_recovery(name);
            warn!(
                target: "acp::locks",
                "ACP lock '{}' was poisoned; continuing with recovered state",
                name
            );
            let mut guard = poisoned.into_inner();
            operation(&mut guard)
        }
    }
}

/// Metrics snapshot
#[derive(Debug, Clone, Serialize, Default)]
pub struct MetricsSnapshot {
    /// Total requests processed
    pub total_requests: u64,
    /// Successful requests
    pub successful_requests: u64,
    /// Failed requests
    pub failed_requests: u64,
    /// Average request duration in milliseconds
    pub avg_request_duration_ms: f64,
    /// Cumulative request duration in milliseconds
    pub request_latency_sum_ms: f64,
    /// Request latency histogram bucket counts (ms buckets +Inf)
    pub request_latency_bucket_counts: [u64; 10],
    /// Current active requests
    pub active_requests: u32,
    /// Cache hit rate (0.0 to 1.0)
    pub cache_hit_rate: f64,
    /// Circuit breaker open count
    pub circuit_breaker_open_count: u32,
    /// Memory usage in bytes
    pub memory_usage_bytes: u64,
    /// CPU usage percentage (0.0 to 100.0)
    pub cpu_usage_percent: f64,
    /// Total chat requests
    pub chat_requests_total: u64,
    /// Agent request timeout count across chat / execution paths
    pub agent_timeout_failures_total: u64,
    /// Local runtime probe timeout count for agent readiness checks
    pub runtime_probe_timeout_total: u64,
    /// Vector search requests executed
    pub vector_search_total: u64,
    /// Vector hits returned across searches
    pub vector_hit_total: u64,
    /// Vector entries stored
    pub vector_store_total: u64,
    /// Summary lookups executed
    pub summary_read_total: u64,
    /// Summary cache hits
    pub summary_hit_total: u64,
    /// Summary entries stored
    pub summary_store_total: u64,
    /// Cumulative chat duration in milliseconds
    pub chat_latency_sum_ms: f64,
    /// Chat latency histogram bucket counts (ms buckets +Inf)
    pub chat_latency_bucket_counts: [u64; 10],
    /// Review gate invocations
    pub review_gate_total: u64,
    /// Cumulative review-gate duration in milliseconds
    pub review_latency_sum_ms: f64,
    /// Review latency histogram bucket counts (ms buckets +Inf)
    pub review_latency_bucket_counts: [u64; 10],
    /// Review gate approved count
    pub review_gate_approved_total: u64,
    /// Review gate rejected count
    pub review_gate_rejected_total: u64,
    /// Review gate timeout count
    pub review_gate_timeout_total: u64,
    /// Review gate degraded count
    pub review_gate_degraded_total: u64,
    /// Review gate invalid response count
    pub review_gate_invalid_response_total: u64,
}

/// Circuit breaker snapshot
#[derive(Debug, Clone, Serialize, Default)]
pub struct CircuitBreakerSnapshot {
    /// Circuit breaker name
    pub name: String,
    /// Current state (closed, open, half-open)
    pub state: String,
    /// Failure count
    pub failure_count: u32,
    /// Success count
    pub success_count: u32,
    /// Last state change timestamp
    pub last_state_change: i64,
    /// Total failures
    pub total_failures: u64,
    /// Total successes
    pub total_successes: u64,
}

/// Lifecycle snapshot
#[derive(Debug, Clone, Serialize, Default)]
pub struct LifecycleSnapshot {
    /// Server start time
    pub start_time: i64,
    /// Uptime in seconds
    pub uptime_seconds: i64,
    /// Total requests processed
    pub total_requests: u64,
    /// Current phase
    pub current_phase: String,
    /// Is healthy
    pub is_healthy: bool,
    /// Health check timestamp
    pub last_health_check: i64,
    /// Shutdown requested
    pub shutdown_requested: bool,
}

/// Maintenance snapshot
#[derive(Debug, Clone, Serialize, Default)]
pub struct MaintenanceSnapshot {
    /// Whether maintenance is running
    #[allow(dead_code)] // F-GAP-49 — planned wiring
    pub running: bool,
    /// Total maintenance cycles completed
    pub cycles_total: u64,
    /// Last maintenance started timestamp
    pub last_started_at: Option<i64>,
    /// Last maintenance completed timestamp
    pub last_completed_at: Option<i64>,
    /// Last memory expired entries removed
    pub last_memory_expired_removed: u64,
    /// Last SQLite expired entries removed
    pub last_sqlite_expired_removed: u64,
    /// Whether last cycle vacuumed cache
    pub last_cache_vacuumed: bool,
    /// Whether last cycle vacuumed vector store
    pub last_vector_vacuumed: bool,
    /// Last error message if any
    pub last_error: Option<String>,
    /// Last maintenance timestamp (legacy)
    pub last_maintenance: i64,
    /// Maintenance interval in seconds (legacy)
    pub maintenance_interval: i64,
    /// Next maintenance due timestamp (legacy)
    pub next_maintenance_due: i64,
    /// Maintenance tasks completed (legacy)
    pub tasks_completed: u32,
    /// Maintenance tasks failed (legacy)
    pub tasks_failed: u32,
    /// Whether maintenance is in progress (legacy)
    pub maintenance_in_progress: bool,
}

/// Server status structure
#[derive(Debug, Clone, Serialize)]
pub struct ServerStatus {
    /// Metrics snapshot
    pub metrics: MetricsSnapshot,
    /// Circuit breaker snapshots
    pub circuit_breakers: Vec<CircuitBreakerSnapshot>,
    /// Lifecycle snapshot
    pub lifecycle: LifecycleSnapshot,
    /// Maintenance snapshot
    pub maintenance: MaintenanceSnapshot,
    /// Timestamp of this status
    pub timestamp: i64,
}

/// Re-export of `ReviewTimeoutPolicy` from the agent implementation module.
/// The canonical definition lives in `crate::acp::impl::agent`.
pub use crate::acp::r#impl::agent::ReviewTimeoutPolicy;

/// Re-export of `ReviewGateOutcome` from the agent implementation module.
/// The canonical definition lives in `crate::acp::impl::agent`.
pub use crate::acp::r#impl::agent::ReviewGateOutcome;

/// Review decision
#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub struct ReviewDecision {
    /// Reviewer name
    pub reviewer: String,
    /// Verdict (pass/fail/invalid)
    pub verdict: String,
    /// Review response
    pub response: String,
}

/// Review verdict enum
///
/// This public enum uses `Pass`/`Fail`/`Invalid` semantics.
/// There is a separate governance-internal `ReviewVerdict` in
/// `crate::governance::review_controls` that uses `Approve`/`Reject`/`Invalid`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub enum ReviewVerdict {
    /// Review passed
    Pass,
    /// Review failed
    Fail,
    /// Invalid review response
    Invalid,
}

impl ReviewVerdict {
    /// Convert to string
    #[allow(dead_code)] // F-GAP-49 — planned wiring
    pub fn as_str(&self) -> &'static str {
        match self {
            ReviewVerdict::Pass => "pass",
            ReviewVerdict::Fail => "fail",
            ReviewVerdict::Invalid => "invalid",
        }
    }
}

/// Chat parameters structure
#[derive(Debug, Clone, Deserialize, Serialize)]
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub struct ChatParams {
    /// Chat mode (e.g., "ask", "edit", "agent", "safeguard", "full_auto")
    pub mode: String,
    /// Messages to process
    pub messages: Vec<Message>,
    /// Phase options
    pub phase_options: Option<PhaseOptions>,
    /// Requirement contract
    pub requirement_contract: Option<RequirementContractArtifact>,
    /// Task plan
    pub plan: Option<TaskPlanArtifact>,
    /// Additional parameters
    pub extras: Option<Value>,
}

/// Task characteristics
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub struct TaskCharacteristics {
    /// Task complexity (simple, medium, complex)
    pub complexity: String,
    /// Estimated duration in seconds
    pub estimated_duration_seconds: u32,
    /// Required expertise level
    pub required_expertise: String,
    /// Risk level (low, medium, high)
    pub risk_level: String,
    /// Whether parallel execution is possible
    pub can_parallelize: bool,
    /// Required safeguards
    pub required_safeguards: Vec<String>,
}

/// Routing decision
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub struct RoutingDecision {
    /// Selected roles in execution order
    pub roles: Vec<AgentRole>,
    /// Detailed requirements for each role
    pub requirements: Vec<crate::orchestration::task_router::RoleRequirement>,
    /// Estimated probability of success with selected roles
    pub predicted_success_rate: f32,
    /// Estimated total execution time in seconds
    pub estimated_duration_seconds: u32,
    /// Whether parallel execution is recommended for any roles
    pub can_parallelize: Vec<(AgentRole, AgentRole)>,
    /// Key risk factors identified
    pub risk_factors: Vec<String>,
    /// Recommended safeguards
    pub recommended_safeguards: Vec<String>,
    /// PUA enforcement plan that must be honored downstream
    pub pua_enforcement: crate::pua::PuaEnforcementPlan,
}

// RequirementContractArtifact is imported from crate::reinforcement

// TaskPlanArtifact is imported from crate::reinforcement

// These types are imported from crate::reinforcement:
// - ExecutionDecisionCandidate
// - CheckpointSummaryArtifact

// ============================================================================
// Utility Functions
// ============================================================================

/// Get current timestamp in seconds
pub fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Get current timestamp in milliseconds
pub fn now_ts_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Calculate checkpoint message characters
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub fn checkpoint_message_chars(messages: &[Message]) -> usize {
    messages.iter().map(|m| m.content.chars().count()).sum()
}

/// Touch conversation order (update LRU)
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub fn touch_conversation_order(order: &StdMutex<Vec<String>>, conversation_id: &str) {
    let mut guard = order.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("conversation order lock poisoned, recovering");
        poisoned.into_inner()
    });
    // Remove if exists
    guard.retain(|id| id != conversation_id);
    // Add to front (most recent)
    guard.insert(0, conversation_id.to_string());
    // Trim if too long
    if guard.len() > MAX_CONVERSATIONS_TRACKED {
        guard.truncate(MAX_CONVERSATIONS_TRACKED);
    }
}

/// Enforce checkpoint capacity
pub fn enforce_checkpoint_capacity(
    state: &mut ConversationState,
    incoming: usize,
    rollback_target: Option<&str>,
) {
    let total_after_insert = state.checkpoints.len().saturating_add(incoming);
    if total_after_insert <= MAX_CHECKPOINTS_PER_CONVERSATION {
        return;
    }

    let mut overflow = total_after_insert - MAX_CHECKPOINTS_PER_CONVERSATION;
    let mut cursor = 0usize;

    // Prefer removing oldest checkpoints, but keep the rollback target when requested.
    while overflow > 0 && cursor < state.checkpoints.len() {
        let checkpoint = &state.checkpoints[cursor];
        if rollback_target.is_some_and(|target| checkpoint.checkpoint_id == target) {
            cursor += 1;
            continue;
        }

        // Remove this checkpoint
        state.checkpoints.remove(cursor);
        overflow -= 1;
        // Don't increment cursor because we removed the element at this position
    }
}

/// Evict oldest conversation
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub fn evict_oldest_conversation(
    store: &mut HashMap<String, ConversationState>,
    order: &StdMutex<Vec<String>>,
) -> Option<String> {
    let mut order_guard = match order.lock() {
        Ok(guard) => guard,
        Err(_) => return None,
    };

    while let Some(oldest_id) = order_guard.pop() {
        if store.contains_key(&oldest_id) {
            store.remove(&oldest_id);
            return Some(oldest_id);
        }
    }

    None
}

// ============================================================================
// Additional Types from Original Prelude
// ============================================================================

/// Circuit breaker registry for managing circuit breakers
#[derive(Debug, Default)]
pub struct CircuitBreakerRegistry {
    inner: StdMutex<HashMap<String, CircuitBreakerState>>,
}

/// Circuit breaker state
#[derive(Debug, Clone)]
struct CircuitBreakerState {
    stage: CircuitBreakerStage,
    failure_count: u32,
    success_count: u32,
    last_state_change: i64,
    open_until: Option<i64>,
}

/// Circuit breaker stage
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[allow(dead_code)] // F-GAP-49 — planned wiring
enum CircuitBreakerStage {
    #[default]
    Closed,
    Open,
    HalfOpen,
}

impl Default for CircuitBreakerState {
    fn default() -> Self {
        Self {
            stage: CircuitBreakerStage::Closed,
            failure_count: 0,
            success_count: 0,
            last_state_change: 0,
            open_until: None,
        }
    }
}

/// Circuit breaker admission result
#[non_exhaustive]
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub enum CircuitBreakerAdmission {
    Closed,
    Rejected {
        state: &'static str,
        retry_after_seconds: Option<i64>,
    },
}

impl CircuitBreakerRegistry {
    /// Create a new circuit breaker registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the number of open circuit breakers
    pub fn open_count(&self) -> u32 {
        let guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard
            .values()
            .filter(|state| matches!(state.stage, CircuitBreakerStage::Open))
            .count() as u32
    }

    /// Get circuit breaker snapshots
    pub fn snapshots(&self) -> Vec<CircuitBreakerSnapshot> {
        let guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard
            .iter()
            .map(|(name, state)| CircuitBreakerSnapshot {
                name: name.clone(),
                state: match state.stage {
                    CircuitBreakerStage::Closed => "closed".to_string(),
                    CircuitBreakerStage::Open => "open".to_string(),
                    CircuitBreakerStage::HalfOpen => "half-open".to_string(),
                },
                failure_count: state.failure_count,
                success_count: state.success_count,
                last_state_change: state.last_state_change,
                total_failures: state.failure_count as u64,
                total_successes: state.success_count as u64,
            })
            .collect()
    }

    /// Reset one circuit breaker or all tracked breakers back to closed state.
    pub fn reset(&self, name: Option<&str>) -> usize {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });

        let reset_state = |state: &mut CircuitBreakerState| {
            state.stage = CircuitBreakerStage::Closed;
            state.failure_count = 0;
            state.success_count = 0;
            state.last_state_change = now_ts();
            state.open_until = None;
        };

        if let Some(name) = name {
            if let Some(state) = guard.get_mut(name) {
                reset_state(state);
                return 1;
            }
            return 0;
        }

        let count = guard.len();
        for state in guard.values_mut() {
            reset_state(state);
        }
        count
    }

    /// Check if circuit breakers are healthy
    pub fn is_healthy(&self) -> bool {
        self.open_count() == 0
    }
}

/// Lifecycle state for server lifecycle management
#[derive(Debug)]
pub struct LifecycleState {
    healthy: bool,
    shutdown_requested: bool,
    start_time: i64,
    total_requests: u64,
    current_phase: String,
    last_health_check: i64,
}

impl Default for LifecycleState {
    fn default() -> Self {
        Self::new()
    }
}

impl LifecycleState {
    /// Create a new lifecycle state
    pub fn new() -> Self {
        Self {
            healthy: true,
            shutdown_requested: false,
            start_time: now_ts(),
            total_requests: 0,
            current_phase: "running".to_string(),
            last_health_check: now_ts(),
        }
    }

    /// Check if server is healthy
    pub fn is_healthy(&self) -> bool {
        self.healthy
    }

    /// Mark server as healthy
    pub fn mark_healthy(&mut self) {
        self.healthy = true;
    }

    /// Mark server as unhealthy
    pub fn mark_unhealthy(&mut self) {
        self.healthy = false;
    }

    /// Check if shutdown has been requested
    pub fn shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }

    /// Begin shutdown
    pub fn begin_shutdown(&mut self) {
        self.shutdown_requested = true;
    }

    /// Check if server is shutting down
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown_requested
    }

    /// Get a snapshot of the lifecycle state
    pub fn snapshot(&self) -> LifecycleSnapshot {
        let now = now_ts();
        LifecycleSnapshot {
            start_time: self.start_time,
            uptime_seconds: now.saturating_sub(self.start_time),
            total_requests: self.total_requests,
            current_phase: self.current_phase.clone(),
            is_healthy: self.healthy,
            last_health_check: self.last_health_check,
            shutdown_requested: self.shutdown_requested,
        }
    }

    /// Increment total requests counter
    pub fn increment_requests(&mut self) {
        self.total_requests = self.total_requests.saturating_add(1);
    }

    /// Update current phase
    pub fn update_phase(&mut self, phase: &str) {
        self.current_phase = phase.to_string();
    }

    /// Update health check timestamp
    pub fn update_health_check(&mut self) {
        self.last_health_check = now_ts();
    }
}

/// Maintenance tracker for system maintenance
#[derive(Debug)]
pub struct MaintenanceTracker {
    inner: StdMutex<MaintenanceSnapshot>,
}

impl Default for MaintenanceTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl MaintenanceTracker {
    /// Create a new maintenance tracker
    pub fn new() -> Self {
        let now = now_ts();
        Self {
            inner: StdMutex::new(MaintenanceSnapshot {
                running: false,
                cycles_total: 0,
                last_started_at: None,
                last_completed_at: None,
                last_memory_expired_removed: 0,
                last_sqlite_expired_removed: 0,
                last_cache_vacuumed: false,
                last_vector_vacuumed: false,
                last_error: None,
                last_maintenance: now,
                maintenance_interval: 3600, // 1 hour default
                next_maintenance_due: now + 3600,
                tasks_completed: 0,
                tasks_failed: 0,
                maintenance_in_progress: false,
            }),
        }
    }

    /// Get a snapshot of the maintenance state
    pub fn snapshot(&self) -> MaintenanceSnapshot {
        self.inner
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Begin maintenance
    pub fn begin_maintenance(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.maintenance_in_progress = true;
    }

    /// Note that maintenance has started
    pub fn note_started(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.running = true;
        guard.last_started_at = Some(now_ts());
        guard.last_error = None;
    }

    /// End maintenance
    pub fn end_maintenance(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.maintenance_in_progress = false;
        guard.last_maintenance = now_ts();
        guard.next_maintenance_due = guard.last_maintenance + guard.maintenance_interval;
    }

    /// Note that maintenance has failed
    pub fn note_failed(&self, error: &str) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.last_error = Some(error.to_string());
    }

    /// Record maintenance cycle completion
    pub fn note_completed(
        &self,
        memory_removed: usize,
        sqlite_removed: usize,
        cache_vacuumed: bool,
        vector_vacuumed: bool,
    ) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.running = false;
        guard.last_completed_at = Some(now_ts());
        guard.last_memory_expired_removed = memory_removed as u64;
        guard.last_sqlite_expired_removed = sqlite_removed as u64;
        guard.last_cache_vacuumed = cache_vacuumed;
        guard.last_vector_vacuumed = vector_vacuumed;
        guard.last_error = None;
        guard.cycles_total += 1;
    }

    /// Record health check result
    pub fn record_health_check(&self, healthy: bool) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        if healthy {
            guard.tasks_completed += 1;
        } else {
            guard.tasks_failed += 1;
        }
    }
}

/// Online controller state - real implementation from governance module
pub use crate::governance::runtime_controls::OnlineControllerState;

/// Phase rate limiter for phase-level throttling
#[derive(Debug, Default)]
pub struct PhaseRateLimiter {
    inner: StdMutex<HashMap<String, TokenBucketState>>,
}

#[derive(Debug, Clone)]
struct TokenBucketState {
    tokens: f64,
    capacity: f64,
    refill_per_second: f64,
    last_refill_ms: i64,
}

impl TokenBucketState {
    fn new(capacity: f64, refill_per_second: f64, now_ms: i64) -> Self {
        Self {
            tokens: capacity,
            capacity,
            refill_per_second,
            last_refill_ms: now_ms,
        }
    }

    fn refill(&mut self, now_ms: i64) {
        let elapsed_ms = (now_ms - self.last_refill_ms).max(0) as f64;
        if elapsed_ms > 0.0 {
            let refill = elapsed_ms / 1000.0 * self.refill_per_second;
            self.tokens = (self.tokens + refill).min(self.capacity);
            self.last_refill_ms = now_ms;
        }
    }
}

impl PhaseRateLimiter {
    /// Check if request can pass phase token bucket limiter.
    pub fn allow(&self, phase_name: &str, rpm_limit: u64, burst_capacity: Option<u64>) -> bool {
        if rpm_limit == 0 {
            return false;
        }

        let now = now_ts_ms();
        let refill_per_second = rpm_limit as f64 / 60.0;
        let capacity = burst_capacity.unwrap_or(rpm_limit).max(1) as f64;

        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        let state = guard
            .entry(phase_name.to_string())
            .or_insert_with(|| TokenBucketState::new(capacity, refill_per_second, now));

        if (state.capacity - capacity).abs() > f64::EPSILON
            || (state.refill_per_second - refill_per_second).abs() > f64::EPSILON
        {
            *state = TokenBucketState::new(capacity, refill_per_second, now);
        }

        state.refill(now);
        if state.tokens < 1.0 {
            return false;
        }
        state.tokens -= 1.0;
        true
    }

    pub fn tracked_phases(&self) -> usize {
        self.inner.lock().map(|guard| guard.len()).unwrap_or(0)
    }

    pub fn snapshot(&self) -> HashMap<String, (f64, f64)> {
        self.inner
            .lock()
            .map(|guard| {
                guard
                    .iter()
                    .map(|(phase, state)| (phase.clone(), (state.tokens, state.capacity)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Check if rate limiter is healthy
    pub fn is_healthy(&self) -> bool {
        true
    }
}

/// Inflight limiter for request concurrency control
#[derive(Debug, Default)]
pub struct InflightLimiter {
    inner: StdMutex<InflightState>,
}

#[derive(Debug, Default)]
struct InflightState {
    global: usize,
    phase: HashMap<String, usize>,
}

pub struct InflightGuard {
    limiter: Arc<InflightLimiter>,
    phase_name: String,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.limiter.leave(&self.phase_name);
    }
}

impl InflightLimiter {
    /// Create a new inflight limiter
    pub fn new(_max_inflight: u32) -> Self {
        Self::default()
    }

    pub fn try_enter(
        self: &Arc<Self>,
        phase_name: &str,
        phase_limit: Option<u64>,
        global_limit: Option<u64>,
    ) -> Option<InflightGuard> {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        if let Some(limit) = global_limit {
            if guard.global as u64 >= limit.max(1) {
                return None;
            }
        }

        let phase_count = guard.phase.get(phase_name).copied().unwrap_or(0);
        if let Some(limit) = phase_limit {
            if phase_count as u64 >= limit.max(1) {
                return None;
            }
        }

        guard.global += 1;
        *guard.phase.entry(phase_name.to_string()).or_insert(0) += 1;
        Some(InflightGuard {
            limiter: Arc::clone(self),
            phase_name: phase_name.to_string(),
        })
    }

    fn leave(&self, phase_name: &str) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.global = guard.global.saturating_sub(1);
        if let Some(value) = guard.phase.get_mut(phase_name) {
            *value = value.saturating_sub(1);
            if *value == 0 {
                guard.phase.remove(phase_name);
            }
        }
    }

    pub fn snapshot(&self) -> (usize, HashMap<String, usize>) {
        self.inner
            .lock()
            .map(|guard| (guard.global, guard.phase.clone()))
            .unwrap_or_default()
    }

    /// Check if inflight limiter is healthy
    pub fn is_healthy(&self) -> bool {
        true
    }
}

impl Default for InflightGuard {
    fn default() -> Self {
        Self {
            limiter: Arc::new(InflightLimiter::default()),
            phase_name: String::new(),
        }
    }
}

/// Runtime metrics for tracking server performance
#[derive(Debug)]
pub struct RuntimeMetrics {
    inner: StdMutex<MetricsSnapshot>,
}

const METRIC_LATENCY_BUCKETS_MS: [f64; 9] =
    [1.0, 5.0, 10.0, 50.0, 100.0, 500.0, 1000.0, 5000.0, 10000.0];

fn latency_bucket_index_ms(duration_ms: f64) -> usize {
    for (idx, boundary) in METRIC_LATENCY_BUCKETS_MS.iter().enumerate() {
        if duration_ms <= *boundary {
            return idx;
        }
    }
    METRIC_LATENCY_BUCKETS_MS.len()
}

impl RuntimeMetrics {
    /// Create new runtime metrics
    pub fn new() -> Self {
        Self {
            inner: StdMutex::new(MetricsSnapshot::default()),
        }
    }

    /// Increment successful requests
    pub fn inc_successful_requests(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.successful_requests += 1;
        guard.total_requests += 1;
    }

    /// Increment failed requests
    pub fn inc_failed_requests(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.failed_requests += 1;
        guard.total_requests += 1;
    }

    /// Increment active requests
    pub fn inc_active_requests(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.active_requests += 1;
    }

    /// Decrement active requests
    pub fn dec_active_requests(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.active_requests = guard.active_requests.saturating_sub(1);
    }

    /// Get successful requests count
    pub fn successful_requests(&self) -> u64 {
        let guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.successful_requests
    }

    /// Get failed requests count
    pub fn failed_requests(&self) -> u64 {
        let guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.failed_requests
    }

    /// Get active requests count
    pub fn active_requests(&self) -> u32 {
        let guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.active_requests
    }

    /// Get total requests count
    pub fn total_requests(&self) -> u64 {
        let guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.total_requests
    }

    /// Get average request duration in milliseconds
    pub fn avg_request_duration_ms(&self) -> f64 {
        let guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.avg_request_duration_ms
    }

    /// Update average request duration
    pub fn update_avg_duration(&self, duration_ms: f64) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        let total = guard.total_requests as f64;
        guard.avg_request_duration_ms = if total <= 1.0 {
            duration_ms
        } else {
            (guard.avg_request_duration_ms * (total - 1.0) + duration_ms) / total
        };
    }

    /// Increment review gate count
    pub fn inc_review_gate(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.review_gate_total += 1;
    }

    /// Increment review gate rejected count
    pub fn inc_review_gate_rejected(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.review_gate_rejected_total += 1;
    }

    /// Increment review gate timeout count
    pub fn inc_review_gate_timeout(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.review_gate_timeout_total += 1;
    }

    /// Increment review gate degraded count
    pub fn inc_review_gate_degraded(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.review_gate_degraded_total += 1;
    }

    /// Increment review gate approved count
    pub fn inc_review_gate_approved(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.review_gate_approved_total += 1;
    }

    /// Increment review gate invalid response count
    pub fn inc_review_gate_invalid_response(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.review_gate_invalid_response_total += 1;
    }

    /// Increment chat requests count
    pub fn inc_chat_requests(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.chat_requests_total += 1;
    }

    /// Record one ACP request outcome with duration.
    pub fn record_request_outcome(&self, success: bool, duration_ms: f64) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        if success {
            guard.successful_requests += 1;
        } else {
            guard.failed_requests += 1;
        }
        guard.total_requests += 1;

        let duration_ms = duration_ms.max(0.0);
        guard.request_latency_sum_ms += duration_ms;
        let bucket_idx = latency_bucket_index_ms(duration_ms);
        guard.request_latency_bucket_counts[bucket_idx] =
            guard.request_latency_bucket_counts[bucket_idx].saturating_add(1);
        guard.avg_request_duration_ms = if guard.total_requests == 0 {
            0.0
        } else {
            guard.request_latency_sum_ms / guard.total_requests as f64
        };
    }

    /// Record chat latency.
    pub fn record_chat_latency(&self, duration_ms: f64) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        let duration_ms = duration_ms.max(0.0);
        guard.chat_requests_total += 1;
        guard.chat_latency_sum_ms += duration_ms;
        let bucket_idx = latency_bucket_index_ms(duration_ms);
        guard.chat_latency_bucket_counts[bucket_idx] =
            guard.chat_latency_bucket_counts[bucket_idx].saturating_add(1);
    }

    pub fn inc_agent_timeout_failure(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.agent_timeout_failures_total = guard.agent_timeout_failures_total.saturating_add(1);
    }

    pub fn inc_runtime_probe_timeout(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.runtime_probe_timeout_total = guard.runtime_probe_timeout_total.saturating_add(1);
    }

    pub fn record_vector_search(&self, hit_count: usize) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.vector_search_total = guard.vector_search_total.saturating_add(1);
        guard.vector_hit_total = guard.vector_hit_total.saturating_add(hit_count as u64);
    }

    pub fn record_vector_store(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.vector_store_total = guard.vector_store_total.saturating_add(1);
    }

    pub fn record_summary_read(&self, hit: bool) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.summary_read_total = guard.summary_read_total.saturating_add(1);
        if hit {
            guard.summary_hit_total = guard.summary_hit_total.saturating_add(1);
        }
    }

    pub fn record_summary_store(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.summary_store_total = guard.summary_store_total.saturating_add(1);
    }

    /// Record review gate latency.
    pub fn record_review_latency(&self, duration_ms: f64) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        let duration_ms = duration_ms.max(0.0);
        guard.review_latency_sum_ms += duration_ms;
        let bucket_idx = latency_bucket_index_ms(duration_ms);
        guard.review_latency_bucket_counts[bucket_idx] =
            guard.review_latency_bucket_counts[bucket_idx].saturating_add(1);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        self.inner
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Reset all collected runtime metrics.
    pub fn reset_all(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        *guard = MetricsSnapshot::default();
    }
}

impl Default for RuntimeMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex as StdMutex};

    use super::{
        with_acp_lock, AcpLockMonitor, PhaseRateLimiter, RuntimeMetrics,
        ACP_LOCK_PHASE_RATE_LIMITER,
    };

    #[test]
    fn runtime_metrics_records_request_latency_and_outcomes() {
        let metrics = RuntimeMetrics::new();
        metrics.record_request_outcome(true, 12.0);
        metrics.record_request_outcome(false, 24.0);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.total_requests, 2);
        assert_eq!(snapshot.successful_requests, 1);
        assert_eq!(snapshot.failed_requests, 1);
        assert_eq!(snapshot.request_latency_sum_ms, 36.0);
        assert_eq!(snapshot.avg_request_duration_ms, 18.0);
        assert_eq!(snapshot.request_latency_bucket_counts[3], 2); // <= 50ms
    }

    #[test]
    fn runtime_metrics_records_chat_and_review_latency_buckets() {
        let metrics = RuntimeMetrics::new();
        metrics.record_chat_latency(3.0);
        metrics.record_review_latency(5001.0);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.chat_requests_total, 1);
        assert_eq!(snapshot.chat_latency_sum_ms, 3.0);
        assert_eq!(snapshot.chat_latency_bucket_counts[1], 1); // <= 5ms
        assert_eq!(snapshot.review_latency_sum_ms, 5001.0);
        assert_eq!(snapshot.review_latency_bucket_counts[8], 1); // <= 10000ms
    }

    #[test]
    fn runtime_metrics_record_vector_and_summary_counters() {
        let metrics = RuntimeMetrics::new();
        metrics.record_vector_search(2);
        metrics.record_vector_store();
        metrics.record_summary_read(true);
        metrics.record_summary_read(false);
        metrics.record_summary_store();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.vector_search_total, 1);
        assert_eq!(snapshot.vector_hit_total, 2);
        assert_eq!(snapshot.vector_store_total, 1);
        assert_eq!(snapshot.summary_read_total, 2);
        assert_eq!(snapshot.summary_hit_total, 1);
        assert_eq!(snapshot.summary_store_total, 1);
    }

    #[test]
    fn runtime_metrics_tracks_agent_and_probe_timeouts() {
        let metrics = RuntimeMetrics::new();
        metrics.inc_agent_timeout_failure();
        metrics.inc_agent_timeout_failure();
        metrics.inc_runtime_probe_timeout();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.agent_timeout_failures_total, 2);
        assert_eq!(snapshot.runtime_probe_timeout_total, 1);
    }

    #[test]
    fn acp_lock_monitor_recovers_poisoned_mutex_and_records_stats() {
        let monitor = AcpLockMonitor::default();
        let shared: Arc<StdMutex<PhaseRateLimiter>> =
            Arc::new(StdMutex::new(PhaseRateLimiter::default()));

        let poison_target = Arc::clone(&shared);
        let join = std::thread::spawn(move || {
            let _guard = poison_target.lock().expect("lock should be acquired");
            panic!("poison the lock");
        })
        .join();
        assert!(join.is_err(), "poisoning thread should panic");

        let tracked_before = with_acp_lock(
            &monitor,
            ACP_LOCK_PHASE_RATE_LIMITER,
            shared.as_ref(),
            |guard: &mut PhaseRateLimiter| {
                let _ = guard.allow("entry:test", 60, Some(5));
                guard.tracked_phases()
            },
        );
        assert_eq!(tracked_before, 1);

        let snapshot = monitor
            .snapshot()
            .into_iter()
            .find(|item| item.name == ACP_LOCK_PHASE_RATE_LIMITER)
            .expect("phase rate limiter snapshot should exist");

        assert_eq!(snapshot.poisoned_total, 1);
        assert_eq!(snapshot.recovered_total, 1);
        assert_eq!(snapshot.acquisitions, 1);
    }
}
