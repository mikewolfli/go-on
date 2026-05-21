use std::mem;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex as StdMutex};

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::signal;
use tokio::sync::{Mutex, Notify};
use tracing::{debug, error, info, warn};

use crate::acp::background::start_background_tasks;
use crate::acp::r#impl::cors::{
    build_cors_headers, build_preflight_response_headers, is_origin_allowed,
};
use crate::acp::r#impl::io::send_error;
use crate::acp::r#impl::request::{handle_request, inject_platform_profiles_if_absent};
use crate::i18n::runtime::{t, tf};

use crate::acp::server::{AcpServer, CacheLayer, ObservabilityLayer};
use crate::adaptive_selector::AdaptiveModelSelector;
use crate::agent::AgentRegistry;
use crate::config::{AutoTuneConfig, AutoTuneState, RuntimeConfig, VectorConfig};
use crate::failure_prevention::FailurePrevention;
use crate::flow::FlowManager;
use crate::flow_with_models::FlowModelSelector;
use crate::governance::hardening::{rbac_fallback_allows_action, GovernanceAction};
use crate::governance::rbac::{AccessDecision, Permission, Principal};
use crate::memory_module::{MemoryPolicy, MemoryStore};
use crate::memory_response_cache::MemoryResponseCache;
use crate::observability::telemetry::TelemetryRuntime;
use crate::orchestration::skill::SkillRegistry;
use crate::reinforcement::ArtifactLedger;
use crate::rpc_protocol::{chat_trace_context, JsonRpcRequest, RequestTraceContext};
use crate::shared::secret_override::get_secret;
use crate::vector::VectorStore;

/// Serializes concurrent `/rpc` calls to prevent pipe-swapping race conditions.
/// `server.output` is a global singleton — without this guard, two concurrent
/// `/rpc` requests would corrupt each other's response capture pipes.
pub(crate) static RPC_SERIAL: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

pub(crate) static RESPONSES_ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn next_responses_api_id(prefix: &str) -> String {
    let seq = RESPONSES_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}_{}_{}", prefix, crate::acp::prelude::now_ts_ms(), seq)
}

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
    app_config: Option<Arc<crate::config::AppConfig>>,
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
    // For now, we'll build with defaults and let the caller set additional fields.
    // The builder already initializes ForkRegistry, Planner, Executor, BenchmarkSuite,
    // SchemaRegistry, TenantBudgetEnforcer, OptimizerRegistry, PromptAssembler, and
    // PromotionRegistry with sensible defaults.
    // Create HarnessBus and CapabilityBus to wire into the server
    let mut harness_bus = {
        let config_path_ref = config_path.as_deref().map(Path::new);
        let storage_path = config_path_ref
            .and_then(|p| p.parent())
            .map(|p| p.join("governance"));
        if let Some(ref cfg) = app_config {
            Arc::new(crate::governance::harness_bus::config_aware_harness_bus(
                cfg.as_ref(),
                storage_path,
            ))
        } else {
            Arc::new(crate::governance::harness_bus::default_harness_bus(
                storage_path,
            ))
        }
    };
    // Inject RBAC enforcer into the harness bus and create HTTP-level enforcer
    use crate::governance::rbac::{Permission, RbacEnforcer};
    let rbac_enforcer: Arc<std::sync::RwLock<RbacEnforcer>> = {
        let mut enforcer = RbacEnforcer::new();
        enforcer.register_role(
            "admin",
            vec![
                Permission::Read,
                Permission::Write,
                Permission::Execute,
                Permission::Admin,
                Permission::ManageUsers,
                Permission::ManageConfig,
                Permission::Audit,
            ],
        );
        enforcer.register_role(
            "user",
            vec![Permission::Read, Permission::Write, Permission::Execute],
        );
        enforcer.register_role("viewer", vec![Permission::Read]);
        // Clone the enforcer for harness bus injection
        let bus_enforcer = enforcer.clone();
        // The Arc has strong count 1 at this point, so get_mut succeeds
        if let Some(bus) = Arc::get_mut(&mut harness_bus) {
            bus.set_rbac_enforcer(bus_enforcer);
        }
        Arc::new(std::sync::RwLock::new(enforcer))
    };

    let workflow_registry = Arc::new(std::sync::Mutex::new(
        crate::orchestration::workflow_registry::WorkflowRegistry::new(),
    ));
    // Create a shared provenance ledger
    let provenance_ledger = Arc::new(crate::observability::provenance::ProvenanceLedger::default());
    let capability_bus = Arc::new(
        crate::intelligence::capability_bus::core::CapabilityBus::new_default(
            Arc::clone(&harness_bus),
            Some(workflow_registry),
        )
        .with_provenance_ledger(Arc::clone(&provenance_ledger)),
    );

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
            server.harness_bus = Some(harness_bus);
            server.capability_bus = Some(capability_bus);
            server.provenance_ledger = Some(provenance_ledger);
            server.rbac_enforcer = Some(rbac_enforcer);

            // Create session manager if user auth is enabled
            if server.runtime_config.user_auth_enabled {
                use crate::acp::r#impl::session::{AuthConfig, SessionManager};
                let auth_config = AuthConfig::from(&server.runtime_config);
                server.session_manager =
                    Some(Arc::new(SessionManager::with_auth_config(auth_config)));
            }

            // Wire dual-level task scheduler (ARCH-02): create the scheduler and
            // register one worker per known agent so the priority queue has real routing
            // targets.  The scheduler tracks queue depth and active-worker counts that
            // are surfaced in governance.status.
            let task_scheduler = {
                let config = crate::orchestration::scheduler::SchedulerConfig::default();
                let s = Arc::new(crate::orchestration::scheduler::AgentWorkerScheduler::new(
                    config,
                ));
                for agent_name in registry.names() {
                    if let Err(e) = s.register_worker(&agent_name, &agent_name) {
                        warn!(
                            "failed to register worker for agent '{}': {}",
                            agent_name, e
                        );
                    }
                }
                s
            };
            server.scheduler = Some(task_scheduler);

            if server.runtime_config.skills_enabled {
                server.register_skill(Arc::new(crate::orchestration::skill::EchoSkill));
                server.register_skill(Arc::new(
                    crate::orchestration::skill::SkillCreatorSkill::new(
                        server.skill_registry.clone(),
                    ),
                ));
            }

            // Wire the new modules' state from CapabilityBus into the server's
            // standalone fields so process_chat_request can access them directly.
            server.schema_registry = Arc::clone(
                &server
                    .capability_bus
                    .as_ref()
                    .map(|cb| Arc::clone(&cb.schema_registry))
                    .unwrap_or_default(),
            );
            server.tenant_budget = Arc::clone(
                &server
                    .capability_bus
                    .as_ref()
                    .map(|cb| Arc::clone(&cb.tenant_budget))
                    .unwrap_or_default(),
            );

            // Auto-provision a default tenant quota when user auth is enabled so
            // the budget enforcer does not reject every request with "no quota
            // configured for tenant 'default-tenant'" (F-GAP-08).
            if server.runtime_config.user_auth_enabled {
                if let Ok(mut budget) = server.tenant_budget.lock() {
                    budget.auto_provision_default(&server.runtime_config);
                }
            }

            server.optimizer_registry = Arc::clone(
                &server
                    .capability_bus
                    .as_ref()
                    .map(|cb| Arc::clone(&cb.optimizer_registry))
                    .unwrap_or_default(),
            );

            // Wire the token cache into the agent registry so that all
            // agents returned by registry.get() are automatically wrapped
            // with CachedAgentWrapper.
            registry.set_token_cache(Some(Arc::clone(&server.cache.token_cache)));

            server
        }
        Err(err) => {
            // Fallback to creating a minimal server if builder fails
            tracing::error!("Failed to build server with ServerBuilder: {}", err);

            // Create a minimal server with just the essential components
            use crate::acp::prelude::{
                AcpLockMonitor, CircuitBreakerRegistry, ConversationState, InflightLimiter,
                LifecycleState, MaintenanceTracker, OnlineControllerState, PhaseRateLimiter,
                ReviewTimeoutPolicy, RuntimeMetrics,
            };

            let mut failure_prevention_state = FailurePrevention::new();
            for name in registry.names() {
                failure_prevention_state.register_service(&name);
            }

            let mut fallback_server = AcpServer {
                flow_manager: Some(flow.clone()),
                agent_registry: Some(registry.clone()),
                cache: CacheLayer {
                    response_cache: cache.clone(),
                    memory_response_cache: Arc::new(StdMutex::new(MemoryResponseCache::default())),
                    vector_store: vector_store.clone(),
                    token_cache: Arc::new(
                        crate::intelligence::token_cache::TokenMultiLevelCache::new(
                            500,
                            200,
                            ".goon/token_cache",
                        ),
                    ),
                },
                vector_config,
                autotune,
                autotune_config,
                autotune_state_path,
                config_path: config_path.clone(),
                runtime_config: runtime_config.clone(),
                observability: ObservabilityLayer {
                    metrics: Arc::new(RuntimeMetrics::new()),
                    lock_monitor: Arc::new(AcpLockMonitor::default()),
                    telemetry_runtime: Arc::new(StdMutex::new(TelemetryRuntime::new(
                        &runtime_config,
                    ))),
                },
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
                failure_prevention: Arc::new(StdMutex::new(failure_prevention_state)),
                flow_model_selector: Arc::new(StdMutex::new(FlowModelSelector {})),
                memory_store: Arc::new(StdMutex::new(MemoryStore::new(MemoryPolicy::default()))),
                skill_registry: {
                    let mut registry = SkillRegistry::default();
                    let prompt_skills_path =
                        std::path::PathBuf::from(&runtime_config.skills_cache_dir)
                            .join("prompt_skills.json");
                    registry.set_persistence_path(prompt_skills_path);
                    if let Err(e) = registry.load_prompt_skills_from_disk() {
                        tracing::warn!("Failed to load prompt skills from disk: {}", e);
                    }
                    Arc::new(StdMutex::new(registry))
                },
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
                harness_bus: Some(Arc::clone(&harness_bus)),
                capability_bus: Some(Arc::clone(&capability_bus)),
                provenance_ledger: Some(provenance_ledger),
                fork_registry: Arc::new(StdMutex::new(
                    crate::orchestration::fork_registry::ForkRegistry::new(
                        crate::orchestration::fork_registry::ForkConfig::default(),
                    ),
                )),
                planner: crate::orchestration::planner_executor::Planner,
                executor: crate::orchestration::planner_executor::Executor,
                evaluation_suite: Arc::new(StdMutex::new(
                    crate::intelligence::evaluation::BenchmarkSuite::new(),
                )),
                schema_registry: Arc::new(StdMutex::new(
                    crate::orchestration::task_schema::SchemaRegistry::new(),
                )),
                tenant_budget: Arc::new(StdMutex::new(
                    crate::governance::hardening::TenantBudgetEnforcer::new(),
                )),
                optimizer_registry: Arc::new(StdMutex::new(
                    crate::orchestration::workflow_optimizer::OptimizerRegistry::new(),
                )),
                prompt_assembler: crate::orchestration::prompt_layers::PromptAssembler,
                promotion_registry: Arc::new(StdMutex::new(
                    crate::orchestration::promotion_plugin::PromotionRegistry::new(),
                )),
                output: Arc::new(Mutex::new(
                    Box::new(tokio::io::stdout()) as Box<dyn tokio::io::AsyncWrite + Send + Unpin>
                )),
                shutdown_notify: Arc::new(Notify::new()),
                responses_api_store: Arc::new(StdMutex::new(std::collections::HashMap::new())),
                task_graph_store: None,
                scheduler: None,
                prompt_manager: crate::acp::r#impl::request::prompts_pack::PromptManager::new(
                    std::path::PathBuf::from("./prompts"),
                ),
                session_manager: None,
                rbac_enforcer: None,
            };

            // Create session manager if user auth is enabled
            if fallback_server.runtime_config.user_auth_enabled {
                use crate::acp::r#impl::session::{AuthConfig, SessionManager};
                let auth_config = AuthConfig::from(&fallback_server.runtime_config);
                fallback_server.session_manager =
                    Some(Arc::new(SessionManager::with_auth_config(auth_config)));
            }

            // Wire RBAC enforcer into the fallback server for HTTP-level authorization
            fallback_server.rbac_enforcer = Some(rbac_enforcer);

            // Wire dual-level task scheduler (ARCH-02): create the scheduler and
            // register one worker per known agent so the priority queue has real routing
            // targets.  The scheduler tracks queue depth and active-worker counts that
            // are surfaced in governance.status.
            let task_scheduler = {
                let config = crate::orchestration::scheduler::SchedulerConfig::default();
                let s = Arc::new(crate::orchestration::scheduler::AgentWorkerScheduler::new(
                    config,
                ));
                for agent_name in registry.names() {
                    if let Err(e) = s.register_worker(&agent_name, &agent_name) {
                        warn!(
                            "failed to register worker for agent '{}': {}",
                            agent_name, e
                        );
                    }
                }
                s
            };
            fallback_server.scheduler = Some(task_scheduler);

            if fallback_server.runtime_config.skills_enabled {
                fallback_server.register_skill(Arc::new(crate::orchestration::skill::EchoSkill));
                fallback_server.register_skill(Arc::new(
                    crate::orchestration::skill::SkillCreatorSkill::new(
                        fallback_server.skill_registry.clone(),
                    ),
                ));
            }

            // Wire the new modules' state from CapabilityBus into the fallback server's
            // standalone fields so process_chat_request can access them directly.
            fallback_server.schema_registry = Arc::clone(
                &fallback_server
                    .capability_bus
                    .as_ref()
                    .map(|cb| Arc::clone(&cb.schema_registry))
                    .unwrap_or_default(),
            );
            fallback_server.tenant_budget = Arc::clone(
                &fallback_server
                    .capability_bus
                    .as_ref()
                    .map(|cb| Arc::clone(&cb.tenant_budget))
                    .unwrap_or_default(),
            );

            // Auto-provision a default tenant quota when user auth is enabled so
            // the budget enforcer does not reject every request with "no quota
            // configured for tenant 'default-tenant'" (F-GAP-08).
            if fallback_server.runtime_config.user_auth_enabled {
                if let Ok(mut budget) = fallback_server.tenant_budget.lock() {
                    budget.auto_provision_default(&fallback_server.runtime_config);
                }
            }

            fallback_server.optimizer_registry = Arc::clone(
                &fallback_server
                    .capability_bus
                    .as_ref()
                    .map(|cb| Arc::clone(&cb.optimizer_registry))
                    .unwrap_or_default(),
            );

            // Wire the token cache into the agent registry for the fallback path too.
            registry.set_token_cache(Some(Arc::clone(&fallback_server.cache.token_cache)));

            fallback_server
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

    // Set up signal watchers for graceful shutdown
    let mut sigterm = std::pin::pin!(async {
        #[cfg(unix)]
        {
            match signal::unix::signal(signal::unix::SignalKind::terminate()) {
                Ok(mut stream) => {
                    stream.recv().await;
                }
                Err(e) => {
                    warn!("failed to register SIGTERM handler: {e}; graceful shutdown via SIGTERM disabled");
                    std::future::pending::<()>().await;
                }
            }
        }
        #[cfg(not(unix))]
        std::future::pending::<()>().await;
    });

    loop {
        if server.shutdown_requested() {
            break;
        }

        let next_line = tokio::select! {
            _ = shutdown_notify.notified() => {
                break;
            }
            _ = signal::ctrl_c() => {
                info!("Received SIGINT (Ctrl+C), initiating graceful shutdown...");
                break;
            }
            _ = sigterm.as_mut() => {
                info!("Received SIGTERM, initiating graceful shutdown...");
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
                send_error(
                    server,
                    None,
                    -32700,
                    tf("error.parse_error", &[("error", &err.to_string())]),
                    None,
                )
                .await?;
                continue;
            }
        };

        if request.jsonrpc != "2.0" {
            send_error(
                server,
                request.id,
                -32600,
                t("error.jsonrpc_must_be_2_0").to_string(),
                None,
            )
            .await?;
            continue;
        }

        if let Err(err) = handle_request(server, request).await {
            error!("request failed: {err:#}");
        }
    }

    // ── Graceful shutdown ──────────────────────────────────────────
    server.begin_shutdown();

    // Notify background tasks to shut down.  No drain for stdio — the
    // server runs until stdin EOF / SIGINT / SIGTERM, so there are no
    // in-flight network connections to drain.
    shutdown_notify.notify_waiters();
    info!("ACP server shutting down");
    Ok(())
}

pub async fn run_acp_http_server(server: Arc<AcpServer>, bind_addr: String) -> Result<()> {
    info!("ACP HTTP server starting on {}", bind_addr);

    let shutdown_notify = Arc::clone(&server.shutdown_notify);

    if let Err(err) = start_background_tasks(server.as_ref(), Arc::clone(&shutdown_notify)).await {
        error!("Failed to start background tasks: {}", err);
        return Err(err);
    }

    // Create socket with SO_REUSEADDR to avoid "Address already in use" after restart
    let listener = match bind_addr.parse::<SocketAddr>() {
        Ok(addr) => {
            let s = match addr {
                SocketAddr::V4(_) => TcpSocket::new_v4()?,
                SocketAddr::V6(_) => TcpSocket::new_v6()?,
            };
            s.set_reuseaddr(true)?;
            s.bind(addr)?;
            s.listen(1024)?
        }
        Err(_) => TcpListener::bind(&bind_addr).await?,
    };

    // Set up signal watchers for graceful shutdown
    let mut sigterm = std::pin::pin!(async {
        #[cfg(unix)]
        {
            match signal::unix::signal(signal::unix::SignalKind::terminate()) {
                Ok(mut stream) => {
                    stream.recv().await;
                }
                Err(e) => {
                    warn!("failed to register SIGTERM handler: {e}; graceful shutdown via SIGTERM disabled");
                    std::future::pending::<()>().await;
                }
            }
        }
        #[cfg(not(unix))]
        std::future::pending::<()>().await;
    });

    loop {
        tokio::select! {
            _ = shutdown_notify.notified() => {
                break;
            }
            _ = signal::ctrl_c() => {
                info!("Received SIGINT (Ctrl+C), initiating graceful shutdown...");
                break;
            }
            _ = sigterm.as_mut() => {
                info!("Received SIGTERM, initiating graceful shutdown...");
                break;
            }
            incoming = listener.accept() => {
                let (mut socket, peer_addr) = incoming?;
                let server_ref = Arc::clone(&server);
                tokio::spawn(async move {
                    if let Err(err) = handle_http_connection(&mut socket, server_ref, peer_addr).await {
                        warn!("ACP HTTP connection {} failed: {}", peer_addr, err);
                    }
                });
            }
        }
    }

    // ── Graceful shutdown with drain ────────────────────────────────
    server.begin_shutdown();

    // Wait for in-flight requests to complete (shutdown drain).
    let drain_secs = server.runtime_config.shutdown_drain_seconds.max(1);
    info!(
        "ACP HTTP server draining connections for {} seconds...",
        drain_secs
    );
    tokio::time::sleep(std::time::Duration::from_secs(drain_secs)).await;

    server.shutdown_notify.notify_waiters();
    info!("ACP HTTP server shutting down");
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
#[allow(dead_code)] // F-GAP-09 — planned wiring: memory/caching accessor
#[must_use]
pub fn cache_handle(server: &AcpServer) -> Option<Arc<crate::cache::ResponseCache>> {
    server.cache.response_cache.clone()
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
#[allow(dead_code)] // F-GAP-08 — planned wiring: learning/intelligence accessor
#[must_use]
pub fn vector_store_handle(server: &AcpServer) -> Option<Arc<VectorStore>> {
    server.cache.vector_store.clone()
}

/// Get vector configuration snapshot
#[allow(dead_code)] // F-GAP-08 — planned wiring: learning/intelligence accessor
pub fn vector_config_snapshot(server: &AcpServer) -> Option<VectorConfig> {
    server.vector_config.clone()
}

/// Get autotune handle
#[allow(dead_code)] // F-GAP-08 — planned wiring: learning/intelligence accessor
#[must_use]
pub fn autotune_handle(server: &AcpServer) -> Option<Arc<tokio::sync::Mutex<AutoTuneState>>> {
    server.autotune.clone()
}

// ── HTTP request parsing ────────────────────────────────────────────

/// Parse the raw HTTP request text into method, path, header_part, body_initial_part,
/// and adaptive_signal.
struct ParsedHttpRequest<'a> {
    method: &'a str,
    path: &'a str,
    header_part: &'a str,
    body_initial_part: &'a str,
    #[allow(dead_code)] // F-GAP-05 — reserved for planner/executor adaptive signal
    adaptive_signal: &'static str,
}

fn parse_http_request(request_text: &str) -> Result<ParsedHttpRequest<'_>> {
    let header_end = request_text
        .find("\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("invalid HTTP request: missing header terminator"))?;

    let (header_part, body_initial_part) = request_text.split_at(header_end + 4);
    let request_line = header_part
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid HTTP request: missing request line"))?;

    let mut request_line_parts = request_line.split_whitespace();
    let method = request_line_parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid HTTP request: missing method"))?;
    let path = request_line_parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid HTTP request: missing path"))?;
    let adaptive_signal = infer_adaptive_signal(method, path, header_part);

    Ok(ParsedHttpRequest {
        method,
        path,
        header_part,
        body_initial_part,
        adaptive_signal,
    })
}

// ── HTTP entry guards ───────────────────────────────────────────────

/// Apply entry guards and return `true` if the request was rejected (response already written).
async fn http_entry_guard(
    socket: &mut TcpStream,
    server: &AcpServer,
    header_part: &str,
    method: &str,
    path: &str,
    peer_addr: SocketAddr,
    cors_headers: &str,
) -> Result<bool> {
    apply_entry_guards(
        socket,
        server,
        header_part,
        method,
        path,
        peer_addr,
        cors_headers,
    )
    .await
}

// ── HTTP GET routing ────────────────────────────────────────────────

/// Route an HTTP GET request based on the path and write the response back to the socket.
async fn route_http_get(
    socket: &mut TcpStream,
    server: &AcpServer,
    path: &str,
    cors_headers: &str,
) -> Result<()> {
    match path {
        "/health" => {
            write_http_json_response_with_context(
                socket,
                200,
                serde_json::to_value(server.get_status())?,
                "health",
                cors_headers,
            )
            .await?;
        }
        "/v1/responses" => {
            let data = list_responses_api_payloads(server);
            write_http_json_response_with_context(
                socket,
                200,
                serde_json::json!({
                    "object": "list",
                    "data": data,
                }),
                "responses.api",
                cors_headers,
            )
            .await?;
        }
        "/v1/models" | "/v1/model" | "/models" => {
            write_http_json_response_with_context(
                socket,
                200,
                build_openai_models_response(),
                "openai.chat.completions",
                cors_headers,
            )
            .await?;
        }
        "/" => {
            write_http_json_response_with_context(
                socket,
                200,
                build_root_capabilities_response(),
                "initialize",
                cors_headers,
            )
            .await?;
        }
        _ if extract_response_id_from_path(path).is_some() => {
            let response_id = extract_response_id_from_path(path).expect("guard ensured Some; qed");
            handle_response_get(socket, server, response_id, cors_headers).await?;
        }
        _ => {
            write_http_json_response_with_context(
                socket,
                404,
                serde_json::json!({"error": t("error.not_found")}),
                "chat",
                cors_headers,
            )
            .await?;
        }
    }
    Ok(())
}

// ── HTTP POST routing ───────────────────────────────────────────────

/// Route a POST request — reads body, dispatches to the appropriate handler,
/// and writes the response to the socket. Returns the path label for logging.
///
/// `body_initial_part` is the portion of the body already in the initial buffer read.
#[allow(clippy::question_mark)]
// Intentional — early return for the !path check and JSON parse error below,
// where we write an error response to the socket before returning Ok(path).
// Using `?` would propagate the error upward without writing the response.
async fn route_http_post(
    socket: &mut TcpStream,
    server: Arc<AcpServer>,
    path: &str,
    header_part: &str,
    body_initial_part: &str,
    user_session: Option<crate::acp::r#impl::session::UserSession>,
    cors_headers: &str,
) -> Result<String> {
    let responses_path = path == "/v1/responses";
    let content_length = extract_content_length(header_part).unwrap_or(0);
    if content_length == 0 {
        if responses_path {
            write_http_json_response_with_context(
                socket,
                400,
                build_responses_error(
                    "missing_required_field",
                    "invalid_request_error",
                    t("error.body_required"),
                ),
                "responses.api",
                cors_headers,
            )
            .await?;
        } else {
            write_http_json_response_with_context(
                socket,
                400,
                serde_json::json!({"error": t("error.body_required")}),
                "chat",
                cors_headers,
            )
            .await?;
        }
        return Ok(path.to_string());
    }

    let mut body_bytes = body_initial_part.as_bytes().to_vec();
    if body_bytes.len() < content_length {
        let mut remaining = vec![0u8; content_length - body_bytes.len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            socket.read_exact(&mut remaining),
        )
        .await
        .map_err(|_| anyhow::anyhow!("timeout reading HTTP body"))?
        .map_err(|e| anyhow::anyhow!("HTTP body read error: {e}"))?;
        body_bytes.extend_from_slice(&remaining);
    }
    body_bytes.truncate(content_length);

    // Enforce max body size (10MB)
    const MAX_BODY_SIZE: usize = 10 * 1024 * 1024;
    if body_bytes.len() > MAX_BODY_SIZE {
        anyhow::bail!(
            "HTTP body too large: {} bytes (max {})",
            body_bytes.len(),
            MAX_BODY_SIZE
        );
    }

    let body: serde_json::Value = match serde_json::from_slice(&body_bytes) {
        Ok(value) => value,
        Err(err) => {
            if responses_path {
                write_http_json_response_with_context(
                    socket,
                    400,
                    build_responses_error(
                        "invalid_request_error",
                        "invalid_request_error",
                        tf("error.invalid_json", &[("error", &err.to_string())]),
                    ),
                    "responses.api",
                    cors_headers,
                )
                .await?;
            } else {
                write_http_json_response_with_context(
                    socket,
                    400,
                    serde_json::json!({"error": tf("error.invalid_json", &[("error", &err.to_string())])}),
                    "chat",
                    cors_headers,
                )
                .await?;
            }
            return Ok(path.to_string());
        }
    };

    let (dispatch_result, duration) =
        crate::observability::performance::utils::measure_time_async(move || async move {
            match path {
                "/chat" => {
                    let params: crate::acp::r#impl::chat::ChatParams =
                        match serde_json::from_value(body) {
                            Ok(value) => value,
                            Err(err) => {
                                write_http_json_response_with_context(
                                socket,
                                400,
                                serde_json::json!({"error": tf("error.invalid_chat_params", &[("error", &err.to_string())])}),
                                "chat",
                                cors_headers,
                            )
                            .await?;
                                return Ok(());
                            }
                        };
                    let trace = http_trace_context("chat");
                    let ctx = Some(crate::acp::r#impl::chat::ChatRequestContext::new(
                        user_session,
                    ));
                    let result = crate::acp::r#impl::chat::process_chat_request(
                        server.as_ref(),
                        &params,
                        None,
                        &trace,
                        None,
                        ctx,
                    )
                    .await?;
                    let result = inject_platform_profiles_if_absent(result, "chat");
                    write_http_json_response(socket, 200, result, cors_headers).await?;
                }
                "/chat/stream" => {
                    let params: crate::acp::r#impl::chat::ChatParams =
                        match serde_json::from_value(body) {
                            Ok(value) => value,
                            Err(err) => {
                                write_http_json_response_with_context(
                                socket,
                                400,
                                serde_json::json!({"error": tf("error.invalid_chat_params", &[("error", &err.to_string())])}),
                                "chat",
                                cors_headers,
                            )
                            .await?;
                                return Ok(());
                            }
                        };
                    write_sse_headers(socket, cors_headers).await?;

                    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
                    let trace = http_trace_context("chat.stream");
                    let ctx = Some(crate::acp::r#impl::chat::ChatRequestContext::new(
                        user_session,
                    ));
                    let server_ref = Arc::clone(&server);
                    let task = tokio::spawn(async move {
                        crate::acp::r#impl::chat::process_chat_request(
                            server_ref.as_ref(),
                            &params,
                            Some(crate::acp::r#impl::chat::StreamObserver::sse(tx)),
                            &trace,
                            None,
                            ctx,
                        )
                        .await
                    });

                    while let Some(frame) = rx.recv().await {
                        if let Err(err) = write_sse_event(socket, &frame.event, &frame.payload).await {
                            // Client disconnected while backend task is still active.
                            // Abort task to avoid orphan compute and channel buildup.
                            task.abort();
                            return Err(err);
                        }
                    }

                    match task.await {
                        Ok(Ok(result)) => {
                            let result = inject_platform_profiles_if_absent(result, "chat");
                            write_sse_event(socket, "result", &result).await?
                        }
                        Ok(Err(err)) => {
                            let payload = inject_platform_profiles_if_absent(
                                serde_json::json!({"message": err.to_string()}),
                                "chat",
                            );
                            write_sse_event(
                                socket,
                                "error",
                                &payload,
                            )
                            .await?
                        }
                        Err(err) => {
                            let payload = inject_platform_profiles_if_absent(
                                serde_json::json!({"message": format!("chat task panicked: {err}")}),
                                "chat",
                            );
                            write_sse_event(socket, "error", &payload).await?
                        }
                    }
                }
                "/chat/completions" | "/v1/chat/completions" | "/chat/chat/completions" => {
                    handle_openai_chat_completions(
                        socket,
                        Arc::clone(&server),
                        body,
                        user_session,
                        cors_headers,
                    )
                    .await?;
                }
                "/" | "/rpc" => {
                    // SERIALIZED: Only one RPC call at a time.
                    // server.output is a global singleton used for pipe-based response
                    // capture. Without this lock, concurrent RPC calls would corrupt
                    // the pipe assignment (swap-in → dispatch → swap-out is not atomic).
                    let _rpc_guard = RPC_SERIAL.lock().await;

                    let request: JsonRpcRequest = match serde_json::from_value(body) {
                        Ok(r) => r,
                        Err(e) => {
                            write_http_json_response_with_context(
                                socket,
                                400,
                                serde_json::json!({"error": format!("invalid RPC request: {}", e)}),
                                path,
                                cors_headers,
                            )
                            .await?;
                            return Ok(());
                        }
                    };

                    // Create a pipe to capture the JSON-RPC response written to server.output
                    // Buffer must be large enough to hold all notifications + final response.
                    // AI responses with tool results can exceed 64KB, so use 10MB.
                    let (pipe_writer, mut pipe_reader) = tokio::io::duplex(10 * 1024 * 1024);

                    // Temporarily swap stdout with the pipe writer
                    {
                        let mut guard = server.output.lock().await;
                        let _ = mem::replace(&mut *guard, Box::new(pipe_writer));
                    }

                    // Dispatch the RPC request — response goes into the pipe
                    if let Err(err) = handle_request(server.as_ref(), request).await {
                        // Restore stdout before erroring out
                        {
                            let mut guard = server.output.lock().await;
                            let _ = mem::replace(
                                &mut *guard,
                                Box::new(tokio::io::stdout()) as Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
                            );
                        }
                        write_http_json_response_with_context(
                            socket,
                            500,
                            serde_json::json!({"error": format!("RPC dispatch error: {}", err)}),
                            path,
                            cors_headers,
                        )
                        .await?;
                        return Ok(());
                    }

                    // Restore stdout
                    {
                        let mut guard = server.output.lock().await;
                        let _ = mem::replace(
                            &mut *guard,
                            Box::new(tokio::io::stdout()) as Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
                        );
                    }

                    // Read the captured RPC response from the pipe.
                    // The pipe may contain multiple JSON-RPC messages
                    // (notifications such as chat.stream.chunk + final response).
                    // Parse line by line and find the last line that is a
                    // valid JSON-RPC response (has "id" field).
                    let mut response_bytes = Vec::new();
                    tokio::time::timeout(
                        std::time::Duration::from_secs(60),
                        pipe_reader.read_to_end(&mut response_bytes),
                    ).await
                    .map_err(|_| anyhow::anyhow!("timeout reading RPC pipe response"))?
                    .map_err(|e| anyhow::anyhow!("RPC pipe read error: {e}"))?;

                    let response_str = String::from_utf8_lossy(&response_bytes);
                    // Parse each line; find the last JSON value that has an "id" field
                    // (i.e. a JSON-RPC response, not a notification).
                    let response_value: serde_json::Value = {
                        let mut last_response =
                            serde_json::json!({"raw": response_str.to_string()});
                        for line in response_str.lines() {
                            let trimmed = line.trim();
                            if trimmed.is_empty() {
                                continue;
                            }
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
                                if val.get("id").is_some() {
                                    last_response = val;
                                }
                            }
                        }
                        last_response
                    };

                    write_http_json_response(socket, 200, response_value, cors_headers).await?;
                }
                "/v1/responses" => {
                    handle_responses_api(
                        socket,
                        Arc::clone(&server),
                        body,
                        user_session,
                        cors_headers,
                    )
                    .await?;
                }
                _ => {
                    write_http_json_response_with_context(
                        socket,
                        404,
                        serde_json::json!({"error": t("error.not_found")}),
                        "chat",
                        cors_headers,
                    )
                    .await?;
                }
            }

            Ok(())
        })
        .await;

    let path_label = path.to_string();
    let success = dispatch_result.is_ok();
    crate::observability::performance::record_global_operation(
        success,
        duration.as_secs_f64() * 1000.0,
    );
    info!(
        "HTTP {} completed in {:?} (ok={})",
        path_label, duration, success,
    );

    if let Err(e) = dispatch_result {
        return Err(e);
    }
    Ok(path_label)
}

// ── HTTP response writing ───────────────────────────────────────────

/// Write data to a TcpStream with a 30-second timeout.
/// Returns an error if the write times out or the connection is broken.
pub(crate) async fn tcp_write_timeout(socket: &mut TcpStream, data: &[u8]) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(30), socket.write_all(data))
        .await
        .map_err(|_| anyhow::anyhow!("timeout writing to socket"))?
        .map_err(|e| anyhow::anyhow!("socket write error: {e}"))
}

/// Write an HTTP JSON response with platform profile injection.
pub(crate) async fn write_http_json_response_with_context(
    socket: &mut TcpStream,
    status: u16,
    body: serde_json::Value,
    method: &str,
    extra_headers: &str,
) -> Result<()> {
    let body = inject_platform_profiles_if_absent(body, method);
    write_http_json_response(socket, status, body, extra_headers).await
}

/// Write a standard HTTP JSON response. Thin wrapper for consistency.
#[allow(dead_code)] // F-GAP-03 — planned wiring: lifecycle/utility
async fn write_http_response(
    socket: &mut TcpStream,
    status: u16,
    body: serde_json::Value,
) -> Result<()> {
    write_http_json_response(socket, status, body, "").await
}

/// Compute CORS response headers for an incoming request.
///
/// Extracts the `Origin` header from the request, checks it against the
/// server's CORS configuration, and returns a formatted string of CORS
/// headers (each ending with `\r\n`).  Returns an empty string when CORS
/// is disabled or the origin is not allowed.
fn compute_cors_response_headers(headers: &str, server: &AcpServer) -> String {
    let config = match server.runtime_config.cors_config() {
        Some(c) => c,
        None => return String::new(),
    };
    let origin = extract_header_value(headers, "origin");
    let cors_headers = build_cors_headers(origin.as_deref(), &config);
    if cors_headers.is_empty() {
        return String::new();
    }
    cors_headers
        .iter()
        .map(|(k, v)| format!("{}: {}\r\n", k, v))
        .collect()
}

/// Handle an OPTIONS (CORS preflight) request.
async fn handle_cors_preflight(
    socket: &mut TcpStream,
    headers: &str,
    server: &AcpServer,
) -> Result<()> {
    let config = match server.runtime_config.cors_config() {
        Some(c) => c,
        None => {
            write_http_json_response(
                socket,
                405,
                serde_json::json!({"error": "Method Not Allowed"}),
                "",
            )
            .await?;
            return Ok(());
        }
    };
    let origin = extract_header_value(headers, "origin");
    let allow_origin = origin.as_deref().filter(|o| is_origin_allowed(o, &config));

    if allow_origin.is_none() && !config.allowed_origins.contains(&"*".to_string()) {
        write_http_json_response(
            socket,
            403,
            serde_json::json!({"error": "Origin not allowed"}),
            "",
        )
        .await?;
        return Ok(());
    }

    let rh = extract_header_value(headers, "access-control-request-headers");
    let preflight_headers = build_preflight_response_headers(rh.as_deref(), &config);
    let origin_val = allow_origin.unwrap_or("*").to_string();

    let mut cors_str = format!("Access-Control-Allow-Origin: {}\r\n", origin_val);
    for (k, v) in &preflight_headers {
        cors_str.push_str(&format!("{}: {}\r\n", k, v));
    }
    cors_str.push_str("Access-Control-Max-Age: ");
    cors_str.push_str(&config.max_age_seconds.to_string());
    cors_str.push_str("\r\n");

    write_http_json_response(socket, 200, serde_json::json!({"ok": true}), &cors_str).await?;
    Ok(())
}

/// Check if the user session is authorized for the given request path and method.
/// Returns `Ok(true)` if a response has been written (request is handled/denied),
/// or `Ok(false)` if the request should proceed.
async fn check_http_authorization(
    socket: &mut TcpStream,
    server: &AcpServer,
    user_session: Option<&crate::acp::r#impl::session::UserSession>,
    method: &str,
    path: &str,
    cors_headers: &str,
) -> Result<bool> {
    // If user auth is disabled, allow everything
    if !server.runtime_config.user_auth_enabled {
        return Ok(false);
    }

    // If no session, reject with 401
    let session = match user_session {
        Some(s) => s,
        None => {
            write_http_json_response(
                socket,
                401,
                serde_json::json!({"error": "Authentication required", "code": "AUTH_REQUIRED"}),
                cors_headers,
            )
            .await?;
            return Ok(true);
        }
    };

    // Exempt paths (health, root capabilities)
    if matches!(path, "/" | "/health") {
        return Ok(false);
    }

    // Map HTTP method + path to required permission
    let required_perm = match (method, path) {
        // Admin-only operations
        ("POST", "/rpc") => Permission::Execute,
        ("GET", _) => Permission::Read,
        ("POST", "/chat" | "/chat/stream") => Permission::Execute,
        ("POST", "/chat/completions" | "/v1/chat/completions") => Permission::Execute,
        ("POST", "/v1/responses") => Permission::Execute,
        _ => Permission::Read,
    };

    // Create principal from session
    let principal = Principal::new(
        &session.user_id,
        session.roles.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        session.tenant_id.as_deref(),
    );

    // Resolve permissions from roles (lock is scoped to avoid holding a non-Send
    // guard across .await points)
    let access_decision = server.rbac_enforcer.as_ref().map(|enforcer| {
        let guard = enforcer.read().unwrap_or_else(|e| e.into_inner());
        let mut p = principal.clone();
        guard.resolve_permissions(&mut p);
        guard.check_access(&p, &required_perm)
    });

    if let Some(decision) = access_decision {
        match decision {
            AccessDecision::Allow => {
                return Ok(false);
            }
            AccessDecision::Deny { reason } => {
                write_http_json_response(
                    socket,
                    403,
                    serde_json::json!({
                        "error": "Forbidden",
                        "code": "ACCESS_DENIED",
                        "reason": reason
                    }),
                    cors_headers,
                )
                .await?;
                return Ok(true);
            }
            AccessDecision::Escalate { required_role } => {
                write_http_json_response(
                    socket,
                    403,
                    serde_json::json!({
                        "error": "Insufficient privileges",
                        "code": "PRIVILEGE_ESCALATION_REQUIRED",
                        "required_role": required_role
                    }),
                    cors_headers,
                )
                .await?;
                return Ok(true);
            }
        }
    }

    // No RBAC enforcer configured — apply explicit deployment fallback policy.
    let fallback_action = match required_perm {
        Permission::Execute => GovernanceAction::Shell,
        Permission::Write => GovernanceAction::Write,
        _ => GovernanceAction::Read,
    };
    let fallback = rbac_fallback_allows_action(
        server.runtime_config.deployment_target.as_deref(),
        fallback_action,
    );
    if fallback.allowed {
        return Ok(false);
    }

    write_http_json_response(
        socket,
        403,
        serde_json::json!({
            "error": "Forbidden",
            "code": "RBAC_UNAVAILABLE_POLICY_DENY",
            "reason": fallback.reason,
            "policy": fallback.policy_name,
            "sandbox_level": fallback.sandbox_level,
        }),
        cors_headers,
    )
    .await?;
    Ok(true)
}

// ── Main HTTP connection handler ────────────────────────────────────

/// Main HTTP connection handler — parses, guards, routes, and times the request.
async fn handle_http_connection(
    socket: &mut TcpStream,
    server: Arc<AcpServer>,
    peer_addr: SocketAddr,
) -> Result<()> {
    let mut buffer = vec![0u8; 64 * 1024];
    let bytes_read =
        tokio::time::timeout(std::time::Duration::from_secs(30), socket.read(&mut buffer))
            .await
            .map_err(|_| anyhow::anyhow!("timeout reading HTTP request"))??;
    if bytes_read == 0 {
        return Ok(());
    }

    let request_text = String::from_utf8_lossy(&buffer[..bytes_read]);
    let parsed = parse_http_request(&request_text)?;

    // Compute CORS headers for this request (empty string when disabled)
    let cors_headers = compute_cors_response_headers(parsed.header_part, server.as_ref());

    // Extract user session if user auth is enabled
    let user_session: Option<crate::acp::r#impl::session::UserSession> =
        server.session_manager.as_ref().and_then(|sm| {
            let session = sm.extract_user_from_request(parsed.header_part);
            if let Some(ref s) = session {
                debug!("Authenticated user: {} (roles: {:?})", s.user_id, s.roles);
            }
            session
        });

    // ── RBAC authorization check ──────────────────────────────
    if check_http_authorization(
        socket,
        server.as_ref(),
        user_session.as_ref(),
        parsed.method,
        parsed.path,
        &cors_headers,
    )
    .await?
    {
        return Ok(());
    }

    if parsed.method == "OPTIONS" {
        return handle_cors_preflight(socket, parsed.header_part, server.as_ref()).await;
    }

    if http_entry_guard(
        socket,
        server.as_ref(),
        parsed.header_part,
        parsed.method,
        parsed.path,
        peer_addr,
        &cors_headers,
    )
    .await?
    {
        return Ok(());
    }

    if parsed.method == "GET" {
        return route_http_get(socket, server.as_ref(), parsed.path, &cors_headers).await;
    }

    if parsed.method != "POST" {
        write_http_json_response_with_context(
            socket,
            405,
            serde_json::json!({"error": t("error.method_not_allowed")}),
            "chat",
            &cors_headers,
        )
        .await?;
        return Ok(());
    }

    let _path_label = route_http_post(
        socket,
        server,
        parsed.path,
        parsed.header_part,
        parsed.body_initial_part,
        user_session,
        &cors_headers,
    )
    .await?;

    Ok(())
}

// ── Adaptive signal inference ───────────────────────────────────────

fn infer_adaptive_signal(method: &str, path: &str, headers: &str) -> &'static str {
    if matches!(path, "/chat" | "/chat/stream") {
        return "acp_http_path";
    }
    if matches!(
        path,
        "/chat/completions" | "/v1/chat/completions" | "/v1/responses"
    ) {
        return "openai_http_path";
    }
    if path.starts_with("/v1/") {
        return "openai_api_prefix";
    }

    if let Some(protocol_hint) = extract_header_value(headers, "x-go-on-protocol") {
        let hint = protocol_hint.trim().to_ascii_lowercase();
        if hint == "acp" {
            return "header_hint_acp";
        }
        if hint == "mcp" {
            return "header_hint_mcp";
        }
    }

    if let Some(content_type) = extract_header_value(headers, "content-type") {
        if content_type
            .to_ascii_lowercase()
            .contains("application/json")
        {
            if method == "POST" {
                return "json_post_fallback";
            }
            return "json_http_fallback";
        }
    }

    if method == "GET" {
        "read_probe_fallback"
    } else {
        "generic_http_fallback"
    }
}

// ── HTTP header utilities ───────────────────────────────────────────

fn extract_content_length(headers: &str) -> Option<usize> {
    let mut found: Option<usize> = None;
    for line in headers.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("content-length") {
            continue;
        }
        let val: usize = value.trim().parse().ok()?;
        match found {
            None => found = Some(val),
            Some(prev) if prev == val => {} // duplicate with same value — OK
            Some(_) => return None,         // different values — reject per RFC 7230
        }
    }
    found
}

fn extract_header_value(headers: &str, header_name: &str) -> Option<String> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case(header_name) {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

fn extract_entry_token(headers: &str) -> Option<String> {
    if let Some(auth) = extract_header_value(headers, "authorization") {
        let lower = auth.to_ascii_lowercase();
        if lower.starts_with("bearer ") {
            return Some(auth[7..].trim().to_string());
        }
    }

    extract_header_value(headers, "x-api-key")
        .or_else(|| extract_header_value(headers, "x-go-on-key"))
        .filter(|value| !value.trim().is_empty())
}

fn entry_guard_exempt_path(path: &str) -> bool {
    matches!(path, "/" | "/health")
}

// ── Entry guards ────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn write_entry_rejection(
    socket: &mut TcpStream,
    status: u16,
    code: &str,
    kind: &str,
    message: String,
    source: &str,
    path: &str,
    policy: &str,
    cors_headers: &str,
) -> Result<()> {
    let trace_id = format!("entry-{}", crate::acp::prelude::now_ts_ms());
    write_http_json_response(
        socket,
        status,
        serde_json::json!({
            "ok": false,
            "error": {
                "code": code,
                "kind": kind,
                "message": message,
                "source": source,
                "path": path,
                "policy": policy,
                "trace_id": trace_id,
            }
        }),
        cors_headers,
    )
    .await
}

async fn apply_entry_guards(
    socket: &mut TcpStream,
    server: &AcpServer,
    headers: &str,
    method: &str,
    path: &str,
    peer_addr: SocketAddr,
    cors_headers: &str,
) -> Result<bool> {
    if entry_guard_exempt_path(path) {
        return Ok(false);
    }

    let source = peer_addr.ip().to_string();

    if server.runtime_config.entry_auth_enabled {
        let env_name = server.runtime_config.entry_auth_api_key_env.trim();
        let expected_key = get_secret(env_name)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        if expected_key.is_none() {
            warn!(
                "entry auth enabled but env is missing/empty; denying {} {} from {}",
                method, path, source
            );
            write_entry_rejection(
                socket,
                503,
                "ENTRY_AUTH_MISCONFIGURED",
                "service_unavailable",
                format!(
                    "entry auth is enabled but env '{}' is missing or empty",
                    env_name
                ),
                &source,
                path,
                "entry_auth",
                cors_headers,
            )
            .await?;
            return Ok(true);
        }

        let provided = extract_entry_token(headers)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if provided != expected_key {
            warn!(
                "entry auth rejected {} {} from {} (missing or invalid key)",
                method, path, source
            );
            write_entry_rejection(
                socket,
                401,
                "ENTRY_AUTH_REQUIRED",
                "unauthorized",
                "missing or invalid entry API key".to_string(),
                &source,
                path,
                "entry_auth",
                cors_headers,
            )
            .await?;
            return Ok(true);
        }
    }

    let key = format!("entry:{}", source);
    let rpm_limit = server.runtime_config.entry_rate_limit_rpm.max(1);
    let burst = server.runtime_config.entry_rate_limit_burst.max(1);
    let allowed = server
        .phase_rate_limiter
        .lock()
        .map(|guard| guard.allow(&key, rpm_limit, Some(burst)))
        .unwrap_or(true);

    if !allowed {
        warn!(
            "entry rate limit rejected {} {} from {} (rpm={}, burst={})",
            method, path, source, rpm_limit, burst
        );
        write_entry_rejection(
            socket,
            429,
            "ENTRY_RATE_LIMITED",
            "rate_limited",
            "entry rate limit exceeded".to_string(),
            &source,
            path,
            "entry_rate_limit",
            cors_headers,
        )
        .await?;
        return Ok(true);
    }

    Ok(false)
}

// ── Trace context ───────────────────────────────────────────────────

fn http_trace_context(method: &str) -> RequestTraceContext {
    let request_id = format!("http-{}", crate::acp::prelude::now_ts_ms());
    let seed = Some(serde_json::json!(request_id.clone()));
    let mut trace = chat_trace_context(&seed, "chat.http");
    trace.method = method.to_string();
    trace.request_id = request_id;
    trace
}

// ── HTTP/Socket write helpers ──────────────────────────────────────

pub(crate) async fn write_http_json_response(
    socket: &mut TcpStream,
    status: u16,
    value: serde_json::Value,
    extra_headers: &str,
) -> Result<()> {
    let status_text = match status {
        200 => "OK",
        401 => "Unauthorized",
        429 => "Too Many Requests",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "OK",
    };
    let body = serde_json::to_vec(&value)?;
    let headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n",
        status,
        status_text,
        body.len(),
        extra_headers
    );
    tcp_write_timeout(socket, headers.as_bytes()).await?;
    tcp_write_timeout(socket, &body).await?;
    let _ = socket.shutdown().await;
    Ok(())
}

pub(crate) async fn write_sse_headers(socket: &mut TcpStream, extra_headers: &str) -> Result<()> {
    let header_bytes = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\nX-Accel-Buffering: no\r\n{}\r\n",
        extra_headers
    );
    tcp_write_timeout(socket, header_bytes.as_bytes()).await?;
    Ok(())
}

pub(crate) async fn write_sse_event(
    socket: &mut TcpStream,
    event: &str,
    payload: &serde_json::Value,
) -> Result<()> {
    let frame = format!(
        "event: {}\ndata: {}\n\n",
        event,
        serde_json::to_string(payload)?
    );
    debug!("ACP SSE event: {}", event);
    tcp_write_timeout(socket, frame.as_bytes()).await?;
    tokio::time::timeout(std::time::Duration::from_secs(30), socket.flush())
        .await
        .map_err(|_| anyhow::anyhow!("timeout flushing socket"))?
        .map_err(|e| anyhow::anyhow!("socket flush error: {e}"))?;
    Ok(())
}

// ── Shared HTTP/SSE utilities ────────────────────────────────────────

/// Write the SSE `[DONE]` marker and shutdown the socket.
pub(crate) async fn write_openai_sse_done(socket: &mut TcpStream) -> Result<()> {
    tcp_write_timeout(socket, b"data: [DONE]\n\n").await?;
    tokio::time::timeout(std::time::Duration::from_secs(30), socket.flush())
        .await
        .map_err(|_| anyhow::anyhow!("timeout flushing socket"))?
        .map_err(|e| anyhow::anyhow!("socket flush error: {e}"))?;
    let _ = socket.shutdown().await;
    Ok(())
}

/// Build a root capabilities response listing all available endpoints.
pub(crate) fn build_root_capabilities_response() -> serde_json::Value {
    serde_json::json!({
        "service": "go-on",
        "protocol": "acp-http",
        "health": "/health",
        "endpoints": {
            "chat": ["/chat", "/chat/stream"],
            "openai": ["/v1/models", "/v1/model", "/models", "/v1/chat/completions", "/chat/completions"],
            "responses": ["/v1/responses", "/v1/responses/{id}"],
        }
    })
}

/// Check if an error indicates setup failure or upstream unavailability.
pub(crate) fn is_setup_or_upstream_unavailable(err: &anyhow::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("missing environment variable")
        || msg.contains("error sending request")
        || msg.contains("connection refused")
        || msg.contains("timed out")
}

/// Build a human-readable degraded message for when upstream is unavailable.
pub(crate) fn degraded_openai_message(err: &anyhow::Error) -> String {
    format!(
        "go-on is running, but upstream model service is unavailable. {}. Configure at least one reachable provider (for example set DEEPSEEK_API_KEY) or start your copilot-compatible upstream on 127.0.0.1:8080.",
        err
    )
}
