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
use crate::advanced_modules::{DynamicParameterTuner, ResourceAllocator};
use crate::agent::AgentRegistry;
use crate::cache::ResponseCache;
use crate::config::{AutoTuneConfig, AutoTuneState, RuntimeConfig, VectorConfig};
use crate::cost_optimizer::CostOptimizer;
use crate::failure_prevention::FailurePrevention;
use crate::flow::FlowManager;
use crate::flow_with_models::FlowModelSelector;
use crate::memory_module::{MemoryPolicy, MemoryStore};
use crate::memory_response_cache::MemoryResponseCache;
use crate::observability::telemetry::TelemetryRuntime;
use crate::orchestration::skill::SkillRegistry;
use crate::reinforcement::ArtifactLedger;
use crate::vector::VectorStore;

use super::prelude::{
    CircuitBreakerRegistry, ConversationState, InflightLimiter, LifecycleState, MaintenanceTracker,
    OnlineControllerState, PhaseRateLimiter, ReviewTimeoutPolicy,
};

/// Main ACP server structure
///
/// This struct represents the core ACP server that handles incoming requests,
/// manages agents, and coordinates the overall system flow.
pub struct AcpServer {
    /// Flow manager for handling request routing through phases
    pub flow_manager: Option<Arc<FlowManager>>,
    /// Agent registry for managing available agents
    pub agent_registry: Option<Arc<AgentRegistry>>,
    /// Response cache (SQLite-based)
    pub response_cache: Option<Arc<ResponseCache>>,
    /// Vector store for similarity search and memory
    pub vector_store: Option<Arc<VectorStore>>,
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
    /// Runtime metrics collection
    pub metrics: Arc<RuntimeMetrics>,
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
    /// Dynamic parameter tuner
    pub dynamic_parameter_tuner: Arc<StdMutex<DynamicParameterTuner>>,
    /// Resource allocator
    pub resource_allocator: Arc<StdMutex<ResourceAllocator>>,
    /// Cost optimizer
    pub cost_optimizer: Arc<StdMutex<CostOptimizer>>,
    /// Failure prevention system
    pub failure_prevention: Arc<StdMutex<FailurePrevention>>,
    /// Flow model selector
    pub flow_model_selector: Arc<StdMutex<FlowModelSelector>>,
    /// Memory response cache
    pub memory_response_cache: Arc<StdMutex<MemoryResponseCache>>,
    /// Cross-request memory policy store
    pub memory_store: Arc<StdMutex<MemoryStore>>,
    /// Registry for MCP skills
    pub skill_registry: Arc<StdMutex<SkillRegistry>>,
    /// Telemetry runtime
    pub telemetry_runtime: Arc<StdMutex<TelemetryRuntime>>,
    /// PUA enforcement plan
    pub pua_enforcement_plan: Arc<StdMutex<crate::pua::PuaEnforcementPlan>>,
    /// Artifact ledger
    pub artifact_ledger: Arc<StdMutex<ArtifactLedger>>,
    /// Verbose logging flag
    pub verbose: bool,
    /// Output stream for responses
    pub output: Arc<Mutex<tokio::io::Stdout>>,
    /// Shutdown notification mechanism
    pub shutdown_notify: Arc<Notify>,
    /// In-memory registry for Responses API objects
    pub responses_api_store: Arc<StdMutex<HashMap<String, serde_json::Value>>>,
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
        self.response_cache.clone()
    }

    /// Get the vector store handle
    pub fn vector_store(&self) -> Option<Arc<VectorStore>> {
        self.vector_store.clone()
    }

    /// Get total requests count
    pub fn total_requests(&self) -> u64 {
        self.metrics.total_requests()
    }

    /// Get server status
    pub fn get_status(&self) -> crate::acp::prelude::ServerStatus {
        use crate::acp::prelude::{LifecycleSnapshot, MetricsSnapshot, ServerStatus};

        let mut total_requests = self.metrics.total_requests();
        let mut successful_requests = self.metrics.successful_requests();
        let mut failed_requests = self.metrics.failed_requests();
        let mut avg_request_duration_ms = self.metrics.avg_request_duration_ms();

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
            active_requests: self.metrics.active_requests(),
            cache_hit_rate: 0.0,
            circuit_breaker_open_count: self
                .circuit_breakers
                .lock()
                .map(|guard| guard.open_count())
                .unwrap_or(0),
            memory_usage_bytes: 0,
            cpu_usage_percent: 0.0,
            ..MetricsSnapshot::default()
        };
        let runtime_snapshot = self.metrics.snapshot();
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
            .lifecycle_state
            .lock()
            .map(|guard| guard.snapshot())
            .unwrap_or_else(|_| LifecycleSnapshot::default());

        let circuit_breakers = self
            .circuit_breakers
            .lock()
            .map(|guard| guard.snapshots())
            .unwrap_or_default();

        let maintenance = self
            .maintenance_tracker
            .lock()
            .map(|guard| guard.snapshot())
            .unwrap_or_default();

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
        self.lifecycle_state
            .lock()
            .map(|guard| guard.is_healthy())
            .unwrap_or(false)
    }

    /// Check if shutdown has been requested
    pub fn shutdown_requested(&self) -> bool {
        self.lifecycle_state
            .lock()
            .map(|guard| guard.shutdown_requested())
            .unwrap_or(false)
    }

    /// Begin shutdown process
    pub fn begin_shutdown(&self) {
        if let Ok(mut guard) = self.lifecycle_state.lock() {
            guard.begin_shutdown();
        }
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
        &self.metrics
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
        self.metrics.inc_successful_requests();
        self.metrics.total_requests()
    }

    pub fn register_skill(&self, skill: Arc<dyn crate::orchestration::skill::Skill>) {
        if let Ok(mut registry) = self.skill_registry.lock() {
            registry.register(skill);
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

    /// Build the server
    pub fn build(self) -> Result<AcpServer> {
        use crate::acp::prelude::{
            CircuitBreakerRegistry, ConversationState, InflightLimiter, LifecycleState,
            MaintenanceTracker, OnlineControllerState, RuntimeMetrics,
        };

        let metrics = Arc::new(RuntimeMetrics::default());
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
        let dynamic_parameter_tuner = Arc::new(StdMutex::new(DynamicParameterTuner::default()));
        let resource_allocator = Arc::new(StdMutex::new(ResourceAllocator {}));
        let cost_optimizer = Arc::new(StdMutex::new(CostOptimizer::new()));
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
            response_cache: self.response_cache,
            vector_store: self.vector_store,
            vector_config: None,
            autotune: None,
            autotune_config: None,
            autotune_state_path: None,
            runtime_config: RuntimeConfig::default(),
            config_path: self.config_path,
            metrics,
            online_controller,
            circuit_breakers,
            maintenance_tracker,
            inflight_limiter,
            lifecycle_state,
            conversation_state,
            phase_rate_limiter,
            review_timeout_policy,
            adaptive_model_selector,
            dynamic_parameter_tuner,
            resource_allocator,
            cost_optimizer,
            failure_prevention,
            flow_model_selector,
            memory_response_cache,
            memory_store,
            skill_registry,
            telemetry_runtime,
            pua_enforcement_plan,
            artifact_ledger,
            verbose: self.verbose,
            output: Arc::new(Mutex::new(tokio::io::stdout())),
            shutdown_notify: Arc::new(Notify::new()),
            responses_api_store,
        })
    }
}

impl Default for ServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}
