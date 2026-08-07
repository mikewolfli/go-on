use anyhow::{Context, Result};

use crate::i18n::runtime::tf;

use super::super::types::{AppConfig, PhaseOptions};

impl AppConfig {
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
            .any(|phase| phase == self.default_phase())
        {
            anyhow::bail!(
                "{}",
                tf(
                    "error.default_phase_not_in_list",
                    &[("phase", self.default_phase())]
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
                    if !self.agents().contains_key(agent_name) {
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
                        if !self.agents().contains_key(reviewer) {
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
                        if !self.agents().contains_key(reviewer) {
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
            "adaptive_selector",
            "advanced_modules",
            "cost_optimizer",
            "speed_optimizer",
            "reliability_optimizer",
            "hyper_resilience",
            // Legacy alias: failure_prevention was merged into hyper_resilience.
            "failure_prevention",
        ],
    )?;

    if let Some(policy) = options
        .extra
        .get("review_timeout_policy")
        .and_then(|value| value.as_str())
    {
        if !policy.eq_ignore_ascii_case("reject")
            && !policy.eq_ignore_ascii_case("degrade_single")
            && !policy.eq_ignore_ascii_case("warn")
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

pub(crate) fn phase_uses_complex_autopilot(options: Option<&PhaseOptions>) -> bool {
    options
        .and_then(|opts| opts.autopilot_complexity.as_deref())
        .map(|value| value.eq_ignore_ascii_case("complex"))
        .unwrap_or(false)
}
