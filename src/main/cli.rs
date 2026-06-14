use std::path::PathBuf;

use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand};

use crate::shared::protocol_mode::{ProtocolMode, ProtocolModeError};

pub(crate) fn validate_cli_protocol_mode(raw: Option<&str>) -> Result<Option<String>> {
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
#[command(version = "1.2.0")]
#[command(about = "ACP proxy with flow, phases and multi-agent routing")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<CliCommand>,

    /// Path to configuration file
    #[arg(short = 'c', long)]
    pub config: Option<PathBuf>,

    /// Phase to run
    #[arg(long)]
    pub phase: Option<String>,

    /// Enable verbose logging
    #[arg(short = 'v', long, action = ArgAction::Count)]
    pub verbose: u8,

    /// Validate configuration and exit
    #[arg(long, visible_alias = "doctor", default_value_t = false)]
    pub validate_config: bool,

    /// Run end-to-end diagnosis and print concise remediation output
    #[arg(long, default_value_t = false)]
    pub diagnose: bool,

    /// Run setup wizard
    #[arg(long, visible_alias = "init", default_value_t = false)]
    pub setup: bool,

    /// Setup profile to use
    #[arg(long)]
    pub setup_profile: Option<String>,

    /// Setup wizard level to use (quick|standard|custom)
    #[arg(long)]
    pub setup_level: Option<String>,

    /// Secret mode for setup
    #[arg(long)]
    pub setup_secrets: Option<String>,

    /// Add or update a local model agent entry in config
    #[arg(long, visible_alias = "add-model", default_value_t = false)]
    pub add_local_model: bool,

    /// Local model agent name when using --add-model
    #[arg(long)]
    pub local_model_name: Option<String>,

    /// Local model endpoint URL when using --add-model
    #[arg(long)]
    pub local_model_url: Option<String>,

    /// Local model provider type when using --add-model (default: openai)
    #[arg(long)]
    pub local_model_type: Option<String>,

    /// Local model model-id when using --add-model
    #[arg(long)]
    pub local_model_model: Option<String>,

    /// Optional API key env var field for local model when using --add-model
    #[arg(long)]
    pub local_model_api_key_env: Option<String>,

    /// Optional secret key env var field for local model when using --add-model
    #[arg(long)]
    pub local_model_secret_key_env: Option<String>,

    /// Only register local model under [agents], do not auto-attach it to phase agent lists
    #[arg(long, default_value_t = false)]
    pub local_model_register_only: bool,

    /// Apply provider capability recommendations to current config.toml and exit
    #[arg(long, default_value_t = false)]
    pub apply_recommended: bool,

    /// Force setup even if files exist
    #[arg(long, default_value_t = false)]
    pub force: bool,

    /// Secret management action
    #[arg(long)]
    pub secret: Option<String>,

    /// Secret name for management
    #[arg(long)]
    pub secret_name: Option<String>,

    /// Secret value for management
    #[arg(long)]
    pub secret_value: Option<String>,

    /// Generate a runtime healthcheck report and persist it into .goon/
    #[arg(long, default_value_t = false)]
    pub healthcheck: bool,

    /// Run action checks (all/spec/qa/retest/final) against .goon/ artifacts
    #[arg(long)]
    pub action_check: Option<String>,

    /// Build and persist a controlled task plan artifact for a complex task
    #[arg(long)]
    pub plan_task: Option<String>,

    /// Print configured AI providers and runtime readiness status
    #[arg(long, visible_alias = "check", default_value_t = false)]
    pub status: bool,

    /// Bind ACP HTTP server and expose /health, /chat, and /chat/stream
    #[arg(short = 'b', long, visible_alias = "bind")]
    pub acp_http_bind: Option<String>,

    /// Access protocol mode override (adaptive|acp_stdio|acp_http|mcp_stdio|mcp_http)
    #[arg(short = 'm', long, visible_alias = "mode", value_name = "MODE")]
    pub protocol_mode: Option<String>,

    /// Start interactive terminal chat session (like Claude Code / Codex)
    #[arg(short = 'a', long, default_value_t = false)]
    pub chat: bool,

    /// Enable low-memory mode: reduce cache/vector/inflight limits to
    /// absolute minimum to avoid OOM killer (SIGKILL) on memory-constrained systems.
    #[arg(long, default_value_t = false)]
    pub low_memory: bool,
}

#[derive(Debug, Subcommand)]
pub enum CliCommand {
    /// Generate default configuration interactively
    Init,
    /// Print runtime readiness status
    Status,
    /// Run end-to-end diagnosis with remediation hints
    Diagnose,
}
