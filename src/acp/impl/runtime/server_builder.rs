//! Server builder — constructs an `AcpServer` instance with all dependency wiring.
//!
//! Extracted from `runtime.rs` to reduce module size. Contains the original
//! `new_acp_server` constructor and the shared `wire_server` helper.

use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{Mutex, Notify};
use tracing::{debug, info};

use crate::acp::server::{
    AcpServer, CacheLayer, CacheServerDeps, GovernanceServerDeps, ModelServerDeps,
    ObservabilityLayer, OrchestrationServerDeps,
};
use crate::adaptive_selector::AdaptiveModelSelector;
use crate::agent::AgentRegistry;
use crate::config::{AutoTuneConfig, AutoTuneState, RuntimeConfig, VectorConfig};
use crate::failure_prevention::FailurePrevention;
use crate::flow::FlowManager;
use crate::flow_with_models::FlowModelSelector;
use crate::memory_module::{MemoryPolicy, MemoryStore};
use crate::memory_response_cache::MemoryResponseCache;
use crate::observability::telemetry::TelemetryRuntime;
use crate::orchestration::skill::SkillRegistry;
use crate::reinforcement::ArtifactLedger;
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
    let harness_bus = {
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
    // Inject RBAC enforcer into the harness bus and create HTTP-level enforcer (GAP-B58-D05)
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
        // Register tenants from environment-backed sources (activated, formerly F-GAP-15 / MCP-5):
        // - GO_ON_TENANTS (inline IDs)
        // - GO_ON_TENANTS_FILE (file-backed tenant registry)
        let tenant_count = enforcer.register_tenants_from_sources();
        if tenant_count > 0 {
            tracing::info!(
                "registered {} tenant(s) from {} and/or {}",
                tenant_count,
                crate::governance::rbac::GO_ON_TENANTS_ENV,
                crate::governance::rbac::GO_ON_TENANTS_FILE_ENV,
            );
        }
        // Wrap enforcer in a shared Arc<RwLock> so that both the harness bus
        // and the HTTP-level enforcer reference the same instance (GAP-B58-D05).
        let shared = Arc::new(std::sync::RwLock::new(enforcer));
        // Inject a clone of the Arc into the harness bus policy evaluator.
        harness_bus.set_rbac_enforcer(Arc::clone(&shared));
        shared
    };

    let workflow_registry = Arc::new(std::sync::Mutex::new(
        crate::orchestration::workflow_registry::WorkflowRegistry::new(),
    ));
    // Create a shared provenance ledger
    let provenance_ledger = Arc::new(crate::observability::provenance::ProvenanceLedger::default());
    // BLUE56-GAP-B02: Inject first available LLM agent into MetacognitiveController
    let first_agent: Option<Arc<dyn crate::agent::Agent>> = registry
        .get("coder")
        .or_else(|| registry.get("assistant"))
        .or_else(|| {
            let names = registry.names();
            names.first().and_then(|n| registry.get(n))
        });

    // Build capability bus and optionally inject an LLM agent
    let cb_builder = crate::intelligence::capability_bus::core::CapabilityBus::new_default(
        Arc::clone(&harness_bus),
        Some(workflow_registry),
    )
    .with_provenance_ledger(Arc::clone(&provenance_ledger));
    let cb_builder = if let Some(agent) = first_agent {
        cb_builder.with_metacognitive_llm(agent)
    } else {
        cb_builder
    };
    let mut capability_bus = Arc::new(cb_builder);

    // GAP-B58-B09: Set self-model identity from package metadata
    if let Some(cb_mut) = Arc::get_mut(&mut capability_bus) {
        cb_mut
            .self_model
            .set_identity(crate::intelligence::self_model::SelfIdentity {
                system_name: env!("CARGO_PKG_NAME").to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                description: "Go-On ACP cognitive engine with full capability bus".to_string(),
                creator: "go-on".to_string(),
                created_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
                tags: vec!["acp".to_string(), "intelligence".to_string()],
            });
        tracing::info!("self_model: system identity set from package metadata");
    }

    // BLUE56-C15: Warn if default credentials are still in use
    {
        let env_name = runtime_config.entry_auth_api_key_env.trim();
        if runtime_config.entry_auth_enabled && !env_name.is_empty() {
            let key = std::env::var(env_name).unwrap_or_default();
            if key == "change-me" || key.is_empty() {
                tracing::warn!(
                    target: "startup",
                    env = %env_name,
                    "entry auth enabled but API key is still default/empty — set {} to a random secret",
                    env_name
                );
            }
        }
    }

    // ── Governance dependency wiring ──────────────────────────────────────
    // Wire approval engine with preference learner (GAP-B58-D01)
    {
        use crate::governance::approval_engine::{ApprovalEngine, TimeoutPolicy};
        use crate::governance::approval_learning::ApprovalPreferenceLearner;
        use crate::governance::pua::PuaRuleEngine;
        let pua_plan = Arc::new(std::sync::Mutex::new(
            crate::pua::PuaEnforcementPlan::default(),
        ));
        let pua_rule_engine = Arc::new(tokio::sync::Mutex::new(PuaRuleEngine::new(pua_plan)));
        let preference_learner = Arc::new(std::sync::RwLock::new(
            ApprovalPreferenceLearner::with_thresholds(20, 0.9),
        ));
        let engine = Arc::new(std::sync::RwLock::new(
            ApprovalEngine::new(pua_rule_engine, TimeoutPolicy::default())
                .with_learner(preference_learner),
        ));
        builder = builder.with_approval_engine(engine);
    }

    // Wire injection detector with runtime config (BLUE56-GAP-D08)
    {
        use crate::security::prompt_injection::InjectionDetector;
        let detector = Arc::new(InjectionDetector::new(runtime_config.detection_config()));
        builder = builder.with_injection_detector(detector);
    }

    // Wire safety checker
    {
        use crate::security::content_safety::{ContentSafetyConfig, SafetyChecker};
        // SafetyChecker::new is now infallible — regex compilation errors are
        // logged and result in an empty ruleset rather than a hard failure.
        let checker = Arc::new(SafetyChecker::new(ContentSafetyConfig::default()));
        builder = builder.with_safety_checker(checker);
    }

    // Wire hash chain auditor (requires config path)
    if let Some(ref path) = config_path {
        let auditor_path = std::path::Path::new(path)
            .parent()
            .map(|p| p.join("audit_chain.ndjson"));
        if let Some(auditor_path) = auditor_path {
            use crate::security::audit_integrity::HashChainAuditor;
            match HashChainAuditor::new(auditor_path) {
                Ok(auditor) => {
                    builder =
                        builder.with_hash_chain_auditor(Arc::new(std::sync::Mutex::new(auditor)));
                }
                Err(e) => {
                    tracing::warn!("Failed to create HashChainAuditor: {}", e);
                }
            }
        }
    }

    // Wire secret manager
    {
        use crate::security::secret_rotation::{MemoryRotator, RotationPolicy, SecretManager};
        let rotator = Arc::new(MemoryRotator::new());
        let manager = Arc::new(SecretManager::new(RotationPolicy::default(), rotator));
        builder = builder.with_secret_manager(manager);
    }

    // Wire memory persistence and memory retrieval engine (GAP-B58-D03)
    {
        use crate::memory::memory_bridge::memory_base_path;
        use crate::memory::memory_persistence::MemoryPersistence;
        use crate::memory::memory_retrieval::MemoryRetrievalEngine;
        let base = memory_base_path();
        let db_path = base.join("warm.db");
        let cold_path = base.join("cold");
        if let Ok(mp) = MemoryPersistence::new(&db_path, &cold_path, None) {
            // Create a separate MemoryPersistence for the retrieval engine.
            // Both instances share the same underlying SQLite database store.
            let retrieval_engine = MemoryPersistence::new(&db_path, &cold_path, None)
                .ok()
                .map(|retrieval_mp| Arc::new(MemoryRetrievalEngine::new(retrieval_mp)));
            let mp = Arc::new(mp);
            builder = builder.with_memory_persistence(mp);
            if let Some(engine) = retrieval_engine {
                builder = builder.with_memory_retrieval_engine(engine);
            } else {
                tracing::warn!("Memory retrieval engine not wired (secondary persistence failed)");
            }
        } else {
            tracing::warn!("Failed to create MemoryPersistence");
        }
    }

    // Wire evolution loop
    {
        use crate::orchestration::self_evolution::evolution_loop::EvolutionLoop;
        let workdir = std::path::PathBuf::from(".goon/evolution");
        let evolution_loop = Arc::new(tokio::sync::Mutex::new(EvolutionLoop::new(workdir)));
        builder = builder.with_evolution_loop(evolution_loop);
    }

    // ── Security scanning (GAP-B52-24, GAP-B52-30) ──────────────────────
    // Wire dependency vulnerability scanner
    {
        use crate::security::vulnerability_scan::DependencyVulnerabilityScanner;
        let scanner = DependencyVulnerabilityScanner::new()
            .with_min_severity(crate::security::vulnerability_scan::Severity::Medium);
        if let Some(ref path) = config_path {
            if let Some(parent) = std::path::Path::new(path).parent() {
                if let Some(project_root) = parent.to_str() {
                    builder = builder.with_dependency_vulnerability_scanner(Arc::new(
                        scanner.with_scan_path(project_root),
                    ));
                } else {
                    builder = builder.with_dependency_vulnerability_scanner(Arc::new(scanner));
                }
            } else {
                builder = builder.with_dependency_vulnerability_scanner(Arc::new(scanner));
            }
        } else {
            builder = builder.with_dependency_vulnerability_scanner(Arc::new(scanner));
        }
    };

    // Wire secret exposure detector
    {
        use crate::security::vulnerability_scan::SecretExposureDetector;
        let detector = SecretExposureDetector::default();
        builder = builder.with_secret_exposure_detector(Arc::new(detector));
    }

    // Wire permit exposure analyzer
    {
        use crate::security::vulnerability_scan::PermitExposureAnalyzer;
        let analyzer = PermitExposureAnalyzer::default();
        builder = builder.with_permit_exposure_analyzer(Arc::new(analyzer));
    }

    // Wire security advisor agent
    let _security_advisor = {
        use crate::security::security_advisor::{SecurityAdvisorAgent, SecurityAdvisorConfig};
        let advisor = Arc::new(SecurityAdvisorAgent::new(SecurityAdvisorConfig::default()));
        builder = builder.with_security_advisor(Arc::clone(&advisor));
        advisor
    };

    // Wire multimodal processor for document, audio, video, and repo analysis (GAP-B55-110)
    // MM-FIX1: Activate all sub-processors when multimodal features are available.
    #[cfg(any(feature = "sub-bus-multimodal", feature = "profile-full"))]
    {
        use crate::multimodal::MultimodalProcessor;
        builder = builder.with_multimodal_processor(MultimodalProcessor::new_with_all_processors());
    }
    #[cfg(not(any(feature = "sub-bus-multimodal", feature = "profile-full")))]
    {
        use crate::multimodal::MultimodalProcessor;
        builder = builder.with_multimodal_processor(MultimodalProcessor::default());
    }

    // Wire policy reloader for hot-reloading governance policies (GAP-B58-D04)
    {
        use crate::governance::reloadable_policy::{
            PolicyReloader, QualityCompassPolicy, RedLinePolicy, SandboxPolicyReloadable,
        };
        let mut reloader = PolicyReloader::new();
        // Register concrete reloadable policies.
        reloader.register(Box::new(RedLinePolicy::new(".goon/policies/redlines.toml")));
        reloader.register(Box::new(QualityCompassPolicy::new(
            ".goon/policies/quality_compass.toml",
        )));
        reloader.register(Box::new(SandboxPolicyReloadable::new(
            ".goon/policies/sandbox.toml",
        )));
        let reloader = Arc::new(std::sync::Mutex::new(reloader));
        builder = builder.with_policy_reloader(reloader);
    }

    match builder.build() {
        Ok(mut server) => {
            // Set fields that aren't available in ServerBuilder yet
            server.cache_deps.vector_config = vector_config;
            server.cache_deps.autotune = autotune;
            server.cache_deps.autotune_config = autotune_config;
            server.cache_deps.autotune_state_path = autotune_state_path;
            server.config_path = config_path;
            server.runtime_config = runtime_config;
            server.verbose = _verbose;
            server.governance_deps.harness_bus = Some(harness_bus);
            server.governance_deps.capability_bus = Some(capability_bus);
            server.governance_deps.provenance_ledger = Some(provenance_ledger);
            server.governance_deps.rbac_enforcer = Some(rbac_enforcer);
            server.skill_market_registry = None;
            server.rate_limit_middleware = Some(Arc::new(
                crate::protocol::rate_limit::RateLimitMiddleware::new(
                    crate::protocol::rate_limit::TenantRateLimit::default(),
                ),
            ));

            // B51-26: Shared wiring extracted to wire_server()
            wire_server(&mut server, &registry);

            // GAP-B52-30: Register security advisor alert channel with the alert manager.
            // Forward security alerts to the observability pipeline.
            // Use spawn to circumvent the non-async context of new_acp_server.
            if let Some(ref advisor) = server.governance_deps.security_advisor {
                let (alert_tx, mut alert_rx) = tokio::sync::mpsc::unbounded_channel();
                let alert_tx_clone = alert_tx.clone();
                let advisor_clone = Arc::clone(advisor);
                tokio::spawn(async move {
                    advisor_clone.register_ws_sender(alert_tx_clone).await;
                });
                let alert_manager = Arc::clone(&server.observability.alert_manager);
                tokio::spawn(async move {
                    use crate::shared::alert_severity::AlertSeverity;
                    while let Some(security_alert) = alert_rx.recv().await {
                        let _severity = match &security_alert.severity {
                            crate::security::vulnerability_scan::Severity::Critical => {
                                AlertSeverity::Critical
                            }
                            crate::security::vulnerability_scan::Severity::High => {
                                AlertSeverity::Warning
                            }
                            _ => AlertSeverity::Info,
                        };
                        let mut mgr = alert_manager.lock().unwrap_or_else(|poisoned| {
                            tracing::warn!("alert_manager lock poisoned");
                            poisoned.into_inner()
                        });
                        // Map AlertSource to a string label
                        let source_label = match &security_alert.source {
                            crate::security::security_advisor::AlertSource::DependencyVulnerability => "dependency",
                            crate::security::security_advisor::AlertSource::SecretExposure => "secret",
                            crate::security::security_advisor::AlertSource::PermitExposure => "permit",
                            crate::security::security_advisor::AlertSource::SecurityAdvisor => "advisor",
                            crate::security::security_advisor::AlertSource::UserReported => "user",
                        };
                        mgr.evaluate(
                            &format!("security.{}", source_label),
                            match security_alert.severity {
                                crate::security::vulnerability_scan::Severity::Critical => 9.0,
                                crate::security::vulnerability_scan::Severity::High => 7.0,
                                crate::security::vulnerability_scan::Severity::Medium => 5.0,
                                crate::security::vulnerability_scan::Severity::Low => 2.0,
                                crate::security::vulnerability_scan::Severity::Unknown => 0.0,
                            },
                        );
                    }
                });
            }

            // BLUE48 Step 2: Pre-initialize SSE buffer pool at startup to
            // avoid first-request latency penalty from lazy initialization.
            crate::acp::r#impl::chat::pre_init_sse_buffer_pool();

            // GAP-B55-020: Initialize memory bridge with auto-migration
            // Called unconditionally (not gated on vector_store) so bridge
            // functions bridge_store/bridge_promote are wired early.
            crate::memory::memory_bridge::init_memory_persistence_with_auto_migrate(None);
            tracing::info!("memory bridge: auto-migration task started");

            // BLUE48 Step 1: Initialize global embedding vector store for
            // semantic task classification in the Planner.
            if let Some(ref vs) = server.cache_deps.cache.vector_store {
                crate::orchestration::planner_embedding::init_global_task_vector_store(Arc::clone(
                    vs,
                ));

                // GAP-B58-B12: Pre-initialize AgentMemoryBus with VectorStore
                // so retrieve_memories() uses vector similarity instead of linear scan.
                crate::memory::agent_memory_bus::init_agent_memory_bus_with_vector_store(
                    Arc::clone(vs),
                );
            }

            // BLUE57-B01: Inject cache backends into CapabilityBus MemoryBus
            // CapabilityBus does not implement Clone, so Arc::make_mut cannot be used.
            // Arc::get_mut warns if the Arc is already shared (GAP-B58-C08).
            if let Some(ref mut cb_arc) = server.governance_deps.capability_bus {
                if let Some(cb_mut) = Arc::get_mut(cb_arc) {
                    #[cfg(feature = "sub-bus-memory")]
                    cb_mut.memory_bus.set_backends(
                        Some(server.cache_deps.cache.response_cache.clone()),
                        Some(server.cache_deps.cache.vector_store.clone()),
                        None,
                        Some(Some(Arc::clone(
                            &server.cache_deps.cache.memory_response_cache,
                        ))),
                    );
                    tracing::info!("capability_bus: memory bus backends injected");
                } else {
                    tracing::warn!(
                        "capability_bus: Arc already shared, cannot inject memory backends"
                    );
                }
            }

            // GAP-B55-042: Start approval engine timeout processing background task
            if let Some(ref approval_engine) = server.governance_deps.approval_engine {
                let engine = Arc::clone(approval_engine);
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
                    loop {
                        interval.tick().await;
                        let mut guard = match engine.write() {
                            Ok(g) => g,
                            Err(poisoned) => {
                                tracing::error!("approval_engine lock poisoned in timeout task");
                                poisoned.into_inner()
                            }
                        };
                        let changed = guard.process_timeouts();
                        if !changed.is_empty() {
                            tracing::info!(
                                "approval engine timed out {} request(s)",
                                changed.len()
                            );
                        }
                    }
                });
            }

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

            let cache_layer = CacheLayer {
                response_cache: cache.clone(),
                memory_response_cache: Arc::new(StdMutex::new(MemoryResponseCache::default())),
                vector_store: vector_store.clone(),
                token_cache: Arc::new(crate::intelligence::token_cache::TokenMultiLevelCache::new(
                    500,
                    200,
                    ".goon/token_cache",
                )),
                semantic_cache: Arc::new(StdMutex::new(
                    crate::memory::semantic_cache::SemanticResponseCache::new(Default::default()),
                )),
            };
            let mut fallback_server = AcpServer {
                cache_deps: CacheServerDeps {
                    cache: cache_layer,
                    vector_config,
                    autotune,
                    autotune_config,
                    autotune_state_path,
                },
                model_deps: ModelServerDeps {
                    flow_manager: Some(flow.clone()),
                    agent_registry: Some(registry.clone()),
                    adaptive_model_selector: Arc::new(StdMutex::new(AdaptiveModelSelector::new())),
                    flow_model_selector: Arc::new(StdMutex::new(FlowModelSelector {})),
                },
                governance_deps: GovernanceServerDeps {
                    harness_bus: Some(Arc::clone(&harness_bus)),
                    capability_bus: Some(Arc::clone(&capability_bus)),
                    pua_enforcement_plan: Arc::new(StdMutex::new(crate::pua::PuaEnforcementPlan {
                        escalation_level: String::new(),
                        mandatory_roles: Vec::new(),
                        red_lines: Vec::new(),
                        quality_compass: Vec::new(),
                        mandatory_safeguards: Vec::new(),
                        mandatory_evidence: Vec::new(),
                        stage_requirements: Vec::new(),
                    })),
                    rbac_enforcer: None,
                    provenance_ledger: Some(provenance_ledger),
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
                },
                orchestration_deps: OrchestrationServerDeps {
                    scheduler: None,
                    planner: crate::orchestration::planner_executor::Planner,
                    executor: crate::orchestration::planner_executor::Executor,
                    planner_executor_config: Default::default(),
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
                },
                config_path: config_path.clone(),
                runtime_config: runtime_config.clone(),
                observability: ObservabilityLayer {
                    metrics: Arc::new(RuntimeMetrics::new()),
                    lock_monitor: Arc::new(AcpLockMonitor::default()),
                    telemetry_runtime: Arc::new(StdMutex::new(TelemetryRuntime::new(
                        &runtime_config,
                    ))),
                    alert_manager: Arc::new(StdMutex::new(
                        crate::observability::alert_manager::AlertManager::new(
                            crate::observability::alert_manager::default_alert_rules(),
                        ),
                    )),
                },
                online_controller: Arc::new(StdMutex::new(OnlineControllerState::default())),
                circuit_breakers: Arc::new(StdMutex::new(CircuitBreakerRegistry::new())),
                hyper_resilience: Arc::new(
                    crate::resilience::hyper_resilience::HyperResilienceEngine::new(
                        crate::resilience::hyper_resilience::ResilienceConfig::default(),
                    ),
                ),
                maintenance_tracker: Arc::new(StdMutex::new(MaintenanceTracker::new())),
                inflight_limiter: Arc::new(StdMutex::new(InflightLimiter::default())),
                lifecycle_state: Arc::new(StdMutex::new(LifecycleState::new())),
                conversation_state: Arc::new(Mutex::new(ConversationState::default())),
                phase_rate_limiter: Arc::new(StdMutex::new(PhaseRateLimiter::default())),
                review_timeout_policy: Arc::new(StdMutex::new(ReviewTimeoutPolicy {
                    timeout_seconds: None,
                    fail_on_timeout: false,
                })),
                failure_prevention: Arc::new(StdMutex::new(failure_prevention_state)),
                memory_store: Arc::new(StdMutex::new(MemoryStore::new(MemoryPolicy::default()))),
                artifact_ledger: Arc::new(StdMutex::new(ArtifactLedger::new(
                    config_path.as_deref().map(Path::new),
                ))),
                fork_registry: Arc::new(StdMutex::new(
                    crate::orchestration::fork_registry::ForkRegistry::new(
                        crate::orchestration::fork_registry::ForkConfig::default(),
                    ),
                )),
                verbose: _verbose,
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
                prompt_manager: crate::acp::r#impl::request::prompts_pack::PromptManager::new(
                    std::path::PathBuf::from("./prompts"),
                ),
                session_manager: None,
                skill_market_registry: None,
                rate_limit_middleware: None,
                audit_log: crate::governance::audit::ThreadSafeAuditLog::new_with_default_path(
                    10_000,
                ),
                tool_registry: Arc::new(crate::orchestration::tool::ToolRegistry::new()),
                drain_guard: crate::acp::server::DrainGuard::default(),
                session_registry: None,
                websocket_hub: None,
                multimodal_processor: Some(crate::multimodal::MultimodalProcessor::default()),
            };

            // Wire RBAC enforcer into the fallback server for HTTP-level authorization
            fallback_server.governance_deps.rbac_enforcer = Some(rbac_enforcer);

            // B51-26: Shared wiring extracted to wire_server()
            wire_server(&mut fallback_server, &registry);

            fallback_server
        }
    }
}

/// Shared wiring applied after AcpServer construction in both the primary builder
/// success path and the fallback path.  Extracted to eliminate ~250 lines of
/// duplicated code between the two branches (B51-26).
fn wire_server(server: &mut AcpServer, registry: &AgentRegistry) {
    // Create session manager if user auth is enabled
    if server.runtime_config.user_auth_enabled {
        use crate::acp::r#impl::session::{AuthConfig, SessionManager};
        let auth_config = AuthConfig::from(&server.runtime_config);
        server.session_manager = Some(Arc::new(SessionManager::with_auth_config(auth_config)));
    }

    // Wire dual-level task scheduler (ARCH-02): create the scheduler and
    // register one worker per known agent so the priority queue has real routing
    // targets.  The scheduler tracks queue depth and active-worker counts that
    // are surfaced in governance.status.
    {
        let db_path = server
            .runtime_config
            .sqlite_vacuum_interval_cycles
            .checked_add(1)
            .map(|_| std::path::PathBuf::from("scheduler_queue.db"));
        let _persistent = crate::orchestration::scheduler::create_persistent_scheduler(db_path);
        let config = crate::orchestration::scheduler::SchedulerConfig::default();
        let s = Arc::new(crate::orchestration::scheduler::AgentWorkerScheduler::new(
            config,
        ));
        for agent_name in registry.names() {
            if let Err(e) = s.register_worker(&agent_name, &agent_name) {
                tracing::warn!(
                    "failed to register worker for agent '{}': {}",
                    agent_name,
                    e
                );
            }
        }
        // Start the aging timer so queued tasks receive periodic priority
        // boosts and don't starve (B51-09).
        s.level1
            .start_aging_timer(std::time::Duration::from_secs(5));
        server.orchestration_deps.scheduler = Some(s);
    }

    if server.runtime_config.skills_enabled {
        server.register_skill(Arc::new(crate::orchestration::skill::EchoSkill));
        server.register_skill(Arc::new(
            crate::orchestration::skill::SkillCreatorSkill::new(
                server.orchestration_deps.skill_registry.clone(),
            ),
        ));
    }

    // Wire the skill registry into the global discovery engine
    crate::acp::r#impl::request::tools_pack::init_skill_discovery(
        server.orchestration_deps.skill_registry.clone(),
    );

    // Wire the new modules' state from CapabilityBus into the server's
    // standalone fields so process_chat_request can access them directly.
    server.schema_registry = Arc::clone(
        &server
            .governance_deps
            .capability_bus
            .as_ref()
            .map(|cb| Arc::clone(&cb.schema_registry))
            .unwrap_or_default(),
    );
    server.tenant_budget = Arc::clone(
        &server
            .governance_deps
            .capability_bus
            .as_ref()
            .map(|cb| Arc::clone(&cb.tenant_budget))
            .unwrap_or_default(),
    );

    // Auto-provision a default tenant quota when user auth is enabled so
    // the budget enforcer does not reject every request with "no quota
    // configured for tenant 'default-tenant'" (F-GAP-08).
    if server.runtime_config.user_auth_enabled {
        let mut budget = server.tenant_budget.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("tenant_budget lock poisoned in wire_server");
            poisoned.into_inner()
        });
        budget.auto_provision_default(&server.runtime_config);
    }

    server.optimizer_registry = Arc::clone(
        &server
            .governance_deps
            .capability_bus
            .as_ref()
            .map(|cb| Arc::clone(&cb.optimizer_registry))
            .unwrap_or_default(),
    );

    // Wire the token cache into the agent registry so that all
    // agents returned by registry.get() are automatically wrapped
    // with CachedAgentWrapper.
    registry.set_token_cache(Some(Arc::clone(&server.cache_deps.cache.token_cache)));

    // BLUE48 Step 19: Initialize intelligence hub at startup so consensus
    // voting, rationalization, and audit are wired into the request path.
    crate::intelligence::hub::init_intel_hub(false);

    // Initialise the 3 concrete AgentVoter impls for the Delphi debate /
    // weighted-vote system.
    crate::intelligence::hub::init_intel_voters(server.governance_deps.capability_bus.clone());

    // BLUE51 Step 1: Wire WebSocket hub to SessionRegistry for real-time sync
    let session_registry = Arc::new(crate::protocol::session_sync::SessionRegistry::new());
    let ws_hub = Arc::new(crate::protocol::websocket::WebSocketHub::new(
        crate::protocol::websocket::WebSocketConfig::default(),
    ));

    // Start WebSocket heartbeat and wire broadcast fn.
    // new_acp_server is sync, so wrap the one-time async setup in block_in_place.
    // Use try_current() to avoid panicking if no runtime is present (GAP-B58-C07).
    {
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => {
                tracing::warn!("No tokio runtime available — skipping WebSocket heartbeat setup");
                return;
            }
        };
        let ws = ws_hub.clone();
        let sr = session_registry.clone();
        tokio::task::block_in_place(move || {
            handle.block_on(async {
                ws.start_heartbeat().await;
                let broadcast_fn = ws.create_broadcast_fn();
                sr.set_broadcast_fn(broadcast_fn).await;
            });
        });
    }

    server.session_registry = Some(session_registry);
    server.websocket_hub = Some(ws_hub);

    // ── Federated learning initializer (activated, formerly BLUE2 / F-GAP-19) ────
    // If FEDERATED_PEERS is set, initialize the FederatedRLAdapter with
    // differential privacy and model versioning enabled.
    {
        use crate::intelligence::reinforcement::FederatedRLAdapter;
        use std::sync::OnceLock;

        static FEDERATED_ADAPTER: OnceLock<FederatedRLAdapter> = OnceLock::new();

        if std::env::var("FEDERATED_PEERS").is_ok() {
            let adapter = FederatedRLAdapter::new(true, true);
            if FEDERATED_ADAPTER.set(adapter).is_ok() {
                info!("wire_server: FederatedRLAdapter initialized with privacy + versioning");
            }
        } else {
            debug!("wire_server: FEDERATED_PEERS not set, federated learning disabled");
        }
    }

    // ── Memory health monitor (F-GAP-49 activation) ───────────────
    // Start background memory pressure monitoring. Periodically queries
    // system memory, logs warnings on low/critical conditions, and
    // evaluates AlertManager rules for threshold-based alerting.
    crate::observability::memory_health::start_memory_monitor();
    info!(
        "memory health monitor started (interval: {}s)",
        crate::observability::memory_health::MEMORY_MONITOR_INTERVAL_SECS
    );

    // ── Wire security subsystems (GAP-B52, S-FIX3) ────────────────────
    crate::security::wire_content_safety(&server.runtime_config);
    crate::security::wire_prompt_injection(&server.runtime_config);
    crate::security::wire_cert_monitor(&server.runtime_config);
    crate::security::start_secret_rotation_if_configured(&server.runtime_config);
}
