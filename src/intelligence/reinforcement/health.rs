//! Healthcheck module — runtime health probes and component reports.
//!
//! Extracted from the original monolithic `reinforcement.rs`.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cache::ResponseCache;
use crate::config::{validate_runtime_readiness, AgentConfig, AppConfig};
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
    let mut details = Vec::new();
    let mut status = CheckStatus::Healthy;
    let mut message = String::from("provider dependencies:");

    let provider_api_map: Vec<(&str, &str)> = vec![
        ("copilot", "COPILOT_API_KEY"),
        ("deepseek", "DEEPSEEK_API_KEY"),
        ("anthropic", "ANTHROPIC_API_KEY"),
        ("openai", "OPENAI_API_KEY"),
        ("gemini", "GEMINI_API_KEY"),
        ("wenxin", "WENXIN_API_KEY"),
        ("qwen", "QWEN_API_KEY"),
        ("glm", "GLM_API_KEY"),
        ("hunyuan", "HUNYUAN_API_KEY"),
        ("doubao", "DOUBAO_API_KEY"),
        ("groq", "GROQ_API_KEY"),
        ("mistral", "MISTRAL_API_KEY"),
        ("minimax", "MINIMAX_API_KEY"),
    ];

    let mut found_vendor = false;
    for agent_config in config.agents.values() {
        for (vendor_name, env_var) in &provider_api_map {
            if agent_config.provider == *vendor_name {
                found_vendor = true;
                match std::env::var(env_var) {
                    Ok(_) => {
                        details.push(json!({
                            "provider": vendor_name,
                            "env_var": env_var,
                            "status": "set"
                        }));
                    }
                    Err(_) => {
                        status = CheckStatus::Warn;
                        details.push(json!({
                            "provider": vendor_name,
                            "env_var": env_var,
                            "status": "missing"
                        }));
                    }
                }
            }
        }
    }

    if !found_vendor {
        status = CheckStatus::Skipped;
        message.push_str(" no known provider found");
    }

    ComponentReport {
        name: "provider_dependency".to_string(),
        status,
        message,
        details: json!({
            "provider_api_map": details,
            "secrets": secret_pool_status(config),
            "missing_envs": missing_envs_for_agent(config),
        }),
    }
}

fn secret_pool_status(config: &AppConfig) -> Value {
    let mut secrets = Vec::new();
    for agent_config in config.agents.values() {
        if let Some(api_key) = &agent_config.api_key {
            if api_key.starts_with("keyring:") {
                let name = api_key.trim_start_matches("keyring://").to_string();
                let exists = crate::agent::inspect_secret_pool(&name);
                secrets.push(json!({
                    "secret_name": name,
                    "resolved": exists,
                }));
            }
        }
    }
    json!(secrets)
}

fn missing_envs_for_agent(config: &AppConfig) -> Vec<Value> {
    let mut missing = Vec::new();
    for agent_config in config.agents.values() {
        if let Some(api_key) = &agent_config.api_key {
            if api_key.starts_with("env://") {
                let env_var = api_key.trim_start_matches("env://");
                if std::env::var(env_var).is_err() {
                    missing.push(json!({
                        "agent": agent_config.provider,
                        "expected_env": env_var,
                        "hint": "set the environment variable or use keyring://"
                    }));
                }
            }
        }
    }
    missing
}

fn probe_local_endpoint(url: &str, timeout_secs: u64) -> CheckStatus {
    let (host, port) = extract_host_port(url);
    let addr: SocketAddr = match format!("{}:{}", host, port).to_socket_addrs() {
        Ok(mut addrs) => match addrs.next() {
            Some(addr) => addr,
            None => return CheckStatus::Error,
        },
        Err(_) => return CheckStatus::Error,
    };
    match TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(timeout_secs)) {
        Ok(_) => CheckStatus::Healthy,
        Err(_) => CheckStatus::Error,
    }
}

fn extract_host_port(url: &str) -> (String, u16) {
    let url = url.trim_start_matches("http://").trim_start_matches("https://");
    let without_path = url.split('/').next().unwrap_or(url);
    if let Some((host, port_str)) = without_path.rsplit_once(':') {
        if let Ok(port) = port_str.parse::<u16>() {
            return (host.to_string(), port);
        }
    }
    (without_path.to_string(), 443)
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
