//! Main entry point for the go-on ACP proxy
//!
//! This module handles command-line arguments, configuration loading, and server initialization.
//!
//! # Features
//!
//! - **Structured Logging**: Uses `tracing` for comprehensive observability
//! - **Performance Monitoring**: Integrated performance metrics and profiling
//! - **Configuration Validation**: Advanced validation with dependency analysis
//! - **Agent Management**: Support for multiple AI agent vendors
//! - **Error Handling**: Comprehensive error handling with panic recovery
//! - **Internationalization**: Multi-language support with hot-reloading
//!
//! # Usage Examples
//!
//! ```bash
//! # Start server with default configuration
//! go-on
//!
//! # Start server with custom configuration
//! go-on --config /path/to/config.toml
//!
//! # Validate configuration without starting server
//! go-on --validate-config
//!
//! # Enable verbose logging
//! go-on --verbose
//!
//! # Specify phase to run
//! go-on --phase review
//! ```
//!
//! # Architecture Overview
//!
//! The application follows a modular architecture:
//!
//! 1. **Configuration Layer**: `config.rs`, `config_validation.rs`
//! 2. **Telemetry Layer**: `telemetry.rs`, `telemetry_enhanced.rs`
//! 3. **Performance Layer**: `performance.rs`, `observability.rs`
//! 4. **Agent Layer**: `agent.rs`, `agents/` directory
//! 5. **Protocol Layer**: `acp.rs`, `rpc_protocol.rs`
//! 6. **Business Logic**: `flow.rs`, `task_router.rs`, `orchestrator.rs`
//!
//! Each layer has clear responsibilities and well-defined interfaces.

#![allow(dead_code)]

mod acp;
mod agents;
mod core;
mod governance;
mod i18n;
mod intelligence;
mod mcp;
mod memory;
mod observability;
mod optimization;
mod orchestration;
mod protocol;

pub use crate::agents::agent;
pub use crate::core::config;
pub use crate::core::config_validation;
pub use crate::core::context;
pub use crate::core::error;
pub use crate::core::setup;
pub use crate::governance::audit;
pub use crate::governance::hardening;
pub use crate::governance::pua;
pub use crate::governance::review_controls;
pub use crate::governance::runtime_controls;
pub use crate::i18n::runtime;
pub use crate::i18n::watcher as i18n_watcher;
pub use crate::intelligence::adaptive_selector;
pub use crate::intelligence::advanced_modules;
pub use crate::intelligence::evaluation;
pub use crate::intelligence::model_selector;
pub use crate::intelligence::promotion;
pub use crate::intelligence::quality_models;
pub use crate::intelligence::reinforcement;
pub use crate::intelligence::verification;
pub use crate::memory::cache;
pub use crate::memory::memory as memory_module;
pub use crate::memory::memory_response_cache;
pub use crate::memory::vector;
pub use crate::observability::observability as observability_module;
pub use crate::observability::performance;
pub use crate::observability::telemetry;
pub use crate::observability::telemetry_enhanced;
pub use crate::optimization::cost_optimizer;
pub use crate::optimization::failure_prevention;
pub use crate::optimization::reliability_optimizer;
pub use crate::optimization::speed_optimizer;
pub use crate::optimization::workflow_optimizer;
pub use crate::orchestration::flow;
pub use crate::orchestration::flow_with_models;
pub use crate::orchestration::graph;
pub use crate::orchestration::mode;
pub use crate::orchestration::orchestrator;
pub use crate::orchestration::roles;
pub use crate::orchestration::task_decomposer;
pub use crate::orchestration::task_graph;
pub use crate::orchestration::task_router;
pub use crate::orchestration::tool;
pub use crate::protocol::mcp_server;
pub use crate::protocol::rpc_protocol;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use tracing::{error, info, warn};

use crate::acp::r#impl::{new_acp_server, run_acp_server};
use crate::agent::AgentRegistry;
use crate::cache::ResponseCache;
use crate::config::{
    validate_runtime_readiness, AppConfig, AutoTuneState, ConfigWarning, RuntimeConfig,
};
use crate::flow::FlowManager;
use crate::i18n::runtime::{init_i18n, tf};
use crate::reinforcement::{
    build_runtime_healthcheck_report, build_task_plan, persist_runtime_healthcheck,
    persist_task_plan, run_action_check, ActionCheckKind, ArtifactLedger,
};
use crate::setup::{parse_secret_action, parse_secret_mode, parse_setup_profile, SetupOptions};
use crate::vector::VectorStore;

/// Command-line interface arguments for the go-on application
#[derive(Debug, Parser)]
#[command(name = "go-on")]
#[command(about = "ACP proxy with flow, phases and multi-agent routing")]
struct Cli {
    /// Path to configuration file
    #[arg(long)]
    config: Option<PathBuf>,

    /// Phase to run
    #[arg(long)]
    phase: Option<String>,

    /// Enable verbose logging
    #[arg(long, default_value_t = false)]
    verbose: bool,

    /// Validate configuration and exit
    #[arg(long, default_value_t = false)]
    validate_config: bool,

    /// Run setup wizard
    #[arg(long, default_value_t = false)]
    setup: bool,

    /// Setup profile to use
    #[arg(long)]
    setup_profile: Option<String>,

    /// Secret mode for setup
    #[arg(long)]
    setup_secrets: Option<String>,

    /// Force setup even if files exist
    #[arg(long, default_value_t = false)]
    force: bool,

    /// Secret management action
    #[arg(long)]
    secret: Option<String>,

    /// Secret name for management
    #[arg(long)]
    secret_name: Option<String>,

    /// Secret value for management
    #[arg(long)]
    secret_value: Option<String>,

    /// Generate a runtime healthcheck report and persist it into .goon/
    #[arg(long, default_value_t = false)]
    healthcheck: bool,

    /// Run action checks (all/spec/qa/retest/final) against .goon/ artifacts
    #[arg(long)]
    action_check: Option<String>,

    /// Build and persist a controlled task plan artifact for a complex task
    #[arg(long)]
    plan_task: Option<String>,
}

/// Get the default configuration file path
///
/// Returns the path to config.toml in the same directory as the executable
fn default_config_path() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("failed to resolve executable directory"))?;
    Ok(dir.join("config.toml"))
}

/// Emit configuration warnings to the log and optionally to stderr
///
/// # Arguments
/// * `warnings` - Slice of configuration warnings
/// * `mirror_stderr` - Whether to also print warnings to stderr
fn emit_config_warnings(warnings: &[ConfigWarning], mirror_stderr: bool) {
    for warning in warnings {
        let severity = match warning.severity {
            crate::config::ConfigWarningSeverity::Critical => "critical",
            crate::config::ConfigWarningSeverity::Warn => "warn",
            crate::config::ConfigWarningSeverity::Info => "info",
        };
        warn!(
            "config warning [{}:{}] {}",
            severity, warning.code, warning.message
        );
        if mirror_stderr {
            eprintln!(
                "config warning [{}:{}] {}",
                severity, warning.code, warning.message
            );
        }
    }
}

fn resolve_config_relative_path(config_path: &std::path::Path, raw_path: &str) -> PathBuf {
    let candidate = PathBuf::from(raw_path);
    if candidate.is_absolute() {
        candidate
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join(candidate)
    }
}

async fn initialize_cache(
    config_path: PathBuf,
    cache_cfg: Option<crate::config::CacheConfig>,
) -> Result<Option<Arc<ResponseCache>>> {
    match cache_cfg {
        Some(cache_cfg) if cache_cfg.enabled => {
            let cache_path = resolve_config_relative_path(&config_path, &cache_cfg.path);
            info!(
                "sqlite cache enabled at {} (ttl={}s, max_entries={})",
                cache_path.display(),
                cache_cfg.default_ttl_seconds,
                cache_cfg.max_entries
            );

            tokio::task::spawn_blocking(move || {
                ResponseCache::new(
                    &cache_path,
                    cache_cfg.default_ttl_seconds,
                    cache_cfg.max_entries,
                )
                .map(Arc::new)
                .map(Some)
            })
            .await
            .map_err(|err| anyhow::anyhow!("cache init task join error: {}", err))?
        }
        _ => Ok(None),
    }
}

async fn initialize_vector_store(
    config_path: PathBuf,
    vector_cfg: Option<crate::config::VectorConfig>,
) -> Result<Option<Arc<VectorStore>>> {
    match vector_cfg {
        Some(vector_cfg) if vector_cfg.enabled => {
            let vector_path = resolve_config_relative_path(&config_path, &vector_cfg.path);
            info!(
                "vector memory enabled at {} (dims={}, top_k={}, similarity={})",
                vector_path.display(),
                vector_cfg.dimensions,
                vector_cfg.top_k,
                vector_cfg.min_similarity
            );

            tokio::task::spawn_blocking(move || {
                VectorStore::new(&vector_path, vector_cfg.dimensions, vector_cfg.max_entries)
                    .map(Arc::new)
                    .map(Some)
            })
            .await
            .map_err(|err| anyhow::anyhow!("vector init task join error: {}", err))?
        }
        _ => Ok(None),
    }
}

async fn initialize_autotune(
    config_path: PathBuf,
    autotune_cfg: Option<crate::config::AutoTuneConfig>,
) -> Result<(
    Option<Arc<tokio::sync::Mutex<AutoTuneState>>>,
    Option<crate::config::AutoTuneConfig>,
    Option<String>,
)> {
    match autotune_cfg {
        Some(autotune_cfg) if autotune_cfg.enabled => {
            let state_path = resolve_config_relative_path(&config_path, &autotune_cfg.state_path)
                .to_string_lossy()
                .to_string();
            info!(
                "autotune enabled (min_chars: {}-{}, step: {}, evaluate_interval: {})",
                autotune_cfg.min_query_chars_min,
                autotune_cfg.min_query_chars_max,
                autotune_cfg.min_query_chars_step,
                autotune_cfg.evaluate_interval
            );

            let autotune_cfg_for_load = autotune_cfg.clone();
            let state_path_for_load = state_path.clone();
            let state = tokio::task::spawn_blocking(move || {
                AutoTuneState::load_or_default(&state_path_for_load, &autotune_cfg_for_load)
            })
            .await
            .map_err(|err| anyhow::anyhow!("autotune init task join error: {}", err))?;

            Ok((
                Some(Arc::new(tokio::sync::Mutex::new(state))),
                Some(autotune_cfg),
                Some(state_path),
            ))
        }
        _ => Ok((None, None, None)),
    }
}

/// Main function - entry point for the application
#[tokio::main]
async fn main() {
    // Set up enhanced panic hook for production
    std::panic::set_hook(Box::new(|panic_info| {
        let location = panic_info
            .location()
            .map(|loc| loc.to_string())
            .unwrap_or_else(|| "unknown location".to_string());
        let payload = panic_info.payload();
        let message = if let Some(s) = payload.downcast_ref::<&str>() {
            *s
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.as_str()
        } else {
            "unknown panic"
        };

        error!("panic captured at {}: {}", location, message);

        // Log backtrace if available
        #[cfg(debug_assertions)]
        {
            let backtrace = std::backtrace::Backtrace::capture();
            error!("backtrace:\n{:?}", backtrace);
        }

        // Exit with error code
        std::process::exit(1);
    }));

    // Run the application and handle any errors
    if let Err(err) = run().await {
        error!("fatal error: {err:#}");
        eprintln!("{}", tf("error.fatal", &[("error", &format!("{err:#}"))]));
        std::process::exit(1);
    }
}

/// Core application logic
///
/// Handles command-line arguments, configuration loading, and server initialization
async fn run() -> Result<()> {
    // Parse command-line arguments
    let cli = Cli::parse();

    // Configure telemetry (structured logging, metrics, tracing)
    let telemetry_config = telemetry_enhanced::TelemetryConfig {
        log_level: if cli.verbose {
            "debug".to_string()
        } else {
            "info".to_string()
        },
        ..Default::default()
    };

    telemetry_enhanced::init_telemetry(&telemetry_config)
        .map_err(|err| anyhow::anyhow!("failed to initialize telemetry: {}", err))?;

    // Initialize enhanced telemetry components
    let _metrics_recorder = telemetry_enhanced::MetricsRecorder::new();
    let _health_metrics = telemetry_enhanced::HealthMetrics::new();
    info!("enhanced telemetry components initialized");

    // Initialize performance monitoring
    let _performance_monitor = performance::init_performance_monitoring();
    info!("performance monitoring initialized");

    // Determine configuration file path
    let config_path = match cli.config {
        Some(path) => path,
        None => default_config_path()?,
    };

    // Initialize i18n system
    let languages_dir = config_path
        .parent()
        .map(|p| p.join("languages"))
        .unwrap_or_else(|| std::path::Path::new("languages").to_path_buf());

    if let Err(e) = init_i18n(&languages_dir) {
        warn!(
            "Failed to initialize i18n system: {}. Continuing without translations.",
            e
        );
    } else {
        info!(
            "i18n system initialized with language directory: {:?}",
            languages_dir
        );
    }

    // Handle secret management commands
    if let Some(action) = cli.secret.as_deref() {
        let action = parse_secret_action(action)?;
        setup::run_secret_command(
            action,
            cli.secret_name.as_deref(),
            cli.secret_value.as_deref(),
        )?;
        return Ok(());
    }

    // Handle setup wizard
    if cli.setup {
        let options = SetupOptions {
            profile: cli
                .setup_profile
                .as_deref()
                .map(parse_setup_profile)
                .transpose()?,
            secret_mode: cli
                .setup_secrets
                .as_deref()
                .map(parse_secret_mode)
                .transpose()?,
            force: cli.force,
            prompt_for_secrets: cli.setup_profile.is_none() && cli.setup_secrets.is_none(),
        };
        setup::run_setup_with_options(&config_path, options)?;
        return Ok(());
    }

    // Load and validate configuration
    info!("loading config from {}", config_path.display());
    let config = Arc::new(AppConfig::load(&config_path)?);

    // Perform enhanced configuration validation
    let validation_result = config_validation::validate_config_file(&config_path)?;

    // Also run legacy validation for compatibility
    let health_report = validate_runtime_readiness(&config_path, &config)?;
    emit_config_warnings(&health_report.warnings, cli.validate_config);

    // If only validating config, exit after validation
    if cli.validate_config {
        // Enhanced validation report
        let validation_report =
            config_validation::ConfigValidator::new(&config_path, config.as_ref().clone())
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
            return Ok(());
        }

        return Ok(());
    }

    // Check if configuration is valid before proceeding
    if !validation_result.is_valid {
        error!("Configuration validation failed. Cannot start server.");
        let report = config_validation::ConfigValidator::new(&config_path, config.as_ref().clone())
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

    // Create HTTP client with timeout
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

    // Initialize agent registry and flow manager
    let registry = Arc::new(AgentRegistry::from_config(
        Arc::clone(&config),
        http_client.clone(),
    )?);
    let flow = Arc::new(FlowManager::new(Arc::clone(&config), cli.phase.clone()));

    // Display agent vendor information
    let agents_by_vendor = registry.agents_by_vendor();
    info!("Agents organized by vendor category:");
    for (category, agents) in &agents_by_vendor {
        info!("  {:?}: {} agents", category, agents.len());
        for agent in agents {
            info!("    - {}", agent);
        }
    }

    let (cache, vector_store, (autotune_state, autotune_config, autotune_state_path)) = tokio::try_join!(
        initialize_cache(config_path.clone(), config.cache.clone()),
        initialize_vector_store(config_path.clone(), config.vector.clone()),
        initialize_autotune(config_path.clone(), config.autotune.clone()),
    )?;

    let ledger = ArtifactLedger::new(Some(&config_path));

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
            Some(&config_path),
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
    let runtime_config = config
        .runtime
        .clone()
        .unwrap_or_else(RuntimeConfig::default);

    // Create and run the ACP server
    let mut server = new_acp_server(
        flow,
        registry,
        cache,
        vector_store,
        config.vector.clone(),
        autotune_state,
        autotune_config,
        autotune_state_path,
        Some(config_path.to_string_lossy().to_string()),
        runtime_config,
        Some(http_client),
        cli.verbose,
    );
    run_acp_server(&mut server).await
}
