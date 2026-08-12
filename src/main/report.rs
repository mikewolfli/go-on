use crate::config::{AppConfig, ConfigWarning, ConfigWarningSeverity};
use crate::i18n::runtime::tf;
use crate::reinforcement::RuntimeHealthcheckReport;
use crate::setup::config_gen::default_recommendation_snapshot;
use crate::setup::recommendation_snapshot_for_config;

/// Emit configuration warnings to the log and optionally to stderr
///
/// # Arguments
/// * `warnings` - Slice of configuration warnings
/// * `mirror_stderr` - Whether to also print warnings to stderr
pub(crate) fn emit_config_warnings(warnings: &[ConfigWarning], mirror_stderr: bool) {
    if !mirror_stderr {
        return;
    }
    for warning in warnings {
        let severity = match warning.severity {
            ConfigWarningSeverity::Critical => "critical",
            ConfigWarningSeverity::Warn => "warn",
            ConfigWarningSeverity::Info => "info",
        };
        tracing::warn!(
            "config warning [{}:{}] {}",
            severity,
            warning.code,
            warning.message
        );
    }
}

pub(crate) fn print_runtime_status(
    config_path: &std::path::Path,
    report: &RuntimeHealthcheckReport,
) {
    println!("{}", tf("status.title", &[]));
    println!(
        "{}",
        tf(
            "status.config_path",
            &[("path", &config_path.to_string_lossy())]
        )
    );
    println!(
        "{}",
        tf(
            "status.overall",
            &[("status", &format!("{:?}", report.overall_status))]
        )
    );

    let provider_component = report
        .components
        .iter()
        .find(|component| component.name == "provider_dependencies");

    let Some(component) = provider_component else {
        println!("{}", tf("status.no_provider_component", &[]));
        println!("{}", tf("status.done", &[]));
        return;
    };

    let configured_agents = component
        .details
        .get("total")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    println!(
        "{}",
        tf(
            "status.configured_agents",
            &[("count", &configured_agents.to_string())]
        )
    );

    let details = component
        .details
        .get("agents")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    for item in details {
        let name = item
            .get("agent")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let agent_type = item
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let ready = item
            .get("ready")
            .and_then(|value| value.as_bool())
            .map(|value| value.to_string())
            .unwrap_or_else(|| "false".to_string());
        let endpoint_status = item
            .get("endpoint_status")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let missing_envs = item
            .get("missing_envs")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|entry| entry.as_str())
                    .collect::<Vec<&str>>()
            })
            .unwrap_or_default();
        let missing_envs = if missing_envs.is_empty() {
            "-".to_string()
        } else {
            missing_envs.join("|")
        };

        println!(
            "{}",
            tf(
                "status.agent_line",
                &[
                    ("name", name),
                    ("type", agent_type),
                    ("ready", &ready),
                    ("endpoint_status", endpoint_status),
                    ("missing_envs", &missing_envs),
                ]
            )
        );

        for (label, key_status) in [
            ("api_keys", item.get("api_key_status")),
            ("secret_keys", item.get("secret_key_status")),
        ] {
            let Some(key_status) = key_status else {
                continue;
            };
            if key_status.is_null() {
                continue;
            }

            let secret_ref = key_status
                .get("ref")
                .and_then(|value| value.as_str())
                .unwrap_or("-");
            let count = key_status
                .get("count")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            let fingerprints = key_status
                .get("fingerprints")
                .and_then(|value| value.as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|entry| entry.as_str())
                        .collect::<Vec<&str>>()
                })
                .unwrap_or_default();
            let fingerprints = if fingerprints.is_empty() {
                "-".to_string()
            } else {
                fingerprints.join(" | ")
            };

            println!(
                "{}",
                tf(
                    "status.secret_line",
                    &[
                        ("label", label),
                        ("count", &count.to_string()),
                        ("secret_ref", secret_ref),
                        ("fingerprints", &fingerprints),
                    ]
                )
            );
        }
    }

    println!("{}", tf("status.done", &[]));
}

#[derive(Default)]
pub(crate) struct StatusCompleteness {
    pub score: u32,
    pub missing: Vec<String>,
    pub recommended: Vec<StatusRecommendation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecommendationLevel {
    Warning,
    Info,
}

#[derive(Clone, Debug)]
pub(crate) struct StatusRecommendation {
    pub level: RecommendationLevel,
    pub message: String,
}

impl StatusCompleteness {
    fn push_warning(&mut self, message: String) {
        self.recommended.push(StatusRecommendation {
            level: RecommendationLevel::Warning,
            message,
        });
    }

    fn push_info(&mut self, message: String) {
        self.recommended.push(StatusRecommendation {
            level: RecommendationLevel::Info,
            message,
        });
    }
}

fn inflight_recommendation(
    field: &str,
    expected: i64,
    current: i64,
) -> (RecommendationLevel, String) {
    let delta = (expected - current).abs();
    let ratio = if expected > 0 {
        delta as f64 / expected as f64
    } else {
        0.0
    };
    let level = if ratio >= 0.25 {
        RecommendationLevel::Warning
    } else {
        RecommendationLevel::Info
    };
    (
        level,
        format!(
            "{} recommended={}, current={} (delta={:.0}%)",
            field,
            expected,
            current,
            ratio * 100.0
        ),
    )
}

pub(crate) fn build_completeness_report(
    config: &AppConfig,
    report: &RuntimeHealthcheckReport,
) -> StatusCompleteness {
    let mut out = StatusCompleteness::default();
    let mut score = 0.0_f64;
    let provider_recommendation = recommendation_snapshot_for_config(config);
    // Fallback thresholds when no provider is configured: reuse the shared
    // `ProviderRecommendations::default` projection (config_gen.rs) so the
    // report thresholds cannot drift from the values used to generate a
    // fresh config. All fallback literals below were previously duplicated
    // from that default — behavior is unchanged.
    let fallback = default_recommendation_snapshot();

    let provider = report
        .components
        .iter()
        .find(|component| component.name == "provider_dependencies");

    let (ready, total) = provider
        .map(|component| {
            let ready = component
                .details
                .get("ready")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            let total = component
                .details
                .get("total")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            (ready, total)
        })
        .unwrap_or((0, 0));

    if total > 0 {
        score += 55.0 * (ready as f64 / total as f64);
        if ready < total {
            out.missing
                .push("provider credentials or endpoint readiness incomplete".to_string());
        }
    }

    for kind in ["planning", "coding", "review", "delivery"] {
        // Resolve the config's actual phase name for this semantic kind
        // (accepts both the canonical vocabulary and the shipped template's
        // think/act/check/done). Previously the loop hard-coded the canonical
        // names, so the official default config was reported as missing
        // review/delivery phases.
        let Some(phase_name) = crate::config::phase_name_for_kind(config, kind) else {
            out.missing.push(format!("{kind} phase missing"));
            continue;
        };

        let expected = provider_recommendation
            .as_ref()
            .map(|item| match kind {
                "planning" => item.planning_request_timeout_seconds,
                "coding" => item.coding_request_timeout_seconds,
                "review" => item.review_request_timeout_seconds,
                _ => item.delivery_request_timeout_seconds,
            })
            .unwrap_or(match kind {
                "planning" => fallback.planning_request_timeout_seconds,
                "coding" => fallback.coding_request_timeout_seconds,
                "review" => fallback.review_request_timeout_seconds,
                _ => fallback.delivery_request_timeout_seconds,
            });

        let actual = config
            .phases
            .get(phase_name)
            .and_then(|phase| phase.options.as_ref())
            .and_then(|options| options.request_timeout_seconds);

        match actual {
            Some(timeout) if timeout == expected => score += 2.5,
            Some(timeout) => {
                score += 1.5;
                out.push_info(format!(
                    "phases.{}.options.request_timeout_seconds recommended={}, current={}",
                    phase_name, expected, timeout
                ));
            }
            None => out.missing.push(format!(
                "phases.{}.options.request_timeout_seconds",
                phase_name
            )),
        }
    }

    // The coding-phase checks below resolve the actual phase name so they work
    // with both the canonical and template (think/act/check/done) vocabularies.
    let coding_phase = crate::config::phase_name_for_kind(config, "coding");
    let expected_review_timeout = provider_recommendation
        .as_ref()
        .map(|item| item.coding_review_timeout_seconds)
        .unwrap_or(fallback.coding_review_timeout_seconds);
    let actual_review_timeout = coding_phase
        .and_then(|phase_name| config.phases.get(phase_name))
        .and_then(|phase| phase.options.as_ref())
        .and_then(|options| options.review_timeout_seconds);
    match actual_review_timeout {
        Some(timeout) if timeout == expected_review_timeout => score += 5.0,
        Some(timeout) => {
            score += 2.5;
            out.push_info(format!(
                "phases.{}.options.review_timeout_seconds recommended={}, current={}",
                coding_phase.unwrap_or("coding"),
                expected_review_timeout,
                timeout
            ));
        }
        None => out
            .missing
            .push("phases.coding.options.review_timeout_seconds".to_string()),
    }

    let expected_phase_inflight = provider_recommendation
        .as_ref()
        .map(|item| item.phase_max_inflight as i64)
        .unwrap_or(fallback.phase_max_inflight as i64);
    let expected_global_inflight = provider_recommendation
        .as_ref()
        .map(|item| item.global_max_inflight as i64)
        .unwrap_or(fallback.global_max_inflight as i64);
    let coding_options = coding_phase
        .and_then(|phase_name| config.phases.get(phase_name))
        .and_then(|phase| phase.options.as_ref());
    let actual_phase_inflight = coding_options.and_then(|options| {
        options
            .extra
            .get("phase_max_inflight")
            .and_then(|value| value.as_i64())
    });
    let actual_global_inflight = coding_options.and_then(|options| {
        options
            .extra
            .get("global_max_inflight")
            .and_then(|value| value.as_i64())
    });

    match actual_phase_inflight {
        Some(value) if value == expected_phase_inflight => score += 2.5,
        Some(value) => {
            score += 1.5;
            let (level, message) = inflight_recommendation(
                "phases.coding.options.phase_max_inflight",
                expected_phase_inflight,
                value,
            );
            match level {
                RecommendationLevel::Warning => out.push_warning(message),
                RecommendationLevel::Info => out.push_info(message),
            }
        }
        None => out
            .missing
            .push("phases.coding.options.phase_max_inflight".to_string()),
    }

    match actual_global_inflight {
        Some(value) if value == expected_global_inflight => score += 2.5,
        Some(value) => {
            score += 1.5;
            let (level, message) = inflight_recommendation(
                "phases.coding.options.global_max_inflight",
                expected_global_inflight,
                value,
            );
            match level {
                RecommendationLevel::Warning => out.push_warning(message),
                RecommendationLevel::Info => out.push_info(message),
            }
        }
        None => out
            .missing
            .push("phases.coding.options.global_max_inflight".to_string()),
    }

    let recommended_cache = provider_recommendation
        .as_ref()
        .map(|item| item.cache_enabled)
        .unwrap_or(fallback.cache_enabled);
    let cache_enabled = config
        .cache
        .as_ref()
        .map(|cache| cache.enabled)
        .unwrap_or(false);
    if cache_enabled == recommended_cache {
        score += 5.0;
    } else {
        out.push_info(format!("cache.enabled={} recommended", recommended_cache));
    }

    let recommended_vector = provider_recommendation
        .as_ref()
        .map(|item| item.vector_enabled)
        .unwrap_or(fallback.vector_enabled);
    let vector_enabled = config
        .vector
        .as_ref()
        .map(|vector| vector.enabled)
        .unwrap_or(false);
    if vector_enabled {
        if vector_enabled == recommended_vector {
            score += 5.0;
        } else {
            out.push_info(format!("vector.enabled={} recommended", recommended_vector));
        }
    } else {
        if !recommended_vector {
            score += 5.0;
        } else {
            out.push_info(format!("vector.enabled={} recommended", recommended_vector));
        }
    }

    if crate::config::phase_name_for_kind(config, "review").is_some() {
        score += 5.0;
    } else {
        out.missing.push("review phase missing".to_string());
    }

    if crate::config::phase_name_for_kind(config, "delivery").is_some() {
        score += 5.0;
    } else {
        out.missing.push("delivery phase missing".to_string());
    }

    if config
        .runtime
        .as_ref()
        .map(|runtime| runtime.health_interval_seconds > 0)
        .unwrap_or(false)
    {
        score += 5.0;
    } else {
        out.missing
            .push("runtime.health_interval_seconds missing".to_string());
    }

    if config
        .runtime
        .as_ref()
        .map(|runtime| runtime.maintenance_interval_seconds > 0)
        .unwrap_or(false)
    {
        score += 5.0;
    } else {
        out.missing
            .push("runtime.maintenance_interval_seconds missing".to_string());
    }

    out.score = score.round().clamp(0.0, 100.0) as u64 as u32;
    out
}

pub(crate) fn print_completeness_report(config: &AppConfig, report: &RuntimeHealthcheckReport) {
    let completeness = build_completeness_report(config, report);
    println!(
        "{}",
        tf(
            "status.completeness",
            &[("score", &completeness.score.to_string())]
        )
    );

    if completeness.missing.is_empty() {
        println!("{}", tf("status.missing_none", &[]));
    } else {
        println!("{}", tf("status.missing_title", &[]));
        for item in completeness.missing {
            println!("- {}", item);
        }
    }

    if !completeness.recommended.is_empty() {
        println!("{}", tf("status.recommended_title", &[]));
        for item in &completeness.recommended {
            let level = match item.level {
                RecommendationLevel::Warning => "warning",
                RecommendationLevel::Info => "info",
            };
            println!(
                "{}",
                tf(
                    "status.recommended_item",
                    &[("level", level), ("message", &item.message)]
                )
            );
        }
    }
}
