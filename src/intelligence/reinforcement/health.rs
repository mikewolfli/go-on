//! Healthcheck module — runtime health probes and component reports.
//!
//! Extracted from the original monolithic `reinforcement.rs`.

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cache::ResponseCache;
use crate::config::{validate_runtime_readiness, AppConfig};
use crate::vector::VectorStore;

use super::ArtifactLedger;

/// Short-TTL cache for healthcheck reports. The report probes keyring
/// (per-agent D-Bus round-trips), SQLite/vector counts, and the config file;
/// several status endpoints (health.probes, runtime.stability, runtime.self_model,
/// release.readiness) build it on every request, and release.readiness alone
/// used to build it twice within a single request. A 2s TTL keeps probes fresh
/// for operators while collapsing duplicate builds.
type HealthcheckCacheEntry = (Instant, Result<RuntimeHealthcheckReport, String>);
static HEALTHCHECK_CACHE: OnceLock<Mutex<Option<HealthcheckCacheEntry>>> = OnceLock::new();
const HEALTHCHECK_TTL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Healthy,
    Warn,
    Error,
    Skipped,
}

impl CheckStatus {
    fn severity(self) -> u8 {
        match self {
            Self::Healthy => 0,
            Self::Skipped => 0,
            Self::Warn => 1,
            Self::Error => 2,
        }
    }

    pub fn merge(self, other: Self) -> Self {
        if self.severity() >= other.severity() {
            self
        } else {
            other
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentReport {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeHealthcheckReport {
    pub generated_at: i64,
    pub overall_status: CheckStatus,
    pub components: Vec<ComponentReport>,
}

/// Aggregate an iterator of statuses into a single overall status.
pub fn aggregate_status<I>(statuses: I) -> CheckStatus
where
    I: IntoIterator<Item = CheckStatus>,
{
    statuses
        .into_iter()
        .fold(CheckStatus::Healthy, |acc, status| acc.merge(status))
}

/// Build a comprehensive runtime healthcheck report, with a short TTL cache
/// so multiple status endpoints sharing one request do not re-probe keyring /
/// SQLite / vector counters (each probe is a D-Bus or disk round-trip).
pub async fn build_runtime_healthcheck_report(
    config_path: Option<&Path>,
    cache: Option<&ResponseCache>,
    vector_store: Option<&VectorStore>,
) -> Result<RuntimeHealthcheckReport> {
    if let Some((stored_at, stored)) = HEALTHCHECK_CACHE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
    {
        if stored_at.elapsed() < HEALTHCHECK_TTL {
            return match stored {
                Ok(report) => Ok(report),
                Err(err) => anyhow::bail!("{err}"),
            };
        }
    }

    let result =
        build_runtime_healthcheck_report_inner(config_path, None, cache, vector_store).await;
    let cached = result
        .as_ref()
        .map(|r| r.clone())
        .map_err(|e| e.to_string());
    if let Ok(mut guard) = HEALTHCHECK_CACHE.get_or_init(|| Mutex::new(None)).lock() {
        *guard = Some((Instant::now(), cached));
    }
    result
}

/// Build a runtime healthcheck report reusing an already-loaded config.
///
/// One-shot CLI paths (e.g. `go-on --status`) have already parsed the config
/// via `load_uncached`; passing it in avoids a second disk load + re-parse
/// (plus a fresh mtime-cache write) purely to build the report.
pub async fn build_runtime_healthcheck_report_with_config(
    config_path: &Path,
    config: &AppConfig,
    cache: Option<&ResponseCache>,
    vector_store: Option<&VectorStore>,
) -> Result<RuntimeHealthcheckReport> {
    build_runtime_healthcheck_report_inner(Some(config_path), Some(config), cache, vector_store)
        .await
}

async fn build_runtime_healthcheck_report_inner(
    config_path: Option<&Path>,
    loaded_config: Option<&AppConfig>,
    cache: Option<&ResponseCache>,
    vector_store: Option<&VectorStore>,
) -> Result<RuntimeHealthcheckReport> {
    let ledger = ArtifactLedger::new(config_path);
    let mut components = Vec::new();

    match ledger.ensure_ready() {
        Ok(()) => components.push(ComponentReport {
            name: "ledger".to_string(),
            status: CheckStatus::Healthy,
            message: "durable ledger is writable".to_string(),
            details: json!({ "root": ledger.root().display().to_string() }),
        }),
        Err(err) => components.push(ComponentReport {
            name: "ledger".to_string(),
            status: CheckStatus::Error,
            message: err.to_string(),
            details: json!({ "root": ledger.root().display().to_string() }),
        }),
    }

    // Config component: reuse the caller's pre-loaded config when available
    // (e.g. `go-on --status` already parsed it via `load_uncached`), otherwise
    // load from disk here.
    match loaded_config {
        Some(config) => match config_path {
            Some(path) => push_config_components(path, config, &mut components).await,
            None => components.push(ComponentReport {
                name: "config".to_string(),
                status: CheckStatus::Skipped,
                message: "config path unavailable".to_string(),
                details: json!({}),
            }),
        },
        None => match config_path {
            Some(path) => match AppConfig::load(path) {
                Ok(config) => push_config_components(path, &config, &mut components).await,
                Err(err) => components.push(ComponentReport {
                    name: "config".to_string(),
                    status: CheckStatus::Error,
                    message: err.to_string(),
                    details: json!({ "config_path": path.display().to_string() }),
                }),
            },
            None => components.push(ComponentReport {
                name: "config".to_string(),
                status: CheckStatus::Skipped,
                message: "config path unavailable".to_string(),
                details: json!({}),
            }),
        },
    }

    // Cache + vector probes run concurrently — both are disk/SQLite round-trips —
    // and are pushed in their canonical order (cache first, then vector).
    let cache_probe = async {
        match cache {
            Some(cache) => match cache.entry_count().await {
                Ok(entries) => ComponentReport {
                    name: "cache".to_string(),
                    status: CheckStatus::Healthy,
                    message: format!("sqlite cache reachable with {} entries", entries),
                    details: json!({ "entries": entries }),
                },
                Err(err) => ComponentReport {
                    name: "cache".to_string(),
                    status: CheckStatus::Error,
                    message: err.to_string(),
                    details: json!({}),
                },
            },
            None => ComponentReport {
                name: "cache".to_string(),
                status: CheckStatus::Skipped,
                message: "cache disabled".to_string(),
                details: json!({}),
            },
        }
    };
    let vector_probe = async {
        match vector_store {
            Some(vector_store) => match (
                vector_store.memory_entry_count().await,
                vector_store.summary_entry_count().await,
            ) {
                (Ok(memory_entries), Ok(summary_entries)) => ComponentReport {
                    name: "vector".to_string(),
                    status: CheckStatus::Healthy,
                    message: format!(
                        "vector store reachable with {} memory entries and {} summaries",
                        memory_entries, summary_entries
                    ),
                    details: json!({
                        "memory_entries": memory_entries,
                        "summary_entries": summary_entries,
                    }),
                },
                (Err(err), _) | (_, Err(err)) => ComponentReport {
                    name: "vector".to_string(),
                    status: CheckStatus::Error,
                    message: err.to_string(),
                    details: json!({}),
                },
            },
            None => ComponentReport {
                name: "vector".to_string(),
                status: CheckStatus::Skipped,
                message: "vector store disabled".to_string(),
                details: json!({}),
            },
        }
    };
    let (cache_component, vector_component) = tokio::join!(cache_probe, vector_probe);
    components.push(cache_component);
    components.push(vector_component);

    let overall_status = aggregate_status(components.iter().map(|component| component.status));
    Ok(RuntimeHealthcheckReport {
        generated_at: now_ts(),
        overall_status,
        components,
    })
}

/// Build the `config`, `provider_dependencies`, `secret_pool`, and `agent_env`
/// components for a given config. `config_path` is required for the readiness
/// report; callers that pass a pre-loaded config always have the path.
async fn push_config_components(
    config_path: &Path,
    config: &AppConfig,
    components: &mut Vec<ComponentReport>,
) {
    match validate_runtime_readiness(config_path, config) {
        Ok(report) => {
            let status = if report.critical_count > 0 {
                CheckStatus::Error
            } else if report.warn_count > 0 || report.info_count > 0 {
                CheckStatus::Warn
            } else {
                CheckStatus::Healthy
            };
            components.push(ComponentReport {
                name: "config".to_string(),
                status,
                message: format!(
                    "config score {}/100, profile {}",
                    report.score, report.profile_recommendation
                ),
                details: serde_json::to_value(&report).unwrap_or_else(|_| json!({})),
            });
            components.push(build_provider_dependency_component(config).await);

            // Secret pool diagnostics — shows which secrets are resolved.
            let secret_details = secret_pool_status(config).await;
            components.push(ComponentReport {
                name: "secret_pool".to_string(),
                status: CheckStatus::Healthy,
                message: "secret pool status".to_string(),
                details: secret_details,
            });

            // Missing environment variables for agents.
            let missing = missing_envs_for_agent(config);
            if missing.is_empty() {
                components.push(ComponentReport {
                    name: "agent_env".to_string(),
                    status: CheckStatus::Healthy,
                    message: "all agent environment variables are set".to_string(),
                    details: json!({}),
                });
            } else {
                components.push(ComponentReport {
                    name: "agent_env".to_string(),
                    status: CheckStatus::Warn,
                    message: format!(
                        "{} agent(s) have missing environment variables",
                        missing.len()
                    ),
                    details: json!(missing),
                });
            }
        }
        Err(err) => components.push(ComponentReport {
            name: "config".to_string(),
            status: CheckStatus::Error,
            message: err.to_string(),
            details: json!({ "config_path": config_path.display().to_string() }),
        }),
    }
}

/// Persist a healthcheck report to the artifact ledger.
pub fn persist_runtime_healthcheck(
    ledger: &ArtifactLedger,
    report: &RuntimeHealthcheckReport,
) -> Result<std::path::PathBuf> {
    ledger.write_json("qa", "latest-healthcheck.json", report)
}

// ── Internal helpers ──────────────────────────────────────────────────────

async fn build_provider_dependency_component(config: &AppConfig) -> ComponentReport {
    let mut status = CheckStatus::Healthy;
    let mut message = String::from("provider dependencies:");
    let mut agents = Vec::new();
    let mut ready_count: u64 = 0;
    let mut degraded_count: u64 = 0;
    let mut total_count: u64 = 0;

    // Probe every agent's secrets concurrently: each probe may hit the keyring
    // (D-Bus/Keychain round-trip, cached with a 30s TTL), so a serial loop
    // stalls the whole healthcheck on slow keychains. `join_all` preserves the
    // config iteration order in the report.
    let mut probes = Vec::new();
    for (agent_key, agent_config) in config.agents() {
        let env_var = agent_config.api_key_env.clone().unwrap_or_default();
        let secret_env_var = agent_config.secret_key_env.clone();
        let agent_name = agent_key;

        if env_var.is_empty() && secret_env_var.as_deref().is_none_or(|s| s.is_empty()) {
            continue;
        }

        total_count += 1;

        // Try keyring first, fall back to env var (same chain as load_secret_value)
        let api_env = env_var.clone();
        let api_probe = async move { secret_ref_ready_async(&api_env).await };
        let secret_probe = secret_probe(secret_env_var.clone().unwrap_or_default());
        probes.push(async move {
            (
                agent_name.clone(),
                env_var,
                secret_env_var,
                api_probe.await,
                secret_probe.await,
            )
        });
    }

    for (agent_name, env_var, secret_env_var, api_ready, secret_ready) in join_all(probes).await {
        let is_ready = api_ready && secret_ready;

        if is_ready {
            ready_count += 1;
        } else {
            degraded_count += 1;
            status = CheckStatus::Warn;
        }

        agents.push(json!({
            "name": agent_name,
            "env_var": env_var,
            "secret_env_var": secret_env_var,
            "api_ready": api_ready,
            "secret_ready": secret_ready,
            "ready": is_ready,
        }));
    }

    if total_count == 0 {
        status = CheckStatus::Skipped;
        message.push_str(" no agents configured");
    } else if ready_count == total_count {
        message.push_str(&format!(" {} of {} ready", ready_count, total_count));
    } else {
        message.push_str(&format!(
            " {} of {} ready ({} missing)",
            ready_count,
            total_count,
            total_count - ready_count
        ));
    }

    ComponentReport {
        name: "provider_dependencies".to_string(),
        status,
        message,
        details: json!({
            "ready": ready_count,
            "degraded": degraded_count,
            "total": total_count,
            "agents": agents,
        }),
    }
}

async fn secret_ref_ready_async(secret_ref: &str) -> bool {
    // Use get_secret() which checks in-memory override map first, then env vars.
    // This ensures API keys set via GUI/CLI secret overrides are properly detected.
    let keyring_prefix = crate::shared::keyring_ref::KEYRING_PREFIX;
    if secret_ref.starts_with(keyring_prefix) {
        let locator = secret_ref.trim_start_matches(keyring_prefix);
        if let Some((service, account)) = locator.split_once('/') {
            for (service_name, account_name) in
                crate::agent::keyring_lookup_accounts(service, account)
            {
                // Async wrapper: keyring reads are blocking I/O (D-Bus /
                // Keychain) and must not run on a tokio worker. The shared
                // cache (30s TTL) also short-circuits repeated probes.
                if crate::shared::secret_override::get_keyring_cached_async(
                    &service_name,
                    &account_name,
                )
                .await
                .is_some_and(|v| !v.trim().is_empty())
                {
                    return true;
                }
            }

            // Also check env var fallback via get_secret() for in-memory overrides
            return crate::agent::keyring_env_fallback_candidates(service, account)
                .into_iter()
                .any(|var| {
                    crate::shared::secret_override::get_secret(&var)
                        .is_some_and(|v| !v.trim().is_empty())
                });
        }

        return false;
    }

    // Direct secret ref: check override map + env var
    crate::shared::secret_override::get_secret(secret_ref).is_some_and(|v| !v.trim().is_empty())
}

/// Probe whether a secret ref is ready; an empty ref is trivially ready
/// (no secret configured). Owned input so callers can spawn all probes
/// concurrently without borrowing the config.
async fn secret_probe(secret_ref: String) -> bool {
    if secret_ref.is_empty() {
        return true;
    }
    secret_ref_ready_async(&secret_ref).await
}

async fn secret_pool_status(config: &AppConfig) -> Value {
    let mut secrets = Vec::new();
    for agent_config in config.agents().values() {
        if let Some(env_var) = &agent_config.api_key_env {
            // Keyring probes are blocking I/O (5s-timeout keychain reads), so
            // resolution goes through the async + cached path in
            // `secret_ref_ready_async` instead of `inspect_secret_pool`.
            let exists = secret_ref_ready_async(env_var).await;
            secrets.push(json!({
                "secret_name": env_var,
                "resolved": exists,
            }));
        }
    }
    json!(secrets)
}

fn missing_envs_for_agent(config: &AppConfig) -> Vec<Value> {
    let mut missing = Vec::new();
    for agent_config in config.agents().values() {
        if let Some(api_key_env) = &agent_config.api_key_env {
            if api_key_env.starts_with("env://") {
                let env_var = api_key_env.trim_start_matches("env://");
                if std::env::var(env_var).is_err() {
                    missing.push(json!({
                        "agent": agent_config.agent_type,
                        "expected_env": env_var,
                        "hint": "set the environment variable or use keyring://"
                    }));
                }
            }
        }
    }
    missing
}

pub fn now_ts() -> i64 {
    crate::shared::timestamps::now_ts()
}
