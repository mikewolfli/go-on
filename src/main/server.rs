use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use tracing::{debug, error, info, warn};

use crate::agent::AgentRegistry;
use crate::config::{validate_runtime_readiness, AppConfig};
use crate::core::config_validation;
use crate::core::setup;
use crate::i18n::runtime::{t, tf};
use crate::intelligence::capability_graph::CapabilityGraph;
use crate::protocol::access_mode::resolve_access_selection;
use crate::reinforcement::{
    build_runtime_healthcheck_report, build_runtime_healthcheck_report_with_config,
    build_task_plan, persist_runtime_healthcheck, persist_task_plan, run_action_check,
    ActionCheckKind, ArtifactLedger,
};
use crate::setup::{
    add_local_model, apply_recommended_to_config, parse_secret_action, parse_secret_mode,
    parse_setup_level, parse_setup_profile, LocalModelOptions, SetupOptions,
};

use super::cli::{validate_cli_protocol_mode, Cli};
use super::report::{emit_config_warnings, print_completeness_report, print_runtime_status};

/// Start the server with the given configuration and CLI options.
///
/// `skill_registry` — an optional pre-populated skill registry from bootstrap,
/// avoiding a redundant scan of `~/.agents/skills/` on server startup.
pub(crate) async fn start_server(
    config: Arc<AppConfig>,
    cli: &Cli,
    config_path: &Path,
    skill_registry: Option<Arc<std::sync::RwLock<crate::orchestration::skill::SkillRegistry>>>,
) -> Result<()> {
    // Create HTTP client with timeout
    // Use HTTP/1.1 only to avoid HTTP/2 'unknown stream error' issues with
    // some AI provider APIs (e.g., DeepSeek). HTTP/2 multiplexing can cause
    // intermittent stream resets on long-lived SSE connections.
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .http1_only()
        .build()?;

    // ── Initialize Layer 3 URL policy config ───────────────────────────
    // Load the UrlPolicyConfig from SecurityConfig and inject it into the
    // http_request tool's global state for runtime sandboxing.
    crate::orchestration::tool_extended::http::init_url_policy(config.security.url_policy.clone());

    // Initialize capability graph for agent capability-based routing
    let capability_graph = Arc::new(Mutex::new(CapabilityGraph::new()));

    // Initialize agent registry and flow manager
    let registry = Arc::new(AgentRegistry::from_config(
        Arc::clone(&config),
        http_client.clone(),
        Arc::clone(&capability_graph),
    )?);
    // FlowManager is initialized inside dispatch_server via transport_factory::flow_manager()
    // with access to the parsed AppConfig. No need to create it here.

    let agent_names = registry.names();
    info!("Registered {} agents: {:?}", agent_names.len(), agent_names);

    // Note: the first available LLM agent is now injected into the
    // ContinuousLearningCenter inside new_acp_server()/ServerBuilder::build
    // (server_builder.rs) — the previous throwaway cl_agent_handle pipeline
    // in main/ was never read by the center and has been removed.

    // ── Memory-aware resource limiting ───────────────────────────────────────
    // Adjust cache/vector limits based on available system memory.
    // This prevents OOM kills on memory-constrained systems.
    let user_cache_max = config.cache.as_ref().map(|c| c.max_entries);
    let user_vector_max = config.vector.as_ref().map(|v| v.max_entries);

    let (safe_cache_max, safe_vector_max, _safe_inflight_max) =
        crate::observability::memory_health::estimate_safe_limits(
            user_cache_max,
            user_vector_max,
            None,
            cli.low_memory,
        );

    // Clone and adjust cache config with safe limits
    let mut adjusted_cache_cfg = config.cache.clone();
    if let Some(ref mut cache_cfg) = adjusted_cache_cfg {
        if cache_cfg.max_entries > safe_cache_max {
            info!(
                "reducing cache max_entries from {} to {} (memory-aware)",
                cache_cfg.max_entries, safe_cache_max
            );
            cache_cfg.max_entries = safe_cache_max;
        }
    }

    // Clone and adjust vector config with safe limits
    let mut adjusted_vector_cfg = config.vector.clone();
    if let Some(ref mut vector_cfg) = adjusted_vector_cfg {
        if vector_cfg.max_entries > safe_vector_max {
            info!(
                "reducing vector max_entries from {} to {} (memory-aware)",
                vector_cfg.max_entries, safe_vector_max
            );
            vector_cfg.max_entries = safe_vector_max;
        }
    }

    // Log the applied limits
    info!(
        "memory-aware limits applied: cache_max={}, vector_max={}",
        safe_cache_max, safe_vector_max
    );

    // ── Security wiring (GAP-B52) ──────────────────────────────────────────
    // DEFERRED to wire_server() where runtime_config is available and
    // all security subsystems are initialized once, not twice.
    // See: src/acp/impl/runtime/server_builder.rs wire_server() L845-847.
    // The security init was previously duplicated in both start_server()
    // and wire_server(). This saves ~5ms of startup time.
    //
    // let _secret_rotation_handle = start_secret_rotation_if_configured(rt);
    // wire_cert_monitor(rt); — REMOVED (duplicate, handled in wire_server)

    // Initialize StartupContext (load project context once per process).
    // Honor the user's `[startup_context] enabled=false` setting instead of
    // hard-coding it on.
    let startup_cfg = config.startup_context.clone().unwrap_or_default();
    if startup_cfg.enabled {
        tokio::spawn(async move {
            if let Err(e) = crate::orchestration::startup_context::load(&startup_cfg).await {
                warn!("startup context loading failed: {}", e);
            }
        });
    }

    // persist_cache defaults to true (CacheConfig::persist_enabled) so existing
    // [cache] enabled=true deployments get the token-cache L3 durable layer.
    // Read it before `adjusted_cache_cfg` is moved into initialize_cache below.
    let persist_cache = adjusted_cache_cfg
        .as_ref()
        .map(|c| c.persist_enabled)
        .unwrap_or(true);

    let (cache, vector_store, (autotune_state, autotune_config, autotune_state_path)) = tokio::try_join!(
        crate::acp::transport_factory::initialize_cache(config_path, adjusted_cache_cfg),
        crate::acp::transport_factory::initialize_vector_store(config_path, adjusted_vector_cfg),
        crate::acp::transport_factory::initialize_autotune(config_path, config.autotune.clone()),
    )?;

    let ledger = ArtifactLedger::new(Some(config_path));

    // The CapabilityBus (14-Bus system with all cognitive modules) is owned
    // and initialized inside the ACP runtime (new_acp_server) where it is
    // genuinely wired into sense→decide→act→feedback→evolve lifecycle
    // and exposed via the /health endpoint. No orphaned instance here.
    info!("CapabilityBus lifecycle managed by ACP runtime");

    if let Some(task) = cli.plan_task.as_deref() {
        let plan = build_task_plan(task);
        let path = persist_task_plan(&ledger, &plan)?;
        println!(
            "persisted task plan to {} (sub_agent_recommended={})",
            path.display(),
            plan.sub_agent_recommended
        );
        return Ok(());
    }

    if cli.healthcheck {
        let report = build_runtime_healthcheck_report(
            Some(config_path),
            cache.as_deref(),
            vector_store.as_deref(),
        )
        .await?;
        let path = persist_runtime_healthcheck(&ledger, &report)?;
        println!(
            "healthcheck: {:?} -> {}",
            report.overall_status,
            path.display()
        );
        return Ok(());
    }

    if let Some(raw_kind) = cli.action_check.as_deref() {
        let kind = ActionCheckKind::parse(raw_kind).ok_or_else(|| {
            anyhow::anyhow!(
                "invalid --action-check value '{}'; expected one of: all, spec, qa, retest, final",
                raw_kind
            )
        })?;
        let report = run_action_check(&ledger, kind)?;
        println!(
            "action check {}: {:?} (ok={})",
            raw_kind, report.overall_status, report.ok
        );
        return Ok(());
    }

    // Get runtime configuration.
    // protocol.mode is already synced into runtime.protocol_mode during
    // AppConfig::load() (parser.rs), so no manual TOML re-read is needed.
    let mut runtime_config = config.runtime.clone().unwrap_or_default();

    // CLI override has higher priority than config file protocol section.
    if let Some(mode) = validate_cli_protocol_mode(cli.protocol_mode.as_deref())? {
        runtime_config.protocol_mode = Some(mode);
    }

    let acp_http_bind = cli
        .acp_http_bind
        .clone()
        .or_else(|| runtime_config.acp_http_bind_addr.clone());
    let access_selection = resolve_access_selection(
        runtime_config.protocol_mode.as_deref(),
        acp_http_bind.as_deref(),
    );
    runtime_config.protocol_mode = Some(access_selection.configured_mode.clone());

    let dispatch_mode = if access_selection.configured_mode == "adaptive" {
        match access_selection.startup_transport {
            crate::protocol::access_mode::TransportMode::Http => "acp_http",
            crate::protocol::access_mode::TransportMode::Stdio => "acp_stdio",
        }
    } else {
        access_selection.configured_mode.as_str()
    };

    // Protocol mode has been fully resolved by resolve_access_selection above.
    // Log the final dispatch mode for observability.
    info!("dispatch mode resolved: {}", dispatch_mode);

    // Delegate to the transport factory for protocol-mode-specific server construction
    // P0 optimization: pass pre-loaded config to avoid double-load in flow_manager().
    crate::acp::transport_factory::dispatch_server(
        registry,
        cache,
        vector_store,
        config_path,
        runtime_config,
        dispatch_mode,
        &acp_http_bind.unwrap_or_else(|| "127.0.0.1:8090".to_string()),
        autotune_state,
        autotune_config,
        autotune_state_path,
        skill_registry,
        Some(Arc::clone(&config)),
        persist_cache,
    )
    .await
}

/// Handle terminal chat mode (--chat flag).
/// If agents are configured, starts interactive chat. Otherwise redirects to setup.
pub(crate) async fn handle_chat_mode(
    config: Arc<AppConfig>,
    _cli: &Cli,
    config_path: &Path,
    skill_registry: Option<Arc<std::sync::RwLock<crate::orchestration::skill::SkillRegistry>>>,
) -> Result<()> {
    if config.agents().is_empty() {
        eprintln!("{}", t("error.no_providers_configured"));
        eprintln!("{}", t("error.setup_wizard_first"));
        eprintln!("  go-on -c {} --setup", config_path.display());
        return Ok(());
    }

    // Chat mode uses ACP stdio — the protocol negotiator is not needed
    // here because the terminal chat bypasses the ACP transport layer.
    debug!("chat mode: starting terminal chat");

    crate::cli::chat::run_terminal_chat(config, skill_registry, config_path).await
}

/// Handle secret management commands, local model setup, recommended config, setup wizard, and AI onboarding.
///
/// Returns `true` if a command was handled and `run()` should return early.
///
/// # Sync boundary
///
/// This function performs synchronous I/O throughout: keyring access
/// (`setup::run_secret_command`), config file writes, and interactive
/// stdin/stdout prompts (setup wizard / `--add-model` URL prompt). The caller
/// in `main/mod.rs::run()` (async context) therefore invokes it inside
/// `tokio::task::spawn_blocking`. Future async callers must do the same.
pub(crate) fn handle_secret_commands(cli: &Cli, config_path: &Path) -> Result<bool> {
    // Handle secret management commands
    if let Some(action) = cli.secret.as_deref() {
        let action = parse_secret_action(action)?;
        setup::run_secret_command(
            action,
            cli.secret_name.as_deref(),
            cli.secret_value.as_deref(),
        )?;
        return Ok(true);
    }

    if cli.add_local_model {
        add_local_model(
            config_path,
            LocalModelOptions {
                name: cli.local_model_name.clone(),
                url: cli.local_model_url.clone(),
                agent_type: cli.local_model_type.clone(),
                model: cli.local_model_model.clone(),
                api_key_env: cli.local_model_api_key_env.clone(),
                secret_key_env: cli.local_model_secret_key_env.clone(),
                apply_to_phases: !cli.local_model_register_only,
            },
        )?;
        return Ok(true);
    }

    if cli.apply_recommended {
        apply_recommended_to_config(config_path)?;
        return Ok(true);
    }

    // Handle setup wizard
    if cli.setup {
        let options = SetupOptions {
            profile: cli
                .setup_profile
                .as_deref()
                .map(parse_setup_profile)
                .transpose()?,
            level: cli
                .setup_level
                .as_deref()
                .map(parse_setup_level)
                .transpose()?,
            secret_mode: cli
                .setup_secrets
                .as_deref()
                .map(parse_secret_mode)
                .transpose()?,
            force: cli.force,
            prompt_for_secrets: cli.setup_profile.is_none() && cli.setup_secrets.is_none(),
        };
        setup::run_setup_with_options(config_path, options)?;
        return Ok(true);
    }

    Ok(false)
}

/// Load and validate configuration, then handle validation-only modes
/// (--validate-config; --diagnose is handled earlier in main/mod.rs).
///
/// Returns `Some(config)` if validation passed and the server should start,
/// or `None` if a validation-only command was handled and `run()` should return.
pub(crate) async fn handle_validation_mode(
    cli: &Cli,
    config_path: &Path,
) -> Result<Option<Arc<AppConfig>>> {
    // If the config is missing or blank, write the non-AI bootstrap config.
    // Single helper shared with the config parser (defaults::ensure_bootstrap_config)
    // so the "write default config" behavior lives in exactly one place.
    crate::config::defaults::ensure_bootstrap_config(config_path)?;

    // Load and validate configuration
    info!("loading config from {}", config_path.display());
    let config = Arc::new(AppConfig::load_uncached(config_path)?);

    // Perform enhanced configuration validation (reuses the already-loaded
    // config — avoids a second AppConfig::load from disk on every startup).
    // This is the single report/analysis engine used by --validate-config.
    let validation_result =
        config_validation::validate_config_with(config_path, config.as_ref().clone())?;

    // Runtime readiness: hard structural gate (config.validate()) plus
    // env-secret/strict-mode checks and the legacy health report. The two
    // engines are complementary — ConfigValidator reports; this one enforces
    // the hard gate (also used by the ACP config.reload endpoint) — and both
    // now share the process-global I18N (no duplicate language-file reads).
    let health_report = validate_runtime_readiness(config_path, &config)?;
    emit_config_warnings(&health_report.warnings, cli.validate_config);

    // If only validating config, exit after validation
    if cli.validate_config {
        // Enhanced validation report
        let validation_report =
            config_validation::ConfigValidator::new(config_path, config.as_ref().clone())
                .generate_report(&validation_result);

        info!("configuration validation completed\n{}", validation_report);

        // Legacy report for compatibility
        info!(
            "legacy health report: score={}/100 warnings={} (critical={}, warn={}, info={})",
            health_report.score,
            health_report.total,
            health_report.critical_count,
            health_report.warn_count,
            health_report.info_count
        );

        println!("{}", validation_report);
        println!(
            "{}",
            tf(
                "ui.legacy_health_score",
                &[
                    ("score", &health_report.score.to_string()),
                    ("critical", &health_report.critical_count.to_string()),
                    ("warn", &health_report.warn_count.to_string()),
                    ("info", &health_report.info_count.to_string()),
                ]
            )
        );

        if !validation_result.is_valid {
            error!("Configuration validation failed");
            return Ok(None);
        }

        return Ok(None);
    }

    if cli.status {
        // Reuse the already-loaded config (loaded via `load_uncached` above)
        // instead of letting the report builder re-load + re-parse the file
        // and write a fresh mtime-cache entry.
        let report =
            build_runtime_healthcheck_report_with_config(config_path, &config, None, None).await?;
        print_runtime_status(config_path, &report);
        print_completeness_report(&config, &report);
        return Ok(None);
    }

    // Check if configuration is valid before proceeding.
    // `is_valid` is defined as "no critical errors" (see ValidationResult::validate),
    // so an invalid result always carries critical errors — the former
    // `has_errors()` (non-critical) and "unknown reasons" branches were dead.
    if !validation_result.is_valid {
        error!("Configuration validation failed. Cannot start server.");
        let report = config_validation::ConfigValidator::new(config_path, config.as_ref().clone())
            .generate_report(&validation_result);
        error!("Validation report:\n{}", report);

        error!("Critical errors detected:");
        for err in validation_result.critical_errors() {
            error!("  [CRITICAL] {}: {}", err.section, err.message);
        }
        anyhow::bail!("Configuration has critical errors that must be fixed");
    } else {
        // Configuration is valid, log warnings and recommendations if any
        if !validation_result.warnings.is_empty() {
            warn!("Configuration warnings detected:");
            for warning in &validation_result.warnings {
                warn!("  [WARNING] {}: {}", warning.section, warning.message);
            }
        }

        if !validation_result.recommendations.is_empty() {
            info!("Configuration recommendations:");
            for recommendation in &validation_result.recommendations {
                info!(
                    "  [RECOMMENDATION] {:?}: {}",
                    recommendation.category, recommendation.message
                );
            }
        }
    }

    Ok(Some(config))
}
