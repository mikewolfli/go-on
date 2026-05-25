//! Agent onboarding — interactive terminal prompts for first-time setup.
//! Detects agent configuration state and guides the user through setup.

use crate::config::{is_agent_env_ready, AppConfig};
use crate::i18n::runtime::tf;
use crate::setup::{parse_setup_level, parse_setup_profile, SetupOptions};
use anyhow::{Context, Result};
use std::path::Path;
use tracing::info;

/// Configuration for the onboarding flow.
pub struct OnboardingConfig {
    pub enabled: bool,
    /// Whether we're in a terminal that supports interactive prompts.
    pub is_terminal: bool,
}

impl Default for OnboardingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            is_terminal: false,
        }
    }
}

/// State detected from the config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiOnboardingState {
    MissingConfig,
    BlankConfig,
    InvalidConfig,
    NoAgents,
    AgentsNotReady,
}

impl AiOnboardingState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MissingConfig => "missing_config",
            Self::BlankConfig => "blank_config",
            Self::InvalidConfig => "invalid_config",
            Self::NoAgents => "no_agents",
            Self::AgentsNotReady => "agents_not_ready",
        }
    }
}

/// Detect the current AI onboarding state from the config file.
fn detect_ai_onboarding_state(config_path: &Path) -> Result<Option<AiOnboardingState>> {
    let content = match std::fs::read_to_string(config_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some(AiOnboardingState::MissingConfig));
        }
        Err(e) => {
            return Err(e)
                .with_context(|| format!("failed to read config file: {}", config_path.display()))
        }
    };
    if content.trim().is_empty() {
        return Ok(Some(AiOnboardingState::BlankConfig));
    }

    let root: toml::Value = match toml::from_str(&content) {
        Ok(val) => val,
        Err(_) => return Ok(Some(AiOnboardingState::InvalidConfig)),
    };

    let no_agents = root
        .get("agents")
        .and_then(|v| v.as_table())
        .map(|t| t.is_empty())
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

/// Run the interactive onboarding flow if needed.
/// Returns `true` if setup was completed and server should restart, `false` if skipped.
pub async fn run_onboarding(config: &OnboardingConfig, config_path: &Path) -> Result<bool> {
    if !config.enabled {
        return Ok(false);
    }

    let Some(state) = detect_ai_onboarding_state(config_path)? else {
        return Ok(false);
    };

    // Non-terminal mode: log a single clear message instead of spamming warnings
    if !config.is_terminal {
        let msg = match state {
            AiOnboardingState::MissingConfig | AiOnboardingState::BlankConfig => {
                format!(
                    "no config found at {} — run `go-on --init` or use GUI",
                    config_path.display()
                )
            }
            AiOnboardingState::NoAgents => {
                format!(
                    "config at {} has no AI providers — run `go-on --init`",
                    config_path.display()
                )
            }
            AiOnboardingState::AgentsNotReady => {
                format!(
                    "config at {} has providers but API keys missing — run `go-on --init`",
                    config_path.display()
                )
            }
            AiOnboardingState::InvalidConfig => {
                format!(
                    "config at {} is invalid — run `go-on --validate-config`",
                    config_path.display()
                )
            }
        };
        info!("{msg}");
        return Ok(false);
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
