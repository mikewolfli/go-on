use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use tracing::{info, warn};

use crate::agent::inspect_secret_pool;
use crate::i18n::runtime::tf;
use crate::orchestration::roles::install_role_registry;

use super::defaults;
use super::types::{AppConfig, PhaseOptions};

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

impl AppConfig {
    /// Load configuration from file
    ///
    /// # Arguments
    /// * `path` - Path to configuration file
    ///
    /// # Returns
    /// * `Result<Self>` - Returns Ok(Self) if loaded successfully, or an error if something goes wrong
    #[must_use]
    #[allow(clippy::double_must_use)]
    pub fn load(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path).with_context(|| {
            tf(
                "error.config_read_failed",
                &[("error", &path.display().to_string())],
            )
        })?;

        let normalized = if content.trim().is_empty() {
            let bootstrap = defaults::default_non_ai_config_toml();
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

        let mut cfg: AppConfig = toml::from_str(&normalized).with_context(|| {
            tf(
                "error.config_parse_failed",
                &[("error", &path.display().to_string())],
            )
        })?;
        defaults::normalize_nested_phase_option_extra(&mut cfg);
        defaults::apply_auto_rules(path, &mut cfg);
        if !cfg.role_registry.is_empty() {
            install_role_registry(cfg.role_registry.clone());
        }

        // Validate and migrate schema version based on the parsed config's version field.
        // If schema_version is missing from the config, default to "0.1.0" so migration is triggered.
        let schema_version_str = if normalized.contains("schema_version") {
            cfg.schema_version.clone()
        } else {
            warn!(
                "Config file does not contain a schema_version field; defaulting to \"0.1.0\" for migration"
            );
            "0.1.0".to_string()
        };

        let parsed_version =
            match super::schema_version::SchemaVersion::from_str(&schema_version_str) {
                Ok(v) => v,
                Err(e) => {
                    warn!(
                        "Failed to parse schema_version '{}' from config: {}; skipping migration",
                        schema_version_str, e
                    );
                    return Ok(cfg);
                }
            };

        let manager = super::schema_version::SchemaManager::new();
        match manager.validate_version(&parsed_version) {
            Ok(()) => {
                if parsed_version != super::schema_version::SchemaVersion::CURRENT {
                    match manager.find_migration_path(&parsed_version) {
                        Some(steps) => {
                            if steps.is_empty() {
                                info!(
                                    "Config schema version {} is compatible with current {}; no migration needed",
                                    parsed_version,
                                    super::schema_version::SchemaVersion::CURRENT
                                );
                            } else {
                                info!(
                                    "Applying {} config migration step(s) from {} to {}",
                                    steps.len(),
                                    parsed_version,
                                    super::schema_version::SchemaVersion::CURRENT
                                );
                                for step in &steps {
                                    info!(
                                        "  Migration: {} -> {}: {}",
                                        step.from_version, step.to_version, step.description
                                    );
                                }
                            }
                            // Update the config's schema_version to CURRENT after migration
                            cfg.schema_version =
                                super::schema_version::SchemaVersion::CURRENT.to_string();
                        }
                        None => {
                            warn!(
                                "No migration path found from {} to {}; config may be incompatible",
                                parsed_version,
                                super::schema_version::SchemaVersion::CURRENT
                            );
                        }
                    }
                } else {
                    info!(
                        "Config schema version {} matches current version",
                        parsed_version
                    );
                }
            }
            Err(msg) => {
                warn!(
                    "Config schema version validation failed: {}; attempting to load anyway",
                    msg
                );
            }
        }

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
    #[must_use]
    #[allow(clippy::double_must_use)]
    pub fn validate(&self) -> Result<()> {
        if self.flow.phases.is_empty() {
            anyhow::bail!("{}", tf("error.flow_phases_empty", &[]));
        }

        if !self
            .flow
            .phases
            .iter()
            .any(|phase| phase == &self.default_phase)
        {
            anyhow::bail!(
                "{}",
                tf(
                    "error.default_phase_not_in_list",
                    &[("phase", &self.default_phase)]
                )
            );
        }

        for phase_name in &self.flow.phases {
            let phase_cfg = self
                .phases
                .get(phase_name)
                .with_context(|| format!("phase '{}' missing in [phases]", phase_name))?;

            // Agents list is optional: Path B (auto-map) resolves agents dynamically
            // from the registry at request time. Skip validation when empty.
            if !phase_cfg.agents.is_empty() {
                for agent_name in &phase_cfg.agents {
                    if !self.agents.contains_key(agent_name) {
                        anyhow::bail!(
                            "{}",
                            tf(
                                "error.phase_references_undefined_agent",
                                &[("phase", phase_name), ("agent", agent_name)]
                            )
                        );
                    }
                }
            }

            if let Some(options) = phase_cfg.options.as_ref() {
                validate_phase_options(phase_name, options)?;
            }

            if phase_uses_complex_autopilot(phase_cfg.options.as_ref()) {
                if !self.flow.phases.iter().any(|phase| phase == "review") {
                    anyhow::bail!(
                        "{}",
                        tf(
                            "error.complex_autopilot_missing_review_phase",
                            &[("phase", phase_name)]
                        )
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
                            "{}",
                            tf(
                                "error.complex_autopilot_no_review_agents",
                                &[("phase", phase_name)]
                            )
                        )
                    })?;

                if reviewers.len() < 2 {
                    anyhow::bail!(
                        "{}",
                        tf(
                            "error.complex_autopilot_min_review_agents",
                            &[("phase", phase_name)]
                        )
                    );
                }
                if reviewers.len() > 2 {
                    anyhow::bail!(
                        "{}",
                        tf(
                            "error.complex_autopilot_max_review_agents",
                            &[("phase", phase_name)]
                        )
                    );
                }

                if review_phase.agents.is_empty() {
                    // Path B: agents resolved dynamically — skip static agent checks.
                    // Still verify that reviewer names exist in the config.
                    for reviewer in reviewers.iter().take(2) {
                        if !self.agents.contains_key(reviewer) {
                            anyhow::bail!(
                                "{}",
                                tf(
                                    "error.phase_references_undefined_review_agent",
                                    &[("phase", phase_name), ("agent", reviewer)]
                                )
                            );
                        }
                    }
                } else {
                    if review_phase.agents.len() < 2 {
                        anyhow::bail!("{}", tf("error.phases_review_min_agents", &[]));
                    }

                    for reviewer in reviewers.iter().take(2) {
                        if !self.agents.contains_key(reviewer) {
                            anyhow::bail!(
                                "{}",
                                tf(
                                    "error.phase_references_undefined_review_agent",
                                    &[("phase", phase_name), ("agent", reviewer)]
                                )
                            );
                        }

                        if !review_phase.agents.iter().any(|agent| agent == reviewer) {
                            anyhow::bail!(
                                "{}",
                                tf(
                                    "error.review_agent_must_be_in_phases",
                                    &[("agent", reviewer)]
                                )
                            );
                        }
                    }
                }
            }
        }

        if let Some(cache) = &self.cache {
            if cache.enabled {
                if cache.default_ttl_seconds == 0 {
                    anyhow::bail!("{}", tf("error.cache_ttl_must_be_positive", &[]));
                }
                if cache.max_entries == 0 {
                    anyhow::bail!("{}", tf("error.cache_max_entries_must_be_positive", &[]));
                }
            }
        }

        if let Some(runtime) = &self.runtime {
            if runtime.maintenance_interval_seconds == 0 {
                anyhow::bail!(
                    "{}",
                    tf(
                        "error.runtime_must_be_positive",
                        &[("field", "maintenance_interval_seconds")]
                    )
                );
            }
            if runtime.health_interval_seconds == 0 {
                anyhow::bail!(
                    "{}",
                    tf(
                        "error.runtime_must_be_positive",
                        &[("field", "health_interval_seconds")]
                    )
                );
            }
            if runtime.shutdown_drain_seconds == 0 {
                anyhow::bail!(
                    "{}",
                    tf(
                        "error.runtime_must_be_positive",
                        &[("field", "shutdown_drain_seconds")]
                    )
                );
            }
            if runtime.entry_auth_api_key_env.trim().is_empty() {
                anyhow::bail!("{}", tf("error.entry_auth_api_key_empty", &[]));
            }
            if runtime.entry_rate_limit_rpm == 0 {
                anyhow::bail!("{}", tf("error.entry_rate_limit_rpm_positive", &[]));
            }
            if runtime.entry_rate_limit_burst == 0 {
                anyhow::bail!("{}", tf("error.entry_rate_limit_burst_positive", &[]));
            }
            if runtime.sqlite_vacuum_interval_cycles == 0 {
                anyhow::bail!(
                    "{}",
                    tf(
                        "error.runtime_must_be_positive",
                        &[("field", "sqlite_vacuum_interval_cycles")]
                    )
                );
            }
            if !(0.0..=1.0).contains(&runtime.otel_sample_ratio) {
                anyhow::bail!("{}", tf("error.otel_sample_ratio_range", &[]));
            }
            if runtime.trace_slow_top_n == 0 {
                anyhow::bail!(
                    "{}",
                    tf(
                        "error.runtime_must_be_positive",
                        &[("field", "trace_slow_top_n")]
                    )
                );
            }
            let exporter = runtime.otel_exporter.to_ascii_lowercase();
            if runtime.otel_enabled && exporter != "otlp" && exporter != "jaeger" {
                anyhow::bail!("{}", tf("error.otel_exporter_invalid", &[]));
            }
        }

        if let Some(vector) = &self.vector {
            if vector.enabled {
                if vector.dimensions == 0 {
                    anyhow::bail!("{}", tf("error.vector_dimensions_positive", &[]));
                }
                if vector.top_k == 0 {
                    anyhow::bail!("{}", tf("error.vector_top_k_positive", &[]));
                }
                if !(0.0..=1.0).contains(&vector.min_similarity) {
                    anyhow::bail!("{}", tf("error.vector_min_similarity_range", &[]));
                }
                if vector.max_entries == 0 {
                    anyhow::bail!("{}", tf("error.vector_max_entries_positive", &[]));
                }
                if vector.summary_trigger_messages == 0 {
                    anyhow::bail!(
                        "{}",
                        tf(
                            "error.phase_field_positive",
                            &[("phase", "vector"), ("field", "summary_trigger_messages")]
                        )
                    );
                }
                if vector.summary_max_chars == 0 {
                    anyhow::bail!(
                        "{}",
                        tf(
                            "error.phase_field_positive",
                            &[("phase", "vector"), ("field", "summary_max_chars")]
                        )
                    );
                }
            }
        }

        if let Some(autotune) = &self.autotune {
            if autotune.enabled {
                if autotune.evaluate_interval == 0 {
                    anyhow::bail!("{}", tf("error.autotune_interval_positive", &[]));
                }
                if autotune.min_query_chars_step == 0 {
                    anyhow::bail!(
                        "{}",
                        tf(
                            "error.phase_field_positive",
                            &[("phase", "autotune"), ("field", "min_query_chars_step")]
                        )
                    );
                }
                if autotune.min_query_chars_min == 0 {
                    anyhow::bail!(
                        "{}",
                        tf(
                            "error.phase_field_positive",
                            &[("phase", "autotune"), ("field", "min_query_chars_min")]
                        )
                    );
                }
                if autotune.min_query_chars_min > autotune.min_query_chars_max {
                    anyhow::bail!(
                        "{}",
                        tf(
                            "error.autotune_min_le_max",
                            &[
                                ("field1", "min_query_chars_min"),
                                ("field2", "min_query_chars_max")
                            ]
                        )
                    );
                }
                if autotune.max_top_k == 0 {
                    anyhow::bail!(
                        "{}",
                        tf(
                            "error.phase_field_positive",
                            &[("phase", "autotune"), ("field", "max_top_k")]
                        )
                    );
                }
                if !(0.0..=1.0).contains(&autotune.low_precision_threshold) {
                    anyhow::bail!(
                        "{}",
                        tf(
                            "error.autotune_range_invalid",
                            &[
                                ("field", "low_precision_threshold"),
                                ("min", "0"),
                                ("max", "1")
                            ]
                        )
                    );
                }
                if !(0.0..=1.0).contains(&autotune.high_precision_threshold) {
                    anyhow::bail!(
                        "{}",
                        tf(
                            "error.autotune_range_invalid",
                            &[
                                ("field", "high_precision_threshold"),
                                ("min", "0"),
                                ("max", "1")
                            ]
                        )
                    );
                }
                if autotune.low_precision_threshold >= autotune.high_precision_threshold {
                    anyhow::bail!(
                        "{}",
                        tf(
                            "error.autotune_min_le_max",
                            &[
                                ("field1", "low_precision_threshold"),
                                ("field2", "high_precision_threshold")
                            ]
                        )
                    );
                }
                if autotune.min_vector_searches == 0 {
                    anyhow::bail!(
                        "{}",
                        tf(
                            "error.phase_field_positive",
                            &[("phase", "autotune"), ("field", "min_vector_searches")]
                        )
                    );
                }
                if autotune.summary_trigger_min == 0 {
                    anyhow::bail!(
                        "{}",
                        tf(
                            "error.phase_field_positive",
                            &[("phase", "autotune"), ("field", "summary_trigger_min")]
                        )
                    );
                }
                if autotune.summary_trigger_min > autotune.summary_trigger_max {
                    anyhow::bail!(
                        "{}",
                        tf(
                            "error.autotune_min_le_max",
                            &[
                                ("field1", "summary_trigger_min"),
                                ("field2", "summary_trigger_max")
                            ]
                        )
                    );
                }
            }
        }

        Ok(())
    }
}

// ── Phase option validation ───────────────────────────────────────────────

fn validate_phase_options(phase_name: &str, options: &PhaseOptions) -> Result<()> {
    if matches!(options.cache_ttl_seconds, Some(0)) {
        anyhow::bail!(
            "{}",
            tf(
                "error.phase_field_positive",
                &[("phase", phase_name), ("field", "cache_ttl_seconds")]
            )
        );
    }
    if matches!(options.vector_min_query_chars, Some(0)) {
        anyhow::bail!(
            "{}",
            tf(
                "error.phase_field_positive",
                &[("phase", phase_name), ("field", "vector_min_query_chars")]
            )
        );
    }
    if matches!(options.vector_top_k, Some(0)) {
        anyhow::bail!(
            "{}",
            tf(
                "error.phase_field_positive",
                &[("phase", phase_name), ("field", "vector_top_k")]
            )
        );
    }
    if let Some(value) = options.vector_min_similarity {
        if !(0.0..=1.0).contains(&value) {
            anyhow::bail!(
                "{}",
                tf(
                    "error.phase_option_must_be_number",
                    &[("phase", phase_name), ("option", "vector_min_similarity")]
                )
            );
        }
    }
    if matches!(options.vector_max_snippet_chars, Some(0)) {
        anyhow::bail!(
            "{}",
            tf(
                "error.phase_field_positive",
                &[("phase", phase_name), ("field", "vector_max_snippet_chars")]
            )
        );
    }
    if matches!(options.summary_trigger_messages, Some(0)) {
        anyhow::bail!(
            "{}",
            tf(
                "error.phase_field_positive",
                &[("phase", phase_name), ("field", "summary_trigger_messages")]
            )
        );
    }
    if matches!(options.summary_max_chars, Some(0)) {
        anyhow::bail!(
            "{}",
            tf(
                "error.phase_field_positive",
                &[("phase", phase_name), ("field", "summary_max_chars")]
            )
        );
    }
    if matches!(options.max_history_messages, Some(0)) {
        anyhow::bail!(
            "{}",
            tf(
                "error.phase_field_positive",
                &[("phase", phase_name), ("field", "max_history_messages")]
            )
        );
    }
    if matches!(options.max_history_chars, Some(0)) {
        anyhow::bail!(
            "{}",
            tf(
                "error.phase_field_positive",
                &[("phase", phase_name), ("field", "max_history_chars")]
            )
        );
    }
    if matches!(options.request_timeout_seconds, Some(0)) {
        anyhow::bail!(
            "{}",
            tf(
                "error.phase_field_positive",
                &[("phase", phase_name), ("field", "request_timeout_seconds")]
            )
        );
    }
    if matches!(options.review_timeout_seconds, Some(0)) {
        anyhow::bail!(
            "{}",
            tf(
                "error.phase_field_positive",
                &[("phase", phase_name), ("field", "review_timeout_seconds")]
            )
        );
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
                "{}",
                tf(
                    "error.phase_option_must_be_bool",
                    &[("phase", phase_name), ("option", "review_timeout_policy")]
                )
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
                "{}",
                tf(
                    "error.phase_option_must_be_number",
                    &[("phase", phase_name), ("option", "required_approvals")]
                )
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
            "{}",
            tf(
                "error.phase_option_must_be_number",
                &[("phase", phase_name), ("option", key)]
            )
        );
    };

    if num < min || num > max {
        anyhow::bail!(
            "{}",
            tf(
                "error.phase_option_must_be_number",
                &[("phase", phase_name), ("option", key)]
            )
        );
    }

    Ok(())
}

fn validate_extra_bool(phase_name: &str, options: &PhaseOptions, key: &str) -> Result<()> {
    let Some(value) = options.extra.get(key) else {
        return Ok(());
    };

    if !value.is_boolean() {
        anyhow::bail!(
            "{}",
            tf(
                "error.phase_option_must_be_bool",
                &[("phase", phase_name), ("option", key)]
            )
        );
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
            "{}",
            tf(
                "error.phase_option_must_be_bool",
                &[("phase", phase_name), ("option", key)]
            )
        );
    };

    for item in items {
        let Some(module_name) = item.as_str() else {
            anyhow::bail!(
                "{}",
                tf(
                    "error.phase_option_must_be_bool",
                    &[("phase", phase_name), ("option", key)]
                )
            );
        };

        if !allowed.iter().any(|candidate| candidate == &module_name) {
            anyhow::bail!(
                "{}",
                tf(
                    "error.phase_option_must_be_number",
                    &[("phase", phase_name), ("option", key)]
                )
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
        anyhow::bail!(
            "{}",
            tf(
                "error.phase_option_must_be_number",
                &[("phase", phase_name), ("option", key)]
            )
        );
    };

    if num < min || num > max {
        anyhow::bail!(
            "{}",
            tf(
                "error.phase_option_must_be_number",
                &[("phase", phase_name), ("option", key)]
            )
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

// ── Environment variable helpers ──────────────────────────────────────────

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

fn required_env_vars(agent: &super::types::AgentConfig) -> Vec<String> {
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

// ── Production strict checks ──────────────────────────────────────────────

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

// ── Runtime readiness ─────────────────────────────────────────────────────

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
                "{}",
                tf(
                    "error.missing_field",
                    &[(
                        "field",
                        &format!("production_strict agent secrets: {}", blocked)
                    )]
                )
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
                "{}",
                tf(
                    "error.missing_field",
                    &[(
                        "field",
                        &format!(
                            "production_strict violations: {}",
                            strict_violations.join("; ")
                        )
                    )]
                )
            );
        }
    }

    // F-GAP-14: warn when user_auth is enabled but token secret is still the default
    if let Some(runtime) = &config.runtime {
        if runtime.user_auth_enabled && runtime.user_auth_token_secret == "go-on-multi-user-secret"
        {
            warn!(
                "runtime.user_auth_enabled=true with default user_auth_token_secret 'go-on-multi-user-secret'; \
                 set a strong, unique token secret in production"
            );
        }
    }

    Ok(build_config_health_report(config_path, config))
}

// ── Config health / warnings ──────────────────────────────────────────────

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

    // F-GAP-14: warn when CORS is configured with a wildcard origin
    if let Some(runtime) = &config.runtime {
        if runtime.cors_allowed_origins.iter().any(|o| o == "*") {
            warnings.push(ConfigWarning {
                code: "CORS_WILDCARD_ORIGIN".to_string(),
                severity: ConfigWarningSeverity::Warn,
                message: "runtime.cors_allowed_origins contains '*' wildcard; this allows any origin to access the API. Consider restricting to specific origins for production.".to_string(),
            });
        }
    }

    for path in defaults::shared_rule_paths(config_path.parent().unwrap_or_else(|| Path::new(".")))
    {
        push_rule_warning(&mut warnings, &path, "RULE_FILE_EMPTY");
    }
    for phase_name in config.phases.keys() {
        for path in defaults::phase_rule_paths(
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
    if path.exists() && defaults::load_optional_rule_items(path).is_empty() {
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

// ── Keyring / secret validation ───────────────────────────────────────────

fn keyring_env_fallback_candidates(service: &str, account: &str) -> Vec<String> {
    let mut candidates = Vec::new();

    if account == "openai_api_key" {
        candidates.push("OPENAI_API_KEY".to_string());
    }

    if account == "openai_compatible_api_key" {
        candidates.push("OPENAI_COMPATIBLE_API_KEY".to_string());
        candidates.push("OPENAI_API_KEY".to_string());
    }

    if service == "go-on" && (account == "copilot_api_key" || account == "github_copilot_token") {
        // Copilot supports both historical and current names.
        candidates.push("GITHUB_COPILOT_TOKEN".to_string());
        candidates.push("GITHUB_TOKEN".to_string());
    }

    candidates.push(account.replace('-', "_").to_ascii_uppercase());
    candidates.push(
        format!("{}_{}", service, account)
            .replace('-', "_")
            .to_ascii_uppercase(),
    );

    candidates.sort();
    candidates.dedup();
    candidates
}

fn keyring_lookup_accounts(service: &str, account: &str) -> Vec<(String, String)> {
    let mut targets = vec![(service.to_string(), account.to_string())];

    // Backward/forward compatibility for Copilot key naming.
    if service == "go-on" {
        if account == "copilot_api_key" {
            targets.push((service.to_string(), "github_copilot_token".to_string()));
        } else if account == "github_copilot_token" {
            targets.push((service.to_string(), "copilot_api_key".to_string()));
        }
    }

    targets
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
    let mut secret = String::new();
    for (service_name, account_name) in keyring_lookup_accounts(service, account) {
        match keyring::Entry::new(&service_name, &account_name) {
            Ok(entry) => match entry.get_password() {
                Ok(value) if !value.trim().is_empty() => {
                    secret = value;
                    break;
                }
                Ok(_) => {
                    // resolved to empty value — fallback to env
                }
                Err(err) => {
                    warn!(
                        "keyring entry for {}/{} cannot be read: {}",
                        service_name, account_name, err
                    );
                }
            },
            Err(err) => {
                warn!(
                    "failed to open keyring entry for {}/{}: {}",
                    service_name, account_name, err
                );
            }
        }
    }

    if secret.is_empty() {
        let fallback_candidates = keyring_env_fallback_candidates(service, account);
        for env_name in &fallback_candidates {
            if let Ok(env_value) = std::env::var(env_name) {
                if !env_value.trim().is_empty() {
                    warn!(
                        "{} keyring ref {} fell back to env {}",
                        field_name, value, env_name
                    );
                    secret = env_value;
                    break;
                }
            }
        }

        if secret.is_empty() {
            anyhow::bail!(
                "{}",
                tf(
                    "error.missing_field",
                    &[("field", &format!("keyring {}/{}", service, account))]
                )
            );
        }
    }

    // Validate secret key security.
    validate_secret_security(&secret, field_name)?;

    Ok(())
}

/// Validates the security of a secret string.
///
/// # Parameters
/// * `secret` - The secret value to validate.
/// * `field_name` - Field name used in error messages.
///
/// # Returns
/// * `Result<()>` - `Ok` if the secret is considered safe; an error otherwise.
fn validate_secret_security(secret: &str, field_name: &str) -> Result<()> {
    use tracing::warn;

    if secret.trim().is_empty() {
        anyhow::bail!("{}", tf("error.missing_field", &[("field", field_name)]));
    }

    // Check for newlines (possible multi-line secret or injection attempt).
    if secret.contains('\n') || secret.contains('\r') {
        warn!(
            "{} contains newline characters, which may be a security issue",
            field_name
        );
    }

    // Check secret length — very short secrets are likely insecure.
    if secret.len() < 8 {
        warn!(
            "{} is very short ({} characters), which may be insecure",
            field_name,
            secret.len()
        );
    }

    // Check for common insecure patterns.
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

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::*;
    use crate::core::config::types::{
        AgentConfig, CacheConfig, FlowConfig, PhaseConfig, PhaseOptions, RuntimeConfig,
        VectorConfig, WorkflowType,
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
            supports_vision: None,
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
                supports_vision: None,
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
                supports_vision: None,
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
            schema_version: "1.0.0".to_string(),
            default_phase: "coding".to_string(),
            agents,
            flow: FlowConfig {
                name: "flow".to_string(),
                phases: vec!["coding".to_string(), "review".to_string()],
                workflow_type: WorkflowType::Auto,
            },
            phases,
            runtime: Some(RuntimeConfig::default()),
            cache: None,
            vector: None,
            autotune: None,
            model_selection_mode: "adaptive".to_string(),
            compliance: None,
            startup_context: None,
            scheduler: None,
            reputation: None,
            role_registry: HashMap::new(),
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
            err.to_string().contains("error.default_phase_not_in_list"),
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
                .contains("error.phase_references_undefined_agent"),
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
        cfg.autotune = Some(crate::core::config::autotune::AutoTuneConfig {
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
            err.to_string().contains("error.autotune_min_le_max"),
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
            platform_mode: Some("phase_compat".to_string()),
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
            cors_allowed_origins: Vec::new(),
            user_auth_enabled: false,
            user_auth_token_secret: String::new(),
            user_auth_token_secret_env: "GO_ON_USER_AUTH_TOKEN_SECRET".to_string(),
            user_auth_token_ttl_seconds: 86400,
            tenant_default_daily_token_limit: 1_000_000,
            tenant_default_concurrent_tasks: 10,
            tenant_default_daily_api_calls: 10_000,
            i18n_default_language: "en".to_string(),
            enable_dag_execution: false,
            enable_agent_reroute: true,
            enable_metacognitive_feedback: true,
        });

        let err = cfg
            .validate()
            .expect_err("zero maintenance interval must fail");
        assert!(
            err.to_string().contains("error.runtime_must_be_positive"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_autotune_summary_range() {
        let mut cfg = valid_config();
        cfg.autotune = Some(crate::core::config::autotune::AutoTuneConfig {
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
            err.to_string().contains("error.autotune_min_le_max"),
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
                .contains("error.complex_autopilot_min_review_agents"),
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
                .contains("error.review_agent_must_be_in_phases"),
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
            err.to_string().contains("error.phase_field_positive"),
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
                .contains("error.phase_option_must_be_number"),
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
                .contains("error.phase_option_must_be_number"),
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
            err.to_string()
                .contains("error.complex_autopilot_max_review_agents"),
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
                .contains("error.phase_option_must_be_number"),
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
            err.to_string()
                .contains("error.phase_option_must_be_number"),
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
            err.to_string()
                .contains("error.phase_option_must_be_number"),
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
            err.to_string().contains("error.phase_option_must_be_bool"),
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
            err.to_string().contains("error.phase_option_must_be_bool"),
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
            err.to_string()
                .contains("error.phase_option_must_be_number"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn missing_env_vars_detects_agent_requirements() {
        let cfg = valid_config();
        let missing = missing_env_vars(&cfg);

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

        validate_runtime_readiness(&config_path, &cfg)
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

        validate_runtime_readiness(&config_path, &cfg)
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

        let err = validate_runtime_readiness(&config_path, &cfg)
            .expect_err("strict mode should fail when any configured agent is missing secrets");
        assert!(
            err.to_string().contains("error.missing_field"),
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

        let err = validate_runtime_readiness(&config_path, &cfg).expect_err(
            "strict mode should fail when entry auth is disabled for exposed HTTP endpoint",
        );
        assert!(
            err.to_string().contains("error.missing_field"),
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

        validate_runtime_readiness(&config_path, &cfg)
            .expect("strict mode should pass when all strict checks are satisfied");
    }

    #[test]
    fn adaptive_template_loads_and_validates() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config/config.toml");
        let cfg = AppConfig::load(&path).expect("config.toml should parse");

        cfg.validate()
            .expect("config.toml should be internally consistent");
    }

    #[test]
    fn build_config_health_report_recommends_minimal_on_clean_config() {
        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "# test").expect("config marker should be written");

        let cfg = valid_config();
        let report = build_config_health_report(&config_path, &cfg);

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
            connection_string: None,
        });
        cfg.vector = Some(VectorConfig {
            enabled: false,
            auto_mode: true,
            path: "vector.sqlite3".to_string(),
            connection_string: None,
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
            platform_mode: Some("phase_compat".to_string()),
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
            cors_allowed_origins: Vec::new(),
            user_auth_enabled: false,
            user_auth_token_secret: String::new(),
            user_auth_token_secret_env: "GO_ON_USER_AUTH_TOKEN_SECRET".to_string(),
            user_auth_token_ttl_seconds: 86400,
            tenant_default_daily_token_limit: 1_000_000,
            tenant_default_concurrent_tasks: 10,
            tenant_default_daily_api_calls: 10_000,
            i18n_default_language: "en".to_string(),
            enable_dag_execution: false,
            enable_agent_reroute: true,
            enable_metacognitive_feedback: true,
        });

        let report = build_config_health_report(&config_path, &cfg);
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
}
