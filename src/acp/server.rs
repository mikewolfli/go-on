//! ACP Server - Main server implementation
//!
//! This module contains the main AcpServer struct definition and related
//! server management functionality.

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex as StdMutex,
};

use std::time::Duration;
use tokio::sync::{mpsc, Mutex, Notify, Semaphore};

use crate::acp::prelude::RuntimeMetrics;
use crate::adaptive_selector::AdaptiveModelSelector;
use crate::observability::provenance::ProvenanceLedger;

use crate::agent::AgentRegistry;
use crate::cache::ResponseCache;
use crate::config::{AutoTuneConfig, AutoTuneState, RuntimeConfig, VectorConfig};

use crate::acp::r#impl::request::prompts_pack::PromptManager;
use crate::acp::r#impl::session::SessionManager;
use crate::failure_prevention::FailurePrevention;
use crate::flow::FlowManager;
use crate::flow_with_models::FlowModelSelector;
use crate::governance::audit::ThreadSafeAuditLog;
use crate::governance::harness_bus::HarnessBus;
use crate::intelligence::capability_bus::core::CapabilityBus;
use crate::intelligence::token_cache::TokenMultiLevelCache;
use crate::memory::memory_retrieval::MemoryRetrievalEngine;
use crate::memory::semantic_cache::SemanticResponseCache;
use crate::memory_module::{MemoryPolicy, MemoryStore};
use crate::memory_response_cache::MemoryResponseCache;
use crate::observability::alert_manager::AlertManager;
use crate::observability::telemetry::TelemetryRuntime;
use crate::orchestration::fork_registry::{ForkConfig, ForkRegistry};
use crate::orchestration::promotion_plugin::PromotionRegistry;
use crate::orchestration::prompt_layers::PromptAssembler;
use crate::orchestration::scheduler::AgentWorkerScheduler;
use crate::orchestration::skill::SkillRegistry;
use crate::orchestration::skill_market::SkillMarketRegistry;
use crate::orchestration::task_graph_store::TaskGraphStore;
use crate::orchestration::task_schema::SchemaRegistry;
use crate::orchestration::tool::ToolRegistry;
use crate::orchestration::workflow_optimizer::OptimizerRegistry;
use crate::reinforcement::ArtifactLedger;
use crate::security::rate_limiter::GlobalRateLimiter;
use crate::vector::VectorStore;

/// Fire-and-forget outcome event for online_controller.
/// Write-only operations are sent via channel to eliminate lock contention.
pub enum OutcomeEvent {
    AgentOutcome {
        phase_name: String,
        agent_name: String,
        success: bool,
        duration_ms: u64,
    },
    PhaseOutcome {
        phase_name: String,
        success: bool,
        duration_ms: u64,
    },
}

use super::prelude::{
    with_acp_lock, CircuitBreakerRegistry, ConversationState, InflightLimiter, LifecycleState,
    MaintenanceTracker, OnlineControllerState, PhaseRateLimiter, ReviewTimeoutPolicy,
};

/// DrainGuard — graceful shutdown state tracking.
///
/// When draining is active, new requests are rejected with 503 + Retry-After.
/// In-flight requests are given `drain_timeout` to complete before force-shutdown.
pub struct DrainGuard {
    /// Whether the server is currently draining.
    pub draining: AtomicBool,
    /// Semaphore tracking in-flight request count.
    pub inflight: Arc<Semaphore>,
    /// Maximum number of in-flight permits (stored separately as Semaphore doesn't expose total_permits).
    pub max_permits: usize,
    /// Maximum time to wait for in-flight requests to complete.
    pub drain_timeout: Duration,
    /// Monotonic request ID counter for tracing.
    pub request_seq: AtomicU64,
    /// Notify when a permit is released (BLUE56-C03).
    notify_drain: Arc<tokio::sync::Notify>,
}

/// A permit that notifies the drain waiter when dropped (BLUE56-C03).
///
/// Wraps `OwnedSemaphorePermit` so that releasing the permit triggers
/// a notification to `DrainGuard::wait_for_drain()` instead of polling.
pub struct DrainPermit {
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
    notify: Arc<tokio::sync::Notify>,
}

impl Drop for DrainPermit {
    fn drop(&mut self) {
        // Drop the permit first, then notify
        drop(self.permit.take());
        self.notify.notify_one();
    }
}

impl DrainPermit {
    /// Consume the permit and return the inner semaphore permit.
    /// The caller is responsible for ensuring the inner permit is eventually dropped.
    pub fn into_inner(mut self) -> tokio::sync::OwnedSemaphorePermit {
        self.permit.take().expect("DrainPermit already consumed")
    }
}

impl DrainGuard {
    /// Create a new DrainGuard with the given capacity and timeout.
    pub fn new(max_inflight: usize, drain_timeout_secs: u64) -> Self {
        Self {
            draining: AtomicBool::new(false),
            inflight: Arc::new(Semaphore::new(max_inflight)),
            max_permits: max_inflight,
            drain_timeout: Duration::from_secs(drain_timeout_secs.max(5)),
            request_seq: AtomicU64::new(0),
            notify_drain: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Begin the drain process: reject new requests.
    pub fn start_drain(&self) {
        self.draining.store(true, Ordering::SeqCst);
        tracing::info!(
            "DrainGuard: drain started, timeout={:?}",
            self.drain_timeout
        );
    }

    /// Returns true if the server is currently draining.
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    /// Wait until all in-flight requests complete or the timeout elapses.
    /// Uses tokio::sync::Notify for zero-wait notification (BLUE56-C03).
    pub async fn wait_for_drain(&self) -> bool {
        let permit_count = self.inflight.available_permits();
        if permit_count == self.max_permits {
            tracing::info!("DrainGuard: no in-flight requests, drain complete");
            return true;
        }
        tracing::info!(
            "DrainGuard: waiting for in-flight requests ({} active of {} max)",
            self.max_permits.saturating_sub(permit_count),
            self.max_permits
        );
        // Wait for all permits to be released or timeout, using Notify
        let deadline = tokio::time::Instant::now() + self.drain_timeout;
        loop {
            let wait_fut = self.notify_drain.notified();
            tokio::select! {
                _ = wait_fut => {
                    let current_avail = self.inflight.available_permits();
                    if current_avail == self.max_permits {
                        return true;
                    }
                    // Notify was spurious or partial — continue waiting
                }
                _ = tokio::time::sleep_until(deadline) => {
                    tracing::warn!(
                        "DrainGuard: drain timeout reached, {} requests still in-flight",
                        self.max_permits
                            .saturating_sub(self.inflight.available_permits())
                    );
                    return false;
                }
            }
        }
    }

    /// Acquire a permit for an incoming request. Returns None if draining.
    pub async fn acquire(&self) -> Option<DrainPermit> {
        if self.is_draining() {
            return None;
        }
        let permit = self.inflight.clone().acquire_owned().await.ok()?;
        if self.is_draining() {
            // Race: drain started between our check and acquire.
            drop(permit);
            return None;
        }
        Some(DrainPermit {
            permit: Some(permit),
            notify: self.notify_drain.clone(),
        })
    }

    /// Get the next request sequence number.
    pub fn next_seq(&self) -> u64 {
        self.request_seq.fetch_add(1, Ordering::SeqCst)
    }
}

impl Default for DrainGuard {
    fn default() -> Self {
        Self::new(128, 30)
    }
}

/// Cache-related subsystems grouped together
pub struct CacheLayer {
    /// Response cache (SQLite-based)
    pub response_cache: Option<Arc<ResponseCache>>,
    /// Memory response cache (std::sync::Mutex for consistency with memory_bus set_backends)
    pub memory_response_cache: Arc<StdMutex<MemoryResponseCache>>,
    /// Vector store for similarity search and memory
    pub vector_store: Option<Arc<VectorStore>>,
    /// Multi-level token cache for Agent output reuse (L1 exact, L2 semantic, L3 template)
    pub token_cache: Arc<TokenMultiLevelCache>,
    /// Semantic response cache for near-duplicate request detection
    pub semantic_cache: Arc<std::sync::RwLock<SemanticResponseCache>>,
}

/// Cache + vector + autotune subsystems grouped together
pub struct CacheServerDeps {
    /// Cache-layer subsystems (response cache, vector store, token cache)
    pub cache: CacheLayer,
    /// Vector store configuration
    pub vector_config: Option<VectorConfig>,
    /// Autotune state for adaptive configuration
    pub autotune: Option<Arc<Mutex<AutoTuneState>>>,
    /// Autotune configuration
    pub autotune_config: Option<AutoTuneConfig>,
    /// Path to autotune state file
    pub autotune_state_path: Option<String>,
}

/// Flow + agent + model selection subsystems grouped together
pub struct ModelServerDeps {
    /// Flow manager for handling request routing through phases
    pub flow_manager: Option<Arc<FlowManager>>,
    /// Agent registry for managing available agents
    pub agent_registry: Option<Arc<AgentRegistry>>,
    /// Adaptive model selector
    pub adaptive_model_selector: Arc<StdMutex<AdaptiveModelSelector>>,
    /// Flow model selector
    pub flow_model_selector: Arc<StdMutex<FlowModelSelector>>,
}

/// Governance subsystems grouped together (harness + capability + audit + pua + rbac)
pub struct GovernanceServerDeps {
    /// HarnessBus strategy engine (BLUE38 ARCH-13)
    pub harness_bus: Option<Arc<HarnessBus>>,
    /// CapabilityBus scheduling coordinator (BLUE38 ARCH-13)
    pub capability_bus: Option<Arc<CapabilityBus>>,
    /// PUA enforcement plan
    pub pua_enforcement_plan: Arc<StdMutex<crate::pua::PuaEnforcementPlan>>,
    /// RBAC enforcer for request-level authorization
    pub rbac_enforcer: Option<Arc<std::sync::RwLock<crate::governance::rbac::RbacEnforcer>>>,
    /// Provenance ledger — immutable data lineage tracking
    pub provenance_ledger: Option<Arc<ProvenanceLedger>>,
    /// Approval engine for HITL workflow (GAP-B52-19)
    pub approval_engine:
        Option<Arc<tokio::sync::RwLock<crate::governance::approval_engine::ApprovalEngine>>>,
    /// Prompt injection detector (GAP-B52-25)
    pub injection_detector: Option<Arc<crate::security::prompt_injection::InjectionDetector>>,
    /// Content safety checker (GAP-B52-28)
    pub safety_checker: Option<Arc<crate::security::content_safety::SafetyChecker>>,
    /// Hash chain audit integrity protector (GAP-B52-27)
    pub hash_chain_auditor:
        Option<Arc<std::sync::Mutex<crate::security::audit_integrity::HashChainAuditor>>>,
    /// Secret manager with auto-rotation (GAP-B52-26)
    pub secret_manager: Option<Arc<crate::security::secret_rotation::SecretManager>>,
    /// Memory persistence manager (GAP-B52-11)
    pub memory_persistence: Option<Arc<crate::memory::memory_persistence::MemoryPersistence>>,
    /// Memory retrieval engine with link graph and semantic search (GAP-B52-13)
    pub memory_retrieval_engine: Option<Arc<MemoryRetrievalEngine>>,
    /// Self-evolution loop handle (GAP-B52-02)
    pub evolution_loop: Option<
        Arc<
            tokio::sync::Mutex<crate::orchestration::self_evolution::evolution_loop::EvolutionLoop>,
        >,
    >,

    // ── Security scanning (GAP-B52) ──────────────────────────────────────
    /// Dependency vulnerability scanner (GAP-B52-24)
    pub dependency_vulnerability_scanner:
        Option<Arc<crate::security::vulnerability_scan::DependencyVulnerabilityScanner>>,
    /// Secret exposure detector (GAP-B52-24)
    pub secret_exposure_detector:
        Option<Arc<crate::security::vulnerability_scan::SecretExposureDetector>>,
    /// Permit/mode exposure analyzer (GAP-B52-24)
    pub permit_exposure_analyzer:
        Option<Arc<crate::security::vulnerability_scan::PermitExposureAnalyzer>>,
    /// Security advisor agent (GAP-B52-30)
    pub security_advisor: Option<Arc<crate::security::security_advisor::SecurityAdvisorAgent>>,
    /// Policy reloader for hot-reloading governance policies (GAP-B58-D04)
    pub policy_reloader:
        Option<Arc<std::sync::Mutex<crate::governance::reloadable_policy::PolicyReloader>>>,
}

/// Orchestration subsystems grouped together (scheduler + planner + executor + skill)
pub struct OrchestrationServerDeps {
    /// Dual-level task scheduler for priority queue and worker pool
    pub scheduler: Option<Arc<AgentWorkerScheduler>>,
    /// Planner — task decomposition engine (F-GAP-05)
    pub planner: crate::orchestration::planner_executor::Planner,
    /// Executor — plan execution engine (F-GAP-05)
    pub executor: crate::orchestration::planner_executor::Executor,
    /// Planner-Executor configuration (BLUE47 Step 7)
    pub planner_executor_config: crate::orchestration::planner_executor::PlannerExecutorConfig,
    /// Registry for MCP skills
    pub skill_registry: Arc<std::sync::RwLock<SkillRegistry>>,
}

/// Observability-related subsystems grouped together
pub struct ObservabilityLayer {
    /// Runtime metrics collection
    pub metrics: Arc<RuntimeMetrics>,
    /// Telemetry runtime
    // SAFETY: StdMutex is never held across `.await` — all access is short synchronous
    // critical sections (read/write metrics counters) that complete and drop the guard
    // before any async yield point.
    pub telemetry_runtime: Arc<StdMutex<TelemetryRuntime>>,
    /// Alert manager for threshold-based alerting
    // SAFETY: StdMutex is never held across `.await` — all access is short synchronous
    // critical sections (evaluate alert thresholds, push/broadcast) that complete and drop
    // the guard before any async yield point.
    pub alert_manager: Arc<StdMutex<AlertManager>>,
}

/// Resilience-related subsystems grouped together (circuit breaking, rate limiting, lifecycle, failover)
///
/// # Why two circuit breaker systems?
///
/// | System | Type | Scope | Purpose |
/// |--------|------|-------|---------|
/// | `circuit_breakers` | `CircuitBreakerRegistry` | ACP per-service | Tracks open/closed state per agent; used by `get_status()` for UI reporting |
/// | `hyper_resilience` | `HyperResilienceEngine` | Cross-service | Full failover groups, self-healing, latency-aware degradation (BLUE56-GAP-C04) |
///
/// `circuit_breakers` is a lightweight per-service registry whose state is
/// reported to the GUI and the `/status` endpoint. `hyper_resilience` is a
/// superset engine that consumes the same signals but also orchestrates
/// failover between services and automatic healing. Both coexist because
/// the simple registry provides the synchronous `open_count()` / `snapshots()`
/// accessors needed by the ACP status API, while the hyper engine runs
/// its own async decision loops for advanced resilience patterns.
///
/// All StdMutex fields here are safe because they are never held across `.await` boundaries:
/// locks are acquired, used in short synchronous operations, and released within the same scope.
/// None of these fields have an `.await` point inside their critical sections.
pub struct ResilienceContext {
    /// Online controller for adaptive strategy from live outcomes
    // SAFETY: StdMutex is never held across `.await` — all access uses `with_acp_lock()`
    // which acquires, reads/writes, and drops the guard within a single synchronous closure.
    pub online_controller: Arc<StdMutex<OnlineControllerState>>,
    /// Channel sender for fire-and-forget outcome events.
    /// Write operations are sent here to avoid lock contention on the hot path.
    pub outcome_tx: mpsc::UnboundedSender<OutcomeEvent>,
    /// Circuit breaker registry for failure prevention
    // SAFETY: StdMutex is never held across `.await` — all access uses `with_acp_lock()`
    // which acquires, reads/writes, and drops the guard within a single synchronous closure.
    pub circuit_breakers: Arc<StdMutex<CircuitBreakerRegistry>>,
    /// Hyper-resilience engine for circuit breaking, failover, and self-healing (BLUE56-GAP-C04)
    // This field is NOT a StdMutex — it is an async-safe engine.
    pub hyper_resilience: Arc<crate::resilience::hyper_resilience::HyperResilienceEngine>,
    /// Maintenance tracker for system health monitoring
    // SAFETY: StdMutex is never held across `.await` — all access uses `with_acp_lock()`
    // which acquires, reads/writes, and drops the guard within a single synchronous closure.
    pub maintenance_tracker: Arc<std::sync::RwLock<MaintenanceTracker>>,
    /// Inflight request limiter
    // SAFETY: StdMutex is never held across `.await` — all access is short synchronous
    // critical sections (check/adjust inflight count) with no `.await` inside.
    pub inflight_limiter: Arc<std::sync::RwLock<InflightLimiter>>,
    /// Lifecycle state management
    // SAFETY: StdMutex is never held across `.await` — all access uses `with_acp_lock()`
    // which acquires, reads/writes, and drops the guard within a single synchronous closure.
    pub lifecycle_state: Arc<std::sync::RwLock<LifecycleState>>,
    /// Review timeout policy
    // SAFETY: StdMutex is never held across `.await` — review timeout checks are
    // synchronous lookups that complete and drop the guard within the same scope.
    pub review_timeout_policy: Arc<std::sync::RwLock<ReviewTimeoutPolicy>>,
    /// Failure prevention system
    // SAFETY: StdMutex is never held across `.await` — all access is short synchronous
    // critical sections (evaluate failure conditions, update state) with no `.await` inside.
    pub failure_prevention: Arc<StdMutex<FailurePrevention>>,
    /// Phase rate limiter (held by ResilienceContext since it governs per-phase request admission)
    // SAFETY: StdMutex is never held across `.await` — rate limit admission checks are
    // synchronous token-bucket operations that complete before any async yield.
    pub phase_rate_limiter: Arc<StdMutex<PhaseRateLimiter>>,
}

/// Session and conversation state grouped together
///
/// `conversation_state` uses `tokio::sync::Mutex` because its lock is held across `.await` boundaries
/// (e.g. checkpoint operations that involve async I/O). The other fields are either lock-free
/// (`audit_log` is thread-safe internally) or use StdMutex for short synchronous operations.
pub struct SessionContext {
    /// Conversation state management (uses tokio::sync::Mutex — held across .await points)
    pub conversation_state: Arc<Mutex<ConversationState>>,
    /// User session manager for authentication and session lifecycle
    pub session_manager: Option<Arc<SessionManager>>,
    /// Session registry for cross-client state synchronization
    pub session_registry: Option<Arc<crate::protocol::session_sync::SessionRegistry>>,
    /// Thread-safe audit log with NDJSON persistence at ~/.goon/audit.ndjson
    pub audit_log: ThreadSafeAuditLog,
    /// In-memory registry for Responses API objects
    // SAFETY: StdMutex is never held across `.await` — map lookups/inserts are
    // short synchronous operations that complete and drop the guard before any async yield.
    pub responses_api_store: Arc<StdMutex<HashMap<String, serde_json::Value>>>,
}

/// Rate limiting and tenant quota enforcement grouped together
///
/// Both fields use StdMutex for short synchronous critical sections:
/// `rate_limit_middleware.check()` and `tenant_budget.check_can_start()`/`start_task()`
/// are fast non-async operations with no `.await` inside the locked scope.
pub struct RateLimitContext {
    /// Tenant-level rate limit middleware (F-GAP-49)
    pub rate_limit_middleware: Option<Arc<crate::protocol::rate_limit::RateLimitMiddleware>>,
    /// TenantBudgetEnforcer — per-tenant resource quota management (F-GAP-08)
    // SAFETY: StdMutex is never held across `.await` — `check_can_start()` and `start_task()`
    // are synchronous token-budget operations that complete and drop the guard before any `.await`.
    pub tenant_budget: Arc<StdMutex<crate::governance::hardening::TenantBudgetEnforcer>>,
    /// Global per-tenant token-bucket rate limiter (migrated from OnceLock static).
    pub global_rate_limiter: GlobalRateLimiter,
}

/// Registries for reusable system components
///
/// All fields use StdMutex — each is accessed in short synchronous critical sections
/// (locking, reading/writing, releasing). No `.await` points inside any locked scope.
pub struct RegistryContext {
    /// SchemaRegistry — task envelope validation (F-GAP-07)
    // SAFETY: StdMutex is never held across `.await` — schema lookups are synchronous
    // map accesses that complete and drop the guard before any async yield.
    pub schema_registry: Arc<StdMutex<crate::orchestration::task_schema::SchemaRegistry>>,
    /// OptimizerRegistry — workflow optimization plugins (ARCH-11)
    // SAFETY: StdMutex is never held across `.await` — plugin registry lookups are synchronous
    // that complete and drop the guard before any async yield.
    pub optimizer_registry:
        Arc<StdMutex<crate::orchestration::workflow_optimizer::OptimizerRegistry>>,
    /// PromotionRegistry — promotion plugin evaluation (ARCH-10)
    // SAFETY: StdMutex is never held across `.await` — promotion evaluations are synchronous
    // that complete and drop the guard before any async yield.
    pub promotion_registry:
        Arc<StdMutex<crate::orchestration::promotion_plugin::PromotionRegistry>>,
    /// BenchmarkSuite — evaluation suite for agent quality (F-GAP-06)
    // SAFETY: StdMutex is never held across `.await` — benchmark operations are synchronous
    // that complete and drop the guard before any async yield.
    pub evaluation_suite: Arc<StdMutex<crate::intelligence::evaluation::BenchmarkSuite>>,
    /// ForkRegistry — sub-agent process isolation (ARCH-05)
    // SAFETY: StdMutex is never held across `.await` — fork registry operations are synchronous
    // that complete and drop the guard before any async yield.
    pub fork_registry: Arc<StdMutex<ForkRegistry>>,
}

/// Persistence-related data stores grouped together
///
/// All fields use StdMutex — locks are acquired, used synchronously, and released.
/// No `.await` points inside any locked scope.
pub struct PersistenceContext {
    /// Cross-request memory policy store
    // SAFETY: StdMutex is never held across `.await` — memory store lookups are synchronous
    // map accesses that complete and drop the guard before any async yield.
    pub memory_store: Arc<StdMutex<MemoryStore>>,
    /// Artifact ledger
    // SAFETY: StdMutex is never held across `.await` — artifact ledger operations are synchronous
    // that complete and drop the guard before any async yield.
    pub artifact_ledger: Arc<StdMutex<ArtifactLedger>>,
    /// Persistent task graph store for checkpoints and recovery
    pub task_graph_store: Option<Arc<TaskGraphStore>>,
}

/// Main ACP server structure
///
/// This struct represents the core ACP server that handles incoming requests,
/// manages agents, and coordinates the overall system flow.
pub struct AcpServer {
    /// Cache-related deps (cache + vector + autotune)
    pub cache_deps: CacheServerDeps,
    /// Model-related deps (flow_manager + agent_registry + selectors)
    pub model_deps: ModelServerDeps,
    /// Governance deps (harness + capability + audit + pua + rbac)
    pub governance_deps: GovernanceServerDeps,
    /// Orchestration deps (scheduler + planner + executor + skill)
    pub orchestration_deps: OrchestrationServerDeps,
    /// Runtime configuration
    pub runtime_config: RuntimeConfig,
    /// Loaded config file path if available
    pub config_path: Option<String>,
    /// Observability-related subsystems
    pub observability: ObservabilityLayer,
    /// Resilience subsystems (circuit breakers, lifecycle, rate limiting, failover)
    pub resilience: ResilienceContext,
    /// Session and conversation state management
    pub session: SessionContext,
    /// Rate limiting and tenant quota enforcement
    pub rate_limiting: RateLimitContext,
    /// Registries for reusable system components
    pub registries: RegistryContext,
    /// Persistence data stores
    pub persistence: PersistenceContext,
    /// PromptAssembler — 8-layer prompt assembly (ARCH-03)
    pub prompt_assembler: crate::orchestration::prompt_layers::PromptAssembler,
    /// Prompt manager for prompt template management
    pub prompt_manager: PromptManager,
    /// Verbose logging flag
    pub verbose: bool,
    /// Output stream for responses
    pub output: Arc<Mutex<Box<dyn tokio::io::AsyncWrite + Send + Unpin>>>,
    /// Serializes concurrent `/rpc` calls to prevent pipe-swapping race conditions.
    /// Per-server-instance (not global) to avoid the RPC_SERIAL bottleneck (F-GAP-49).
    pub rpc_serial: tokio::sync::Mutex<()>,
    /// Shutdown notification mechanism
    pub shutdown_notify: Arc<Notify>,
    /// Skill market registry for external skill discovery and installation
    pub skill_market_registry: Option<Arc<SkillMarketRegistry>>,
    /// DrainGuard for graceful shutdown
    pub drain_guard: DrainGuard,
    /// Tool registry for built-in tool execution
    pub tool_registry: Arc<ToolRegistry>,
    /// WebSocket hub for real-time push to connected clients
    pub websocket_hub: Option<Arc<crate::protocol::websocket::WebSocketHub>>,
    /// Optional multimodal processor for document, audio, video, and repo analysis.
    /// When `None`, the chat pipeline falls back to text-only processing.
    pub multimodal_processor: Option<crate::multimodal::MultimodalProcessor>,
}

impl AcpServer {
    /// Get the flow manager handle
    pub fn flow_manager(&self) -> Option<Arc<FlowManager>> {
        self.model_deps.flow_manager.clone()
    }

    /// Get the agent registry handle
    pub fn agent_registry(&self) -> Option<Arc<AgentRegistry>> {
        self.model_deps.agent_registry.clone()
    }

    /// Get the response cache handle
    pub fn response_cache(&self) -> Option<Arc<ResponseCache>> {
        self.cache_deps.cache.response_cache.clone()
    }

    /// Get the vector store handle
    pub fn vector_store(&self) -> Option<Arc<VectorStore>> {
        self.cache_deps.cache.vector_store.clone()
    }

    /// Get total requests count
    pub fn total_requests(&self) -> u64 {
        self.observability.metrics.total_requests()
    }

    /// Get server status
    pub fn get_status(&self) -> crate::acp::prelude::ServerStatus {
        use crate::acp::prelude::{MetricsSnapshot, ServerStatus};

        let mut total_requests = self.observability.metrics.total_requests();
        let mut successful_requests = self.observability.metrics.successful_requests();
        let mut failed_requests = self.observability.metrics.failed_requests();
        let mut avg_request_duration_ms = self.observability.metrics.avg_request_duration_ms();

        if let Some(snapshot) = crate::observability::performance::global_metrics_snapshot() {
            total_requests = total_requests.max(snapshot.total_ops);
            successful_requests = successful_requests.max(snapshot.successful_ops);
            failed_requests = failed_requests.max(snapshot.failed_ops);
            if snapshot.total_ops > 0 {
                avg_request_duration_ms = snapshot.avg_latency_ms;
            }
        }

        let mut metrics = MetricsSnapshot {
            total_requests,
            successful_requests,
            failed_requests,
            avg_request_duration_ms,
            active_requests: self.observability.metrics.active_requests(),
            cache_hit_rate: 0.0,
            circuit_breaker_open_count: with_acp_lock(
                "circuit_breakers",
                self.resilience.circuit_breakers.as_ref(),
                |guard| guard.open_count(),
            ),
            memory_usage_bytes: crate::observability::performance::get_memory_usage(),
            cpu_usage_percent: 0.0,
            ..MetricsSnapshot::default()
        };
        let runtime_snapshot = self.observability.metrics.snapshot();
        metrics.chat_requests_total = runtime_snapshot.chat_requests_total;
        metrics.vector_search_total = runtime_snapshot.vector_search_total;
        metrics.vector_hit_total = runtime_snapshot.vector_hit_total;
        metrics.vector_store_total = runtime_snapshot.vector_store_total;
        metrics.summary_read_total = runtime_snapshot.summary_read_total;
        metrics.summary_hit_total = runtime_snapshot.summary_hit_total;
        metrics.summary_store_total = runtime_snapshot.summary_store_total;
        metrics.review_gate_total = runtime_snapshot.review_gate_total;
        metrics.review_gate_approved_total = runtime_snapshot.review_gate_approved_total;
        metrics.review_gate_rejected_total = runtime_snapshot.review_gate_rejected_total;
        metrics.review_gate_timeout_total = runtime_snapshot.review_gate_timeout_total;
        metrics.review_gate_degraded_total = runtime_snapshot.review_gate_degraded_total;
        metrics.review_gate_invalid_response_total =
            runtime_snapshot.review_gate_invalid_response_total;

        let lifecycle = self
            .resilience
            .lifecycle_state
            .read()
            .map(|guard| guard.snapshot())
            .unwrap_or_else(|poisoned| {
                tracing::warn!(
                    "ACP lock 'lifecycle_state' was poisoned; continuing with recovered state"
                );
                poisoned.into_inner().snapshot()
            });

        let circuit_breakers = with_acp_lock(
            "circuit_breakers",
            self.resilience.circuit_breakers.as_ref(),
            |guard| guard.snapshots(),
        );

        let maintenance = self
            .resilience
            .maintenance_tracker
            .read()
            .map(|guard| guard.snapshot())
            .unwrap_or_else(|poisoned| {
                tracing::warn!(
                    "ACP lock 'maintenance_tracker' was poisoned; continuing with recovered state"
                );
                poisoned.into_inner().snapshot()
            });

        // Snapshot governance subsystem health from the harness bus profile
        let governance = self.governance_deps.harness_bus.as_ref().map(|hb| {
            let profile = hb.governance_profile();
            crate::governance::status::GovernanceStatus::current(&profile)
        });

        ServerStatus {
            metrics,
            lifecycle,
            circuit_breakers,
            maintenance,
            governance,
            timestamp: crate::acp::prelude::now_ts(),
        }
    }

    /// Check if server is healthy
    pub fn is_healthy(&self) -> bool {
        self.resilience
            .lifecycle_state
            .read()
            .map(|guard| guard.is_healthy())
            .unwrap_or_else(|poisoned| {
                tracing::warn!(
                    "ACP lock 'lifecycle_state' was poisoned; continuing with recovered state"
                );
                poisoned.into_inner().is_healthy()
            })
    }

    /// Check if shutdown has been requested
    pub fn shutdown_requested(&self) -> bool {
        self.resilience
            .lifecycle_state
            .read()
            .map(|guard| guard.shutdown_requested())
            .unwrap_or_else(|poisoned| {
                tracing::warn!(
                    "ACP lock 'lifecycle_state' was poisoned; continuing with recovered state"
                );
                poisoned.into_inner().shutdown_requested()
            })
    }

    /// Begin shutdown process
    pub fn begin_shutdown(&self) {
        self.resilience
            .lifecycle_state
            .write()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("lifecycle_state poisoned during shutdown; recovering state");
                poisoned.into_inner()
            })
            .begin_shutdown();
    }

    /// Get maintenance tracker reference
    pub fn maintenance(&self) -> &Arc<std::sync::RwLock<MaintenanceTracker>> {
        &self.resilience.maintenance_tracker
    }

    /// Get circuit breakers reference
    pub fn circuit_breakers(&self) -> &Arc<StdMutex<CircuitBreakerRegistry>> {
        &self.resilience.circuit_breakers
    }

    /// Get metrics reference
    pub fn metrics(&self) -> &Arc<RuntimeMetrics> {
        &self.observability.metrics
    }

    /// Get a reference to the thread-safe audit log.
    pub fn audit_log(&self) -> &ThreadSafeAuditLog {
        &self.session.audit_log
    }

    /// Get audit health information: total entries and last write time.
    pub fn audit_health(&self) -> serde_json::Value {
        serde_json::json!({
            "total_entries": self.session.audit_log.len(),
            "last_write_time": self.session.audit_log.last_write_time(),
        })
    }

    /// Get the artifact ledger handle
    pub fn artifact_ledger(&self) -> Option<Arc<ArtifactLedger>> {
        self.persistence
            .artifact_ledger
            .lock()
            .ok()
            .map(|guard| Arc::new(guard.clone()))
    }

    /// Increment the request counter and return the new value
    pub fn increment_request_counter(&self) -> u64 {
        self.observability.metrics.inc_successful_requests();
        self.observability.metrics.total_requests()
    }

    pub fn register_skill(&self, skill: Arc<dyn crate::orchestration::skill::Skill>) {
        let mut registry = self
            .orchestration_deps
            .skill_registry
            .write()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
        if let Err(err) = registry.register(skill) {
            tracing::warn!("skill registration failed: {err}");
        }
    }

    /// Create a `FullAutoFlow` using this server's real skill and tool registries.
    pub fn full_auto_flow(&self) -> crate::orchestration::full_auto::FullAutoFlow {
        crate::orchestration::full_auto::FullAutoFlow::new_with_registries(
            self.orchestration_deps.skill_registry.clone(),
            self.tool_registry.clone(),
        )
    }

    // ── B51-25: Key subsystem accessors ────────────────────────────────────

    /// Get the capability bus handle
    pub fn capability_bus(
        &self,
    ) -> Option<Arc<crate::intelligence::capability_bus::core::CapabilityBus>> {
        self.governance_deps.capability_bus.clone()
    }

    /// Get the harness bus handle
    pub fn harness_bus(&self) -> Option<Arc<crate::governance::harness_bus::HarnessBus>> {
        self.governance_deps.harness_bus.clone()
    }

    /// Get the session manager handle
    pub fn session_manager(&self) -> Option<Arc<crate::acp::r#impl::session::SessionManager>> {
        self.session.session_manager.clone()
    }

    /// Get the session registry handle
    pub fn session_registry(&self) -> Option<Arc<crate::protocol::session_sync::SessionRegistry>> {
        self.session.session_registry.clone()
    }

    /// Get the WebSocket hub handle
    pub fn websocket_hub(&self) -> Option<Arc<crate::protocol::websocket::WebSocketHub>> {
        self.websocket_hub.clone()
    }

    /// Get the tool registry reference
    pub fn tool_registry(&self) -> &Arc<crate::orchestration::tool::ToolRegistry> {
        &self.tool_registry
    }

    /// Get lifecycle state reference
    pub fn lifecycle_state(&self) -> &Arc<std::sync::RwLock<crate::acp::prelude::LifecycleState>> {
        &self.resilience.lifecycle_state
    }

    /// Get conversation state reference
    pub fn conversation_state(
        &self,
    ) -> &Arc<tokio::sync::Mutex<crate::acp::prelude::ConversationState>> {
        &self.session.conversation_state
    }

    /// Get runtime config reference
    pub fn runtime_config(&self) -> &crate::config::RuntimeConfig {
        &self.runtime_config
    }

    /// Get rate limit middleware reference
    pub fn rate_limit_middleware(
        &self,
    ) -> Option<&crate::protocol::rate_limit::RateLimitMiddleware> {
        self.rate_limiting.rate_limit_middleware.as_deref()
    }

    /// Get prompt manager reference
    pub fn prompt_manager(&self) -> &crate::acp::r#impl::request::prompts_pack::PromptManager {
        &self.prompt_manager
    }

    /// Get the drain guard reference
    pub fn drain_guard(&self) -> &DrainGuard {
        &self.drain_guard
    }
}

/// Server builder for constructing AcpServer instances
pub struct ServerBuilder {
    flow_manager: Option<Arc<FlowManager>>,
    agent_registry: Option<Arc<AgentRegistry>>,
    response_cache: Option<Arc<ResponseCache>>,
    vector_store: Option<Arc<VectorStore>>,
    artifact_ledger: Option<ArtifactLedger>,
    memory_response_cache: Option<MemoryResponseCache>,
    config_path: Option<String>,
    verbose: bool,
    harness_bus: Option<Arc<HarnessBus>>,
    capability_bus: Option<Arc<CapabilityBus>>,
    task_graph_store: Option<Arc<TaskGraphStore>>,
    scheduler: Option<Arc<AgentWorkerScheduler>>,
    provenance_ledger: Option<Arc<ProvenanceLedger>>,
    planner_executor_config: crate::orchestration::planner_executor::PlannerExecutorConfig,
    approval_engine:
        Option<Arc<tokio::sync::RwLock<crate::governance::approval_engine::ApprovalEngine>>>,
    injection_detector: Option<Arc<crate::security::prompt_injection::InjectionDetector>>,
    safety_checker: Option<Arc<crate::security::content_safety::SafetyChecker>>,
    hash_chain_auditor:
        Option<Arc<std::sync::Mutex<crate::security::audit_integrity::HashChainAuditor>>>,
    secret_manager: Option<Arc<crate::security::secret_rotation::SecretManager>>,
    memory_persistence: Option<Arc<crate::memory::memory_persistence::MemoryPersistence>>,
    memory_retrieval_engine: Option<Arc<MemoryRetrievalEngine>>,
    evolution_loop: Option<
        Arc<
            tokio::sync::Mutex<crate::orchestration::self_evolution::evolution_loop::EvolutionLoop>,
        >,
    >,
    // ── Security scanning (GAP-B52) ──────────────────────────────────────
    dependency_vulnerability_scanner:
        Option<Arc<crate::security::vulnerability_scan::DependencyVulnerabilityScanner>>,
    secret_exposure_detector:
        Option<Arc<crate::security::vulnerability_scan::SecretExposureDetector>>,
    permit_exposure_analyzer:
        Option<Arc<crate::security::vulnerability_scan::PermitExposureAnalyzer>>,
    security_advisor: Option<Arc<crate::security::security_advisor::SecurityAdvisorAgent>>,
    /// Policy reloader for hot-reloading governance policies (GAP-B58-D04)
    policy_reloader:
        Option<Arc<std::sync::Mutex<crate::governance::reloadable_policy::PolicyReloader>>>,
    /// Optional multimodal processor for document, audio, video, and repo analysis.
    multimodal_processor: Option<crate::multimodal::MultimodalProcessor>,
    /// Runtime config for gating governance, tenant quotas, etc.
    runtime_config: Option<RuntimeConfig>,
}

impl ServerBuilder {
    /// Create a new server builder
    pub fn new() -> Self {
        Self {
            flow_manager: None,
            agent_registry: None,
            response_cache: None,
            vector_store: None,
            artifact_ledger: None,
            memory_response_cache: None,
            config_path: None,
            verbose: false,
            harness_bus: None,
            capability_bus: None,
            task_graph_store: None,
            scheduler: None,
            provenance_ledger: None,
            planner_executor_config: Default::default(),
            approval_engine: None,
            injection_detector: None,
            safety_checker: None,
            hash_chain_auditor: None,
            secret_manager: None,
            memory_persistence: None,
            memory_retrieval_engine: None,
            evolution_loop: None,
            dependency_vulnerability_scanner: None,
            secret_exposure_detector: None,
            permit_exposure_analyzer: None,
            security_advisor: None,
            policy_reloader: None,
            multimodal_processor: None,
            runtime_config: None,
        }
    }

    /// Set config path
    pub fn with_config_path(mut self, config_path: Option<String>) -> Self {
        self.config_path = config_path;
        self
    }

    /// Set the flow manager
    pub fn with_flow_manager(mut self, flow_manager: Arc<FlowManager>) -> Self {
        self.flow_manager = Some(flow_manager);
        self
    }

    /// Set the agent registry
    pub fn with_agent_registry(mut self, agent_registry: Arc<AgentRegistry>) -> Self {
        self.agent_registry = Some(agent_registry);
        self
    }

    /// Set the response cache
    pub fn with_response_cache(mut self, response_cache: Arc<ResponseCache>) -> Self {
        self.response_cache = Some(response_cache);
        self
    }

    /// Set the vector store
    pub fn with_vector_store(mut self, vector_store: Arc<VectorStore>) -> Self {
        self.vector_store = Some(vector_store);
        self
    }

    /// Set the artifact ledger
    pub fn with_artifact_ledger(mut self, artifact_ledger: ArtifactLedger) -> Self {
        self.artifact_ledger = Some(artifact_ledger);
        self
    }

    /// Set the memory response cache
    /// Set the approval engine
    pub fn with_approval_engine(
        mut self,
        engine: Arc<tokio::sync::RwLock<crate::governance::approval_engine::ApprovalEngine>>,
    ) -> Self {
        self.approval_engine = Some(engine);
        self
    }

    /// Set the injection detector
    pub fn with_injection_detector(
        mut self,
        detector: Arc<crate::security::prompt_injection::InjectionDetector>,
    ) -> Self {
        self.injection_detector = Some(detector);
        self
    }

    /// Set the safety checker
    pub fn with_safety_checker(
        mut self,
        checker: Arc<crate::security::content_safety::SafetyChecker>,
    ) -> Self {
        self.safety_checker = Some(checker);
        self
    }

    /// Set the hash chain auditor
    pub fn with_hash_chain_auditor(
        mut self,
        auditor: Arc<std::sync::Mutex<crate::security::audit_integrity::HashChainAuditor>>,
    ) -> Self {
        self.hash_chain_auditor = Some(auditor);
        self
    }

    /// Set the secret manager
    pub fn with_secret_manager(
        mut self,
        manager: Arc<crate::security::secret_rotation::SecretManager>,
    ) -> Self {
        self.secret_manager = Some(manager);
        self
    }

    /// Set the memory persistence manager
    pub fn with_memory_persistence(
        mut self,
        mp: Arc<crate::memory::memory_persistence::MemoryPersistence>,
    ) -> Self {
        self.memory_persistence = Some(mp);
        self
    }

    /// Set the memory retrieval engine (GAP-B52-13).
    ///
    /// To create an engine from a `MemoryPersistence` instance, use the
    /// convenience function [`wire_memory_retrieval`](crate::memory::wire_memory_retrieval).
    pub fn with_memory_retrieval_engine(mut self, engine: Arc<MemoryRetrievalEngine>) -> Self {
        self.memory_retrieval_engine = Some(engine);
        self
    }

    /// Set the evolution loop
    pub fn with_evolution_loop(
        mut self,
        evolution_loop: Arc<
            tokio::sync::Mutex<crate::orchestration::self_evolution::evolution_loop::EvolutionLoop>,
        >,
    ) -> Self {
        self.evolution_loop = Some(evolution_loop);
        self
    }

    /// Set the dependency vulnerability scanner
    pub fn with_dependency_vulnerability_scanner(
        mut self,
        scanner: Arc<crate::security::vulnerability_scan::DependencyVulnerabilityScanner>,
    ) -> Self {
        self.dependency_vulnerability_scanner = Some(scanner);
        self
    }

    /// Set the secret exposure detector
    pub fn with_secret_exposure_detector(
        mut self,
        detector: Arc<crate::security::vulnerability_scan::SecretExposureDetector>,
    ) -> Self {
        self.secret_exposure_detector = Some(detector);
        self
    }

    /// Set the permit exposure analyzer
    pub fn with_permit_exposure_analyzer(
        mut self,
        analyzer: Arc<crate::security::vulnerability_scan::PermitExposureAnalyzer>,
    ) -> Self {
        self.permit_exposure_analyzer = Some(analyzer);
        self
    }

    /// Set the security advisor agent
    pub fn with_security_advisor(
        mut self,
        advisor: Arc<crate::security::security_advisor::SecurityAdvisorAgent>,
    ) -> Self {
        self.security_advisor = Some(advisor);
        self
    }

    /// Set the policy reloader for hot-reloading governance policies (GAP-B58-D04)
    pub fn with_policy_reloader(
        mut self,
        reloader: Arc<std::sync::Mutex<crate::governance::reloadable_policy::PolicyReloader>>,
    ) -> Self {
        self.policy_reloader = Some(reloader);
        self
    }

    /// Build the server
    pub fn build(self) -> AcpServer {
        use crate::acp::prelude::{
            CircuitBreakerRegistry, ConversationState, InflightLimiter, LifecycleState,
            MaintenanceTracker, OnlineControllerState, PhaseRateLimiter, ReviewTimeoutPolicy,
            RuntimeMetrics,
        };

        let metrics = Arc::new(RuntimeMetrics::default());
        let (outcome_tx, mut outcome_rx) = mpsc::unbounded_channel::<OutcomeEvent>();
        let online_controller = Arc::new(StdMutex::new(OnlineControllerState::default()));
        let circuit_breakers = Arc::new(StdMutex::new(CircuitBreakerRegistry::default()));
        let hyper_resilience = Arc::new(
            crate::resilience::hyper_resilience::HyperResilienceEngine::new(
                crate::resilience::hyper_resilience::ResilienceConfig::default(),
            ),
        );
        let maintenance_tracker = Arc::new(std::sync::RwLock::new(MaintenanceTracker::new()));
        let inflight_limiter = Arc::new(std::sync::RwLock::new(InflightLimiter::default()));
        let lifecycle_state = Arc::new(std::sync::RwLock::new(LifecycleState::new()));
        let conversation_state = Arc::new(Mutex::new(ConversationState::default()));
        let phase_rate_limiter = Arc::new(StdMutex::new(PhaseRateLimiter::default()));
        let review_timeout_policy = Arc::new(std::sync::RwLock::new(ReviewTimeoutPolicy {
            timeout_seconds: None,
            fail_on_timeout: false,
        }));

        let adaptive_model_selector = Arc::new(StdMutex::new(AdaptiveModelSelector::default()));
        let mut failure_prevention_state = FailurePrevention::new();
        if let Some(agent_registry) = &self.agent_registry {
            for name in agent_registry.names() {
                failure_prevention_state.register_service(&name);
            }
        }
        let failure_prevention = Arc::new(StdMutex::new(failure_prevention_state));
        let flow_model_selector = Arc::new(StdMutex::new(FlowModelSelector {}));
        let memory_response_cache = Arc::new(StdMutex::new(
            self.memory_response_cache.unwrap_or_default(),
        ));
        let memory_store = Arc::new(StdMutex::new(MemoryStore::new(MemoryPolicy::default())));

        // Initialize skill registry with disk-persisted prompt-based skills
        let mut registry = SkillRegistry::default();
        let prompt_skills_path =
            std::path::PathBuf::from("./skills-cache").join("prompt_skills.json");
        registry.set_persistence_path(prompt_skills_path);
        if let Err(e) = registry.load_prompt_skills_from_disk() {
            tracing::warn!("Failed to load prompt skills from disk: {}", e);
        }
        let skill_registry = Arc::new(std::sync::RwLock::new(registry));

        // Register built-in skills
        {
            let mut reg = skill_registry.write().unwrap_or_else(|e| e.into_inner());
            let _ = reg.register(Arc::new(crate::orchestration::skill::EchoSkill));
            let _ = reg.register(Arc::new(
                crate::orchestration::skill::SkillCreatorSkill::new(skill_registry.clone()),
            ));
        }

        // Discover and register local skills from ~/.agents/skills/
        // so user-authored SKILL.md files are available to the server.
        {
            let mut reg = skill_registry.write().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = reg.discover_and_register_local_skills(None) {
                tracing::warn!(
                    "Failed to discover local skills in ServerBuilder::build: {}",
                    e
                );
            }
        }

        // Wire the skill registry into the global discovery and tool layers
        crate::acp::r#impl::request::tools_pack::init_skill_discovery(skill_registry.clone());
        crate::orchestration::tool::set_skill_registry(skill_registry.clone());

        // Spawn background task to periodically rescan ~/.agents/skills/ for new skills.
        // New SKILL.md files placed in the directory will be registered automatically
        // without requiring a server restart.
        // `spawn_skill_refresh_task` returns `None` when no tokio runtime is active
        // (e.g. during synchronous tests). We silently accept that case.
        let _ = crate::orchestration::skill::registry::spawn_skill_refresh_task(
            skill_registry.clone(),
            None,
        );

        // Resolve runtime config: use provided one or default.
        let runtime_config = self.runtime_config.unwrap_or_default();

        let telemetry_runtime = Arc::new(StdMutex::new(TelemetryRuntime::new(&runtime_config)));

        // ── B54-073: Gate governance initialization ──────────────────────
        let governance_enabled = runtime_config.governance_enabled;
        if !governance_enabled {
            tracing::info!(
                "governance_enabled=false — skipping governance subsystem initialization"
            );
        } else {
            let policy_mode = &runtime_config.governance_policy_mode;
            if !policy_mode.is_empty() {
                tracing::info!(
                    "governance_policy_mode={} — governance subsystem initialized with policy mode",
                    policy_mode
                );
            }
        }

        // ── B54-016: Wire continuous learning review cycle (background task) ──
        if let Some(ref capability_bus) = self.capability_bus {
            let cl_bus = capability_bus.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(600)); // 10 min
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    interval.tick().await;
                    // Clone the center briefly under the outer lock, then release
                    // the lock before calling the async review_cycle. This avoids
                    // holding a !Send std::sync::MutexGuard across an .await point.
                    let center_for_review = match cl_bus.continuous_learning.lock() {
                        Ok(cl) => cl.clone(),
                        Err(poisoned) => {
                            tracing::warn!(
                                "continuous_learning mutex poisoned in review_cycle task — recovering"
                            );
                            poisoned.into_inner().clone()
                        }
                    };
                    let (replayed, evicted, patterns) =
                        center_for_review.review_cycle("local-agent").await;
                    if replayed > 0 || evicted > 0 || patterns > 0 {
                        tracing::debug!(
                            "ContinuousLearning: review_cycle replayed={} evicted={} patterns={}",
                            replayed,
                            evicted,
                            patterns
                        );
                    }
                }
            });
        }

        // Create a default PUA enforcement plan
        let pua_enforcement_plan = Arc::new(StdMutex::new(crate::pua::PuaEnforcementPlan {
            escalation_level: String::new(),
            mandatory_roles: Vec::new(),
            red_lines: Vec::new(),
            quality_compass: Vec::new(),
            mandatory_safeguards: Vec::new(),
            mandatory_evidence: Vec::new(),
            stage_requirements: Vec::new(),
        }));

        let artifact_ledger = Arc::new(StdMutex::new(
            self.artifact_ledger
                .unwrap_or_else(|| ArtifactLedger::new(None)),
        ));
        let responses_api_store = Arc::new(StdMutex::new(HashMap::new()));

        let prompt_manager = PromptManager::new(std::path::PathBuf::from("./prompts"));

        let cache_layer = CacheLayer {
            response_cache: self.response_cache,
            memory_response_cache,
            vector_store: self.vector_store,
            token_cache: Arc::new(crate::intelligence::token_cache::TokenMultiLevelCache::new(
                500,
                200,
                ".goon/token_cache",
            )),
            semantic_cache: Arc::new(std::sync::RwLock::new(
                crate::memory::semantic_cache::SemanticResponseCache::new(Default::default()),
            )),
        };

        // Spawn background processor for online_controller outcome events
        let outcome_controller = online_controller.clone();
        tokio::spawn(async move {
            while let Some(event) = outcome_rx.recv().await {
                let mut guard = outcome_controller.lock().unwrap_or_else(|p| p.into_inner());
                match event {
                    OutcomeEvent::AgentOutcome {
                        phase_name,
                        agent_name,
                        success,
                        duration_ms,
                    } => {
                        guard.record_agent_outcome(&phase_name, &agent_name, success, duration_ms);
                    }
                    OutcomeEvent::PhaseOutcome {
                        phase_name,
                        success,
                        duration_ms,
                    } => {
                        guard.record_phase_outcome(&phase_name, success, duration_ms);
                    }
                }
            }
        });

        AcpServer {
            cache_deps: CacheServerDeps {
                cache: cache_layer,
                vector_config: None,
                autotune: None,
                autotune_config: None,
                autotune_state_path: None,
            },
            model_deps: ModelServerDeps {
                flow_manager: self.flow_manager,
                agent_registry: self.agent_registry,
                adaptive_model_selector,
                flow_model_selector,
            },
            governance_deps: GovernanceServerDeps {
                harness_bus: if governance_enabled {
                    self.harness_bus
                } else {
                    None
                },
                capability_bus: if governance_enabled {
                    self.capability_bus
                } else {
                    None
                },
                pua_enforcement_plan,
                rbac_enforcer: None,
                provenance_ledger: if governance_enabled {
                    self.provenance_ledger
                } else {
                    None
                },
                approval_engine: if governance_enabled {
                    self.approval_engine
                } else {
                    None
                },
                injection_detector: if governance_enabled {
                    self.injection_detector
                } else {
                    None
                },
                safety_checker: if governance_enabled {
                    self.safety_checker
                } else {
                    None
                },
                hash_chain_auditor: if governance_enabled {
                    self.hash_chain_auditor
                } else {
                    None
                },
                secret_manager: if governance_enabled {
                    self.secret_manager
                } else {
                    None
                },
                memory_persistence: if governance_enabled {
                    self.memory_persistence
                } else {
                    None
                },
                memory_retrieval_engine: if governance_enabled {
                    self.memory_retrieval_engine
                } else {
                    None
                },
                evolution_loop: if governance_enabled {
                    self.evolution_loop
                } else {
                    None
                },
                dependency_vulnerability_scanner: if governance_enabled {
                    self.dependency_vulnerability_scanner
                } else {
                    None
                },
                secret_exposure_detector: if governance_enabled {
                    self.secret_exposure_detector
                } else {
                    None
                },
                permit_exposure_analyzer: if governance_enabled {
                    self.permit_exposure_analyzer
                } else {
                    None
                },
                security_advisor: if governance_enabled {
                    self.security_advisor
                } else {
                    None
                },
                policy_reloader: if governance_enabled {
                    self.policy_reloader
                } else {
                    None
                },
            },
            orchestration_deps: OrchestrationServerDeps {
                scheduler: self.scheduler,
                planner: crate::orchestration::planner_executor::Planner,
                executor: crate::orchestration::planner_executor::Executor,
                planner_executor_config: self.planner_executor_config,
                skill_registry,
            },
            runtime_config,
            config_path: self.config_path,
            observability: ObservabilityLayer {
                metrics,
                telemetry_runtime,
                alert_manager: {
                    let am = Arc::new(StdMutex::new(
                        crate::observability::alert_manager::AlertManager::new(
                            crate::observability::alert_manager::default_alert_rules(),
                        ),
                    ));
                    // GAP-B58-C12: Call configure_from_env() so webhook is picked up
                    am.lock()
                        .unwrap_or_else(|e| {
                            tracing::warn!("AlertManager lock poisoned – recovering");
                            e.into_inner()
                        })
                        .configure_from_env();
                    am
                },
            },
            resilience: ResilienceContext {
                online_controller,
                outcome_tx,
                circuit_breakers,
                hyper_resilience,
                maintenance_tracker,
                inflight_limiter,
                lifecycle_state,
                review_timeout_policy,
                failure_prevention,
                phase_rate_limiter,
            },
            session: SessionContext {
                conversation_state,
                session_manager: None,
                session_registry: None,
                audit_log: ThreadSafeAuditLog::new_with_default_path(10_000),
                responses_api_store,
            },
            rate_limiting: RateLimitContext {
                rate_limit_middleware: None,
                tenant_budget: Arc::new(StdMutex::new(
                    crate::governance::hardening::TenantBudgetEnforcer::new(),
                )),
                global_rate_limiter: GlobalRateLimiter::new(
                    crate::security::rate_limiter::RateLimitConfig::default(),
                ),
            },
            registries: RegistryContext {
                schema_registry: Arc::new(StdMutex::new(SchemaRegistry::new())),
                optimizer_registry: Arc::new(StdMutex::new(OptimizerRegistry::new())),
                promotion_registry: Arc::new(StdMutex::new(PromotionRegistry::new())),
                evaluation_suite: Arc::new(StdMutex::new(
                    crate::intelligence::evaluation::BenchmarkSuite::new(),
                )),
                fork_registry: Arc::new(StdMutex::new(ForkRegistry::new(ForkConfig::default()))),
            },
            persistence: PersistenceContext {
                memory_store,
                artifact_ledger,
                task_graph_store: self.task_graph_store,
            },
            prompt_assembler: PromptAssembler,
            prompt_manager,
            verbose: self.verbose,
            output: Arc::new(Mutex::new(
                Box::new(tokio::io::stdout()) as Box<dyn tokio::io::AsyncWrite + Send + Unpin>
            )),
            rpc_serial: tokio::sync::Mutex::new(()),
            shutdown_notify: Arc::new(Notify::new()),
            skill_market_registry: None,
            drain_guard: DrainGuard::default(),
            tool_registry: Arc::new(ToolRegistry::new()),
            websocket_hub: None,
            multimodal_processor: self.multimodal_processor,
        }
    }
}

impl Default for ServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}
