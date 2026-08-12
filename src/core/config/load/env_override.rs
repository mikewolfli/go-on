//! Secret validation, environment-readiness, and runtime health checks.
//!
//! Historical name (`env_override`) predates the current contents: this module
//! performs keyring/secret-reference validation, per-agent environment
//! readiness probing, production-strict violation collection, and runtime
//! readiness health reports. It does not implement an "environment variable
//! overrides config file" layer — config fields are loaded from the TOML file
//! (see `parser.rs`); only secrets fall back to environment variables via
//! `shared::secret_override`.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use tracing::warn;

use crate::agent::inspect_secret_pool;
use crate::i18n::runtime::tf;

use super::super::defaults;
use super::super::types::{AgentConfig, AppConfig};
use super::parser::{ConfigHealthReport, ConfigWarning, ConfigWarningSeverity};
use super::validator::phase_uses_complex_autopilot;

/// Cache `max_entries` below this value is reported as "very low".
///
/// Single source of truth shared with `core::config_validation` so the
/// report engine and the health-warning engine cannot drift apart.
pub(crate) const CACHE_MAX_ENTRIES_LOW: usize = 100;

/// Cache `max_entries` at or above this value is reported as "unusually high".
///
/// Single source of truth shared with `core::config_validation`.
pub(crate) const CACHE_MAX_ENTRIES_HIGH: usize = 50_000;

/// Check for missing environment variables across all agents
pub fn missing_env_vars(config: &AppConfig) -> Vec<String> {
    let mut missing = Vec::new();

    for agent in config.agents().values() {
        for secret_ref in required_env_vars(agent) {
            if probe_secret_ref(&secret_ref) {
                continue;
            }
            missing.push(secret_ref);
        }
    }

    missing.sort();
    missing.dedup();
    missing
}

/// Check if a specific agent's environment is ready
pub fn is_agent_env_ready(config: &AppConfig, agent_name: &str) -> bool {
    let Some(agent) = config.agents().get(agent_name) else {
        return false;
    };
    required_env_vars(agent)
        .into_iter()
        .all(|secret_ref| probe_secret_ref(&secret_ref))
}

/// `missing_env_vars_by_agent` probe body (see the wrapper below for the
/// blocking-thread rationale).
fn probe_missing_env_vars_by_agent(config: &AppConfig) -> HashMap<String, Vec<String>> {
    let mut missing = HashMap::new();

    for (agent_name, agent) in config.agents() {
        let mut per_agent_missing = required_env_vars(agent)
            .into_iter()
            .filter(|secret_ref| !probe_secret_ref(secret_ref))
            .collect::<Vec<_>>();

        if !per_agent_missing.is_empty() {
            per_agent_missing.sort();
            per_agent_missing.dedup();
            missing.insert(agent_name.clone(), per_agent_missing);
        }
    }

    missing
}

/// True when any configured agent references the system keyring.
fn has_keyring_refs(config: &AppConfig) -> bool {
    config.agents().values().any(|agent| {
        required_env_vars(agent)
            .iter()
            .any(|secret_ref| is_keyring_ref(secret_ref))
    })
}

pub(crate) fn missing_env_vars_by_agent(config: &AppConfig) -> HashMap<String, Vec<String>> {
    // Keyring lookups (keyring::Entry::get_password inside
    // agents::inspect_secret_pool) are blocking I/O. The callers of this
    // probe run on tokio workers (startup validation, config reload, health
    // checks), where a slow D-Bus/Keychain round-trip stalls the runtime. Run
    // the whole probe on a dedicated OS thread whenever any agent references
    // the keyring; env-only configs probe inline (env reads are cheap and
    // non-blocking). This keeps agents' timeout protection intact — the probe
    // still goes through `inspect_secret_pool`, which guards keychain access
    // with its own 5s timeout.
    if !has_keyring_refs(config) {
        return probe_missing_env_vars_by_agent(config);
    }
    std::thread::scope(|scope| {
        scope
            .spawn(|| probe_missing_env_vars_by_agent(config))
            .join()
            .unwrap_or_default()
    })
}

/// Probe whether a secret ref resolves to a non-empty value.
///
/// Returns `true` when the secret is available (keyring or env fallback), so
/// callers treat a `false` result as "missing".
fn probe_secret_ref(secret_ref: &str) -> bool {
    inspect_secret_pool(secret_ref, secret_ref).is_ok()
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
    value.starts_with(crate::shared::keyring_ref::KEYRING_PREFIX)
}

// ── Keyring / secret validation ───────────────────────────────────────────

pub fn validate_external_secret_refs(config: &AppConfig) -> Result<()> {
    // Keyring reads (`keyring::Entry::get_password` inside `validate_secret_ref`)
    // are blocking I/O with slow D-Bus/Keychain round-trips. The callers of
    // this validation run on tokio workers (startup validation, config reload,
    // health checks), so run the whole validation on a dedicated OS thread
    // whenever any agent references the keyring; env-only configs validate
    // inline (env reads are cheap and non-blocking). Same pattern as
    // `missing_env_vars_by_agent`.
    if !has_keyring_refs(config) {
        return validate_external_secret_refs_inner(config);
    }
    std::thread::scope(|scope| {
        scope
            .spawn(|| validate_external_secret_refs_inner(config))
            .join()
            .unwrap_or_else(|_| anyhow::bail!("external secret validation thread panicked"))
    })
}

/// `validate_external_secret_refs` probe body (see the wrapper above for the
/// blocking-thread rationale).
fn validate_external_secret_refs_inner(config: &AppConfig) -> Result<()> {
    for (agent_name, agent) in config.agents() {
        if let Some(value) = agent.api_key_env.as_deref() {
            validate_secret_ref(value, &format!("agents.{}.api_key_env", agent_name))?;
        }
        if let Some(value) = agent.secret_key_env.as_deref() {
            validate_secret_ref(value, &format!("agents.{}.secret_key_env", agent_name))?;
        }
    }
    Ok(())
}

pub(crate) fn validate_secret_ref(value: &str, field_name: &str) -> Result<()> {
    if !is_keyring_ref(value) {
        return Ok(());
    }

    let locator = value
        .strip_prefix(crate::shared::keyring_ref::KEYRING_PREFIX)
        .ok_or_else(|| anyhow::anyhow!("invalid keyring ref for {}", field_name))?;
    let (service, account) = locator.split_once('/').ok_or_else(|| {
        anyhow::anyhow!(
            "invalid {} keyring reference '{}': expected keyring://<service>/<account>",
            field_name,
            value
        )
    })?;
    let mut secret = String::new();
    for (service_name, account_name) in crate::agent::keyring_lookup_accounts(service, account) {
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
        let fallback_candidates = crate::agent::keyring_env_fallback_candidates(service, account);
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

    // Validate secret key security (shared implementation).
    crate::shared::secret_override::validate_secret_security(
        &secret,
        field_name,
        "error.missing_field",
    )?;

    Ok(())
}

// ── Production strict checks ──────────────────────────────────────────────

pub fn collect_production_strict_violations(config: &AppConfig) -> Vec<String> {
    let mut violations = Vec::new();

    for (agent_name, agent) in config.agents() {
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

        let total_agents = config.agents().len();
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

    // NOTE: the default user_auth_token_secret warning is intentionally NOT
    // duplicated here — `core::config_validation::ConfigValidator` (the single
    // report engine) already emits it once (runtime.user_auth section) and it
    // is logged at startup via the validation-warnings block in
    // `handle_validation_mode`. Duplicating it here warned twice per startup.

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
        if cache.enabled && cache.max_entries >= CACHE_MAX_ENTRIES_HIGH {
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
                severity: ConfigWarningSeverity::Warn,
                message: "runtime.otel_enabled=true but otel_endpoint is not set; OTLP traces will NOT be exported unless OTEL_EXPORTER_OTLP_ENDPOINT is set".to_string(),
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
    // Map the warning/critical posture onto the REAL build profiles
    // (local / simple-server / multi-users-server / full). The previous
    // "minimal"/"balanced"/"full" labels referenced profiles that do not
    // exist as Cargo features, and the "full" copytext pointed at a
    // non-shipped config.toml.autopilot-adaptive template.
    let profile = if critical_count > 0 {
        "full"
    } else if warn_count >= 3 {
        "multi-users-server"
    } else if warn_count == 0 {
        "local"
    } else {
        "simple-server"
    }
    .to_string();

    let mut recommendations = Vec::new();
    recommendations.push(match profile.as_str() {
        "full" => {
            "full profile: keep review gates and safeguard defaults enabled and address the critical warnings before production".to_string()
        }
        "multi-users-server" => {
            "multi-users-server profile: several warnings present — keep review/safeguard defaults and re-check the flagged warnings".to_string()
        }
        "local" => {
            "local profile: config quality is stable; keep minimal defaults unless the workload changes".to_string()
        }
        _ => {
            "simple-server profile: keep key safeguards while avoiding high-cost optional toggles".to_string()
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
