use std::path::PathBuf;

use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand};

use std::sync::Arc;

use crate::orchestration::skill::SkillRegistry;
use crate::orchestration::skill_market::SkillMarketRegistry;
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
    /// Manage skills (marketplace, import, list)
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Start Hub daemon (distributed memory / multi-process mode)
    #[cfg(feature = "sub-bus-distributed-memory")]
    Hub {
        /// Port to bind (0 = auto-assign)
        #[arg(long, default_value_t = 0)]
        port: u16,
    },
}

#[derive(Debug, Subcommand)]
pub enum SkillCommand {
    /// List available skills from the marketplace
    List {
        /// Optional filter by tag
        #[arg(long)]
        tag: Option<String>,
    },
    /// Search marketplace skills by name or description
    Search {
        /// Search query
        query: String,
    },
    /// List skills installed from the marketplace
    ListInstalled,
    /// Enable a marketplace-installed skill
    Enable {
        /// Name of the skill to enable
        name: String,
    },
    /// Disable a marketplace-installed skill
    Disable {
        /// Name of the skill to disable
        name: String,
    },
    /// Import a skill from a source
    Import {
        /// Source: "github:owner/repo" | "url:<url>" | "local:<path>"
        source: String,
    },
    /// List imported/registered skills
    ListImported,
    /// Show detailed info about a specific installed skill
    Info {
        /// Name of the skill to inspect
        name: String,
    },
    /// Trigger an immediate rescan of ~/.agents/skills/ for new or modified skills
    Refresh,
    /// Remove a skill from the registry
    Remove {
        /// Name of the skill to remove
        name: String,
    },
}

/// Handle the `skill` CLI subcommand by creating in-process registries
/// and performing the requested action.
pub async fn handle_skill_command(cmd: SkillCommand) -> anyhow::Result<()> {
    // Create a minimal SkillRegistry for local/imported skills
    let skill_registry = Arc::new(std::sync::RwLock::new(SkillRegistry::default()));

    // Create a minimal SkillMarketRegistry for marketplace skills
    let market_registry = SkillMarketRegistry::new(
        "https://marketplace.go-on.dev",
        std::env::temp_dir().join("go-on-skill-market"),
        skill_registry.clone(),
    )
    .map_err(|e| anyhow::anyhow!("failed to create skill market registry: {}", e))?;

    match cmd {
        SkillCommand::List { tag } => {
            // Refresh the marketplace to get built-in sample skills
            let count = market_registry.refresh().await?;
            let skills = match tag {
                Some(ref t) => market_registry.list_skills_by_tag(t).await,
                None => market_registry.list_skills().await,
            };
            println!(
                "Available skills ({} total, {} matching):",
                count,
                skills.len()
            );
            for skill in &skills {
                let tags = skill.tags.join(", ");
                let installs = market_registry.get_install_count(&skill.name).await;
                println!(
                    "  {:<20} v{:<8} [{:>5.1}] installs:{:<4} {:<40} tags: {}",
                    skill.name, skill.version, skill.rating, installs, skill.description, tags,
                );
            }
            if skills.is_empty() {
                println!(
                    "  (no skills found{})",
                    tag.map_or(String::new(), |t| format!(" for tag '{}'", t))
                );
            }
        }
        SkillCommand::Enable { name } => {
            market_registry.refresh().await?;
            market_registry.set_enabled(&name, true).await?;
            println!("Skill '{}' enabled", name);
        }
        SkillCommand::Disable { name } => {
            market_registry.refresh().await?;
            market_registry.set_enabled(&name, false).await?;
            println!("Skill '{}' disabled", name);
        }
        SkillCommand::Import { source } => {
            println!("Importing skill from: {}", source);
            // Refresh the marketplace to populate built-in skills
            market_registry.refresh().await?;
            // Attempt to install from the marketplace by name
            match market_registry.install_skill(&source).await {
                Ok(installation) => {
                    println!(
                        "Skill '{}' v{} imported successfully",
                        installation.name, installation.version
                    );
                    println!("  Installed at: {}", installation.installed_path.display());
                }
                Err(e) => {
                    anyhow::bail!("Failed to import skill '{}': {}", source, e);
                }
            }
        }
        SkillCommand::Search { query } => {
            market_registry.refresh().await?;
            let results = market_registry.search_skills(&query).await;
            println!("Search results for '{}' ({}):", query, results.len());
            for skill in &results {
                let tags = skill.tags.join(", ");
                let installs = market_registry.get_install_count(&skill.name).await;
                println!(
                    "  {:<20} v{:<8} [{:>5.1}] installs:{:<4} {:<40} tags: {}",
                    skill.name, skill.version, skill.rating, installs, skill.description, tags,
                );
            }
            if results.is_empty() {
                println!("  (no matching skills)");
            }
        }
        SkillCommand::ListInstalled => {
            let installed = market_registry.list_installed().await;
            println!("Marketplace-installed skills ({}):", installed.len());
            for inst in &installed {
                println!(
                    "  {:<20} v{:<8} enabled:{}  {}",
                    inst.name,
                    inst.version,
                    inst.enabled,
                    inst.installed_path.display()
                );
            }
            if installed.is_empty() {
                println!("  (no marketplace-installed skills)");
            }
        }
        SkillCommand::ListImported => {
            // List skills registered in the local SkillRegistry
            let descriptors = skill_registry
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .list(false);
            println!("Registered skills ({}):", descriptors.len());
            for desc in &descriptors {
                println!(
                    "  {:<20}  score: {:>5.2}  calls: {}  {}",
                    desc.name, desc.score, desc.total_calls, desc.description,
                );
            }
            if descriptors.is_empty() {
                println!("  (no skills registered)");
            }
        }
        SkillCommand::Info { name } => {
            let registry = skill_registry.read().unwrap_or_else(|e| e.into_inner());
            match registry.descriptor(&name) {
                Some(desc) => {
                    println!("Skill: {}", desc.name);
                    println!("  Description: {}", desc.description);
                    println!("  Score: {:.2}", desc.score);
                    println!("  Total calls: {}", desc.total_calls);
                    println!("  Successful calls: {}", desc.success_calls);
                    println!("  Failed calls: {}", desc.failure_calls);
                    println!("  Avg latency: {:.1} ms", desc.average_latency_ms);
                    println!("  Input schema: {}", desc.input_schema);
                }
                None => {
                    anyhow::bail!("skill '{}' not found in registry", name);
                }
            }
        }
        SkillCommand::Refresh => {
            let mut registry = skill_registry.write().unwrap_or_else(|e| e.into_inner());
            match registry.discover_and_register_local_skills(None) {
                Ok(summary) => {
                    println!(
                        "Skill refresh complete: {} registered, {} skipped, {} errors",
                        summary.registered,
                        summary.skipped,
                        summary.errors.len()
                    );
                    for err in &summary.errors {
                        println!("  Error: {}", err);
                    }
                }
                Err(e) => {
                    anyhow::bail!("Failed to refresh skills: {}", e);
                }
            }
        }
        SkillCommand::Remove { name } => {
            // Remove from the local registry first.
            let removed = {
                let mut registry = skill_registry.write().unwrap_or_else(|e| e.into_inner());
                registry.unregister(&name)
            };
            if removed {
                println!("Skill '{}' removed from registry", name);
            }
            // Also uninstall from the marketplace if present.
            if market_registry.is_installed(&name).await {
                market_registry.uninstall_skill(&name).await?;
                println!("Skill '{}' uninstalled from marketplace", name);
            } else if !removed {
                anyhow::bail!("skill '{}' not found in registry or marketplace", name);
            }
        }
    }
    Ok(())
}
