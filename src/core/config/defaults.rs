use super::types::{ComplianceConfig, ReputationConfig, RuntimeConfig, StartupContextConfig};

pub(crate) use crate::core::providers::{provider_spec_by_name, provider_specs};

// Re-export default functions that are referenced by `#[serde(default = "...")]`
// in types.rs via their canonical paths.
pub(crate) use self::default_functions::*;
pub(crate) use self::rules::*;

// ── Default helper functions ──────────────────────────────────────────────
mod default_functions {
    pub fn default_true() -> bool {
        true
    }
    pub fn default_coding_phase() -> String {
        "coding".to_string()
    }
    pub fn default_communication_style() -> String {
        "direct".to_string()
    }
    pub fn default_detail_level() -> String {
        "balanced".to_string()
    }
    pub fn default_learning_speed() -> String {
        "adaptive".to_string()
    }

    // ── Cache defaults ──────────────────────────────────────────
    pub fn default_cache_path() -> String {
        "sqlite3/acp_cache.sqlite3".to_string()
    }
    pub fn default_cache_ttl_seconds() -> u64 {
        3600
    }
    pub fn default_cache_max_entries() -> usize {
        5000
    }

    // ── Vector defaults ─────────────────────────────────────────
    pub fn default_vector_auto_mode() -> bool {
        true
    }
    pub fn default_vector_path() -> String {
        "sqlite3/acp_vector.sqlite3".to_string()
    }
    pub fn default_vector_dimensions() -> usize {
        192
    }
    pub fn default_vector_min_query_chars() -> usize {
        80
    }
    pub fn default_vector_top_k() -> usize {
        2
    }
    pub fn default_vector_min_similarity() -> f32 {
        0.82
    }
    pub fn default_vector_max_snippet_chars() -> usize {
        800
    }
    pub fn default_vector_max_entries() -> usize {
        10000
    }
    pub fn default_summary_enabled() -> bool {
        true
    }
    pub fn default_summary_trigger_messages() -> usize {
        8
    }
    pub fn default_summary_max_chars() -> usize {
        1200
    }

    // ── Compliance defaults ─────────────────────────────────────
    pub fn default_compliance_audit_retention_days() -> u32 {
        90
    }

    // ── Startup context defaults ────────────────────────────────
    pub fn default_startup_readme_max_chars() -> usize {
        2000
    }
    pub fn default_startup_recent_commits() -> usize {
        5
    }
    pub fn default_startup_io_timeout_ms() -> u64 {
        5_000
    }

    // ── Reputation defaults ─────────────────────────────────────
    pub fn default_reputation_alpha() -> f64 {
        0.2
    }
    pub fn default_reputation_degraded() -> f64 {
        0.65
    }
    pub fn default_reputation_excluded() -> f64 {
        0.30
    }

    // ── Runtime defaults ────────────────────────────────────────
    pub fn default_runtime_maintenance_interval_seconds() -> u64 {
        60
    }
    pub fn default_runtime_health_interval_seconds() -> u64 {
        120
    }
    pub fn default_runtime_shutdown_drain_seconds() -> u64 {
        30
    }
    pub fn default_runtime_entry_auth_api_key_env() -> String {
        "GO_ON_ENTRY_API_KEY".to_string()
    }
    pub fn default_runtime_entry_rate_limit_rpm() -> u64 {
        240
    }
    pub fn default_runtime_entry_rate_limit_burst() -> u64 {
        60
    }
    pub fn default_runtime_otel_exporter() -> String {
        "otlp".to_string()
    }
    pub fn default_runtime_otel_service_name() -> String {
        "go-on".to_string()
    }
    pub fn default_runtime_otel_sample_ratio() -> f64 {
        1.0
    }
    pub fn default_runtime_trace_slow_top_n() -> usize {
        20
    }
    pub fn default_runtime_skills_enabled() -> bool {
        true
    }
    pub fn default_runtime_skills_require_sha256() -> bool {
        true
    }
    pub fn default_runtime_skills_cache_dir() -> String {
        "./skills-cache".to_string()
    }
    pub fn default_runtime_user_auth_token_secret() -> String {
        "go-on-multi-user-secret".to_string()
    }
    pub fn default_runtime_user_auth_token_secret_env() -> String {
        "GO_ON_USER_AUTH_TOKEN_SECRET".to_string()
    }
    pub fn default_runtime_user_auth_token_ttl_seconds() -> u64 {
        86_400
    }
    pub fn default_runtime_tenant_default_daily_token_limit() -> u64 {
        1_000_000
    }
    pub fn default_runtime_tenant_default_concurrent_tasks() -> usize {
        10
    }
    pub fn default_runtime_i18n_default_language() -> String {
        "en".to_string()
    }
    pub fn default_runtime_tenant_default_daily_api_calls() -> usize {
        10_000
    }
}

// ── Default trait implementations ─────────────────────────────────────────

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            protocol_mode: None,
            platform_mode: Some("phase_compat".to_string()),
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
            otel_enabled: false,
            otel_exporter: default_runtime_otel_exporter(),
            otel_endpoint: Some("http://localhost:4317".to_string()),
            otel_service_name: default_runtime_otel_service_name(),
            otel_sample_ratio: default_runtime_otel_sample_ratio(),
            trace_slow_top_n: default_runtime_trace_slow_top_n(),
            skills_enabled: default_runtime_skills_enabled(),
            skills_import_enabled: false,
            skills_allowed_sources: Vec::new(),
            skills_require_sha256: default_runtime_skills_require_sha256(),
            skills_allow_floating_ref: false,
            skills_cache_dir: default_runtime_skills_cache_dir(),
            cors_allowed_origins: Vec::new(),
            user_auth_enabled: false,
            user_auth_token_secret: default_runtime_user_auth_token_secret(),
            user_auth_token_secret_env: default_runtime_user_auth_token_secret_env(),
            user_auth_token_ttl_seconds: default_runtime_user_auth_token_ttl_seconds(),
            tenant_default_daily_token_limit: default_runtime_tenant_default_daily_token_limit(),
            tenant_default_concurrent_tasks: default_runtime_tenant_default_concurrent_tasks(),
            i18n_default_language: default_runtime_i18n_default_language(),
            tenant_default_daily_api_calls: default_runtime_tenant_default_daily_api_calls(),
            enable_dag_execution: false,
            enable_agent_reroute: true,
            enable_metacognitive_feedback: true,
            enable_delphi_debate: false,
            governance_enabled: true,
            governance_policy_mode: String::new(),
            // Security (GAP-B52)
            request_signing_enabled: false,
            request_signing_public_key: String::new(),
            request_signing_hmac_secret: String::new(),
            mtls_enabled: false,
            mtls_ca_cert_path: String::new(),
            mtls_server_cert_path: String::new(),
            mtls_server_key_path: String::new(),
            mtls_require_client_cert: false,
            mtls_allowed_cns: String::new(),
        }
    }
}

impl Default for ComplianceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            standards: vec!["gdpr".to_string()],
            data_classification_default: "internal".to_string(),
            retention_policy_default: "standard_30d".to_string(),
            audit_retention_days: default_compliance_audit_retention_days(),
            pii_fields: Vec::new(),
        }
    }
}

impl Default for StartupContextConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            readme_max_chars: default_startup_readme_max_chars(),
            recent_commits: default_startup_recent_commits(),
            io_timeout_ms: default_startup_io_timeout_ms(),
        }
    }
}

impl Default for ReputationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ema_alpha: default_reputation_alpha(),
            degraded_threshold: default_reputation_degraded(),
            exclusion_threshold: default_reputation_excluded(),
        }
    }
}

// Provider specs are now managed centrally in `crate::core::providers`.

// ── Adaptive config helpers ───────────────────────────────────────────────
mod adaptive {
    use std::collections::HashMap;

    use serde_json::Value;

    use super::super::types::{
        AgentConfig, AppConfig, CacheConfig, ConversationContext, FlowConfig, LearningPreferences,
        MinimalConfig, PhaseConfig, PhaseOptions, RuntimeConfig, VectorConfig, WorkflowType,
    };

    /// Convert AppConfig to AdaptiveConfig
    impl From<AppConfig> for super::super::types::AdaptiveConfig {
        fn from(config: AppConfig) -> Self {
            let mut available_providers: Vec<String> = config
                .agents()
                .values()
                .filter_map(|agent| normalize_provider_name(&agent.agent_type))
                .collect();
            available_providers.sort();
            available_providers.dedup();

            if available_providers.is_empty() {
                available_providers.push("copilot".to_string());
            }

            super::super::types::AdaptiveConfig {
                adaptive_mode: true,
                minimal_config: MinimalConfig {
                    default_phase: config.default_phase().to_string(),
                    available_providers,
                    enable_cache: config.cache.is_some(),
                    enable_vector_memory: config.vector.is_some(),
                },
                learning_preferences: LearningPreferences::default(),
                conversation_context: Vec::new(),
            }
        }
    }

    impl super::super::types::AdaptiveConfig {
        /// Create adaptive configuration with auto-detection
        pub fn auto_detect() -> Self {
            let mut available_providers = Vec::new();

            for spec in super::provider_specs() {
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

            super::super::types::AdaptiveConfig {
                adaptive_mode: true,
                minimal_config: MinimalConfig {
                    default_phase: super::default_coding_phase(),
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
                ctx.last_interaction = super::now_ts();
            } else {
                self.conversation_context.push(ConversationContext {
                    conversation_id: conversation_id.to_string(),
                    expressed_preferences: preferences,
                    successful_adaptations: Vec::new(),
                    last_interaction: super::now_ts(),
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

            if text.contains("detailed")
                || text.contains("thorough")
                || text.contains("comprehensive")
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
                            self.learning_preferences.communication_style =
                                "explanatory".to_string();
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
                workflow_type: WorkflowType::Auto,
            };

            let mut phases = HashMap::new();
            phases.insert(
                "planning".to_string(),
                PhaseConfig {
                    description: "Adaptive planning phase".to_string(),
                    agents: vec![],
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
                    agents: vec![],
                    fallback: Some(true),
                    principles: Some(adaptive_principles(&self.learning_preferences, "coding")),
                    options: Some(adaptive_coding_options(
                        self.minimal_config.enable_cache,
                        self.minimal_config.enable_vector_memory,
                        &[],
                    )),
                },
            );
            phases.insert(
                "review".to_string(),
                PhaseConfig {
                    description: "Adaptive review phase".to_string(),
                    agents: vec![],
                    fallback: Some(true),
                    principles: Some(adaptive_principles(&self.learning_preferences, "review")),
                    options: Some(adaptive_review_options()),
                },
            );
            phases.insert(
                "delivery".to_string(),
                PhaseConfig {
                    description: "Adaptive delivery phase".to_string(),
                    agents: vec![],
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
                    path: "sqlite3/acp_cache.sqlite3".to_string(),
                    default_ttl_seconds: 3600,
                    max_entries: 5000,
                    connection_string: None,
                    read_replica_connection_string: None,
                    persist_enabled: true,
                })
            } else {
                None
            };

            let vector = if self.minimal_config.enable_vector_memory {
                Some(VectorConfig {
                    enabled: true,
                    auto_mode: true,
                    path: "sqlite3/acp_vector.sqlite3".to_string(),
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
                    read_replica_connection_string: None,
                })
            } else {
                None
            };

            AppConfig {
                schema_version: "1.0.0".to_string(),
                provider: crate::core::config::types::ProviderConfig {
                    default_phase: self.minimal_config.default_phase.clone(),
                    agents,
                    role_registry: HashMap::new(),
                },
                flow,
                phases,
                runtime: Some(RuntimeConfig::default()),
                cache,
                vector,
                autotune: Some(super::super::autotune::default_autotune_config()),
                security: crate::core::config::types::SecurityConfig::default(),
                feature: crate::core::config::types::FeatureConfig {
                    model_selection_mode: "adaptive".to_string(),
                    ..Default::default()
                },
                compliance: None,
                startup_context: None,
                reputation: None,
                protocol: None,
            }
        }
    }

    pub(crate) fn normalize_provider_name(agent_type: &str) -> Option<String> {
        super::provider_specs()
            .iter()
            .find(|spec| {
                spec.agent_type.eq_ignore_ascii_case(agent_type)
                    || spec.name.eq_ignore_ascii_case(agent_type)
                    || (spec.name == "anthropic" && agent_type.eq_ignore_ascii_case("claude"))
            })
            .map(|spec| spec.name.clone())
    }

    pub(crate) fn normalized_provider_list(providers: &[String]) -> Vec<String> {
        let mut normalized: Vec<String> = providers
            .iter()
            .filter_map(|provider| normalize_provider_name(provider))
            .collect();
        normalized.sort();
        normalized.dedup();

        if normalized.is_empty() {
            tracing::warn!("no recognized providers found, defaulting to 'copilot'");
            normalized.push("copilot".to_string());
        }

        normalized
    }

    pub(crate) fn default_agent_config(provider: &str) -> Option<AgentConfig> {
        let spec = super::provider_spec_by_name(provider)?;
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
            supports_vision: spec.supports_vision,
        })
    }

    pub(crate) fn adaptive_principles(
        preferences: &LearningPreferences,
        phase: &str,
    ) -> Vec<String> {
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
            "delivery" => {
                principles.push("Summarize outcome and residual risks concisely".to_string())
            }
            _ => {}
        }

        principles
    }

    pub(crate) fn adaptive_coding_options(
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

    pub(crate) fn adaptive_review_options() -> PhaseOptions {
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
}

// ── Rule loading utilities ────────────────────────────────────────────────
mod rules {
    use std::path::Path;

    use tracing::debug;

    use super::super::types::AppConfig;

    pub(crate) fn normalize_nested_phase_option_extra(config: &mut AppConfig) {
        for phase in config.phases.values_mut() {
            let Some(options) = phase.options.as_mut() else {
                continue;
            };

            let nested_extra = options.extra.remove("extra");
            let Some(serde_json::Value::Object(map)) = nested_extra else {
                continue;
            };

            for (key, value) in map {
                options.extra.entry(key).or_insert(value);
            }
        }
    }

    pub(crate) fn apply_auto_rules(config_path: &Path, config: &mut AppConfig) {
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

    pub(crate) fn shared_rule_paths(config_dir: &Path) -> Vec<std::path::PathBuf> {
        let rules_dir = config_dir.join("RULES");
        vec![
            config_dir.join("RULES.md"),
            rules_dir.join("global.md"),
            rules_dir.join("common.md"),
            rules_dir.join("local.md"),
            rules_dir.join("pua.md"),
        ]
    }

    pub(crate) fn phase_rule_paths(config_dir: &Path, phase_name: &str) -> Vec<std::path::PathBuf> {
        let rules_dir = config_dir.join("RULES");
        vec![
            config_dir.join(format!("{}.rules.md", phase_name)),
            rules_dir.join(format!("{}.md", phase_name)),
            rules_dir.join(format!("{}.rules.md", phase_name)),
            rules_dir.join(format!("{}.local.md", phase_name)),
        ]
    }

    pub(crate) fn load_optional_rule_items(path: &Path) -> Vec<String> {
        match std::fs::read_to_string(path) {
            Ok(content) => parse_rule_items(&content),
            Err(err) => {
                debug!("skipped optional rule file {}: {}", path.display(), err);
                Vec::new()
            }
        }
    }

    pub(crate) fn parse_rule_items(content: &str) -> Vec<String> {
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

    pub(crate) fn append_unique(target: &mut Vec<String>, items: Vec<String>) {
        for item in items {
            if !target.iter().any(|existing| existing == &item) {
                target.push(item);
            }
        }
    }
}

// ── Helper function to get current timestamp ──────────────────────────────

/// Helper function to get current timestamp
fn now_ts() -> i64 {
    crate::shared::timestamps::now_ts()
}

// ── Non-AI bootstrap config TOML ──────────────────────────────────────────

pub fn default_non_ai_config_toml() -> String {
    [
        "default_phase = \"coding\"",
        "model_selection_mode = \"adaptive\"",
        "[protocol]",
        "mode = \"adaptive\"",
        "",
        "[cache]",
        "enabled = true",
        "path = \"sqlite3/acp_cache.sqlite3\"",
        "default_ttl_seconds = 3600",
        "max_entries = 5000",
        "",
        "[vector]",
        "enabled = true",
        "auto_mode = true",
        "path = \"sqlite3/acp_vector.sqlite3\"",
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
        "workflow_type = \"auto\"",
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

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::adaptive;

    #[test]
    fn normalize_provider_name_maps_claude_to_anthropic() {
        assert_eq!(
            adaptive::normalize_provider_name("claude").as_deref(),
            Some("anthropic")
        );
        assert_eq!(
            adaptive::normalize_provider_name("anthropic").as_deref(),
            Some("anthropic")
        );
    }

    #[test]
    fn default_agent_config_reads_provider_specs() {
        let openai = adaptive::default_agent_config("openai")
            .expect("openai should be available in provider specs");
        assert_eq!(openai.agent_type, "openai");
        assert_eq!(
            openai.api_key_env.as_deref(),
            Some("keyring://go-on/openai_api_key")
        );
        assert_eq!(openai.url.as_deref(), Some("https://api.openai.com/v1"));
    }
}
