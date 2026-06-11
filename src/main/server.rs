use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tracing::{debug, error, info, warn};

use crate::agent::AgentRegistry;
use crate::config::{validate_runtime_readiness, AppConfig};
use crate::core::config_validation;
use crate::core::setup;
use crate::i18n::runtime::{t, tf};
use crate::intelligence::capability_graph::CapabilityGraph;
use crate::intelligence::continuous_learning::AgentInjector;
use crate::protocol::access_mode::resolve_access_selection;
use crate::protocol::negotiator::{ProtocolMode as NegProtocolMode, ProtocolNegotiator};
use crate::reinforcement::{
    build_runtime_healthcheck_report, build_task_plan, persist_runtime_healthcheck,
    persist_task_plan, run_action_check, ActionCheckKind, ArtifactLedger,
};
use crate::security::{
    start_secret_rotation_if_configured, wire_cert_monitor, wire_content_safety,
    wire_prompt_injection,
};
use crate::setup::{
    add_local_model, apply_recommended_to_config, parse_secret_action, parse_secret_mode,
    parse_setup_level, parse_setup_profile, LocalModelOptions, SetupOptions,
};

use super::cli::{validate_cli_protocol_mode, Cli};
use super::report::{emit_config_warnings, print_completeness_report, print_runtime_status};

/// Start the server with the given configuration and CLI options.
///
/// `cl_agent_handle` — when `Some`, the first available agent from the registry
/// is injected into the `ContinuousLearningCenter` for LLM-based semantic
/// distillation (replacing the TF-IDF fallback).  Pass `None` or an empty handle
/// to skip injection.
pub(crate) async fn start_server(
    config: Arc<AppConfig>,
    cli: &Cli,
    config_path: &Path,
    cl_agent_handle: Option<AgentInjector>,
) -> Result<()> {
    // Create HTTP client with timeout
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

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

    // Display agent vendor information
    let agents_by_vendor = registry.agents_by_vendor();
    info!("Agents organized by vendor category:");
    for (category, agents) in &agents_by_vendor {
        info!("  {:?}: {} agents", category, agents.len());
        for agent in agents {
            info!("    - {}", agent);
        }
    }

    // ── Inject the first available agent into ContinuousLearningCenter ──
    // This enables true LLM-based semantic distillation during review cycles
    // (otherwise the center falls back to TF-IDF keyword extraction).
    if let Some(handle) = cl_agent_handle {
        if let Some(first_name) = registry.names().first().cloned() {
            if let Some(agent) = registry.get(&first_name) {
                let mut guard = handle.lock().unwrap_or_else(|e| e.into_inner());
                info!(
                    "ContinuousLearningCenter: injecting agent '{}' for LLM-based semantic distillation",
                    first_name
                );
                *guard = Some(agent);
            }
        }
    }

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
    // Wire security components into the server startup path.
    // Only call wire functions when runtime config is available.
    if let Some(ref rt) = config.runtime {
        wire_content_safety(rt);
        wire_prompt_injection(rt);
        let _secret_rotation_handle = start_secret_rotation_if_configured(rt);
        wire_cert_monitor(rt);
    }

    // Initialize StartupContext (load project context once per process)
    let startup_cfg = crate::orchestration::startup_context::StartupContextConfig {
        enabled: true,
        ..Default::default()
    };
    tokio::spawn(async move {
        if let Err(e) = crate::orchestration::startup_context::load(&startup_cfg).await {
            warn!("startup context loading failed: {}", e);
        }
    });

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
        )?;
        let path = persist_runtime_healthcheck(&ledger, &report)?;
        println!(
            "healthcheck: {:?} -> {}",
            report.overall_status,
            path.display()
        );
        return Ok(());
    }

    if cli.status {
        let report = build_runtime_healthcheck_report(
            Some(config_path),
            cache.as_deref(),
            vector_store.as_deref(),
        )?;
        print_runtime_status(config_path, &report);
        print_completeness_report(config.as_ref(), &report);
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

    // Get runtime configuration
    let mut runtime_config = config.runtime.clone().unwrap_or_default();
    // Read [protocol].mode (supports 5 options with adaptive default)
    if let Ok(config_str) = std::fs::read_to_string(config_path) {
        if let Ok(toml_value) = config_str.parse::<toml::Value>() {
            if let Some(protocol_section) = toml_value.get("protocol") {
                if let Some(mode) = protocol_section.get("mode").and_then(|v| v.as_str()) {
                    runtime_config.protocol_mode = Some(mode.to_string());
                }
            }
        }
    }

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

    // ── ProtocolNegotiator ───────────────────────────────────────────
    // Create negotiator with the resolved mode and log the negotiated result.
    let negotiator_mode = NegProtocolMode::from_str(dispatch_mode)
        .unwrap_or_else(|e| panic!("fatal: invalid dispatch mode '{}': {:?}", dispatch_mode, e));
    let negotiator = ProtocolNegotiator::new(negotiator_mode);
    let negotiated = negotiator.negotiate(None, None);
    info!(
        "protocol negotiated: mode={}, version={}, auto_detected={}",
        negotiated.mode, negotiated.version, negotiated.auto_detected
    );

    // Delegate to the transport factory for protocol-mode-specific server construction
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
        http_client,
    )
    .await
}

/// Handle terminal chat mode (--chat flag).
/// If agents are configured, starts interactive chat. Otherwise redirects to setup.
pub(crate) async fn handle_chat_mode(
    config: Arc<AppConfig>,
    _cli: &Cli,
    config_path: &Path,
) -> Result<()> {
    if config.agents().is_empty() {
        eprintln!("{}", t("error.no_providers_configured"));
        eprintln!("{}", t("error.setup_wizard_first"));
        eprintln!("  go-on -c {} --setup", config_path.display());
        return Ok(());
    }

    // ── ProtocolNegotiator: chat mode uses ACP stdio ────────────────
    let negotiator = ProtocolNegotiator::new(NegProtocolMode::AcpStdio);
    let negotiated = negotiator.negotiate(None, None);
    debug!(
        "chat mode protocol: mode={}, version={}",
        negotiated.mode, negotiated.version
    );

    crate::cli::chat::run_terminal_chat(config).await
}

/// Handle secret management commands, local model setup, recommended config, setup wizard, and AI onboarding.
///
/// Returns `true` if a command was handled and `run()` should return early.
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

/// Load and validate configuration, then handle validation-only modes (--validate-config, --diagnose).
///
/// Returns `Some(config)` if validation passed and the server should start,
/// or `None` if a validation-only command was handled and `run()` should return.
pub(crate) fn handle_validation_mode(
    cli: &Cli,
    config_path: &Path,
) -> Result<Option<Arc<AppConfig>>> {
    // If config doesn't exist, create a minimal bootstrap config
    if !config_path.exists() {
        info!(
            "config not found at {}, creating bootstrap",
            config_path.display()
        );
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create config directory: {}", parent.display())
            })?;
        }
        let bootstrap = crate::config::default_non_ai_config_toml();
        std::fs::write(config_path, &bootstrap).with_context(|| {
            format!(
                "failed to write bootstrap config to {}",
                config_path.display()
            )
        })?;
    }

    // Load and validate configuration
    info!("loading config from {}", config_path.display());
    let config = Arc::new(AppConfig::load(config_path)?);

    // Perform enhanced configuration validation
    let validation_result = config_validation::validate_config_file(config_path)?;

    // Also run legacy validation for compatibility
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

    if cli.diagnose {
        let report = build_runtime_healthcheck_report(Some(config_path), None, None)?;
        let error_count = report
            .components
            .iter()
            .filter(|item| item.status == crate::reinforcement::CheckStatus::Error)
            .count();
        let warn_count = report
            .components
            .iter()
            .filter(|item| item.status == crate::reinforcement::CheckStatus::Warn)
            .count();
        let healthy_count = report
            .components
            .iter()
            .filter(|item| {
                item.status == crate::reinforcement::CheckStatus::Healthy
                    || item.status == crate::reinforcement::CheckStatus::Skipped
            })
            .count();
        println!("=== go-on diagnose ===");
        println!("config: {}", config_path.display());
        println!("overall: {:?}", report.overall_status);
        println!(
            "summary: error={} warn={} healthy={}",
            error_count, warn_count, healthy_count
        );
        if error_count > 0 {
            println!(
                "suggestion: run `go-on --validate-config -c {}`",
                config_path.display()
            );
        } else {
            println!("suggestion: runtime baseline looks healthy");
        }
        return Ok(None);
    }

    // Check if configuration is valid before proceeding
    if !validation_result.is_valid {
        error!("Configuration validation failed. Cannot start server.");
        let report = config_validation::ConfigValidator::new(config_path, config.as_ref().clone())
            .generate_report(&validation_result);
        error!("Validation report:\n{}", report);

        // Provide more detailed error information
        if validation_result.has_critical_errors() {
            error!("Critical errors detected:");
            for err in validation_result.critical_errors() {
                error!("  [CRITICAL] {}: {}", err.section, err.message);
            }
            anyhow::bail!("Configuration has critical errors that must be fixed");
        } else if validation_result.has_errors() {
            error!("Configuration errors detected (non-critical):");
            for err in validation_result.regular_errors() {
                error!("  [ERROR] {}: {}", err.section, err.message);
            }
            warn!("Configuration has errors but may still work. Consider fixing them.");
        } else {
            anyhow::bail!("Configuration validation failed for unknown reasons");
        }
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
