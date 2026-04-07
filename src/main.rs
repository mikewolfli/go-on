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

mod acp;
mod adaptive_selector;
mod advanced_modules;
mod agent;
mod agents;
mod audit;
mod cache;
mod config;
mod config_validation;
mod context;
mod cost_optimizer;
mod error;
mod evaluation;
mod failure_prevention;
mod flow;
mod flow_with_models;
mod graph;
mod hardening;
mod i18n;
mod i18n_watcher;
mod mcp;
mod mcp_server;
mod memory;
mod memory_response_cache;
mod mode;
mod model_selector;
mod observability;
mod orchestrator;
mod performance;
mod promotion;
mod pua;
mod quality_models;
mod reliability_optimizer;
mod review_controls;
mod roles;
mod rpc_protocol;
mod runtime_controls;
mod setup;
mod speed_optimizer;
mod task_decomposer;
mod task_graph;
mod task_router;
mod telemetry;
mod telemetry_enhanced;
mod tool;
mod vector;
mod verification;
mod workflow_optimizer;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use tracing::{error, info, warn};

use crate::acp::AcpServer;
use crate::agent::AgentRegistry;
use crate::cache::ResponseCache;
use crate::config::{
    validate_runtime_readiness, AppConfig, AutoTuneState, ConfigWarning, RuntimeConfig,
};
use crate::flow::FlowManager;
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
        eprintln!("fatal error: {err:#}");
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
            "legacy health score: {}/100 (warnings: {} critical, {} warn, {} info)",
            health_report.score,
            health_report.critical_count,
            health_report.warn_count,
            health_report.info_count
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

    // Get runtime configuration
    let runtime_config = config
        .runtime
        .clone()
        .unwrap_or_else(RuntimeConfig::default);

    // Create and run the ACP server
    let mut server = AcpServer::new(
        flow,
        registry,
        cache,
        vector_store,
        config.vector.clone(),
        autotune_state,
        autotune_config,
        autotune_state_path,
        runtime_config,
        Some(config_path.clone()),
        cli.phase.clone(),
        Some(http_client),
        cli.verbose,
    );
    server.run().await
}
