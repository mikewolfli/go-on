//! setup.rs
//! Auto-generated English doc: module overview.
//!
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::config::AdaptiveConfig;
use crate::i18n::runtime::{t, tf};
use anyhow::{Context, Result};

// 自适应配置模板名称
const ADAPTIVE_TEMPLATE: &str = "config.toml.autopilot-adaptive";
const PROVIDER_CAPABILITY_FILE: &str = "providers.toml";

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

#[derive(Debug, serde::Deserialize)]
struct ProviderCapabilityCatalog {
    providers: Vec<ProviderSpec>,
}

static PROVIDER_SPECS: OnceLock<Vec<ProviderSpec>> = OnceLock::new();

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

// Secret key targets used for keyring operations.
// Each tuple is (command name, keyring service, keyring account).
const SECRET_TARGETS: &[(&str, &str, &str)] = &[
    ("deepseek_api_key", "go-on", "deepseek_api_key"),
    ("wenxin_api_key", "go-on", "wenxin_api_key"),
    ("wenxin_secret_key", "go-on", "wenxin_secret_key"),
    ("anthropic_api_key", "go-on", "anthropic_api_key"),
    ("doubao_api_key", "go-on", "doubao_api_key"),
    ("gemini_api_key", "go-on", "gemini_api_key"),
    ("groq_api_key", "go-on", "groq_api_key"),
    ("mistral_api_key", "go-on", "mistral_api_key"),
    ("minimax_api_key", "go-on", "minimax_api_key"),
    ("glm_api_key", "go-on", "glm_api_key"),
    ("yi_api_key", "go-on", "yi_api_key"),
    ("moonshot_api_key", "go-on", "moonshot_api_key"),
    ("qianfan_api_key", "go-on", "qianfan_api_key"),
    ("qianfan_secret_key", "go-on", "qianfan_secret_key"),
    ("qwen_api_key", "go-on", "qwen_api_key"),
    ("qwen_secret_key", "go-on", "qwen_secret_key"),
    ("hunyuan_api_key", "go-on", "hunyuan_api_key"),
    ("hunyuan_secret_key", "go-on", "hunyuan_secret_key"),
    (
        "openai_compatible_api_key",
        "go-on",
        "openai_compatible_api_key",
    ),
];

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
    PROVIDER_SPECS.get_or_init(load_provider_specs).as_slice()
}

fn load_provider_specs() -> Vec<ProviderSpec> {
    if let Some(path) = find_template(PROVIDER_CAPABILITY_FILE) {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(catalog) = toml::from_str::<ProviderCapabilityCatalog>(&content) {
                if !catalog.providers.is_empty() {
                    return catalog.providers;
                }
            }
        }
    }
    built_in_provider_specs()
}

fn built_in_provider_specs() -> Vec<ProviderSpec> {
    vec![
        ProviderSpec {
            name: "openai".to_string(),
            agent_type: "openai".to_string(),
            url: Some("https://api.openai.com/v1".to_string()),
            chat_path: None,
            model: Some("gpt-4o-mini".to_string()),
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: Some(true),
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
            name: "anthropic".to_string(),
            agent_type: "claude".to_string(),
            url: Some("https://api.anthropic.com".to_string()),
            chat_path: None,
            model: Some("claude-3-7-sonnet-latest".to_string()),
            api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
            secret_key_env: None,
            anthropic_version: Some("2023-06-01".to_string()),
            max_tokens: Some(4096),
            supports_system: None,
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
            url: None,
            chat_path: None,
            model: Some("deepseek-chat".to_string()),
            api_key_env: Some("DEEPSEEK_API_KEY".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: Some(true),
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
            name: "wenxin".to_string(),
            agent_type: "wenxin".to_string(),
            url: None,
            chat_path: None,
            model: Some("ERNIE-Bot".to_string()),
            api_key_env: Some("WENXIN_API_KEY".to_string()),
            secret_key_env: Some("WENXIN_SECRET_KEY".to_string()),
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
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
            api_key_env: None,
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: None,
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

    let cache = ensure_table(table, "cache");
    cache.insert(
        "enabled".to_string(),
        toml::Value::Boolean(recommendations.cache_enabled),
    );

    let vector = ensure_table(table, "vector");
    vector.insert(
        "enabled".to_string(),
        toml::Value::Boolean(recommendations.vector_enabled),
    );

    let agent_names = table
        .get("agents")
        .and_then(|value| value.as_table())
        .map(|agents| agents.keys().cloned().collect::<Vec<String>>())
        .unwrap_or_default();
    let phases = ensure_table(table, "phases");
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
        let options = ensure_table(phase, "options");
        options.insert(
            "request_timeout_seconds".to_string(),
            toml::Value::Integer(timeout as i64),
        );
    }

    if let Some(coding_phase) = phases
        .get_mut("coding")
        .and_then(|value| value.as_table_mut())
    {
        let options = ensure_table(coding_phase, "options");
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
        "applied provider recommendations to {}",
        config_path.to_string_lossy()
    );
    if !created_phases.is_empty() {
        println!("created missing phases: {}", created_phases.join(", "));
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

#[allow(dead_code)]
/// Run setup with default options.
///
/// This helper is a thin wrapper around `run_setup_with_options`.
pub fn run_setup(config_path: &Path) -> Result<()> {
    run_setup_with_options(config_path, SetupOptions::default())
}

/// Entry point for setup logic.
///
/// Handles profile selection, secret mode, writing config template, writing RULES files,
/// and optionally storing secrets into keyring.
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
            // 自动检测：如果已有环境变量，使用Env模式，否则询问
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

    // 检测可用的AI提供商
    let detected_providers = detect_available_providers(&secret_mode);
    let available_providers = prompt_provider_selection(&detected_providers)?;

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

    let mut content = generate_adaptive_config_toml(&adaptive_config, &secret_mode);

    // 如果使用keyring模式，转换环境变量占位符
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
        SecretMode::Keyring => {
            if options.prompt_for_secrets {
                prompt_yes_no(&t("setup.prompt_store_secrets"), true)?
            } else {
                false
            }
        }

        SecretMode::AutoDetect => {
            // 自动检测模式下，询问是否要设置API密钥
            prompt_yes_no(&t("setup.prompt_setup_api_keys"), true)?
        }
        _ => false,
    };

    if should_store_secrets {
        store_keyring_secrets_interactive(&adaptive_config.minimal_config.available_providers)?;
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
            for (name, service, account) in SECRET_TARGETS {
                let entry = keyring::Entry::new(service, account)
                    .map_err(|err| anyhow::anyhow!("failed to open keyring entry: {}", err))?;
                let status = if entry.get_password().is_ok() {
                    "present"
                } else {
                    "missing"
                };
                println!(
                    "{}",
                    tf("setup.secret_status", &[("name", name), ("status", status)])
                );
            }
            Ok(())
        }
        SecretAction::Set => {
            let (service, account) = resolve_secret_target(name)?;
            let value =
                value.ok_or_else(|| anyhow::anyhow!("{}", t("error.secret_value_required")))?;
            let entry = keyring::Entry::new(service, account).map_err(|err| {
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
            let entry = keyring::Entry::new(service, account).map_err(|err| {
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
            println!("{}", secret);
            Ok(())
        }
        SecretAction::Delete => {
            let (service, account) = resolve_secret_target(name)?;
            let entry = keyring::Entry::new(service, account).map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.keyring_open", &[("error", &format!("{}", err))])
                )
            })?;
            entry.delete_credential().map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.keyring_delete", &[("error", &format!("{}", err))])
                )
            })?;
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

fn prompt_provider_selection(detected_providers: &[String]) -> Result<Vec<String>> {
    loop {
        println!("{}", t("setup.provider_selection_title"));
        for (index, spec) in provider_specs().iter().enumerate() {
            println!("  {}. {}", index + 1, spec.name);
        }

        let default = if detected_providers.is_empty() {
            "manual".to_string()
        } else {
            detected_providers.join(",")
        };

        print!("{} [{}]: ", t("setup.provider_selection_prompt"), default);
        io::stdout().flush().context("failed to flush stdout")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read input")?;
        let value = input.trim();

        if value.is_empty() || value.eq_ignore_ascii_case("auto") {
            if detected_providers.is_empty() {
                println!("{}", t("setup.provider_selection_required"));
                continue;
            }
            return Ok(detected_providers.to_vec());
        }

        let mut selected = Vec::new();
        let mut invalid = None;
        for token in value.split(',') {
            let raw = token.trim();
            if raw.is_empty() {
                continue;
            }

            if let Ok(index) = raw.parse::<usize>() {
                if let Some(spec) = provider_specs().get(index.saturating_sub(1)) {
                    selected.push(spec.name.clone());
                    continue;
                }
                invalid = Some(raw.to_string());
                break;
            }

            if provider_specs().iter().any(|spec| spec.name == raw) {
                selected.push(raw.to_string());
            } else {
                invalid = Some(raw.to_string());
                break;
            }
        }

        if let Some(value) = invalid {
            println!(
                "{}",
                tf(
                    "error.invalid_provider_selection",
                    &[("value", value.as_str())]
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

fn keyring_target_for_env(env_name: &str) -> Option<(String, String)> {
    let account = if env_name == "OPENAI_API_KEY" {
        "openai_compatible_api_key".to_string()
    } else {
        env_name.to_ascii_lowercase()
    };
    Some(("go-on".to_string(), account))
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
) -> String {
    let providers = adaptive_config.minimal_config.available_providers.clone();
    let recommendations = aggregate_provider_recommendations(&providers);
    let review_agents = if providers.len() > 1 {
        providers.clone()
    } else {
        vec![providers[0].clone()]
    };
    let delivery_agents = vec![providers[0].clone()];

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

    content.push_str("[flow]\nname = \"Autopilot Adaptive\"\nphases = [\"planning\", \"coding\", \"review\", \"delivery\"]\n\n");
    content.push_str(&format!(
        "[phases.planning]\ndescription = \"Adaptive planning phase\"\nagents = {}\nfallback = true\nprinciples = [\"Choose the smallest correct plan\", \"Use the available agents adaptively\"]\n\n",
        toml_array(&providers)
    ));
    content.push_str(&format!(
        "[phases.planning.options]\nrequest_timeout_seconds = {}\n\n",
        recommendations.planning_request_timeout_seconds
    ));
    content.push_str(&format!(
        "[phases.coding]\ndescription = \"Adaptive coding phase\"\nagents = {}\nfallback = true\nprinciples = [\"Make the smallest correct change\", \"Do not claim done without verification\"]\n\n",
        toml_array(&providers)
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
    let mappings = [
        ("DEEPSEEK_API_KEY", "keyring://go-on/deepseek_api_key"),
        ("WENXIN_API_KEY", "keyring://go-on/wenxin_api_key"),
        ("WENXIN_SECRET_KEY", "keyring://go-on/wenxin_secret_key"),
        ("ANTHROPIC_API_KEY", "keyring://go-on/anthropic_api_key"),
        ("DOUBAO_API_KEY", "keyring://go-on/doubao_api_key"),
        ("GEMINI_API_KEY", "keyring://go-on/gemini_api_key"),
        ("GROQ_API_KEY", "keyring://go-on/groq_api_key"),
        ("MISTRAL_API_KEY", "keyring://go-on/mistral_api_key"),
        ("MINIMAX_API_KEY", "keyring://go-on/minimax_api_key"),
        ("GLM_API_KEY", "keyring://go-on/glm_api_key"),
        ("YI_API_KEY", "keyring://go-on/yi_api_key"),
        ("MOONSHOT_API_KEY", "keyring://go-on/moonshot_api_key"),
        ("QIANFAN_API_KEY", "keyring://go-on/qianfan_api_key"),
        ("QIANFAN_SECRET_KEY", "keyring://go-on/qianfan_secret_key"),
        ("QWEN_API_KEY", "keyring://go-on/qwen_api_key"),
        ("QWEN_SECRET_KEY", "keyring://go-on/qwen_secret_key"),
        ("HUNYUAN_API_KEY", "keyring://go-on/hunyuan_api_key"),
        ("HUNYUAN_SECRET_KEY", "keyring://go-on/hunyuan_secret_key"),
        (
            "OTHER_PROVIDER_API_KEY",
            "keyring://go-on/openai_compatible_api_key",
        ),
    ];

    let mut out = content.to_string();
    for (from, to) in mappings {
        out = out.replace(from, to);
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
fn store_keyring_secrets_interactive(selected_providers: &[String]) -> Result<()> {
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

    let mut handled_envs = BTreeSet::new();

    for (name, service, account) in SECRET_TARGETS {
        if let Some(env_name) = secret_name_to_env(name) {
            handled_envs.insert(env_name.to_string());
            if !required_envs.iter().any(|existing| existing == env_name) {
                continue;
            }
        }

        let value = prompt_value(name)?;
        if value.trim().is_empty() {
            continue;
        }

        let entry = keyring::Entry::new(service, account)
            .map_err(|err| anyhow::anyhow!("failed to open keyring entry: {}", err))?;
        entry
            .set_password(value.trim())
            .map_err(|err| anyhow::anyhow!("failed to write keyring entry: {}", err))?;
    }

    for env_name in required_envs {
        if handled_envs.contains(&env_name) {
            continue;
        }
        let value = prompt_value(&env_name)?;
        if value.trim().is_empty() {
            continue;
        }
        let Some((service, account)) = keyring_target_for_env(&env_name) else {
            continue;
        };
        let entry = keyring::Entry::new(&service, &account)
            .map_err(|err| anyhow::anyhow!("failed to open keyring entry: {}", err))?;
        entry
            .set_password(value.trim())
            .map_err(|err| anyhow::anyhow!("failed to write keyring entry: {}", err))?;
    }

    Ok(())
}

fn secret_name_to_env(secret_name: &str) -> Option<&'static str> {
    match secret_name {
        "deepseek_api_key" => Some("DEEPSEEK_API_KEY"),
        "wenxin_api_key" => Some("WENXIN_API_KEY"),
        "wenxin_secret_key" => Some("WENXIN_SECRET_KEY"),
        "anthropic_api_key" => Some("ANTHROPIC_API_KEY"),
        "doubao_api_key" => Some("DOUBAO_API_KEY"),
        "gemini_api_key" => Some("GEMINI_API_KEY"),
        "groq_api_key" => Some("GROQ_API_KEY"),
        "mistral_api_key" => Some("MISTRAL_API_KEY"),
        "minimax_api_key" => Some("MINIMAX_API_KEY"),
        "glm_api_key" => Some("GLM_API_KEY"),
        "yi_api_key" => Some("YI_API_KEY"),
        "moonshot_api_key" => Some("MOONSHOT_API_KEY"),
        "qianfan_api_key" => Some("QIANFAN_API_KEY"),
        "qianfan_secret_key" => Some("QIANFAN_SECRET_KEY"),
        "qwen_api_key" => Some("QWEN_API_KEY"),
        "qwen_secret_key" => Some("QWEN_SECRET_KEY"),
        "hunyuan_api_key" => Some("HUNYUAN_API_KEY"),
        "hunyuan_secret_key" => Some("HUNYUAN_SECRET_KEY"),
        "openai_compatible_api_key" => Some("OPENAI_API_KEY"),
        _ => None,
    }
}

/// Resolve secret command name to keyring service/account.
/// Used by run_secret_command handlers to map human-readable secret names.
fn resolve_secret_target(name: Option<&str>) -> Result<(&'static str, &'static str)> {
    let name = name.ok_or_else(|| anyhow::anyhow!("--secret-name is required"))?;
    SECRET_TARGETS
        .iter()
        .find(|(known_name, _, _)| *known_name == name)
        .map(|(_, service, account)| (*service, *account))
        .ok_or_else(|| anyhow::anyhow!("{}", tf("error.unknown_secret_name", &[("name", name)])))
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

    let agents = ensure_table(table, "agents");
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
        let phases = ensure_table(table, "phases");
        for phase_name in ["planning", "coding", "review", "delivery"] {
            let Some(phase) = phases
                .get_mut(phase_name)
                .and_then(|value| value.as_table_mut())
            else {
                continue;
            };
            ensure_string_array_contains(phase, "agents", &name);

            if phase_name == "coding" {
                let options = ensure_table(phase, "options");
                ensure_string_array_contains(options, "full_auto_review_agents", &name);
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
) -> &'a mut toml::map::Map<String, toml::Value> {
    let value = parent
        .entry(key.to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    if !value.is_table() {
        *value = toml::Value::Table(toml::map::Map::new());
    }
    value
        .as_table_mut()
        .expect("table must be available after normalization")
}

fn ensure_string_array_contains(
    parent: &mut toml::map::Map<String, toml::Value>,
    key: &str,
    item: &str,
) {
    let value = parent
        .entry(key.to_string())
        .or_insert_with(|| toml::Value::Array(Vec::new()));
    if !value.is_array() {
        *value = toml::Value::Array(Vec::new());
    }
    let array = value
        .as_array_mut()
        .expect("array must be available after normalization");
    let exists = array
        .iter()
        .any(|entry| entry.as_str().map(|v| v == item).unwrap_or(false));
    if !exists {
        array.push(toml::Value::String(item.to_string()));
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
