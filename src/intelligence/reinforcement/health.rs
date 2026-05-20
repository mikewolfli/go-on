//! Healthcheck module — runtime health probes and component reports.
//!
//! Extracted from the original monolithic `reinforcement.rs`.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cache::ResponseCache;
use crate::config::{validate_runtime_readiness, AppConfig};
use crate::vector::VectorStore;

use super::ArtifactLedger;

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

/// Build a comprehensive runtime healthcheck report.
pub fn build_runtime_healthcheck_report(
    config_path: Option<&Path>,
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

    if let Some(path) = config_path {
        match AppConfig::load(path) {
            Ok(config) => match validate_runtime_readiness(path, &config) {
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
                    components.push(build_provider_dependency_component(&config));
                }
                Err(err) => components.push(ComponentReport {
                    name: "config".to_string(),
                    status: CheckStatus::Error,
                    message: err.to_string(),
                    details: json!({ "config_path": path.display().to_string() }),
                }),
            },
            Err(err) => components.push(ComponentReport {
                name: "config".to_string(),
                status: CheckStatus::Error,
                message: err.to_string(),
                details: json!({ "config_path": path.display().to_string() }),
            }),
        }
    } else {
        components.push(ComponentReport {
            name: "config".to_string(),
            status: CheckStatus::Skipped,
            message: "config path unavailable".to_string(),
            details: json!({}),
        });
    }

    if let Some(cache) = cache {
        match cache.entry_count() {
            Ok(entries) => components.push(ComponentReport {
                name: "cache".to_string(),
                status: CheckStatus::Healthy,
                message: format!("sqlite cache reachable with {} entries", entries),
                details: json!({ "entries": entries }),
            }),
            Err(err) => components.push(ComponentReport {
                name: "cache".to_string(),
                status: CheckStatus::Error,
                message: err.to_string(),
                details: json!({}),
            }),
        }
    } else {
        components.push(ComponentReport {
            name: "cache".to_string(),
            status: CheckStatus::Skipped,
            message: "cache disabled".to_string(),
            details: json!({}),
        });
    }

    if let Some(vector_store) = vector_store {
        match (
            vector_store.memory_entry_count(),
            vector_store.summary_entry_count(),
        ) {
            (Ok(memory_entries), Ok(summary_entries)) => components.push(ComponentReport {
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
            }),
            (Err(err), _) | (_, Err(err)) => components.push(ComponentReport {
                name: "vector".to_string(),
                status: CheckStatus::Error,
                message: err.to_string(),
                details: json!({}),
            }),
        }
    } else {
        components.push(ComponentReport {
            name: "vector".to_string(),
            status: CheckStatus::Skipped,
            message: "vector store disabled".to_string(),
            details: json!({}),
        });
    }

    let overall_status = aggregate_status(components.iter().map(|component| component.status));
    Ok(RuntimeHealthcheckReport {
        generated_at: now_ts(),
        overall_status,
        components,
    })
}

/// Persist a healthcheck report to the artifact ledger.
pub fn persist_runtime_healthcheck(
    ledger: &ArtifactLedger,
    report: &RuntimeHealthcheckReport,
) -> Result<std::path::PathBuf> {
    ledger.write_json("qa", "latest-healthcheck.json", report)
}

// ── Internal helpers ──────────────────────────────────────────────────────

fn build_provider_dependency_component(config: &AppConfig) -> ComponentReport {
    let mut status = CheckStatus::Healthy;
    let mut message = String::from("provider dependencies:");
    let mut agents = Vec::new();
    let mut ready_count: u64 = 0;
    let mut degraded_count: u64 = 0;
    let mut total_count: u64 = 0;

    for (agent_key, agent_config) in &config.agents {
        let env_var = agent_config.api_key_env.as_deref().unwrap_or("");
        let secret_env_var = agent_config.secret_key_env.as_deref();
        let agent_name = agent_key;

        if env_var.is_empty() && secret_env_var.is_none_or(|s| s.is_empty()) {
            continue;
        }

        total_count += 1;

        // Try keyring first, fall back to env var (same chain as load_secret_value)
        let api_ready = secret_ref_ready(env_var);
        let secret_ready = match secret_env_var {
            Some(secret_ref) => secret_ref_ready(secret_ref),
            None => true,
        };
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

fn secret_ref_ready(secret_ref: &str) -> bool {
    // Use get_secret() which checks in-memory override map first, then env vars.
    // This ensures API keys set via GUI/CLI secret overrides are properly detected.
    if secret_ref.starts_with("keyring://") {
        let locator = secret_ref.trim_start_matches("keyring://");
        if let Some((service, account)) = locator.split_once('/') {
            if keyring_lookup_accounts(service, account).into_iter().any(
                |(service_name, account_name)| {
                    keyring::Entry::new(&service_name, &account_name)
                        .and_then(|e| e.get_password())
                        .is_ok_and(|v| !v.trim().is_empty())
                },
            ) {
                return true;
            }

            // Also check env var fallback via get_secret() for in-memory overrides
            return keyring_env_fallback_candidates(service, account)
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

fn keyring_lookup_accounts(service: &str, account: &str) -> Vec<(String, String)> {
    let mut targets = vec![(service.to_string(), account.to_string())];

    if service == "go-on" {
        if account == "copilot_api_key" {
            targets.push((service.to_string(), "github_copilot_token".to_string()));
        } else if account == "github_copilot_token" {
            targets.push((service.to_string(), "copilot_api_key".to_string()));
        }
    }

    targets
}

fn keyring_env_fallback_candidates(service: &str, account: &str) -> Vec<String> {
    let mut candidates = Vec::new();

    // NOTE: Must stay in sync with agent.rs load_secret_value() fallback logic.
    // service is NOT checked for openai to maximize backward compatibility for
    // users whose keyring entries may use different service names.
    if account == "openai_api_key" {
        candidates.push("OPENAI_API_KEY".to_string());
    }
    if account == "openai_compatible_api_key" {
        candidates.push("OPENAI_COMPATIBLE_API_KEY".to_string());
        candidates.push("OPENAI_API_KEY".to_string());
    }
    if service == "go-on" && (account == "copilot_api_key" || account == "github_copilot_token") {
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

#[allow(dead_code)] // F-GAP-13 — reserved for secret pool diagnostics
fn secret_pool_status(config: &AppConfig) -> Value {
    let mut secrets = Vec::new();
    for agent_config in config.agents.values() {
        if let Some(env_var) = &agent_config.api_key_env {
            let exists = crate::agent::inspect_secret_pool(env_var, "api_key_env").is_ok();
            secrets.push(json!({
                "secret_name": env_var,
                "resolved": exists,
            }));
        }
    }
    json!(secrets)
}

#[allow(dead_code)] // F-GAP-13 — reserved for agent environment validation
fn missing_envs_for_agent(config: &AppConfig) -> Vec<Value> {
    let mut missing = Vec::new();
    for agent_config in config.agents.values() {
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
    crate::acp::prelude::now_ts()
}
