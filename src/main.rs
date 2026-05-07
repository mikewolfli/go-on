#![recursion_limit = "512"]
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

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
mod agents;
mod core;
mod fault_tolerance;
mod governance;
mod i18n;
mod intelligence;
mod mcp;
mod memory;
mod observability;
mod optimization;
mod orchestration;
mod protocol;
mod resilience;
mod shared;

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

pub use crate::intelligence::evaluation;
pub use crate::intelligence::model_selector;
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

pub use crate::optimization::failure_prevention;

pub use crate::orchestration::flow;
pub use crate::orchestration::flow_with_models;
pub use crate::orchestration::mode;
pub use crate::orchestration::orchestrator;
pub use crate::orchestration::roles;
pub use crate::orchestration::task_decomposer;
pub use crate::orchestration::task_graph;
pub use crate::orchestration::task_router;
pub use crate::orchestration::tool;
pub use crate::protocol::mcp_server;
pub use crate::protocol::rpc_protocol;

use std::fs;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use clap::{ArgAction, Parser, Subcommand};
use tracing::{error, info, warn};

use crate::acp::background::start_background_tasks;
use crate::acp::r#impl::{new_acp_server, run_acp_http_server, run_acp_server};
use crate::agent::AgentRegistry;
use crate::cache::ResponseCache;
use crate::config::{
    is_agent_env_ready, validate_runtime_readiness, AppConfig, AutoTuneState, ConfigWarning,
};
use crate::flow::FlowManager;
use crate::i18n::runtime::{init_i18n, tf};
use crate::intelligence::capability_graph::CapabilityGraph;
use crate::mcp_server::{McpHttpServer, McpStdioServer};
use crate::protocol::access_mode::{resolve_access_selection, TransportMode};
use crate::reinforcement::{
    build_runtime_healthcheck_report, build_task_plan, persist_runtime_healthcheck,
    persist_task_plan, run_action_check, ActionCheckKind, ArtifactLedger, RuntimeHealthcheckReport,
};
use crate::setup::{
    add_local_model, apply_recommended_to_config, parse_secret_action, parse_secret_mode,
    parse_setup_level, parse_setup_profile, recommendation_snapshot_for_config, LocalModelOptions,
    SetupOptions,
};
use crate::shared::protocol_mode::{ProtocolMode, ProtocolModeError};
use crate::tool::ToolRegistry;
use crate::vector::VectorStore;

fn validate_cli_protocol_mode(raw: Option<&str>) -> Result<Option<String>> {
    let Some(value) = raw else {
        return Ok(None);
    };

    let normalized = match ProtocolMode::from_fuzzy(value) {
        Ok(mode) => mode.to_cli_arg(),
        Err(ProtocolModeError::FromConfigNotSupported) => {
            anyhow::bail!(
                "invalid --protocol-mode '{}'; from_config is only supported in GUI/VS Code startup settings",
                value
            );
        }
        Err(ProtocolModeError::AmbiguousPrefix(prefix)) => {
            anyhow::bail!(
                "ambiguous --protocol-mode prefix '{}'; allowed: {}",
                prefix,
                ProtocolMode::CANONICAL_MODES.join(", ")
            );
        }
        Err(ProtocolModeError::InvalidValue(_)) => {
            anyhow::bail!(
                "invalid --protocol-mode '{}'; allowed: {}",
                value,
                ProtocolMode::CANONICAL_MODES.join(", ")
            );
        }
    };

    Ok(Some(normalized.to_string()))
}

/// Command-line interface arguments for the go-on application
#[derive(Debug, Parser)]
#[command(name = "go-on")]
#[command(about = "ACP proxy with flow, phases and multi-agent routing")]
struct Cli {
    #[command(subcommand)]
    command: Option<CliCommand>,

    /// Path to configuration file
    #[arg(short = 'c', long)]
    config: Option<PathBuf>,

    /// Phase to run
    #[arg(long)]
    phase: Option<String>,

    /// Enable verbose logging
    #[arg(short = 'v', long, action = ArgAction::Count)]
    verbose: u8,

    /// Validate configuration and exit
    #[arg(long, visible_alias = "doctor", default_value_t = false)]
    validate_config: bool,

    /// Run end-to-end diagnosis and print concise remediation output
    #[arg(long, default_value_t = false)]
    diagnose: bool,

    /// Run setup wizard
    #[arg(long, visible_alias = "init", default_value_t = false)]
    setup: bool,

    /// Setup profile to use
    #[arg(long)]
    setup_profile: Option<String>,

    /// Setup wizard level to use (quick|standard|custom)
    #[arg(long)]
    setup_level: Option<String>,

    /// Secret mode for setup
    #[arg(long)]
    setup_secrets: Option<String>,

    /// Add or update a local model agent entry in config
    #[arg(long, visible_alias = "add-model", default_value_t = false)]
    add_local_model: bool,

    /// Local model agent name when using --add-model
    #[arg(long)]
    local_model_name: Option<String>,

    /// Local model endpoint URL when using --add-model
    #[arg(long)]
    local_model_url: Option<String>,

    /// Local model provider type when using --add-model (default: openai)
    #[arg(long)]
    local_model_type: Option<String>,

    /// Local model model-id when using --add-model
    #[arg(long)]
    local_model_model: Option<String>,

    /// Optional API key env var field for local model when using --add-model
    #[arg(long)]
    local_model_api_key_env: Option<String>,

    /// Optional secret key env var field for local model when using --add-model
    #[arg(long)]
    local_model_secret_key_env: Option<String>,

    /// Only register local model under [agents], do not auto-attach it to phase agent lists
    #[arg(long, default_value_t = false)]
    local_model_register_only: bool,

    /// Apply provider capability recommendations to current config.toml and exit
    #[arg(long, default_value_t = false)]
    apply_recommended: bool,

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

    /// Print configured AI providers and runtime readiness status
    #[arg(long, visible_alias = "check", default_value_t = false)]
    status: bool,

    /// Bind ACP HTTP server and expose /health, /chat, and /chat/stream
    #[arg(short = 'b', long, visible_alias = "bind")]
    acp_http_bind: Option<String>,

    /// Access protocol mode override (adaptive|acp_stdio|acp_http|mcp_stdio|mcp_http)
    #[arg(short = 'm', long, visible_alias = "mode", value_name = "MODE")]
    protocol_mode: Option<String>,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Generate default configuration interactively
    Init,
    /// Print runtime readiness status
    Status,
    /// Run end-to-end diagnosis with remediation hints
    Diagnose,
}

/// Get the default configuration file path
///
/// Search order:
/// 1) ./config.toml
/// 2) $HOME/.config/go-on/config.toml (created if missing)
/// 3) Exe directory (fallback only — may not be writable)
fn default_config_path() -> Result<PathBuf> {
    let cwd_candidate = std::env::current_dir()?.join("config.toml");
    if cwd_candidate.exists() {
        return Ok(cwd_candidate);
    }

    // Cross-platform home directory: $HOME (Unix) or %USERPROFILE% (Windows)
    let home_dir = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    if let Some(home) = home_dir {
        let home = PathBuf::from(home);
        let home_candidate = home.join(".config/go-on/config.toml");
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
            tracing::warn!(
                "config warning [{}:{}] {}",
                severity,
                warning.code,
                warning.message
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

fn print_runtime_status(config_path: &std::path::Path, report: &RuntimeHealthcheckReport) {
    println!("{}", tf("status.title", &[]));
    println!(
        "{}",
        tf(
            "status.config_path",
            &[("path", &config_path.to_string_lossy())]
        )
    );
    println!(
        "{}",
        tf(
            "status.overall",
            &[("status", &format!("{:?}", report.overall_status))]
        )
    );

    let provider_component = report
        .components
        .iter()
        .find(|component| component.name == "provider_dependencies");

    let Some(component) = provider_component else {
        println!("{}", tf("status.no_provider_component", &[]));
        println!("{}", tf("status.done", &[]));
        return;
    };

    let configured_agents = component
        .details
        .get("total")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    println!(
        "{}",
        tf(
            "status.configured_agents",
            &[("count", &configured_agents.to_string())]
        )
    );

    let details = component
        .details
        .get("agents")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    for item in details {
        let name = item
            .get("agent")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let agent_type = item
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let ready = item
            .get("ready")
            .and_then(|value| value.as_bool())
            .map(|value| value.to_string())
            .unwrap_or_else(|| "false".to_string());
        let endpoint_status = item
            .get("endpoint_status")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let missing_envs = item
            .get("missing_envs")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|entry| entry.as_str())
                    .collect::<Vec<&str>>()
            })
            .unwrap_or_default();
        let missing_envs = if missing_envs.is_empty() {
            "-".to_string()
        } else {
            missing_envs.join("|")
        };

        println!(
            "{}",
            tf(
                "status.agent_line",
                &[
                    ("name", name),
                    ("type", agent_type),
                    ("ready", &ready),
                    ("endpoint_status", endpoint_status),
                    ("missing_envs", &missing_envs),
                ]
            )
        );

        for (label, key_status) in [
            ("api_keys", item.get("api_key_status")),
            ("secret_keys", item.get("secret_key_status")),
        ] {
            let Some(key_status) = key_status else {
                continue;
            };
            if key_status.is_null() {
                continue;
            }

            let secret_ref = key_status
                .get("ref")
                .and_then(|value| value.as_str())
                .unwrap_or("-");
            let count = key_status
                .get("count")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            let fingerprints = key_status
                .get("fingerprints")
                .and_then(|value| value.as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|entry| entry.as_str())
                        .collect::<Vec<&str>>()
                })
                .unwrap_or_default();
            let fingerprints = if fingerprints.is_empty() {
                "-".to_string()
            } else {
                fingerprints.join(" | ")
            };

            println!(
                "{}",
                tf(
                    "status.secret_line",
                    &[
                        ("label", label),
                        ("count", &count.to_string()),
                        ("secret_ref", secret_ref),
                        ("fingerprints", &fingerprints),
                    ]
                )
            );
        }
    }

    println!("{}", tf("status.done", &[]));
}

#[derive(Default)]
struct StatusCompleteness {
    score: u32,
    missing: Vec<String>,
    recommended: Vec<StatusRecommendation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecommendationLevel {
    Warning,
    Info,
}

#[derive(Clone, Debug)]
struct StatusRecommendation {
    level: RecommendationLevel,
    message: String,
}

impl StatusCompleteness {
    fn push_warning(&mut self, message: String) {
        self.recommended.push(StatusRecommendation {
            level: RecommendationLevel::Warning,
            message,
        });
    }

    fn push_info(&mut self, message: String) {
        self.recommended.push(StatusRecommendation {
            level: RecommendationLevel::Info,
            message,
        });
    }
}

fn inflight_recommendation(
    field: &str,
    expected: i64,
    current: i64,
) -> (RecommendationLevel, String) {
    let delta = (expected - current).abs();
    let ratio = if expected > 0 {
        delta as f64 / expected as f64
    } else {
        0.0
    };
    let level = if ratio >= 0.25 {
        RecommendationLevel::Warning
    } else {
        RecommendationLevel::Info
    };
    (
        level,
        format!(
            "{} recommended={}, current={} (delta={:.0}%)",
            field,
            expected,
            current,
            ratio * 100.0
        ),
    )
}

fn build_completeness_report(
    config: &crate::config::AppConfig,
    report: &RuntimeHealthcheckReport,
) -> StatusCompleteness {
    let mut out = StatusCompleteness::default();
    let mut score = 0.0_f64;
    let provider_recommendation = recommendation_snapshot_for_config(config);

    let provider = report
        .components
        .iter()
        .find(|component| component.name == "provider_dependencies");

    let (ready, total) = provider
        .map(|component| {
            let ready = component
                .details
                .get("ready")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            let total = component
                .details
                .get("total")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            (ready, total)
        })
        .unwrap_or((0, 0));

    if total > 0 {
        score += 30.0 * (ready as f64 / total as f64);
        if ready < total {
            out.missing
                .push("provider credentials or endpoint readiness incomplete".to_string());
        }
    }

    if total > 0 {
        score += 25.0 * (ready as f64 / total as f64);
    }

    for phase_name in ["planning", "coding", "review", "delivery"] {
        let expected = provider_recommendation
            .as_ref()
            .map(|item| match phase_name {
                "planning" => item.planning_request_timeout_seconds,
                "coding" => item.coding_request_timeout_seconds,
                "review" => item.review_request_timeout_seconds,
                _ => item.delivery_request_timeout_seconds,
            })
            .unwrap_or(match phase_name {
                "planning" => 120,
                "coding" => 150,
                "review" => 60,
                _ => 90,
            });

        let actual = config
            .phases
            .get(phase_name)
            .and_then(|phase| phase.options.as_ref())
            .and_then(|options| options.request_timeout_seconds);

        match actual {
            Some(timeout) if timeout == expected => score += 2.5,
            Some(timeout) => {
                score += 1.5;
                out.push_info(format!(
                    "phases.{}.options.request_timeout_seconds recommended={}, current={}",
                    phase_name, expected, timeout
                ));
            }
            None => out.missing.push(format!(
                "phases.{}.options.request_timeout_seconds",
                phase_name
            )),
        }
    }

    let expected_review_timeout = provider_recommendation
        .as_ref()
        .map(|item| item.coding_review_timeout_seconds)
        .unwrap_or(60);
    let actual_review_timeout = config
        .phases
        .get("coding")
        .and_then(|phase| phase.options.as_ref())
        .and_then(|options| options.review_timeout_seconds);
    match actual_review_timeout {
        Some(timeout) if timeout == expected_review_timeout => score += 5.0,
        Some(timeout) => {
            score += 2.5;
            out.push_info(format!(
                "phases.coding.options.review_timeout_seconds recommended={}, current={}",
                expected_review_timeout, timeout
            ));
        }
        None => out
            .missing
            .push("phases.coding.options.review_timeout_seconds".to_string()),
    }

    let expected_phase_inflight = provider_recommendation
        .as_ref()
        .map(|item| item.phase_max_inflight as i64)
        .unwrap_or(24);
    let expected_global_inflight = provider_recommendation
        .as_ref()
        .map(|item| item.global_max_inflight as i64)
        .unwrap_or(128);
    let coding_options = config
        .phases
        .get("coding")
        .and_then(|phase| phase.options.as_ref());
    let actual_phase_inflight = coding_options.and_then(|options| {
        options
            .extra
            .get("phase_max_inflight")
            .and_then(|value| value.as_i64())
    });
    let actual_global_inflight = coding_options.and_then(|options| {
        options
            .extra
            .get("global_max_inflight")
            .and_then(|value| value.as_i64())
    });

    match actual_phase_inflight {
        Some(value) if value == expected_phase_inflight => score += 2.5,
        Some(value) => {
            score += 1.5;
            let (level, message) = inflight_recommendation(
                "phases.coding.options.phase_max_inflight",
                expected_phase_inflight,
                value,
            );
            match level {
                RecommendationLevel::Warning => out.push_warning(message),
                RecommendationLevel::Info => out.push_info(message),
            }
        }
        None => out
            .missing
            .push("phases.coding.options.phase_max_inflight".to_string()),
    }

    match actual_global_inflight {
        Some(value) if value == expected_global_inflight => score += 2.5,
        Some(value) => {
            score += 1.5;
            let (level, message) = inflight_recommendation(
                "phases.coding.options.global_max_inflight",
                expected_global_inflight,
                value,
            );
            match level {
                RecommendationLevel::Warning => out.push_warning(message),
                RecommendationLevel::Info => out.push_info(message),
            }
        }
        None => out
            .missing
            .push("phases.coding.options.global_max_inflight".to_string()),
    }

    let recommended_cache = provider_recommendation
        .as_ref()
        .map(|item| item.cache_enabled)
        .unwrap_or(true);
    let cache_enabled = config
        .cache
        .as_ref()
        .map(|cache| cache.enabled)
        .unwrap_or(false);
    if cache_enabled == recommended_cache {
        score += 5.0;
    } else {
        out.push_info(format!("cache.enabled={} recommended", recommended_cache));
    }

    let recommended_vector = provider_recommendation
        .as_ref()
        .map(|item| item.vector_enabled)
        .unwrap_or(true);
    let vector_enabled = config
        .vector
        .as_ref()
        .map(|vector| vector.enabled)
        .unwrap_or(false);
    if config
        .vector
        .as_ref()
        .map(|vector| vector.enabled)
        .unwrap_or(false)
    {
        if vector_enabled == recommended_vector {
            score += 5.0;
        } else {
            out.push_info(format!("vector.enabled={} recommended", recommended_vector));
        }
    } else {
        if !recommended_vector {
            score += 5.0;
        } else {
            out.push_info(format!("vector.enabled={} recommended", recommended_vector));
        }
    }

    if config.phases.contains_key("review") {
        score += 5.0;
    } else {
        out.missing.push("review phase missing".to_string());
    }

    if config.phases.contains_key("delivery") {
        score += 5.0;
    } else {
        out.missing.push("delivery phase missing".to_string());
    }

    if config
        .runtime
        .as_ref()
        .map(|runtime| runtime.health_interval_seconds > 0)
        .unwrap_or(false)
    {
        score += 5.0;
    } else {
        out.missing
            .push("runtime.health_interval_seconds missing".to_string());
    }

    if config
        .runtime
        .as_ref()
        .map(|runtime| runtime.maintenance_interval_seconds > 0)
        .unwrap_or(false)
    {
        score += 5.0;
    } else {
        out.missing
            .push("runtime.maintenance_interval_seconds missing".to_string());
    }

    out.score = score.round().clamp(0.0, 100.0) as u32;
    out
}

fn print_completeness_report(config: &crate::config::AppConfig, report: &RuntimeHealthcheckReport) {
    let completeness = build_completeness_report(config, report);
    println!(
        "{}",
        tf(
            "status.completeness",
            &[("score", &completeness.score.to_string())]
        )
    );

    if completeness.missing.is_empty() {
        println!("{}", tf("status.missing_none", &[]));
    } else {
        println!("{}", tf("status.missing_title", &[]));
        for item in completeness.missing {
            println!("- {}", item);
        }
    }

    if !completeness.recommended.is_empty() {
        println!("{}", tf("status.recommended_title", &[]));
        for item in completeness.recommended {
            let level = match item.level {
                RecommendationLevel::Warning => "warning",
                RecommendationLevel::Info => "info",
            };
            println!(
                "{}",
                tf(
                    "status.recommended_item",
                    &[("level", level), ("message", &item.message)]
                )
            );
        }
    }
}

fn maybe_prompt_ai_onboarding(cli: &Cli, config_path: &std::path::Path) -> Result<bool> {
    if cli.setup
        || cli.validate_config
        || cli.healthcheck
        || cli.status
        || cli.add_local_model
        || cli.apply_recommended
        || cli.secret.is_some()
        || cli.plan_task.is_some()
        || cli.action_check.is_some()
    {
        return Ok(false);
    }

    let is_terminal = std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::env::var("GO_ON_ENABLE_LOCAL_TEST_AGENTS").is_err();

    let Some(state) = detect_ai_onboarding_state(config_path)? else {
        return Ok(false);
    };

    // Non-terminal mode (GUI / addon): log a single clear message instead of spamming warnings
    if !is_terminal {
        match state {
            AiOnboardingState::MissingConfig | AiOnboardingState::BlankConfig => {
                info!(
                    "no configuration found at {} — create one with `go-on --init` or use the GUI setup wizard",
                    config_path.display()
                );
            }
            AiOnboardingState::NoAgents => {
                info!(
                    "configuration at {} has no AI providers — add providers with `go-on --init` or use the GUI settings page",
                    config_path.display()
                );
            }
            AiOnboardingState::AgentsNotReady => {
                info!(
                    "configuration at {} has AI providers but API keys are not set — configure credentials with `go-on --init` or the GUI settings page",
                    config_path.display()
                );
            }
            AiOnboardingState::InvalidConfig => {
                info!(
                    "configuration at {} is invalid — run `go-on --validate-config` for details",
                    config_path.display()
                );
            }
        }
        return Ok(false); // allow server to start, caller handles provider errors gracefully
    }

    // Terminal mode: interactive onboarding
    info!("starting onboarding flow for state={}", state.as_str());
    println!("{}", tf("setup.onboarding_intro", &[]));
    println!("{}", tf("setup.onboarding_option_1", &[]));
    println!("{}", tf("setup.onboarding_option_2", &[]));
    println!("{}", tf("setup.onboarding_option_3", &[]));
    print!("{}", tf("setup.onboarding_select", &[]));
    std::io::Write::flush(&mut std::io::stdout()).ok();

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let selection = input.trim();
    let selection = if selection.is_empty() { "1" } else { selection };

    match selection {
        "2" => {
            crate::setup::run_setup_with_options(
                config_path,
                SetupOptions {
                    profile: Some(parse_setup_profile("adaptive")?),
                    level: Some(parse_setup_level("custom")?),
                    secret_mode: None,
                    force: true,
                    prompt_for_secrets: true,
                },
            )?;
            println!("{}", tf("setup.onboarding_done_next", &[]));
            Ok(true)
        }
        "3" => {
            println!("{}", tf("setup.onboarding_skipped", &[]));
            println!("{}", tf("setup.onboarding_next", &[]));
            Ok(true)
        }
        _ => {
            crate::setup::run_setup_with_options(
                config_path,
                SetupOptions {
                    profile: Some(parse_setup_profile("adaptive")?),
                    level: Some(parse_setup_level("quick")?),
                    secret_mode: None,
                    force: true,
                    prompt_for_secrets: true,
                },
            )?;
            println!("{}", tf("setup.onboarding_done_next", &[]));
            Ok(true)
        }
    }
}

enum AiOnboardingState {
    MissingConfig,
    BlankConfig,
    InvalidConfig,
    NoAgents,
    AgentsNotReady,
}

impl AiOnboardingState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::MissingConfig => "missing_config",
            Self::BlankConfig => "blank_config",
            Self::InvalidConfig => "invalid_config",
            Self::NoAgents => "no_agents",
            Self::AgentsNotReady => "agents_not_ready",
        }
    }
}

fn detect_ai_onboarding_state(config_path: &std::path::Path) -> Result<Option<AiOnboardingState>> {
    if !config_path.exists() {
        return Ok(Some(AiOnboardingState::MissingConfig));
    }

    let content = fs::read_to_string(config_path)
        .with_context(|| format!("failed to read config file: {}", config_path.display()))?;
    if content.trim().is_empty() {
        return Ok(Some(AiOnboardingState::BlankConfig));
    }

    let root: toml::Value = match toml::from_str(&content) {
        Ok(value) => value,
        Err(_) => return Ok(Some(AiOnboardingState::InvalidConfig)),
    };

    let no_agents = root
        .get("agents")
        .and_then(|value| value.as_table())
        .map(|table| table.is_empty())
        .unwrap_or(true);
    if no_agents {
        return Ok(Some(AiOnboardingState::NoAgents));
    }

    let config = match AppConfig::load(config_path) {
        Ok(cfg) => cfg,
        Err(_) => return Ok(Some(AiOnboardingState::InvalidConfig)),
    };

    if config.agents.is_empty() {
        return Ok(Some(AiOnboardingState::NoAgents));
    }

    let ready = config
        .agents
        .keys()
        .filter(|name| is_agent_env_ready(&config, name))
        .count();

    if ready == 0 {
        return Ok(Some(AiOnboardingState::AgentsNotReady));
    }

    Ok(None)
}

async fn initialize_cache(
    config_path: PathBuf,
    cache_cfg: Option<crate::config::CacheConfig>,
) -> Result<Option<Arc<ResponseCache>>> {
    let Some(cache_cfg) = cache_cfg else {
        return Ok(None);
    };
    if !cache_cfg.enabled {
        return Ok(None);
    }
    tracing::trace!(config_path = %config_path.display(), "initializing response cache");

    // ── PostgreSQL backend (profile-multi-users-server) ──────────────────────
    #[cfg(feature = "backend-postgres")]
    {
        let url = cache_cfg.connection_string.clone().ok_or_else(|| {
            anyhow::anyhow!("cache.connection_string is required for profile-multi-users-server")
        })?;
        info!(
            "postgres cache enabled (ttl={}s, max_entries={})",
            cache_cfg.default_ttl_seconds, cache_cfg.max_entries
        );
        tokio::task::spawn_blocking(move || {
            ResponseCache::new(&url, cache_cfg.default_ttl_seconds, cache_cfg.max_entries)
                .map(Arc::new)
                .map(Some)
        })
        .await
        .map_err(|e| anyhow::anyhow!("cache init task join error: {}", e))?
    }

    // ── SQLite backend (profile-local / profile-simple-server) ───────────────
    #[cfg(not(feature = "backend-postgres"))]
    {
        let cache_path = resolve_config_relative_path(&config_path, &cache_cfg.path);
        info!(
            "sqlite cache enabled at {} (ttl={}s, max_entries={})",
            cache_path.display(),
            cache_cfg.default_ttl_seconds,
            cache_cfg.max_entries
        );

        let result = tokio::task::spawn_blocking(move || {
            ResponseCache::new(
                &cache_path,
                cache_cfg.default_ttl_seconds,
                cache_cfg.max_entries,
            )
            .map(Arc::new)
            .map(Some)
        })
        .await
        .map_err(|e| anyhow::anyhow!("cache init task join error: {}", e))?;

        // profile-local: cache init failure is non-fatal (adaptive behaviour).
        #[cfg(all(
            feature = "profile-local",
            not(feature = "profile-simple-server"),
            not(feature = "profile-multi-users-server")
        ))]
        {
            match result {
                Ok(cache) => Ok(cache),
                Err(e) => {
                    warn!(
                        "sqlite cache init failed (adaptive, continuing without cache): {}",
                        e
                    );
                    Ok(None)
                }
            }
        }

        #[cfg(any(
            feature = "profile-simple-server",
            feature = "profile-multi-users-server"
        ))]
        return result;
    }
}

async fn initialize_vector_store(
    config_path: PathBuf,
    vector_cfg: Option<crate::config::VectorConfig>,
) -> Result<Option<Arc<VectorStore>>> {
    let Some(vector_cfg) = vector_cfg else {
        return Ok(None);
    };
    if !vector_cfg.enabled {
        return Ok(None);
    }
    tracing::trace!(config_path = %config_path.display(), "initializing vector store");

    // ── PostgreSQL backend (profile-multi-users-server) ──────────────────────
    #[cfg(feature = "backend-postgres")]
    {
        let url = vector_cfg.connection_string.clone().ok_or_else(|| {
            anyhow::anyhow!("vector.connection_string is required for profile-multi-users-server")
        })?;
        info!(
            "postgres vector store enabled (dims={}, top_k={}, similarity={})",
            vector_cfg.dimensions, vector_cfg.top_k, vector_cfg.min_similarity
        );
        tokio::task::spawn_blocking(move || {
            VectorStore::new(&url, vector_cfg.dimensions, vector_cfg.max_entries)
                .map(Arc::new)
                .map(Some)
        })
        .await
        .map_err(|e| anyhow::anyhow!("vector init task join error: {}", e))?
    }

    // ── SQLite backend (profile-local / profile-simple-server) ───────────────
    #[cfg(not(feature = "backend-postgres"))]
    {
        let vector_path = resolve_config_relative_path(&config_path, &vector_cfg.path);
        info!(
            "sqlite vector store enabled at {} (dims={}, top_k={}, similarity={})",
            vector_path.display(),
            vector_cfg.dimensions,
            vector_cfg.top_k,
            vector_cfg.min_similarity
        );

        let result = tokio::task::spawn_blocking(move || {
            VectorStore::new(&vector_path, vector_cfg.dimensions, vector_cfg.max_entries)
                .map(Arc::new)
                .map(Some)
        })
        .await
        .map_err(|e| anyhow::anyhow!("vector init task join error: {}", e))?;

        // profile-local: vector init failure is non-fatal (adaptive behaviour).
        #[cfg(all(
            feature = "profile-local",
            not(feature = "profile-simple-server"),
            not(feature = "profile-multi-users-server")
        ))]
        {
            match result {
                Ok(store) => Ok(store),
                Err(e) => {
                    warn!(
                        "sqlite vector store init failed (adaptive, continuing without vector): {}",
                        e
                    );
                    Ok(None)
                }
            }
        }

        #[cfg(any(
            feature = "profile-simple-server",
            feature = "profile-multi-users-server"
        ))]
        return result;
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
    let mut cli = Cli::parse();
    if let Some(command) = cli.command.take() {
        match command {
            CliCommand::Init => cli.setup = true,
            CliCommand::Status => cli.status = true,
            CliCommand::Diagnose => cli.diagnose = true,
        }
    }

    // Configure telemetry (structured logging, metrics, tracing)
    let telemetry_config = telemetry_enhanced::TelemetryConfig {
        log_level: match cli.verbose {
            0 => "warn".to_string(),
            1 => "info".to_string(),
            2 => "debug".to_string(),
            _ => "trace".to_string(),
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
        Some(ref path) => path.clone(),
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

        // Start language file watcher for hot-reloading (best-effort)
        if let Err(e) =
            i18n_watcher::start_watcher(&languages_dir, std::time::Duration::from_secs(5))
        {
            warn!("Failed to start language file watcher: {}", e);
        } else {
            info!("Language file watcher started for hot-reloading");
        }
    }

    // Handle secret management commands, local model setup, and onboarding
    if handle_secret_commands(&cli, &config_path)? {
        return Ok(());
    }

    // Load, validate configuration, and handle validation-only modes
    let config = match handle_validation_mode(&cli, &config_path)? {
        Some(config) => config,
        None => return Ok(()),
    };

    // Check agent readiness — if no agents configured, prompt for setup or skip
    if !cli.setup
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && !std::env::args().any(|a| a == "--setup" || a == "--init")
        && std::env::var("GO_ON_ENABLE_LOCAL_TEST_AGENTS").is_err()
    {
        let provider_count = config.agents.len();
        let ready_count = config
            .agents
            .keys()
            .filter(|name| is_agent_env_ready(&config, name))
            .count();

        if provider_count == 0 {
            println!();
            println!("{}", tf("setup.onboarding_intro", &[]));
            println!("  {}", tf("setup.onboarding_option_1", &[]));
            println!("  {}", tf("setup.onboarding_option_3", &[]));
            print!("{} ", tf("setup.onboarding_select", &[]));
            std::io::Write::flush(&mut std::io::stdout()).ok();

            let mut input = String::new();
            std::io::stdin().read_line(&mut input).ok();
            let selection = input.trim();

            if selection == "1" || selection.is_empty() {
                crate::setup::run_setup_with_options(
                    &config_path,
                    SetupOptions {
                        profile: Some(parse_setup_profile("adaptive")?),
                        level: Some(parse_setup_level("quick")?),
                        secret_mode: None,
                        force: true,
                        prompt_for_secrets: true,
                    },
                )?;
                println!("{}", tf("setup.onboarding_done_next", &[]));
                // Reload config after setup
                let config = Arc::new(AppConfig::load(&config_path)?);
                return start_server(config, &cli, &config_path).await;
            } else {
                println!("{}", tf("setup.onboarding_skipped", &[]));
                println!("{}", tf("setup.onboarding_next", &[]));
            }
        } else if ready_count == 0 && provider_count > 0 {
            let missing: Vec<String> = config
                .agents
                .keys()
                .filter(|name| !is_agent_env_ready(&config, name))
                .cloned()
                .collect();
            println!();
            println!(
                "{} API key(s) not set: {}",
                missing.len(),
                missing.join(", ")
            );
            println!("  Run `go-on --init` to configure credentials, or continue without them.");
            print!("Press Enter to continue (or type 's' to run setup): ");
            std::io::Write::flush(&mut std::io::stdout()).ok();
            let mut input = String::new();
            std::io::stdin().read_line(&mut input).ok();
            if input.trim().eq_ignore_ascii_case("s") {
                crate::setup::run_setup_with_options(
                    &config_path,
                    SetupOptions {
                        profile: Some(parse_setup_profile("adaptive")?),
                        level: Some(parse_setup_level("quick")?),
                        secret_mode: None,
                        force: true,
                        prompt_for_secrets: true,
                    },
                )?;
                println!("{}", tf("setup.onboarding_done_next", &[]));
                let config = Arc::new(AppConfig::load(&config_path)?);
                return start_server(config, &cli, &config_path).await;
            }
        }
    }

    // Start the server
    start_server(config, &cli, &config_path).await
}

/// Handle secret management commands, local model setup, recommended config, setup wizard, and AI onboarding.
///
/// Returns `true` if a command was handled and `run()` should return early.
fn handle_secret_commands(cli: &Cli, config_path: &std::path::Path) -> Result<bool> {
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

    if maybe_prompt_ai_onboarding(cli, config_path)? {
        return Ok(true);
    }

    Ok(false)
}

/// Load and validate configuration, then handle validation-only modes (--validate-config, --diagnose).
///
/// Returns `Some(config)` if validation passed and the server should start,
/// or `None` if a validation-only command was handled and `run()` should return.
fn handle_validation_mode(
    cli: &Cli,
    config_path: &std::path::Path,
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

/// Start the server with the given configuration and CLI options.
async fn start_server(
    config: Arc<AppConfig>,
    cli: &Cli,
    config_path: &std::path::Path,
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
        initialize_cache(config_path.to_path_buf(), config.cache.clone()),
        initialize_vector_store(config_path.to_path_buf(), config.vector.clone()),
        initialize_autotune(config_path.to_path_buf(), config.autotune.clone()),
    )?;

    let ledger = ArtifactLedger::new(Some(config_path));

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

    match access_selection.configured_mode.as_str() {
        "adaptive" | "acp_stdio" => {
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
                cli.verbose > 0,
                Some(Arc::clone(&config)),
            );
            if matches!(access_selection.startup_transport, TransportMode::Stdio) {
                run_acp_server(&mut server).await
            } else {
                let bind_addr = acp_http_bind.unwrap_or_else(|| "127.0.0.1:8090".to_string());
                run_acp_http_server(Arc::new(server), bind_addr).await
            }
        }
        "acp_http" => {
            let bind_addr = acp_http_bind.unwrap_or_else(|| "127.0.0.1:8090".to_string());
            let server = new_acp_server(
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
                cli.verbose > 0,
                Some(Arc::clone(&config)),
            );
            run_acp_http_server(Arc::new(server), bind_addr).await
        }
        "mcp_stdio" => {
            let tool_registry = Arc::new(ToolRegistry::new());
            let acp_server = Arc::new(new_acp_server(
                flow,
                registry.clone(),
                cache,
                vector_store,
                config.vector.clone(),
                autotune_state,
                autotune_config,
                autotune_state_path,
                Some(config_path.to_string_lossy().to_string()),
                runtime_config,
                Some(http_client),
                cli.verbose > 0,
                Some(Arc::clone(&config)),
            ));
            let shutdown_notify = Arc::clone(&acp_server.shutdown_notify);
            if let Err(e) = start_background_tasks(&acp_server, Arc::clone(&shutdown_notify)).await
            {
                error!("Failed to start MCP background tasks: {}", e);
            }
            let server = McpStdioServer::new_with_acp(
                registry,
                tool_registry,
                "go-on".to_string(),
                env!("CARGO_PKG_VERSION").to_string(),
                Some(acp_server),
            );
            server.run().await
        }
        "mcp_http" => {
            let bind_addr = acp_http_bind.unwrap_or_else(|| "127.0.0.1:8090".to_string());
            let tool_registry = Arc::new(ToolRegistry::new());
            let acp_server = Arc::new(new_acp_server(
                flow,
                registry.clone(),
                cache,
                vector_store,
                config.vector.clone(),
                autotune_state,
                autotune_config,
                autotune_state_path,
                Some(config_path.to_string_lossy().to_string()),
                runtime_config,
                Some(http_client),
                cli.verbose > 0,
                Some(Arc::clone(&config)),
            ));
            let shutdown_notify = Arc::clone(&acp_server.shutdown_notify);
            if let Err(e) = start_background_tasks(&acp_server, shutdown_notify).await {
                error!("Failed to start MCP HTTP background tasks: {}", e);
            }
            let server = McpHttpServer::new_with_acp(
                registry,
                tool_registry,
                "go-on".to_string(),
                env!("CARGO_PKG_VERSION").to_string(),
                bind_addr,
                Some(acp_server),
            );
            server.run().await
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::{build_completeness_report, validate_cli_protocol_mode, RecommendationLevel};
    use crate::config::{
        AgentConfig, AppConfig, CacheConfig, FlowConfig, PhaseConfig, PhaseOptions, RuntimeConfig,
        VectorConfig,
    };
    use crate::reinforcement::{CheckStatus, ComponentReport, RuntimeHealthcheckReport};

    fn openai_config_with_inflight(
        phase_max_inflight: Option<i64>,
        global_max_inflight: Option<i64>,
    ) -> AppConfig {
        let mut agents = HashMap::new();
        agents.insert(
            "primary".to_string(),
            AgentConfig {
                agent_type: "openai".to_string(),
                url: Some("https://api.openai.com/v1".to_string()),
                chat_path: None,
                api_key_env: Some("OPENAI_API_KEY".to_string()),
                secret_key_env: None,
                anthropic_version: None,
                model: Some("gpt-4o-mini".to_string()),
                max_tokens: None,
                supports_system: Some(true),
            },
        );

        let mut coding_extra = HashMap::new();
        if let Some(value) = phase_max_inflight {
            coding_extra.insert("phase_max_inflight".to_string(), json!(value));
        }
        if let Some(value) = global_max_inflight {
            coding_extra.insert("global_max_inflight".to_string(), json!(value));
        }

        let mut phases = HashMap::new();
        phases.insert(
            "planning".to_string(),
            PhaseConfig {
                description: "planning".to_string(),
                agents: vec!["primary".to_string()],
                fallback: Some(true),
                principles: None,
                options: Some(PhaseOptions {
                    request_timeout_seconds: Some(120),
                    ..PhaseOptions::default()
                }),
            },
        );
        phases.insert(
            "coding".to_string(),
            PhaseConfig {
                description: "coding".to_string(),
                agents: vec!["primary".to_string()],
                fallback: Some(true),
                principles: None,
                options: Some(PhaseOptions {
                    request_timeout_seconds: Some(150),
                    review_timeout_seconds: Some(60),
                    extra: coding_extra,
                    ..PhaseOptions::default()
                }),
            },
        );
        phases.insert(
            "review".to_string(),
            PhaseConfig {
                description: "review".to_string(),
                agents: vec!["primary".to_string()],
                fallback: Some(true),
                principles: None,
                options: Some(PhaseOptions {
                    request_timeout_seconds: Some(60),
                    ..PhaseOptions::default()
                }),
            },
        );
        phases.insert(
            "delivery".to_string(),
            PhaseConfig {
                description: "delivery".to_string(),
                agents: vec!["primary".to_string()],
                fallback: Some(false),
                principles: None,
                options: Some(PhaseOptions {
                    request_timeout_seconds: Some(90),
                    ..PhaseOptions::default()
                }),
            },
        );

        AppConfig {
            default_phase: "coding".to_string(),
            agents,
            flow: FlowConfig {
                name: "flow".to_string(),
                phases: vec![
                    "planning".to_string(),
                    "coding".to_string(),
                    "review".to_string(),
                    "delivery".to_string(),
                ],
                workflow_type: crate::config::WorkflowType::Auto,
            },
            phases,
            runtime: Some(RuntimeConfig::default()),
            cache: Some(CacheConfig {
                enabled: true,
                path: "acp_cache.sqlite3".to_string(),
                default_ttl_seconds: 3600,
                max_entries: 5000,
                connection_string: None,
            }),
            vector: Some(VectorConfig {
                enabled: true,
                auto_mode: true,
                path: "acp_vector.sqlite3".to_string(),
                connection_string: None,
                dimensions: 192,
                min_query_chars: 80,
                top_k: 2,
                min_similarity: 0.82,
                max_snippet_chars: 800,
                max_entries: 10000,
                summary_enabled: true,
                summary_trigger_messages: 8,
                summary_max_chars: 1200,
            }),
            autotune: None,
            model_selection_mode: "adaptive".to_string(),
            compliance: None,
            startup_context: None,
            scheduler: None,
            reputation: None,
            role_registry: HashMap::new(),
        }
    }

    fn ready_report() -> RuntimeHealthcheckReport {
        RuntimeHealthcheckReport {
            generated_at: 0,
            overall_status: CheckStatus::Healthy,
            components: vec![ComponentReport {
                name: "provider_dependencies".to_string(),
                status: CheckStatus::Healthy,
                message: "ok".to_string(),
                details: json!({"ready": 1, "total": 1, "agents": []}),
            }],
        }
    }

    #[test]
    fn completeness_reports_inflight_recommendation_mismatch() {
        let cfg = openai_config_with_inflight(Some(8), Some(32));
        let report = build_completeness_report(&cfg, &ready_report());

        assert!(report.recommended.iter().any(|item| item
            .message
            .contains("phases.coding.options.phase_max_inflight recommended=")));
        assert!(report
            .recommended
            .iter()
            .any(|item| item.level == RecommendationLevel::Warning));
        assert!(report.recommended.iter().any(|item| item
            .message
            .contains("phases.coding.options.global_max_inflight recommended=")));
    }

    #[test]
    fn completeness_reports_missing_inflight_keys() {
        let cfg = openai_config_with_inflight(None, None);
        let report = build_completeness_report(&cfg, &ready_report());

        assert!(report
            .missing
            .iter()
            .any(|item| item == "phases.coding.options.phase_max_inflight"));
        assert!(report
            .missing
            .iter()
            .any(|item| item == "phases.coding.options.global_max_inflight"));
    }

    #[test]
    fn cli_protocol_mode_overrides_config() {
        let mut runtime_config = RuntimeConfig {
            protocol_mode: Some("adaptive".to_string()),
            ..RuntimeConfig::default()
        };

        if let Some(mode) = validate_cli_protocol_mode(Some("mcp_http")).unwrap() {
            runtime_config.protocol_mode = Some(mode);
        }

        assert_eq!(runtime_config.protocol_mode.as_deref(), Some("mcp_http"));
    }

    #[test]
    fn cli_protocol_mode_accepts_all_valid_values() {
        for mode in [
            "adaptive",
            "adap",
            "acp_stdio",
            "acp_http",
            "acp-http",
            "mcp_stdio",
            "mcp_http",
            "mcp-http",
            "auto",
            "acp",
            "mcp",
            "acp+http",
            "mcp+stdio",
        ] {
            assert!(
                validate_cli_protocol_mode(Some(mode)).is_ok(),
                "mode={mode}"
            );
        }
    }

    #[test]
    fn cli_protocol_mode_rejects_invalid_value() {
        let err = validate_cli_protocol_mode(Some("invalid_mode")).unwrap_err();
        assert!(err
            .to_string()
            .contains("invalid --protocol-mode 'invalid_mode'"));
    }

    #[test]
    fn cli_protocol_mode_rejects_ambiguous_prefix() {
        let err = validate_cli_protocol_mode(Some("acp_")).unwrap_err();
        assert!(err.to_string().contains("ambiguous --protocol-mode prefix"));
    }
}
