//! ACP Server - Main server implementation
//!
//! This module contains the main AcpServer struct definition and related
//! server management functionality.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::Result;
use tokio::sync::{Mutex, Notify};

use crate::acp::prelude::RuntimeMetrics;
use crate::adaptive_selector::AdaptiveModelSelector;
use crate::observability::provenance::ProvenanceLedger;

use crate::agent::AgentRegistry;
use crate::cache::ResponseCache;
use crate::config::{AutoTuneConfig, AutoTuneState, RuntimeConfig, VectorConfig};

use crate::failure_prevention::FailurePrevention;
use crate::flow::FlowManager;
use crate::flow_with_models::FlowModelSelector;
use crate::governance::harness_bus::HarnessBus;
use crate::intelligence::capability_bus::core::CapabilityBus;
use crate::intelligence::token_cache::TokenMultiLevelCache;
use crate::memory_module::{MemoryPolicy, MemoryStore};
use crate::memory_response_cache::MemoryResponseCache;
use crate::observability::telemetry::TelemetryRuntime;
use crate::orchestration::fork_registry::ForkRegistry;
use crate::orchestration::promotion_plugin::PromotionRegistry;
use crate::orchestration::prompt_layers::PromptAssembler;
use crate::orchestration::scheduler::AgentWorkerScheduler;
use crate::orchestration::skill::SkillRegistry;
use crate::orchestration::task_graph_store::TaskGraphStore;
use crate::orchestration::task_schema::SchemaRegistry;
use crate::orchestration::workflow_optimizer::OptimizerRegistry;
use crate::reinforcement::ArtifactLedger;
use crate::vector::VectorStore;

use super::prelude::{
    with_acp_lock, AcpLockMonitor, CircuitBreakerRegistry, ConversationState, InflightLimiter,
    LifecycleState, MaintenanceTracker, OnlineControllerState, PhaseRateLimiter,
    ReviewTimeoutPolicy, ACP_LOCK_CIRCUIT_BREAKERS, ACP_LOCK_LIFECYCLE, ACP_LOCK_MAINTENANCE,
};

/// Cache-related subsystems grouped together
pub struct CacheLayer {
    /// Response cache (SQLite-based)
    pub response_cache: Option<Arc<ResponseCache>>,
    /// Memory response cache
    pub memory_response_cache: Arc<StdMutex<MemoryResponseCache>>,
    /// Vector store for similarity search and memory
    pub vector_store: Option<Arc<VectorStore>>,
    /// Multi-level token cache for Agent output reuse (L1 exact, L2 semantic, L3 template)
    pub token_cache: Arc<TokenMultiLevelCache>,
}

/// Observability-related subsystems grouped together
pub struct ObservabilityLayer {
    /// Runtime metrics collection
    pub metrics: Arc<RuntimeMetrics>,
    /// ACP lock monitoring and poison recovery telemetry
    pub lock_monitor: Arc<AcpLockMonitor>,
    /// Telemetry runtime
    pub telemetry_runtime: Arc<StdMutex<TelemetryRuntime>>,
}

/// Main ACP server structure
///
/// This struct represents the core ACP server that handles incoming requests,
/// manages agents, and coordinates the overall system flow.
pub struct AcpServer {
    /// Flow manager for handling request routing through phases
    pub flow_manager: Option<Arc<FlowManager>>,
    /// Agent registry for managing available agents
    pub agent_registry: Option<Arc<AgentRegistry>>,
    /// Cache-related subsystems
    /// Cache-related subsystems (response cache, vector store, token cache)
    pub cache: CacheLayer,
    /// Vector store configuration
    pub vector_config: Option<VectorConfig>,
    /// Autotune state for adaptive configuration
    pub autotune: Option<Arc<Mutex<AutoTuneState>>>,
    /// Autotune configuration
    pub autotune_config: Option<AutoTuneConfig>,
    /// Path to autotune state file
    pub autotune_state_path: Option<String>,
    /// Runtime configuration
    pub runtime_config: RuntimeConfig,
    /// Loaded config file path if available
    pub config_path: Option<String>,
    /// Observability-related subsystems
    pub observability: ObservabilityLayer,
    /// Online controller for adaptive strategy from live outcomes
    pub online_controller: Arc<StdMutex<OnlineControllerState>>,
    /// Circuit breaker registry for failure prevention
    pub circuit_breakers: Arc<StdMutex<CircuitBreakerRegistry>>,
    /// Maintenance tracker for system health monitoring
    pub maintenance_tracker: Arc<StdMutex<MaintenanceTracker>>,
    /// Inflight request limiter
    pub inflight_limiter: Arc<StdMutex<InflightLimiter>>,
    /// Lifecycle state management
    pub lifecycle_state: Arc<StdMutex<LifecycleState>>,
    /// Conversation state management
    pub conversation_state: Arc<Mutex<ConversationState>>,
    /// Phase rate limiter
    pub phase_rate_limiter: Arc<StdMutex<PhaseRateLimiter>>,
    /// Review timeout policy
    pub review_timeout_policy: Arc<StdMutex<ReviewTimeoutPolicy>>,
    /// Adaptive model selector
    pub adaptive_model_selector: Arc<StdMutex<AdaptiveModelSelector>>,
    /// Failure prevention system
    pub failure_prevention: Arc<StdMutex<FailurePrevention>>,
    /// Flow model selector
    pub flow_model_selector: Arc<StdMutex<FlowModelSelector>>,
    /// Cross-request memory policy store
    pub memory_store: Arc<StdMutex<MemoryStore>>,
    /// Registry for MCP skills
    pub skill_registry: Arc<StdMutex<SkillRegistry>>,
    /// PUA enforcement plan
    pub pua_enforcement_plan: Arc<StdMutex<crate::pua::PuaEnforcementPlan>>,
    /// Artifact ledger
    pub artifact_ledger: Arc<StdMutex<ArtifactLedger>>,
    /// HarnessBus strategy engine (BLUE38 ARCH-13)
    pub harness_bus: Option<Arc<HarnessBus>>,
    /// CapabilityBus scheduling coordinator (BLUE38 ARCH-13)
    pub capability_bus: Option<Arc<CapabilityBus>>,
    /// ForkRegistry — sub-agent process isolation (ARCH-05)
    pub fork_registry: Arc<StdMutex<ForkRegistry>>,
    /// Planner — task decomposition engine (F-GAP-05)
    pub planner: crate::orchestration::planner_executor::Planner,
    /// Executor — plan execution engine (F-GAP-05)
    pub executor: crate::orchestration::planner_executor::Executor,
    /// BenchmarkSuite — evaluation suite for agent quality (F-GAP-06)
    pub evaluation_suite: Arc<StdMutex<crate::intelligence::evaluation::BenchmarkSuite>>,
    /// SchemaRegistry — task envelope validation (F-GAP-07)
    pub schema_registry: Arc<StdMutex<crate::orchestration::task_schema::SchemaRegistry>>,
    /// TenantBudgetEnforcer — per-tenant resource quota management (F-GAP-08)
    pub tenant_budget: Arc<StdMutex<crate::governance::hardening::TenantBudgetEnforcer>>,
    /// OptimizerRegistry — workflow optimization plugins (ARCH-11)
    pub optimizer_registry:
        Arc<StdMutex<crate::orchestration::workflow_optimizer::OptimizerRegistry>>,
    /// PromptAssembler — 8-layer prompt assembly (ARCH-03)
    pub prompt_assembler: crate::orchestration::prompt_layers::PromptAssembler,
    /// PromotionRegistry — promotion plugin evaluation (ARCH-10)
    pub promotion_registry:
        Arc<StdMutex<crate::orchestration::promotion_plugin::PromotionRegistry>>,
    /// Verbose logging flag
    pub verbose: bool,
    /// Output stream for responses
    pub output: Arc<Mutex<tokio::io::Stdout>>,
    /// Shutdown notification mechanism
    pub shutdown_notify: Arc<Notify>,
    /// In-memory registry for Responses API objects
    pub responses_api_store: Arc<StdMutex<HashMap<String, serde_json::Value>>>,
    /// Persistent task graph store for checkpoints and recovery
    pub task_graph_store: Option<Arc<TaskGraphStore>>,
    /// Dual-level task scheduler for priority queue and worker pool
    pub scheduler: Option<Arc<AgentWorkerScheduler>>,
    /// Provenance ledger — immutable data lineage tracking
    pub provenance_ledger: Option<Arc<ProvenanceLedger>>,
}

impl AcpServer {
    /// Get the flow manager handle
    pub fn flow_manager(&self) -> Option<Arc<FlowManager>> {
        self.flow_manager.clone()
    }

    /// Get the agent registry handle
    pub fn agent_registry(&self) -> Option<Arc<AgentRegistry>> {
        self.agent_registry.clone()
    }

    /// Get the response cache handle
    pub fn response_cache(&self) -> Option<Arc<ResponseCache>> {
        self.cache.response_cache.clone()
    }

    /// Get the vector store handle
    pub fn vector_store(&self) -> Option<Arc<VectorStore>> {
        self.cache.vector_store.clone()
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
                self.observability.lock_monitor.as_ref(),
                ACP_LOCK_CIRCUIT_BREAKERS,
                self.circuit_breakers.as_ref(),
                |guard| guard.open_count(),
            ),
            memory_usage_bytes: 0,
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

        let lifecycle = with_acp_lock(
            self.observability.lock_monitor.as_ref(),
            ACP_LOCK_LIFECYCLE,
            self.lifecycle_state.as_ref(),
            |guard| guard.snapshot(),
        );

        let circuit_breakers = with_acp_lock(
            self.observability.lock_monitor.as_ref(),
            ACP_LOCK_CIRCUIT_BREAKERS,
            self.circuit_breakers.as_ref(),
            |guard| guard.snapshots(),
        );

        let maintenance = with_acp_lock(
            self.observability.lock_monitor.as_ref(),
            ACP_LOCK_MAINTENANCE,
            self.maintenance_tracker.as_ref(),
            |guard| guard.snapshot(),
        );

        ServerStatus {
            metrics,
            lifecycle,
            circuit_breakers,
            maintenance,
            timestamp: crate::acp::prelude::now_ts(),
        }
    }

    /// Check if server is healthy
    pub fn is_healthy(&self) -> bool {
        with_acp_lock(
            self.observability.lock_monitor.as_ref(),
            ACP_LOCK_LIFECYCLE,
            self.lifecycle_state.as_ref(),
            |guard| guard.is_healthy(),
        )
    }

    /// Check if shutdown has been requested
    pub fn shutdown_requested(&self) -> bool {
        with_acp_lock(
            self.observability.lock_monitor.as_ref(),
            ACP_LOCK_LIFECYCLE,
            self.lifecycle_state.as_ref(),
            |guard| guard.shutdown_requested(),
        )
    }

    /// Begin shutdown process
    pub fn begin_shutdown(&self) {
        with_acp_lock(
            self.observability.lock_monitor.as_ref(),
            ACP_LOCK_LIFECYCLE,
            self.lifecycle_state.as_ref(),
            |guard| guard.begin_shutdown(),
        );
    }

    /// Get maintenance tracker reference
    pub fn maintenance(&self) -> &Arc<StdMutex<MaintenanceTracker>> {
        &self.maintenance_tracker
    }

    /// Get circuit breakers reference
    pub fn circuit_breakers(&self) -> &Arc<StdMutex<CircuitBreakerRegistry>> {
        &self.circuit_breakers
    }

    /// Get metrics reference
    pub fn metrics(&self) -> &Arc<RuntimeMetrics> {
        &self.observability.metrics
    }

    /// Get the artifact ledger handle
    pub fn artifact_ledger(&self) -> Option<Arc<ArtifactLedger>> {
        self.artifact_ledger
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
        if let Ok(mut registry) = self.skill_registry.lock() {
            if let Err(err) = registry.register(skill) {
                tracing::warn!("skill registration failed: {err}");
            }
        }
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
    pub fn with_memory_response_cache(
        mut self,
        memory_response_cache: MemoryResponseCache,
    ) -> Self {
        self.memory_response_cache = Some(memory_response_cache);
        self
    }

    /// Set verbose mode
    pub fn verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Set the harness bus
    pub fn with_harness_bus(mut self, harness_bus: Arc<HarnessBus>) -> Self {
        self.harness_bus = Some(harness_bus);
        self
    }

    /// Set the capability bus
    pub fn with_capability_bus(mut self, capability_bus: Arc<CapabilityBus>) -> Self {
        self.capability_bus = Some(capability_bus);
        self
    }

    /// Set the task graph store
    pub fn with_task_graph_store(mut self, store: Arc<TaskGraphStore>) -> Self {
        self.task_graph_store = Some(store);
        self
    }

    /// Set the dual-level task scheduler
    pub fn with_scheduler(mut self, scheduler: Arc<AgentWorkerScheduler>) -> Self {
        self.scheduler = Some(scheduler);
        self
    }

    /// Set the provenance ledger
    pub fn with_provenance_ledger(mut self, ledger: Arc<ProvenanceLedger>) -> Self {
        self.provenance_ledger = Some(ledger);
        self
    }

    /// Build the server
    pub fn build(self) -> Result<AcpServer> {
        use crate::acp::prelude::{
            CircuitBreakerRegistry, ConversationState, InflightLimiter, LifecycleState,
            MaintenanceTracker, OnlineControllerState, RuntimeMetrics,
        };

        let metrics = Arc::new(RuntimeMetrics::default());
        let lock_monitor = Arc::new(AcpLockMonitor::default());
        let online_controller = Arc::new(StdMutex::new(OnlineControllerState::default()));
        let circuit_breakers = Arc::new(StdMutex::new(CircuitBreakerRegistry::default()));
        let maintenance_tracker = Arc::new(StdMutex::new(MaintenanceTracker::new()));
        let inflight_limiter = Arc::new(StdMutex::new(InflightLimiter::default()));
        let lifecycle_state = Arc::new(StdMutex::new(LifecycleState::new()));
        let conversation_state = Arc::new(Mutex::new(ConversationState::default()));
        let phase_rate_limiter = Arc::new(StdMutex::new(PhaseRateLimiter::default()));
        let review_timeout_policy = Arc::new(StdMutex::new(ReviewTimeoutPolicy {
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
        let skill_registry = Arc::new(StdMutex::new(SkillRegistry::default()));
        let telemetry_runtime = Arc::new(StdMutex::new(TelemetryRuntime::new(
            &RuntimeConfig::default(),
        )));

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

        Ok(AcpServer {
            flow_manager: self.flow_manager,
            agent_registry: self.agent_registry,
            cache: CacheLayer {
                response_cache: self.response_cache,
                memory_response_cache,
                vector_store: self.vector_store,
                token_cache: Arc::new(crate::intelligence::token_cache::TokenMultiLevelCache::new(
                    500,
                    200,
                    ".goon/token_cache",
                )),
            },
            vector_config: None,
            autotune: None,
            autotune_config: None,
            autotune_state_path: None,
            runtime_config: RuntimeConfig::default(),
            config_path: self.config_path,
            observability: ObservabilityLayer {
                metrics,
                lock_monitor,
                telemetry_runtime,
            },
            online_controller,
            circuit_breakers,
            maintenance_tracker,
            inflight_limiter,
            lifecycle_state,
            conversation_state,
            phase_rate_limiter,
            review_timeout_policy,
            adaptive_model_selector,

            failure_prevention,
            flow_model_selector,
            memory_store,
            skill_registry,
            pua_enforcement_plan,
            artifact_ledger,
            harness_bus: self.harness_bus,
            capability_bus: self.capability_bus,
            fork_registry: Arc::new(StdMutex::new(ForkRegistry::new(100))),
            planner: crate::orchestration::planner_executor::Planner,
            executor: crate::orchestration::planner_executor::Executor,
            evaluation_suite: Arc::new(StdMutex::new(
                crate::intelligence::evaluation::BenchmarkSuite::new(),
            )),
            schema_registry: Arc::new(StdMutex::new(SchemaRegistry::new())),
            tenant_budget: Arc::new(StdMutex::new(
                crate::governance::hardening::TenantBudgetEnforcer::new(),
            )),
            optimizer_registry: Arc::new(StdMutex::new(OptimizerRegistry::new())),
            prompt_assembler: PromptAssembler,
            promotion_registry: Arc::new(StdMutex::new(PromotionRegistry::new())),
            verbose: self.verbose,
            output: Arc::new(Mutex::new(tokio::io::stdout())),
            shutdown_notify: Arc::new(Notify::new()),
            responses_api_store,
            task_graph_store: self.task_graph_store,
            scheduler: self.scheduler,
            provenance_ledger: self.provenance_ledger,
        })
    }
}

impl Default for ServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}
