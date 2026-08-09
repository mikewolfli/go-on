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
//! go-on --doctor --config /path/to/config.toml
//!
//! # Run guided onboarding
//! go-on --init --config /path/to/config.toml
//!
//! # Check readiness and completeness
//! go-on --check --config /path/to/config.toml
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

pub(crate) mod cli;
pub(crate) mod report;
pub(crate) mod server;

#[cfg(test)]
mod tests;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use tokio::sync::Notify;
use tracing::{error, info};

// Re-exports used by tests (via use super::*;)
#[cfg(test)]
pub(crate) use cli::validate_cli_protocol_mode;
#[cfg(test)]
pub(crate) use report::{build_completeness_report, RecommendationLevel};

use crate::config::AppConfig;
use crate::i18n::runtime::tf;

/// Get the default configuration file path
///
/// Search order:
/// 1) ./config.toml
/// 2) Platform config dir + /go-on/config.toml (created if missing)
/// 3) Exe directory (fallback only — may not be writable)
pub(crate) fn default_config_path() -> Result<PathBuf> {
    let cwd_candidate = std::env::current_dir()?.join("config.toml");
    if cwd_candidate.exists() {
        return Ok(cwd_candidate);
    }

    let config_root = preferred_config_root(std::env::consts::OS, |key| {
        std::env::var_os(key).map(PathBuf::from)
    });
    if let Some(root) = config_root {
        let home_candidate = root.join("go-on").join("config.toml");
        if home_candidate.exists() {
            return Ok(home_candidate);
        }
        if let Some(parent) = home_candidate.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        return Ok(home_candidate);
    }

    // Last resort: exe directory (may not be writable, but better than nothing)
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("failed to resolve executable directory"))?;
    Ok(dir.join("config.toml"))
}

pub(crate) fn preferred_config_root<F>(current_os: &str, env_get: F) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<PathBuf>,
{
    if current_os == "windows" {
        if let Some(appdata) = env_get("APPDATA") {
            return Some(appdata);
        }
        return env_get("USERPROFILE").map(|p| p.join("AppData").join("Roaming"));
    }

    if let Some(xdg) = env_get("XDG_CONFIG_HOME") {
        return Some(xdg);
    }
    env_get("HOME").map(|p| p.join(".config"))
}

/// Main function - entry point for the application
pub(crate) async fn main() {
    // CapabilityBus is wired inside the ACP runtime
    // (transport_factory::dispatch_server → new_acp_server), where it is
    // genuinely used for tool execution and cognitive orchestration.
    // No orphaned scaffolding needed here.

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

async fn run() -> Result<()> {
    // Initialize telemetry (tracing subscriber) early so that all subsequent
    // tracing::info!() / warn!() calls are captured on stderr.
    let telemetry_cfg = crate::observability::telemetry_enhanced::TelemetryConfig {
        enable_logging: true,
        enable_tracing: false,
        enable_metrics: false,
        service_name: "go-on".to_string(),
        ..Default::default()
    };
    let _ = crate::observability::telemetry_enhanced::init_telemetry(&telemetry_cfg);

    // GAP-46-12: Plugin info is now built inline as a simple PluginInfo list.
    // SessionCompressor is fully wired into the chat pipeline (compress method
    // called from session.rs). The #[cfg_attr(not(test), allow(dead_code))]
    // on SessionCompressor struct covers test-only fields.
    // Parse command-line arguments
    let mut cli = cli::Cli::parse();
    if let Some(command) = cli.command.take() {
        match command {
            cli::CliCommand::Init => cli.setup = true,
            cli::CliCommand::Status => cli.status = true,
            cli::CliCommand::Diagnose => cli.diagnose = true,
            cli::CliCommand::Skill { command } => {
                return cli::handle_skill_command(command).await;
            }
            #[cfg(feature = "sub-bus-distributed-memory")]
            cli::CliCommand::Hub { port } => {
                let mut hub = crate::hub::server::HubServer::with_port(port)?;
                hub.start().await?;
                tokio::signal::ctrl_c().await?;
                return Ok(());
            }
        }
    }

    // Determine configuration file path
    let config_path = match cli.config {
        Some(ref path) => path.clone(),
        None => default_config_path()?,
    };

    // ── System bootstrap: i18n, observability, provider, skills discovery ──
    // Telemetry is already initialized above, so skip it here.
    // The returned SkillRegistry is populated with ~/.agents/skills/ SKILL.md files.
    let skill_registry =
        match crate::core::bootstrap::perform_bootstrap(&crate::core::bootstrap::BootstrapConfig {
            enable_telemetry: false,
            enable_i18n: true,
            config_path: config_path.clone(),
        })
        .await
        {
            Ok(registry) => {
                tracing::info!("bootstrap complete, skill registry populated");
                Some(Arc::new(std::sync::RwLock::new(registry)))
            }
            Err(e) => {
                tracing::warn!("bootstrap skipped: {e}");
                None
            }
        };

    // GAP-B50-33: Check startup memory (critical check only)
    // Background monitoring is deferred to wire_server() to avoid
    // competing with the critical stdio initialization path.
    //
    // Integration tests can bypass this check by setting
    // GO_ON_SKIP_MEMORY_CHECK=true — useful when the test environment has
    // critically low but non-fatal available memory.
    if std::env::var("GO_ON_SKIP_MEMORY_CHECK").map_or(true, |v| v != "true") {
        let memory_health = crate::observability::memory_health::check_startup_memory();
        tracing::info!(?memory_health, "startup memory check");
        crate::observability::memory_health::print_memory_health(&memory_health);
        if let crate::observability::memory_health::MemoryHealth::Critical { free_mb, message } =
            &memory_health
        {
            anyhow::bail!(
                "Insufficient memory to start server: {} MB free — {}",
                free_mb,
                message
            );
        }
    }
    // Memory background monitor starts in wire_server() (deduplicated).

    // Handle secret management commands, local model setup, and onboarding
    if server::handle_secret_commands(&cli, &config_path)? {
        return Ok(());
    }

    // Handle diagnose mode: run system diagnostics and exit
    if cli.diagnose {
        diagnose_and_exit(&config_path).await;
        return Ok(());
    }

    // Load, validate configuration, and handle validation-only modes
    let config = match server::handle_validation_mode(&cli, &config_path).await? {
        Some(config) => config,
        None => return Ok(()),
    };

    // ── Graceful shutdown notify (shared with all background tasks) ──
    let shutdown_notify = Arc::new(Notify::new());
    let sig_shutdown = shutdown_notify.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Received SIGINT, initiating top-level graceful shutdown");
        sig_shutdown.notify_waiters();
    });

    // Note: ContinuousLearningCenter agent injection happens inside
    // ServerBuilder::build (server_builder.rs) with the first available
    // agent — the previous throwaway cl_agent_handle pipeline was removed
    // because the center never read it.

    // Delegate interactive agent onboarding to the onboarding module
    let onboarding_cfg = crate::core::onboarding::OnboardingConfig {
        enabled: !cli.setup
            && !cli.chat
            && std::env::var("GO_ON_ENABLE_LOCAL_TEST_AGENTS").is_err(),
        is_terminal: std::io::stdin().is_terminal()
            && std::io::stdout().is_terminal()
            && !std::env::args().any(|a| a == "--setup" || a == "--init"),
    };
    if crate::core::onboarding::run_onboarding(&onboarding_cfg, &config_path).await? {
        let cp = config_path.clone();
        let config =
            Arc::new(tokio::task::spawn_blocking(move || AppConfig::load_uncached(&cp)).await??);
        tokio::select! {
            result = server::start_server(config.clone(), &cli, &config_path, skill_registry.clone()) => {
                result?;
            }
            _ = shutdown_notify.notified() => {
                info!("Top-level shutdown signal received during onboarding server start");
            }
        }
        return Ok(());
    }

    // Handle terminal chat mode
    if cli.chat {
        return server::handle_chat_mode(config, &cli, &config_path).await;
    }

    // Start the server with top-level graceful shutdown signal handling
    tokio::select! {
        result = server::start_server(config, &cli, &config_path, skill_registry) => {
            result?;
        }
        _ = shutdown_notify.notified() => {
            info!("Top-level shutdown signal received, server exiting");
        }
    }

    Ok(())
}

/// Run system diagnostics and print a detailed report, then exit.
///
/// Diagnoses configuration, connectivity, agents, and system health
/// without starting the full server. Useful for pre-flight checks.
async fn diagnose_and_exit(config_path: &std::path::Path) {
    use crate::config::AppConfig;
    use crate::core::config_validation::ConfigValidator;

    println!("═══════════════════════════════════");
    println!("  go-on System Diagnostics");
    println!("═══════════════════════════════════");
    println!();

    // 1. Config file check
    println!("[1/5] Configuration");
    match config_path.try_exists() {
        Ok(true) => println!("  ✅ Config file found: {}", config_path.display()),
        Ok(false) => {
            println!("  ❌ Config file not found: {}", config_path.display());
            return;
        }
        Err(e) => {
            println!("  ❌ Cannot access config: {e}");
            return;
        }
    }

    // 2. Config parsing and validation
    println!("[2/5] Config Validation");
    match AppConfig::load(config_path) {
        Ok(config) => {
            println!("  ✅ Config parsed successfully");
            let agent_count = config.agents().len();
            println!("  ℹ️  Agents configured: {agent_count}");
            println!("  ℹ️  Workflow type: {:?}", config.flow.workflow_type);
            let validator = ConfigValidator::new(config_path, config);
            let report = validator.validate();
            if report.errors.is_empty() && report.warnings.is_empty() {
                println!("  ✅ Config validation: no issues");
            } else {
                for err in &report.errors {
                    println!("  ❌ Validation error: {err:?}");
                }
                for warn in &report.warnings {
                    println!("  ⚠️  Validation warning: {warn:?}");
                }
            }
        }
        Err(e) => {
            println!("  ❌ Config parse error: {e}");
        }
    }

    // 3. Memory health
    println!("[3/5] System Health");
    let mem = crate::observability::memory_health::check_startup_memory();
    match &mem {
        crate::observability::memory_health::MemoryHealth::Healthy => {
            println!("  ✅ Memory: healthy");
        }
        crate::observability::memory_health::MemoryHealth::Low { free_mb, .. } => {
            println!("  ⚠️  Memory: low ({free_mb} MB free)");
        }
        crate::observability::memory_health::MemoryHealth::Critical { free_mb, message } => {
            println!("  ❌ Memory: critical ({free_mb} MB free) — {message}");
        }
        crate::observability::memory_health::MemoryHealth::Unknown => {
            println!("  ⚠️  Memory: unknown");
        }
    }
    crate::observability::memory_health::print_memory_health(&mem);

    // 4. i18n readiness
    println!("[4/5] Internationalization");
    let i18n_dir = config_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("languages");
    match std::fs::read_dir(&i18n_dir) {
        Ok(entries) => {
            let count = entries.filter_map(|e| e.ok()).count();
            println!("  ✅ i18n directory exists: {} entries", count);
        }
        Err(_) => {
            println!("  ⚠️  i18n directory not found: {}", i18n_dir.display());
        }
    }

    // 5. Governance status gate
    println!("[5/5] Governance Readiness");
    let known_tools = crate::governance::status::known_tool_names();
    println!("  ✅ Governance gate: {} known tools", known_tools.len());
    println!("  ℹ️  Tools: {:?}", known_tools.iter().collect::<Vec<_>>());

    // Test governance gate with safe and dangerous inputs
    let safe_check = crate::governance::status::quick_check_tool(
        "read_file",
        &serde_json::json!({"path": "src/main.rs"}),
    );
    println!(
        "  {} Governance gate: safe read test passed",
        if safe_check.is_ok() { "✅" } else { "❌" }
    );

    let block_check = crate::governance::status::quick_check_tool(
        "write_file",
        &serde_json::json!({"path": "/etc/hosts", "content": "evil"}),
    );
    println!(
        "  {} Governance gate: write protection active",
        if block_check.is_err() { "✅" } else { "❌" }
    );

    println!();
    println!("═══════════════════════════════════");
    println!("  Diagnostics complete.");
    println!("═══════════════════════════════════");
}
