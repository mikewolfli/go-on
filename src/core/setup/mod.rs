//! setup.rs — prompts, config-gen, and secrets sub-modules.
//!
//! This module has grown large. Key sub-areas are:
//! - `secrets`   : keyring / env-based secret management
//! - `config_gen`: TOML config generation and recommendation
//! - `prompts`   : interactive setup prompt logic
//!
//! New code should live in the appropriate sub-module rather than in this file.
//!
//! GAP-B53-23: Extract sections from this file into the sub-module files.
//! The sub-module stubs (`config_gen.rs`, `prompts.rs`, `secrets.rs`) already
//! exist and re-export functions from here. The extraction is tracked under
//! GAP-B53-23 to keep the diff manageable for review.

pub mod config_gen;
pub mod prompts;
pub mod secrets;

// ---------------------------------------------------------------------------
// Re-export provider spec helpers from the single canonical location.
pub(crate) use crate::core::providers::{
    provider_spec_by_agent_type, provider_spec_by_name, provider_specs,
};

// Filename for the adaptive config template.
pub(crate) const ADAPTIVE_TEMPLATE: &str = "config.toml.autopilot-adaptive";

// Re-export config-gen types and functions moved to `config_gen` sub-module.
pub use config_gen::{
    apply_recommended_to_config, recommendation_snapshot_for_config, CustomAgentSpec,
    LocalModelOptions, ProviderRecommendationSnapshot,
};

// Re-export prompt functions.
pub use prompts::{
    add_local_model, parse_setup_level, parse_setup_profile, run_setup, run_setup_with_options,
};
pub(crate) use prompts::{
    ensure_table, prompt_secret_pool_deletion_selection, prompt_secret_pool_values, prompt_yes_no,
};

const KEYRING_SERVICE: &str = "go-on";

/// Setup profile mode: adaptive autopilot with AI-driven configuration.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SetupProfile {
    Adaptive,
}

/// Setup wizard level: quick, standard, or custom.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SetupLevel {
    Quick,
    Standard,
    Custom,
}

pub use secrets::{
    convert_env_placeholders_to_keyring, parse_secret_action, parse_secret_mode,
    run_secret_command, SecretAction, SecretMode,
};

/// Options controlling go-on setup behavior.
///
/// - `profile`: chosen setup profile; if None, user is prompted.
/// - `secret_mode`: how to store secrets; if None, user is prompted.
/// - `force`: overwrite existing config without prompting when true.
/// - `prompt_for_secrets`: whether to ask to set keyring secrets immediately.
pub struct SetupOptions {
    pub profile: Option<SetupProfile>,
    pub level: Option<SetupLevel>,
    pub secret_mode: Option<SecretMode>,
    pub force: bool,
    pub prompt_for_secrets: bool,
}

impl Default for SetupOptions {
    fn default() -> Self {
        Self {
            profile: None,
            level: None,
            secret_mode: None,
            force: false,
            prompt_for_secrets: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        apply_recommended_to_config, convert_env_placeholders_to_keyring, parse_secret_action,
        parse_secret_mode, parse_setup_level, parse_setup_profile, SecretAction, SecretMode,
        SetupProfile,
    };

    #[test]
    fn converts_known_env_vars_to_keyring_refs() {
        let input = "api_key_env = \"DEEPSEEK_API_KEY\"\nsecret_key_env = \"WENXIN_SECRET_KEY\"\n";
        let out = convert_env_placeholders_to_keyring(input);
        assert!(out.contains("keyring://go-on/deepseek_api_key"));
        assert!(out.contains("keyring://go-on/wenxin_secret_key"));
    }

    #[test]
    fn parses_setup_profile_secret_mode_and_action() {
        assert!(matches!(
            parse_setup_profile("adaptive").expect("adaptive profile should parse"),
            SetupProfile::Adaptive
        ));
        assert!(matches!(
            parse_setup_level("quick").expect("quick level should parse"),
            super::SetupLevel::Quick
        ));
        assert!(matches!(
            parse_setup_level("standard").expect("standard level should parse"),
            super::SetupLevel::Standard
        ));
        assert!(matches!(
            parse_setup_level("custom").expect("custom level should parse"),
            super::SetupLevel::Custom
        ));
        assert!(matches!(
            parse_secret_mode("auto").expect("auto secret mode should parse"),
            SecretMode::AutoDetect
        ));
        assert!(matches!(
            parse_secret_mode("env").expect("env secret mode should parse"),
            SecretMode::Env
        ));
        assert!(matches!(
            parse_secret_mode("keyring").expect("keyring secret mode should parse"),
            SecretMode::Keyring
        ));
        assert!(matches!(
            parse_secret_action("list").expect("list secret action should parse"),
            SecretAction::List
        ));
    }

    #[test]
    fn apply_recommended_creates_missing_phases_and_inflight_options() {
        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"default_phase = "coding"

[agents.primary]
type = "openai"
url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
model = "gpt-4o-mini"

[phases.coding]
description = "Coding phase"
agents = ["primary"]
fallback = true

[phases.coding.options]
request_timeout_seconds = 100
review_timeout_seconds = 40
"#,
        )
        .expect("config should be written");

        apply_recommended_to_config(&config_path)
            .expect("apply_recommended should handle missing phases");

        let updated = fs::read_to_string(&config_path).expect("updated config should be readable");
        let parsed: toml::Value = toml::from_str(&updated).expect("updated config should parse");
        let phases = parsed
            .get("phases")
            .and_then(|value| value.as_table())
            .expect("phases table should exist");
        assert!(phases.contains_key("planning"));
        assert!(phases.contains_key("review"));
        assert!(phases.contains_key("delivery"));

        let coding_options = phases
            .get("coding")
            .and_then(|value| value.as_table())
            .and_then(|phase| phase.get("options"))
            .and_then(|value| value.as_table())
            .expect("coding.options should exist");
        let phase_inflight = coding_options
            .get("phase_max_inflight")
            .and_then(|value| value.as_integer())
            .expect("phase_max_inflight should be written");
        let global_inflight = coding_options
            .get("global_max_inflight")
            .and_then(|value| value.as_integer())
            .expect("global_max_inflight should be written");
        assert!(phase_inflight > 0);
        assert!(global_inflight > 0);
    }
}
