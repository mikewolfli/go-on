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
// Legacy module content
use std::sync::OnceLock;

// Filename for the adaptive config template.
pub(crate) const ADAPTIVE_TEMPLATE: &str = "config.toml.autopilot-adaptive";

#[derive(Clone, Debug, serde::Deserialize)]
struct ProviderSpec {
    name: String,
    #[serde(rename = "type")]
    agent_type: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    chat_path: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    api_key_env: Option<String>,
    #[serde(default)]
    secret_key_env: Option<String>,
    #[serde(default)]
    anthropic_version: Option<String>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    supports_system: Option<bool>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    recommended_default_phase: Option<String>,
    #[serde(default)]
    recommended_request_timeout_seconds: Option<u64>,
    #[serde(default)]
    recommended_review_timeout_seconds: Option<u64>,
    #[serde(default)]
    recommended_cache_enabled: Option<bool>,
    #[serde(default)]
    recommended_vector_enabled: Option<bool>,
    #[serde(default)]
    recommended_phase_max_inflight: Option<usize>,
    #[serde(default)]
    recommended_global_max_inflight: Option<usize>,
    #[serde(default)]
    recommended_planning_request_timeout_seconds: Option<u64>,
    #[serde(default)]
    recommended_coding_request_timeout_seconds: Option<u64>,
    #[serde(default)]
    recommended_review_request_timeout_seconds: Option<u64>,
    #[serde(default)]
    recommended_delivery_request_timeout_seconds: Option<u64>,
}

static PROVIDER_SPECS: OnceLock<Vec<ProviderSpec>> = OnceLock::new();

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

fn provider_specs() -> &'static [ProviderSpec] {
    PROVIDER_SPECS
        .get_or_init(built_in_provider_specs)
        .as_slice()
}

fn built_in_provider_specs() -> Vec<ProviderSpec> {
    vec![
        ProviderSpec {
            name: "openai".to_string(),
            agent_type: "openai".to_string(),
            url: Some("https://api.openai.com/v1".to_string()),
            chat_path: None,
            model: Some("gpt-4o-mini".to_string()),
            api_key_env: Some("keyring://go-on/openai_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: Some(true),
            region: Some("Global".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(150),
            recommended_review_timeout_seconds: Some(60),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(24),
            recommended_global_max_inflight: Some(128),
            recommended_planning_request_timeout_seconds: Some(120),
            recommended_coding_request_timeout_seconds: Some(150),
            recommended_review_request_timeout_seconds: Some(60),
            recommended_delivery_request_timeout_seconds: Some(90),
        },
        ProviderSpec {
            name: "openai_compatible".to_string(),
            agent_type: "openai_compatible".to_string(),
            url: Some("http://127.0.0.1:8080/v1".to_string()),
            chat_path: None,
            model: Some("compatible-model".to_string()),
            api_key_env: Some("keyring://go-on/openai_compatible_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: Some(true),
            region: Some("Global".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(120),
            recommended_review_timeout_seconds: Some(60),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(100),
            recommended_coding_request_timeout_seconds: Some(120),
            recommended_review_request_timeout_seconds: Some(60),
            recommended_delivery_request_timeout_seconds: Some(90),
        },
        ProviderSpec {
            name: "anthropic".to_string(),
            agent_type: "claude".to_string(),
            url: Some("https://api.anthropic.com".to_string()),
            chat_path: None,
            model: Some("claude-sonnet-4-20250514".to_string()),
            api_key_env: Some("keyring://go-on/anthropic_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: Some("2023-06-01".to_string()),
            max_tokens: Some(8192),
            supports_system: None,
            region: Some("Global".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(180),
            recommended_review_timeout_seconds: Some(75),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(96),
            recommended_planning_request_timeout_seconds: Some(140),
            recommended_coding_request_timeout_seconds: Some(180),
            recommended_review_request_timeout_seconds: Some(75),
            recommended_delivery_request_timeout_seconds: Some(110),
        },
        ProviderSpec {
            name: "deepseek".to_string(),
            agent_type: "deepseek".to_string(),
            url: Some("https://api.deepseek.com".to_string()),
            chat_path: None,
            model: Some("deepseek-v4-flash".to_string()),
            api_key_env: Some("keyring://go-on/deepseek_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: Some(true),
            region: Some("Global".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(120),
            recommended_review_timeout_seconds: Some(60),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(32),
            recommended_global_max_inflight: Some(128),
            recommended_planning_request_timeout_seconds: Some(110),
            recommended_coding_request_timeout_seconds: Some(120),
            recommended_review_request_timeout_seconds: Some(60),
            recommended_delivery_request_timeout_seconds: Some(90),
        },
        ProviderSpec {
            name: "doubao".to_string(),
            agent_type: "doubao".to_string(),
            url: Some("https://ark.cn-beijing.volces.com/api/v3".to_string()),
            chat_path: Some("/chat/completions".to_string()),
            model: Some("doubao-1.5-pro-256k-250115".to_string()),
            api_key_env: Some("keyring://go-on/doubao_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: Some(true),
            region: Some("China".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(180),
            recommended_review_timeout_seconds: Some(90),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(24),
            recommended_global_max_inflight: Some(96),
            recommended_planning_request_timeout_seconds: Some(160),
            recommended_coding_request_timeout_seconds: Some(180),
            recommended_review_request_timeout_seconds: Some(90),
            recommended_delivery_request_timeout_seconds: Some(120),
        },
        ProviderSpec {
            name: "wenxin".to_string(),
            agent_type: "wenxin".to_string(),
            url: None,
            chat_path: None,
            model: Some("ERNIE-4.5-8K".to_string()),
            api_key_env: Some("keyring://go-on/wenxin_api_key".to_string()),
            secret_key_env: Some("keyring://go-on/wenxin_secret_key".to_string()),
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
            region: Some("China".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(180),
            recommended_review_timeout_seconds: Some(90),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(false),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(96),
            recommended_planning_request_timeout_seconds: Some(160),
            recommended_coding_request_timeout_seconds: Some(180),
            recommended_review_request_timeout_seconds: Some(90),
            recommended_delivery_request_timeout_seconds: Some(120),
        },
        ProviderSpec {
            name: "copilot".to_string(),
            agent_type: "copilot".to_string(),
            url: Some("http://127.0.0.1:8080".to_string()),
            chat_path: None,
            model: None,
            api_key_env: Some("keyring://go-on/copilot_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
            region: Some("Global".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(120),
            recommended_review_timeout_seconds: Some(60),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(24),
            recommended_global_max_inflight: Some(128),
            recommended_planning_request_timeout_seconds: Some(100),
            recommended_coding_request_timeout_seconds: Some(120),
            recommended_review_request_timeout_seconds: Some(60),
            recommended_delivery_request_timeout_seconds: Some(90),
        },
        ProviderSpec {
            name: "ai21".to_string(),
            agent_type: "ai21".to_string(),
            url: Some("https://api.ai21.com/studio/v1".to_string()),
            chat_path: None,
            model: Some("jamba-1.5-mini".to_string()),
            api_key_env: Some("keyring://go-on/ai21_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
            region: Some("Global".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(120),
            recommended_review_timeout_seconds: Some(60),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(100),
            recommended_coding_request_timeout_seconds: Some(120),
            recommended_review_request_timeout_seconds: Some(60),
            recommended_delivery_request_timeout_seconds: Some(90),
        },
        ProviderSpec {
            name: "aleph".to_string(),
            agent_type: "aleph".to_string(),
            url: Some("https://api.aleph-alpha.com".to_string()),
            chat_path: None,
            model: Some("luminous-base".to_string()),
            api_key_env: Some("keyring://go-on/aleph_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
            region: Some("Global".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(120),
            recommended_review_timeout_seconds: Some(60),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(100),
            recommended_coding_request_timeout_seconds: Some(120),
            recommended_review_request_timeout_seconds: Some(60),
            recommended_delivery_request_timeout_seconds: Some(90),
        },
        ProviderSpec {
            name: "cohere".to_string(),
            agent_type: "cohere".to_string(),
            url: Some("https://api.cohere.ai/v1".to_string()),
            chat_path: None,
            model: Some("command-r-plus-08-2024".to_string()),
            api_key_env: Some("keyring://go-on/cohere_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: Some(true),
            region: Some("Global".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(120),
            recommended_review_timeout_seconds: Some(60),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(100),
            recommended_coding_request_timeout_seconds: Some(120),
            recommended_review_request_timeout_seconds: Some(60),
            recommended_delivery_request_timeout_seconds: Some(90),
        },
        ProviderSpec {
            name: "gemini".to_string(),
            agent_type: "gemini".to_string(),
            url: Some("https://generativelanguage.googleapis.com/v1beta".to_string()),
            chat_path: None,
            model: Some("gemini-2.5-flash".to_string()),
            api_key_env: Some("keyring://go-on/gemini_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
            region: Some("Global".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(120),
            recommended_review_timeout_seconds: Some(60),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(100),
            recommended_coding_request_timeout_seconds: Some(120),
            recommended_review_request_timeout_seconds: Some(60),
            recommended_delivery_request_timeout_seconds: Some(90),
        },
        ProviderSpec {
            name: "groq".to_string(),
            agent_type: "groq".to_string(),
            url: Some("https://api.groq.com/openai/v1".to_string()),
            chat_path: None,
            model: Some("llama-3.3-70b-versatile".to_string()),
            api_key_env: Some("keyring://go-on/groq_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
            region: Some("Global".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(120),
            recommended_review_timeout_seconds: Some(60),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(24),
            recommended_global_max_inflight: Some(96),
            recommended_planning_request_timeout_seconds: Some(100),
            recommended_coding_request_timeout_seconds: Some(120),
            recommended_review_request_timeout_seconds: Some(60),
            recommended_delivery_request_timeout_seconds: Some(90),
        },
        ProviderSpec {
            name: "mistral".to_string(),
            agent_type: "mistral".to_string(),
            url: Some("https://api.mistral.ai/v1".to_string()),
            chat_path: None,
            model: Some("mistral-small-latest".to_string()),
            api_key_env: Some("keyring://go-on/mistral_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
            region: Some("Global".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(120),
            recommended_review_timeout_seconds: Some(60),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(100),
            recommended_coding_request_timeout_seconds: Some(120),
            recommended_review_request_timeout_seconds: Some(60),
            recommended_delivery_request_timeout_seconds: Some(90),
        },
        ProviderSpec {
            name: "qwen".to_string(),
            agent_type: "qwen".to_string(),
            url: Some("https://dashscope.aliyuncs.com/compatible-mode/v1".to_string()),
            chat_path: None,
            model: Some("qwen-max".to_string()),
            api_key_env: Some("keyring://go-on/qwen_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: Some(true),
            region: Some("China".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(180),
            recommended_review_timeout_seconds: Some(90),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(160),
            recommended_coding_request_timeout_seconds: Some(180),
            recommended_review_request_timeout_seconds: Some(90),
            recommended_delivery_request_timeout_seconds: Some(120),
        },
        ProviderSpec {
            name: "perplexity".to_string(),
            agent_type: "perplexity".to_string(),
            url: Some("https://api.perplexity.ai".to_string()),
            chat_path: None,
            model: Some("sonar-pro".to_string()),
            api_key_env: Some("keyring://go-on/perplexity_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
            region: Some("Global".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(120),
            recommended_review_timeout_seconds: Some(60),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(100),
            recommended_coding_request_timeout_seconds: Some(120),
            recommended_review_request_timeout_seconds: Some(60),
            recommended_delivery_request_timeout_seconds: Some(90),
        },
        ProviderSpec {
            name: "yi".to_string(),
            agent_type: "yi".to_string(),
            url: Some("https://api.lingyiwanwu.com/v1".to_string()),
            chat_path: None,
            model: Some("yi-lightning".to_string()),
            api_key_env: Some("keyring://go-on/yi_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
            region: Some("China".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(180),
            recommended_review_timeout_seconds: Some(90),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(160),
            recommended_coding_request_timeout_seconds: Some(180),
            recommended_review_request_timeout_seconds: Some(90),
            recommended_delivery_request_timeout_seconds: Some(120),
        },
        ProviderSpec {
            name: "moonshot".to_string(),
            agent_type: "moonshot".to_string(),
            url: Some("https://api.moonshot.cn/v1".to_string()),
            chat_path: None,
            model: Some("moonshot-v1-8k".to_string()),
            api_key_env: Some("keyring://go-on/moonshot_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
            region: Some("China".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(180),
            recommended_review_timeout_seconds: Some(90),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(160),
            recommended_coding_request_timeout_seconds: Some(180),
            recommended_review_request_timeout_seconds: Some(90),
            recommended_delivery_request_timeout_seconds: Some(120),
        },
        ProviderSpec {
            name: "glm".to_string(),
            agent_type: "glm".to_string(),
            url: Some("https://open.bigmodel.cn/api/paas/v4".to_string()),
            chat_path: None,
            model: Some("glm-4-flash".to_string()),
            api_key_env: Some("keyring://go-on/glm_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
            region: Some("China".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(180),
            recommended_review_timeout_seconds: Some(90),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(160),
            recommended_coding_request_timeout_seconds: Some(180),
            recommended_review_request_timeout_seconds: Some(90),
            recommended_delivery_request_timeout_seconds: Some(120),
        },
        ProviderSpec {
            name: "hunyuan".to_string(),
            agent_type: "hunyuan".to_string(),
            url: Some("https://api.hunyuan.cloud.tencent.com/v1".to_string()),
            chat_path: None,
            model: Some("hunyuan-turbo-latest".to_string()),
            api_key_env: Some("keyring://go-on/hunyuan_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
            region: Some("China".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(180),
            recommended_review_timeout_seconds: Some(90),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(160),
            recommended_coding_request_timeout_seconds: Some(180),
            recommended_review_request_timeout_seconds: Some(90),
            recommended_delivery_request_timeout_seconds: Some(120),
        },
        // ── Kimi / 月之暗面 ────────────────────────────────
        ProviderSpec {
            name: "kimi".to_string(),
            agent_type: "kimi".to_string(),
            url: Some("https://api.moonshot.cn/v1".to_string()),
            chat_path: None,
            model: Some("kimi-k2.6".to_string()),
            api_key_env: Some("keyring://go-on/kimi_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
            region: Some("China".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(180),
            recommended_review_timeout_seconds: Some(90),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(160),
            recommended_coding_request_timeout_seconds: Some(180),
            recommended_review_request_timeout_seconds: Some(90),
            recommended_delivery_request_timeout_seconds: Some(120),
        },
        ProviderSpec {
            name: "minimax".to_string(),
            agent_type: "minimax".to_string(),
            url: Some("https://api.minimax.chat/v1".to_string()),
            chat_path: None,
            model: Some("MiniMax-Text-01".to_string()),
            api_key_env: Some("keyring://go-on/minimax_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
            region: Some("China".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(180),
            recommended_review_timeout_seconds: Some(90),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(160),
            recommended_coding_request_timeout_seconds: Some(180),
            recommended_review_request_timeout_seconds: Some(90),
            recommended_delivery_request_timeout_seconds: Some(120),
        },
        // ── SiliconFlow / 硅基流动 ────────────────────────
        ProviderSpec {
            name: "siliconflow".to_string(),
            agent_type: "openai_compatible".to_string(),
            url: Some("https://api.siliconflow.cn/v1".to_string()),
            chat_path: None,
            model: Some("deepseek-ai/DeepSeek-V3.2".to_string()),
            api_key_env: Some("keyring://go-on/siliconflow_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: Some(true),
            region: Some("China".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(120),
            recommended_review_timeout_seconds: Some(60),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(32),
            recommended_global_max_inflight: Some(128),
            recommended_planning_request_timeout_seconds: Some(100),
            recommended_coding_request_timeout_seconds: Some(120),
            recommended_review_request_timeout_seconds: Some(60),
            recommended_delivery_request_timeout_seconds: Some(90),
        },
        ProviderSpec {
            name: "stepfun".to_string(),
            agent_type: "stepfun".to_string(),
            url: Some("https://api.stepfun.com/v1".to_string()),
            chat_path: None,
            model: Some("step-2-16k".to_string()),
            api_key_env: Some("keyring://go-on/stepfun_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
            region: Some("China".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(180),
            recommended_review_timeout_seconds: Some(90),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(160),
            recommended_coding_request_timeout_seconds: Some(180),
            recommended_review_request_timeout_seconds: Some(90),
            recommended_delivery_request_timeout_seconds: Some(120),
        },
        ProviderSpec {
            name: "together".to_string(),
            agent_type: "together".to_string(),
            url: Some("https://api.together.xyz/v1".to_string()),
            chat_path: None,
            model: Some("meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo".to_string()),
            api_key_env: Some("keyring://go-on/together_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
            region: Some("Global".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(120),
            recommended_review_timeout_seconds: Some(60),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(100),
            recommended_coding_request_timeout_seconds: Some(120),
            recommended_review_request_timeout_seconds: Some(60),
            recommended_delivery_request_timeout_seconds: Some(90),
        },
        // ── X.AI / Grok ──────────────────────────────────────
        ProviderSpec {
            name: "xai".to_string(),
            agent_type: "openai_compatible".to_string(),
            url: Some("https://api.x.ai/v1".to_string()),
            chat_path: None,
            model: Some("grok-3".to_string()),
            api_key_env: Some("keyring://go-on/xai_api_key".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: Some(true),
            region: Some("Global".to_string()),
            recommended_default_phase: Some("coding".to_string()),
            recommended_request_timeout_seconds: Some(120),
            recommended_review_timeout_seconds: Some(60),
            recommended_cache_enabled: Some(true),
            recommended_vector_enabled: Some(true),
            recommended_phase_max_inflight: Some(16),
            recommended_global_max_inflight: Some(64),
            recommended_planning_request_timeout_seconds: Some(100),
            recommended_coding_request_timeout_seconds: Some(120),
            recommended_review_request_timeout_seconds: Some(60),
            recommended_delivery_request_timeout_seconds: Some(90),
        },
    ]
}

fn provider_spec_by_name(name: &str) -> Option<&'static ProviderSpec> {
    provider_specs().iter().find(|spec| spec.name == name)
}

fn provider_spec_by_agent_type(agent_type: &str) -> Option<&'static ProviderSpec> {
    provider_specs()
        .iter()
        .find(|spec| spec.agent_type.eq_ignore_ascii_case(agent_type))
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
            parse_setup_profile("adaptive").unwrap(),
            SetupProfile::Adaptive
        ));
        assert!(matches!(
            parse_setup_level("quick").unwrap(),
            super::SetupLevel::Quick
        ));
        assert!(matches!(
            parse_setup_level("standard").unwrap(),
            super::SetupLevel::Standard
        ));
        assert!(matches!(
            parse_setup_level("custom").unwrap(),
            super::SetupLevel::Custom
        ));
        assert!(matches!(
            parse_secret_mode("auto").unwrap(),
            SecretMode::AutoDetect
        ));
        assert!(matches!(parse_secret_mode("env").unwrap(), SecretMode::Env));
        assert!(matches!(
            parse_secret_mode("keyring").unwrap(),
            SecretMode::Keyring
        ));
        assert!(matches!(
            parse_secret_action("list").unwrap(),
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
