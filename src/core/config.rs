//! Configuration system implementation
//!
//! This module defines the configuration structures and validation logic for the go-on application.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use tracing::{info, warn};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::inspect_secret_pool;

/// Application configuration structure
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct AppConfig {
    /// Default phase to use when none is specified
    pub default_phase: String,
    /// Map of agent configurations
    pub agents: HashMap<String, AgentConfig>,
    /// Flow configuration defining phase sequence
    pub flow: FlowConfig,
    /// Map of phase configurations
    pub phases: HashMap<String, PhaseConfig>,
    /// Runtime configuration
    pub runtime: Option<RuntimeConfig>,
    /// Cache configuration
    pub cache: Option<CacheConfig>,
    /// Vector store configuration
    pub vector: Option<VectorConfig>,
    /// Autotune configuration
    pub autotune: Option<AutoTuneConfig>,
    /// Model selection mode for automatic selection (Phase 10+)
    #[serde(default)]
    pub model_selection_mode: String,
}

/// Simplified adaptive configuration for AI-driven setup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveConfig {
    /// Whether to use adaptive mode (AI determines best configuration)
    #[serde(default = "default_true")]
    pub adaptive_mode: bool,

    /// Minimum configuration required for operation
    pub minimal_config: MinimalConfig,

    /// Learning preferences for AI adaptation
    #[serde(default)]
    pub learning_preferences: LearningPreferences,

    /// Conversation history for context-aware adaptation
    #[serde(default)]
    pub conversation_context: Vec<ConversationContext>,
}

/// Minimal configuration required for operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinimalConfig {
    /// Default phase name
    #[serde(default = "default_coding_phase")]
    pub default_phase: String,

    /// Available AI providers (auto-detected from environment)
    #[serde(default)]
    pub available_providers: Vec<String>,

    /// Whether to enable caching
    #[serde(default = "default_true")]
    pub enable_cache: bool,

    /// Whether to enable vector memory
    #[serde(default = "default_true")]
    pub enable_vector_memory: bool,
}

/// Learning preferences for AI adaptation
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LearningPreferences {
    /// Preferred communication style
    #[serde(default = "default_communication_style")]
    pub communication_style: String,

    /// Preferred level of detail
    #[serde(default = "default_detail_level")]
    pub detail_level: String,

    /// Learning speed preference
    #[serde(default = "default_learning_speed")]
    pub learning_speed: String,

    /// Whether to ask for clarification when uncertain
    #[serde(default = "default_true")]
    pub ask_for_clarification: bool,

    /// Whether to adapt based on conversation history
    #[serde(default = "default_true")]
    pub adapt_from_history: bool,
}

/// Conversation context for adaptive configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationContext {
    /// Conversation ID
    pub conversation_id: String,
    /// User preferences expressed in conversation
    pub expressed_preferences: Vec<String>,
    /// Successful adaptations from this conversation
    pub successful_adaptations: Vec<String>,
    /// Timestamp of last interaction
    pub last_interaction: i64,
}

// Default value functions
fn default_true() -> bool {
    true
}
fn default_coding_phase() -> String {
    "coding".to_string()
}
fn default_communication_style() -> String {
    "direct".to_string()
}
fn default_detail_level() -> String {
    "balanced".to_string()
}
fn default_learning_speed() -> String {
    "adaptive".to_string()
}
#[allow(dead_code)]
fn default_evaluate_interval() -> u32 {
    20
}
#[allow(dead_code)]
fn default_min_query_chars_step() -> u32 {
    20
}

const PROVIDER_CAPABILITY_FILE: &str = "providers.toml";

#[derive(Debug, Clone, Deserialize)]
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
}

#[derive(Debug, Deserialize)]
struct ProviderCapabilityCatalog {
    providers: Vec<ProviderSpec>,
}

static PROVIDER_SPECS: OnceLock<Vec<ProviderSpec>> = OnceLock::new();

fn provider_specs() -> &'static [ProviderSpec] {
    PROVIDER_SPECS.get_or_init(load_provider_specs).as_slice()
}

fn load_provider_specs() -> Vec<ProviderSpec> {
    if let Some(path) = find_template(PROVIDER_CAPABILITY_FILE) {
        if let Ok(content) = fs::read_to_string(path) {
            if let Ok(catalog) = toml::from_str::<ProviderCapabilityCatalog>(&content) {
                if !catalog.providers.is_empty() {
                    return catalog.providers;
                }
            }
        }
    }
    vec![
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
            supports_system: Some(true),
        },
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
            supports_system: Some(true),
        },
    ]
}

fn find_template(name: &str) -> Option<std::path::PathBuf> {
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

fn provider_spec_by_name(name: &str) -> Option<&'static ProviderSpec> {
    provider_specs().iter().find(|spec| spec.name == name)
}

/// Convert AppConfig to AdaptiveConfig
impl From<AppConfig> for AdaptiveConfig {
    fn from(config: AppConfig) -> Self {
        let mut available_providers: Vec<String> = config
            .agents
            .values()
            .filter_map(|agent| normalize_provider_name(&agent.agent_type))
            .collect();
        available_providers.sort();
        available_providers.dedup();

        if available_providers.is_empty() {
            available_providers.push("copilot".to_string());
        }

        AdaptiveConfig {
            adaptive_mode: true,
            minimal_config: MinimalConfig {
                default_phase: config.default_phase,
                available_providers,
                enable_cache: config.cache.is_some(),
                enable_vector_memory: config.vector.is_some(),
            },
            learning_preferences: LearningPreferences::default(),
            conversation_context: Vec::new(),
        }
    }
}

/// Create adaptive configuration from minimal input
impl AdaptiveConfig {
    /// Create adaptive configuration with auto-detection
    pub fn auto_detect() -> Self {
        let mut available_providers = Vec::new();

        for spec in provider_specs() {
            let mut required = Vec::new();
            if let Some(api) = spec.api_key_env.as_ref() {
                required.push(api);
            }
            if let Some(secret) = spec.secret_key_env.as_ref() {
                required.push(secret);
            }
            if required.is_empty() {
                continue;
            }
            if required.iter().all(|name| std::env::var(name).is_ok()) {
                available_providers.push(spec.name.clone());
            }
        }

        available_providers.sort();
        available_providers.dedup();

        if available_providers.is_empty() {
            available_providers.push("copilot".to_string());
        }

        AdaptiveConfig {
            adaptive_mode: true,
            minimal_config: MinimalConfig {
                default_phase: default_coding_phase(),
                available_providers,
                enable_cache: true,
                enable_vector_memory: true,
            },
            learning_preferences: LearningPreferences::default(),
            conversation_context: Vec::new(),
        }
    }

    /// Learn from conversation and adapt configuration
    pub fn learn_from_conversation(
        &mut self,
        conversation_id: &str,
        user_message: &str,
        ai_response: &str,
    ) {
        // Extract preferences from conversation
        let preferences = self.extract_preferences(user_message, ai_response);

        // Find or create conversation context
        let context = self
            .conversation_context
            .iter_mut()
            .find(|ctx| ctx.conversation_id == conversation_id);

        if let Some(ctx) = context {
            ctx.expressed_preferences.extend(preferences);
            ctx.last_interaction = now_ts();
        } else {
            self.conversation_context.push(ConversationContext {
                conversation_id: conversation_id.to_string(),
                expressed_preferences: preferences,
                successful_adaptations: Vec::new(),
                last_interaction: now_ts(),
            });
        }

        // Apply learning to preferences
        self.adapt_from_conversation_history();
    }

    /// Extract preferences from conversation messages
    fn extract_preferences(&self, user_message: &str, ai_response: &str) -> Vec<String> {
        let mut preferences = Vec::new();

        // Simple preference extraction (in real implementation, use NLP)
        let text = format!("{} {}", user_message, ai_response).to_lowercase();

        if text.contains("detailed") || text.contains("thorough") || text.contains("comprehensive")
        {
            preferences.push("prefers_detailed_responses".to_string());
        }

        if text.contains("brief") || text.contains("concise") || text.contains("short") {
            preferences.push("prefers_brief_responses".to_string());
        }

        if text.contains("explain") || text.contains("teach") || text.contains("educate") {
            preferences.push("wants_explanations".to_string());
        }

        if text.contains("fast") || text.contains("quick") || text.contains("efficient") {
            preferences.push("values_speed".to_string());
        }

        if text.contains("accurate") || text.contains("precise") || text.contains("correct") {
            preferences.push("values_accuracy".to_string());
        }

        preferences
    }

    /// Adapt configuration based on conversation history
    fn adapt_from_conversation_history(&mut self) {
        // Analyze all conversation contexts to find patterns
        let mut preference_counts = std::collections::HashMap::new();

        for ctx in &self.conversation_context {
            for pref in &ctx.expressed_preferences {
                *preference_counts.entry(pref.clone()).or_insert(0) += 1;
            }
        }

        // Apply most common preferences
        for (pref, count) in preference_counts {
            if count >= 2 {
                // At least mentioned in 2 conversations
                match pref.as_str() {
                    "prefers_detailed_responses" => {
                        self.learning_preferences.detail_level = "detailed".to_string();
                    }
                    "prefers_brief_responses" => {
                        self.learning_preferences.detail_level = "brief".to_string();
                    }
                    "wants_explanations" => {
                        self.learning_preferences.communication_style = "explanatory".to_string();
                    }
                    "values_speed" => {
                        self.learning_preferences.learning_speed = "fast".to_string();
                    }
                    "values_accuracy" => {
                        self.learning_preferences.ask_for_clarification = true;
                    }
                    _ => {}
                }
            }
        }
    }

    /// Generate AppConfig from adaptive configuration
    pub fn to_app_config(&self) -> AppConfig {
        let providers = normalized_provider_list(&self.minimal_config.available_providers);
        let planning_agents = providers.clone();
        let coding_agents = providers.clone();
        let review_agents = preferred_review_agents(&providers);
        let delivery_agents = preferred_delivery_agents(&providers);

        let mut agents = HashMap::new();
        for provider in &providers {
            if let Some(config) = default_agent_config(provider) {
                agents.insert(provider.clone(), config);
            }
        }

        let flow = FlowConfig {
            name: "Adaptive Flow".to_string(),
            phases: vec![
                "planning".to_string(),
                "coding".to_string(),
                "review".to_string(),
                "delivery".to_string(),
            ],
        };

        let mut phases = HashMap::new();
        phases.insert(
            "planning".to_string(),
            PhaseConfig {
                description: "Adaptive planning phase".to_string(),
                agents: planning_agents,
                fallback: Some(true),
                principles: Some(adaptive_principles(&self.learning_preferences, "planning")),
                options: Some(PhaseOptions {
                    request_timeout_seconds: Some(120),
                    ..PhaseOptions::default()
                }),
            },
        );
        phases.insert(
            "coding".to_string(),
            PhaseConfig {
                description: "Adaptive coding phase".to_string(),
                agents: coding_agents,
                fallback: Some(true),
                principles: Some(adaptive_principles(&self.learning_preferences, "coding")),
                options: Some(adaptive_coding_options(
                    self.minimal_config.enable_cache,
                    self.minimal_config.enable_vector_memory,
                    &review_agents,
                )),
            },
        );
        phases.insert(
            "review".to_string(),
            PhaseConfig {
                description: "Adaptive review phase".to_string(),
                agents: review_agents.clone(),
                fallback: Some(true),
                principles: Some(adaptive_principles(&self.learning_preferences, "review")),
                options: Some(adaptive_review_options()),
            },
        );
        phases.insert(
            "delivery".to_string(),
            PhaseConfig {
                description: "Adaptive delivery phase".to_string(),
                agents: delivery_agents,
                fallback: Some(false),
                principles: Some(adaptive_principles(&self.learning_preferences, "delivery")),
                options: Some(PhaseOptions {
                    request_timeout_seconds: Some(90),
                    ..PhaseOptions::default()
                }),
            },
        );

        let cache = if self.minimal_config.enable_cache {
            Some(CacheConfig {
                enabled: true,
                path: "acp_cache.sqlite3".to_string(),
                default_ttl_seconds: 3600,
                max_entries: 5000,
            })
        } else {
            None
        };

        let vector = if self.minimal_config.enable_vector_memory {
            Some(VectorConfig {
                enabled: true,
                auto_mode: true,
                path: "acp_vector.sqlite3".to_string(),
                dimensions: 192,
                min_query_chars: 80,
                top_k: 2,
                min_similarity: 0.82,
                max_snippet_chars: 800,
                max_entries: 10000,
                summary_enabled: true,
                summary_trigger_messages: 8,
                summary_max_chars: 1200,
            })
        } else {
            None
        };

        AppConfig {
            default_phase: self.minimal_config.default_phase.clone(),
            agents,
            flow,
            phases,
            runtime: Some(RuntimeConfig::default()),
            cache,
            vector,
            autotune: Some(default_autotune_config()),
            model_selection_mode: "adaptive".to_string(),
        }
    }
}

fn normalize_provider_name(agent_type: &str) -> Option<String> {
    provider_specs()
        .iter()
        .find(|spec| {
            spec.agent_type.eq_ignore_ascii_case(agent_type)
                || spec.name.eq_ignore_ascii_case(agent_type)
                || (spec.name == "anthropic" && agent_type.eq_ignore_ascii_case("claude"))
        })
        .map(|spec| spec.name.clone())
}

fn normalized_provider_list(providers: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = providers
        .iter()
        .filter_map(|provider| normalize_provider_name(provider))
        .collect();
    normalized.sort();
    normalized.dedup();

    if normalized.is_empty() {
        normalized.push("copilot".to_string());
    }

    normalized
}

fn default_agent_config(provider: &str) -> Option<AgentConfig> {
    let spec = provider_spec_by_name(provider)?;
    Some(AgentConfig {
        agent_type: spec.agent_type.clone(),
        url: spec.url.clone(),
        chat_path: spec.chat_path.clone(),
        api_key_env: spec.api_key_env.clone(),
        secret_key_env: spec.secret_key_env.clone(),
        anthropic_version: spec.anthropic_version.clone(),
        model: spec.model.clone(),
        max_tokens: spec.max_tokens,
        supports_system: Some(spec.supports_system.unwrap_or(true)),
    })
}

fn preferred_review_agents(providers: &[String]) -> Vec<String> {
    let mut reviewers: Vec<String> = providers
        .iter()
        .filter(|provider| provider.as_str() != "copilot")
        .cloned()
        .collect();

    if reviewers.is_empty() {
        reviewers = providers.to_vec();
    }

    if reviewers.is_empty() {
        vec!["copilot".to_string()]
    } else {
        reviewers
    }
}

fn preferred_delivery_agents(providers: &[String]) -> Vec<String> {
    if providers.iter().any(|provider| provider == "copilot") {
        return vec!["copilot".to_string()];
    }

    providers
        .first()
        .cloned()
        .map(|provider| vec![provider])
        .unwrap_or_else(|| vec!["copilot".to_string()])
}

fn adaptive_principles(preferences: &LearningPreferences, phase: &str) -> Vec<String> {
    let mut principles = vec![
        "Adapt agent choice to the task and available models".to_string(),
        "Prefer evidence-backed results and explicit verification".to_string(),
    ];

    if preferences.ask_for_clarification {
        principles
            .push("Ask for clarification only when uncertainty blocks correctness".to_string());
    }

    match phase {
        "planning" => principles.push("Keep plans minimal and execution-oriented".to_string()),
        "coding" => principles.push("Make the smallest correct change".to_string()),
        "review" => {
            principles.push("Prioritize regressions, risks, and missing validation".to_string())
        }
        "delivery" => principles.push("Summarize outcome and residual risks concisely".to_string()),
        _ => {}
    }

    principles
}

fn adaptive_coding_options(
    enable_cache: bool,
    enable_vector_memory: bool,
    review_agents: &[String],
) -> PhaseOptions {
    let mut extra = HashMap::new();
    extra.insert("review_gate_timeout_seconds".to_string(), Value::from(90));
    extra.insert("phase_max_inflight".to_string(), Value::from(24));
    extra.insert("global_max_inflight".to_string(), Value::from(128));

    PhaseOptions {
        cache_enabled: Some(enable_cache),
        vector_enabled: Some(enable_vector_memory),
        summary_enabled: Some(enable_vector_memory),
        autopilot_complexity: Some("auto".to_string()),
        full_auto_review_agents: Some(review_agents.iter().take(2).cloned().collect()),
        request_timeout_seconds: Some(150),
        review_timeout_seconds: Some(60),
        extra,
        ..PhaseOptions::default()
    }
}

fn adaptive_review_options() -> PhaseOptions {
    let mut extra = HashMap::new();
    extra.insert("review_timeout_policy".to_string(), Value::from("reject"));
    extra.insert("review_gate_timeout_seconds".to_string(), Value::from(90));
    extra.insert("phase_max_inflight".to_string(), Value::from(16));
    extra.insert("global_max_inflight".to_string(), Value::from(128));

    PhaseOptions {
        request_timeout_seconds: Some(60),
        extra,
        ..PhaseOptions::default()
    }
}

fn default_autotune_config() -> AutoTuneConfig {
    AutoTuneConfig {
        enabled: false,
        evaluate_interval: default_autotune_evaluate_interval(),
        min_query_chars_step: default_autotune_min_query_chars_step(),
        min_query_chars_min: default_autotune_min_query_chars_min(),
        min_query_chars_max: default_autotune_min_query_chars_max(),
        max_top_k: default_autotune_max_top_k(),
        low_precision_threshold: default_autotune_low_precision(),
        high_precision_threshold: default_autotune_high_precision(),
        state_path: default_autotune_state_path(),
        cooldown_windows: default_autotune_cooldown_windows(),
        min_vector_searches: default_autotune_min_vector_searches(),
        summary_trigger_min: default_autotune_summary_trigger_min(),
        summary_trigger_max: default_autotune_summary_trigger_max(),
    }
}

/// Helper function to get current timestamp
fn now_ts() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Configuration warning severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigWarningSeverity {
    /// Informational warning
    Info,
    /// Warning that may affect functionality
    Warn,
    /// Critical issue that will prevent proper operation
    Critical,
}

/// Configuration warning structure
#[derive(Debug, Clone, Serialize)]
pub struct ConfigWarning {
    /// Warning code
    pub code: String,
    /// Warning severity
    pub severity: ConfigWarningSeverity,
    /// Warning message
    pub message: String,
}

/// Configuration health report
#[derive(Debug, Clone, Serialize)]
pub struct ConfigHealthReport {
    /// Health score (0-100)
    pub score: u32,
    /// Total number of warnings
    pub total: usize,
    /// Number of informational warnings
    pub info_count: usize,
    /// Number of warnings
    pub warn_count: usize,
    /// Number of critical warnings
    pub critical_count: usize,
    /// Recommended profile based on current warning/risk posture
    pub profile_recommendation: String,
    /// Actionable recommendations for improving configuration quality
    pub recommendations: Vec<String>,
    /// List of warnings
    pub warnings: Vec<ConfigWarning>,
}

impl ConfigHealthReport {
    /// Get all warning messages
    ///
    /// # Returns
    /// * `Vec<String>` - List of warning messages
    pub fn warning_messages(&self) -> Vec<String> {
        self.warnings
            .iter()
            .map(|item| item.message.clone())
            .collect()
    }
}

/// Runtime configuration
#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeConfig {
    /// 协议模式: auto/acp/mcp
    #[serde(default)]
    pub protocol_mode: Option<String>,
    /// Emit PUA execution report into JSON-RPC response metadata when enabled
    #[serde(default)]
    pub pua_report: bool,
    /// Deployment target for hardening profile selection: local-dev | ci | managed-service
    #[serde(default)]
    pub deployment_target: Option<String>,
    /// Maintenance interval in seconds
    #[serde(default = "default_runtime_maintenance_interval_seconds")]
    pub maintenance_interval_seconds: u64,
    /// Health check interval in seconds
    #[serde(default = "default_runtime_health_interval_seconds")]
    pub health_interval_seconds: u64,
    /// Shutdown drain time in seconds
    #[serde(default = "default_runtime_shutdown_drain_seconds")]
    pub shutdown_drain_seconds: u64,
    /// Optional ACP HTTP bind address for REST/SSE endpoints
    #[serde(default)]
    pub acp_http_bind_addr: Option<String>,
    /// Whether inbound entry auth is enabled at gateway/edge for exposed HTTP endpoints
    #[serde(default)]
    pub entry_auth_enabled: bool,
    /// Env var name holding entry API key used for HTTP ingress auth
    #[serde(default = "default_runtime_entry_auth_api_key_env")]
    pub entry_auth_api_key_env: String,
    /// Entry layer source-based rate limit (requests per minute)
    #[serde(default = "default_runtime_entry_rate_limit_rpm")]
    pub entry_rate_limit_rpm: u64,
    /// Entry layer token bucket burst capacity per source
    #[serde(default = "default_runtime_entry_rate_limit_burst")]
    pub entry_rate_limit_burst: u64,
    /// Enforce production strict fail-fast checks on unsafe runtime configuration
    #[serde(default)]
    pub production_strict: bool,
    /// How often background maintenance performs SQLite VACUUM cycles
    #[serde(default = "default_runtime_sqlite_vacuum_interval_cycles")]
    pub sqlite_vacuum_interval_cycles: u64,
    /// Enable OpenTelemetry exporter for distributed traces
    #[serde(default)]
    pub otel_enabled: bool,
    /// Exporter type: otlp or jaeger (jaeger uses OTLP endpoint)
    #[serde(default = "default_runtime_otel_exporter")]
    pub otel_exporter: String,
    /// Optional OTLP endpoint (for Jaeger, point to collector OTLP endpoint)
    #[serde(default)]
    pub otel_endpoint: Option<String>,
    /// OpenTelemetry service name
    #[serde(default = "default_runtime_otel_service_name")]
    pub otel_service_name: String,
    /// Sampling ratio in [0.0, 1.0]
    #[serde(default = "default_runtime_otel_sample_ratio")]
    pub otel_sample_ratio: f64,
    /// Number of slow requests to keep in top-N trace metrics
    #[serde(default = "default_runtime_trace_slow_top_n")]
    pub trace_slow_top_n: usize,
    /// Enable builtin skills (e.g. `builtin.echo`) at server startup.
    /// Default is `true` for development; set to `false` in production (`config.production.toml`).
    #[serde(default = "default_runtime_skills_enabled")]
    pub skills_enabled: bool,
    /// Enable skills import APIs (`skill.import`, `skill.enable`, etc.).
    #[serde(default)]
    pub skills_import_enabled: bool,
    /// Allowed source prefixes for importing skills. Supports trailing `*` wildcard prefix matching.
    #[serde(default)]
    pub skills_allowed_sources: Vec<String>,
    /// Require import requests to provide expected SHA256 digest.
    #[serde(default = "default_runtime_skills_require_sha256")]
    pub skills_require_sha256: bool,
    /// Allow floating refs (`main`, `latest`, non-SHA refs`) when importing from GitHub.
    #[serde(default)]
    pub skills_allow_floating_ref: bool,
    /// Cache directory used to persist imported skill manifests and index.
    #[serde(default = "default_runtime_skills_cache_dir")]
    pub skills_cache_dir: String,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            protocol_mode: None,
            pua_report: false,
            deployment_target: None,
            maintenance_interval_seconds: default_runtime_maintenance_interval_seconds(),
            health_interval_seconds: default_runtime_health_interval_seconds(),
            shutdown_drain_seconds: default_runtime_shutdown_drain_seconds(),
            acp_http_bind_addr: None,
            entry_auth_enabled: false,
            entry_auth_api_key_env: default_runtime_entry_auth_api_key_env(),
            entry_rate_limit_rpm: default_runtime_entry_rate_limit_rpm(),
            entry_rate_limit_burst: default_runtime_entry_rate_limit_burst(),
            production_strict: false,
            sqlite_vacuum_interval_cycles: default_runtime_sqlite_vacuum_interval_cycles(),
            otel_enabled: false,
            otel_exporter: default_runtime_otel_exporter(),
            otel_endpoint: None,
            otel_service_name: default_runtime_otel_service_name(),
            otel_sample_ratio: default_runtime_otel_sample_ratio(),
            trace_slow_top_n: default_runtime_trace_slow_top_n(),
            skills_enabled: default_runtime_skills_enabled(),
            skills_import_enabled: false,
            skills_allowed_sources: Vec::new(),
            skills_require_sha256: default_runtime_skills_require_sha256(),
            skills_allow_floating_ref: false,
            skills_cache_dir: default_runtime_skills_cache_dir(),
        }
    }
}

fn default_runtime_maintenance_interval_seconds() -> u64 {
    60
}

fn default_runtime_health_interval_seconds() -> u64 {
    120
}

fn default_runtime_shutdown_drain_seconds() -> u64 {
    30
}

fn default_runtime_entry_auth_api_key_env() -> String {
    "GO_ON_ENTRY_API_KEY".to_string()
}

fn default_runtime_entry_rate_limit_rpm() -> u64 {
    240
}

fn default_runtime_entry_rate_limit_burst() -> u64 {
    60
}

fn default_runtime_sqlite_vacuum_interval_cycles() -> u64 {
    60
}

fn default_runtime_otel_exporter() -> String {
    "otlp".to_string()
}

fn default_runtime_otel_service_name() -> String {
    "go-on".to_string()
}

fn default_runtime_otel_sample_ratio() -> f64 {
    1.0
}

fn default_runtime_trace_slow_top_n() -> usize {
    20
}

fn default_runtime_skills_enabled() -> bool {
    true
}

fn default_runtime_skills_require_sha256() -> bool {
    true
}

fn default_runtime_skills_cache_dir() -> String {
    "skills_cache".to_string()
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct AutoTuneConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_autotune_evaluate_interval")]
    pub evaluate_interval: usize,
    #[serde(default = "default_autotune_min_query_chars_step")]
    pub min_query_chars_step: usize,
    #[serde(default = "default_autotune_min_query_chars_min")]
    pub min_query_chars_min: usize,
    #[serde(default = "default_autotune_min_query_chars_max")]
    pub min_query_chars_max: usize,
    #[serde(default = "default_autotune_max_top_k")]
    pub max_top_k: usize,
    #[serde(default = "default_autotune_low_precision")]
    pub low_precision_threshold: f32,
    #[serde(default = "default_autotune_high_precision")]
    pub high_precision_threshold: f32,
    #[serde(default = "default_autotune_state_path")]
    pub state_path: String,
    #[serde(default = "default_autotune_cooldown_windows")]
    pub cooldown_windows: usize,
    #[serde(default = "default_autotune_min_vector_searches")]
    pub min_vector_searches: usize,
    #[serde(default = "default_autotune_summary_trigger_min")]
    pub summary_trigger_min: usize,
    #[serde(default = "default_autotune_summary_trigger_max")]
    pub summary_trigger_max: usize,
}

fn default_autotune_evaluate_interval() -> usize {
    20
}

fn default_autotune_min_query_chars_step() -> usize {
    20
}

fn default_autotune_min_query_chars_min() -> usize {
    40
}

fn default_autotune_min_query_chars_max() -> usize {
    300
}

fn default_autotune_max_top_k() -> usize {
    4
}

fn default_autotune_low_precision() -> f32 {
    0.35
}

fn default_autotune_high_precision() -> f32 {
    0.75
}

fn default_autotune_state_path() -> String {
    "acp_autotune_state.json".to_string()
}

fn default_autotune_cooldown_windows() -> usize {
    2
}

fn default_autotune_min_vector_searches() -> usize {
    5
}

fn default_autotune_summary_trigger_min() -> usize {
    3
}

fn default_autotune_summary_trigger_max() -> usize {
    20
}

/// Runtime autotune state: tracks current parameter values and precision feedback metrics.
/// Persisted to JSON file at state_path to survive across server restarts.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AutoTuneState {
    /// Current minimum query character threshold for vector searches.
    pub current_min_query_chars: usize,
    /// Current top-k value for vector result limiting.
    pub current_top_k: usize,
    /// Which evaluation window we're in (incremented every evaluate_interval searches).
    pub window_phase: usize,
    /// Number of vector searches with high precision (above high_precision_threshold).
    pub high_precision_count: usize,
    /// Number of vector searches with low precision (below low_precision_threshold).
    pub low_precision_count: usize,
    /// Total vector searches in current window.
    pub vector_search_count: usize,
    /// Windows remaining before next adjustment is allowed (cooldown logic).
    pub cooldown_remaining: usize,
}

impl AutoTuneState {
    /// Create new state from AutoTuneConfig defaults.
    pub fn new(config: &AutoTuneConfig) -> Self {
        Self {
            current_min_query_chars: config.min_query_chars_min,
            current_top_k: 2, // Conservative initial value
            window_phase: 0,
            high_precision_count: 0,
            low_precision_count: 0,
            vector_search_count: 0,
            cooldown_remaining: 0,
        }
    }

    /// Load state from JSON file, or return new default if file doesn't exist.
    pub fn load_or_default(path: &str, config: &AutoTuneConfig) -> Self {
        match fs::read_to_string(path) {
            Ok(content) => match serde_json::from_str::<AutoTuneState>(&content) {
                Ok(state) => state,
                Err(e) => {
                    warn!(
                        "failed to parse autotune state from {}: {}, using defaults",
                        path, e
                    );
                    Self::new(config)
                }
            },
            Err(_) => Self::new(config),
        }
    }

    /// Save state to JSON file.
    pub fn save(&self, path: &str) -> Result<()> {
        let json =
            serde_json::to_string_pretty(self).context("failed to serialize autotune state")?;
        fs::write(path, json).context("failed to write autotune state to file")?;
        Ok(())
    }

    /// Record a vector search result with precision score.
    /// Called after each vector search to update metrics.
    pub fn record_vector_search(&mut self, precision: f32, config: &AutoTuneConfig) {
        if precision >= config.high_precision_threshold {
            self.high_precision_count += 1;
        } else if precision < config.low_precision_threshold {
            self.low_precision_count += 1;
        }
        self.vector_search_count += 1;
    }

    /// Advance one evaluation window while cooling down.
    /// This prevents the tuner from getting stuck with a non-zero cooldown.
    pub fn advance_cooldown_window(&mut self, config: &AutoTuneConfig) -> bool {
        if self.cooldown_remaining == 0 || self.vector_search_count < config.evaluate_interval {
            return false;
        }

        self.vector_search_count = 0;
        self.high_precision_count = 0;
        self.low_precision_count = 0;
        self.window_phase += 1;
        self.cooldown_remaining -= 1;
        true
    }

    /// Determine if it's time to evaluate and possibly adjust parameters.
    /// Returns true if adjustment window reached and cooldown expired.
    pub fn should_evaluate(&self, config: &AutoTuneConfig) -> bool {
        self.vector_search_count >= config.evaluate_interval && self.cooldown_remaining == 0
    }

    /// Evaluate precision metrics and adjust parameters if needed.
    /// Returns true if parameters were adjusted.
    pub fn evaluate_and_adjust(&mut self, config: &AutoTuneConfig) -> bool {
        if !self.should_evaluate(config) {
            return false;
        }

        if self.vector_search_count < config.min_vector_searches {
            // Not enough data, reset counters and proceed to next window
            self.vector_search_count = 0;
            self.high_precision_count = 0;
            self.low_precision_count = 0;
            self.window_phase += 1;
            return false;
        }

        let high_precision_ratio =
            self.high_precision_count as f32 / self.vector_search_count as f32;
        let low_precision_ratio = self.low_precision_count as f32 / self.vector_search_count as f32;

        let adjusted = if high_precision_ratio > 0.6 {
            // Most results are good - we can be more selective
            self.increase_min_query_chars(config)
        } else if low_precision_ratio > 0.4 {
            // Many poor results - relax the filter
            self.decrease_min_query_chars(config)
        } else {
            false
        };

        // Reset counters and move to next window
        self.vector_search_count = 0;
        self.high_precision_count = 0;
        self.low_precision_count = 0;
        self.window_phase += 1;

        if adjusted {
            self.cooldown_remaining = config.cooldown_windows;
        } else {
            self.cooldown_remaining = 0;
        }

        adjusted
    }

    /// Increase min_query_chars to be more selective (fewer but better results).
    fn increase_min_query_chars(&mut self, config: &AutoTuneConfig) -> bool {
        let new_value = (self.current_min_query_chars + config.min_query_chars_step)
            .min(config.min_query_chars_max);
        if new_value != self.current_min_query_chars {
            info!(
                "autotune: increasing min_query_chars from {} to {}",
                self.current_min_query_chars, new_value
            );
            self.current_min_query_chars = new_value;
            true
        } else {
            false
        }
    }

    /// Decrease min_query_chars to be more permissive (more results).
    fn decrease_min_query_chars(&mut self, config: &AutoTuneConfig) -> bool {
        let new_value = self
            .current_min_query_chars
            .saturating_sub(config.min_query_chars_step)
            .max(config.min_query_chars_min);
        if new_value != self.current_min_query_chars {
            info!(
                "autotune: decreasing min_query_chars from {} to {}",
                self.current_min_query_chars, new_value
            );
            self.current_min_query_chars = new_value;
            true
        } else {
            false
        }
    }

    /// Return current tuning state as JSON for RPC responses.
    pub fn snapshot(&self) -> Value {
        serde_json::json!({
            "current_min_query_chars": self.current_min_query_chars,
            "current_top_k": self.current_top_k,
            "window_phase": self.window_phase,
            "high_precision_count": self.high_precision_count,
            "low_precision_count": self.low_precision_count,
            "vector_search_count": self.vector_search_count,
            "cooldown_remaining": self.cooldown_remaining,
        })
    }

    /// Decrement cooldown counter (called once per evaluation window).
    #[allow(dead_code)]
    pub fn tick_cooldown(&mut self) {
        if self.cooldown_remaining > 0 {
            self.cooldown_remaining -= 1;
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CacheConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_cache_path")]
    pub path: String,
    #[serde(default = "default_cache_ttl_seconds")]
    pub default_ttl_seconds: u64,
    #[serde(default = "default_cache_max_entries")]
    pub max_entries: usize,
}

fn default_cache_path() -> String {
    "acp_cache.sqlite3".to_string()
}

fn default_cache_ttl_seconds() -> u64 {
    3600
}

fn default_cache_max_entries() -> usize {
    5000
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct VectorConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_vector_auto_mode")]
    pub auto_mode: bool,
    #[serde(default = "default_vector_path")]
    pub path: String,
    #[serde(default = "default_vector_dimensions")]
    pub dimensions: usize,
    #[serde(default = "default_vector_min_query_chars")]
    pub min_query_chars: usize,
    #[serde(default = "default_vector_top_k")]
    pub top_k: usize,
    #[serde(default = "default_vector_min_similarity")]
    pub min_similarity: f32,
    #[serde(default = "default_vector_max_snippet_chars")]
    pub max_snippet_chars: usize,
    #[serde(default = "default_vector_max_entries")]
    pub max_entries: usize,
    #[serde(default = "default_summary_enabled")]
    pub summary_enabled: bool,
    #[serde(default = "default_summary_trigger_messages")]
    pub summary_trigger_messages: usize,
    #[serde(default = "default_summary_max_chars")]
    pub summary_max_chars: usize,
}

fn default_vector_auto_mode() -> bool {
    true
}

fn default_vector_path() -> String {
    "acp_vector.sqlite3".to_string()
}

fn default_vector_dimensions() -> usize {
    192
}

fn default_vector_min_query_chars() -> usize {
    80
}

fn default_vector_top_k() -> usize {
    2
}

fn default_vector_min_similarity() -> f32 {
    0.82
}

fn default_vector_max_snippet_chars() -> usize {
    800
}

fn default_vector_max_entries() -> usize {
    10000
}

fn default_summary_enabled() -> bool {
    true
}

fn default_summary_trigger_messages() -> usize {
    8
}

fn default_summary_max_chars() -> usize {
    1200
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    #[serde(rename = "type")]
    pub agent_type: String,
    pub url: Option<String>,
    pub chat_path: Option<String>,
    pub api_key_env: Option<String>,
    pub secret_key_env: Option<String>,
    pub anthropic_version: Option<String>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub supports_system: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FlowConfig {
    pub name: String,
    pub phases: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PhaseConfig {
    pub description: String,
    pub agents: Vec<String>,
    pub fallback: Option<bool>,
    pub principles: Option<Vec<String>>,
    pub options: Option<PhaseOptions>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PhaseOptions {
    pub cache_enabled: Option<bool>,
    pub cache_ttl_seconds: Option<u64>,
    pub vector_enabled: Option<bool>,
    pub vector_auto: Option<bool>,
    pub vector_min_query_chars: Option<usize>,
    pub vector_top_k: Option<usize>,
    pub vector_min_similarity: Option<f32>,
    pub vector_max_snippet_chars: Option<usize>,
    pub summary_enabled: Option<bool>,
    pub summary_trigger_messages: Option<usize>,
    pub summary_max_chars: Option<usize>,
    pub max_history_messages: Option<usize>,
    pub max_history_chars: Option<usize>,
    pub autopilot_complexity: Option<String>,
    pub full_auto_review_agents: Option<Vec<String>>,
    pub request_timeout_seconds: Option<u64>,
    pub review_timeout_seconds: Option<u64>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl PhaseOptions {
    pub fn agent_options(&self) -> Option<HashMap<String, Value>> {
        if self.extra.is_empty() {
            None
        } else {
            Some(self.extra.clone())
        }
    }
}

impl AppConfig {
    /// Load configuration from file
    ///
    /// # Arguments
    /// * `path` - Path to configuration file
    ///
    /// # Returns
    /// * `Result<Self>` - Returns Ok(Self) if loaded successfully, or an error if something goes wrong
    pub fn load(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file: {}", path.display()))?;

        let normalized = if content.trim().is_empty() {
            let bootstrap = default_non_ai_config_toml();
            fs::write(path, &bootstrap).with_context(|| {
                format!(
                    "failed to write bootstrap defaults to blank config: {}",
                    path.display()
                )
            })?;
            info!(
                "blank config detected; wrote non-AI bootstrap defaults to {}",
                path.display()
            );
            bootstrap
        } else {
            content
        };

        let mut cfg: AppConfig = toml::from_str(&normalized)
            .with_context(|| format!("failed to parse toml: {}", path.display()))?;
        normalize_nested_phase_option_extra(&mut cfg);
        apply_auto_rules(path, &mut cfg);
        Ok(cfg)
    }

    /// Validate configuration
    ///
    /// This method performs comprehensive validation of the configuration, including:
    /// - Checking that flow.phases is not empty
    /// - Verifying that default_phase is in flow.phases
    /// - Ensuring all phases in flow.phases are defined
    /// - Validating that each phase references only defined agents
    /// - Checking that all agents referenced in phases exist
    /// - Validating phase options
    /// - Verifying complex autopilot requirements
    ///
    /// # Returns
    /// * `Result<()>` - Returns Ok(()) if validation passes, or an error if validation fails
    pub fn validate(&self) -> Result<()> {
        if self.flow.phases.is_empty() {
            anyhow::bail!("flow.phases must not be empty");
        }

        if !self
            .flow
            .phases
            .iter()
            .any(|phase| phase == &self.default_phase)
        {
            anyhow::bail!(
                "default_phase '{}' is not listed in flow.phases",
                self.default_phase
            );
        }

        for phase_name in &self.flow.phases {
            let phase_cfg = self
                .phases
                .get(phase_name)
                .with_context(|| format!("phase '{}' missing in [phases]", phase_name))?;

            for agent_name in &phase_cfg.agents {
                if !self.agents.contains_key(agent_name) {
                    anyhow::bail!(
                        "phase '{}' references undefined agent '{}'",
                        phase_name,
                        agent_name
                    );
                }
            }

            if let Some(options) = phase_cfg.options.as_ref() {
                validate_phase_options(phase_name, options)?;
            }

            if phase_uses_complex_autopilot(phase_cfg.options.as_ref()) {
                if !self.flow.phases.iter().any(|phase| phase == "review") {
                    anyhow::bail!(
                        "phase '{}' uses complex autopilot but flow.phases does not include 'review'",
                        phase_name
                    );
                }

                let review_phase = self
                    .phases
                    .get("review")
                    .with_context(|| "complex autopilot requires a [phases.review] definition")?;

                let reviewers = phase_cfg
                    .options
                    .as_ref()
                    .and_then(|options| options.full_auto_review_agents.clone())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "phase '{}' uses complex autopilot but does not define full_auto_review_agents",
                            phase_name
                        )
                    })?;

                if reviewers.len() < 2 {
                    anyhow::bail!(
                        "phase '{}' uses complex autopilot but must define at least 2 full_auto_review_agents",
                        phase_name
                    );
                }
                if reviewers.len() > 2 {
                    anyhow::bail!(
                        "phase '{}' uses complex autopilot but full_auto_review_agents supports at most 2 reviewers",
                        phase_name
                    );
                }

                if review_phase.agents.len() < 2 {
                    anyhow::bail!(
                        "[phases.review] must contain at least 2 agents when complex autopilot is enabled"
                    );
                }

                for reviewer in reviewers.iter().take(2) {
                    if !self.agents.contains_key(reviewer) {
                        anyhow::bail!(
                            "phase '{}' references undefined review agent '{}'",
                            phase_name,
                            reviewer
                        );
                    }

                    if !review_phase.agents.iter().any(|agent| agent == reviewer) {
                        anyhow::bail!(
                            "review agent '{}' must also appear in [phases.review].agents",
                            reviewer
                        );
                    }
                }
            }
        }

        if let Some(cache) = &self.cache {
            if cache.enabled {
                if cache.default_ttl_seconds == 0 {
                    anyhow::bail!("cache.default_ttl_seconds must be > 0 when cache is enabled");
                }
                if cache.max_entries == 0 {
                    anyhow::bail!("cache.max_entries must be > 0 when cache is enabled");
                }
            }
        }

        if let Some(runtime) = &self.runtime {
            if runtime.maintenance_interval_seconds == 0 {
                anyhow::bail!("runtime.maintenance_interval_seconds must be > 0");
            }
            if runtime.health_interval_seconds == 0 {
                anyhow::bail!("runtime.health_interval_seconds must be > 0");
            }
            if runtime.shutdown_drain_seconds == 0 {
                anyhow::bail!("runtime.shutdown_drain_seconds must be > 0");
            }
            if runtime.entry_auth_api_key_env.trim().is_empty() {
                anyhow::bail!("runtime.entry_auth_api_key_env must not be empty");
            }
            if runtime.entry_rate_limit_rpm == 0 {
                anyhow::bail!("runtime.entry_rate_limit_rpm must be > 0");
            }
            if runtime.entry_rate_limit_burst == 0 {
                anyhow::bail!("runtime.entry_rate_limit_burst must be > 0");
            }
            if runtime.sqlite_vacuum_interval_cycles == 0 {
                anyhow::bail!("runtime.sqlite_vacuum_interval_cycles must be > 0");
            }
            if !(0.0..=1.0).contains(&runtime.otel_sample_ratio) {
                anyhow::bail!("runtime.otel_sample_ratio must be in [0.0, 1.0]");
            }
            if runtime.trace_slow_top_n == 0 {
                anyhow::bail!("runtime.trace_slow_top_n must be > 0");
            }
            let exporter = runtime.otel_exporter.to_ascii_lowercase();
            if runtime.otel_enabled && exporter != "otlp" && exporter != "jaeger" {
                anyhow::bail!("runtime.otel_exporter must be 'otlp' or 'jaeger'");
            }
        }

        if let Some(vector) = &self.vector {
            if vector.enabled {
                if vector.dimensions == 0 {
                    anyhow::bail!("vector.dimensions must be > 0 when vector is enabled");
                }
                if vector.top_k == 0 {
                    anyhow::bail!("vector.top_k must be > 0 when vector is enabled");
                }
                if !(0.0..=1.0).contains(&vector.min_similarity) {
                    anyhow::bail!(
                        "vector.min_similarity must be in [0.0, 1.0] when vector is enabled"
                    );
                }
                if vector.max_entries == 0 {
                    anyhow::bail!("vector.max_entries must be > 0 when vector is enabled");
                }
                if vector.summary_trigger_messages == 0 {
                    anyhow::bail!(
                        "vector.summary_trigger_messages must be > 0 when vector is enabled"
                    );
                }
                if vector.summary_max_chars == 0 {
                    anyhow::bail!("vector.summary_max_chars must be > 0 when vector is enabled");
                }
            }
        }

        if let Some(autotune) = &self.autotune {
            if autotune.enabled {
                if autotune.evaluate_interval == 0 {
                    anyhow::bail!("autotune.evaluate_interval must be > 0 when enabled");
                }
                if autotune.min_query_chars_step == 0 {
                    anyhow::bail!("autotune.min_query_chars_step must be > 0 when enabled");
                }
                if autotune.min_query_chars_min == 0 {
                    anyhow::bail!("autotune.min_query_chars_min must be > 0 when enabled");
                }
                if autotune.min_query_chars_min > autotune.min_query_chars_max {
                    anyhow::bail!(
                        "autotune.min_query_chars_min must be <= autotune.min_query_chars_max"
                    );
                }
                if autotune.max_top_k == 0 {
                    anyhow::bail!("autotune.max_top_k must be > 0 when enabled");
                }
                if !(0.0..=1.0).contains(&autotune.low_precision_threshold) {
                    anyhow::bail!("autotune.low_precision_threshold must be in [0, 1]");
                }
                if !(0.0..=1.0).contains(&autotune.high_precision_threshold) {
                    anyhow::bail!("autotune.high_precision_threshold must be in [0, 1]");
                }
                if autotune.low_precision_threshold >= autotune.high_precision_threshold {
                    anyhow::bail!(
                        "autotune.low_precision_threshold must be < autotune.high_precision_threshold"
                    );
                }
                if autotune.min_vector_searches == 0 {
                    anyhow::bail!("autotune.min_vector_searches must be > 0 when enabled");
                }
                if autotune.summary_trigger_min == 0 {
                    anyhow::bail!("autotune.summary_trigger_min must be > 0 when enabled");
                }
                if autotune.summary_trigger_min > autotune.summary_trigger_max {
                    anyhow::bail!(
                        "autotune.summary_trigger_min must be <= autotune.summary_trigger_max"
                    );
                }
            }
        }

        Ok(())
    }
}

fn default_non_ai_config_toml() -> String {
    [
        "default_phase = \"coding\"",
        "model_selection_mode = \"adaptive\"",
        "",
        "[protocol]",
        "mode = \"auto\"",
        "",
        "[cache]",
        "enabled = true",
        "path = \"acp_cache.sqlite3\"",
        "default_ttl_seconds = 3600",
        "max_entries = 5000",
        "",
        "[vector]",
        "enabled = true",
        "auto_mode = true",
        "path = \"acp_vector.sqlite3\"",
        "dimensions = 192",
        "min_query_chars = 80",
        "top_k = 2",
        "min_similarity = 0.82",
        "max_snippet_chars = 800",
        "max_entries = 10000",
        "summary_enabled = true",
        "summary_trigger_messages = 8",
        "summary_max_chars = 1200",
        "",
        "[runtime]",
        "maintenance_interval_seconds = 60",
        "health_interval_seconds = 120",
        "shutdown_drain_seconds = 30",
        "sqlite_vacuum_interval_cycles = 60",
        "",
        "[autotune]",
        "enabled = false",
        "evaluate_interval = 20",
        "min_query_chars_step = 20",
        "min_query_chars_min = 40",
        "min_query_chars_max = 300",
        "max_top_k = 4",
        "low_precision_threshold = 0.35",
        "high_precision_threshold = 0.75",
        "state_path = \"acp_autotune_state.json\"",
        "cooldown_windows = 2",
        "min_vector_searches = 5",
        "summary_trigger_min = 3",
        "summary_trigger_max = 20",
        "",
        "[agents]",
        "",
        "[flow]",
        "name = \"Autopilot Adaptive\"",
        "phases = [\"planning\", \"coding\", \"review\", \"delivery\"]",
        "",
        "[phases.planning]",
        "description = \"Planning phase\"",
        "agents = []",
        "fallback = true",
        "",
        "[phases.coding]",
        "description = \"Coding phase\"",
        "agents = []",
        "fallback = true",
        "",
        "[phases.coding.options]",
        "autopilot_complexity = \"auto\"",
        "request_timeout_seconds = 150",
        "review_timeout_seconds = 60",
        "cache_enabled = true",
        "vector_enabled = true",
        "summary_enabled = true",
        "full_auto_review_agents = []",
        "phase_max_inflight = 24",
        "global_max_inflight = 128",
        "",
        "[phases.review]",
        "description = \"Review phase\"",
        "agents = []",
        "fallback = true",
        "",
        "[phases.review.options]",
        "request_timeout_seconds = 60",
        "review_timeout_policy = \"reject\"",
        "review_min_response_chars = 12",
        "phase_max_inflight = 16",
        "global_max_inflight = 128",
        "",
        "[phases.delivery]",
        "description = \"Delivery phase\"",
        "agents = []",
        "fallback = false",
        "",
        "[phases.delivery.options]",
        "request_timeout_seconds = 90",
    ]
    .join("\n")
}

fn normalize_nested_phase_option_extra(config: &mut AppConfig) {
    for phase in config.phases.values_mut() {
        let Some(options) = phase.options.as_mut() else {
            continue;
        };

        let nested_extra = options.extra.remove("extra");
        let Some(Value::Object(map)) = nested_extra else {
            continue;
        };

        for (key, value) in map {
            options.extra.entry(key).or_insert(value);
        }
    }
}

fn apply_auto_rules(config_path: &Path, config: &mut AppConfig) {
    let config_dir = config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));

    let mut shared_rules = Vec::new();
    for path in shared_rule_paths(config_dir) {
        append_unique(&mut shared_rules, load_optional_rule_items(&path));
    }

    for (phase_name, phase_cfg) in config.phases.iter_mut() {
        let mut merged = phase_cfg.principles.clone().unwrap_or_default();
        append_unique(&mut merged, shared_rules.clone());

        for path in phase_rule_paths(config_dir, phase_name) {
            append_unique(&mut merged, load_optional_rule_items(&path));
        }

        phase_cfg.principles = if merged.is_empty() {
            None
        } else {
            Some(merged)
        };
    }
}

fn shared_rule_paths(config_dir: &Path) -> Vec<std::path::PathBuf> {
    let rules_dir = config_dir.join("RULES");
    vec![
        config_dir.join("RULES.md"),
        rules_dir.join("global.md"),
        rules_dir.join("common.md"),
        rules_dir.join("local.md"),
    ]
}

fn phase_rule_paths(config_dir: &Path, phase_name: &str) -> Vec<std::path::PathBuf> {
    let rules_dir = config_dir.join("RULES");
    vec![
        config_dir.join(format!("{}.rules.md", phase_name)),
        rules_dir.join(format!("{}.md", phase_name)),
        rules_dir.join(format!("{}.rules.md", phase_name)),
        rules_dir.join(format!("{}.local.md", phase_name)),
    ]
}

fn load_optional_rule_items(path: &Path) -> Vec<String> {
    if !path.exists() {
        return Vec::new();
    }

    match fs::read_to_string(path) {
        Ok(content) => parse_rule_items(&content),
        Err(err) => {
            warn!(
                "failed to read optional rule file {}: {}",
                path.display(),
                err
            );
            Vec::new()
        }
    }
}

fn parse_rule_items(content: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut in_code_block = false;

    for raw_line in content.lines() {
        let trimmed = raw_line.trim();

        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let line = strip_rule_prefix(trimmed).trim();
        if !line.is_empty() {
            items.push(line.to_string());
        }
    }

    items
}

fn strip_rule_prefix(line: &str) -> &str {
    for prefix in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return rest;
        }
    }

    strip_ordered_prefix(line).unwrap_or(line)
}

fn strip_ordered_prefix(line: &str) -> Option<&str> {
    let mut idx = 0;
    for ch in line.chars() {
        if ch.is_ascii_digit() {
            idx += ch.len_utf8();
            continue;
        }
        break;
    }

    if idx == 0 || idx + 1 >= line.len() {
        return None;
    }

    let marker = line.as_bytes()[idx] as char;
    if (marker == '.' || marker == ')') && line.as_bytes()[idx + 1] == b' ' {
        return Some(&line[idx + 2..]);
    }

    None
}

fn append_unique(target: &mut Vec<String>, items: Vec<String>) {
    for item in items {
        if !target.iter().any(|existing| existing == &item) {
            target.push(item);
        }
    }
}

fn validate_phase_options(phase_name: &str, options: &PhaseOptions) -> Result<()> {
    if matches!(options.cache_ttl_seconds, Some(0)) {
        anyhow::bail!("phase '{}' cache_ttl_seconds must be > 0", phase_name);
    }
    if matches!(options.vector_min_query_chars, Some(0)) {
        anyhow::bail!("phase '{}' vector_min_query_chars must be > 0", phase_name);
    }
    if matches!(options.vector_top_k, Some(0)) {
        anyhow::bail!("phase '{}' vector_top_k must be > 0", phase_name);
    }
    if let Some(value) = options.vector_min_similarity {
        if !(0.0..=1.0).contains(&value) {
            anyhow::bail!(
                "phase '{}' vector_min_similarity must be in [0.0, 1.0]",
                phase_name
            );
        }
    }
    if matches!(options.vector_max_snippet_chars, Some(0)) {
        anyhow::bail!(
            "phase '{}' vector_max_snippet_chars must be > 0",
            phase_name
        );
    }
    if matches!(options.summary_trigger_messages, Some(0)) {
        anyhow::bail!(
            "phase '{}' summary_trigger_messages must be > 0",
            phase_name
        );
    }
    if matches!(options.summary_max_chars, Some(0)) {
        anyhow::bail!("phase '{}' summary_max_chars must be > 0", phase_name);
    }
    if matches!(options.max_history_messages, Some(0)) {
        anyhow::bail!("phase '{}' max_history_messages must be > 0", phase_name);
    }
    if matches!(options.max_history_chars, Some(0)) {
        anyhow::bail!("phase '{}' max_history_chars must be > 0", phase_name);
    }
    if matches!(options.request_timeout_seconds, Some(0)) {
        anyhow::bail!("phase '{}' request_timeout_seconds must be > 0", phase_name);
    }
    if matches!(options.review_timeout_seconds, Some(0)) {
        anyhow::bail!("phase '{}' review_timeout_seconds must be > 0", phase_name);
    }

    validate_extra_u64_range(phase_name, options, "max_request_chars", 1, 2_000_000)?;
    validate_extra_u64_range(phase_name, options, "rate_limit_rpm", 1, 10_000)?;
    validate_extra_u64_range(phase_name, options, "rate_limit_burst", 1, 50_000)?;
    validate_extra_f64_range(
        phase_name,
        options,
        "rate_limit_burst_multiplier",
        0.1,
        20.0,
    )?;
    validate_extra_u64_range(phase_name, options, "min_reviewers", 1, 2)?;
    validate_extra_u64_range(phase_name, options, "required_approvals", 1, 2)?;
    validate_extra_u64_range(phase_name, options, "phase_max_inflight", 1, 10_000)?;
    validate_extra_u64_range(phase_name, options, "global_max_inflight", 1, 10_000)?;
    validate_extra_u64_range(phase_name, options, "circuit_breaker_failures", 1, 100)?;
    validate_extra_u64_range(phase_name, options, "circuit_breaker_open_seconds", 1, 3600)?;
    validate_extra_u64_range(phase_name, options, "review_gate_timeout_seconds", 1, 3600)?;
    validate_extra_u64_range(phase_name, options, "review_min_response_chars", 1, 4000)?;
    validate_extra_bool(phase_name, options, "auto_attach")?;
    validate_extra_bool(phase_name, options, "auto_detach")?;
    validate_extra_string_array(
        phase_name,
        options,
        "optimization_modules",
        &[
            "workflow_optimizer",
            "adaptive_selector",
            "advanced_modules",
            "cost_optimizer",
            "speed_optimizer",
            "reliability_optimizer",
            "failure_prevention",
        ],
    )?;

    if let Some(policy) = options
        .extra
        .get("review_timeout_policy")
        .and_then(|value| value.as_str())
    {
        if !policy.eq_ignore_ascii_case("reject") && !policy.eq_ignore_ascii_case("degrade_single")
        {
            anyhow::bail!(
                "phase '{}' option 'review_timeout_policy' must be one of: reject, degrade_single",
                phase_name
            );
        }
    }

    let min_reviewers = options
        .extra
        .get("min_reviewers")
        .and_then(|value| value.as_u64());
    let required_approvals = options
        .extra
        .get("required_approvals")
        .and_then(|value| value.as_u64());
    if let (Some(min_reviewers), Some(required_approvals)) = (min_reviewers, required_approvals) {
        if required_approvals > min_reviewers {
            anyhow::bail!(
                "phase '{}' required_approvals must be <= min_reviewers",
                phase_name
            );
        }
    }

    Ok(())
}

fn validate_extra_u64_range(
    phase_name: &str,
    options: &PhaseOptions,
    key: &str,
    min: u64,
    max: u64,
) -> Result<()> {
    let Some(value) = options.extra.get(key) else {
        return Ok(());
    };

    let Some(num) = value.as_u64() else {
        anyhow::bail!(
            "phase '{}' option '{}' must be a positive integer",
            phase_name,
            key
        );
    };

    if num < min || num > max {
        anyhow::bail!(
            "phase '{}' option '{}' must be in [{}, {}]",
            phase_name,
            key,
            min,
            max
        );
    }

    Ok(())
}

fn validate_extra_bool(phase_name: &str, options: &PhaseOptions, key: &str) -> Result<()> {
    let Some(value) = options.extra.get(key) else {
        return Ok(());
    };

    if !value.is_boolean() {
        anyhow::bail!("phase '{}' option '{}' must be a boolean", phase_name, key);
    }

    Ok(())
}

fn validate_extra_string_array(
    phase_name: &str,
    options: &PhaseOptions,
    key: &str,
    allowed: &[&str],
) -> Result<()> {
    let Some(value) = options.extra.get(key) else {
        return Ok(());
    };

    let Some(items) = value.as_array() else {
        anyhow::bail!(
            "phase '{}' option '{}' must be an array of strings",
            phase_name,
            key
        );
    };

    for item in items {
        let Some(module_name) = item.as_str() else {
            anyhow::bail!(
                "phase '{}' option '{}' must contain only strings",
                phase_name,
                key
            );
        };

        if !allowed.iter().any(|candidate| candidate == &module_name) {
            anyhow::bail!(
                "phase '{}' option '{}' contains unsupported module '{}'",
                phase_name,
                key,
                module_name
            );
        }
    }

    Ok(())
}

fn validate_extra_f64_range(
    phase_name: &str,
    options: &PhaseOptions,
    key: &str,
    min: f64,
    max: f64,
) -> Result<()> {
    let Some(value) = options.extra.get(key) else {
        return Ok(());
    };

    let Some(num) = value.as_f64() else {
        anyhow::bail!("phase '{}' option '{}' must be a number", phase_name, key);
    };

    if num < min || num > max {
        anyhow::bail!(
            "phase '{}' option '{}' must be in [{}, {}]",
            phase_name,
            key,
            min,
            max
        );
    }

    Ok(())
}

fn phase_uses_complex_autopilot(options: Option<&PhaseOptions>) -> bool {
    options
        .and_then(|opts| opts.autopilot_complexity.as_deref())
        .map(|value| value.eq_ignore_ascii_case("complex"))
        .unwrap_or(false)
}

pub fn missing_env_vars(config: &AppConfig) -> Vec<String> {
    let mut missing = Vec::new();

    for agent in config.agents.values() {
        for secret_ref in required_env_vars(agent) {
            if inspect_secret_pool(&secret_ref, &secret_ref).is_err() {
                missing.push(secret_ref);
            }
        }
    }

    missing.sort();
    missing.dedup();
    missing
}

pub fn is_agent_env_ready(config: &AppConfig, agent_name: &str) -> bool {
    let Some(agent) = config.agents.get(agent_name) else {
        return false;
    };
    required_env_vars(agent)
        .into_iter()
        .all(|secret_ref| inspect_secret_pool(&secret_ref, &secret_ref).is_ok())
}

fn missing_env_vars_by_agent(config: &AppConfig) -> HashMap<String, Vec<String>> {
    let mut missing = HashMap::new();

    for (agent_name, agent) in &config.agents {
        let mut per_agent_missing = required_env_vars(agent)
            .into_iter()
            .filter(|secret_ref| inspect_secret_pool(secret_ref, secret_ref).is_err())
            .collect::<Vec<_>>();

        if !per_agent_missing.is_empty() {
            per_agent_missing.sort();
            per_agent_missing.dedup();
            missing.insert(agent_name.clone(), per_agent_missing);
        }
    }

    missing
}

fn required_env_vars(agent: &AgentConfig) -> Vec<String> {
    let mut envs = Vec::new();
    if let Some(value) = agent.api_key_env.as_deref() {
        envs.push(value.to_string());
    }
    if let Some(value) = agent.secret_key_env.as_deref() {
        envs.push(value.to_string());
    }
    envs
}

fn is_keyring_ref(value: &str) -> bool {
    value.starts_with("keyring://")
}

pub fn collect_production_strict_violations(config: &AppConfig) -> Vec<String> {
    let mut violations = Vec::new();

    for (agent_name, agent) in &config.agents {
        if let Some(url) = agent.url.as_deref() {
            if url.starts_with("http://") {
                violations.push(format!(
                    "agents.{}.url uses insecure upstream HTTP ({})",
                    agent_name, url
                ));
            }
        }
    }

    let missing_by_agent = missing_env_vars_by_agent(config);
    for (agent_name, missing_vars) in missing_by_agent {
        violations.push(format!(
            "agents.{} is missing required secrets: {}",
            agent_name,
            missing_vars.join(",")
        ));
    }

    if let Some(runtime) = config.runtime.as_ref() {
        if runtime.acp_http_bind_addr.is_some() && !runtime.entry_auth_enabled {
            violations.push(
                "runtime.acp_http_bind_addr is set but runtime.entry_auth_enabled=false"
                    .to_string(),
            );
        }
    }

    violations.sort();
    violations.dedup();
    violations
}

pub fn validate_external_secret_refs(config: &AppConfig) -> Result<()> {
    for (agent_name, agent) in &config.agents {
        if let Some(value) = agent.api_key_env.as_deref() {
            validate_secret_ref(value, &format!("agents.{}.api_key_env", agent_name))?;
        }
        if let Some(value) = agent.secret_key_env.as_deref() {
            validate_secret_ref(value, &format!("agents.{}.secret_key_env", agent_name))?;
        }
    }
    Ok(())
}

pub fn validate_runtime_readiness(
    config_path: &Path,
    config: &AppConfig,
) -> Result<ConfigHealthReport> {
    config.validate()?;

    let strict_enabled = config
        .runtime
        .as_ref()
        .map(|runtime| runtime.production_strict)
        .unwrap_or(false);

    let missing_by_agent = missing_env_vars_by_agent(config);
    if !missing_by_agent.is_empty() {
        if strict_enabled {
            let blocked = missing_by_agent
                .iter()
                .map(|(agent, vars)| format!("{}({})", agent, vars.join(",")))
                .collect::<Vec<_>>()
                .join("; ");
            anyhow::bail!(
                "production_strict is enabled and runtime readiness is blocked by missing agent secrets: {}",
                blocked
            );
        }

        let total_agents = config.agents.len();
        let ready_agents = total_agents.saturating_sub(missing_by_agent.len());
        let blocked = missing_by_agent
            .iter()
            .map(|(agent, vars)| format!("{}({})", agent, vars.join(",")))
            .collect::<Vec<_>>()
            .join("; ");
        if ready_agents == 0 {
            warn!(
                "runtime readiness degraded: 0 of {} agents are env-ready; startup continues in non-strict mode; unavailable agents: {}",
                total_agents,
                blocked
            );
        } else {
            warn!(
                "runtime readiness degraded: {} of {} agents are env-ready; unavailable agents: {}",
                ready_agents, total_agents, blocked
            );
        }
    }

    if strict_enabled {
        validate_external_secret_refs(config)?;
    } else if let Err(err) = validate_external_secret_refs(config) {
        warn!(
            "runtime readiness degraded: external secret validation failed in non-strict mode; startup continues: {}",
            err
        );
    }

    if strict_enabled {
        let strict_violations = collect_production_strict_violations(config);
        if !strict_violations.is_empty() {
            anyhow::bail!(
                "production_strict is enabled and unsafe configuration was detected: {}",
                strict_violations.join("; ")
            );
        }
    }

    Ok(build_config_health_report(config_path, config))
}

#[allow(dead_code)]
pub fn collect_config_warnings(config_path: &Path, config: &AppConfig) -> Vec<String> {
    collect_config_warnings_detailed(config_path, config)
        .into_iter()
        .map(|item| item.message)
        .collect()
}

pub fn build_config_health_report(config_path: &Path, config: &AppConfig) -> ConfigHealthReport {
    let mut warnings = collect_config_warnings_detailed(config_path, config);
    warnings.sort_by(|left, right| {
        severity_rank(left.severity)
            .cmp(&severity_rank(right.severity))
            .then_with(|| left.code.cmp(&right.code))
            .then_with(|| left.message.cmp(&right.message))
    });

    let info_count = warnings
        .iter()
        .filter(|item| item.severity == ConfigWarningSeverity::Info)
        .count();
    let warn_count = warnings
        .iter()
        .filter(|item| item.severity == ConfigWarningSeverity::Warn)
        .count();
    let critical_count = warnings
        .iter()
        .filter(|item| item.severity == ConfigWarningSeverity::Critical)
        .count();
    let (profile_recommendation, recommendations) =
        profile_recommendations_for(&warnings, warn_count, critical_count);
    let penalty = (info_count * 5) + (warn_count * 15) + (critical_count * 40);
    let score = 100_u32.saturating_sub(penalty.min(100) as u32);

    ConfigHealthReport {
        score,
        total: warnings.len(),
        info_count,
        warn_count,
        critical_count,
        profile_recommendation,
        recommendations,
        warnings,
    }
}

fn collect_config_warnings_detailed(config_path: &Path, config: &AppConfig) -> Vec<ConfigWarning> {
    let mut warnings = Vec::new();

    if let Some(vector) = &config.vector {
        if vector.enabled && !vector.summary_enabled {
            warnings.push(ConfigWarning {
                code: "VECTOR_SUMMARY_DISABLED".to_string(),
                severity: ConfigWarningSeverity::Info,
                message: "vector memory is enabled while summary_enabled=false; retrieval quality and long-session compression may degrade".to_string(),
            });
        }

        if vector.enabled && vector.max_entries >= 50_000 {
            warnings.push(ConfigWarning {
                code: "VECTOR_MAX_ENTRIES_HIGH".to_string(),
                severity: ConfigWarningSeverity::Warn,
                message: format!(
                    "vector.max_entries={} is unusually high; startup I/O, SQLite growth, and maintenance time may increase",
                    vector.max_entries
                ),
            });
        }
    }

    if let Some(cache) = &config.cache {
        if cache.enabled && cache.max_entries >= 50_000 {
            warnings.push(ConfigWarning {
                code: "CACHE_MAX_ENTRIES_HIGH".to_string(),
                severity: ConfigWarningSeverity::Warn,
                message: format!(
                    "cache.max_entries={} is unusually high; consider lowering it if startup or VACUUM pauses increase",
                    cache.max_entries
                ),
            });
        }

        if cache.enabled && cache.default_ttl_seconds <= 60 && cache.max_entries >= 10_000 {
            warnings.push(ConfigWarning {
                code: "CACHE_CHURN_RISK".to_string(),
                severity: ConfigWarningSeverity::Warn,
                message: format!(
                    "cache.default_ttl_seconds={} with cache.max_entries={} may cause high churn and frequent refreshes",
                    cache.default_ttl_seconds, cache.max_entries
                ),
            });
        }
    }

    if let Some(autotune) = &config.autotune {
        let vector_enabled = config.vector.as_ref().map(|v| v.enabled).unwrap_or(false);
        if autotune.enabled && !vector_enabled {
            warnings.push(ConfigWarning {
                code: "AUTOTUNE_WITHOUT_VECTOR".to_string(),
                severity: ConfigWarningSeverity::Warn,
                message: "autotune is enabled but vector memory is disabled; autotune will have little practical effect".to_string(),
            });
        }
    }

    if let Some(runtime) = &config.runtime {
        if runtime.production_strict {
            let strict_violations = collect_production_strict_violations(config);
            if strict_violations.is_empty() {
                warnings.push(ConfigWarning {
                    code: "PRODUCTION_STRICT_ENABLED".to_string(),
                    severity: ConfigWarningSeverity::Info,
                    message: "runtime.production_strict=true; unsafe runtime configuration will fail fast at startup"
                        .to_string(),
                });
            }
        }

        if runtime.otel_enabled && runtime.otel_endpoint.is_none() {
            warnings.push(ConfigWarning {
                code: "OTEL_ENDPOINT_DEFAULTED".to_string(),
                severity: ConfigWarningSeverity::Info,
                message: "runtime.otel_enabled=true without otel_endpoint; default collector endpoint http://127.0.0.1:4317 will be used".to_string(),
            });
        }

        if runtime.otel_enabled
            && runtime.otel_sample_ratio >= 0.95
            && runtime.maintenance_interval_seconds <= 30
        {
            warnings.push(ConfigWarning {
                code: "RUNTIME_OBSERVABILITY_OVERHEAD_RISK".to_string(),
                severity: ConfigWarningSeverity::Warn,
                message: format!(
                    "runtime.otel_sample_ratio={} with maintenance_interval_seconds={} may add noticeable runtime overhead",
                    runtime.otel_sample_ratio, runtime.maintenance_interval_seconds
                ),
            });
        }

        if !runtime.production_strict {
            let strict_violations = collect_production_strict_violations(config);
            if !strict_violations.is_empty() {
                warnings.push(ConfigWarning {
                    code: "PRODUCTION_STRICT_RECOMMENDED".to_string(),
                    severity: ConfigWarningSeverity::Warn,
                    message: format!(
                        "runtime.production_strict=false while {} strict violation(s) are present; consider enabling strict mode to enforce fail-fast guardrails",
                        strict_violations.len()
                    ),
                });
            }
        }
    }

    let cache_explicitly_disabled = config
        .cache
        .as_ref()
        .map(|item| !item.enabled)
        .unwrap_or(false);
    let vector_explicitly_disabled = config
        .vector
        .as_ref()
        .map(|item| !item.enabled)
        .unwrap_or(false);
    if cache_explicitly_disabled && vector_explicitly_disabled {
        warnings.push(ConfigWarning {
            code: "MEMORY_LAYERS_DISABLED".to_string(),
            severity: ConfigWarningSeverity::Warn,
            message: "cache and vector memory are both disabled; repeated prompts may be slower and less context-aware"
                .to_string(),
        });
    }

    for path in shared_rule_paths(config_path.parent().unwrap_or_else(|| Path::new("."))) {
        push_rule_warning(&mut warnings, &path, "RULE_FILE_EMPTY");
    }
    for phase_name in config.phases.keys() {
        for path in phase_rule_paths(
            config_path.parent().unwrap_or_else(|| Path::new(".")),
            phase_name,
        ) {
            push_rule_warning(&mut warnings, &path, "RULE_FILE_EMPTY");
        }
    }

    for (phase_name, phase_cfg) in &config.phases {
        let uses_complex = phase_uses_complex_autopilot(phase_cfg.options.as_ref());
        if !uses_complex {
            continue;
        }

        let review_options = config
            .phases
            .get("review")
            .and_then(|phase| phase.options.as_ref());
        let gate_timeout = phase_cfg
            .options
            .as_ref()
            .and_then(|opts| opts.extra.get("review_gate_timeout_seconds"))
            .and_then(|value| value.as_u64())
            .or_else(|| {
                review_options.and_then(|opts| {
                    opts.extra
                        .get("review_gate_timeout_seconds")
                        .and_then(|value| value.as_u64())
                })
            });
        let reviewer_timeout = review_options
            .and_then(|opts| opts.review_timeout_seconds.or(opts.request_timeout_seconds))
            .or_else(|| {
                phase_cfg
                    .options
                    .as_ref()
                    .and_then(|opts| opts.review_timeout_seconds.or(opts.request_timeout_seconds))
            });

        if gate_timeout.is_none() && reviewer_timeout.is_none() {
            warnings.push(ConfigWarning {
                code: "REVIEW_GATE_TIMEOUT_MISSING".to_string(),
                severity: ConfigWarningSeverity::Critical,
                message: format!(
                    "phase '{}' uses complex autopilot without review_gate_timeout_seconds or review_timeout_seconds/request_timeout_seconds; review gate may hang too long",
                    phase_name
                ),
            });
        }

        let review_phase_limit = review_options
            .and_then(|opts| opts.extra.get("phase_max_inflight"))
            .and_then(|value| value.as_u64());
        let review_global_limit = review_options
            .and_then(|opts| opts.extra.get("global_max_inflight"))
            .and_then(|value| value.as_u64());
        if review_phase_limit.is_none() || review_global_limit.is_none() {
            warnings.push(ConfigWarning {
                code: "REVIEW_INFLIGHT_LIMIT_MISSING".to_string(),
                severity: ConfigWarningSeverity::Warn,
                message:
                    "review phase is missing phase_max_inflight or global_max_inflight; high concurrency can degrade review stability"
                        .to_string(),
            });
        }
    }

    warnings.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.message.cmp(&right.message))
    });
    warnings.dedup_by(|left, right| left.code == right.code && left.message == right.message);
    warnings
}

fn severity_rank(value: ConfigWarningSeverity) -> usize {
    match value {
        ConfigWarningSeverity::Critical => 0,
        ConfigWarningSeverity::Warn => 1,
        ConfigWarningSeverity::Info => 2,
    }
}

fn profile_recommendations_for(
    warnings: &[ConfigWarning],
    warn_count: usize,
    critical_count: usize,
) -> (String, Vec<String>) {
    let profile = if critical_count > 0 || warn_count >= 3 {
        "full"
    } else if warn_count == 0 {
        "minimal"
    } else {
        "balanced"
    }
    .to_string();

    let mut recommendations = Vec::new();
    recommendations.push(match profile.as_str() {
        "full" => {
            "use config.toml.autopilot-adaptive and keep review and safeguard defaults enabled"
                .to_string()
        }
        "minimal" => {
            "config quality is stable for quick-start profile; keep minimal defaults unless workload changes"
                .to_string()
        }
        _ => {
            "use a balanced profile: keep key safeguards while avoiding high-cost optional toggles"
                .to_string()
        }
    });

    let mut has_memory_layers_disabled = false;
    let mut has_review_timeout_missing = false;
    let mut has_cache_churn_risk = false;
    let mut has_overhead_risk = false;

    for warning in warnings {
        match warning.code.as_str() {
            "MEMORY_LAYERS_DISABLED" => has_memory_layers_disabled = true,
            "REVIEW_GATE_TIMEOUT_MISSING" => has_review_timeout_missing = true,
            "CACHE_CHURN_RISK" => has_cache_churn_risk = true,
            "RUNTIME_OBSERVABILITY_OVERHEAD_RISK" => has_overhead_risk = true,
            _ => {}
        }
    }

    if has_memory_layers_disabled {
        recommendations.push(
            "enable either cache or vector memory for better recall and lower repeated provider cost"
                .to_string(),
        );
    }
    if has_review_timeout_missing {
        recommendations.push(
            "set review_gate_timeout_seconds and review/request timeout to prevent stuck review gates"
                .to_string(),
        );
    }
    if has_cache_churn_risk {
        recommendations.push(
            "increase cache.default_ttl_seconds or reduce cache.max_entries to reduce cache churn"
                .to_string(),
        );
    }
    if has_overhead_risk {
        recommendations.push(
            "reduce otel_sample_ratio or increase maintenance_interval_seconds to lower runtime overhead"
                .to_string(),
        );
    }

    (profile, recommendations)
}

fn push_rule_warning(warnings: &mut Vec<ConfigWarning>, path: &Path, code: &str) {
    if path.exists() && load_optional_rule_items(path).is_empty() {
        warnings.push(ConfigWarning {
            code: code.to_string(),
            severity: ConfigWarningSeverity::Info,
            message: format!(
                "rule file '{}' exists but contributed no usable rule lines",
                path.display()
            ),
        });
    }
}

fn validate_secret_ref(value: &str, field_name: &str) -> Result<()> {
    if !is_keyring_ref(value) {
        return Ok(());
    }

    let locator = value
        .strip_prefix("keyring://")
        .ok_or_else(|| anyhow::anyhow!("invalid keyring ref for {}", field_name))?;
    let (service, account) = locator.split_once('/').ok_or_else(|| {
        anyhow::anyhow!(
            "invalid {} keyring reference '{}': expected keyring://<service>/<account>",
            field_name,
            value
        )
    })?;
    let entry = keyring::Entry::new(service, account).map_err(|err| {
        anyhow::anyhow!("failed to open keyring entry for {}: {}", field_name, err)
    })?;
    let secret = entry.get_password().map_err(|err| {
        anyhow::anyhow!(
            "keyring entry for {}/{} not found or cannot be read: {}. \
             Hint: Run 'go-on --setup --config config.toml' to store credentials in keyring.",
            service,
            account,
            err
        )
    })?;
    if secret.trim().is_empty() {
        anyhow::bail!(
            "keyring entry for {}/{} resolved to empty value. \
             Hint: Run 'go-on --setup --config config.toml' to update the credential.",
            service,
            account
        );
    }

    // 验证密钥安全性
    validate_secret_security(&secret, field_name)?;

    Ok(())
}

/// 验证密钥的安全性
///
/// # 参数
/// * `secret` - 要验证的密钥
/// * `field_name` - 字段名称，用于错误消息
///
/// # 返回
/// * `Result<()>` - 如果密钥安全则返回Ok，否则返回错误
fn validate_secret_security(secret: &str, field_name: &str) -> Result<()> {
    use tracing::warn;

    if secret.trim().is_empty() {
        anyhow::bail!("{} is empty", field_name);
    }

    // 检查是否有换行符（可能是多行密钥或注入尝试）
    if secret.contains('\n') || secret.contains('\r') {
        warn!(
            "{} contains newline characters, which may be a security issue",
            field_name
        );
    }

    // 检查密钥长度
    if secret.len() < 8 {
        warn!(
            "{} is very short ({} characters), which may be insecure",
            field_name,
            secret.len()
        );
    }

    // 检查是否包含常见的不安全模式
    let insecure_patterns = [
        ("password", "contains the word 'password'"),
        ("123456", "contains simple numeric sequence"),
        ("admin", "contains the word 'admin'"),
        ("test", "contains the word 'test'"),
        ("secret", "contains the word 'secret'"),
    ];

    let secret_lower = secret.to_lowercase();
    for (pattern, description) in insecure_patterns {
        if secret_lower.contains(pattern) {
            warn!(
                "{} {} - consider using a stronger secret",
                field_name, description
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{
        AgentConfig, AppConfig, CacheConfig, FlowConfig, PhaseConfig, PhaseOptions, RuntimeConfig,
        VectorConfig,
    };

    fn base_agent() -> AgentConfig {
        AgentConfig {
            agent_type: "copilot".to_string(),
            url: Some("http://127.0.0.1:8080".to_string()),
            chat_path: None,
            api_key_env: None,
            secret_key_env: None,
            anthropic_version: None,
            model: None,
            max_tokens: None,
            supports_system: None,
        }
    }

    fn valid_config() -> AppConfig {
        let mut agents = HashMap::new();
        agents.insert("copilot".to_string(), base_agent());
        agents.insert(
            "reviewer_a".to_string(),
            AgentConfig {
                agent_type: "claude".to_string(),
                url: Some("https://api.anthropic.com".to_string()),
                chat_path: None,
                api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
                secret_key_env: None,
                anthropic_version: Some("2023-06-01".to_string()),
                model: Some("claude-3-7-sonnet-latest".to_string()),
                max_tokens: Some(4096),
                supports_system: None,
            },
        );
        agents.insert(
            "reviewer_b".to_string(),
            AgentConfig {
                agent_type: "wenxin".to_string(),
                url: None,
                chat_path: None,
                api_key_env: Some("WENXIN_API_KEY".to_string()),
                secret_key_env: Some("WENXIN_SECRET_KEY".to_string()),
                anthropic_version: None,
                model: None,
                max_tokens: None,
                supports_system: None,
            },
        );

        let mut phases = HashMap::new();
        phases.insert(
            "coding".to_string(),
            PhaseConfig {
                description: "coding".to_string(),
                agents: vec!["copilot".to_string()],
                fallback: Some(true),
                principles: None,
                options: None,
            },
        );
        phases.insert(
            "review".to_string(),
            PhaseConfig {
                description: "review".to_string(),
                agents: vec!["reviewer_a".to_string(), "reviewer_b".to_string()],
                fallback: Some(true),
                principles: None,
                options: None,
            },
        );

        AppConfig {
            default_phase: "coding".to_string(),
            agents,
            flow: FlowConfig {
                name: "flow".to_string(),
                phases: vec!["coding".to_string(), "review".to_string()],
            },
            phases,
            runtime: Some(RuntimeConfig::default()),
            cache: None,
            vector: None,
            autotune: None,
            model_selection_mode: "adaptive".to_string(),
        }
    }

    #[test]
    fn validate_accepts_valid_configuration() {
        let cfg = valid_config();
        cfg.validate().expect("valid config should pass");
    }

    #[test]
    fn validate_rejects_default_phase_not_in_flow() {
        let mut cfg = valid_config();
        cfg.default_phase = "missing".to_string();
        let err = cfg
            .validate()
            .expect_err("default phase outside flow must fail");
        assert!(
            err.to_string()
                .contains("default_phase 'missing' is not listed in flow.phases"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_phase_with_unknown_agent() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .agents = vec!["missing".to_string()];

        let err = cfg
            .validate()
            .expect_err("phase referencing undefined agent must fail");
        assert!(
            err.to_string()
                .contains("phase 'coding' references undefined agent 'missing'"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_accepts_phase_with_no_agents() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .agents = vec![];

        cfg.validate()
            .expect("phase without agents should be allowed for AI-optional templates");
    }

    #[test]
    fn validate_rejects_autotune_threshold_order() {
        let mut cfg = valid_config();
        cfg.autotune = Some(super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.8,
            high_precision_threshold: 0.5,
            state_path: "state.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        });

        let err = cfg
            .validate()
            .expect_err("invalid autotune threshold order must fail");
        assert!(
            err.to_string().contains(
                "autotune.low_precision_threshold must be < autotune.high_precision_threshold"
            ),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_zero_runtime_maintenance_interval() {
        let mut cfg = valid_config();
        cfg.runtime = Some(RuntimeConfig {
            maintenance_interval_seconds: 0,
            health_interval_seconds: 30,
            shutdown_drain_seconds: 10,
            protocol_mode: None,
            pua_report: false,
            deployment_target: None,
            acp_http_bind_addr: None,
            entry_auth_enabled: false,
            entry_auth_api_key_env: "GO_ON_ENTRY_API_KEY".to_string(),
            entry_rate_limit_rpm: 240,
            entry_rate_limit_burst: 60,
            production_strict: false,
            sqlite_vacuum_interval_cycles: 60,
            otel_enabled: false,
            otel_exporter: "otlp".to_string(),
            otel_endpoint: None,
            otel_service_name: "go-on".to_string(),
            otel_sample_ratio: 1.0,
            trace_slow_top_n: 20,
            skills_enabled: true,
            skills_import_enabled: false,
            skills_allowed_sources: Vec::new(),
            skills_require_sha256: true,
            skills_allow_floating_ref: false,
            skills_cache_dir: "skills_cache".to_string(),
        });

        let err = cfg
            .validate()
            .expect_err("zero maintenance interval must fail");
        assert!(
            err.to_string()
                .contains("runtime.maintenance_interval_seconds must be > 0"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_autotune_summary_range() {
        let mut cfg = valid_config();
        cfg.autotune = Some(super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "state.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 9,
            summary_trigger_max: 6,
        });

        let err = cfg
            .validate()
            .expect_err("invalid autotune summary range must fail");
        assert!(
            err.to_string()
                .contains("autotune.summary_trigger_min must be <= autotune.summary_trigger_max"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_complex_autopilot_without_two_reviewers() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            autopilot_complexity: Some("complex".to_string()),
            full_auto_review_agents: Some(vec!["reviewer_a".to_string()]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("complex autopilot with one reviewer must fail");
        assert!(
            err.to_string()
                .contains("must define at least 2 full_auto_review_agents"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_complex_autopilot_when_reviewer_not_in_review_phase() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("review")
            .expect("review phase must exist")
            .agents = vec!["reviewer_a".to_string(), "copilot".to_string()];
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            autopilot_complexity: Some("complex".to_string()),
            full_auto_review_agents: Some(vec!["reviewer_a".to_string(), "reviewer_b".to_string()]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("missing reviewer in review phase must fail");
        assert!(
            err.to_string()
                .contains("review agent 'reviewer_b' must also appear in [phases.review].agents"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_zero_phase_timeout() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            request_timeout_seconds: Some(0),
            ..PhaseOptions::default()
        });

        let err = cfg.validate().expect_err("zero request timeout must fail");
        assert!(
            err.to_string()
                .contains("phase 'coding' request_timeout_seconds must be > 0"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_required_approvals_exceeding_min_reviewers() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([
                ("min_reviewers".to_string(), serde_json::Value::from(1_u64)),
                (
                    "required_approvals".to_string(),
                    serde_json::Value::from(2_u64),
                ),
            ]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("required approvals above min reviewers must fail");
        assert!(
            err.to_string()
                .contains("phase 'coding' required_approvals must be <= min_reviewers"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_min_reviewers_above_two() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([("min_reviewers".to_string(), serde_json::Value::from(3_u64))]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("min_reviewers above two must fail");
        assert!(
            err.to_string()
                .contains("phase 'coding' option 'min_reviewers' must be in [1, 2]"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_complex_autopilot_with_more_than_two_reviewers() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("review")
            .expect("review phase must exist")
            .agents = vec![
            "reviewer_a".to_string(),
            "reviewer_b".to_string(),
            "copilot".to_string(),
        ];
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            autopilot_complexity: Some("complex".to_string()),
            full_auto_review_agents: Some(vec![
                "reviewer_a".to_string(),
                "reviewer_b".to_string(),
                "copilot".to_string(),
            ]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("complex autopilot with >2 reviewers must fail");
        assert!(
            err.to_string().contains("supports at most 2 reviewers"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_invalid_rate_limit_type() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([(
                "rate_limit_rpm".to_string(),
                serde_json::Value::from("fast"),
            )]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("non-numeric rate_limit_rpm must fail");
        assert!(
            err.to_string()
                .contains("phase 'coding' option 'rate_limit_rpm' must be a positive integer"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_invalid_burst_multiplier_range() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([(
                "rate_limit_burst_multiplier".to_string(),
                serde_json::Value::from(100.0_f64),
            )]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("burst multiplier out of range must fail");
        assert!(
            err.to_string().contains(
                "phase 'coding' option 'rate_limit_burst_multiplier' must be in [0.1, 20]"
            ),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_zero_breaker_open_seconds() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([(
                "circuit_breaker_open_seconds".to_string(),
                serde_json::Value::from(0_u64),
            )]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("zero breaker open seconds must fail");
        assert!(
            err.to_string().contains(
                "phase 'coding' option 'circuit_breaker_open_seconds' must be in [1, 3600]"
            ),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_invalid_review_timeout_policy() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([(
                "review_timeout_policy".to_string(),
                serde_json::Value::from("maybe"),
            )]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("invalid review timeout policy must fail");
        assert!(
            err.to_string().contains(
                "phase 'coding' option 'review_timeout_policy' must be one of: reject, degrade_single"
            ),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_non_boolean_auto_attach() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([("auto_attach".to_string(), serde_json::Value::from("yes"))]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("non-boolean auto_attach must fail");
        assert!(
            err.to_string()
                .contains("phase 'coding' option 'auto_attach' must be a boolean"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_unsupported_optimization_module() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([(
                "optimization_modules".to_string(),
                serde_json::Value::from(vec!["unknown_module"]),
            )]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("unsupported optimization module must fail");
        assert!(
            err.to_string().contains(
                "phase 'coding' option 'optimization_modules' contains unsupported module 'unknown_module'"
            ),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn missing_env_vars_detects_agent_requirements() {
        let cfg = valid_config();
        let missing = super::missing_env_vars(&cfg);

        assert!(missing.iter().any(|value| value == "ANTHROPIC_API_KEY"));
        assert!(missing.iter().any(|value| value == "WENXIN_API_KEY"));
        assert!(missing.iter().any(|value| value == "WENXIN_SECRET_KEY"));
    }

    #[test]
    fn runtime_readiness_allows_when_at_least_one_agent_ready() {
        let cfg = valid_config();
        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "# test").expect("config marker should be written");

        super::validate_runtime_readiness(&config_path, &cfg)
            .expect("runtime readiness should pass when at least one agent is env-ready");
    }

    #[test]
    fn runtime_readiness_allows_degraded_when_all_agents_are_env_blocked() {
        let mut cfg = valid_config();
        cfg.agents.remove("copilot");
        cfg.phases
            .get_mut("coding")
            .expect("coding phase should exist")
            .agents = vec!["reviewer_a".to_string()];
        if let Some(agent) = cfg.agents.get_mut("reviewer_a") {
            agent.api_key_env = Some("UNITTEST_MISSING_REVIEWER_A_KEY".to_string());
        }
        if let Some(agent) = cfg.agents.get_mut("reviewer_b") {
            agent.api_key_env = Some("UNITTEST_MISSING_REVIEWER_B_KEY".to_string());
            agent.secret_key_env = Some("UNITTEST_MISSING_REVIEWER_B_SECRET".to_string());
        }

        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "# test").expect("config marker should be written");

        super::validate_runtime_readiness(&config_path, &cfg)
            .expect("runtime readiness should allow degraded startup in non-strict mode");
    }

    #[test]
    fn runtime_readiness_strict_mode_fails_when_agent_secrets_missing() {
        let mut cfg = valid_config();
        cfg.runtime = Some(RuntimeConfig {
            production_strict: true,
            ..RuntimeConfig::default()
        });

        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "# test").expect("config marker should be written");

        let err = super::validate_runtime_readiness(&config_path, &cfg)
            .expect_err("strict mode should fail when any configured agent is missing secrets");
        assert!(
            err.to_string().contains("production_strict is enabled"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn runtime_readiness_strict_mode_fails_when_entry_auth_disabled_for_http_bind() {
        let mut cfg = valid_config();
        if let Some(agent) = cfg.agents.get_mut("copilot") {
            agent.url = None;
        }
        if let Some(agent) = cfg.agents.get_mut("reviewer_a") {
            agent.api_key_env = None;
        }
        if let Some(agent) = cfg.agents.get_mut("reviewer_b") {
            agent.api_key_env = None;
            agent.secret_key_env = None;
        }
        cfg.runtime = Some(RuntimeConfig {
            production_strict: true,
            acp_http_bind_addr: Some("127.0.0.1:8090".to_string()),
            entry_auth_enabled: false,
            ..RuntimeConfig::default()
        });

        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "# test").expect("config marker should be written");

        let err = super::validate_runtime_readiness(&config_path, &cfg).expect_err(
            "strict mode should fail when entry auth is disabled for exposed HTTP endpoint",
        );
        assert!(
            err.to_string().contains("entry_auth_enabled=false"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn runtime_readiness_strict_mode_passes_with_safe_configuration() {
        let mut cfg = valid_config();
        if let Some(agent) = cfg.agents.get_mut("copilot") {
            agent.url = None;
        }
        if let Some(agent) = cfg.agents.get_mut("reviewer_a") {
            agent.api_key_env = None;
        }
        if let Some(agent) = cfg.agents.get_mut("reviewer_b") {
            agent.api_key_env = None;
            agent.secret_key_env = None;
        }
        cfg.runtime = Some(RuntimeConfig {
            production_strict: true,
            entry_auth_enabled: true,
            ..RuntimeConfig::default()
        });

        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "# test").expect("config marker should be written");

        super::validate_runtime_readiness(&config_path, &cfg)
            .expect("strict mode should pass when all strict checks are satisfied");
    }

    #[test]
    fn adaptive_template_loads_and_validates() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.toml.autopilot-adaptive");
        let cfg = AppConfig::load(&path).expect("config.toml.autopilot-adaptive should parse");

        cfg.validate()
            .expect("config.toml.autopilot-adaptive should be internally consistent");
    }

    #[test]
    fn build_config_health_report_recommends_minimal_on_clean_config() {
        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "# test").expect("config marker should be written");

        let cfg = valid_config();
        let report = super::build_config_health_report(&config_path, &cfg);

        assert_eq!(report.total, 1);
        assert_eq!(report.info_count, 0);
        assert_eq!(report.warn_count, 1);
        assert_eq!(report.profile_recommendation, "balanced");
        assert!(!report.recommendations.is_empty());
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "PRODUCTION_STRICT_RECOMMENDED"));
    }

    #[test]
    fn build_config_health_report_flags_suspicious_combo_and_recommendations() {
        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "# test").expect("config marker should be written");

        let mut cfg = valid_config();
        cfg.cache = Some(CacheConfig {
            enabled: false,
            path: "cache.sqlite3".to_string(),
            default_ttl_seconds: 30,
            max_entries: 20_000,
        });
        cfg.vector = Some(VectorConfig {
            enabled: false,
            auto_mode: true,
            path: "vector.sqlite3".to_string(),
            dimensions: 192,
            min_query_chars: 80,
            top_k: 2,
            min_similarity: 0.82,
            max_snippet_chars: 800,
            max_entries: 10_000,
            summary_enabled: true,
            summary_trigger_messages: 8,
            summary_max_chars: 1200,
        });
        cfg.runtime = Some(RuntimeConfig {
            maintenance_interval_seconds: 20,
            health_interval_seconds: 120,
            shutdown_drain_seconds: 30,
            protocol_mode: None,
            pua_report: false,
            deployment_target: None,
            acp_http_bind_addr: None,
            entry_auth_enabled: false,
            entry_auth_api_key_env: "GO_ON_ENTRY_API_KEY".to_string(),
            entry_rate_limit_rpm: 240,
            entry_rate_limit_burst: 60,
            production_strict: false,
            sqlite_vacuum_interval_cycles: 60,
            otel_enabled: true,
            otel_exporter: "otlp".to_string(),
            otel_endpoint: None,
            otel_service_name: "go-on".to_string(),
            otel_sample_ratio: 1.0,
            trace_slow_top_n: 20,
            skills_enabled: true,
            skills_import_enabled: false,
            skills_allowed_sources: Vec::new(),
            skills_require_sha256: true,
            skills_allow_floating_ref: false,
            skills_cache_dir: "skills_cache".to_string(),
        });

        let report = super::build_config_health_report(&config_path, &cfg);
        let codes = report
            .warnings
            .iter()
            .map(|w| w.code.clone())
            .collect::<Vec<_>>();

        assert!(codes.iter().any(|code| code == "MEMORY_LAYERS_DISABLED"));
        assert!(codes
            .iter()
            .any(|code| code == "RUNTIME_OBSERVABILITY_OVERHEAD_RISK"));
        assert!(codes
            .iter()
            .any(|code| code == "PRODUCTION_STRICT_RECOMMENDED"));
        assert_eq!(report.warn_count, 3);
        assert_eq!(report.profile_recommendation, "full");
        assert!(report
            .recommendations
            .iter()
            .any(|text| text.contains("enable either cache or vector memory")));
    }

    #[test]
    fn load_auto_rules_from_rules_directory_and_phase_files() {
        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        let rules_dir = dir.path().join("RULES");
        fs::create_dir_all(&rules_dir).expect("rules directory should be created");

        fs::write(
            &config_path,
            r#"default_phase = "coding"

[flow]
name = "test"
phases = ["coding", "review"]

[agents.copilot]
type = "copilot"
url = "http://127.0.0.1:8080"

[phases.coding]
description = "coding"
agents = ["copilot"]
fallback = true
principles = ["inline principle"]

[phases.review]
description = "review"
agents = ["copilot"]
fallback = true
"#,
        )
        .expect("config should be written");

        fs::write(
            dir.path().join("RULES.md"),
            "# Shared\n- shared one\n- shared two\n",
        )
        .expect("shared rules should be written");
        fs::write(
            rules_dir.join("coding.md"),
            "## Coding\n1. coding phase rule\n* extra coding rule\n",
        )
        .expect("phase rules should be written");

        let cfg = AppConfig::load(&config_path).expect("config should load");
        let coding = cfg
            .phases
            .get("coding")
            .and_then(|phase| phase.principles.as_ref())
            .expect("coding principles should exist");
        assert!(coding.iter().any(|v| v == "inline principle"));
        assert!(coding.iter().any(|v| v == "shared one"));
        assert!(coding.iter().any(|v| v == "shared two"));
        assert!(coding.iter().any(|v| v == "coding phase rule"));
        assert!(coding.iter().any(|v| v == "extra coding rule"));

        let review = cfg
            .phases
            .get("review")
            .and_then(|phase| phase.principles.as_ref())
            .expect("review principles should exist");
        assert!(review.iter().any(|v| v == "shared one"));
        assert!(review.iter().any(|v| v == "shared two"));
    }

    #[test]
    fn normalize_provider_name_maps_claude_to_anthropic() {
        assert_eq!(
            super::normalize_provider_name("claude").as_deref(),
            Some("anthropic")
        );
        assert_eq!(
            super::normalize_provider_name("anthropic").as_deref(),
            Some("anthropic")
        );
    }

    #[test]
    fn default_agent_config_reads_provider_specs() {
        let openai = super::default_agent_config("openai")
            .expect("openai should be available in provider specs");
        assert_eq!(openai.agent_type, "openai");
        assert_eq!(openai.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
        assert_eq!(openai.url.as_deref(), Some("https://api.openai.com/v1"));
    }

    #[test]
    fn load_auto_rules_from_sidecar_phase_file() {
        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");

        fs::write(
            &config_path,
            r#"default_phase = "coding"

[flow]
name = "test"
phases = ["coding"]

[agents.copilot]
type = "copilot"
url = "http://127.0.0.1:8080"

[phases.coding]
description = "coding"
agents = ["copilot"]
fallback = true
"#,
        )
        .expect("config should be written");

        fs::write(
            dir.path().join("coding.rules.md"),
            "- keep functions short\n- add tests\n",
        )
        .expect("sidecar rules should be written");

        let cfg = AppConfig::load(&config_path).expect("config should load");
        let coding = cfg
            .phases
            .get("coding")
            .and_then(|phase| phase.principles.as_ref())
            .expect("coding principles should exist");

        assert!(coding.iter().any(|v| v == "keep functions short"));
        assert!(coding.iter().any(|v| v == "add tests"));
    }

    #[test]
    fn autotune_state_initializes_with_config_defaults() {
        let config = super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let state = super::AutoTuneState::new(&config);
        assert_eq!(state.current_min_query_chars, 40);
        assert_eq!(state.current_top_k, 2);
        assert_eq!(state.window_phase, 0);
        assert_eq!(state.vector_search_count, 0);
    }

    #[test]
    fn autotune_state_records_vector_search_metrics() {
        let config = super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = super::AutoTuneState::new(&config);
        state.record_vector_search(0.9, &config); // high precision
        state.record_vector_search(0.3, &config); // low precision
        state.record_vector_search(0.5, &config); // medium (no increment)

        assert_eq!(state.vector_search_count, 3);
        assert_eq!(state.high_precision_count, 1);
        assert_eq!(state.low_precision_count, 1);
    }

    #[test]
    fn autotune_state_adjusts_on_high_precision() {
        let config = super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = super::AutoTuneState::new(&config);
        // Record 20 searches: 15 high precision (75%)
        for _ in 0..15 {
            state.record_vector_search(0.9, &config);
        }
        for _ in 0..5 {
            state.record_vector_search(0.5, &config);
        }

        let adjusted = state.evaluate_and_adjust(&config);
        assert!(adjusted, "should adjust when precision is high");
        assert_eq!(
            state.current_min_query_chars, 60,
            "should increase min_query_chars"
        );
        assert_eq!(state.vector_search_count, 0, "should reset counters");
        assert_eq!(state.window_phase, 1);
    }

    #[test]
    fn autotune_state_adjusts_on_low_precision() {
        let config = super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = super::AutoTuneState::new(&config);
        state.current_min_query_chars = 100; // start higher
                                             // Record 20 searches: 10 low precision (50%)
        for _ in 0..10 {
            state.record_vector_search(0.2, &config);
        }
        for _ in 0..10 {
            state.record_vector_search(0.5, &config);
        }

        let adjusted = state.evaluate_and_adjust(&config);
        assert!(adjusted, "should adjust when precision is low");
        assert_eq!(
            state.current_min_query_chars, 80,
            "should decrease min_query_chars"
        );
    }

    #[test]
    fn autotune_state_respects_cooldown() {
        let config = super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = super::AutoTuneState::new(&config);
        // Fill evaluation window with high precision
        for _ in 0..15 {
            state.record_vector_search(0.9, &config);
        }
        for _ in 0..5 {
            state.record_vector_search(0.5, &config);
        }

        // First adjustment should succeed
        let adjusted1 = state.evaluate_and_adjust(&config);
        assert!(adjusted1);
        assert_eq!(state.cooldown_remaining, 2);
        let min_query_chars_after_first = state.current_min_query_chars;

        // Fill next evaluation window
        for _ in 0..15 {
            state.record_vector_search(0.9, &config);
        }
        for _ in 0..5 {
            state.record_vector_search(0.5, &config);
        }

        // Second adjustment attempt should fail due to cooldown
        let adjusted2 = state.evaluate_and_adjust(&config);
        assert!(!adjusted2, "should not adjust during cooldown");
        assert_eq!(
            state.current_min_query_chars, min_query_chars_after_first,
            "parameters should not change"
        );

        // Tick cooldown and try again
        state.tick_cooldown();
        state.tick_cooldown();
        state.tick_cooldown(); // Extra to fully clear

        // Now should be able to adjust again (cooldown expired and new window filled)
        for _ in 0..15 {
            state.record_vector_search(0.9, &config);
        }
        for _ in 0..5 {
            state.record_vector_search(0.5, &config);
        }
        let adjusted3 = state.evaluate_and_adjust(&config);
        assert!(adjusted3, "should adjust after cooldown expires");
    }

    #[test]
    fn autotune_cooldown_advances_across_windows() {
        let config = super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 4,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 2,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = super::AutoTuneState::new(&config);
        state.cooldown_remaining = 2;
        state.vector_search_count = 4;
        state.high_precision_count = 3;
        state.low_precision_count = 1;

        let advanced = state.advance_cooldown_window(&config);
        assert!(
            advanced,
            "cooldown window should advance once interval is reached"
        );
        assert_eq!(state.cooldown_remaining, 1);
        assert_eq!(state.vector_search_count, 0);
        assert_eq!(state.high_precision_count, 0);
        assert_eq!(state.low_precision_count, 0);
        assert_eq!(state.window_phase, 1);
    }

    #[test]
    fn autotune_state_load_and_save_roundtrip() {
        use tempfile::NamedTempFile;

        let config = super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let temp_file = NamedTempFile::new().expect("failed to create temp file");
        let path = temp_file
            .path()
            .to_str()
            .expect("failed to get path")
            .to_string();

        // Create, modify, and save state
        let mut state = super::AutoTuneState::new(&config);
        state.current_min_query_chars = 120;
        state.current_top_k = 3;
        state.window_phase = 5;
        state.vector_search_count = 10;
        state.high_precision_count = 8;
        state.low_precision_count = 1;

        state.save(&path).expect("failed to save state");

        // Load and verify
        let loaded = super::AutoTuneState::load_or_default(&path, &config);
        assert_eq!(loaded.current_min_query_chars, 120);
        assert_eq!(loaded.current_top_k, 3);
        assert_eq!(loaded.window_phase, 5);
        assert_eq!(loaded.vector_search_count, 10);
        assert_eq!(loaded.high_precision_count, 8);
        assert_eq!(loaded.low_precision_count, 1);
    }
}
