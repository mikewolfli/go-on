//! ACP (Agent Coordination Protocol) server implementation
//!
//! This module implements the core server functionality for the go-on ACP proxy,
//! including request handling, caching, vector storage, and circuit breaking.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Mutex, Notify};
use tokio::task::{spawn_blocking, JoinHandle};
use tokio::time::{sleep, timeout, MissedTickBehavior};

use crate::agent::{Agent, AgentRegistry, Message};
use crate::cache::ResponseCache;
use crate::config::{
    validate_runtime_readiness, AppConfig, AutoTuneConfig, AutoTuneState, PhaseOptions,
    RuntimeConfig, VectorConfig,
};
use crate::error::ProxyError;
use crate::flow::{FlowManager, ResolvedPhase};
use crate::vector::{VectorHit, VectorStore};

/// Default minimum characters for vector search queries
const DEFAULT_VECTOR_MIN_QUERY_CHARS: usize = 80;
/// Default number of top results for vector search
const DEFAULT_VECTOR_TOP_K: usize = 2;
/// Default minimum similarity threshold for vector search
const DEFAULT_VECTOR_MIN_SIMILARITY: f32 = 0.82;
/// Default maximum characters for vector search snippets
const DEFAULT_VECTOR_MAX_SNIPPET_CHARS: usize = 800;
/// Default number of messages to trigger summary
const DEFAULT_SUMMARY_TRIGGER_MESSAGES: usize = 8;
/// Default maximum characters for summaries
const DEFAULT_SUMMARY_MAX_CHARS: usize = 1200;
/// Default failure threshold for circuit breakers
const DEFAULT_BREAKER_FAILURE_THRESHOLD: u32 = 3;
/// Default time circuit breakers remain open (in seconds)
const DEFAULT_BREAKER_OPEN_SECONDS: i64 = 30;
/// Histogram buckets for latency measurements (in seconds)
const HISTOGRAM_BUCKETS_SECONDS: [f64; 10] =
    [0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0];

/// JSON-RPC request structure
#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    /// JSON-RPC version
    jsonrpc: String,
    /// Request ID
    id: Option<Value>,
    /// Method name
    method: String,
    /// Request parameters
    params: Option<Value>,
}

/// JSON-RPC response structure
#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    /// JSON-RPC version
    jsonrpc: &'static str,
    /// Response ID (matches request ID)
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    /// Response result
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    /// Error information
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

/// JSON-RPC error structure
#[derive(Debug, Serialize)]
struct JsonRpcError {
    /// Error code
    code: i64,
    /// Error message
    message: String,
    /// Additional error data
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

/// Chat mode enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatMode {
    /// Ask mode - regular chat
    Ask,
    /// Edit mode - code editing
    Edit,
    /// Agent mode - agent execution
    Agent,
    /// Full auto mode - autonomous operation
    FullAuto,
}

impl ChatMode {
    /// Parse chat mode from string
    fn parse(raw: Option<&str>) -> Option<Self> {
        let value = raw?.trim().to_ascii_lowercase();
        match value.as_str() {
            "ask" => Some(Self::Ask),
            "edit" => Some(Self::Edit),
            "agent" => Some(Self::Agent),
            "full_auto" | "full-auto" | "auto" => Some(Self::FullAuto),
            _ => None,
        }
    }

    /// Convert chat mode to string
    fn as_str(&self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Edit => "edit",
            Self::Agent => "agent",
            Self::FullAuto => "full_auto",
        }
    }
}

/// Autopilot complexity level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutopilotComplexity {
    /// Simple autopilot mode
    Simple,
    /// Complex autopilot mode
    Complex,
}

impl AutopilotComplexity {
    /// Parse complexity from string
    fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "simple" => Some(Self::Simple),
            "complex" => Some(Self::Complex),
            _ => None,
        }
    }
}

/// Approval strategy enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalStrategy {
    /// Default approval process
    DefaultApprovals,
    /// Bypass approval process
    ByPassApproval,
    /// Simple autopilot approval
    AutoPilotSimple,
    /// Complex autopilot approval (requires dual review)
    AutoPilotComplex,
}

impl ApprovalStrategy {
    /// Convert approval strategy to string
    fn as_str(&self) -> &'static str {
        match self {
            Self::DefaultApprovals => "default_approvals",
            Self::ByPassApproval => "by_pass_approval",
            Self::AutoPilotSimple => "autopilot_simple",
            Self::AutoPilotComplex => "autopilot_complex",
        }
    }

    /// Check if dual review is needed
    fn needs_dual_review(&self) -> bool {
        matches!(self, Self::AutoPilotComplex)
    }
}

/// Convert chat mode and complexity to approval strategy
fn mode_to_approval_strategy(
    mode: Option<ChatMode>,
    complexity: Option<AutopilotComplexity>,
) -> ApprovalStrategy {
    match mode {
        Some(ChatMode::Ask) => ApprovalStrategy::DefaultApprovals,
        Some(ChatMode::Edit) | Some(ChatMode::Agent) => ApprovalStrategy::ByPassApproval,
        Some(ChatMode::FullAuto) => match complexity {
            Some(AutopilotComplexity::Simple) => ApprovalStrategy::AutoPilotSimple,
            Some(AutopilotComplexity::Complex) => ApprovalStrategy::AutoPilotComplex,
            None => ApprovalStrategy::AutoPilotSimple,
        },
        None => ApprovalStrategy::DefaultApprovals,
    }
}

/// Chat request parameters
#[derive(Debug, Deserialize)]
struct ChatParams {
    /// Chat messages
    messages: Vec<Message>,
    /// Phase name
    phase: Option<String>,
    /// Chat mode
    mode: Option<String>,
    /// Additional context
    #[allow(dead_code)]
    context: Option<Value>,
}

#[derive(Debug, Default, Clone, Serialize)]
struct MetricsSnapshot {
    chat_requests_total: u64,
    cache_lookup_total: u64,
    cache_hit_total: u64,
    cache_store_total: u64,
    vector_search_total: u64,
    vector_hit_total: u64,
    vector_store_total: u64,
    summary_read_total: u64,
    summary_hit_total: u64,
    summary_store_total: u64,
    agent_failures_total: u64,
    review_gate_total: u64,
    review_gate_approved_total: u64,
    review_gate_rejected_total: u64,
    review_gate_timeout_total: u64,
    review_gate_degraded_total: u64,
    review_gate_invalid_response_total: u64,
    agent_timeout_failures_total: u64,
    agent_panic_failures_total: u64,
    agent_other_failures_total: u64,
    chat_latency_count: u64,
    chat_latency_sum_seconds: f64,
    chat_latency_bucket_counts: [u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
    agent_latency_count: u64,
    agent_latency_sum_seconds: f64,
    agent_latency_bucket_counts: [u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
    review_latency_count: u64,
    review_latency_sum_seconds: f64,
    review_latency_bucket_counts: [u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
}

#[derive(Debug, Clone, Default)]
struct RuntimeGaugeSnapshot {
    memory_cache_entries: u64,
    sqlite_cache_entries: u64,
    vector_memory_entries: u64,
    vector_summary_entries: u64,
    circuit_open_agents: u64,
    circuit_half_open_agents: u64,
    circuit_tracked_agents: u64,
    rate_limiter_tracked_phases: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
struct MaintenanceSnapshot {
    running: bool,
    cycles_total: u64,
    last_started_at: Option<i64>,
    last_completed_at: Option<i64>,
    last_memory_expired_removed: u64,
    last_sqlite_expired_removed: u64,
    last_error: Option<String>,
}

#[derive(Default)]
struct MaintenanceTracker {
    inner: StdMutex<MaintenanceSnapshot>,
}

impl MaintenanceTracker {
    fn snapshot(&self) -> MaintenanceSnapshot {
        self.inner
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    fn note_started(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.running = true;
            guard.last_started_at = Some(now_ts());
            guard.cycles_total = guard.cycles_total.saturating_add(1);
            guard.last_error = None;
        }
    }

    fn note_completed(&self, memory_removed: usize, sqlite_removed: usize) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.running = false;
            guard.last_completed_at = Some(now_ts());
            guard.last_memory_expired_removed = memory_removed as u64;
            guard.last_sqlite_expired_removed = sqlite_removed as u64;
            guard.last_error = None;
        }
    }

    fn note_failed(&self, err: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.running = false;
            guard.last_completed_at = Some(now_ts());
            guard.last_error = Some(err.to_string());
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
struct LifecycleSnapshot {
    shutting_down: bool,
    shutdown_started_at: Option<i64>,
    shutdown_reason: Option<String>,
}

#[derive(Default)]
struct LifecycleState {
    inner: StdMutex<LifecycleSnapshot>,
}

impl LifecycleState {
    fn snapshot(&self) -> LifecycleSnapshot {
        self.inner
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    fn is_shutting_down(&self) -> bool {
        self.inner
            .lock()
            .map(|guard| guard.shutting_down)
            .unwrap_or(false)
    }

    fn start_shutdown(&self, reason: &str) -> bool {
        if let Ok(mut guard) = self.inner.lock() {
            if guard.shutting_down {
                return false;
            }
            guard.shutting_down = true;
            guard.shutdown_started_at = Some(now_ts());
            guard.shutdown_reason = Some(reason.to_string());
            return true;
        }
        false
    }
}

#[derive(Default)]
struct RuntimeMetrics {
    inner: StdMutex<MetricsSnapshot>,
}

impl RuntimeMetrics {
    fn snapshot(&self) -> MetricsSnapshot {
        self.inner.lock().map(|g| g.clone()).unwrap_or_default()
    }

    fn reset(&self) {
        if let Ok(mut metrics) = self.inner.lock() {
            *metrics = MetricsSnapshot::default();
        }
    }

    fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut MetricsSnapshot),
    {
        if let Ok(mut metrics) = self.inner.lock() {
            f(&mut metrics);
        }
    }

    fn inc_chat_requests(&self) {
        self.update(|m| m.chat_requests_total += 1);
    }

    fn inc_cache_lookup(&self) {
        self.update(|m| m.cache_lookup_total += 1);
    }

    fn inc_cache_hit(&self) {
        self.update(|m| m.cache_hit_total += 1);
    }

    fn inc_cache_store(&self) {
        self.update(|m| m.cache_store_total += 1);
    }

    fn inc_vector_search(&self) {
        self.update(|m| m.vector_search_total += 1);
    }

    fn inc_vector_hit(&self) {
        self.update(|m| m.vector_hit_total += 1);
    }

    fn inc_vector_store(&self) {
        self.update(|m| m.vector_store_total += 1);
    }

    fn inc_summary_read(&self) {
        self.update(|m| m.summary_read_total += 1);
    }

    fn inc_summary_hit(&self) {
        self.update(|m| m.summary_hit_total += 1);
    }

    fn inc_summary_store(&self) {
        self.update(|m| m.summary_store_total += 1);
    }

    fn inc_agent_failures(&self) {
        self.update(|m| m.agent_failures_total += 1);
    }

    fn inc_agent_timeout_failures(&self) {
        self.update(|m| m.agent_timeout_failures_total += 1);
    }

    fn inc_agent_panic_failures(&self) {
        self.update(|m| m.agent_panic_failures_total += 1);
    }

    fn inc_agent_other_failures(&self) {
        self.update(|m| m.agent_other_failures_total += 1);
    }

    fn inc_review_gate(&self) {
        self.update(|m| m.review_gate_total += 1);
    }

    fn inc_review_gate_approved(&self) {
        self.update(|m| m.review_gate_approved_total += 1);
    }

    fn inc_review_gate_rejected(&self) {
        self.update(|m| m.review_gate_rejected_total += 1);
    }

    fn inc_review_gate_timeout(&self) {
        self.update(|m| m.review_gate_timeout_total += 1);
    }

    fn inc_review_gate_degraded(&self) {
        self.update(|m| m.review_gate_degraded_total += 1);
    }

    fn inc_review_gate_invalid_response(&self) {
        self.update(|m| m.review_gate_invalid_response_total += 1);
    }

    fn observe_chat_latency(&self, duration: Duration) {
        self.update(|m| {
            observe_latency_histogram(
                duration,
                &mut m.chat_latency_count,
                &mut m.chat_latency_sum_seconds,
                &mut m.chat_latency_bucket_counts,
            )
        });
    }

    fn observe_agent_latency(&self, duration: Duration) {
        self.update(|m| {
            observe_latency_histogram(
                duration,
                &mut m.agent_latency_count,
                &mut m.agent_latency_sum_seconds,
                &mut m.agent_latency_bucket_counts,
            )
        });
    }

    fn observe_review_latency(&self, duration: Duration) {
        self.update(|m| {
            observe_latency_histogram(
                duration,
                &mut m.review_latency_count,
                &mut m.review_latency_sum_seconds,
                &mut m.review_latency_bucket_counts,
            )
        });
    }
}

#[derive(Debug, Clone, Serialize)]
struct ReviewDecision {
    reviewer: String,
    verdict: String,
    response: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewVerdict {
    Approve,
    Reject,
    Invalid,
}

impl ReviewVerdict {
    fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "APPROVE",
            Self::Reject => "REJECT",
            Self::Invalid => "INVALID",
        }
    }

    fn is_approved(self) -> bool {
        matches!(self, Self::Approve)
    }
}

enum ReviewGateOutcome {
    Approved(Vec<ReviewDecision>),
    Rejected(Vec<ReviewDecision>),
    Degraded(Vec<ReviewDecision>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewTimeoutPolicy {
    Reject,
    DegradeSingle,
}

impl ReviewTimeoutPolicy {
    fn from_options(options: Option<&PhaseOptions>) -> Self {
        let value = options
            .and_then(|opts| opts.extra.get("review_timeout_policy"))
            .and_then(|v| v.as_str())
            .unwrap_or("reject");

        if value.eq_ignore_ascii_case("degrade_single") {
            Self::DegradeSingle
        } else {
            Self::Reject
        }
    }
}

struct PreparedChatInput {
    messages: Vec<Message>,
    latest_user_query: Option<String>,
}

#[derive(Clone)]
struct MemoryCachedResponse {
    response_text: String,
    agent_name: Option<String>,
    expires_at: i64,
}

#[derive(Default)]
struct MemoryResponseCache {
    inner: StdMutex<HashMap<String, MemoryCachedResponse>>,
}

impl MemoryResponseCache {
    fn get(&self, key: &str) -> Option<MemoryCachedResponse> {
        let now = now_ts();
        let mut guard = self.inner.lock().ok()?;
        guard.retain(|_, entry| entry.expires_at > now);
        guard.get(key).cloned()
    }

    fn purge_expired(&self) -> usize {
        let now = now_ts();
        if let Ok(mut guard) = self.inner.lock() {
            let before = guard.len();
            guard.retain(|_, entry| entry.expires_at > now);
            return before.saturating_sub(guard.len());
        }
        0
    }

    fn clear_all(&self) -> usize {
        if let Ok(mut guard) = self.inner.lock() {
            let removed = guard.len();
            guard.clear();
            return removed;
        }
        0
    }

    fn active_entries(&self) -> usize {
        self.purge_expired();
        self.inner.lock().map(|guard| guard.len()).unwrap_or(0)
    }

    fn put(
        &self,
        key: String,
        response_text: String,
        agent_name: Option<String>,
        ttl_seconds: u64,
    ) {
        if ttl_seconds == 0 {
            return;
        }

        let expires_at = now_ts() + ttl_seconds as i64;
        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(
                key,
                MemoryCachedResponse {
                    response_text,
                    agent_name,
                    expires_at,
                },
            );

            // Keep L1 cache bounded to avoid unbounded memory growth.
            if guard.len() > 2048 {
                let mut entries: Vec<(String, i64)> = guard
                    .iter()
                    .map(|(k, v)| (k.clone(), v.expires_at))
                    .collect();
                entries.sort_by_key(|(_, expires_at)| *expires_at);
                let remove_count = guard.len() - 2048;
                for (k, _) in entries.into_iter().take(remove_count) {
                    guard.remove(&k);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CircuitBreakerStage {
    Closed,
    Open,
    HalfOpen,
}

impl CircuitBreakerStage {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::HalfOpen => "half_open",
        }
    }
}

#[derive(Debug, Clone)]
struct CircuitBreakerState {
    consecutive_failures: u32,
    stage: CircuitBreakerStage,
    open_until: Option<i64>,
    probe_in_flight: bool,
}

impl Default for CircuitBreakerState {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            stage: CircuitBreakerStage::Closed,
            open_until: None,
            probe_in_flight: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct CircuitBreakerSnapshot {
    consecutive_failures: u32,
    state: String,
    open_until: Option<i64>,
    probe_in_flight: bool,
}

enum CircuitBreakerAdmission {
    Closed,
    HalfOpenProbe,
    Rejected {
        state: &'static str,
        retry_after_seconds: Option<i64>,
    },
}

#[derive(Default)]
struct CircuitBreakerRegistry {
    inner: StdMutex<HashMap<String, CircuitBreakerState>>,
}

impl CircuitBreakerRegistry {
    fn allow_request(&self, agent_name: &str) -> CircuitBreakerAdmission {
        let now = now_ts();
        if let Ok(mut guard) = self.inner.lock() {
            let state = guard.entry(agent_name.to_string()).or_default();
            match state.stage {
                CircuitBreakerStage::Closed => CircuitBreakerAdmission::Closed,
                CircuitBreakerStage::Open => {
                    if let Some(open_until) = state.open_until {
                        if open_until > now {
                            return CircuitBreakerAdmission::Rejected {
                                state: "open",
                                retry_after_seconds: Some((open_until - now).max(0)),
                            };
                        }
                    }

                    state.stage = CircuitBreakerStage::HalfOpen;
                    state.open_until = None;
                    state.probe_in_flight = true;
                    CircuitBreakerAdmission::HalfOpenProbe
                }
                CircuitBreakerStage::HalfOpen => {
                    if state.probe_in_flight {
                        CircuitBreakerAdmission::Rejected {
                            state: "half_open",
                            retry_after_seconds: None,
                        }
                    } else {
                        state.probe_in_flight = true;
                        CircuitBreakerAdmission::HalfOpenProbe
                    }
                }
            }
        } else {
            CircuitBreakerAdmission::Closed
        }
    }

    fn record_success(&self, agent_name: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            let state = guard.entry(agent_name.to_string()).or_default();
            state.consecutive_failures = 0;
            state.stage = CircuitBreakerStage::Closed;
            state.open_until = None;
            state.probe_in_flight = false;
        }
    }

    fn record_failure_with_config(
        &self,
        agent_name: &str,
        failure_threshold: u32,
        open_seconds: i64,
    ) {
        let now = now_ts();
        if let Ok(mut guard) = self.inner.lock() {
            let state = guard.entry(agent_name.to_string()).or_default();
            let effective_threshold = failure_threshold.max(1);
            if state.stage == CircuitBreakerStage::HalfOpen {
                state.consecutive_failures = effective_threshold;
                state.stage = CircuitBreakerStage::Open;
                state.probe_in_flight = false;
                state.open_until = Some(now + open_seconds.max(1));
                return;
            }

            state.consecutive_failures = state.consecutive_failures.saturating_add(1);
            if state.consecutive_failures >= effective_threshold {
                state.stage = CircuitBreakerStage::Open;
                state.probe_in_flight = false;
                state.open_until = Some(now + open_seconds.max(1));
            }
        }
    }

    fn snapshot(&self) -> HashMap<String, CircuitBreakerSnapshot> {
        let now = now_ts();
        self.inner
            .lock()
            .map(|guard| {
                guard
                    .iter()
                    .map(|(name, state)| {
                        let state_name = if state.stage == CircuitBreakerStage::Open
                            && state.open_until.map(|until| until <= now).unwrap_or(false)
                        {
                            "half_open_ready".to_string()
                        } else {
                            state.stage.as_str().to_string()
                        };
                        (
                            name.clone(),
                            CircuitBreakerSnapshot {
                                consecutive_failures: state.consecutive_failures,
                                state: state_name,
                                open_until: state.open_until,
                                probe_in_flight: state.probe_in_flight,
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn open_count(&self) -> usize {
        let now = now_ts();
        if let Ok(guard) = self.inner.lock() {
            return guard
                .values()
                .filter(|state| {
                    state.stage == CircuitBreakerStage::Open
                        && state.open_until.map(|until| until > now).unwrap_or(false)
                })
                .count();
        }
        0
    }

    fn half_open_count(&self) -> usize {
        if let Ok(guard) = self.inner.lock() {
            return guard
                .values()
                .filter(|state| state.stage == CircuitBreakerStage::HalfOpen)
                .count();
        }
        0
    }

    fn tracked_agents(&self) -> usize {
        self.inner.lock().map(|guard| guard.len()).unwrap_or(0)
    }
}

#[derive(Default)]
struct PhaseRateLimiter {
    inner: StdMutex<HashMap<String, TokenBucketState>>,
}

#[derive(Clone)]
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
    fn allow(&self, phase_name: &str, rpm_limit: u64, burst_capacity: Option<u64>) -> bool {
        if rpm_limit == 0 {
            return false;
        }

        let now = now_ms();
        let refill_per_second = rpm_limit as f64 / 60.0;
        let capacity = burst_capacity.unwrap_or(rpm_limit).max(1) as f64;

        if let Ok(mut guard) = self.inner.lock() {
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
            return true;
        }
        true
    }

    fn tracked_phases(&self) -> usize {
        self.inner.lock().map(|guard| guard.len()).unwrap_or(0)
    }

    fn snapshot(&self) -> HashMap<String, (f64, f64)> {
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
}

#[derive(Default)]
struct InflightLimiter {
    inner: StdMutex<InflightState>,
}

#[derive(Default)]
struct InflightState {
    global: usize,
    phase: HashMap<String, usize>,
}

struct InflightGuard {
    limiter: Arc<InflightLimiter>,
    phase_name: String,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.limiter.leave(&self.phase_name);
    }
}

impl InflightLimiter {
    fn try_enter(
        self: &Arc<Self>,
        phase_name: &str,
        phase_limit: Option<u64>,
        global_limit: Option<u64>,
    ) -> Option<InflightGuard> {
        if let Ok(mut guard) = self.inner.lock() {
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
            return Some(InflightGuard {
                limiter: Arc::clone(self),
                phase_name: phase_name.to_string(),
            });
        }
        None
    }

    fn leave(&self, phase_name: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.global = guard.global.saturating_sub(1);
            if let Some(value) = guard.phase.get_mut(phase_name) {
                *value = value.saturating_sub(1);
                if *value == 0 {
                    guard.phase.remove(phase_name);
                }
            }
        }
    }

    fn snapshot(&self) -> (usize, HashMap<String, usize>) {
        self.inner
            .lock()
            .map(|guard| (guard.global, guard.phase.clone()))
            .unwrap_or_default()
    }

    fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.global = 0;
            guard.phase.clear();
        }
    }
}

/// ACP server implementation
///
/// This struct represents the main ACP server that handles incoming requests,
/// manages agents, and coordinates the overall system flow.
pub struct AcpServer {
    /// Flow manager for handling request routing through phases
    flow: Arc<StdMutex<Arc<FlowManager>>>,
    /// Agent registry for managing available agents
    registry: Arc<StdMutex<Arc<AgentRegistry>>>,
    /// Response cache (SQLite-based)
    cache: Arc<StdMutex<Option<Arc<ResponseCache>>>>,
    /// Vector store for similarity search and memory
    vector_store: Arc<StdMutex<Option<Arc<VectorStore>>>>,
    /// Vector store configuration
    vector_config: Arc<StdMutex<Option<VectorConfig>>>,
    /// Autotune state for adaptive configuration
    autotune: Arc<StdMutex<Option<Arc<Mutex<AutoTuneState>>>>>,
    /// Autotune configuration
    autotune_config: Arc<StdMutex<Option<AutoTuneConfig>>>,
    /// Path to autotune state file
    autotune_state_path: Arc<StdMutex<Option<String>>>,
    /// Runtime configuration
    runtime_config: Arc<StdMutex<RuntimeConfig>>,
    /// Runtime metrics collection
    metrics: Arc<RuntimeMetrics>,
    /// In-memory response cache for fast access
    memory_cache: Arc<MemoryResponseCache>,
    /// Maintenance tracker for system health
    maintenance: Arc<MaintenanceTracker>,
    /// Lifecycle state management
    lifecycle: Arc<LifecycleState>,
    /// Circuit breakers for agent failure handling
    circuit_breakers: Arc<CircuitBreakerRegistry>,
    /// Rate limiter for phase-level throttling
    phase_rate_limiter: Arc<PhaseRateLimiter>,
    /// In-flight request limiter
    inflight_limiter: Arc<InflightLimiter>,
    /// Path to configuration file
    config_path: Option<PathBuf>,
    /// Forced phase name (if specified)
    forced_phase: Option<String>,
    /// HTTP client for external requests
    http_client: Option<reqwest::Client>,
    /// Verbose logging flag
    verbose: bool,
    /// Output stream for responses
    output: Arc<Mutex<tokio::io::Stdout>>,
    /// Shutdown notification mechanism
    shutdown_notify: Arc<Notify>,
}

impl AcpServer {
    /// Create a new ACP server instance
    ///
    /// # Arguments
    /// * `flow` - Flow manager for request routing through phases
    /// * `registry` - Agent registry for managing available agents
    /// * `cache` - Response cache (SQLite-based)
    /// * `vector_store` - Vector store for similarity search and memory
    /// * `vector_config` - Vector store configuration
    /// * `autotune` - Autotune state for adaptive configuration
    /// * `autotune_config` - Autotune configuration
    /// * `autotune_state_path` - Path to autotune state file
    /// * `runtime_config` - Runtime configuration
    /// * `config_path` - Path to configuration file
    /// * `forced_phase` - Forced phase name (if specified)
    /// * `http_client` - HTTP client for external requests
    /// * `verbose` - Verbose logging flag
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        flow: Arc<FlowManager>,
        registry: Arc<AgentRegistry>,
        cache: Option<Arc<ResponseCache>>,
        vector_store: Option<Arc<VectorStore>>,
        vector_config: Option<VectorConfig>,
        autotune: Option<Arc<Mutex<AutoTuneState>>>,
        autotune_config: Option<AutoTuneConfig>,
        autotune_state_path: Option<String>,
        runtime_config: RuntimeConfig,
        config_path: Option<PathBuf>,
        forced_phase: Option<String>,
        http_client: Option<reqwest::Client>,
        verbose: bool,
    ) -> Self {
        Self {
            flow: Arc::new(StdMutex::new(flow)),
            registry: Arc::new(StdMutex::new(registry)),
            cache: Arc::new(StdMutex::new(cache)),
            vector_store: Arc::new(StdMutex::new(vector_store)),
            vector_config: Arc::new(StdMutex::new(vector_config)),
            autotune: Arc::new(StdMutex::new(autotune)),
            autotune_config: Arc::new(StdMutex::new(autotune_config)),
            autotune_state_path: Arc::new(StdMutex::new(autotune_state_path)),
            runtime_config: Arc::new(StdMutex::new(runtime_config)),
            metrics: Arc::new(RuntimeMetrics::default()),
            memory_cache: Arc::new(MemoryResponseCache::default()),
            maintenance: Arc::new(MaintenanceTracker::default()),
            lifecycle: Arc::new(LifecycleState::default()),
            circuit_breakers: Arc::new(CircuitBreakerRegistry::default()),
            phase_rate_limiter: Arc::new(PhaseRateLimiter::default()),
            inflight_limiter: Arc::new(InflightLimiter::default()),
            config_path,
            forced_phase,
            http_client,
            verbose,
            output: Arc::new(Mutex::new(tokio::io::stdout())),
            shutdown_notify: Arc::new(Notify::new()),
        }
    }

    /// Run the ACP server
    ///
    /// This method starts the server, handles incoming requests from stdin,
    /// and manages the server lifecycle.
    ///
    /// # Returns
    /// * `Result<()>` - Returns Ok(()) on successful shutdown, or an error if something goes wrong
    pub async fn run(&mut self) -> Result<()> {
        // Spawn background maintenance loop
        let background_task = self.spawn_background_maintenance_loop();
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin).lines();

        // Process incoming requests from stdin
        while let Some(line) = reader.next_line().await? {
            if self.lifecycle.is_shutting_down() {
                break;
            }

            if line.trim().is_empty() {
                continue;
            }

            // Parse JSON-RPC request
            let request: JsonRpcRequest = match serde_json::from_str(&line) {
                Ok(req) => req,
                Err(err) => {
                    self.send_error(None, -32700, format!("parse error: {err}"), None)
                        .await?;
                    continue;
                }
            };

            // Validate JSON-RPC version
            if request.jsonrpc != "2.0" {
                self.send_error(
                    request.id,
                    -32600,
                    ProxyError::InvalidRequest("jsonrpc must be 2.0".to_string()).to_string(),
                    None,
                )
                .await?;
                continue;
            }

            let method = request.method.clone();
            if self.verbose {
                debug!("incoming method: {method}");
            }

            // Handle request in a separate task to avoid blocking the main loop
            let id_for_response = request.id.clone();
            let handle = tokio::spawn(async move { request });
            let request = match handle.await {
                Ok(req) => req,
                Err(join_err) => {
                    self.send_error(
                        id_for_response,
                        -32603,
                        format!("request handling panic: {join_err}"),
                        None,
                    )
                    .await?;
                    continue;
                }
            };

            // Process the request
            let response = self.handle_request(request).await;
            if let Err(err) = response {
                error!("request failed: {err:#}");
            }

            // Check if shutdown is requested
            if method == "shutdown" || self.lifecycle.is_shutting_down() {
                info!("shutdown requested; waiting for in-flight work to complete");
                break;
            }
        }

        // Shutdown sequence
        self.begin_shutdown("stdin closed or shutdown requested");
        self.wait_for_inflight_drain().await;
        self.shutdown_notify.notify_waiters();

        // Wait for background task to complete
        if let Err(err) = background_task.await {
            warn!("background maintenance task exited unexpectedly: {}", err);
        }

        Ok(())
    }

    fn routing_handles(&self) -> Result<(Arc<FlowManager>, Arc<AgentRegistry>)> {
        let flow = self
            .flow
            .lock()
            .map_err(|_| anyhow::anyhow!("flow mutex poisoned"))?
            .clone();
        let registry = self
            .registry
            .lock()
            .map_err(|_| anyhow::anyhow!("registry mutex poisoned"))?
            .clone();
        Ok((flow, registry))
    }

    fn cache_handle(&self) -> Option<Arc<ResponseCache>> {
        self.cache.lock().ok().and_then(|guard| guard.clone())
    }

    fn vector_store_handle(&self) -> Option<Arc<VectorStore>> {
        self.vector_store
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    fn vector_config_snapshot(&self) -> Option<VectorConfig> {
        self.vector_config
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    fn autotune_handle(&self) -> Option<Arc<Mutex<AutoTuneState>>> {
        self.autotune.lock().ok().and_then(|guard| guard.clone())
    }

    fn autotune_config_snapshot(&self) -> Option<AutoTuneConfig> {
        self.autotune_config
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    fn autotune_state_path_snapshot(&self) -> Option<String> {
        self.autotune_state_path
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    fn runtime_config_snapshot(&self) -> RuntimeConfig {
        self.runtime_config
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    fn begin_shutdown(&self, reason: &str) {
        if self.lifecycle.start_shutdown(reason) {
            self.shutdown_notify.notify_waiters();
        }
    }

    async fn wait_for_inflight_drain(&self) {
        let timeout_seconds = self.runtime_config_snapshot().shutdown_drain_seconds.max(1);
        let deadline = Instant::now() + Duration::from_secs(timeout_seconds);

        loop {
            let (global_inflight, _) = self.inflight_limiter.snapshot();
            if global_inflight == 0 {
                return;
            }

            if Instant::now() >= deadline {
                warn!(
                    "shutdown drain timeout reached with {} in-flight request(s) still tracked",
                    global_inflight
                );
                return;
            }

            sleep(Duration::from_millis(100)).await;
        }
    }

    async fn run_maintenance_cycle(&self, source: &str) -> MaintenanceCycleResult {
        match perform_maintenance_cycle(
            Arc::clone(&self.memory_cache),
            Arc::clone(&self.cache),
            Arc::clone(&self.maintenance),
            source,
        )
        .await
        {
            Ok(result) => result,
            Err(err) => {
                warn!("maintenance cycle '{}' failed: {}", source, err);
                MaintenanceCycleResult::default()
            }
        }
    }

    fn spawn_background_maintenance_loop(&self) -> JoinHandle<()> {
        let runtime_config = Arc::clone(&self.runtime_config);
        let memory_cache = Arc::clone(&self.memory_cache);
        let cache = Arc::clone(&self.cache);
        let vector_store = Arc::clone(&self.vector_store);
        let maintenance = Arc::clone(&self.maintenance);
        let lifecycle = Arc::clone(&self.lifecycle);
        let circuit_breakers = Arc::clone(&self.circuit_breakers);
        let phase_rate_limiter = Arc::clone(&self.phase_rate_limiter);
        let inflight_limiter = Arc::clone(&self.inflight_limiter);
        let shutdown_notify = Arc::clone(&self.shutdown_notify);

        tokio::spawn(async move {
            run_background_maintenance_loop(
                runtime_config,
                memory_cache,
                cache,
                vector_store,
                maintenance,
                lifecycle,
                circuit_breakers,
                phase_rate_limiter,
                inflight_limiter,
                shutdown_notify,
            )
            .await;
        })
    }

    async fn handle_request(&self, request: JsonRpcRequest) -> Result<()> {
        if self.lifecycle.is_shutting_down() && request.method != "shutdown" {
            return self
                .send_error(
                    request.id,
                    -32031,
                    "server is shutting down".to_string(),
                    Some(serde_json::to_value(self.lifecycle.snapshot())?),
                )
                .await;
        }

        match request.method.as_str() {
            "initialize" => {
                let result = json!({
                    "name": "go-on",
                    "protocol": "acp",
                    "capabilities": {
                        "chat": true,
                        "streaming": true,
                        "phase": true,
                        "metrics": true,
                        "autotune": self.autotune_config_snapshot().map(|cfg| cfg.enabled).unwrap_or(false),
                    }
                });
                self.send_result(request.id, result).await
            }
            "chat" => self.handle_chat(request.id, request.params).await,
            "metrics.get" => {
                let result = serde_json::to_value(self.metrics.snapshot())?;
                self.send_result(request.id, result).await
            }
            "metrics.prometheus" => {
                let sqlite_cache_entries = if let Some(cache) = self.cache_handle() {
                    self.cache_entry_count(cache.clone()).await.unwrap_or(0)
                } else {
                    0
                };
                let (vector_memory_entries, vector_summary_entries) =
                    if let Some(store) = self.vector_store_handle() {
                        self.vector_entry_counts(store.clone())
                            .await
                            .unwrap_or((0, 0))
                    } else {
                        (0, 0)
                    };

                let gauges = RuntimeGaugeSnapshot {
                    memory_cache_entries: self.memory_cache.active_entries() as u64,
                    sqlite_cache_entries,
                    vector_memory_entries,
                    vector_summary_entries,
                    circuit_open_agents: self.circuit_breakers.open_count() as u64,
                    circuit_half_open_agents: self.circuit_breakers.half_open_count() as u64,
                    circuit_tracked_agents: self.circuit_breakers.tracked_agents() as u64,
                    rate_limiter_tracked_phases: self.phase_rate_limiter.tracked_phases() as u64,
                };
                let breaker_snapshot = self.circuit_breakers.snapshot();
                let phase_limiter_snapshot = self.phase_rate_limiter.snapshot();
                let inflight_snapshot = self.inflight_limiter.snapshot();
                let lifecycle = self.lifecycle.snapshot();
                let maintenance = self.maintenance.snapshot();
                let result = json!({
                    "text": build_prometheus_metrics(
                        &self.metrics.snapshot(),
                        &gauges,
                        &breaker_snapshot,
                        &phase_limiter_snapshot,
                        &inflight_snapshot,
                        &lifecycle,
                        &maintenance,
                    )
                });
                self.send_result(request.id, result).await
            }
            "metrics.reset" => {
                self.metrics.reset();
                self.send_result(request.id, json!({"ok": true})).await
            }
            "runtime.health" => {
                let (global_inflight, phase_inflight) = self.inflight_limiter.snapshot();
                let sqlite_cache_entries = if let Some(cache) = self.cache_handle() {
                    match self.cache_entry_count(cache.clone()).await {
                        Ok(value) => Some(value),
                        Err(err) => {
                            warn!("failed to read sqlite cache entry count: {}", err);
                            None
                        }
                    }
                } else {
                    None
                };

                let vector_entries = if let Some(store) = self.vector_store_handle() {
                    match self.vector_entry_counts(store.clone()).await {
                        Ok((memory, summaries)) => Some(json!({
                            "memory_entries": memory,
                            "summary_entries": summaries,
                        })),
                        Err(err) => {
                            warn!("failed to read vector entry counts: {}", err);
                            None
                        }
                    }
                } else {
                    None
                };

                let result = json!({
                    "memory_cache_entries": self.memory_cache.active_entries(),
                    "sqlite_cache_entries": sqlite_cache_entries,
                    "circuit_breaker": {
                        "open_agents": self.circuit_breakers.open_count(),
                        "half_open_agents": self.circuit_breakers.half_open_count(),
                        "tracked_agents": self.circuit_breakers.tracked_agents(),
                        "agents": self.circuit_breakers.snapshot(),
                    },
                    "rate_limiter": {
                        "tracked_phases": self.phase_rate_limiter.tracked_phases(),
                    },
                    "inflight": {
                        "global": global_inflight,
                        "per_phase": phase_inflight,
                    },
                    "vector": vector_entries,
                    "lifecycle": self.lifecycle.snapshot(),
                    "maintenance": self.maintenance.snapshot(),
                    "review_gate": {
                        "total": self.metrics.snapshot().review_gate_total,
                        "approved": self.metrics.snapshot().review_gate_approved_total,
                        "rejected": self.metrics.snapshot().review_gate_rejected_total,
                        "timeout": self.metrics.snapshot().review_gate_timeout_total,
                        "degraded": self.metrics.snapshot().review_gate_degraded_total,
                        "invalid_response": self.metrics.snapshot().review_gate_invalid_response_total,
                    },
                });
                self.send_result(request.id, result).await
            }
            "phase.status" => {
                let limiter = self
                    .phase_rate_limiter
                    .snapshot()
                    .into_iter()
                    .map(|(phase, (tokens, capacity))| {
                        (
                            phase,
                            json!({
                                "tokens": tokens,
                                "capacity": capacity,
                            }),
                        )
                    })
                    .collect::<serde_json::Map<String, Value>>();
                let inflight = self.inflight_limiter.snapshot().1;
                self.send_result(
                    request.id,
                    json!({
                        "rate_limiter": limiter,
                        "inflight": inflight,
                    }),
                )
                .await
            }
            "breaker.status" => {
                let now = now_ts();
                let status = self
                    .circuit_breakers
                    .snapshot()
                    .into_iter()
                    .map(|(agent, snapshot)| {
                        (
                            agent,
                            json!({
                                "consecutive_failures": snapshot.consecutive_failures,
                                "state": snapshot.state,
                                "open_until": snapshot.open_until,
                                "probe_in_flight": snapshot.probe_in_flight,
                                "open": snapshot.open_until.map(|ts| ts > now).unwrap_or(false),
                            }),
                        )
                    })
                    .collect::<serde_json::Map<String, Value>>();
                self.send_result(request.id, Value::Object(status)).await
            }
            "breaker.reset" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let target = params.get("agent").and_then(|v| v.as_str());
                let removed = if let Some(agent_name) = target {
                    self.circuit_breakers
                        .inner
                        .lock()
                        .ok()
                        .and_then(|mut guard| guard.remove(agent_name).map(|_| 1_usize))
                        .unwrap_or(0)
                } else {
                    self.circuit_breakers
                        .inner
                        .lock()
                        .map(|mut guard| {
                            let count = guard.len();
                            guard.clear();
                            count
                        })
                        .unwrap_or(0)
                };
                self.send_result(request.id, json!({"ok": true, "removed": removed}))
                    .await
            }
            "config.reload" => {
                let reloaded = self.reload_runtime_config().await?;
                self.send_result(request.id, reloaded).await
            }
            "cache.clear" => {
                let memory_removed = self.memory_cache.clear_all();
                let sqlite_removed = if let Some(cache) = self.cache_handle() {
                    self.cache_clear(cache.clone()).await.unwrap_or(0)
                } else {
                    0
                };

                let result = json!({
                    "ok": true,
                    "memory_removed": memory_removed,
                    "sqlite_removed": sqlite_removed,
                });
                self.send_result(request.id, result).await
            }
            "vector.clear" => {
                let (memory_removed, summary_removed) =
                    if let Some(store) = self.vector_store_handle() {
                        self.vector_clear(store.clone()).await?
                    } else {
                        (0, 0)
                    };

                let result = json!({
                    "ok": true,
                    "vector_removed": memory_removed,
                    "summary_removed": summary_removed,
                });
                self.send_result(request.id, result).await
            }
            "maintenance.gc" => {
                let cycle = self.run_maintenance_cycle("rpc").await;
                let result = json!({
                    "ok": true,
                    "memory_expired_removed": cycle.memory_expired_removed,
                    "sqlite_expired_removed": cycle.sqlite_expired_removed,
                    "maintenance": self.maintenance.snapshot(),
                });
                self.send_result(request.id, result).await
            }
            "autotune.get" => {
                if let Some(autotune) = self.autotune_handle() {
                    let state = autotune.lock().await;
                    let result = state.snapshot();
                    self.send_result(request.id, result).await
                } else {
                    self.send_error(
                        request.id,
                        -32603,
                        "autotune is not enabled".to_string(),
                        None,
                    )
                    .await
                }
            }
            "autotune.reset" => {
                if let Some(autotune) = self.autotune_handle() {
                    if let Some(config) = self.autotune_config_snapshot() {
                        let new_state = {
                            let mut state = autotune.lock().await;
                            *state = AutoTuneState::new(&config);
                            state.clone()
                        };
                        if let Some(path) = self.autotune_state_path_snapshot() {
                            let path_ref = path.as_str();
                            if let Err(e) = new_state.save(path_ref) {
                                warn!("failed to save autotune state: {}", e);
                            }
                        } else {
                            warn!("autotune reset skipped persistence because no resolved state path is available");
                        }
                        self.send_result(request.id, json!({"ok": true})).await
                    } else {
                        self.send_error(
                            request.id,
                            -32603,
                            "autotune config not available".to_string(),
                            None,
                        )
                        .await
                    }
                } else {
                    self.send_error(
                        request.id,
                        -32603,
                        "autotune is not enabled".to_string(),
                        None,
                    )
                    .await
                }
            }
            "shutdown" => {
                self.begin_shutdown("rpc shutdown");
                self.send_result(
                    request.id,
                    json!({
                        "ok": true,
                        "lifecycle": self.lifecycle.snapshot(),
                    }),
                )
                .await
            }
            other => {
                self.send_error(
                    request.id,
                    -32601,
                    ProxyError::UnknownMethod(other.to_string()).to_string(),
                    None,
                )
                .await
            }
        }
    }

    async fn handle_chat(&self, id: Option<Value>, params: Option<Value>) -> Result<()> {
        let started = Instant::now();
        let result = async {
            if self.lifecycle.is_shutting_down() {
                self.send_error(
                    id,
                    -32031,
                    "server is shutting down".to_string(),
                    Some(serde_json::to_value(self.lifecycle.snapshot())?),
                )
                .await?;
                return Ok(());
            }

            self.metrics.inc_chat_requests();

            let params_value = params.unwrap_or_else(|| json!({}));
            let chat_params: ChatParams = match serde_json::from_value(params_value) {
                Ok(value) => value,
                Err(err) => {
                    self.send_error(
                        id,
                        -32602,
                        ProxyError::InvalidRequest(format!("invalid chat params: {err}"))
                            .to_string(),
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            };

            let mode = ChatMode::parse(chat_params.mode.as_deref());
            let mode_name = mode.map(|m| m.as_str()).unwrap_or("default");
            let (flow, registry) = self.routing_handles()?;
            let effective_phase = self.infer_phase_name_with_flow(
                flow.as_ref(),
                chat_params.phase.as_deref(),
                mode,
            );

            let total_chars: usize = chat_params
                .messages
                .iter()
                .map(|m| m.content.chars().count())
                .sum();

            let routing = flow
                .resolve(Some(effective_phase.clone()), registry.as_ref())
                .map_err(|err| ProxyError::Internal(err.to_string()))?;

        if let Some(limit) = extra_u64(routing.phase.options.as_ref(), "max_request_chars") {
            if total_chars > limit as usize {
                self.send_error(
                    id,
                    -32600,
                    format!(
                        "request too large: {} chars exceeds limit {}",
                        total_chars, limit
                    ),
                    None,
                )
                .await?;
                return Ok(());
            }
        }

            if let Some(rpm_limit) = extra_u64(routing.phase.options.as_ref(), "rate_limit_rpm") {
                let burst_capacity = extra_u64(routing.phase.options.as_ref(), "rate_limit_burst").or_else(|| {
                    extra_f64(routing.phase.options.as_ref(), "rate_limit_burst_multiplier")
                        .map(|m| ((rpm_limit as f64) * m.max(0.1)).round() as u64)
                });
            if !self
                .phase_rate_limiter
                    .allow(&routing.phase.phase_name, rpm_limit, burst_capacity)
            {
                self.send_error(
                    id,
                    -32029,
                    format!(
                        "phase '{}' rate limited at {} requests/min",
                        routing.phase.phase_name, rpm_limit
                    ),
                    None,
                )
                .await?;
                return Ok(());
            }
            }

            let phase_max_inflight = extra_u64(routing.phase.options.as_ref(), "phase_max_inflight");
            let global_max_inflight = extra_u64(routing.phase.options.as_ref(), "global_max_inflight");
            let _inflight_guard = match self.inflight_limiter.try_enter(
                &routing.phase.phase_name,
                phase_max_inflight,
                global_max_inflight,
            ) {
                Some(guard) => guard,
                None => {
                    self.send_error(
                        id,
                        -32030,
                        "inflight limit exceeded for this phase or globally".to_string(),
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            };

        let autopilot_complexity = routing
            .phase
            .options
            .as_ref()
            .and_then(|opts| opts.autopilot_complexity.as_deref())
            .and_then(AutopilotComplexity::from_str);

        let approval_strategy = mode_to_approval_strategy(mode, autopilot_complexity);

        info!(
            "phase '{}' ({}) selected from flow '{}' with {} candidate agent(s); mode={}, strategy={}",
            routing.phase.phase_name,
            routing.phase.phase_description,
            routing.phase.flow_name,
            routing.agents.len(),
            mode_name,
            approval_strategy.as_str(),
        );

        let review_decisions = if approval_strategy.needs_dual_review() {
            match self
                .run_dual_review_gate(
                    id.clone(),
                    &chat_params.messages,
                    routing.phase.options.as_ref(),
                )
                .await
            {
                Ok(ReviewGateOutcome::Approved(decisions)) => Some(decisions),
                Ok(ReviewGateOutcome::Rejected(decisions)) => {
                    self.send_error(
                        id,
                        -32603,
                        "review gate rejected execution".to_string(),
                        Some(json!({ "reviews": decisions })),
                    )
                    .await?;
                    return Ok(());
                }
                    Ok(ReviewGateOutcome::Degraded(decisions)) => {
                        self.send_notification(
                            "chat.review",
                            json!({
                                "id": id.clone(),
                                "mode": "degrade_single",
                                "reason": "review gate timeout",
                            }),
                        )
                        .await?;
                        Some(decisions)
                    }
                Err(err) => {
                    self.send_error(id, -32603, format!("review gate failed: {err}"), None)
                        .await?;
                    return Ok(());
                }
            }
        } else {
            None
        };

        let prepared_input = self
            .build_effective_messages(&routing.phase, &chat_params.messages)
            .await?;
        let bypass_cache = matches!(mode, Some(ChatMode::FullAuto));
        let cache_enabled = routing
            .phase
            .options
            .as_ref()
            .and_then(|opts| opts.cache_enabled)
            .unwrap_or(true);

        if !bypass_cache && cache_enabled {
            let cache_ttl = routing
                .phase
                .options
                .as_ref()
                .and_then(|opts| opts.cache_ttl_seconds)
                .unwrap_or(300);

            let cache_key = build_cache_key(
                &routing.phase,
                &prepared_input.messages,
                mode_name,
                approval_strategy.as_str(),
                &routing.phase.agent_names,
            )?;

            if let Some(memory_hit) = self.memory_cache.get(&cache_key) {
                self.metrics.inc_cache_hit();
                self.send_notification(
                    "chat.stream",
                    json!({
                        "id": id.clone(),
                        "agent": memory_hit.agent_name.clone().unwrap_or_else(|| "memory-cache".to_string()),
                        "token": memory_hit.response_text,
                        "cached": true,
                        "cache_level": "memory",
                    }),
                )
                .await?;

                self.send_result(
                    id,
                    json!({
                        "agent": memory_hit.agent_name,
                        "phase": routing.phase.phase_name,
                        "mode": mode_name,
                        "approval_strategy": approval_strategy.as_str(),
                        "cached": true,
                        "cache_level": "memory",
                        "done": true,
                        "reviews": review_decisions,
                    }),
                )
                .await?;
                return Ok(());
            }

            if let Some(cache) = self.cache_handle() {
                self.metrics.inc_cache_lookup();
                if let Some(hit) = self.cache_get(cache.clone(), cache_key.clone()).await? {
                    self.metrics.inc_cache_hit();

                    self.memory_cache.put(
                        cache_key,
                        hit.response_text.clone(),
                        hit.agent_name.clone(),
                        cache_ttl,
                    );

                    self.send_notification(
                        "chat.stream",
                        json!({
                            "id": id.clone(),
                            "agent": hit.agent_name.clone().unwrap_or_else(|| "cache".to_string()),
                            "token": hit.response_text,
                            "cached": true,
                            "cache_level": "sqlite",
                        }),
                    )
                    .await?;

                    self.send_result(
                        id,
                        json!({
                            "agent": hit.agent_name,
                            "phase": routing.phase.phase_name,
                            "mode": mode_name,
                            "approval_strategy": approval_strategy.as_str(),
                            "cached": true,
                            "done": true,
                            "reviews": review_decisions,
                        }),
                    )
                    .await?;
                    return Ok(());
                }
            }
        }

        let phase_name = routing.phase.phase_name.clone();
        let phase_options = routing.phase.options.clone();
        let phase_agent_options = routing
            .phase
            .options
            .as_ref()
            .and_then(|opts| opts.agent_options());
        let phase_principles = routing.phase.principles.clone();
        let phase_agent_names = routing.phase.agent_names.clone();
        let mut errors: Vec<String> = Vec::new();

            let breaker_failure_threshold = extra_u64(
                routing.phase.options.as_ref(),
                "circuit_breaker_failures",
            )
            .unwrap_or(DEFAULT_BREAKER_FAILURE_THRESHOLD as u64)
                as u32;
            let breaker_open_seconds = extra_u64(
                routing.phase.options.as_ref(),
                "circuit_breaker_open_seconds",
            )
            .unwrap_or(DEFAULT_BREAKER_OPEN_SECONDS as u64)
                as i64;

            for (agent_name, agent) in routing.agents {
            match self.circuit_breakers.allow_request(&agent_name) {
                CircuitBreakerAdmission::Closed => {}
                CircuitBreakerAdmission::HalfOpenProbe => {
                    info!("agent '{}' entering half-open probe", agent_name);
                }
                CircuitBreakerAdmission::Rejected {
                    state,
                    retry_after_seconds,
                } => {
                    warn!(
                        "agent '{}' skipped due to circuit breaker state {}",
                        agent_name, state
                    );
                    errors.push(match retry_after_seconds {
                        Some(seconds) => format!(
                            "{}: skipped by circuit breaker ({}, retry after {}s)",
                            agent_name, state, seconds
                        ),
                        None => format!(
                            "{}: skipped by circuit breaker ({})",
                            agent_name, state
                        ),
                    });
                    continue;
                }
            }

            match self
                .run_agent_streaming(
                    id.clone(),
                    agent_name.clone(),
                    agent,
                    prepared_input.messages.clone(),
                    phase_principles.clone(),
                    phase_agent_options.clone(),
                    request_timeout(phase_options.as_ref()),
                )
                .await
            {
                Ok(response_text) => {
                    self.circuit_breakers.record_success(&agent_name);
                    if !bypass_cache && cache_enabled {
                        if let Some(cache) = self.cache_handle() {
                            let cache_key = build_cache_key_from_parts(
                                &phase_name,
                                &prepared_input.messages,
                                phase_principles.as_ref(),
                                phase_options.as_ref(),
                                mode_name,
                                approval_strategy.as_str(),
                                &phase_agent_names,
                            )?;
                            let ttl = phase_options
                                .as_ref()
                                .and_then(|opts| opts.cache_ttl_seconds);
                            self.cache_put(
                                cache.clone(),
                                cache_key,
                                response_text.clone(),
                                agent_name.clone(),
                                ttl,
                            )
                            .await?;
                            self.metrics.inc_cache_store();
                        }

                        let ttl = phase_options
                            .as_ref()
                            .and_then(|opts| opts.cache_ttl_seconds)
                            .unwrap_or(300);
                        self.memory_cache.put(
                            build_cache_key_from_parts(
                                &phase_name,
                                &prepared_input.messages,
                                phase_principles.as_ref(),
                                phase_options.as_ref(),
                                mode_name,
                                approval_strategy.as_str(),
                                &phase_agent_names,
                            )?,
                            response_text.clone(),
                            Some(agent_name.clone()),
                            ttl,
                        );
                    }

                    self.persist_memory_updates(
                        &phase_name,
                        phase_options.as_ref(),
                        prepared_input.latest_user_query.as_deref(),
                        &response_text,
                    )
                    .await?;

                    self.send_result(
                        id,
                        json!({
                            "agent": agent_name,
                            "phase": phase_name,
                            "mode": mode_name,
                            "approval_strategy": approval_strategy.as_str(),
                            "cached": false,
                            "done": true,
                            "reviews": review_decisions,
                        }),
                    )
                    .await?;
                    return Ok(());
                }
                Err(err) => {
                    self.metrics.inc_agent_failures();
                    match classify_agent_failure(&err) {
                        "timeout" => self.metrics.inc_agent_timeout_failures(),
                        "panic" => self.metrics.inc_agent_panic_failures(),
                        _ => self.metrics.inc_agent_other_failures(),
                    }
                    self.circuit_breakers.record_failure_with_config(
                        &agent_name,
                        breaker_failure_threshold,
                        breaker_open_seconds,
                    );
                    warn!("agent '{}' failed: {err:#}", agent_name);
                    errors.push(format!("{}: {}", agent_name, err));
                }
            }
            }

            self.send_error(
                id,
                -32603,
                "all candidate agents failed".to_string(),
                Some(json!({ "errors": errors })),
            )
            .await
        }
        .await;

        self.metrics.observe_chat_latency(started.elapsed());
        result
    }

    fn infer_phase_name_with_flow(
        &self,
        flow: &FlowManager,
        explicit_phase: Option<&str>,
        mode: Option<ChatMode>,
    ) -> String {
        if let Some(phase) = explicit_phase {
            return phase.to_string();
        }

        match mode {
            Some(ChatMode::Ask) if flow.has_phase("review") => "review".to_string(),
            Some(ChatMode::Edit) | Some(ChatMode::Agent) | Some(ChatMode::FullAuto)
                if flow.has_phase("coding") =>
            {
                "coding".to_string()
            }
            _ => flow.default_phase().to_string(),
        }
    }

    async fn build_effective_messages(
        &self,
        phase: &ResolvedPhase,
        messages: &[Message],
    ) -> Result<PreparedChatInput> {
        let vector_config_snapshot = self.vector_config_snapshot();
        let optimized_messages = optimize_messages(messages, phase.options.as_ref());
        let latest_query = latest_user_query(&optimized_messages);
        let mut prepared_messages: Vec<Message> = Vec::new();

        if let Some(vector_store) = self.vector_store_handle() {
            let tuned_state = if let Some(autotune) = self.autotune_handle() {
                Some(autotune_state_snapshot(&autotune).await)
            } else {
                None
            };

            let summary_enabled =
                effective_summary_enabled(phase.options.as_ref(), vector_config_snapshot.as_ref());
            let summary_trigger = effective_summary_trigger_messages(
                phase.options.as_ref(),
                vector_config_snapshot.as_ref(),
            );

            if summary_enabled && optimized_messages.len() >= summary_trigger {
                self.metrics.inc_summary_read();
                if let Some(summary) = self
                    .vector_get_phase_summary(vector_store.clone(), phase.phase_name.clone())
                    .await?
                {
                    self.metrics.inc_summary_hit();
                    prepared_messages.push(Message {
                        role: "user".to_string(),
                        content: format!("Conversation summary for this phase:\n{}", summary),
                    });
                }
            }

            let vector_enabled =
                effective_vector_enabled(phase.options.as_ref(), vector_config_snapshot.as_ref());
            if vector_enabled {
                let vector_auto =
                    effective_vector_auto(phase.options.as_ref(), vector_config_snapshot.as_ref());
                let min_query_chars = effective_vector_min_query_chars(
                    phase.options.as_ref(),
                    vector_config_snapshot.as_ref(),
                    tuned_state.as_ref(),
                );

                if let Some(query) = latest_query.as_ref() {
                    let should_search = if vector_auto {
                        query.chars().count() >= min_query_chars
                    } else {
                        !query.trim().is_empty()
                    };

                    if should_search {
                        self.metrics.inc_vector_search();
                        let top_k = effective_vector_top_k(
                            phase.options.as_ref(),
                            vector_config_snapshot.as_ref(),
                            tuned_state.as_ref(),
                        );
                        let min_similarity = effective_vector_min_similarity(
                            phase.options.as_ref(),
                            vector_config_snapshot.as_ref(),
                        );
                        let max_snippet_chars = effective_vector_max_snippet_chars(
                            phase.options.as_ref(),
                            vector_config_snapshot.as_ref(),
                        );

                        let (hits, feedback) = self
                            .vector_search(
                                vector_store.clone(),
                                phase.phase_name.clone(),
                                query.clone(),
                                top_k,
                                min_similarity,
                                max_snippet_chars,
                            )
                            .await?;

                        // Record precision feedback for autotune if enabled
                        if let Some(autotune) = self.autotune_handle() {
                            if let Some(config) = self.autotune_config_snapshot() {
                                let state_to_persist = {
                                    let mut state = autotune.lock().await;
                                    state.record_vector_search(feedback.avg_similarity, &config);

                                    let mut mutated = false;
                                    if state.advance_cooldown_window(&config) {
                                        mutated = true;
                                    } else if state.should_evaluate(&config) {
                                        state.evaluate_and_adjust(&config);
                                        mutated = true;
                                    }

                                    if mutated {
                                        Some(state.clone())
                                    } else {
                                        None
                                    }
                                };

                                if let Some(state) = state_to_persist {
                                    if let Some(path) = self.autotune_state_path_snapshot() {
                                        if let Err(e) = state.save(path.as_str()) {
                                            warn!("failed to persist autotune state: {}", e);
                                        }
                                    } else {
                                        warn!("autotune update skipped persistence because no resolved state path is available");
                                    }
                                }
                            }
                        }

                        if !hits.is_empty() {
                            self.metrics.inc_vector_hit();
                            prepared_messages.push(Message {
                                role: "user".to_string(),
                                content: build_vector_context_message(&hits),
                            });
                        }
                    }
                }
            }
        }

        prepared_messages.extend(optimized_messages);

        Ok(PreparedChatInput {
            messages: prepared_messages,
            latest_user_query: latest_query,
        })
    }

    async fn persist_memory_updates(
        &self,
        phase_name: &str,
        options: Option<&PhaseOptions>,
        latest_user_query: Option<&str>,
        response_text: &str,
    ) -> Result<()> {
        let vector_config_snapshot = self.vector_config_snapshot();
        let Some(vector_store) = self.vector_store_handle() else {
            return Ok(());
        };

        if let Some(query) = latest_user_query {
            self.vector_upsert(
                vector_store.clone(),
                phase_name.to_string(),
                query.to_string(),
                response_text.to_string(),
            )
            .await?;
            self.metrics.inc_vector_store();
        }

        let summary_enabled = effective_summary_enabled(options, vector_config_snapshot.as_ref());
        if !summary_enabled {
            return Ok(());
        }

        self.metrics.inc_summary_read();
        let existing_summary = self
            .vector_get_phase_summary(vector_store.clone(), phase_name.to_string())
            .await?;
        if existing_summary.is_some() {
            self.metrics.inc_summary_hit();
        }

        let summary_max_chars =
            effective_summary_max_chars(options, vector_config_snapshot.as_ref());
        let new_summary = append_recent_summary(
            existing_summary.as_deref(),
            latest_user_query,
            response_text,
            summary_max_chars,
        );

        self.vector_upsert_phase_summary(vector_store.clone(), phase_name.to_string(), new_summary)
            .await?;
        self.metrics.inc_summary_store();
        Ok(())
    }

    async fn cache_get(
        &self,
        cache: Arc<ResponseCache>,
        cache_key: String,
    ) -> Result<Option<crate::cache::CachedResponse>> {
        spawn_blocking(move || cache.get(&cache_key))
            .await
            .map_err(|e| anyhow::anyhow!("cache_get task join error: {}", e))?
    }

    async fn cache_put(
        &self,
        cache: Arc<ResponseCache>,
        cache_key: String,
        response_text: String,
        agent_name: String,
        ttl: Option<u64>,
    ) -> Result<()> {
        spawn_blocking(move || cache.put(&cache_key, &response_text, &agent_name, ttl))
            .await
            .map_err(|e| anyhow::anyhow!("cache_put task join error: {}", e))?
    }

    async fn cache_entry_count(&self, cache: Arc<ResponseCache>) -> Result<u64> {
        spawn_blocking(move || cache.entry_count())
            .await
            .map_err(|e| anyhow::anyhow!("cache_entry_count task join error: {}", e))?
    }

    async fn cache_clear(&self, cache: Arc<ResponseCache>) -> Result<usize> {
        spawn_blocking(move || cache.clear_all())
            .await
            .map_err(|e| anyhow::anyhow!("cache_clear task join error: {}", e))?
    }

    async fn vector_search(
        &self,
        vector_store: Arc<VectorStore>,
        phase: String,
        query: String,
        top_k: usize,
        min_similarity: f32,
        max_snippet_chars: usize,
    ) -> Result<(Vec<VectorHit>, crate::vector::VectorPrecisionFeedback)> {
        spawn_blocking(move || {
            vector_store.search(&phase, &query, top_k, min_similarity, max_snippet_chars)
        })
        .await
        .map_err(|e| anyhow::anyhow!("vector_search task join error: {}", e))?
    }

    async fn vector_get_phase_summary(
        &self,
        vector_store: Arc<VectorStore>,
        phase: String,
    ) -> Result<Option<String>> {
        spawn_blocking(move || vector_store.get_phase_summary(&phase))
            .await
            .map_err(|e| anyhow::anyhow!("vector_get_phase_summary task join error: {}", e))?
    }

    async fn vector_upsert(
        &self,
        vector_store: Arc<VectorStore>,
        phase: String,
        query: String,
        response_text: String,
    ) -> Result<()> {
        spawn_blocking(move || vector_store.upsert(&phase, &query, &response_text))
            .await
            .map_err(|e| anyhow::anyhow!("vector_upsert task join error: {}", e))?
    }

    async fn vector_entry_counts(&self, vector_store: Arc<VectorStore>) -> Result<(u64, u64)> {
        spawn_blocking(move || {
            let memory = vector_store.memory_entry_count()?;
            let summaries = vector_store.summary_entry_count()?;
            Ok::<(u64, u64), anyhow::Error>((memory, summaries))
        })
        .await
        .map_err(|e| anyhow::anyhow!("vector_entry_counts task join error: {}", e))?
    }

    async fn vector_clear(&self, vector_store: Arc<VectorStore>) -> Result<(usize, usize)> {
        spawn_blocking(move || vector_store.clear_all())
            .await
            .map_err(|e| anyhow::anyhow!("vector_clear task join error: {}", e))?
    }

    async fn vector_upsert_phase_summary(
        &self,
        vector_store: Arc<VectorStore>,
        phase: String,
        summary: String,
    ) -> Result<()> {
        spawn_blocking(move || vector_store.upsert_phase_summary(&phase, &summary))
            .await
            .map_err(|e| anyhow::anyhow!("vector_upsert_phase_summary task join error: {}", e))?
    }

    async fn run_dual_review_gate(
        &self,
        id: Option<Value>,
        messages: &[Message],
        phase_options: Option<&PhaseOptions>,
    ) -> Result<ReviewGateOutcome> {
        let started = Instant::now();
        self.metrics.inc_review_gate();

        let timeout_policy = ReviewTimeoutPolicy::from_options(phase_options);
        let gate_timeout = extra_u64(phase_options, "review_gate_timeout_seconds")
            .or_else(|| phase_options.and_then(|opts| opts.review_timeout_seconds))
            .or_else(|| phase_options.and_then(|opts| opts.request_timeout_seconds))
            .map(Duration::from_secs);
        let gate_deadline = gate_timeout.map(|limit| Instant::now() + limit);

        let result = async {
            let (flow, registry) = self.routing_handles()?;

            let review_routing = flow
                .resolve(Some("review".to_string()), registry.as_ref())
                .map_err(|err| {
                    anyhow::anyhow!("review phase is required for complex full_auto mode: {err}")
                })?;

            let reviewer_names = phase_options
                .and_then(|options| options.full_auto_review_agents.clone())
                .unwrap_or_else(|| review_routing.phase.agent_names.clone());

            let min_reviewers = extra_u64(phase_options, "min_reviewers").unwrap_or(2) as usize;
            let required_approvals = extra_u64(phase_options, "required_approvals")
                .unwrap_or(min_reviewers as u64)
                .max(1) as usize;

            if reviewer_names.len() < min_reviewers {
                anyhow::bail!(
                    "complex full_auto mode requires at least {} review agents",
                    min_reviewers
                );
            }

            let mut prepared_review = self
                .build_effective_messages(&review_routing.phase, messages)
                .await?;
            prepared_review.messages.push(Message {
            role: "user".to_string(),
            content: "Act as a strict execution approval gate. Reply with APPROVE or REJECT on the first line only. After the first line, provide a brief rationale grounded in the review principles.".to_string(),
        });

            let mut decisions = Vec::new();
            let mut approved_count = 0usize;
            let min_review_chars =
                extra_u64(phase_options, "review_min_response_chars").unwrap_or(8) as usize;
            let total_reviewers = reviewer_names.len();
            for (idx, reviewer) in reviewer_names.into_iter().enumerate() {
                if let Some(deadline) = gate_deadline {
                    let now = Instant::now();
                    if now >= deadline {
                        let err = anyhow::anyhow!(
                            "review gate timed out after {}s",
                            gate_timeout.map(|d| d.as_secs()).unwrap_or(0)
                        );
                        self.metrics.inc_review_gate_timeout();
                        record_agent_failure_metrics(self.metrics.as_ref(), &err);

                        return match timeout_policy {
                            ReviewTimeoutPolicy::Reject => {
                                self.metrics.inc_review_gate_rejected();
                                Ok(ReviewGateOutcome::Rejected(decisions))
                            }
                            ReviewTimeoutPolicy::DegradeSingle => {
                                if approved_count >= 1 {
                                    self.metrics.inc_review_gate_degraded();
                                    self.metrics.inc_review_gate_approved();
                                    Ok(ReviewGateOutcome::Degraded(decisions))
                                } else {
                                    self.metrics.inc_review_gate_rejected();
                                    Ok(ReviewGateOutcome::Rejected(decisions))
                                }
                            }
                        };
                    }
                }

                let agent = registry.get(&reviewer).ok_or_else(|| {
                    anyhow::anyhow!("review agent '{}' is not available", reviewer)
                })?;

                let reviewer_timeout = if let Some(deadline) = gate_deadline {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    let configured =
                        review_timeout(review_routing.phase.options.as_ref(), phase_options);
                    match configured {
                        Some(configured_limit) => Some(std::cmp::min(configured_limit, remaining)),
                        None => Some(remaining),
                    }
                } else {
                    review_timeout(review_routing.phase.options.as_ref(), phase_options)
                };

                let response = match self
                    .run_agent_collecting(
                        reviewer.clone(),
                        agent,
                        prepared_review.messages.clone(),
                        review_routing.phase.principles.clone(),
                        review_routing
                            .phase
                            .options
                            .as_ref()
                            .and_then(|opts| opts.agent_options()),
                        reviewer_timeout,
                    )
                    .await
                {
                    Ok(response) => response,
                    Err(err) => {
                        record_agent_failure_metrics(self.metrics.as_ref(), &err);
                        return Err(err);
                    }
                };

                let verdict = review_verdict(&response, min_review_chars);
                if verdict == ReviewVerdict::Invalid {
                    self.metrics.inc_review_gate_invalid_response();
                }
                let decision = ReviewDecision {
                    reviewer: reviewer.clone(),
                    verdict: verdict.as_str().to_string(),
                    response: response.clone(),
                };

                self.send_notification(
                    "chat.review",
                    json!({
                        "id": id.clone(),
                        "reviewer": reviewer,
                        "verdict": decision.verdict,
                    }),
                )
                .await?;

                decisions.push(decision);

                if verdict.is_approved() {
                    approved_count += 1;
                    if approved_count >= required_approvals {
                        self.metrics.inc_review_gate_approved();
                        return Ok(ReviewGateOutcome::Approved(decisions));
                    }
                }

                let remaining = total_reviewers - (idx + 1);
                if approved_count + remaining < required_approvals {
                    self.metrics.inc_review_gate_rejected();
                    return Ok(ReviewGateOutcome::Rejected(decisions));
                }
            }

            if approved_count >= required_approvals {
                self.metrics.inc_review_gate_approved();
                Ok(ReviewGateOutcome::Approved(decisions))
            } else {
                self.metrics.inc_review_gate_rejected();
                Ok(ReviewGateOutcome::Rejected(decisions))
            }
        };

        let output = result.await;
        self.metrics.observe_review_latency(started.elapsed());
        output
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_agent_streaming(
        &self,
        id: Option<Value>,
        agent_name: String,
        agent: Arc<dyn Agent>,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        timeout_limit: Option<Duration>,
    ) -> Result<String> {
        let started = Instant::now();
        let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
        let agent_task =
            tokio::spawn(async move { agent.chat(messages, principles, options, sender).await });

        let mut response_text = String::new();
        let collect_stream = async {
            while let Some(token) = receiver.recv().await {
                response_text.push_str(&token);
                self.send_notification(
                    "chat.stream",
                    json!({
                        "id": id.clone(),
                        "agent": agent_name,
                        "token": token,
                    }),
                )
                .await?;
            }

            Ok::<(), anyhow::Error>(())
        };

        if let Some(limit) = timeout_limit {
            if timeout(limit, collect_stream).await.is_err() {
                agent_task.abort();
                return Err(anyhow::anyhow!(
                    "agent '{}' timed out after {}s",
                    agent_name,
                    limit.as_secs()
                ));
            }
        } else {
            collect_stream.await?;
        }

        let result = match agent_task.await {
            Ok(Ok(())) => Ok(response_text),
            Ok(Err(err)) => Err(err),
            Err(join_err) => Err(anyhow::anyhow!(
                "agent '{}' panic: {}",
                agent_name,
                join_err
            )),
        };

        self.metrics.observe_agent_latency(started.elapsed());
        result
    }

    async fn run_agent_collecting(
        &self,
        agent_name: String,
        agent: Arc<dyn Agent>,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        timeout_limit: Option<Duration>,
    ) -> Result<String> {
        let started = Instant::now();
        let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
        let agent_task =
            tokio::spawn(async move { agent.chat(messages, principles, options, sender).await });

        let mut response_text = String::new();
        let collect_stream = async {
            while let Some(token) = receiver.recv().await {
                response_text.push_str(&token);
            }

            Ok::<(), anyhow::Error>(())
        };

        if let Some(limit) = timeout_limit {
            if timeout(limit, collect_stream).await.is_err() {
                agent_task.abort();
                return Err(anyhow::anyhow!(
                    "agent '{}' timed out after {}s",
                    agent_name,
                    limit.as_secs()
                ));
            }
        } else {
            collect_stream.await?;
        }

        let result = match agent_task.await {
            Ok(Ok(())) => Ok(response_text),
            Ok(Err(err)) => Err(err),
            Err(join_err) => Err(anyhow::anyhow!(
                "agent '{}' panic: {}",
                agent_name,
                join_err
            )),
        };

        self.metrics.observe_review_latency(started.elapsed());
        result
    }

    async fn reload_runtime_config(&self) -> Result<Value> {
        let config_path = self
            .config_path
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("config reload is unavailable: config path not set"))?
            .clone();
        let client = self
            .http_client
            .clone()
            .ok_or_else(|| anyhow::anyhow!("config reload is unavailable: http client not set"))?;

        let new_config = AppConfig::load(&config_path)?;
        let health_report = validate_runtime_readiness(&config_path, &new_config)
            .map_err(|err| anyhow::anyhow!("config reload failed: {err}"))?;
        for warning in &health_report.warnings {
            let severity = match warning.severity {
                crate::config::ConfigWarningSeverity::Critical => "critical",
                crate::config::ConfigWarningSeverity::Warn => "warn",
                crate::config::ConfigWarningSeverity::Info => "info",
            };
            warn!(
                "config reload warning [{}:{}] {}",
                severity, warning.code, warning.message
            );
        }

        let config_arc = Arc::new(new_config);
        let new_registry = Arc::new(AgentRegistry::from_config(Arc::clone(&config_arc), client)?);
        let new_flow = Arc::new(FlowManager::new(
            Arc::clone(&config_arc),
            self.forced_phase.clone(),
        ));

        let new_cache = match &config_arc.cache {
            Some(cache_cfg) if cache_cfg.enabled => {
                let cache_path = if PathBuf::from(&cache_cfg.path).is_absolute() {
                    PathBuf::from(&cache_cfg.path)
                } else {
                    config_path
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new("."))
                        .join(&cache_cfg.path)
                };
                Some(Arc::new(ResponseCache::new(
                    &cache_path,
                    cache_cfg.default_ttl_seconds,
                    cache_cfg.max_entries,
                )?))
            }
            _ => None,
        };

        let new_vector_store = match &config_arc.vector {
            Some(vector_cfg) if vector_cfg.enabled => {
                let vector_path = if PathBuf::from(&vector_cfg.path).is_absolute() {
                    PathBuf::from(&vector_cfg.path)
                } else {
                    config_path
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new("."))
                        .join(&vector_cfg.path)
                };
                Some(Arc::new(VectorStore::new(
                    &vector_path,
                    vector_cfg.dimensions,
                    vector_cfg.max_entries,
                )?))
            }
            _ => None,
        };

        let new_autotune_state_path = config_arc.autotune.as_ref().and_then(|autotune_cfg| {
            if !autotune_cfg.enabled {
                return None;
            }
            Some(
                if PathBuf::from(&autotune_cfg.state_path).is_absolute() {
                    PathBuf::from(&autotune_cfg.state_path)
                } else {
                    config_path
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new("."))
                        .join(&autotune_cfg.state_path)
                }
                .to_string_lossy()
                .to_string(),
            )
        });

        let (new_autotune, new_autotune_config) = match config_arc.autotune.as_ref() {
            Some(autotune_cfg) if autotune_cfg.enabled => {
                let state_path = new_autotune_state_path
                    .clone()
                    .unwrap_or_else(|| "acp_autotune_state.json".to_string());
                let state = AutoTuneState::load_or_default(&state_path, autotune_cfg);
                (
                    Some(Arc::new(Mutex::new(state))),
                    Some(autotune_cfg.clone()),
                )
            }
            _ => (None, None),
        };

        let new_runtime_config = config_arc.runtime.clone().unwrap_or_default();

        {
            let mut flow_guard = self
                .flow
                .lock()
                .map_err(|_| anyhow::anyhow!("flow mutex poisoned"))?;
            *flow_guard = new_flow;
        }
        {
            let mut registry_guard = self
                .registry
                .lock()
                .map_err(|_| anyhow::anyhow!("registry mutex poisoned"))?;
            *registry_guard = new_registry;
        }
        {
            let mut cache_guard = self
                .cache
                .lock()
                .map_err(|_| anyhow::anyhow!("cache mutex poisoned"))?;
            *cache_guard = new_cache;
        }
        {
            let mut vector_store_guard = self
                .vector_store
                .lock()
                .map_err(|_| anyhow::anyhow!("vector_store mutex poisoned"))?;
            *vector_store_guard = new_vector_store;
        }
        {
            let mut vector_cfg_guard = self
                .vector_config
                .lock()
                .map_err(|_| anyhow::anyhow!("vector_config mutex poisoned"))?;
            *vector_cfg_guard = config_arc.vector.clone();
        }
        {
            let mut autotune_guard = self
                .autotune
                .lock()
                .map_err(|_| anyhow::anyhow!("autotune mutex poisoned"))?;
            *autotune_guard = new_autotune;
        }
        {
            let mut autotune_cfg_guard = self
                .autotune_config
                .lock()
                .map_err(|_| anyhow::anyhow!("autotune_config mutex poisoned"))?;
            *autotune_cfg_guard = new_autotune_config;
        }
        {
            let mut autotune_path_guard = self
                .autotune_state_path
                .lock()
                .map_err(|_| anyhow::anyhow!("autotune_state_path mutex poisoned"))?;
            *autotune_path_guard = new_autotune_state_path;
        }
        {
            let mut runtime_guard = self
                .runtime_config
                .lock()
                .map_err(|_| anyhow::anyhow!("runtime_config mutex poisoned"))?;
            *runtime_guard = new_runtime_config;
        }

        // Clear dynamic guardrails to avoid stale state after topology changes.
        if let Ok(mut g) = self.circuit_breakers.inner.lock() {
            g.clear();
        }
        if let Ok(mut g) = self.phase_rate_limiter.inner.lock() {
            g.clear();
        }
        self.inflight_limiter.clear();

        Ok(json!({
            "ok": true,
            "note": "flow/registry/cache/vector/autotune resources reloaded",
            "path": config_path,
            "warning_count": health_report.total,
            "warnings": health_report.warning_messages(),
            "health": health_report,
        }))
    }

    async fn send_result(&self, id: Option<Value>, result: Value) -> Result<()> {
        self.write_response(JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        })
        .await
    }

    async fn send_error(
        &self,
        id: Option<Value>,
        code: i64,
        message: String,
        data: Option<Value>,
    ) -> Result<()> {
        self.write_response(JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data,
            }),
        })
        .await
    }

    async fn send_notification(&self, method: &str, params: Value) -> Result<()> {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_json_line(&payload).await
    }

    async fn write_response(&self, response: JsonRpcResponse) -> Result<()> {
        let value = serde_json::to_value(response)?;
        self.write_json_line(&value).await
    }

    async fn write_json_line(&self, value: &Value) -> Result<()> {
        let mut stdout = self.output.lock().await;
        let mut encoded = serde_json::to_vec(value)?;
        encoded.push(b'\n');
        stdout.write_all(&encoded).await?;
        stdout.flush().await?;
        Ok(())
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct MaintenanceCycleResult {
    memory_expired_removed: usize,
    sqlite_expired_removed: usize,
}

#[allow(clippy::too_many_arguments)]
async fn run_background_maintenance_loop(
    runtime_config: Arc<StdMutex<RuntimeConfig>>,
    memory_cache: Arc<MemoryResponseCache>,
    cache: Arc<StdMutex<Option<Arc<ResponseCache>>>>,
    vector_store: Arc<StdMutex<Option<Arc<VectorStore>>>>,
    maintenance: Arc<MaintenanceTracker>,
    lifecycle: Arc<LifecycleState>,
    circuit_breakers: Arc<CircuitBreakerRegistry>,
    phase_rate_limiter: Arc<PhaseRateLimiter>,
    inflight_limiter: Arc<InflightLimiter>,
    shutdown_notify: Arc<Notify>,
) {
    let config = runtime_config
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    let mut maintenance_interval = tokio::time::interval(Duration::from_secs(
        config.maintenance_interval_seconds.max(1),
    ));
    let mut health_interval =
        tokio::time::interval(Duration::from_secs(config.health_interval_seconds.max(1)));
    maintenance_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    health_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown_notify.notified() => break,
            _ = maintenance_interval.tick() => {
                if lifecycle.is_shutting_down() {
                    break;
                }

                if let Err(err) = perform_maintenance_cycle(
                    Arc::clone(&memory_cache),
                    Arc::clone(&cache),
                    Arc::clone(&maintenance),
                    "background",
                ).await {
                    warn!("background maintenance cycle failed: {}", err);
                }
            }
            _ = health_interval.tick() => {
                if lifecycle.is_shutting_down() {
                    break;
                }

                log_background_health(
                    Arc::clone(&memory_cache),
                    Arc::clone(&cache),
                    Arc::clone(&vector_store),
                    Arc::clone(&circuit_breakers),
                    Arc::clone(&phase_rate_limiter),
                    Arc::clone(&inflight_limiter),
                    Arc::clone(&lifecycle),
                    Arc::clone(&maintenance),
                ).await;
            }
        }
    }
}

async fn perform_maintenance_cycle(
    memory_cache: Arc<MemoryResponseCache>,
    cache: Arc<StdMutex<Option<Arc<ResponseCache>>>>,
    maintenance: Arc<MaintenanceTracker>,
    source: &str,
) -> Result<MaintenanceCycleResult> {
    maintenance.note_started();

    let memory_expired_removed = memory_cache.purge_expired();
    let cache_handle = cache.lock().ok().and_then(|guard| guard.clone());
    let sqlite_expired_removed_result = if let Some(cache) = cache_handle {
        spawn_blocking(move || cache.purge_expired())
            .await
            .map_err(|e| anyhow::anyhow!("cache purge task join error: {}", e))?
    } else {
        Ok(0)
    };
    let sqlite_expired_removed = match sqlite_expired_removed_result {
        Ok(value) => value,
        Err(err) => {
            maintenance.note_failed(&err.to_string());
            return Err(err);
        }
    };

    let result = MaintenanceCycleResult {
        memory_expired_removed,
        sqlite_expired_removed,
    };

    maintenance.note_completed(memory_expired_removed, sqlite_expired_removed);
    info!(
        "maintenance cycle '{}' completed (memory_removed={}, sqlite_removed={})",
        source, result.memory_expired_removed, result.sqlite_expired_removed
    );
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
async fn log_background_health(
    memory_cache: Arc<MemoryResponseCache>,
    cache: Arc<StdMutex<Option<Arc<ResponseCache>>>>,
    vector_store: Arc<StdMutex<Option<Arc<VectorStore>>>>,
    circuit_breakers: Arc<CircuitBreakerRegistry>,
    phase_rate_limiter: Arc<PhaseRateLimiter>,
    inflight_limiter: Arc<InflightLimiter>,
    lifecycle: Arc<LifecycleState>,
    maintenance: Arc<MaintenanceTracker>,
) {
    let sqlite_cache_entries =
        if let Some(cache) = cache.lock().ok().and_then(|guard| guard.clone()) {
            match spawn_blocking(move || cache.entry_count()).await {
                Ok(Ok(count)) => Some(count),
                Ok(Err(err)) => {
                    warn!(
                        "background health failed to read sqlite cache entries: {}",
                        err
                    );
                    None
                }
                Err(err) => {
                    warn!("background health cache count task failed: {}", err);
                    None
                }
            }
        } else {
            None
        };

    let vector_counts =
        if let Some(store) = vector_store.lock().ok().and_then(|guard| guard.clone()) {
            match spawn_blocking(move || {
                Ok::<(u64, u64), anyhow::Error>((
                    store.memory_entry_count()?,
                    store.summary_entry_count()?,
                ))
            })
            .await
            {
                Ok(Ok(counts)) => Some(counts),
                Ok(Err(err)) => {
                    warn!("background health failed to read vector counts: {}", err);
                    None
                }
                Err(err) => {
                    warn!("background health vector count task failed: {}", err);
                    None
                }
            }
        } else {
            None
        };

    let (global_inflight, phase_inflight) = inflight_limiter.snapshot();
    let lifecycle_snapshot = lifecycle.snapshot();
    let maintenance_snapshot = maintenance.snapshot();

    info!(
        "runtime health: shutting_down={}, inflight_global={}, inflight_phases={}, memory_cache_entries={}, sqlite_cache_entries={:?}, vector_counts={:?}, breaker_open={}, breaker_half_open={}, rate_limiter_tracked={}, maintenance_running={}, maintenance_cycles={}",
        lifecycle_snapshot.shutting_down,
        global_inflight,
        phase_inflight.len(),
        memory_cache.active_entries(),
        sqlite_cache_entries,
        vector_counts,
        circuit_breakers.open_count(),
        circuit_breakers.half_open_count(),
        phase_rate_limiter.tracked_phases(),
        maintenance_snapshot.running,
        maintenance_snapshot.cycles_total,
    );
}

fn request_timeout(options: Option<&PhaseOptions>) -> Option<Duration> {
    options
        .and_then(|opts| opts.request_timeout_seconds)
        .map(Duration::from_secs)
}

async fn autotune_state_snapshot(autotune: &Arc<Mutex<AutoTuneState>>) -> AutoTuneState {
    autotune.lock().await.clone()
}

fn review_timeout(
    review_options: Option<&PhaseOptions>,
    primary_phase_options: Option<&PhaseOptions>,
) -> Option<Duration> {
    review_options
        .and_then(|opts| opts.review_timeout_seconds.or(opts.request_timeout_seconds))
        .or_else(|| {
            primary_phase_options
                .and_then(|opts| opts.review_timeout_seconds.or(opts.request_timeout_seconds))
        })
        .map(Duration::from_secs)
}

fn effective_vector_enabled(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> bool {
    options
        .and_then(|opts| opts.vector_enabled)
        .or_else(|| vector_config.map(|cfg| cfg.enabled))
        .unwrap_or(true)
}

fn effective_vector_auto(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> bool {
    options
        .and_then(|opts| opts.vector_auto)
        .or_else(|| vector_config.map(|cfg| cfg.auto_mode))
        .unwrap_or(true)
}

fn effective_vector_min_query_chars(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
    autotune_state: Option<&AutoTuneState>,
) -> usize {
    autotune_state
        .map(|state| state.current_min_query_chars)
        .or_else(|| options.and_then(|opts| opts.vector_min_query_chars))
        .or_else(|| vector_config.map(|cfg| cfg.min_query_chars))
        .unwrap_or(DEFAULT_VECTOR_MIN_QUERY_CHARS)
}

fn effective_vector_top_k(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
    autotune_state: Option<&AutoTuneState>,
) -> usize {
    autotune_state
        .map(|state| state.current_top_k)
        .or_else(|| options.and_then(|opts| opts.vector_top_k))
        .or_else(|| vector_config.map(|cfg| cfg.top_k))
        .unwrap_or(DEFAULT_VECTOR_TOP_K)
}

fn effective_vector_min_similarity(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> f32 {
    options
        .and_then(|opts| opts.vector_min_similarity)
        .or_else(|| vector_config.map(|cfg| cfg.min_similarity))
        .unwrap_or(DEFAULT_VECTOR_MIN_SIMILARITY)
}

fn effective_vector_max_snippet_chars(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> usize {
    options
        .and_then(|opts| opts.vector_max_snippet_chars)
        .or_else(|| vector_config.map(|cfg| cfg.max_snippet_chars))
        .unwrap_or(DEFAULT_VECTOR_MAX_SNIPPET_CHARS)
}

fn effective_summary_enabled(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> bool {
    options
        .and_then(|opts| opts.summary_enabled)
        .or_else(|| vector_config.map(|cfg| cfg.summary_enabled))
        .unwrap_or(true)
}

fn effective_summary_trigger_messages(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> usize {
    options
        .and_then(|opts| opts.summary_trigger_messages)
        .or_else(|| vector_config.map(|cfg| cfg.summary_trigger_messages))
        .unwrap_or(DEFAULT_SUMMARY_TRIGGER_MESSAGES)
}

fn effective_summary_max_chars(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> usize {
    options
        .and_then(|opts| opts.summary_max_chars)
        .or_else(|| vector_config.map(|cfg| cfg.summary_max_chars))
        .unwrap_or(DEFAULT_SUMMARY_MAX_CHARS)
}

fn optimize_messages(messages: &[Message], options: Option<&PhaseOptions>) -> Vec<Message> {
    let mut trimmed = messages.to_vec();

    if let Some(max_messages) = options.and_then(|opts| opts.max_history_messages) {
        if trimmed.len() > max_messages {
            trimmed = trimmed[trimmed.len() - max_messages..].to_vec();
        }
    }

    if let Some(max_chars) = options.and_then(|opts| opts.max_history_chars) {
        let mut kept_reversed = Vec::new();
        let mut total_chars = 0usize;

        for message in trimmed.iter().rev() {
            let message_chars = message.content.chars().count();
            if !kept_reversed.is_empty() && total_chars + message_chars > max_chars {
                break;
            }

            kept_reversed.push(message.clone());
            total_chars += message_chars;
        }

        kept_reversed.reverse();
        trimmed = kept_reversed;
    }

    trimmed
}

fn latest_user_query(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.role.eq_ignore_ascii_case("user"))
        .map(|message| message.content.trim().to_string())
        .filter(|content| !content.is_empty())
}

fn build_vector_context_message(hits: &[VectorHit]) -> String {
    let normalized = dedupe_vector_hits(hits);
    let mut content = String::from("Relevant prior context from similar requests:\n");
    for (index, hit) in normalized.iter().enumerate() {
        content.push_str(&format!(
            "{}. [similarity {:.2}] {}\n",
            index + 1,
            hit.similarity,
            hit.response_snippet
        ));
    }
    content
}

fn append_recent_summary(
    existing_summary: Option<&str>,
    latest_user_query: Option<&str>,
    response_text: &str,
    max_chars: usize,
) -> String {
    let mut segments: Vec<String> = Vec::new();
    if let Some(existing) = existing_summary {
        if !existing.trim().is_empty() {
            segments.push(existing.trim().to_string());
        }
    }
    if let Some(query) = latest_user_query {
        segments.push(format!("User focus: {}", query.trim()));
    }
    if !response_text.trim().is_empty() {
        segments.push(format!("Latest response: {}", response_text.trim()));
    }

    trim_to_tail_chars(&segments.join("\n\n"), max_chars)
}

fn trim_to_tail_chars(input: &str, max_chars: usize) -> String {
    let chars: Vec<char> = input.chars().collect();
    if chars.len() <= max_chars {
        return input.to_string();
    }

    chars[chars.len() - max_chars..].iter().collect()
}

fn build_cache_key(
    phase: &ResolvedPhase,
    messages: &[Message],
    mode_name: &str,
    approval_strategy: &str,
    agent_names: &[String],
) -> Result<String> {
    build_cache_key_from_parts(
        &phase.phase_name,
        messages,
        phase.principles.as_ref(),
        phase.options.as_ref(),
        mode_name,
        approval_strategy,
        agent_names,
    )
}

fn build_cache_key_from_parts(
    phase_name: &str,
    messages: &[Message],
    principles: Option<&Vec<String>>,
    options: Option<&PhaseOptions>,
    mode_name: &str,
    approval_strategy: &str,
    agent_names: &[String],
) -> Result<String> {
    let payload = json!({
        "phase": phase_name,
        "messages": messages,
        "principles": principles,
        "options": options,
        "mode": mode_name,
        "approval_strategy": approval_strategy,
        "agents": agent_names,
    });

    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&payload)?);
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn review_verdict(response: &str, min_response_chars: usize) -> ReviewVerdict {
    if response.trim().chars().count() < min_response_chars {
        return ReviewVerdict::Invalid;
    }

    let first_line = response.lines().find(|line| !line.trim().is_empty());
    match first_line.map(|line| line.trim().to_ascii_uppercase()) {
        Some(value) if value.starts_with("APPROVE") => ReviewVerdict::Approve,
        Some(value) if value.starts_with("REJECT") => ReviewVerdict::Reject,
        _ => ReviewVerdict::Invalid,
    }
}

fn dedupe_vector_hits(hits: &[VectorHit]) -> Vec<VectorHit> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for hit in hits {
        let key = hit
            .response_snippet
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        if seen.insert(key) {
            out.push(hit.clone());
        }
    }
    out
}

fn extra_u64(options: Option<&PhaseOptions>, key: &str) -> Option<u64> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_u64())
}

fn extra_f64(options: Option<&PhaseOptions>, key: &str) -> Option<f64> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_f64())
}

fn observe_latency_histogram(
    duration: Duration,
    count: &mut u64,
    sum_seconds: &mut f64,
    buckets: &mut [u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
) {
    let value = duration.as_secs_f64();
    *count += 1;
    *sum_seconds += value;

    let mut idx = HISTOGRAM_BUCKETS_SECONDS.len();
    for (i, bound) in HISTOGRAM_BUCKETS_SECONDS.iter().enumerate() {
        if value <= *bound {
            idx = i;
            break;
        }
    }
    buckets[idx] = buckets[idx].saturating_add(1);
}

fn histogram_prometheus_lines(
    name: &str,
    count: u64,
    sum_seconds: f64,
    buckets: &[u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
) -> Vec<String> {
    let mut lines = Vec::new();
    push_metric_header(
        &mut lines,
        name,
        "histogram",
        "ACP latency distribution in seconds",
    );
    let mut cumulative = 0_u64;
    for (idx, le) in HISTOGRAM_BUCKETS_SECONDS.iter().enumerate() {
        cumulative = cumulative.saturating_add(buckets[idx]);
        lines.push(format!("{}_bucket{{le=\"{}\"}} {}", name, le, cumulative));
    }
    cumulative = cumulative.saturating_add(buckets[HISTOGRAM_BUCKETS_SECONDS.len()]);
    lines.push(format!("{}_bucket{{le=\"+Inf\"}} {}", name, cumulative));
    lines.push(format!("{}_sum {}", name, sum_seconds));
    lines.push(format!("{}_count {}", name, count));
    lines
}

fn classify_agent_failure(err: &anyhow::Error) -> &'static str {
    let msg = err.to_string().to_ascii_lowercase();
    if msg.contains("timed out") || msg.contains("timeout") {
        return "timeout";
    }
    if msg.contains("panic") {
        return "panic";
    }
    "other"
}

fn record_agent_failure_metrics(metrics: &RuntimeMetrics, err: &anyhow::Error) {
    metrics.inc_agent_failures();
    match classify_agent_failure(err) {
        "timeout" => metrics.inc_agent_timeout_failures(),
        "panic" => metrics.inc_agent_panic_failures(),
        _ => metrics.inc_agent_other_failures(),
    }
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn push_metric_header(lines: &mut Vec<String>, name: &str, metric_type: &str, help: &str) {
    lines.push(format!("# HELP {} {}", name, help));
    lines.push(format!("# TYPE {} {}", name, metric_type));
}

fn push_scalar_metric(
    lines: &mut Vec<String>,
    name: &str,
    metric_type: &str,
    help: &str,
    value: impl std::fmt::Display,
) {
    push_metric_header(lines, name, metric_type, help);
    lines.push(format!("{} {}", name, value));
}

fn escape_prometheus_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn build_prometheus_metrics(
    snapshot: &MetricsSnapshot,
    gauges: &RuntimeGaugeSnapshot,
    breaker_snapshot: &HashMap<String, CircuitBreakerSnapshot>,
    phase_limiter_snapshot: &HashMap<String, (f64, f64)>,
    inflight_snapshot: &(usize, HashMap<String, usize>),
    lifecycle: &LifecycleSnapshot,
    maintenance: &MaintenanceSnapshot,
) -> String {
    let mut lines = Vec::new();
    push_scalar_metric(
        &mut lines,
        "acp_chat_requests_total",
        "counter",
        "Total ACP chat requests handled",
        snapshot.chat_requests_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_cache_lookup_total",
        "counter",
        "Total cache lookups performed",
        snapshot.cache_lookup_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_cache_hit_total",
        "counter",
        "Total cache hits served",
        snapshot.cache_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_cache_store_total",
        "counter",
        "Total cache writes performed",
        snapshot.cache_store_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_search_total",
        "counter",
        "Total vector searches performed",
        snapshot.vector_search_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_hit_total",
        "counter",
        "Total vector retrieval hits",
        snapshot.vector_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_store_total",
        "counter",
        "Total vector memory writes",
        snapshot.vector_store_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_summary_read_total",
        "counter",
        "Total summary memory reads",
        snapshot.summary_read_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_summary_hit_total",
        "counter",
        "Total summary memory hits",
        snapshot.summary_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_summary_store_total",
        "counter",
        "Total summary memory writes",
        snapshot.summary_store_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_agent_failures_total",
        "counter",
        "Total agent execution failures",
        snapshot.agent_failures_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_agent_timeout_failures_total",
        "counter",
        "Total agent timeout failures",
        snapshot.agent_timeout_failures_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_agent_panic_failures_total",
        "counter",
        "Total agent panic failures",
        snapshot.agent_panic_failures_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_agent_other_failures_total",
        "counter",
        "Total uncategorized agent failures",
        snapshot.agent_other_failures_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_total",
        "counter",
        "Total review gate evaluations",
        snapshot.review_gate_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_approved_total",
        "counter",
        "Total review gate approvals",
        snapshot.review_gate_approved_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_rejected_total",
        "counter",
        "Total review gate rejections",
        snapshot.review_gate_rejected_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_timeout_total",
        "counter",
        "Total review gate deadline timeouts",
        snapshot.review_gate_timeout_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_degraded_total",
        "counter",
        "Total review gate approvals degraded after timeout",
        snapshot.review_gate_degraded_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_invalid_response_total",
        "counter",
        "Total invalid review gate responses",
        snapshot.review_gate_invalid_response_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_memory_cache_entries",
        "gauge",
        "Current in-memory cache entries",
        gauges.memory_cache_entries,
    );
    push_scalar_metric(
        &mut lines,
        "acp_sqlite_cache_entries",
        "gauge",
        "Current SQLite cache entries",
        gauges.sqlite_cache_entries,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_memory_entries",
        "gauge",
        "Current vector memory entries",
        gauges.vector_memory_entries,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_summary_entries",
        "gauge",
        "Current vector summary entries",
        gauges.vector_summary_entries,
    );
    push_scalar_metric(
        &mut lines,
        "acp_circuit_open_agents",
        "gauge",
        "Current open circuit breaker agents",
        gauges.circuit_open_agents,
    );
    push_scalar_metric(
        &mut lines,
        "acp_circuit_half_open_agents",
        "gauge",
        "Current half-open circuit breaker agents",
        gauges.circuit_half_open_agents,
    );
    push_scalar_metric(
        &mut lines,
        "acp_circuit_tracked_agents",
        "gauge",
        "Current tracked circuit breaker agents",
        gauges.circuit_tracked_agents,
    );
    push_scalar_metric(
        &mut lines,
        "acp_rate_limiter_tracked_phases",
        "gauge",
        "Current tracked phases with rate limiter state",
        gauges.rate_limiter_tracked_phases,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lifecycle_shutting_down",
        "gauge",
        "Whether the ACP server is shutting down",
        if lifecycle.shutting_down { 1 } else { 0 },
    );
    push_scalar_metric(
        &mut lines,
        "acp_maintenance_cycles_total",
        "counter",
        "Total maintenance cycles executed",
        maintenance.cycles_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_maintenance_running",
        "gauge",
        "Whether a maintenance cycle is currently running",
        if maintenance.running { 1 } else { 0 },
    );

    push_metric_header(
        &mut lines,
        "acp_inflight_requests",
        "gauge",
        "Current in-flight request count by scope",
    );
    lines.push(format!(
        "acp_inflight_requests{{scope=\"global\"}} {}",
        inflight_snapshot.0
    ));
    for (phase, count) in inflight_snapshot.1.iter() {
        lines.push(format!(
            "acp_inflight_requests{{scope=\"phase\",phase=\"{}\"}} {}",
            escape_prometheus_label(phase),
            count
        ));
    }

    push_metric_header(
        &mut lines,
        "acp_phase_rate_limiter_tokens",
        "gauge",
        "Current token bucket tokens by phase",
    );
    push_metric_header(
        &mut lines,
        "acp_phase_rate_limiter_capacity",
        "gauge",
        "Current token bucket capacity by phase",
    );
    for (phase, (tokens, capacity)) in phase_limiter_snapshot.iter() {
        let phase = escape_prometheus_label(phase);
        lines.push(format!(
            "acp_phase_rate_limiter_tokens{{phase=\"{}\"}} {:.3}",
            phase, tokens
        ));
        lines.push(format!(
            "acp_phase_rate_limiter_capacity{{phase=\"{}\"}} {:.3}",
            phase, capacity
        ));
    }

    push_metric_header(
        &mut lines,
        "acp_circuit_breaker_state",
        "gauge",
        "Current circuit breaker state per agent",
    );
    push_metric_header(
        &mut lines,
        "acp_circuit_breaker_failures",
        "gauge",
        "Current consecutive failures per agent",
    );
    for (agent, state) in breaker_snapshot.iter() {
        let agent = escape_prometheus_label(agent);
        for stage in ["closed", "open", "half_open", "half_open_ready"] {
            let value = if state.state == stage { 1 } else { 0 };
            lines.push(format!(
                "acp_circuit_breaker_state{{agent=\"{}\",state=\"{}\"}} {}",
                agent, stage, value
            ));
        }
        lines.push(format!(
            "acp_circuit_breaker_failures{{agent=\"{}\"}} {}",
            agent, state.consecutive_failures
        ));
    }

    lines.extend(histogram_prometheus_lines(
        "acp_chat_latency_seconds",
        snapshot.chat_latency_count,
        snapshot.chat_latency_sum_seconds,
        &snapshot.chat_latency_bucket_counts,
    ));
    lines.extend(histogram_prometheus_lines(
        "acp_agent_latency_seconds",
        snapshot.agent_latency_count,
        snapshot.agent_latency_sum_seconds,
        &snapshot.agent_latency_bucket_counts,
    ));
    lines.extend(histogram_prometheus_lines(
        "acp_review_latency_seconds",
        snapshot.review_latency_count,
        snapshot.review_latency_sum_seconds,
        &snapshot.review_latency_bucket_counts,
    ));

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::*;
    use crate::config::{AgentConfig, AppConfig, FlowConfig, PhaseConfig};

    fn vector_config_fixture() -> VectorConfig {
        VectorConfig {
            enabled: true,
            auto_mode: false,
            path: "vector.sqlite3".to_string(),
            dimensions: 192,
            min_query_chars: 140,
            top_k: 4,
            min_similarity: 0.91,
            max_snippet_chars: 640,
            max_entries: 1000,
            summary_enabled: false,
            summary_trigger_messages: 12,
            summary_max_chars: 1500,
        }
    }

    fn phase_inference_server(default_phase: &str, phase_names: &[&str]) -> AcpServer {
        let mut agents = HashMap::new();
        agents.insert(
            "copilot".to_string(),
            AgentConfig {
                agent_type: "copilot".to_string(),
                url: Some("http://127.0.0.1:8080".to_string()),
                chat_path: None,
                api_key_env: None,
                secret_key_env: None,
                anthropic_version: None,
                model: None,
                max_tokens: None,
                supports_system: None,
            },
        );

        let phases = phase_names
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    PhaseConfig {
                        description: format!("{} phase", name),
                        agents: vec!["copilot".to_string()],
                        fallback: Some(true),
                        principles: None,
                        options: None,
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        let config = Arc::new(AppConfig {
            default_phase: default_phase.to_string(),
            agents,
            flow: FlowConfig {
                name: "test-flow".to_string(),
                phases: phase_names.iter().map(|name| (*name).to_string()).collect(),
            },
            phases,
            runtime: Some(RuntimeConfig::default()),
            cache: None,
            vector: None,
            autotune: None,
        });

        let flow = Arc::new(FlowManager::new(Arc::clone(&config), None));
        let registry = Arc::new(
            AgentRegistry::from_config(Arc::clone(&config), reqwest::Client::new())
                .expect("test registry should build"),
        );

        AcpServer::new(
            flow,
            registry,
            None,
            None,
            None,
            None,
            None,
            None,
            RuntimeConfig::default(),
            None,
            None,
            None,
            false,
        )
    }

    fn phase_inference_flow(default_phase: &str, phase_names: &[&str]) -> FlowManager {
        let phases = phase_names
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    PhaseConfig {
                        description: format!("{} phase", name),
                        agents: vec!["copilot".to_string()],
                        fallback: Some(true),
                        principles: None,
                        options: None,
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        FlowManager::new(
            Arc::new(AppConfig {
                default_phase: default_phase.to_string(),
                agents: HashMap::from([(
                    "copilot".to_string(),
                    AgentConfig {
                        agent_type: "copilot".to_string(),
                        url: Some("http://127.0.0.1:8080".to_string()),
                        chat_path: None,
                        api_key_env: None,
                        secret_key_env: None,
                        anthropic_version: None,
                        model: None,
                        max_tokens: None,
                        supports_system: None,
                    },
                )]),
                flow: FlowConfig {
                    name: "test-flow".to_string(),
                    phases: phase_names.iter().map(|name| (*name).to_string()).collect(),
                },
                phases,
                runtime: Some(RuntimeConfig::default()),
                cache: None,
                vector: None,
                autotune: None,
            }),
            None,
        )
    }

    #[test]
    fn chat_mode_parsing() {
        assert_eq!(ChatMode::parse(Some("ask")), Some(ChatMode::Ask));
        assert_eq!(ChatMode::parse(Some("edit")), Some(ChatMode::Edit));
        assert_eq!(ChatMode::parse(Some("agent")), Some(ChatMode::Agent));
        assert_eq!(ChatMode::parse(Some("full_auto")), Some(ChatMode::FullAuto));
        assert_eq!(ChatMode::parse(Some("FULL-AUTO")), Some(ChatMode::FullAuto));
        assert_eq!(ChatMode::parse(Some("unknown")), None);
        assert_eq!(ChatMode::parse(None), None);
    }

    #[test]
    fn autopilot_complexity_parsing() {
        assert_eq!(
            AutopilotComplexity::from_str("simple"),
            Some(AutopilotComplexity::Simple)
        );
        assert_eq!(
            AutopilotComplexity::from_str("complex"),
            Some(AutopilotComplexity::Complex)
        );
        assert_eq!(
            AutopilotComplexity::from_str("SIMPLE"),
            Some(AutopilotComplexity::Simple)
        );
        assert_eq!(AutopilotComplexity::from_str("unknown"), None);
    }

    #[test]
    fn mode_to_strategy_mapping() {
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::Ask), None),
            ApprovalStrategy::DefaultApprovals
        );
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::Edit), None),
            ApprovalStrategy::ByPassApproval
        );
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::Agent), None),
            ApprovalStrategy::ByPassApproval
        );
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::FullAuto), Some(AutopilotComplexity::Simple)),
            ApprovalStrategy::AutoPilotSimple
        );
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::FullAuto), Some(AutopilotComplexity::Complex)),
            ApprovalStrategy::AutoPilotComplex
        );
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::FullAuto), None),
            ApprovalStrategy::AutoPilotSimple
        );
        assert_eq!(
            mode_to_approval_strategy(None, None),
            ApprovalStrategy::DefaultApprovals
        );
    }

    #[test]
    fn infer_phase_prefers_explicit_phase_over_mode_default() {
        let server = phase_inference_server("planning", &["planning", "review", "coding"]);
        let flow = phase_inference_flow("planning", &["planning", "review", "coding"]);

        assert_eq!(
            server.infer_phase_name_with_flow(&flow, Some("delivery"), Some(ChatMode::Ask)),
            "delivery"
        );
    }

    #[test]
    fn infer_phase_uses_review_for_ask_when_available() {
        let server = phase_inference_server("planning", &["planning", "review"]);
        let flow = phase_inference_flow("planning", &["planning", "review"]);

        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::Ask)),
            "review"
        );
    }

    #[test]
    fn infer_phase_uses_coding_for_edit_agent_and_full_auto() {
        let server = phase_inference_server("planning", &["planning", "coding"]);
        let flow = phase_inference_flow("planning", &["planning", "coding"]);

        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::Edit)),
            "coding"
        );
        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::Agent)),
            "coding"
        );
        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::FullAuto)),
            "coding"
        );
    }

    #[test]
    fn infer_phase_falls_back_to_default_when_mode_phase_missing() {
        let server = phase_inference_server("planning", &["planning"]);
        let flow = phase_inference_flow("planning", &["planning"]);

        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::Ask)),
            "planning"
        );
        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::FullAuto)),
            "planning"
        );
    }

    #[test]
    fn approval_strategy_dual_review_check() {
        assert!(!ApprovalStrategy::DefaultApprovals.needs_dual_review());
        assert!(!ApprovalStrategy::ByPassApproval.needs_dual_review());
        assert!(!ApprovalStrategy::AutoPilotSimple.needs_dual_review());
        assert!(ApprovalStrategy::AutoPilotComplex.needs_dual_review());
    }

    #[test]
    fn optimize_messages_respects_limits() {
        let options = PhaseOptions {
            max_history_messages: Some(2),
            max_history_chars: Some(10),
            ..PhaseOptions::default()
        };
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: "12345".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: "67890".to_string(),
            },
            Message {
                role: "user".to_string(),
                content: "abc".to_string(),
            },
        ];

        let optimized = optimize_messages(&messages, Some(&options));
        assert_eq!(optimized.len(), 2);
        assert_eq!(optimized[0].content, "67890");
        assert_eq!(optimized[1].content, "abc");
    }

    #[test]
    fn append_recent_summary_keeps_recent_tail() {
        let summary =
            append_recent_summary(Some("old summary"), Some("new question"), "new answer", 24);

        assert!(summary.contains("new answer"));
    }

    #[test]
    fn review_verdict_requires_approve_first_line() {
        assert_eq!(
            review_verdict("APPROVE\nLooks safe.", 8),
            ReviewVerdict::Approve
        );
        assert_eq!(
            review_verdict("REJECT\nMissing tests.", 8),
            ReviewVerdict::Reject
        );
        assert_eq!(
            review_verdict("Looks fine, APPROVE", 8),
            ReviewVerdict::Invalid
        );
        assert_eq!(review_verdict("OK", 8), ReviewVerdict::Invalid);
    }

    #[test]
    fn review_timeout_prefers_review_phase_override() {
        let review_options = PhaseOptions {
            review_timeout_seconds: Some(15),
            request_timeout_seconds: Some(30),
            ..PhaseOptions::default()
        };
        let primary_options = PhaseOptions {
            review_timeout_seconds: Some(20),
            request_timeout_seconds: Some(40),
            ..PhaseOptions::default()
        };

        let timeout = review_timeout(Some(&review_options), Some(&primary_options));
        assert_eq!(timeout.map(|value| value.as_secs()), Some(15));
    }

    #[test]
    fn vector_defaults_fall_back_to_global_config() {
        let vector_config = vector_config_fixture();

        assert!(!effective_vector_auto(None, Some(&vector_config)));
        assert_eq!(
            effective_vector_min_query_chars(None, Some(&vector_config), None),
            140
        );
        assert_eq!(effective_vector_top_k(None, Some(&vector_config), None), 4);
        assert_eq!(
            effective_vector_min_similarity(None, Some(&vector_config)),
            0.91
        );
        assert_eq!(
            effective_vector_max_snippet_chars(None, Some(&vector_config)),
            640
        );
        assert!(!effective_summary_enabled(None, Some(&vector_config)));
        assert_eq!(
            effective_summary_trigger_messages(None, Some(&vector_config)),
            12
        );
    }

    #[test]
    fn autotune_thresholds_override_static_vector_defaults() {
        let vector_config = vector_config_fixture();
        let tuned_state = AutoTuneState {
            current_min_query_chars: 95,
            current_top_k: 3,
            window_phase: 0,
            high_precision_count: 0,
            low_precision_count: 0,
            vector_search_count: 0,
            cooldown_remaining: 0,
        };

        assert_eq!(
            effective_vector_min_query_chars(None, Some(&vector_config), Some(&tuned_state)),
            95
        );
        assert_eq!(
            effective_vector_top_k(None, Some(&vector_config), Some(&tuned_state)),
            3
        );
    }

    #[test]
    fn autotune_snapshot_includes_all_fields() {
        let config = AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = AutoTuneState::new(&config);
        state.current_min_query_chars = 120;
        state.current_top_k = 3;
        state.window_phase = 5;
        state.high_precision_count = 12;
        state.low_precision_count = 2;
        state.vector_search_count = 18;
        state.cooldown_remaining = 1;

        let snapshot = state.snapshot();
        assert_eq!(snapshot["current_min_query_chars"], 120);
        assert_eq!(snapshot["current_top_k"], 3);
        assert_eq!(snapshot["window_phase"], 5);
        assert_eq!(snapshot["high_precision_count"], 12);
        assert_eq!(snapshot["low_precision_count"], 2);
        assert_eq!(snapshot["vector_search_count"], 18);
        assert_eq!(snapshot["cooldown_remaining"], 1);
    }

    // Integration tests for full ACP protocol flow
    #[test]
    fn initialize_request_returns_server_capabilities() {
        let request_json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let request: JsonRpcRequest = serde_json::from_str(request_json).unwrap();

        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.id, Some(Value::Number(1.into())));
        assert_eq!(request.method, "initialize");
    }

    #[test]
    fn metrics_snapshot_structure() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_chat_requests();
        metrics.inc_cache_lookup();
        metrics.inc_cache_hit();
        metrics.inc_vector_search();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.chat_requests_total, 1);
        assert_eq!(snapshot.cache_lookup_total, 1);
        assert_eq!(snapshot.cache_hit_total, 1);
        assert_eq!(snapshot.vector_search_total, 1);
    }

    #[test]
    fn jsonrpc_response_serialization() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0",
            id: Some(Value::Number(1.into())),
            result: Some(json!({"status": "ok"})),
            error: None,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 1);
        assert_eq!(json["result"]["status"], "ok");
        assert!(json.get("error").is_none());
    }

    #[test]
    fn jsonrpc_error_response_serialization() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0",
            id: Some(Value::Number(2.into())),
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: "Method not found".to_string(),
                data: None,
            }),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 2);
        assert_eq!(json["error"]["code"], -32601);
        assert!(json.get("result").is_none());
    }

    // Cache hit integration test
    #[test]
    fn cache_hit_increments_metrics() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_cache_lookup();
        metrics.inc_cache_hit();
        metrics.inc_cache_lookup();
        metrics.inc_cache_hit();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.cache_lookup_total, 2);
        assert_eq!(snapshot.cache_hit_total, 2);
    }

    #[test]
    fn cache_miss_tracked_correctly() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_cache_lookup();
        // No hit incremented
        metrics.inc_cache_lookup();
        metrics.inc_cache_hit();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.cache_lookup_total, 2);
        assert_eq!(snapshot.cache_hit_total, 1);
    }

    // Dual review integration test
    #[test]
    fn autopilot_complex_requires_dual_review() {
        let mode = ChatMode::FullAuto;
        let complexity = AutopilotComplexity::Complex;
        let strategy = mode_to_approval_strategy(Some(mode), Some(complexity));

        assert_eq!(strategy, ApprovalStrategy::AutoPilotComplex);
        assert!(strategy.needs_dual_review());
    }

    #[test]
    fn autopilot_simple_bypasses_dual_review() {
        let mode = ChatMode::FullAuto;
        let complexity = AutopilotComplexity::Simple;
        let strategy = mode_to_approval_strategy(Some(mode), Some(complexity));

        assert_eq!(strategy, ApprovalStrategy::AutoPilotSimple);
        assert!(!strategy.needs_dual_review());
    }

    #[test]
    fn edit_mode_bypasses_approvals() {
        let mode = ChatMode::Edit;
        let strategy = mode_to_approval_strategy(Some(mode), None);

        assert!(!strategy.needs_dual_review());
        assert_eq!(strategy.as_str(), "by_pass_approval");
    }

    // Fallback chain integration test
    #[test]
    fn approval_strategy_fallback_chain() {
        // Test: Ask mode (default) requires approval
        let strategy_ask = mode_to_approval_strategy(Some(ChatMode::Ask), None);
        assert_eq!(strategy_ask, ApprovalStrategy::DefaultApprovals);

        // Test: No mode defaults to Ask behavior
        let strategy_none = mode_to_approval_strategy(None, None);
        assert_eq!(strategy_none, ApprovalStrategy::DefaultApprovals);

        // Test: FullAuto without complexity defaults to Simple
        let strategy_auto = mode_to_approval_strategy(Some(ChatMode::FullAuto), None);
        assert_eq!(strategy_auto, ApprovalStrategy::AutoPilotSimple);
    }

    #[test]
    fn strategy_string_representations() {
        let strategies = vec![
            (ApprovalStrategy::DefaultApprovals, "default_approvals"),
            (ApprovalStrategy::ByPassApproval, "by_pass_approval"),
            (ApprovalStrategy::AutoPilotSimple, "autopilot_simple"),
            (ApprovalStrategy::AutoPilotComplex, "autopilot_complex"),
        ];

        for (strategy, expected) in strategies {
            assert_eq!(strategy.as_str(), expected);
        }
    }

    #[test]
    fn circuit_breaker_transitions_to_half_open_and_closes_on_success() {
        let breaker = CircuitBreakerRegistry::default();

        breaker.record_failure_with_config("copilot", 2, 1);
        assert!(matches!(
            breaker.allow_request("copilot"),
            CircuitBreakerAdmission::Closed
        ));

        breaker.record_failure_with_config("copilot", 2, 1);
        let snapshot = breaker.snapshot();
        assert_eq!(snapshot["copilot"].state, "open");

        assert!(matches!(
            breaker.allow_request("copilot"),
            CircuitBreakerAdmission::Rejected {
                state: "open",
                retry_after_seconds: Some(_)
            }
        ));

        {
            let mut guard = breaker.inner.lock().unwrap();
            let state = guard.get_mut("copilot").unwrap();
            state.open_until = Some(now_ts() - 1);
        }

        assert!(matches!(
            breaker.allow_request("copilot"),
            CircuitBreakerAdmission::HalfOpenProbe
        ));
        assert!(matches!(
            breaker.allow_request("copilot"),
            CircuitBreakerAdmission::Rejected {
                state: "half_open",
                retry_after_seconds: None
            }
        ));

        breaker.record_success("copilot");
        let snapshot = breaker.snapshot();
        assert_eq!(snapshot["copilot"].state, "closed");
        assert_eq!(snapshot["copilot"].consecutive_failures, 0);
        assert!(!snapshot["copilot"].probe_in_flight);
    }

    #[test]
    fn circuit_breaker_half_open_failure_reopens_breaker() {
        let breaker = CircuitBreakerRegistry::default();

        breaker.record_failure_with_config("claude", 1, 1);
        {
            let mut guard = breaker.inner.lock().unwrap();
            let state = guard.get_mut("claude").unwrap();
            state.open_until = Some(now_ts() - 1);
        }

        assert!(matches!(
            breaker.allow_request("claude"),
            CircuitBreakerAdmission::HalfOpenProbe
        ));

        breaker.record_failure_with_config("claude", 1, 3);
        let snapshot = breaker.snapshot();
        assert_eq!(snapshot["claude"].state, "open");
        assert_eq!(snapshot["claude"].consecutive_failures, 1);
        assert!(!snapshot["claude"].probe_in_flight);
    }

    #[test]
    fn prometheus_export_includes_headers_and_runtime_labels() {
        let mut snapshot = MetricsSnapshot::default();
        snapshot.chat_requests_total = 3;
        snapshot.cache_hit_total = 2;
        snapshot.review_gate_timeout_total = 1;
        snapshot.review_gate_degraded_total = 1;
        snapshot.review_gate_invalid_response_total = 1;
        snapshot.chat_latency_count = 1;
        snapshot.chat_latency_sum_seconds = 0.25;
        snapshot.chat_latency_bucket_counts[1] = 1;

        let gauges = RuntimeGaugeSnapshot {
            memory_cache_entries: 4,
            sqlite_cache_entries: 6,
            vector_memory_entries: 8,
            vector_summary_entries: 2,
            circuit_open_agents: 1,
            circuit_half_open_agents: 1,
            circuit_tracked_agents: 2,
            rate_limiter_tracked_phases: 1,
        };

        let breaker_snapshot = HashMap::from([(
            "copilot-main".to_string(),
            CircuitBreakerSnapshot {
                consecutive_failures: 3,
                state: "half_open_ready".to_string(),
                open_until: Some(now_ts() + 5),
                probe_in_flight: false,
            },
        )]);
        let phase_limiter_snapshot = HashMap::from([("coding".to_string(), (4.5, 12.0))]);
        let inflight_snapshot = (2_usize, HashMap::from([("coding".to_string(), 1_usize)]));
        let lifecycle = LifecycleSnapshot {
            shutting_down: true,
            shutdown_started_at: Some(now_ts()),
            shutdown_reason: Some("unit-test".to_string()),
        };
        let maintenance = MaintenanceSnapshot {
            running: true,
            cycles_total: 7,
            last_started_at: Some(now_ts()),
            last_completed_at: Some(now_ts()),
            last_memory_expired_removed: 3,
            last_sqlite_expired_removed: 5,
            last_error: None,
        };

        let rendered = build_prometheus_metrics(
            &snapshot,
            &gauges,
            &breaker_snapshot,
            &phase_limiter_snapshot,
            &inflight_snapshot,
            &lifecycle,
            &maintenance,
        );

        assert!(rendered.contains("# HELP acp_chat_requests_total Total ACP chat requests handled"));
        assert!(rendered.contains("# TYPE acp_chat_requests_total counter"));
        assert!(rendered.contains("acp_review_gate_timeout_total 1"));
        assert!(rendered.contains("acp_review_gate_degraded_total 1"));
        assert!(rendered.contains("acp_review_gate_invalid_response_total 1"));
        assert!(rendered.contains("acp_inflight_requests{scope=\"global\"} 2"));
        assert!(rendered.contains("acp_inflight_requests{scope=\"phase\",phase=\"coding\"} 1"));
        assert!(rendered.contains(
            "acp_circuit_breaker_state{agent=\"copilot-main\",state=\"half_open_ready\"} 1"
        ));
        assert!(rendered.contains("acp_lifecycle_shutting_down 1"));
        assert!(rendered.contains("acp_maintenance_cycles_total 7"));
        assert!(rendered.contains("acp_chat_latency_seconds_bucket{le=\"0.25\"} 1"));
    }

    #[test]
    fn metrics_reset_clears_all_counters() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_chat_requests();
        metrics.inc_cache_hit();
        metrics.inc_vector_search();

        let snapshot1 = metrics.snapshot();
        assert!(snapshot1.chat_requests_total > 0);

        metrics.reset();
        let snapshot2 = metrics.snapshot();
        assert_eq!(snapshot2.chat_requests_total, 0);
        assert_eq!(snapshot2.cache_hit_total, 0);
        assert_eq!(snapshot2.vector_search_total, 0);
    }

    #[test]
    fn record_agent_failure_metrics_tracks_timeout_bucket() {
        let metrics = RuntimeMetrics::default();
        let err = anyhow::anyhow!("agent timed out after 15s");

        record_agent_failure_metrics(&metrics, &err);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.agent_failures_total, 1);
        assert_eq!(snapshot.agent_timeout_failures_total, 1);
        assert_eq!(snapshot.agent_panic_failures_total, 0);
        assert_eq!(snapshot.agent_other_failures_total, 0);
    }

    #[test]
    fn record_agent_failure_metrics_tracks_panic_and_other_buckets() {
        let metrics = RuntimeMetrics::default();
        let panic_err = anyhow::anyhow!("agent panic: task join error");
        let other_err = anyhow::anyhow!("remote provider returned malformed payload");

        record_agent_failure_metrics(&metrics, &panic_err);
        record_agent_failure_metrics(&metrics, &other_err);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.agent_failures_total, 2);
        assert_eq!(snapshot.agent_timeout_failures_total, 0);
        assert_eq!(snapshot.agent_panic_failures_total, 1);
        assert_eq!(snapshot.agent_other_failures_total, 1);
    }

    // === ACP Runtime RPC Integration Tests ===
    // These tests verify the JSON-RPC protocol contract for ACP server endpoints.

    #[test]
    fn rpc_initialize_response_includes_server_name_and_capabilities() {
        let server = phase_inference_server("planning", &["planning", "coding"]);

        // Verify request parsing
        let request_json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let request: JsonRpcRequest = serde_json::from_str(request_json).unwrap();

        assert_eq!(request.method, "initialize");
        assert_eq!(request.id, Some(Value::Number(1.into())));

        // Runtime defaults are injected when no explicit runtime block is provided.
        assert!(server.runtime_config_snapshot().shutdown_drain_seconds > 0);
    }

    #[test]
    fn rpc_metrics_snapshot_includes_all_metric_types() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_chat_requests();
        metrics.inc_cache_lookup();
        metrics.inc_cache_hit();
        metrics.inc_vector_search();
        metrics.inc_vector_hit();
        metrics.inc_summary_read();
        metrics.inc_summary_hit();
        metrics.inc_agent_failures();
        metrics.inc_agent_timeout_failures();
        metrics.inc_review_gate();
        metrics.inc_review_gate_approved();
        metrics.inc_review_gate_timeout();
        metrics.inc_review_gate_degraded();
        metrics.inc_review_gate_invalid_response();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.chat_requests_total, 1);
        assert_eq!(snapshot.cache_lookup_total, 1);
        assert_eq!(snapshot.cache_hit_total, 1);
        assert_eq!(snapshot.vector_search_total, 1);
        assert_eq!(snapshot.vector_hit_total, 1);
        assert_eq!(snapshot.summary_read_total, 1);
        assert_eq!(snapshot.summary_hit_total, 1);
        assert_eq!(snapshot.agent_failures_total, 1);
        assert_eq!(snapshot.agent_timeout_failures_total, 1);
        assert_eq!(snapshot.review_gate_total, 1);
        assert_eq!(snapshot.review_gate_approved_total, 1);
        assert_eq!(snapshot.review_gate_timeout_total, 1);
        assert_eq!(snapshot.review_gate_degraded_total, 1);
        assert_eq!(snapshot.review_gate_invalid_response_total, 1);
    }

    #[test]
    fn rpc_prometheus_metrics_serializes_to_valid_format() {
        let mut snapshot = MetricsSnapshot::default();
        snapshot.chat_requests_total = 42;
        snapshot.cache_hit_total = 15;
        snapshot.agent_failures_total = 2;
        snapshot.agent_timeout_failures_total = 1;
        snapshot.agent_panic_failures_total = 1;
        snapshot.review_gate_total = 3;
        snapshot.review_gate_approved_total = 2;
        snapshot.review_gate_timeout_total = 1;
        snapshot.review_gate_degraded_total = 1;
        snapshot.review_gate_invalid_response_total = 1;

        let gauges = RuntimeGaugeSnapshot {
            memory_cache_entries: 12,
            sqlite_cache_entries: 45,
            vector_memory_entries: 8,
            vector_summary_entries: 3,
            circuit_open_agents: 1,
            circuit_half_open_agents: 0,
            circuit_tracked_agents: 2,
            rate_limiter_tracked_phases: 4,
        };

        let prometheus = build_prometheus_metrics(
            &snapshot,
            &gauges,
            &HashMap::new(),
            &HashMap::new(),
            &(0, HashMap::new()),
            &LifecycleSnapshot::default(),
            &MaintenanceSnapshot::default(),
        );

        assert!(prometheus.contains("acp_chat_requests_total 42"));
        assert!(prometheus.contains("acp_cache_hit_total 15"));
        assert!(prometheus.contains("acp_agent_failures_total 2"));
        assert!(prometheus.contains("acp_agent_timeout_failures_total 1"));
        assert!(prometheus.contains("acp_agent_panic_failures_total 1"));
        assert!(prometheus.contains("acp_review_gate_total 3"));
        assert!(prometheus.contains("acp_review_gate_approved_total 2"));
        assert!(prometheus.contains("acp_review_gate_timeout_total 1"));
        assert!(prometheus.contains("acp_review_gate_degraded_total 1"));
        assert!(prometheus.contains("acp_review_gate_invalid_response_total 1"));
        assert!(prometheus.contains("acp_memory_cache_entries 12"));
        assert!(prometheus.contains("acp_circuit_tracked_agents 2"));
        assert!(prometheus.contains("acp_rate_limiter_tracked_phases 4"));
    }

    #[test]
    fn rpc_runtime_health_includes_all_subsystems() {
        let server = phase_inference_server("planning", &["planning", "coding"]);
        let memory_cache = &server.memory_cache;
        let circuit_breakers = &server.circuit_breakers;
        let phase_rate_limiter = &server.phase_rate_limiter;
        let inflight_limiter = &server.inflight_limiter;

        // Verify cache is accessible
        assert_eq!(memory_cache.active_entries(), 0);

        // Verify circuit breaker state
        let cb_snapshot = circuit_breakers.snapshot();
        assert!(cb_snapshot.is_empty());
        assert_eq!(circuit_breakers.tracked_agents(), 0);

        // Verify rate limiter
        assert_eq!(phase_rate_limiter.tracked_phases(), 0);

        // Verify inflight tracking
        let (global, phases) = inflight_limiter.snapshot();
        assert_eq!(global, 0);
        assert!(phases.is_empty());
    }

    #[test]
    fn rpc_phase_status_tracks_rate_limiter_state() {
        let phase_limiter = PhaseRateLimiter::default();

        // Test token bucket state tracking
        assert!(phase_limiter.allow("planning", 60, None));
        assert_eq!(phase_limiter.tracked_phases(), 1);

        let snapshot = phase_limiter.snapshot();
        assert!(snapshot.contains_key("planning"));
        let (tokens, capacity) = snapshot["planning"];
        assert!(tokens < capacity);
        assert_eq!(capacity, 60.0);
    }

    #[test]
    fn rpc_phase_status_burst_capacity_respected() {
        let phase_limiter = PhaseRateLimiter::default();

        // Allow requests up to burst capacity
        for _ in 0..5 {
            assert!(phase_limiter.allow("coding", 60, Some(5)));
        }

        // 6th request should fail
        assert!(!phase_limiter.allow("coding", 60, Some(5)));

        // Verify capacity constraint
        let snapshot = phase_limiter.snapshot();
        assert!(snapshot.contains_key("coding"));
        let (tokens, _) = snapshot["coding"];
        // Tokens should be less than 1.0 (since we just consumed one)
        assert!(tokens < 1.0);
    }

    #[test]
    fn rpc_inflight_limiter_enforces_phase_and_global_limits() {
        let limiter = Arc::new(InflightLimiter::default());

        // Test phase limit
        let guard1 = limiter.clone().try_enter("planning", Some(2), Some(5));
        assert!(guard1.is_some());

        let guard2 = limiter.clone().try_enter("planning", Some(2), Some(5));
        assert!(guard2.is_some());

        let guard3 = limiter.clone().try_enter("planning", Some(2), Some(5));
        assert!(guard3.is_none());

        drop(guard1);
        let guard4 = limiter.clone().try_enter("planning", Some(2), Some(5));
        assert!(guard4.is_some());

        let (global, _) = limiter.snapshot();
        assert_eq!(global, 2);
    }

    #[test]
    fn rpc_inflight_limiter_global_limit_respected() {
        let limiter = Arc::new(InflightLimiter::default());

        let mut guards = Vec::new();
        for _ in 0..3 {
            let guard = limiter.clone().try_enter("planning", None, Some(3));
            assert!(guard.is_some());
            guards.push(guard);
        }

        let guard4 = limiter.clone().try_enter("coding", None, Some(3));
        assert!(guard4.is_none());

        drop(guards.pop());
        let guard5 = limiter.clone().try_enter("coding", None, Some(3));
        assert!(guard5.is_some());
    }

    #[test]
    fn rpc_lifecycle_state_tracks_shutdown() {
        let lifecycle = LifecycleState::default();

        assert!(!lifecycle.is_shutting_down());
        assert!(lifecycle.start_shutdown("test shutdown"));
        assert!(lifecycle.is_shutting_down());

        // Second call should fail
        assert!(!lifecycle.start_shutdown("already shutting down"));

        let snapshot = lifecycle.snapshot();
        assert!(snapshot.shutting_down);
        assert_eq!(snapshot.shutdown_reason, Some("test shutdown".to_string()));
        assert!(snapshot.shutdown_started_at.is_some());
    }

    #[test]
    fn rpc_maintenance_tracker_records_cycle_metrics() {
        let maintenance = MaintenanceTracker::default();

        maintenance.note_started();
        let snapshot1 = maintenance.snapshot();
        assert!(snapshot1.running);
        assert_eq!(snapshot1.cycles_total, 1);

        maintenance.note_completed(5, 3);
        let snapshot2 = maintenance.snapshot();
        assert!(!snapshot2.running);
        assert_eq!(snapshot2.last_memory_expired_removed, 5);
        assert_eq!(snapshot2.last_sqlite_expired_removed, 3);
        assert_eq!(snapshot2.cycles_total, 1);
    }

    #[test]
    fn rpc_maintenance_tracker_records_failures() {
        let maintenance = MaintenanceTracker::default();

        maintenance.note_started();
        maintenance.note_failed("connection timeout");

        let snapshot = maintenance.snapshot();
        assert!(!snapshot.running);
        assert_eq!(snapshot.last_error, Some("connection timeout".to_string()));
    }

    #[test]
    fn rpc_circuit_breaker_snapshot_complete() {
        let breaker = CircuitBreakerRegistry::default();

        breaker.record_failure_with_config("agent-a", 2, 10);
        breaker.record_failure_with_config("agent-a", 2, 10);
        breaker.record_failure_with_config("agent-b", 1, 10);

        let snapshot = breaker.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot["agent-a"].state, "open");
        assert_eq!(snapshot["agent-a"].consecutive_failures, 2);
        assert_eq!(snapshot["agent-b"].state, "open");
        assert_eq!(snapshot["agent-b"].consecutive_failures, 1);
    }

    #[test]
    fn rpc_metrics_reset_integration() {
        let metrics = RuntimeMetrics::default();

        metrics.inc_chat_requests();
        metrics.inc_cache_hit();
        metrics.inc_agent_failures();
        metrics.observe_chat_latency(Duration::from_secs_f64(0.25));

        let snapshot1 = metrics.snapshot();
        assert_eq!(snapshot1.chat_requests_total, 1);
        assert_eq!(snapshot1.cache_hit_total, 1);
        assert_eq!(snapshot1.agent_failures_total, 1);
        assert!(snapshot1.chat_latency_count > 0);

        metrics.reset();
        let snapshot2 = metrics.snapshot();
        assert_eq!(snapshot2.chat_requests_total, 0);
        assert_eq!(snapshot2.cache_hit_total, 0);
        assert_eq!(snapshot2.agent_failures_total, 0);
        assert_eq!(snapshot2.chat_latency_count, 0);
    }

    #[test]
    fn rpc_jsonrpc_error_codes_reserved() {
        // Verify standard JSON-RPC error codes
        assert_eq!(-32700, -32700); // Parse error
        assert_eq!(-32600, -32600); // Invalid request
        assert_eq!(-32601, -32601); // Method not found
        assert_eq!(-32602, -32602); // Invalid params
        assert_eq!(-32603, -32603); // Internal error
        assert_eq!(-32031, -32031); // Server state error (custom)
    }

    #[test]
    fn rpc_request_parsing_handles_missing_fields() {
        let request_json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let request: JsonRpcRequest = serde_json::from_str(request_json).unwrap();

        assert_eq!(request.method, "initialize");
        assert_eq!(request.id, Some(Value::Number(1.into())));
        assert_eq!(request.params, None);
    }

    #[test]
    fn rpc_response_with_result_omits_error() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0",
            id: Some(Value::Number(1.into())),
            result: Some(json!({"status": "ok"})),
            error: None,
        };

        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.contains("\"result\""));
        assert!(!serialized.contains("\"error\""));
    }

    #[test]
    fn rpc_response_with_error_omits_result() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0",
            id: Some(Value::Number(2.into())),
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: "Method not found".to_string(),
                data: None,
            }),
        };

        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.contains("\"error\""));
        assert!(!serialized.contains("\"result\""));
    }

    #[test]
    fn rpc_notification_has_no_id() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0",
            id: None,
            result: Some(json!({"type": "notification"})),
            error: None,
        };

        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains("\"id\""));
    }
}
