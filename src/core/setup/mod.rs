//! setup.rs — prompts, config-gen, and secrets sub-modules.
//!
//! This module has grown large. Key sub-areas are:
//! - `secrets`   : keyring / env-based secret management
//! - `config_gen`: TOML config generation and recommendation
//! - `prompts`   : interactive setup prompt logic
//!
//! New code should live in the appropriate sub-module rather than in this file.
//!
//! TODO(GAP-B53-23): Extract sections into the sub-module files below.

pub mod config_gen;
pub mod prompts;
pub mod secrets;

// ---------------------------------------------------------------------------
// Legacy module content
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::config::AdaptiveConfig;
use crate::i18n::runtime::{t, tf};
use anyhow::{Context, Result};

// Filename for the adaptive config template.
const ADAPTIVE_TEMPLATE: &str = "config.toml.autopilot-adaptive";

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

/// Specification for a custom agent entered by the user during interactive setup.
#[derive(Clone, Debug)]
pub struct CustomAgentSpec {
    pub name: String,
    pub agent_type: String,
    pub url: Option<String>,
    pub api_key_env: Option<String>,
    pub secret_key_env: Option<String>,
    pub model: Option<String>,
}

#[derive(Clone, Debug)]
pub struct LocalModelOptions {
    pub name: Option<String>,
    pub url: Option<String>,
    pub agent_type: Option<String>,
    pub model: Option<String>,
    pub api_key_env: Option<String>,
    pub secret_key_env: Option<String>,
    pub apply_to_phases: bool,
}

#[derive(Clone, Debug)]
pub struct ProviderRecommendationSnapshot {
    pub default_phase: String,
    pub planning_request_timeout_seconds: u64,
    pub coding_request_timeout_seconds: u64,
    pub review_request_timeout_seconds: u64,
    pub delivery_request_timeout_seconds: u64,
    pub coding_review_timeout_seconds: u64,
    pub cache_enabled: bool,
    pub vector_enabled: bool,
    pub phase_max_inflight: usize,
    pub global_max_inflight: usize,
}

#[derive(Clone, Debug)]
struct ProviderRecommendations {
    default_phase: Option<String>,
    planning_request_timeout_seconds: u64,
    coding_request_timeout_seconds: u64,
    review_request_timeout_seconds: u64,
    delivery_request_timeout_seconds: u64,
    coding_review_timeout_seconds: u64,
    cache_enabled: bool,
    vector_enabled: bool,
    phase_max_inflight: usize,
    global_max_inflight: usize,
}

impl Default for ProviderRecommendations {
    fn default() -> Self {
        Self {
            default_phase: None,
            planning_request_timeout_seconds: 120,
            coding_request_timeout_seconds: 150,
            review_request_timeout_seconds: 60,
            delivery_request_timeout_seconds: 90,
            coding_review_timeout_seconds: 60,
            cache_enabled: true,
            vector_enabled: true,
            phase_max_inflight: 24,
            global_max_inflight: 128,
        }
    }
}

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

/// Secret storage mode for setup: environment variables or system keyring.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SecretMode {
    Env,
    Keyring,
    AutoDetect,
}

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

fn aggregate_provider_recommendations(providers: &[String]) -> ProviderRecommendations {
    let mut rec = ProviderRecommendations::default();
    let mut cache_votes: Vec<bool> = Vec::new();
    let mut vector_votes: Vec<bool> = Vec::new();

    for provider in providers {
        let Some(spec) = provider_spec_by_name(provider) else {
            continue;
        };

        if rec.default_phase.is_none() {
            rec.default_phase = spec.recommended_default_phase.clone();
        }
        if let Some(timeout) = spec.recommended_request_timeout_seconds {
            rec.coding_request_timeout_seconds = rec.coding_request_timeout_seconds.max(timeout);
        }
        if let Some(timeout) = spec.recommended_review_timeout_seconds {
            rec.coding_review_timeout_seconds = rec.coding_review_timeout_seconds.max(timeout);
        }
        if let Some(timeout) = spec
            .recommended_planning_request_timeout_seconds
            .or(spec.recommended_request_timeout_seconds)
        {
            rec.planning_request_timeout_seconds =
                rec.planning_request_timeout_seconds.max(timeout);
        }
        if let Some(timeout) = spec
            .recommended_coding_request_timeout_seconds
            .or(spec.recommended_request_timeout_seconds)
        {
            rec.coding_request_timeout_seconds = rec.coding_request_timeout_seconds.max(timeout);
        }
        if let Some(timeout) = spec
            .recommended_review_request_timeout_seconds
            .or(spec.recommended_review_timeout_seconds)
            .or(spec.recommended_request_timeout_seconds)
        {
            rec.review_request_timeout_seconds = rec.review_request_timeout_seconds.max(timeout);
        }
        if let Some(timeout) = spec
            .recommended_delivery_request_timeout_seconds
            .or(spec.recommended_request_timeout_seconds)
        {
            rec.delivery_request_timeout_seconds =
                rec.delivery_request_timeout_seconds.max(timeout);
        }
        if let Some(cache_enabled) = spec.recommended_cache_enabled {
            cache_votes.push(cache_enabled);
        }
        if let Some(vector_enabled) = spec.recommended_vector_enabled {
            vector_votes.push(vector_enabled);
        }
        if let Some(max_inflight) = spec.recommended_phase_max_inflight {
            rec.phase_max_inflight = rec.phase_max_inflight.min(max_inflight.max(1));
        }
        if let Some(max_inflight) = spec.recommended_global_max_inflight {
            rec.global_max_inflight = rec.global_max_inflight.min(max_inflight.max(1));
        }
    }

    if !cache_votes.is_empty() {
        rec.cache_enabled = cache_votes.iter().any(|v| *v);
    }
    if !vector_votes.is_empty() {
        rec.vector_enabled = vector_votes.iter().any(|v| *v);
    }

    rec
}

pub fn recommendation_snapshot_for_config(
    config: &crate::config::AppConfig,
) -> Option<ProviderRecommendationSnapshot> {
    let mut provider_names: HashSet<String> = HashSet::new();
    for agent in config.agents.values() {
        if let Some(spec) = provider_spec_by_agent_type(agent.agent_type.as_str()) {
            provider_names.insert(spec.name.clone());
        }
    }

    if provider_names.is_empty() {
        return None;
    }

    let mut providers = provider_names.into_iter().collect::<Vec<_>>();
    providers.sort();
    let rec = aggregate_provider_recommendations(&providers);
    Some(ProviderRecommendationSnapshot {
        default_phase: rec.default_phase.unwrap_or_else(|| "coding".to_string()),
        planning_request_timeout_seconds: rec.planning_request_timeout_seconds,
        coding_request_timeout_seconds: rec.coding_request_timeout_seconds,
        review_request_timeout_seconds: rec.review_request_timeout_seconds,
        delivery_request_timeout_seconds: rec.delivery_request_timeout_seconds,
        coding_review_timeout_seconds: rec.coding_review_timeout_seconds,
        cache_enabled: rec.cache_enabled,
        vector_enabled: rec.vector_enabled,
        phase_max_inflight: rec.phase_max_inflight,
        global_max_inflight: rec.global_max_inflight,
    })
}

pub fn apply_recommended_to_config(config_path: &Path) -> Result<()> {
    if !config_path.exists() {
        anyhow::bail!("config file does not exist: {}", config_path.display());
    }

    let content = fs::read_to_string(config_path)
        .with_context(|| format!("failed to read config file: {}", config_path.display()))?;
    let mut root: toml::Value = toml::from_str(&content)
        .with_context(|| format!("failed to parse toml: {}", config_path.display()))?;

    let table = root
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("root toml is not table"))?;

    let provider_names = collect_provider_names_from_toml(table);
    if provider_names.is_empty() {
        anyhow::bail!("no supported provider found in [agents], cannot apply recommendations");
    }
    let recommendations = aggregate_provider_recommendations(&provider_names);

    table.insert(
        "default_phase".to_string(),
        toml::Value::String(
            recommendations
                .default_phase
                .clone()
                .unwrap_or_else(|| "coding".to_string()),
        ),
    );

    let cache = ensure_table(table, "cache")?;
    cache.insert(
        "enabled".to_string(),
        toml::Value::Boolean(recommendations.cache_enabled),
    );

    let vector = ensure_table(table, "vector")?;
    vector.insert(
        "enabled".to_string(),
        toml::Value::Boolean(recommendations.vector_enabled),
    );

    let agent_names = table
        .get("agents")
        .and_then(|value| value.as_table())
        .map(|agents| agents.keys().cloned().collect::<Vec<String>>())
        .unwrap_or_default();
    let phases = ensure_table(table, "phases")?;
    let mut created_phases = Vec::new();

    for (phase_name, timeout) in [
        ("planning", recommendations.planning_request_timeout_seconds),
        ("coding", recommendations.coding_request_timeout_seconds),
        ("review", recommendations.review_request_timeout_seconds),
        ("delivery", recommendations.delivery_request_timeout_seconds),
    ] {
        if phases
            .get(phase_name)
            .and_then(|value| value.as_table())
            .is_none()
        {
            created_phases.push(phase_name.to_string());
            phases.insert(
                phase_name.to_string(),
                toml::Value::Table(default_phase_table(phase_name, &agent_names)),
            );
        }

        let Some(phase) = phases
            .get_mut(phase_name)
            .and_then(|value| value.as_table_mut())
        else {
            continue;
        };
        let options = ensure_table(phase, "options")?;
        options.insert(
            "request_timeout_seconds".to_string(),
            toml::Value::Integer(timeout as i64),
        );
    }

    if let Some(coding_phase) = phases
        .get_mut("coding")
        .and_then(|value| value.as_table_mut())
    {
        let options = ensure_table(coding_phase, "options")?;
        options.insert(
            "review_timeout_seconds".to_string(),
            toml::Value::Integer(recommendations.coding_review_timeout_seconds as i64),
        );
        options.insert(
            "cache_enabled".to_string(),
            toml::Value::Boolean(recommendations.cache_enabled),
        );
        options.insert(
            "vector_enabled".to_string(),
            toml::Value::Boolean(recommendations.vector_enabled),
        );
        options.insert(
            "summary_enabled".to_string(),
            toml::Value::Boolean(recommendations.vector_enabled),
        );
        options.insert(
            "phase_max_inflight".to_string(),
            toml::Value::Integer(recommendations.phase_max_inflight as i64),
        );
        options.insert(
            "global_max_inflight".to_string(),
            toml::Value::Integer(recommendations.global_max_inflight as i64),
        );
    }

    let output = toml::to_string_pretty(&root).context("failed to serialize updated config")?;
    fs::write(config_path, output)
        .with_context(|| format!("failed to write config file: {}", config_path.display()))?;

    println!(
        "{}",
        tf(
            "setup.recommendations_applied",
            &[("path", &config_path.to_string_lossy())]
        )
    );
    if !created_phases.is_empty() {
        println!(
            "{}",
            tf(
                "setup.created_phases",
                &[("phases", &created_phases.join(", "))]
            )
        );
    }
    Ok(())
}

fn default_phase_table(
    phase_name: &str,
    agent_names: &[String],
) -> toml::map::Map<String, toml::Value> {
    let mut table = toml::map::Map::new();
    table.insert(
        "description".to_string(),
        toml::Value::String(format!("Auto-created {} phase", phase_name)),
    );
    table.insert(
        "fallback".to_string(),
        toml::Value::Boolean(phase_name != "delivery"),
    );
    let agents = if agent_names.is_empty() {
        vec!["copilot".to_string()]
    } else {
        agent_names.to_vec()
    };
    table.insert(
        "agents".to_string(),
        toml::Value::Array(agents.into_iter().map(toml::Value::String).collect()),
    );
    table
}

fn collect_provider_names_from_toml(table: &toml::map::Map<String, toml::Value>) -> Vec<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    let Some(agents) = table.get("agents").and_then(|value| value.as_table()) else {
        return Vec::new();
    };

    for (agent_name, value) in agents {
        let Some(agent_table) = value.as_table() else {
            continue;
        };
        let agent_type = agent_table
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or_default();

        if let Some(spec) = provider_spec_by_agent_type(agent_type) {
            names.insert(spec.name.clone());
            continue;
        }
        if let Some(spec) = provider_spec_by_name(agent_name.as_str()) {
            names.insert(spec.name.clone());
        }
    }

    names.into_iter().collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SecretAction {
    Set,
    Get,
    Delete,
    List,
}

/// Run setup with default options.
///
/// This helper is a thin wrapper around `run_setup_with_options`.
#[must_use]
#[allow(clippy::double_must_use)]
pub fn run_setup(config_path: &Path) -> Result<()> {
    run_setup_with_options(config_path, SetupOptions::default())
}

/// Entry point for setup logic.
///
/// Handles profile selection, secret mode, writing config template, writing RULES files,
/// and optionally storing secrets into keyring.
#[must_use]
#[allow(clippy::double_must_use)]
pub fn run_setup_with_options(config_path: &Path, options: SetupOptions) -> Result<()> {
    println!("{}", t("setup.title"));
    println!(
        "{}",
        tf(
            "setup.target_config",
            &[("path", &config_path.display().to_string())]
        )
    );

    if config_path.exists()
        && !options.force
        && !prompt_yes_no(&t("setup.prompt_overwrite"), false)?
    {
        println!("{}", t("setup.canceled"));
        return Ok(());
    }

    let _profile = options.profile.unwrap_or(SetupProfile::Adaptive);
    let setup_level = match options.level {
        Some(level) => level,
        None => prompt_setup_level()?,
    };
    let template_name = ADAPTIVE_TEMPLATE;

    let _template_path = find_template(template_name)
        .ok_or_else(|| anyhow::anyhow!("template file '{}' not found", template_name))?;

    let secret_mode = match options.secret_mode {
        Some(value) => value,
        None => {
            // Auto-detect: use Env mode if env vars are already set, otherwise prompt.
            let has_env_vars = !detect_available_providers_from_env().is_empty();
            if has_env_vars {
                println!("{}", t("setup.auto_detected_env_vars"));
                SecretMode::Env
            } else {
                match prompt_choice(&t("setup.prompt_secret_mode"), &["1", "2", "3"], "3")?.as_str()
                {
                    "1" => SecretMode::Env,
                    "2" => SecretMode::Keyring,
                    _ => SecretMode::AutoDetect,
                }
            }
        }
    };

    // Detect available AI providers.
    let detected_providers = detect_available_providers(&secret_mode);
    let available_providers = prompt_provider_selection(&detected_providers, setup_level)?;
    // Quick mode: skip extra-agent prompt to keep the flow minimal.
    let custom_agents = if setup_level == SetupLevel::Quick {
        Vec::new()
    } else {
        prompt_additional_agents()?
    };

    let mut adaptive_config = AdaptiveConfig::auto_detect();
    apply_setup_level_to_config(&mut adaptive_config, setup_level)?;
    if available_providers.is_empty() {
        anyhow::bail!("{}", t("setup.provider_selection_required"));
    }
    println!("{}", t("setup.detected_providers"));
    for provider in &available_providers {
        println!("  - {}", provider);
    }
    adaptive_config.minimal_config.available_providers = available_providers;

    let mut content = generate_adaptive_config_toml(&adaptive_config, &secret_mode, &custom_agents);

    // If using keyring mode, convert env-var placeholders to keyring references.
    if secret_mode == SecretMode::Keyring {
        content = convert_env_placeholders_to_keyring(&content);
    }

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory: {}", parent.display()))?;
    }
    fs::write(config_path, content)
        .with_context(|| format!("failed to write config file: {}", config_path.display()))?;

    write_default_rules(config_path.parent().unwrap_or_else(|| Path::new(".")))?;

    let should_store_secrets = match secret_mode {
        SecretMode::Keyring if options.prompt_for_secrets => {
            prompt_yes_no(&t("setup.prompt_store_secrets"), true)?
        }
        SecretMode::Keyring => false,

        SecretMode::AutoDetect => {
            // Auto-detect mode: ask whether to set up API keys now.
            prompt_yes_no(&t("setup.prompt_setup_api_keys"), true)?
        }
        _ => false,
    };

    if should_store_secrets {
        store_keyring_secrets_interactive(
            &adaptive_config.minimal_config.available_providers,
            &custom_agents,
        )?;
    }

    println!("{}", t("setup.complete"));
    println!(
        "{}",
        tf(
            "setup.next_step",
            &[("config", &config_path.to_string_lossy())]
        )
    );
    Ok(())
}

/// Parse setup profile string to SetupProfile enum.
///
/// Accepts case-insensitive "adaptive".
pub fn parse_setup_profile(value: &str) -> Result<SetupProfile> {
    if value.eq_ignore_ascii_case("adaptive") {
        return Ok(SetupProfile::Adaptive);
    }
    anyhow::bail!("{}", tf("error.invalid_setup_profile", &[("value", value)]))
}

/// Parse setup level string to SetupLevel enum.
/// Accepts case-insensitive quick|standard|custom.
pub fn parse_setup_level(value: &str) -> Result<SetupLevel> {
    if value.eq_ignore_ascii_case("quick") {
        return Ok(SetupLevel::Quick);
    }
    if value.eq_ignore_ascii_case("standard") {
        return Ok(SetupLevel::Standard);
    }
    if value.eq_ignore_ascii_case("custom") {
        return Ok(SetupLevel::Custom);
    }
    anyhow::bail!("{}", tf("error.invalid_setup_level", &[("value", value)]))
}

/// Parse secret mode string to SecretMode enum.
///
/// Accepts case-insensitive "env", "keyring", or "auto".
pub fn parse_secret_mode(value: &str) -> Result<SecretMode> {
    if value.eq_ignore_ascii_case("env") {
        return Ok(SecretMode::Env);
    }
    if value.eq_ignore_ascii_case("keyring") {
        return Ok(SecretMode::Keyring);
    }
    if value.eq_ignore_ascii_case("auto") || value.eq_ignore_ascii_case("autodetect") {
        return Ok(SecretMode::AutoDetect);
    }
    anyhow::bail!("{}", tf("error.invalid_secret_mode", &[("value", value)]))
}

/// Parse secret action string to SecretAction enum.
///
/// Accepts case-insensitive set|get|delete|list.
pub fn parse_secret_action(value: &str) -> Result<SecretAction> {
    if value.eq_ignore_ascii_case("set") {
        return Ok(SecretAction::Set);
    }
    if value.eq_ignore_ascii_case("get") {
        return Ok(SecretAction::Get);
    }
    if value.eq_ignore_ascii_case("delete") {
        return Ok(SecretAction::Delete);
    }
    if value.eq_ignore_ascii_case("list") {
        return Ok(SecretAction::List);
    }
    anyhow::bail!("{}", tf("error.invalid_secret_action", &[("value", value)]))
}

pub fn run_secret_command(
    action: SecretAction,
    name: Option<&str>,
    value: Option<&str>,
) -> Result<()> {
    match action {
        SecretAction::List => {
            for (name, service, account) in secret_targets() {
                let entry = keyring::Entry::new(&service, &account)
                    .map_err(|err| anyhow::anyhow!("failed to open keyring entry: {}", err))?;
                match entry.get_password() {
                    Ok(secret) => {
                        let count = parse_secret_pool_entries(&secret).len();
                        println!("{}: present ({} key(s))", name, count);
                    }
                    Err(_) => println!("{}: missing", name),
                }
            }
            Ok(())
        }
        SecretAction::Set => {
            let (service, account) = resolve_secret_target(name)?;
            let value =
                value.ok_or_else(|| anyhow::anyhow!("{}", t("error.secret_value_required")))?;
            let entry = keyring::Entry::new(&service, &account).map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.keyring_open", &[("error", &format!("{}", err))])
                )
            })?;
            entry.set_password(value).map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.keyring_write", &[("error", &format!("{}", err))])
                )
            })?;
            println!(
                "{}",
                tf("setup.secret_stored", &[("name", name.unwrap_or_default())])
            );
            Ok(())
        }
        SecretAction::Get => {
            let (service, account) = resolve_secret_target(name)?;
            let entry = keyring::Entry::new(&service, &account).map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.keyring_open", &[("error", &format!("{}", err))])
                )
            })?;
            let secret = entry.get_password().map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.keyring_read", &[("error", &format!("{}", err))])
                )
            })?;
            println!("{}", mask_secret_pool_entry(&secret));
            Ok(())
        }
        SecretAction::Delete => {
            let (service, account) = resolve_secret_target(name)?;
            let entry = keyring::Entry::new(&service, &account).map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.keyring_open", &[("error", &format!("{}", err))])
                )
            })?;
            let current = entry.get_password().map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.keyring_read", &[("error", &format!("{}", err))])
                )
            })?;
            let mut values = parse_secret_pool_entries(&current);
            if values.is_empty() {
                anyhow::bail!("secret pool is empty");
            }

            if let Some(selector) = value {
                if let Ok(index) = selector.parse::<usize>() {
                    if index == 0 || index > values.len() {
                        anyhow::bail!("invalid secret pool index {}", index);
                    }
                    values.remove(index - 1);
                } else {
                    let position = values
                        .iter()
                        .position(|item| item == selector)
                        .ok_or_else(|| anyhow::anyhow!("secret pool item not found"))?;
                    values.remove(position);
                }
            } else {
                let secret_name = name.unwrap_or_default();
                if values.len() == 1 {
                    if !prompt_yes_no(
                        &format!(
                            "Delete the only key for {} ({})?",
                            secret_name,
                            mask_secret_pool_entry(&values[0])
                        ),
                        false,
                    )? {
                        println!("Canceled.");
                        return Ok(());
                    }
                    values.clear();
                } else {
                    let Some(index) = prompt_secret_pool_deletion_selection(secret_name, &values)?
                    else {
                        println!("Canceled.");
                        return Ok(());
                    };
                    values.remove(index);
                }
            }

            if values.is_empty() {
                entry.delete_credential().map_err(|err| {
                    anyhow::anyhow!(
                        "{}",
                        tf("error.keyring_delete", &[("error", &format!("{}", err))])
                    )
                })?;
            } else {
                entry
                    .set_password(&join_secret_pool_entries(&values))
                    .map_err(|err| {
                        anyhow::anyhow!(
                            "{}",
                            tf("error.keyring_write", &[("error", &format!("{}", err))])
                        )
                    })?;
            }
            println!(
                "{}",
                tf(
                    "setup.secret_deleted",
                    &[("name", name.unwrap_or_default())]
                )
            );
            Ok(())
        }
    }
}

fn detect_available_providers(secret_mode: &SecretMode) -> Vec<String> {
    let env_providers = detect_available_providers_from_env();
    let keyring_providers = detect_available_providers_from_keyring();

    let mut providers = Vec::new();
    for spec in provider_specs() {
        let provider = spec.name.as_str();

        let include = match secret_mode {
            SecretMode::Env => env_providers.iter().any(|item| item == provider),
            SecretMode::Keyring => keyring_providers.iter().any(|item| item == provider),
            SecretMode::AutoDetect => {
                env_providers.iter().any(|item| item == provider)
                    || keyring_providers.iter().any(|item| item == provider)
            }
        };

        if include {
            providers.push((*provider).to_string());
        }
    }

    providers
}

/// Interactively ask the user whether they want to add any custom agents beyond the
/// catalog providers.  Returns a (possibly empty) list of `CustomAgentSpec` values.
fn prompt_additional_agents() -> Result<Vec<CustomAgentSpec>> {
    const KNOWN_TYPES: &[&str] = &[
        "openai",
        "anthropic",
        "gemini",
        "deepseek",
        "groq",
        "glm",
        "doubao",
        "wenxin",
        "hunyuan",
        "kimi",
        "qwen",
        "moonshot",
        "mistral",
        "llama",
        "copilot",
        "openai_compatible",
        "siliconflow",
    ];

    if !prompt_yes_no(
        "Add extra agents beyond the catalog above? (e.g. self-hosted / local models) [n]",
        false,
    )? {
        return Ok(Vec::new());
    }

    let mut agents: Vec<CustomAgentSpec> = Vec::new();

    loop {
        println!(
            "\n{}",
            tf(
                "cli.custom_agent_title",
                &[("name", &(agents.len() + 1).to_string())]
            )
        );

        // Name
        let name = loop {
            let raw = prompt_value(&format!("  {}", t("cli.agent_name_prompt")))?;
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                println!("  {}", t("cli.agent_name_required"));
                continue;
            }
            if trimmed.contains(|c: char| c.is_whitespace()) {
                println!("  {}", t("cli.agent_name_no_spaces"));
                continue;
            }
            break trimmed;
        };

        // Type
        println!(
            "  {}",
            tf(
                "cli.agent_type_available",
                &[("types", &KNOWN_TYPES.join(", "))]
            )
        );
        let agent_type = loop {
            let raw = prompt_value(&format!("  {}", t("cli.agent_type_prompt")))?;
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                break "openai_compatible".to_string();
            }
            if KNOWN_TYPES.contains(&trimmed.as_str()) {
                break trimmed;
            }
            println!(
                "  {}",
                tf(
                    "cli.agent_type_unknown",
                    &[("types", &KNOWN_TYPES.join(", "))]
                )
            );
        };

        // URL (required for non-managed types)
        let url = {
            let raw = prompt_value(&format!("  {}", t("cli.base_url_prompt")))?;
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        };

        // API key env var
        let api_key_env = {
            let raw = prompt_value(&format!("  {}", t("cli.api_key_env_prompt")))?;
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        };

        // Secret key env var (e.g. for providers that need two keys)
        let secret_key_env = {
            let raw = prompt_value(&format!("  {}", t("cli.secret_key_env_prompt")))?;
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        };

        // Model
        let model = {
            let raw = prompt_value(&format!("  {}", t("cli.model_name_prompt")))?;
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        };

        agents.push(CustomAgentSpec {
            name,
            agent_type,
            url,
            api_key_env,
            secret_key_env,
            model,
        });

        if !prompt_yes_no("Add another custom agent?", false)? {
            break;
        }
    }

    Ok(agents)
}

/// Prompt the user to select one or more providers.
/// - Quick level: flat numbered list, no region step.
/// - Standard / Custom level: two-step region → provider flow.
fn prompt_provider_selection(
    detected_providers: &[String],
    setup_level: SetupLevel,
) -> Result<Vec<String>> {
    if matches!(setup_level, SetupLevel::Quick) {
        return prompt_provider_selection_quick(detected_providers);
    }
    prompt_provider_selection_full(detected_providers)
}

/// Flat provider picker for Quick setup — no region step, single numbered list.
fn prompt_provider_selection_quick(detected_providers: &[String]) -> Result<Vec<String>> {
    let specs = provider_specs();
    loop {
        println!("\n{}", t("cli.select_provider"));
        println!();
        for (i, spec) in specs.iter().enumerate() {
            let mark = if detected_providers.contains(&spec.name) {
                t("cli.detected_marker")
            } else {
                "".to_string()
            };
            let region = spec.region.as_deref().unwrap_or("Other");
            println!("  {:>2}. {}{}  [{}]", i + 1, spec.name, mark, region);
        }
        if !detected_providers.is_empty() {
            let default_nums: Vec<String> = detected_providers
                .iter()
                .filter_map(|p| {
                    specs
                        .iter()
                        .position(|s| &s.name == p)
                        .map(|i| (i + 1).to_string())
                })
                .collect();
            println!("\n  {} {}", t("cli.detected_note"), default_nums.join(","));
            print!(
                "\n{} [{}]: ",
                t("cli.enter_numbers"),
                default_nums.join(",")
            );
        } else {
            print!("\n{}: ", t("cli.enter_numbers"));
        }
        io::stdout().flush().context("failed to flush stdout")?;
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read input")?;
        let value = input.trim();
        if value.is_empty() {
            if detected_providers.is_empty() {
                println!("  {}", t("cli.provider_required"));
                continue;
            }
            return Ok(detected_providers.to_vec());
        }
        if value.eq_ignore_ascii_case("all") {
            return Ok(specs.iter().map(|s| s.name.clone()).collect());
        }
        let mut selected: Vec<String> = Vec::new();
        let mut invalid = None;
        for token in value.split(',') {
            let raw = token.trim();
            if raw.is_empty() {
                continue;
            }
            if let Ok(idx) = raw.parse::<usize>() {
                if idx >= 1 && idx <= specs.len() {
                    let name = specs[idx - 1].name.clone();
                    if !selected.contains(&name) {
                        selected.push(name);
                    }
                    continue;
                }
            }
            if let Some(spec) = specs.iter().find(|s| s.name.eq_ignore_ascii_case(raw)) {
                if !selected.contains(&spec.name) {
                    selected.push(spec.name.clone());
                }
            } else {
                invalid = Some(raw.to_string());
                break;
            }
        }
        if let Some(bad) = invalid {
            println!("  {}", tf("cli.invalid_selection", &[("value", &bad)]));
            continue;
        }
        if selected.is_empty() {
            println!("  {}", t("cli.provider_required"));
            continue;
        }
        return Ok(selected);
    }
}

fn prompt_provider_selection_full(detected_providers: &[String]) -> Result<Vec<String>> {
    const REGION_ORDER: &[&str] = &["Global", "China", "Europe", "Local", "Other"];

    let specs = provider_specs();

    // Build canonical ordered region list
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut ordered_regions: Vec<String> = Vec::new();
    for &r in REGION_ORDER {
        if specs
            .iter()
            .any(|s| s.region.as_deref().unwrap_or("Other") == r)
            && seen.insert(r.to_string())
        {
            ordered_regions.push(r.to_string());
        }
    }
    for spec in specs.iter() {
        let r = spec.region.as_deref().unwrap_or("Other").to_string();
        if seen.insert(r.clone()) {
            ordered_regions.push(r);
        }
    }

    // ── Step 1: select region(s) ──────────────────────────────────────────
    let selected_regions: Vec<String> = loop {
        println!("\nStep 1/2 — Select region(s)");
        println!();
        for (i, region) in ordered_regions.iter().enumerate() {
            let region_specs: Vec<_> = specs
                .iter()
                .filter(|s| s.region.as_deref().unwrap_or("Other") == region.as_str())
                .collect();
            let det_count = region_specs
                .iter()
                .filter(|s| detected_providers.contains(&s.name))
                .count();
            let preview: Vec<&str> = region_specs
                .iter()
                .take(4)
                .map(|s| s.name.as_str())
                .collect();
            let mut preview_str = preview.join(", ");
            if region_specs.len() > 4 {
                preview_str.push_str(&format!(", ... ({} total)", region_specs.len()));
            }
            let det_mark = if det_count > 0 {
                format!(" [{} detected *]", det_count)
            } else {
                String::new()
            };
            println!("  {:>2}. {}{}  — {}", i + 1, region, det_mark, preview_str);
        }
        if !detected_providers.is_empty() {
            println!("\n  (* = detected from environment / keyring)");
        }

        // Build default hint from detected providers' regions
        let auto_regions: Vec<String> = {
            let mut v: Vec<String> = Vec::new();
            for p in detected_providers {
                if let Some(spec) = specs.iter().find(|s| &s.name == p) {
                    let r = spec.region.as_deref().unwrap_or("Other").to_string();
                    if !v.contains(&r) {
                        v.push(r);
                    }
                }
            }
            v
        };
        let default_hint = if auto_regions.is_empty() {
            "all".to_string()
        } else {
            auto_regions
                .iter()
                .filter_map(|r| {
                    ordered_regions
                        .iter()
                        .position(|x| x == r)
                        .map(|i| (i + 1).to_string())
                })
                .collect::<Vec<_>>()
                .join(",")
        };

        print!(
            "\nEnter region numbers (e.g. 1,3) or \"all\" [{}]: ",
            default_hint
        );
        io::stdout().flush().context("failed to flush stdout")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read input")?;
        let value = input.trim();

        if value.is_empty() {
            if auto_regions.is_empty() {
                break ordered_regions.clone();
            } else {
                break auto_regions;
            }
        }
        if value.eq_ignore_ascii_case("all") {
            break ordered_regions.clone();
        }

        let mut chosen: Vec<String> = Vec::new();
        let mut invalid = None;
        for token in value.split(',') {
            let raw = token.trim();
            if raw.is_empty() {
                continue;
            }
            if let Ok(idx) = raw.parse::<usize>() {
                if idx >= 1 && idx <= ordered_regions.len() {
                    let r = ordered_regions[idx - 1].clone();
                    if !chosen.contains(&r) {
                        chosen.push(r);
                    }
                    continue;
                }
            }
            if let Some(r) = ordered_regions.iter().find(|r| r.eq_ignore_ascii_case(raw)) {
                if !chosen.contains(r) {
                    chosen.push(r.clone());
                }
            } else {
                invalid = Some(raw.to_string());
                break;
            }
        }
        if let Some(bad) = invalid {
            println!("  Invalid region: '{}'. Try again.", bad);
            continue;
        }
        if chosen.is_empty() {
            println!("  At least one region is required.");
            continue;
        }
        break chosen;
    };

    // ── Step 2: select provider(s) within chosen region(s) ───────────────
    loop {
        println!(
            "\nStep 2/2 — Select providers from: {}",
            selected_regions.join(", ")
        );
        println!();

        let mut index_map: Vec<String> = Vec::new();
        for region in &selected_regions {
            let mut first_in_region = true;
            for spec in specs
                .iter()
                .filter(|s| s.region.as_deref().unwrap_or("Other") == region.as_str())
            {
                if first_in_region {
                    println!("  [{}]", region);
                    first_in_region = false;
                }
                index_map.push(spec.name.clone());
                let mark = if detected_providers.contains(&spec.name) {
                    " *"
                } else {
                    ""
                };
                println!("    {:>2}. {}{}", index_map.len(), spec.name, mark);
            }
        }

        let scoped_detected: Vec<String> = detected_providers
            .iter()
            .filter(|p| index_map.contains(p))
            .cloned()
            .collect();

        if !scoped_detected.is_empty() {
            println!(
                "\n  (* = detected. Default: {})",
                scoped_detected.join(", ")
            );
        }

        let default_hint = if scoped_detected.is_empty() {
            "enter numbers or \"all\"".to_string()
        } else {
            scoped_detected
                .iter()
                .filter_map(|p| {
                    index_map
                        .iter()
                        .position(|x| x == p)
                        .map(|i| (i + 1).to_string())
                })
                .collect::<Vec<_>>()
                .join(",")
        };

        print!("\nSelect providers [{}]: ", default_hint);
        io::stdout().flush().context("failed to flush stdout")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read input")?;
        let value = input.trim();

        if value.is_empty() {
            if scoped_detected.is_empty() {
                println!("  At least one provider is required.");
                continue;
            }
            return Ok(scoped_detected);
        }
        if value.eq_ignore_ascii_case("all") {
            return Ok(index_map);
        }

        let mut selected: Vec<String> = Vec::new();
        let mut invalid = None;
        for token in value.split(',') {
            let raw = token.trim();
            if raw.is_empty() {
                continue;
            }
            if let Ok(idx) = raw.parse::<usize>() {
                if idx >= 1 && idx <= index_map.len() {
                    selected.push(index_map[idx - 1].clone());
                    continue;
                }
                invalid = Some(raw.to_string());
                break;
            }
            if index_map.contains(&raw.to_string()) {
                selected.push(raw.to_string());
            } else {
                invalid = Some(raw.to_string());
                break;
            }
        }
        if let Some(bad) = invalid {
            println!(
                "{}",
                tf(
                    "error.invalid_provider_selection",
                    &[("value", bad.as_str())]
                )
            );
            continue;
        }

        selected.sort();
        selected.dedup();

        if selected.is_empty() {
            println!("{}", t("setup.provider_selection_required"));
            continue;
        }

        return Ok(selected);
    }
}

fn prompt_setup_level() -> Result<SetupLevel> {
    let selected = prompt_choice(&t("setup.prompt_level"), &["1", "2", "3"], "2")?;
    match selected.as_str() {
        "1" => Ok(SetupLevel::Quick),
        "2" => Ok(SetupLevel::Standard),
        _ => Ok(SetupLevel::Custom),
    }
}

fn apply_setup_level_to_config(config: &mut AdaptiveConfig, level: SetupLevel) -> Result<()> {
    match level {
        SetupLevel::Quick => {
            config.minimal_config.default_phase = "coding".to_string();
            config.minimal_config.enable_cache = true;
            config.minimal_config.enable_vector_memory = false;
            println!("{}", t("setup.level_quick_applied"));
        }
        SetupLevel::Standard => {
            config.minimal_config.default_phase = "coding".to_string();
            config.minimal_config.enable_cache = true;
            config.minimal_config.enable_vector_memory = true;
            println!("{}", t("setup.level_standard_applied"));
        }
        SetupLevel::Custom => {
            config.minimal_config.default_phase = prompt_choice(
                &t("setup.prompt_default_phase"),
                &["planning", "coding", "review", "delivery"],
                "coding",
            )?;
            config.minimal_config.enable_cache =
                prompt_yes_no(&t("setup.prompt_enable_cache"), true)?;
            config.minimal_config.enable_vector_memory =
                prompt_yes_no(&t("setup.prompt_enable_vector"), true)?;
            println!("{}", t("setup.level_custom_applied"));
        }
    }
    Ok(())
}

fn required_envs_for_provider(provider: &str) -> Vec<String> {
    let Some(spec) = provider_spec_by_name(provider) else {
        return Vec::new();
    };
    let mut envs = Vec::new();
    if let Some(api) = spec.api_key_env.as_ref() {
        envs.push(api.clone());
    }
    if let Some(secret) = spec.secret_key_env.as_ref() {
        envs.push(secret.clone());
    }
    envs
}

fn detect_available_providers_from_env() -> Vec<String> {
    provider_specs()
        .iter()
        .filter(|spec| {
            let required_envs = required_envs_for_provider(spec.name.as_str());
            !required_envs.is_empty()
                && required_envs.iter().all(|name| std::env::var(name).is_ok())
        })
        .map(|spec| spec.name.clone())
        .collect()
}

fn detect_available_providers_from_keyring() -> Vec<String> {
    provider_specs()
        .iter()
        .filter(|spec| {
            let required_envs = required_envs_for_provider(spec.name.as_str());
            !required_envs.is_empty()
                && required_envs
                    .iter()
                    .all(|env_name| keyring_secret_available(env_name))
        })
        .map(|spec| spec.name.clone())
        .collect()
}

fn keyring_secret_available(env_name: &str) -> bool {
    let Some((service, account)) = keyring_target_for_env(env_name) else {
        return false;
    };

    keyring::Entry::new(&service, &account)
        .and_then(|entry| entry.get_password())
        .is_ok()
}

fn keyring_account_for_env(env_name: &str) -> String {
    match env_name {
        "GITHUB_COPILOT_TOKEN" => "github_copilot_token".to_string(),
        _ => env_name.to_ascii_lowercase(),
    }
}

fn provider_secret_env_names() -> Vec<String> {
    let mut env_names = BTreeSet::new();
    for spec in provider_specs() {
        if let Some(env_name) = spec.api_key_env.as_ref() {
            env_names.insert(env_name.clone());
        }
        if let Some(env_name) = spec.secret_key_env.as_ref() {
            env_names.insert(env_name.clone());
        }
    }
    env_names.into_iter().collect()
}

fn secret_targets() -> Vec<(String, String, String)> {
    let mut targets = BTreeSet::new();
    for env_name in provider_secret_env_names() {
        let account = keyring_account_for_env(&env_name);
        targets.insert((account.clone(), KEYRING_SERVICE.to_string(), account));
    }
    targets.into_iter().collect()
}

fn keyring_target_for_env(env_name: &str) -> Option<(String, String)> {
    Some((
        KEYRING_SERVICE.to_string(),
        keyring_account_for_env(env_name),
    ))
}

fn secret_reference(env_name: &str, secret_mode: &SecretMode) -> String {
    match secret_mode {
        SecretMode::Env => env_name.to_string(),
        SecretMode::Keyring => keyring_reference(env_name).unwrap_or_else(|| env_name.to_string()),
        SecretMode::AutoDetect => {
            if std::env::var(env_name).is_ok() {
                env_name.to_string()
            } else {
                keyring_reference(env_name).unwrap_or_else(|| env_name.to_string())
            }
        }
    }
}

fn keyring_reference(env_name: &str) -> Option<String> {
    keyring_target_for_env(env_name)
        .map(|(service, account)| format!("keyring://{}/{}", service, account))
}

fn generate_adaptive_config_toml(
    adaptive_config: &AdaptiveConfig,
    secret_mode: &SecretMode,
    custom_agents: &[CustomAgentSpec],
) -> String {
    let providers = adaptive_config.minimal_config.available_providers.clone();

    // Combined list: catalog providers + custom agent names for phase arrays
    let mut all_agent_names: Vec<String> = providers.clone();
    for ca in custom_agents {
        all_agent_names.push(ca.name.clone());
    }
    let recommendations = aggregate_provider_recommendations(&providers);
    // all_agent_names is guaranteed non-empty because `providers` always
    // contains at least the default provider, so the `.first()` unwrap is safe.
    let review_agents = if all_agent_names.len() > 1 {
        all_agent_names.clone()
    } else {
        vec![all_agent_names
            .first()
            .cloned()
            .unwrap_or_else(|| "default".to_string())]
    };
    let delivery_agents = vec![all_agent_names
        .first()
        .cloned()
        .unwrap_or_else(|| "default".to_string())];

    let mut content = String::new();
    content.push_str(&format!(
        "default_phase = \"{}\"\nmodel_selection_mode = \"adaptive\"\n\n",
        recommendations
            .default_phase
            .clone()
            .unwrap_or_else(|| adaptive_config.minimal_config.default_phase.clone())
    ));

    if adaptive_config.minimal_config.enable_cache && recommendations.cache_enabled {
        content.push_str(
            "[cache]\nenabled = true\npath = \"acp_cache.sqlite3\"\ndefault_ttl_seconds = 3600\nmax_entries = 5000\n\n",
        );
    }

    if adaptive_config.minimal_config.enable_vector_memory && recommendations.vector_enabled {
        content.push_str(
            "[vector]\nenabled = true\nauto_mode = true\npath = \"acp_vector.sqlite3\"\ndimensions = 192\nmin_query_chars = 80\ntop_k = 2\nmin_similarity = 0.82\nmax_snippet_chars = 800\nmax_entries = 10000\nsummary_enabled = true\nsummary_trigger_messages = 8\nsummary_max_chars = 1200\n\n",
        );
    }

    content.push_str(
        "[runtime]\nmaintenance_interval_seconds = 60\nhealth_interval_seconds = 120\nshutdown_drain_seconds = 30\nsqlite_vacuum_interval_cycles = 60\n\n",
    );

    for provider in &providers {
        append_agent_block(&mut content, provider, secret_mode);
    }
    for custom in custom_agents {
        append_custom_agent_block(&mut content, custom, secret_mode);
    }

    content.push_str("[flow]\nname = \"Autopilot Adaptive\"\nphases = [\"planning\", \"coding\", \"review\", \"delivery\"]\n\n");
    content.push_str(&format!(
        "[phases.planning]\ndescription = \"Adaptive planning phase\"\nagents = {}\nfallback = true\nprinciples = [\"Choose the smallest correct plan\", \"Use the available agents adaptively\"]\n\n",
        toml_array(&all_agent_names)
    ));
    content.push_str(&format!(
        "[phases.planning.options]\nrequest_timeout_seconds = {}\n\n",
        recommendations.planning_request_timeout_seconds
    ));
    content.push_str(&format!(
        "[phases.coding]\ndescription = \"Adaptive coding phase\"\nagents = {}\nfallback = true\nprinciples = [\"Make the smallest correct change\", \"Do not claim done without verification\"]\n\n",
        toml_array(&all_agent_names)
    ));
    content.push_str(&format!(
        "[phases.coding.options]\nautopilot_complexity = \"auto\"\nrequest_timeout_seconds = {}\nreview_timeout_seconds = {}\ncache_enabled = {}\nvector_enabled = {}\nsummary_enabled = {}\nfull_auto_review_agents = {}\nphase_max_inflight = {}\nglobal_max_inflight = {}\n\n",
        recommendations.coding_request_timeout_seconds,
        recommendations.coding_review_timeout_seconds,
        adaptive_config.minimal_config.enable_cache && recommendations.cache_enabled,
        adaptive_config.minimal_config.enable_vector_memory && recommendations.vector_enabled,
        adaptive_config.minimal_config.enable_vector_memory && recommendations.vector_enabled,
        toml_array(&review_agents),
        recommendations.phase_max_inflight,
        recommendations.global_max_inflight
    ));
    content.push_str(&format!(
        "[phases.review]\ndescription = \"Adaptive review phase\"\nagents = {}\nfallback = true\n\n",
        toml_array(&review_agents)
    ));
    content.push_str(
        &format!(
            "[phases.review.options]\nrequest_timeout_seconds = {}\nreview_timeout_policy = \"reject\"\nreview_min_response_chars = 12\n\n",
            recommendations.review_request_timeout_seconds
        ),
    );
    content.push_str(&format!(
        "[phases.delivery]\ndescription = \"Adaptive delivery phase\"\nagents = {}\nfallback = false\n",
        toml_array(&delivery_agents)
    ));
    content.push_str(&format!(
        "\n[phases.delivery.options]\nrequest_timeout_seconds = {}\n",
        recommendations.delivery_request_timeout_seconds
    ));

    content
}

fn append_agent_block(content: &mut String, provider: &str, secret_mode: &SecretMode) {
    if let Some(spec) = provider_spec_by_name(provider) {
        content.push_str(&format!("[agents.{}]\n", provider));
        content.push_str(&format!("type = \"{}\"\n", spec.agent_type));

        if let Some(url) = spec.url.as_ref() {
            content.push_str(&format!("url = \"{}\"\n", url));
        }
        if let Some(chat_path) = spec.chat_path.as_ref() {
            content.push_str(&format!("chat_path = \"{}\"\n", chat_path));
        }
        if let Some(api_key_env) = spec.api_key_env.as_ref() {
            content.push_str(&format!(
                "api_key_env = \"{}\"\n",
                secret_reference(api_key_env, secret_mode)
            ));
        }
        if let Some(secret_key_env) = spec.secret_key_env.as_ref() {
            content.push_str(&format!(
                "secret_key_env = \"{}\"\n",
                secret_reference(secret_key_env, secret_mode)
            ));
        }
        if let Some(model) = spec.model.as_ref() {
            content.push_str(&format!("model = \"{}\"\n", model));
        }
        if let Some(anthropic_version) = spec.anthropic_version.as_ref() {
            content.push_str(&format!("anthropic_version = \"{}\"\n", anthropic_version));
        }
        if let Some(max_tokens) = spec.max_tokens {
            content.push_str(&format!("max_tokens = {}\n", max_tokens));
        }
        if let Some(supports_system) = spec.supports_system {
            content.push_str(&format!("supports_system = {}\n", supports_system));
        }
        content.push('\n');
    }
}

fn append_custom_agent_block(
    content: &mut String,
    spec: &CustomAgentSpec,
    secret_mode: &SecretMode,
) {
    content.push_str(&format!("[agents.{}]\n", spec.name));
    content.push_str(&format!("type = \"{}\"\n", spec.agent_type));
    if let Some(url) = spec.url.as_ref() {
        content.push_str(&format!("url = \"{}\"\n", url));
    }
    if let Some(api_key_env) = spec.api_key_env.as_ref() {
        content.push_str(&format!(
            "api_key_env = \"{}\"\n",
            secret_reference(api_key_env, secret_mode)
        ));
    }
    if let Some(secret_key_env) = spec.secret_key_env.as_ref() {
        content.push_str(&format!(
            "secret_key_env = \"{}\"\n",
            secret_reference(secret_key_env, secret_mode)
        ));
    }
    if let Some(model) = spec.model.as_ref() {
        content.push_str(&format!("model = \"{}\"\n", model));
    }
    content.push_str("supports_system = true\n");
    content.push('\n');
}

fn toml_array(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|item| format!("\"{}\"", item)).collect();
    format!("[{}]", quoted.join(", "))
}

/// Locate setup template file for the provided template name.
///
/// Search order:
/// 1. directory containing current executable
/// 2. current working directory
///
/// Returns first existing match.
fn find_template(name: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(name));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(name));
    }

    candidates.into_iter().find(|path| path.exists())
}

/// Convert known env variable placeholder names in template content into keyring references.
///
/// This is used when setup is requested with keyring secret mode.
fn convert_env_placeholders_to_keyring(content: &str) -> String {
    let mut out = content.to_string();

    // Replace keyring-URL env entries (provider specs now use keyring:// refs directly).
    for env_name in provider_secret_env_names() {
        if let Some(reference) = keyring_reference(&env_name) {
            out = out.replace(&env_name, &reference);
        }
    }

    // Also handle raw env var names (legacy template format).
    // Build a reverse mapping from keyring account → raw env var name.
    let raw_env_names: &[(&str, &str)] = &[
        ("OPENAI_API_KEY", "keyring://go-on/openai_api_key"),
        ("ANTHROPIC_API_KEY", "keyring://go-on/anthropic_api_key"),
        ("DEEPSEEK_API_KEY", "keyring://go-on/deepseek_api_key"),
        ("DOUBAO_API_KEY", "keyring://go-on/doubao_api_key"),
        ("WENXIN_API_KEY", "keyring://go-on/wenxin_api_key"),
        ("WENXIN_SECRET_KEY", "keyring://go-on/wenxin_secret_key"),
        ("COPILOT_API_KEY", "keyring://go-on/copilot_api_key"),
        (
            "GITHUB_COPILOT_TOKEN",
            "keyring://go-on/github_copilot_token",
        ),
        (
            "OTHER_PROVIDER_API_KEY",
            "keyring://go-on/openai_compatible_api_key",
        ),
    ];
    for (raw_name, reference) in raw_env_names {
        out = out.replace(raw_name, reference);
    }

    out
}

/// Create default RULES files in the provided config directory.
///
/// This ensures baseline rule overlay files exist for policy and review behavior.
fn write_default_rules(config_dir: &Path) -> Result<()> {
    let rules_dir = config_dir.join("RULES");
    fs::create_dir_all(&rules_dir)
        .with_context(|| format!("failed to create RULES directory: {}", rules_dir.display()))?;

    write_if_missing(
        &config_dir.join("RULES.md"),
        "# Project Rule Overlay\n\n- Keep ACP protocol compatibility stable.\n- Favor safe and test-backed changes.\n",
    )?;
    write_if_missing(
        &rules_dir.join("global.md"),
        "# Global Rules\n\n- Preserve runtime safety and observability.\n- Do not leak secrets in logs or responses.\n",
    )?;
    write_if_missing(
        &rules_dir.join("local.md"),
        "# Local Overlay\n\n- Add machine or developer local overrides here.\n",
    )?;
    write_if_missing(
        &rules_dir.join("coding.md"),
        "# Coding Rules\n\n- Keep changes minimal and reviewable.\n- Add tests for non-trivial logic updates.\n",
    )?;
    write_if_missing(
        &rules_dir.join("review.md"),
        "# Review Rules\n\n- Enforce strict completeness: no placeholders, no TODO-only branches, and no unhandled errors.\n- Require evidence-backed review outcomes for non-trivial changes.\n",
    )?;

    Ok(())
}

/// Write content to file only if the file does not already exist.
fn write_if_missing(path: &Path, content: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    fs::write(path, content)
        .with_context(|| format!("failed to write file: {}", path.display()))?;
    Ok(())
}

/// Interactive flow to store all configured secrets into system keyring.
///
/// Prompts user for each secret key and stores non-empty values.
fn store_keyring_secrets_interactive(
    selected_providers: &[String],
    custom_agents: &[CustomAgentSpec],
) -> Result<()> {
    println!("{}", t("setup.enter_secrets"));
    let mut required_envs = Vec::new();
    for provider in selected_providers {
        for env_name in required_envs_for_provider(provider) {
            if !required_envs
                .iter()
                .any(|existing: &String| existing == &env_name)
            {
                required_envs.push(env_name);
            }
        }
    }
    for agent in custom_agents {
        for env_name in [agent.api_key_env.as_ref(), agent.secret_key_env.as_ref()]
            .into_iter()
            .flatten()
        {
            if !required_envs
                .iter()
                .any(|existing: &String| existing == env_name)
            {
                required_envs.push(env_name.clone());
            }
        }
    }

    let mut handled_envs = BTreeSet::new();

    for (name, service, account) in secret_targets() {
        if let Some(env_name) = secret_name_to_env(&name) {
            handled_envs.insert(env_name.to_string());
            if !required_envs.iter().any(|existing| existing == &env_name) {
                continue;
            }
        }

        let values = prompt_secret_pool_values(&name)?;
        if values.is_empty() {
            continue;
        }

        let entry = keyring::Entry::new(&service, &account)
            .map_err(|err| anyhow::anyhow!("failed to open keyring entry: {}", err))?;
        entry
            .set_password(&join_secret_pool_entries(&values))
            .map_err(|err| anyhow::anyhow!("failed to write keyring entry: {}", err))?;
    }

    for env_name in required_envs {
        if handled_envs.contains(&env_name) {
            continue;
        }
        let values = prompt_secret_pool_values(&env_name)?;
        if values.is_empty() {
            continue;
        }
        let Some((service, account)) = keyring_target_for_env(&env_name) else {
            continue;
        };
        let entry = keyring::Entry::new(&service, &account)
            .map_err(|err| anyhow::anyhow!("failed to open keyring entry: {}", err))?;
        entry
            .set_password(&join_secret_pool_entries(&values))
            .map_err(|err| anyhow::anyhow!("failed to write keyring entry: {}", err))?;
    }

    Ok(())
}

fn secret_name_to_env(secret_name: &str) -> Option<String> {
    provider_secret_env_names()
        .into_iter()
        .find(|env_name| keyring_account_for_env(env_name) == secret_name)
}

/// Resolve secret command name to keyring service/account.
/// Used by run_secret_command handlers to map human-readable secret names.
fn resolve_secret_target(name: Option<&str>) -> Result<(String, String)> {
    let name = name.ok_or_else(|| anyhow::anyhow!("--secret-name is required"))?;
    if let Some((_, service, account)) = secret_targets()
        .iter()
        .find(|(known_name, _, _)| *known_name == name)
    {
        return Ok((service.clone(), account.clone()));
    }

    if let Some(locator) = name.strip_prefix("keyring://") {
        let (service, account) = locator.split_once('/').ok_or_else(|| {
            anyhow::anyhow!(
                "invalid keyring secret reference '{}': expected keyring://<service>/<account>",
                name
            )
        })?;
        return Ok((service.to_string(), account.to_string()));
    }

    keyring_target_for_env(name)
        .ok_or_else(|| anyhow::anyhow!("{}", tf("error.unknown_secret_name", &[("name", name)])))
}

fn parse_secret_pool_entries(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let multiline: Vec<String> = trimmed
        .lines()
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect();
    if multiline.len() > 1 {
        return multiline;
    }

    if trimmed.contains(',') {
        let comma_split: Vec<String> = trimmed
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect();
        if comma_split.len() > 1 {
            return comma_split;
        }
    }

    vec![trimmed.to_string()]
}

fn join_secret_pool_entries(values: &[String]) -> String {
    values.join("\n")
}

fn mask_secret_pool_entry(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    let len = chars.len();
    if len <= 8 {
        return format!("{} (len={})", "*".repeat(len.min(4)), len);
    }

    let prefix: String = chars.iter().take(4).collect();
    let suffix: String = chars.iter().skip(len.saturating_sub(4)).collect();
    format!("{}...{}", prefix, suffix)
}

fn prompt_secret_pool_deletion_selection(
    secret_name: &str,
    values: &[String],
) -> Result<Option<usize>> {
    if values.is_empty() {
        return Ok(None);
    }

    println!("Select a key to delete from {}:", secret_name);
    for (index, value) in values.iter().enumerate() {
        println!("  {}. {}", index + 1, mask_secret_pool_entry(value));
    }
    println!("  0. Cancel");

    loop {
        let choice = prompt_value("Delete which key")?;
        let trimmed = choice.trim();
        if trimmed.is_empty() || trimmed == "0" {
            return Ok(None);
        }
        if let Ok(index) = trimmed.parse::<usize>() {
            if (1..=values.len()).contains(&index) {
                return Ok(Some(index - 1));
            }
        }

        println!("Invalid selection. Choose 0-{}.", values.len());
    }
}

fn prompt_choice(prompt: &str, allowed: &[&str], default: &str) -> Result<String> {
    loop {
        print!("{} [{}]: ", prompt, default);
        io::stdout().flush().context("failed to flush stdout")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read input")?;
        let value = {
            let trimmed = input.trim();
            if trimmed.is_empty() {
                default.to_string()
            } else {
                trimmed.to_string()
            }
        };

        if allowed.iter().any(|item| *item == value) {
            return Ok(value);
        }

        println!(
            "{}",
            tf("warning.invalid_value", &[("allowed", &allowed.join(", "))])
        );
    }
}

fn prompt_yes_no(prompt: &str, default_yes: bool) -> Result<bool> {
    let default = if default_yes { "Y/n" } else { "y/N" };
    loop {
        print!("{} [{}]: ", prompt, default);
        io::stdout().flush().context("failed to flush stdout")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read input")?;
        let trimmed = input.trim();

        if trimmed.is_empty() {
            return Ok(default_yes);
        }
        if trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes") {
            return Ok(true);
        }
        if trimmed.eq_ignore_ascii_case("n") || trimmed.eq_ignore_ascii_case("no") {
            return Ok(false);
        }
    }
}

fn prompt_value(prompt: &str) -> Result<String> {
    print!("{}: ", prompt);
    io::stdout().flush().context("failed to flush stdout")?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("failed to read input")?;
    Ok(input.trim().to_string())
}

fn prompt_secret_pool_values(prompt: &str) -> Result<Vec<String>> {
    println!("{} (enter one key per line, leave blank to finish)", prompt);
    let mut values = Vec::new();
    loop {
        let value = prompt_value(&format!("{} #{}", prompt, values.len() + 1))?;
        if value.trim().is_empty() {
            break;
        }
        values.push(value.trim().to_string());
    }
    Ok(values)
}

pub fn add_local_model(config_path: &Path, mut options: LocalModelOptions) -> Result<()> {
    if !config_path.exists() {
        anyhow::bail!("config file does not exist: {}", config_path.display());
    }

    let mut name = options
        .name
        .take()
        .unwrap_or_else(|| "local_model".to_string())
        .trim()
        .to_string();
    if name.is_empty() {
        name = "local_model".to_string();
    }

    let mut url = options.url.take().unwrap_or_default();
    if url.trim().is_empty() {
        url = prompt_value("Local model URL (for example http://127.0.0.1:11434/v1)")?;
    }
    if url.trim().is_empty() {
        anyhow::bail!("local model url is required");
    }

    let mut agent_type = options
        .agent_type
        .take()
        .unwrap_or_else(|| "openai".to_string());
    if agent_type.trim().is_empty() {
        agent_type = "openai".to_string();
    }

    let mut model = options
        .model
        .take()
        .unwrap_or_else(|| "local-model".to_string());
    if model.trim().is_empty() {
        model = "local-model".to_string();
    }

    let content = fs::read_to_string(config_path)
        .with_context(|| format!("failed to read config file: {}", config_path.display()))?;
    let mut root: toml::Value = toml::from_str(&content)
        .with_context(|| format!("failed to parse toml: {}", config_path.display()))?;

    let table = root
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("root toml is not table"))?;

    let agents = ensure_table(table, "agents")?;
    let mut agent_table = toml::map::Map::new();
    agent_table.insert(
        "type".to_string(),
        toml::Value::String(agent_type.trim().to_string()),
    );
    agent_table.insert(
        "url".to_string(),
        toml::Value::String(url.trim().to_string()),
    );
    agent_table.insert(
        "model".to_string(),
        toml::Value::String(model.trim().to_string()),
    );
    agent_table.insert("supports_system".to_string(), toml::Value::Boolean(true));

    if let Some(api_key_env) = options.api_key_env.take() {
        if !api_key_env.trim().is_empty() {
            agent_table.insert(
                "api_key_env".to_string(),
                toml::Value::String(api_key_env.trim().to_string()),
            );
        }
    }
    if let Some(secret_key_env) = options.secret_key_env.take() {
        if !secret_key_env.trim().is_empty() {
            agent_table.insert(
                "secret_key_env".to_string(),
                toml::Value::String(secret_key_env.trim().to_string()),
            );
        }
    }

    agents.insert(name.clone(), toml::Value::Table(agent_table));

    if options.apply_to_phases {
        let phases = ensure_table(table, "phases")?;
        for phase_name in ["planning", "coding", "review", "delivery"] {
            let Some(phase) = phases
                .get_mut(phase_name)
                .and_then(|value| value.as_table_mut())
            else {
                continue;
            };
            ensure_string_array_contains(phase, "agents", &name)?;

            if phase_name == "coding" {
                let options = ensure_table(phase, "options")?;
                ensure_string_array_contains(options, "full_auto_review_agents", &name)?;
            }
        }
    }

    let output = toml::to_string_pretty(&root).context("failed to serialize updated config")?;
    fs::write(config_path, output)
        .with_context(|| format!("failed to write config file: {}", config_path.display()))?;
    println!(
        "added local model '{}' to {}",
        name,
        config_path.to_string_lossy()
    );
    Ok(())
}

fn ensure_table<'a>(
    parent: &'a mut toml::map::Map<String, toml::Value>,
    key: &str,
) -> Result<&'a mut toml::map::Map<String, toml::Value>> {
    let value = parent
        .entry(key.to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    if !value.is_table() {
        *value = toml::Value::Table(toml::map::Map::new());
    }
    value
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("expected table after normalization"))
}

fn ensure_string_array_contains(
    parent: &mut toml::map::Map<String, toml::Value>,
    key: &str,
    item: &str,
) -> Result<()> {
    let value = parent
        .entry(key.to_string())
        .or_insert_with(|| toml::Value::Array(Vec::new()));
    if !value.is_array() {
        *value = toml::Value::Array(Vec::new());
    }
    let array = value
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("expected array after normalization"))?;
    let exists = array
        .iter()
        .any(|entry| entry.as_str().map(|v| v == item).unwrap_or(false));
    if !exists {
        array.push(toml::Value::String(item.to_string()));
    }
    Ok(())
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
