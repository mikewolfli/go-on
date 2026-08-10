use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use super::types::{
    AppConfig, ComplianceConfig, ReputationConfig, RuntimeConfig, StartupContextConfig,
};

#[cfg(test)]
mod tests {
    use super::RuntimeConfig;

    /// Default startup invariants: the default runtime config must always be
    /// well-formed (service name set, governance enabled, sane health interval).
    /// Moved inline from the former `tests/structural/test_server_startup_health.rs`.
    #[test]
    fn test_default_runtime_config_is_well_formed() {
        let config = RuntimeConfig::default();
        assert!(
            !config.otel_service_name.is_empty(),
            "otel_service_name must be set"
        );
        assert!(
            config.governance_enabled,
            "governance must be enabled by default"
        );
        assert!(
            config.health_interval_seconds > 0,
            "health_interval_seconds must be positive"
        );
    }
}

pub(crate) use crate::core::providers::provider_specs;

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
            evolution_enabled: false,
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
                minimal_config: super::super::types::MinimalConfig {
                    default_phase: super::default_coding_phase(),
                    available_providers,
                    enable_cache: true,
                    enable_vector_memory: true,
                },
            }
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

/// Ensure a non-AI bootstrap config exists at `path`.
///
/// Writes [`default_non_ai_config_toml`] when the file is missing or blank,
/// verifying the defaults parse in memory before touching the disk (same
/// data-safety ordering the config parser uses). Returns `Ok(true)` when the
/// file was written, `Ok(false)` when it already had content.
///
/// Single helper shared by the config parser (`load/parser.rs`, blank-file
/// path) and the CLI startup path (`main/server.rs`, missing-file path) so
/// the "write default config" behavior lives in exactly one place.
pub(crate) fn ensure_bootstrap_config(path: &Path) -> Result<bool> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("failed to read config file: {}", path.display()))
        }
    };
    if !content.trim().is_empty() {
        return Ok(false);
    }

    let bootstrap = default_non_ai_config_toml();
    // Verify the bootstrap defaults parse correctly in memory before writing to disk.
    let _parsed: AppConfig = toml::from_str(&bootstrap).map_err(|e| {
        anyhow::anyhow!(
            "{}: {}",
            crate::i18n::runtime::tf(
                "error.config_parse_failed",
                &[("error", &path.display().to_string())],
            ),
            e,
        )
    })?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory: {}", parent.display()))?;
    }
    fs::write(path, &bootstrap)
        .with_context(|| format!("failed to write bootstrap defaults to {}", path.display()))?;
    tracing::info!(
        "missing or blank config; wrote non-AI bootstrap defaults to {}",
        path.display()
    );
    Ok(true)
}
