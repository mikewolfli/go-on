//! Server builder — constructs an `AcpServer` instance with all dependency wiring.
//!
//! Extracted from `runtime.rs` to reduce module size. Contains the original
//! `new_acp_server` constructor and the shared `wire_server` helper.

use std::path::Path;
use std::sync::{Arc, RwLock};

use tracing::info;

use crate::acp::server::AcpServer;
use crate::agent::AgentRegistry;
use crate::config::{AutoTuneConfig, AutoTuneState, RuntimeConfig, VectorConfig};
use crate::flow::FlowManager;

use crate::orchestration::skill::SkillRegistry;
use crate::reinforcement::ArtifactLedger;
use crate::vector::VectorStore;

/// Create a new ACP server instance
///
/// This function replaces the `AcpServer::new` constructor.
#[allow(clippy::too_many_arguments)]
pub async fn new_acp_server(
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

    app_config: Option<Arc<crate::config::AppConfig>>,
    skill_registry: Option<Arc<RwLock<SkillRegistry>>>,
    // Whether to wire the durable response cache as the token cache's L3 layer.
    persist_cache: bool,
) -> AcpServer {
    // Use ServerBuilder to create the server with correct field names and types
    use crate::acp::server::ServerBuilder;

    let mut builder = ServerBuilder::new();

    // If a pre-loaded skill registry was provided by bootstrap, inject it
    // so that ServerBuilder::build() skips the redundant disk scan.
    if let Some(registry) = skill_registry {
        builder = builder.with_pre_loaded_skill_registry(registry);
    }

    // Set the components that ServerBuilder supports
    builder = builder.with_flow_manager(flow.clone());
    builder = builder.with_agent_registry(registry.clone());

    if let Some(ref cache) = cache {
        builder = builder.with_response_cache(cache.clone());
    }
    builder = builder.with_persist_cache(persist_cache);

    if let Some(ref vector_store) = vector_store {
        builder = builder.with_vector_store(vector_store.clone());
    }
    if let Some(ref path) = config_path {
        builder = builder.with_artifact_ledger(ArtifactLedger::new(Some(Path::new(path))));
    }
    builder = builder.with_config_path(config_path.clone());

    // Wire the configured `global_max_inflight` phase option into the
    // DrainGuard semaphore so operators can actually tune the process-wide
    // request concurrency cap (previously the option was validated and
    // reported but never consumed at runtime). Max across phases = the most
    // permissive phase's cap; clamped to the validator's allowed range.
    if let Some(ref cfg) = app_config {
        let cap = cfg
            .phases
            .values()
            .filter_map(|p| p.options.as_ref())
            .filter_map(|o| o.extra.get("global_max_inflight"))
            .filter_map(|v| v.as_u64())
            .map(|v| v as usize)
            .max()
            .map(|v| v.clamp(1, 10_000));
        if let Some(cap) = cap {
            builder = builder.with_request_inflight_cap(cap);
        }
    }

    // Note: ServerBuilder doesn't have methods for all parameters yet
    // For now, we'll build with defaults and let the caller set additional fields.
    // The builder already initializes ForkRegistry, Planner, Executor, BenchmarkSuite,
    // SchemaRegistry, TenantBudgetEnforcer, OptimizerRegistry, PromptAssembler, and
    // PromotionRegistry with sensible defaults.
    // Create HarnessBus and CapabilityBus to wire into the server
    let harness_bus = {
        if let Some(ref cfg) = app_config {
            Arc::new(crate::governance::harness_bus::config_aware_harness_bus(
                cfg.as_ref(),
            ))
        } else {
            // When app_config is not provided, try to load it from disk.
            // This ensures governance settings are respected.
            let loaded_config = config_path
                .as_ref()
                .and_then(|p| crate::config::AppConfig::load(std::path::Path::new(p)).ok());
            if let Some(ref cfg) = loaded_config {
                Arc::new(crate::governance::harness_bus::config_aware_harness_bus(
                    cfg,
                ))
            } else {
                Arc::new(crate::governance::harness_bus::default_harness_bus())
            }
        }
    };

    // Start the drift monitor once at server startup (checks for metric drift
    // every 60 seconds). Deferred out of `HarnessBus::new` so synchronous
    // constructions outside a tokio runtime do not spawn background tasks.
    harness_bus.start_drift_monitor(60);

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
        // Wrap enforcer in a shared Arc<RwLock> so that both the harness bus
        // and the HTTP-level enforcer reference the same instance (GAP-B58-D05).
        let shared = Arc::new(std::sync::RwLock::new(enforcer));
        // Inject a clone of the Arc into the harness bus policy evaluator.
        harness_bus.set_rbac_enforcer(Arc::clone(&shared));
        // S2 startup optimization: defer tenant registration from env/file sources
        // to a background tokio task. This avoids blocking fs read + env var access
        // during the critical startup path (saves ~5-20ms).
        // Tenants will be registered shortly after the server starts.
        let rbac_for_lazy = Arc::clone(&shared);
        tokio::spawn(async move {
            if let Ok(mut guard) = rbac_for_lazy.write() {
                let count = guard.register_tenants_from_sources();
                if count > 0 {
                    tracing::info!(
                        "registered {} tenant(s) from {} and/or {} (lazy, S2)",
                        count,
                        crate::governance::rbac::GO_ON_TENANTS_ENV,
                        crate::governance::rbac::GO_ON_TENANTS_FILE_ENV,
                    );
                }
            }
        });
        shared
    };

    // NOTE: Intentionally using std::sync::Mutex (not tokio::sync::Mutex).
    // This is a startup-time-only construction; the mutex is never in a hot path.
    // See docs/log/log-20260625-1.md §Remaining Non-Issues.
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
    // Wire LivePerformanceFeed so model observability is available to decide() (P2-6).
    // Uses the process-global feed so fallback outcome recording and model
    // estimation share one instance.
    let perf_feed = crate::observability::live_performance::global_live_performance().clone();
    let cb_builder = crate::intelligence::capability_bus::core::CapabilityBus::new_default(
        Arc::clone(&harness_bus),
        Some(workflow_registry),
    )
    .with_capability_graph(registry.get_capability_graph())
    .with_provenance_ledger(Arc::clone(&provenance_ledger))
    .with_live_performance(Arc::clone(&perf_feed));
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
                created_ms: crate::shared::timestamps::now_ts_ms_u64(),
                tags: vec!["acp".to_string(), "intelligence".to_string()],
            });
        tracing::info!("self_model: system identity set from package metadata");

        // D1: Inject the first available LLM agent into ContinuousLearningCenter
        // so `review_cycle` → `llm_distill` uses a real LLM for semantic
        // pattern extraction instead of always falling back to TF-IDF.
        // (Previously the injection went through a throwaway handle created in
        // main/mod.rs that was never read by the center — a dead pipeline.)
        if let Some(agent) = &first_agent {
            cb_mut
                .continuous_learning
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .inject_agent(agent.clone());
            tracing::info!("continuous_learning: injected LLM agent for semantic distillation");
        }
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
    // Wire injection detector with runtime config (BLUE56-GAP-D08)
    {
        use crate::security::prompt_injection::InjectionDetector;
        let detector = Arc::new(InjectionDetector::new(runtime_config.detection_config()));
        builder = builder.with_injection_detector(detector);
    }

    // Wire safety checker — REMOVED: the injected GovernanceServerDeps
    // SafetyChecker was never read by any code path. PolicyEvaluator exposes
    // `pub safety_checker: Option<SafetyChecker>` as a designed extension
    // point (check_tool_call / verify_output branches), but wiring the
    // default config (threshold=Low, PII scanning on) would block legitimate
    // tool calls on low-severity matches (e.g. an email address in args),
    // so the wiring is intentionally left to an explicit conservative config.

    // Wire hash chain auditor — REMOVED in the audit-pipeline unification:
    // the canonical sink (`ThreadSafeAuditLog` / `global_audit_log`) now owns
    // the tamper-evident chain and chains every persisted record in its own
    // writer thread (see `governance/audit.rs`). No per-server auditor is
    // needed anymore.

    // Wire secret manager — REMOVED: the SecretManager rotation subsystem
    // (register_key/get_key/rotate_key/VaultRotator) had zero production
    // callers and no key registration path, so the 24h rotation loop rotated
    // an always-empty store. The whole dormant subsystem was deleted in
    // log-20260730-18; secret material lives in the keyring-backed secret
    // commands and the Hub vault instead.

    // Wire memory persistence and memory retrieval engine (GAP-B58-D03) — LAZY INIT
    //
    // **Startup optimization (S1)**: Instead of creating two `MemoryPersistence`
    // instances (each opening a SQLite connection + running DDL) synchronously
    // during `new_acp_server()`, we store the paths and defer the actual SQLite
    // connection to `get_or_init_memory_persistence()` / `get_or_init_memory_retrieval_engine()`.
    // This saves ~100-300ms of startup latency.
    {
        use crate::memory::memory_bridge::memory_base_path;
        let base = memory_base_path();
        let db_path = base.join("warm.db");
        let cold_path = base.join("cold");
        // The summarizer is still created eagerly (it's cheap — no IO),
        // and will be attached to the MemoryPersistence when it's lazily created.
        let summarizer = {
            use crate::memory::summarization::{MemorySummarizer, SummarizationConfig};
            let llm_agent = registry
                .get("summarizer")
                .or_else(|| registry.get("assistant"))
                .or_else(|| {
                    let names = registry.names();
                    names.first().and_then(|n| registry.get(n))
                });
            let mut s = MemorySummarizer::new(SummarizationConfig::default());
            if let Some(agent) = llm_agent {
                s = s.with_llm_agent(agent);
            }
            s
        };
        // Store the paths on the builder via the new method; the builder will
        // transfer them into the AcpServer's lazy_memory_persistence_params during build().
        builder = builder.with_lazy_memory_persistence_params(db_path, cold_path, Some(summarizer));
    }

    // Wire evolution loop — REMOVED: the injected bare EvolutionLoop had no
    // trigger sources / agent / alert manager, so the 60s spawn in
    // run_acp_server was a no-op. The fully-wired loop (default trigger
    // sources + alert manager + SelfEvolutionAgent + fusion bridge) runs in
    // start_background_tasks (log-20260730-18).

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

    // Wire permit exposure analyzer — REMOVED: PermitExposureAnalyzer was
    // injected into GovernanceServerDeps but scan_directory() has no
    // production caller (the security advisor's periodic scans cover
    // dependency + secret exposure; permit scanning was never activated).

    // Wire security advisor agent
    let _security_advisor = {
        use crate::security::security_advisor::{SecurityAdvisorAgent, SecurityAdvisorConfig};
        let advisor = Arc::new(SecurityAdvisorAgent::new(SecurityAdvisorConfig::default()));
        builder = builder.with_security_advisor(Arc::clone(&advisor));
        advisor
    };

    // ── Multimodal processor (F-GAP-66: attachment multimodal support) ────
    // Wire the default processor set so `detect_and_process_multimodal` can
    // process `data:` URIs, `file://` refs and `repo:` queries embedded in
    // user messages (the GUI sends attachments as inline `data:` / `file://`
    // refs). Construction is cheap and every sub-processor degrades to a
    // logged warning on failure — never a fatal error (see multimodal/mod.rs).
    // Document backends activate under the `sub-bus-multimodal` feature.
    {
        use crate::multimodal::MultimodalProcessor;
        builder = builder.with_multimodal_processor(MultimodalProcessor::new_with_all_processors());
    }

    // Wire security advisor agent
    let mut server = builder.build();
    // Pre-register all agents into the unified hyper-resilience engine so
    // breaker/health reports include every agent from startup (formerly done
    // by the removed `failure_prevention` in ServerBuilder::build, which is
    // sync; registration here is async). Idempotent per agent.
    for name in registry.names() {
        server.resilience.hyper_resilience.register_service(&name);
    }
    // Set fields that aren't available in ServerBuilder yet
    server.cache_deps.vector_config = vector_config;
    server.cache_deps.autotune = autotune;
    server.cache_deps.autotune_config = autotune_config;
    server.cache_deps.autotune_state_path = autotune_state_path;
    server.config_path = config_path;
    server.runtime_config = runtime_config;
    server.governance_deps.harness_bus = Some(harness_bus);
    server.governance_deps.capability_bus = Some(capability_bus);
    server.governance_deps.provenance_ledger = Some(provenance_ledger);
    server.governance_deps.rbac_enforcer = Some(rbac_enforcer);

    // Share the resolved skill registry with the capability bus ToolBus so
    // agent_tool_match / tool_bus_skills see the same imported/discovered
    // skills as the execution path (previously ToolBus held a second, empty
    // registry created in CapabilityBus::new). The server now owns the
    // capability bus, so the Arc is still unique here.
    {
        if let Some(cb) = server.governance_deps.capability_bus.as_mut() {
            if let Some(cb_mut) = Arc::get_mut(cb) {
                #[cfg(feature = "sub-bus-tool")]
                cb_mut
                    .tool_bus
                    .set_skill_registry(Arc::clone(&server.orchestration_deps.skill_registry));
            } else {
                tracing::warn!(
                    "capability_bus: Arc already shared before tool_bus skill-registry injection"
                );
            }
        }
    }

    // Register each agent as a monitored node in the fault-tolerance engine so
    // the 30s recovery cycle in start_background_tasks has a real node set to
    // check (previously the engine was created but no node was ever registered
    // — the recovery loop ran against an always-empty heartbeat table). Agents
    // are registered as monitored nodes; liveness is only evaluated for nodes
    // that have actually reported a heartbeat (has_reported), so idle agents
    // stay Online without spurious faults.
    if let Some(hb) = server.governance_deps.harness_bus.as_ref() {
        for name in registry.names() {
            if let Err(e) = hb.fault_tolerance.register_node(&name).await {
                // A duplicate registration (e.g. re-init) is benign.
                tracing::debug!(
                    target: "fault_tolerance",
                    node = %name,
                    "fault-tolerance node registration skipped: {e}"
                );
            }
        }
    }

    #[cfg(feature = "multi-users-server")]
    {
        server.rate_limiting.rate_limit_middleware = Some(Arc::new(
            crate::protocol::rate_limit::RateLimitMiddleware::new(
                crate::protocol::rate_limit::TenantRateLimit::default(),
            ),
        ));
        // Activate the distributed-memory transport (cross-node memory sync).
        // Peers come from GOON_MEMORY_PEERS or the local Hub discovery file
        // (BLUE72 P2 auto-discovery); with no peers the bus stays purely
        // local and no transport thread is spawned.
        if let Some(cb) = server.governance_deps.capability_bus.as_ref() {
            match cb.distributed_memory_bus.configure_cluster() {
                Ok(true) => {
                    tracing::info!(
                        "distributed memory transport started (peers from env or hub discovery)"
                    );
                }
                Ok(false) => {
                    tracing::info!(
                        "distributed memory transport: no peers configured — local only"
                    );
                }
                Err(e) => {
                    tracing::warn!("distributed memory transport failed to start: {e}");
                }
            }
        }
    }

    // Fix 6: Inject shared PuaEnforcementPlan into harness_bus evaluator
    if let Some(ref hb) = server.governance_deps.harness_bus {
        let mut engine = hb.evaluator.rule_engine.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("rule_engine lock poisoned, recovering");
            poisoned.into_inner()
        });
        engine.set_plan(server.governance_deps.pua_enforcement_plan.clone());
    }

    // ── Initialise SpawnAgentTool's global AgentRegistry reference ──
    // This must happen before any tool invocation; the registry is already
    // fully constructed at this point.
    crate::orchestration::tool_extended::spawn_agent::init_spawn_agent_registry(registry.clone());

    // ── BLUE70: Initialise SpawnAgentTool's CommunicationBus reference ──
    // Creates and wires the global CommunicationBus for agent tree-based
    // communication and observability.
    let communication_bus = Arc::new(crate::agents::communication::bus::CommunicationBus::new());
    crate::orchestration::tool_extended::spawn_agent::init_spawn_agent_communication_bus(
        communication_bus.clone(),
    );

    // ── BLUE71 §5: Initialise SpawnAgentTool's SpawnGuard budget ──
    crate::orchestration::tool_extended::spawn_agent::init_spawn_agent_budget();

    // ── GuardianHook: config-gated model-based tool review ──────────
    // Enabled via `guardian_enabled = true` + `guardian_agent = "agent_name"` in config.
    // When enabled, every tool call dispatched through the ToolRegistry (ACP
    // autonomy loop, MCP tools/call, ACP bridge, CLI) runs the async
    // pre-execute hook chain (`run_pre_async`), and the GuardianHook reviews
    // the call with the specified LLM agent before execution. A denied call
    // aborts execution (fail-fast). The reviewer operates at the ToolHook
    // level, not at the HarnessBus level — it does NOT intercept
    // "chat.execute" or other non-tool operations.
    {
        let tool_registry = crate::acp::r#impl::request::tools_pack::global_tool_registry();
        let guardian_enabled = app_config
            .as_ref()
            .map(|cfg| cfg.security.guardian_enabled)
            .unwrap_or(false);
        if guardian_enabled {
            let agent_name = app_config
                .as_ref()
                .map(|cfg| cfg.security.guardian_agent.as_str())
                .unwrap_or("");
            if !agent_name.is_empty() {
                if let Some(agent) = registry.get(agent_name) {
                    let reviewer = std::sync::Arc::new(
                        crate::governance::guardian::GuardianReviewer::new(agent, None),
                    );
                    let guardian_hook = std::sync::Arc::new(
                        crate::orchestration::tool::types::GuardianHook::new(reviewer),
                    );
                    tool_registry.hooks.register(guardian_hook);
                    tracing::info!(
                        "BLUE71: GuardianHook registered with agent '{}'",
                        agent_name
                    );
                } else {
                    tracing::warn!(
                        "BLUE71: guardian_agent '{}' not found in registry — GuardianHook disabled",
                        agent_name
                    );
                }
            } else {
                tracing::warn!(
                    "BLUE71: guardian_enabled=true but guardian_agent is empty — GuardianHook disabled"
                );
            }
        } else {
            tracing::debug!("BLUE71: GuardianHook disabled (guardian_enabled=false)");
        }
    }

    // BLUE57-B01: Inject cache backends into CapabilityBus MemoryBus before
    // wire_server() shares the Arc via init_intelligence_hub's global
    // GLOBAL_CAPABILITY_BUS clone — Arc::get_mut only succeeds while this is
    // the sole owner (the previous placement after wire_server() always hit
    // the "Arc already shared" warning and never wired the backends).
    // CapabilityBus does not implement Clone, so Arc::make_mut cannot be used.
    if let Some(ref mut cb_arc) = server.governance_deps.capability_bus {
        if let Some(_cb_mut) = Arc::get_mut(cb_arc) {
            #[cfg(feature = "sub-bus-memory")]
            _cb_mut.memory_bus.set_backends(
                Some(server.cache_deps.cache.response_cache.clone()),
                Some(server.cache_deps.cache.vector_store.clone()),
                None,
                Some(Some(Arc::clone(&server.cache_deps.cache.semantic_cache))),
            );
            tracing::info!("capability_bus: memory bus backends injected");
        } else {
            tracing::warn!("capability_bus: Arc already shared, cannot inject memory backends");
        }
    }

    // B51-26: Shared wiring extracted to wire_server()
    wire_server(&mut server, &registry).await;

    // GAP-B52-30: Register security advisor alert channel with the alert manager.
    // Forward security alerts to the observability pipeline.
    // Use spawn to circumvent the non-async context of new_acp_server.
    if let Some(ref advisor) = server.governance_deps.security_advisor {
        let (alert_tx, mut alert_rx) = tokio::sync::mpsc::channel(1024);
        let alert_tx_clone = alert_tx.clone();
        let advisor_clone = Arc::clone(advisor);
        tokio::spawn(async move {
            advisor_clone.register_ws_sender(alert_tx_clone).await;
        });
        let alert_manager = Arc::clone(&server.observability.alert_manager);
        tokio::spawn(async move {
            while let Some(security_alert) = alert_rx.recv().await {
                let mut mgr = alert_manager.lock().unwrap_or_else(|poisoned| {
                    tracing::warn!("alert_manager lock poisoned");
                    poisoned.into_inner()
                });
                // Map AlertSource to a string label
                let source_label = match &security_alert.source {
                    crate::security::security_advisor::AlertSource::DependencyVulnerability => {
                        "dependency"
                    }
                    crate::security::security_advisor::AlertSource::SecretExposure => "secret",
                    crate::security::security_advisor::AlertSource::PermitExposure => "permit",
                    crate::security::security_advisor::AlertSource::SecurityAdvisor => "advisor",
                    crate::security::security_advisor::AlertSource::UserReported => "user",
                };
                let severity = match security_alert.severity {
                    crate::security::vulnerability_scan::Severity::Critical => {
                        crate::shared::alert_severity::AlertSeverity::Critical
                    }
                    crate::security::vulnerability_scan::Severity::High => {
                        crate::shared::alert_severity::AlertSeverity::Critical
                    }
                    crate::security::vulnerability_scan::Severity::Medium => {
                        crate::shared::alert_severity::AlertSeverity::Warning
                    }
                    crate::security::vulnerability_scan::Severity::Low => {
                        crate::shared::alert_severity::AlertSeverity::Warning
                    }
                    crate::security::vulnerability_scan::Severity::Unknown => {
                        crate::shared::alert_severity::AlertSeverity::Info
                    }
                };
                // Security findings are discrete events, not numeric metrics:
                // report them directly so rule-name matching cannot skip them.
                mgr.report_direct(
                    &format!("security.{}", source_label),
                    format!("{}: {}", security_alert.title, security_alert.description),
                    severity,
                );
            }
        });
    }

    // ── BLUE48 Step 2: Pre-initialize SSE buffer pool at startup to
    // avoid first-request latency penalty from lazy initialization.
    // NOP in stdio mode — only relevant for HTTP/SSE transports.
    // pre_init_sse_buffer_pool() — deferred to first SSE use.

    // PERF-FIX: Removed init_memory_persistence_with_auto_migrate(None) from
    // the critical startup path.  This call created a *third* MemoryPersistence
    // instance (third SQLite connection + fs::create_dir_all + table/index DDL)
    // synchronously on the tokio worker thread.  The auto-migrate background
    // task is now started in start_background_tasks() using the server's
    // existing MemoryPersistence, after the HTTP port is already bound.
    // See: start_background_tasks() in src/acp/background.rs
    tracing::info!("memory bridge: auto-migration deferred to start_background_tasks");

    // BLUE48 Step 1: Initialize global embedding vector store for
    // semantic task classification in the Planner.
    if let Some(ref vs) = server.cache_deps.cache.vector_store {
        // GAP-B58-B12: Pre-initialize AgentMemoryBus with VectorStore
        // so retrieve_memories() uses vector similarity instead of linear scan.
        crate::memory::agent_memory_bus::init_agent_memory_bus_with_vector_store(
            Arc::clone(vs),
            None,
        );
    }

    server
}

/// Shared wiring applied after AcpServer construction (single call site in
/// `new_acp_server`). Extracted to keep the primary builder path's wiring in
/// one place.
async fn wire_server(server: &mut AcpServer, registry: &AgentRegistry) {
    // Create session manager if user auth is enabled
    if server.runtime_config.user_auth_enabled {
        use crate::acp::r#impl::session::{AuthConfig, SessionManager};
        let auth_config = AuthConfig::from(&server.runtime_config);
        server.session.session_manager =
            Some(Arc::new(SessionManager::with_auth_config(auth_config)));
    }

    if server.runtime_config.skills_enabled {
        server.register_skill(Arc::new(crate::orchestration::skill::EchoSkill));
        server.register_skill(Arc::new(
            crate::orchestration::skill::SkillCreatorSkill::new(
                server.orchestration_deps.skill_registry.clone(),
            ),
        ));

        // ── Register LLM-powered skills ─────────────────────────────────
        // These skills use the configured AI provider to perform semantic
        // operations (code review, classification, summarization, etc.).
        use crate::orchestration::skill::execution::SkillPolicy;
        let llm_skills: Vec<crate::orchestration::skill::PromptBasedSkill> = vec![
            crate::orchestration::skill::PromptBasedSkill {
                name: "semantic-diff".to_string(),
                description: "Analyze code changes semantically — understand what changed, why, and potential impacts".to_string(),
                prompt_template: include_str!("../../../../skills/semantic-diff/SKILL.md").to_string(),
                input_schema: [("input".to_string(), "string".to_string())].into(),
                timeout_secs: 120,
                max_retries: 1,
                disable_model_invocation: false,
                policy: Some(SkillPolicy {
                    allow_implicit_invocation: Some(false),
                    products: Vec::new(),
                }),
            },
            crate::orchestration::skill::PromptBasedSkill {
                name: "note-taking".to_string(),
                description: "Maintain structured working notes across sessions for project context and decisions".to_string(),
                prompt_template: include_str!("../../../../skills/note-taking/SKILL.md").to_string(),
                input_schema: [("input".to_string(), "string".to_string())].into(),
                timeout_secs: 60,
                max_retries: 1,
                disable_model_invocation: false,
                policy: Some(SkillPolicy {
                    allow_implicit_invocation: Some(true),
                    products: Vec::new(),
                }),
            },

            crate::orchestration::skill::PromptBasedSkill {
                name: "summarize-text".to_string(),
                description: "Summarize long text into concise, structured summaries".to_string(),
                prompt_template: include_str!("../../../../skills/summarize-text/SKILL.md").to_string(),
                input_schema: [("input".to_string(), "string".to_string())].into(),
                timeout_secs: 60,
                max_retries: 1,
                disable_model_invocation: false,
                policy: Some(SkillPolicy {
                    allow_implicit_invocation: Some(true),
                    products: Vec::new(),
                }),
            },
            crate::orchestration::skill::PromptBasedSkill {
                name: "translate-text".to_string(),
                description: "Translate text between languages with natural-sounding results".to_string(),
                prompt_template: include_str!("../../../../skills/translate-text/SKILL.md").to_string(),
                input_schema: [("input".to_string(), "string".to_string())].into(),
                timeout_secs: 60,
                max_retries: 1,
                disable_model_invocation: false,
                policy: Some(SkillPolicy {
                    allow_implicit_invocation: Some(true),
                    products: Vec::new(),
                }),
            },
            crate::orchestration::skill::PromptBasedSkill {
                name: "code-review".to_string(),
                description: "Two-mode code review — diff review (git PR/branch changes) and snippet review (static code quality analysis with language-aware scoring)".to_string(),
                prompt_template: include_str!("../../../../skills/code-review/SKILL.md").to_string(),
                input_schema: [("input".to_string(), "string".to_string())].into(),
                timeout_secs: 120,
                max_retries: 1,
                disable_model_invocation: false,
                policy: Some(SkillPolicy {
                    allow_implicit_invocation: Some(false),
                    products: Vec::new(),
                }),
            },
            crate::orchestration::skill::PromptBasedSkill {
                name: "classify-text".to_string(),
                description: "Classify text into predefined categories or generate semantic embeddings/vector representations for similarity search".to_string(),
                prompt_template: include_str!("../../../../skills/classify-text/SKILL.md").to_string(),
                input_schema: [("input".to_string(), "string".to_string())].into(),
                timeout_secs: 60,
                max_retries: 1,
                disable_model_invocation: false,
                policy: Some(SkillPolicy {
                    allow_implicit_invocation: Some(true),
                    products: Vec::new(),
                }),
            },
        ];

        for skill in llm_skills {
            server.register_skill(Arc::new(skill));
        }

        // Wire the prompt skill agent so PromptBasedSkill can call a real LLM.
        // Uses the first available agent from the registry (preferring "primary").
        let agent_names = registry.names();
        let prompt_agent = agent_names
            .iter()
            .find(|n| n.contains("primary"))
            .or_else(|| agent_names.first())
            .and_then(|name| registry.get(name));

        if let Some(agent) = prompt_agent {
            use crate::orchestration::skill::ChatBasedSkillAgent;
            let skill_agent = Arc::new(ChatBasedSkillAgent::new(agent));
            crate::orchestration::skill::set_prompt_skill_agent(skill_agent);
            info!("prompt skill agent wired for LLM-based skill execution");
        } else {
            info!("no agent available for prompt skill execution — skills will use fallback mode");
        }
    }

    // Wire the new modules' state from CapabilityBus into the server's
    // standalone fields so process_chat_request can access them directly.
    server.registries.schema_registry = Arc::clone(
        &server
            .governance_deps
            .capability_bus
            .as_ref()
            .map(|cb| Arc::clone(&cb.schema_registry))
            .unwrap_or_default(),
    );
    server.rate_limiting.tenant_budget = Arc::clone(
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
        let mut budget = server
            .rate_limiting
            .tenant_budget
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("tenant_budget lock poisoned in wire_server");
                poisoned.into_inner()
            });
        budget.auto_provision_default(&server.runtime_config);
    }

    // Wire the token cache into the agent registry so that all
    // agents returned by registry.get() are automatically wrapped
    // with CachedAgentWrapper.
    registry.set_token_cache(Some(Arc::clone(&server.cache_deps.cache.token_cache)));

    // BLUE48 Step 19: Initialize intelligence hub at startup so consensus
    // voting, rationalization, and audit are wired into the request path.
    // Single call replaces old init_intel_hub() + init_intel_voters() pattern.
    // enable_delphi_debate is honored from the runtime config so the
    // previously dead config field is now wired.
    crate::intelligence::hub::init_intelligence_hub(
        server.runtime_config.enable_delphi_debate,
        server.governance_deps.capability_bus.clone(),
    );

    // ── Memory health monitor (F-GAP-49 activation) ───────────────
    // Start background memory pressure monitoring. Periodically queries
    // system memory, logs warnings on low/critical conditions, and
    // evaluates AlertManager rules for threshold-based alerting.
    // query_system_memory does blocking I/O (subprocess spawn on macOS,
    // /proc reads on Linux), so run the startup one-shot on a blocking
    // thread — same pattern as the periodic 30s loop in background.rs.
    tokio::task::spawn_blocking(crate::observability::memory_health::start_memory_monitor)
        .await
        .ok();
    info!("memory health check completed (one-shot at startup)");

    // ── Wire security subsystems (GAP-B52, S-FIX3) ────────────────────
    crate::security::wire_cert_monitor(&server.runtime_config);

    // ── SemanticResponseCache background cleanup ───────────────
    // NOTE: the TokenMultiLevelCache background-cleanup spawn was removed —
    // ttl_ms defaults to 0 and set_ttl_ms has no callers, so the 60s loop
    // ticked forever doing nothing. Lazy lookup-time eviction still runs.
    // Start periodic eviction of expired semantic cache entries.
    // Without this, expired entries accumulate until lazy get()-time eviction.
    if let Ok(mut guard) = server.cache_deps.cache.semantic_cache.write() {
        guard.start_background_cleanup();
    }
    info!("semantic_cache background cleanup started");
}
