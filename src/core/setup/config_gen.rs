//! Config generation sub-module (GAP-B53-23).
//!
//! Contains all config-generation and recommendation logic extracted from
//! `super::mod.rs`.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::AdaptiveConfig;
use crate::i18n::runtime::tf;
use anyhow::{Context, Result};

// ---------------------------------------------------------------------------
// Types

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

// ---------------------------------------------------------------------------
// Recommendation aggregation

fn aggregate_provider_recommendations(providers: &[String]) -> ProviderRecommendations {
    let mut rec = ProviderRecommendations::default();
    let mut cache_votes: Vec<bool> = Vec::new();
    let mut vector_votes: Vec<bool> = Vec::new();

    for provider in providers {
        let Some(spec) = super::provider_spec_by_name(provider) else {
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

// ---------------------------------------------------------------------------
// Public recommendation / application

pub fn recommendation_snapshot_for_config(
    config: &crate::config::AppConfig,
) -> Option<ProviderRecommendationSnapshot> {
    let mut provider_names: HashSet<String> = HashSet::new();
    for agent in config.agents().values() {
        if let Some(spec) = super::provider_spec_by_agent_type(agent.agent_type.as_str()) {
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

    let cache = super::ensure_table(table, "cache")?;
    cache.insert(
        "enabled".to_string(),
        toml::Value::Boolean(recommendations.cache_enabled),
    );

    let vector = super::ensure_table(table, "vector")?;
    vector.insert(
        "enabled".to_string(),
        toml::Value::Boolean(recommendations.vector_enabled),
    );

    let agent_names = table
        .get("agents")
        .and_then(|value| value.as_table())
        .map(|agents| agents.keys().cloned().collect::<Vec<String>>())
        .unwrap_or_default();
    let phases = super::ensure_table(table, "phases")?;
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
        let options = super::ensure_table(phase, "options")?;
        options.insert(
            "request_timeout_seconds".to_string(),
            toml::Value::Integer(timeout as i64),
        );
    }

    if let Some(coding_phase) = phases
        .get_mut("coding")
        .and_then(|value| value.as_table_mut())
    {
        let options = super::ensure_table(coding_phase, "options")?;
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

// ---------------------------------------------------------------------------
// TOML helpers

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

        if let Some(spec) = super::provider_spec_by_agent_type(agent_type) {
            names.insert(spec.name.clone());
            continue;
        }
        if let Some(spec) = super::provider_spec_by_name(agent_name.as_str()) {
            names.insert(spec.name.clone());
        }
    }

    names.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Adaptive config generation

pub(crate) fn generate_adaptive_config_toml(
    adaptive_config: &AdaptiveConfig,
    secret_mode: &super::SecretMode,
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
            "[cache]\nenabled = true\npath = \"sqlite3/acp_cache.sqlite3\"\ndefault_ttl_seconds = 3600\nmax_entries = 5000\n\n",
        );
    }

    if adaptive_config.minimal_config.enable_vector_memory && recommendations.vector_enabled {
        content.push_str(
            "[vector]\nenabled = true\nauto_mode = true\npath = \"sqlite3/acp_vector.sqlite3\"\ndimensions = 192\nmin_query_chars = 80\ntop_k = 2\nmin_similarity = 0.82\nmax_snippet_chars = 800\nmax_entries = 10000\nsummary_enabled = true\nsummary_trigger_messages = 8\nsummary_max_chars = 1200\n\n",
        );
    }

    content.push_str(
        "[runtime]\nmaintenance_interval_seconds = 60\nhealth_interval_seconds = 120\nshutdown_drain_seconds = 30\n\n",
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

fn append_agent_block(content: &mut String, provider: &str, secret_mode: &super::SecretMode) {
    if let Some(spec) = super::provider_spec_by_name(provider) {
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
                super::secrets::secret_reference(api_key_env, secret_mode)
            ));
        }
        if let Some(secret_key_env) = spec.secret_key_env.as_ref() {
            content.push_str(&format!(
                "secret_key_env = \"{}\"\n",
                super::secrets::secret_reference(secret_key_env, secret_mode)
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
    secret_mode: &super::SecretMode,
) {
    content.push_str(&format!("[agents.{}]\n", spec.name));
    content.push_str(&format!("type = \"{}\"\n", spec.agent_type));
    if let Some(url) = spec.url.as_ref() {
        content.push_str(&format!("url = \"{}\"\n", url));
    }
    if let Some(api_key_env) = spec.api_key_env.as_ref() {
        content.push_str(&format!(
            "api_key_env = \"{}\"\n",
            super::secrets::secret_reference(api_key_env, secret_mode)
        ));
    }
    if let Some(secret_key_env) = spec.secret_key_env.as_ref() {
        content.push_str(&format!(
            "secret_key_env = \"{}\"\n",
            super::secrets::secret_reference(secret_key_env, secret_mode)
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
pub(super) fn find_template(name: &str) -> Option<PathBuf> {
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

/// Create default RULES files in the provided config directory.
///
/// This ensures baseline rule overlay files exist for policy and review behavior.
pub(super) fn write_default_rules(config_dir: &Path) -> Result<()> {
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
