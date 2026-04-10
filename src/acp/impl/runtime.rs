//! Runtime implementation functions for ACP server
//!
//! This module contains standalone functions that implement the core runtime
//! functionality previously in the `impl AcpServer` block.
//! These functions take `AcpServer` as their first parameter to maintain
//! compatibility with the original implementation.

use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::Result;
use reqwest;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{Mutex, Notify};
use tracing::{error, info};

use crate::acp::background::start_background_tasks;
use crate::acp::r#impl::io::send_error;
use crate::acp::r#impl::request::handle_request;

use crate::acp::server::AcpServer;
use crate::adaptive_selector::AdaptiveModelSelector;
use crate::advanced_modules::{DynamicParameterTuner, ResourceAllocator};
use crate::agent::AgentRegistry;
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
use crate::rpc_protocol::JsonRpcRequest;
use crate::vector::VectorStore;

/// Create a new ACP server instance
///
/// This function replaces the `AcpServer::new` constructor.
#[allow(clippy::too_many_arguments)]
pub fn new_acp_server(
    flow: Arc<FlowManager>,
    registry: Arc<AgentRegistry>,
    cache: Option<Arc<crate::cache::ResponseCache>>,
    vector_store: Option<Arc<VectorStore>>,
    vector_config: Option<VectorConfig>,
    autotune: Option<Arc<tokio::sync::Mutex<AutoTuneState>>>,
    autotune_config: Option<AutoTuneConfig>,
    autotune_state_path: Option<String>,
    config_path: Option<String>,
    runtime_config: RuntimeConfig,
    _http_client: Option<reqwest::Client>,
    _verbose: bool,
) -> AcpServer {
    // Use ServerBuilder to create the server with correct field names and types
    use crate::acp::server::ServerBuilder;

    let mut builder = ServerBuilder::new();

    // Set the components that ServerBuilder supports
    builder = builder.with_flow_manager(flow.clone());
    builder = builder.with_agent_registry(registry.clone());

    if let Some(ref cache) = cache {
        builder = builder.with_response_cache(cache.clone());
    }

    if let Some(ref vector_store) = vector_store {
        builder = builder.with_vector_store(vector_store.clone());
    }
    if let Some(ref path) = config_path {
        builder = builder.with_artifact_ledger(ArtifactLedger::new(Some(Path::new(path))));
    }
    builder = builder.with_config_path(config_path.clone());

    // Note: ServerBuilder doesn't have methods for all parameters yet
    // For now, we'll build with defaults and let the caller set additional fields
    match builder.build() {
        Ok(mut server) => {
            // Set fields that aren't available in ServerBuilder yet
            server.vector_config = vector_config;
            server.autotune = autotune;
            server.autotune_config = autotune_config;
            server.autotune_state_path = autotune_state_path;
            server.config_path = config_path;
            server.runtime_config = runtime_config;
            server.verbose = _verbose;

            server
        }
        Err(err) => {
            // Fallback to creating a minimal server if builder fails
            tracing::error!("Failed to build server with ServerBuilder: {}", err);

            // Create a minimal server with just the essential components
            use crate::acp::prelude::{
                CircuitBreakerRegistry, ConversationState, InflightLimiter, LifecycleState,
                MaintenanceTracker, OnlineControllerState, PhaseRateLimiter, ReviewTimeoutPolicy,
                RuntimeMetrics,
            };

            let mut failure_prevention_state = FailurePrevention::new();
            for name in registry.names() {
                failure_prevention_state.register_service(&name);
            }

            AcpServer {
                flow_manager: Some(flow.clone()),
                agent_registry: Some(registry.clone()),
                response_cache: cache.clone(),
                vector_store: vector_store.clone(),
                vector_config,
                autotune,
                autotune_config,
                autotune_state_path,
                config_path: config_path.clone(),
                runtime_config: runtime_config.clone(),
                metrics: Arc::new(RuntimeMetrics::new()),
                online_controller: Arc::new(StdMutex::new(OnlineControllerState::default())),
                circuit_breakers: Arc::new(StdMutex::new(CircuitBreakerRegistry::new())),
                maintenance_tracker: Arc::new(StdMutex::new(MaintenanceTracker::new())),
                inflight_limiter: Arc::new(StdMutex::new(InflightLimiter::default())),
                lifecycle_state: Arc::new(StdMutex::new(LifecycleState::new())),
                conversation_state: Arc::new(Mutex::new(ConversationState::default())),
                phase_rate_limiter: Arc::new(StdMutex::new(PhaseRateLimiter::default())),
                review_timeout_policy: Arc::new(StdMutex::new(ReviewTimeoutPolicy {
                    timeout_seconds: None,
                    fail_on_timeout: false,
                })),
                adaptive_model_selector: Arc::new(StdMutex::new(AdaptiveModelSelector::new())),
                dynamic_parameter_tuner: Arc::new(StdMutex::new(DynamicParameterTuner::default())),
                resource_allocator: Arc::new(StdMutex::new(ResourceAllocator {})),
                cost_optimizer: Arc::new(StdMutex::new(CostOptimizer::new())),
                failure_prevention: Arc::new(StdMutex::new(failure_prevention_state)),
                flow_model_selector: Arc::new(StdMutex::new(FlowModelSelector {})),
                memory_response_cache: Arc::new(StdMutex::new(MemoryResponseCache::default())),
                memory_store: Arc::new(StdMutex::new(MemoryStore::new(MemoryPolicy::default()))),
                skill_registry: Arc::new(StdMutex::new(SkillRegistry::default())),
                telemetry_runtime: Arc::new(StdMutex::new(TelemetryRuntime::new(&runtime_config))),
                pua_enforcement_plan: Arc::new(StdMutex::new(crate::pua::PuaEnforcementPlan {
                    escalation_level: String::new(),
                    mandatory_roles: Vec::new(),
                    red_lines: Vec::new(),
                    quality_compass: Vec::new(),
                    mandatory_safeguards: Vec::new(),
                    mandatory_evidence: Vec::new(),
                    stage_requirements: Vec::new(),
                })),
                artifact_ledger: Arc::new(StdMutex::new(ArtifactLedger::new(
                    config_path.as_deref().map(Path::new),
                ))),
                verbose: _verbose,
                output: Arc::new(Mutex::new(tokio::io::stdout())),
                shutdown_notify: Arc::new(Notify::new()),
            }
        }
    }
}

/// Run the ACP server
///
/// This function replaces the `AcpServer::run` method.
pub async fn run_acp_server(server: &mut AcpServer) -> Result<()> {
    info!("ACP server starting");

    let shutdown_notify = Arc::clone(&server.shutdown_notify);

    // Start background tasks
    if let Err(e) = start_background_tasks(server, Arc::clone(&shutdown_notify)).await {
        error!("Failed to start background tasks: {}", e);
        return Err(e);
    }

    info!("ACP server running");

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();

    loop {
        if server.shutdown_requested() {
            break;
        }

        let next_line = tokio::select! {
            _ = shutdown_notify.notified() => {
                break;
            }
            line = lines.next_line() => line?,
        };

        let Some(line) = next_line else {
            break;
        };

        if server.shutdown_requested() {
            break;
        }

        if line.trim().is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(request) => request,
            Err(err) => {
                send_error(server, None, -32700, format!("parse error: {err}"), None).await?;
                continue;
            }
        };

        if request.jsonrpc != "2.0" {
            send_error(
                server,
                request.id,
                -32600,
                "jsonrpc must be 2.0".to_string(),
                None,
            )
            .await?;
            continue;
        }

        if let Err(err) = handle_request(server, request).await {
            error!("request failed: {err:#}");
        }
    }

    // Notify background tasks to shutdown
    server.begin_shutdown();
    shutdown_notify.notify_waiters();

    info!("ACP server shutting down");
    Ok(())
}

/// Get routing handles (flow manager and agent registry)
pub fn routing_handles(server: &AcpServer) -> Result<(Arc<FlowManager>, Arc<AgentRegistry>)> {
    let flow = server
        .flow_manager
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("flow manager not initialized"))?;
    let registry = server
        .agent_registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("agent registry not initialized"))?;
    Ok((Arc::clone(flow), Arc::clone(registry)))
}

/// Get cache handle
pub fn cache_handle(server: &AcpServer) -> Option<Arc<crate::cache::ResponseCache>> {
    server.response_cache.clone()
}

/// Get artifact ledger
pub fn artifact_ledger(_server: &AcpServer) -> crate::reinforcement::ArtifactLedger {
    _server
        .artifact_ledger
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| {
            crate::reinforcement::ArtifactLedger::new(
                _server.config_path.as_deref().map(std::path::Path::new),
            )
        })
}

/// Get vector store handle
pub fn vector_store_handle(server: &AcpServer) -> Option<Arc<VectorStore>> {
    server.vector_store.clone()
}

/// Get vector configuration snapshot
pub fn vector_config_snapshot(server: &AcpServer) -> Option<VectorConfig> {
    server.vector_config.clone()
}

/// Get autotune handle
pub fn autotune_handle(server: &AcpServer) -> Option<Arc<tokio::sync::Mutex<AutoTuneState>>> {
    server.autotune.clone()
}
