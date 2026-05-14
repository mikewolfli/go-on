//! Context helper functions for ACP server
//!
//! This module provides utility functions for managing request context,
//! vector configuration, message optimization, and cache key generation.

use std::future::Future;
use std::time::Duration;

use anyhow::Result;
use tokio::net::{lookup_host, TcpStream};

/// Prefix for keyring secret references
const KEYRING_PREFIX: &str = "keyring://";

use crate::config::{AppConfig, PhaseOptions};

/// Get request timeout from phase options
pub fn request_timeout(options: Option<&PhaseOptions>) -> Option<Duration> {
    options
        .and_then(|opts| opts.request_timeout_seconds)
        .map(Duration::from_secs)
}

/// Get review timeout from phase options, falling back to request timeout.
pub fn review_timeout(options: Option<&PhaseOptions>) -> Option<Duration> {
    options
        .and_then(|opts| opts.review_timeout_seconds)
        .or_else(|| options.and_then(|opts| opts.request_timeout_seconds))
        .map(Duration::from_secs)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointProbeResult {
    Reachable,
    Unreachable,
    TimedOut,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRuntimeReadiness {
    Ready,
    MissingSecret,
    EndpointUnavailable,
    EndpointTimedOut,
}

pub async fn run_with_optional_timeout<T, F, G>(
    timeout_duration: Option<Duration>,
    future: F,
    on_timeout: G,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
    G: FnOnce(Duration) -> anyhow::Error,
{
    if let Some(timeout_duration) = timeout_duration {
        match tokio::time::timeout(timeout_duration, future).await {
            Ok(result) => result,
            Err(_) => Err(on_timeout(timeout_duration)),
        }
    } else {
        future.await
    }
}

pub async fn probe_agent_runtime_readiness(
    config: &AppConfig,
    agent_name: &str,
    timeout_duration: Duration,
) -> AgentRuntimeReadiness {
    let Some(agent) = config.agents.get(agent_name) else {
        return AgentRuntimeReadiness::Ready;
    };
    for key in [
        agent.api_key_env.as_deref(),
        agent.secret_key_env.as_deref(),
    ] {
        let Some(key_name) = key else {
            continue;
        };
        if key_name.starts_with(KEYRING_PREFIX) {
            continue;
        }
        if std::env::var(key_name).is_err() {
            return AgentRuntimeReadiness::MissingSecret;
        }
    }

    let Some(url) = agent.url.as_deref() else {
        return AgentRuntimeReadiness::Ready;
    };

    // Skip endpoint probe for copilot: its local url is only a placeholder;
    // the actual API calls go to api.githubcopilot.com (remote).
    if agent_name.eq_ignore_ascii_case("copilot") {
        return AgentRuntimeReadiness::Ready;
    }

    match probe_local_endpoint(url, timeout_duration).await {
        EndpointProbeResult::Reachable | EndpointProbeResult::Skipped => {
            AgentRuntimeReadiness::Ready
        }
        EndpointProbeResult::Unreachable => AgentRuntimeReadiness::EndpointUnavailable,
        EndpointProbeResult::TimedOut => AgentRuntimeReadiness::EndpointTimedOut,
    }
}

pub async fn probe_local_endpoint(url: &str, timeout_duration: Duration) -> EndpointProbeResult {
    let Some((host, port)) = extract_host_port(url) else {
        return EndpointProbeResult::Skipped;
    };
    let is_local = matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1");
    if !is_local {
        return EndpointProbeResult::Skipped;
    }

    let Ok(addrs) = lookup_host((host.as_str(), port)).await else {
        return EndpointProbeResult::Unreachable;
    };
    let addrs = addrs.collect::<Vec<_>>();
    if addrs.is_empty() {
        return EndpointProbeResult::Unreachable;
    }

    let mut timed_out = false;
    for addr in addrs {
        match tokio::time::timeout(timeout_duration, TcpStream::connect(addr)).await {
            Ok(Ok(_)) => return EndpointProbeResult::Reachable,
            Ok(Err(_)) => continue,
            Err(_) => timed_out = true,
        }
    }

    if timed_out {
        EndpointProbeResult::TimedOut
    } else {
        EndpointProbeResult::Unreachable
    }
}

fn extract_host_port(url: &str) -> Option<(String, u16)> {
    let marker = "://";
    let start = url.find(marker).map(|idx| idx + marker.len()).unwrap_or(0);
    let rest = &url[start..];
    let host_port = rest.split('/').next()?.trim();
    if host_port.is_empty() {
        return None;
    }
    if host_port.starts_with('[') {
        let end = host_port.find(']')?;
        let host = host_port[1..end].to_string();
        let port = host_port[end + 1..]
            .strip_prefix(':')
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(80);
        return Some((host, port));
    }
    if let Some((host, port_raw)) = host_port.rsplit_once(':') {
        if let Ok(port) = port_raw.parse::<u16>() {
            return Some((host.to_string(), port));
        }
    }
    Some((host_port.to_string(), 80))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use super::{probe_agent_runtime_readiness, run_with_optional_timeout, AgentRuntimeReadiness};
    use crate::config::{AgentConfig, AppConfig, FlowConfig};
    use crate::i18n::runtime::tf;

    #[tokio::test]
    async fn run_with_optional_timeout_returns_timeout_error() {
        let err = run_with_optional_timeout(
            Some(Duration::from_millis(5)),
            async {
                tokio::time::sleep(Duration::from_millis(25)).await;
                Ok::<(), anyhow::Error>(())
            },
            |duration| {
                anyhow::anyhow!(
                    "{}",
                    tf(
                        "error.agent_chat_timed_out",
                        &[("duration", &format!("{}ms", duration.as_millis()))]
                    )
                )
            },
        )
        .await
        .expect_err("timeout should be returned");

        assert!(err.to_string().contains("error.agent_chat_timed_out"));
    }

    #[tokio::test]
    async fn probe_agent_runtime_readiness_accepts_async_local_listener() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let port = listener
            .local_addr()
            .expect("local addr should exist")
            .port();

        let mut agents = HashMap::new();
        agents.insert(
            "copilot".to_string(),
            AgentConfig {
                agent_type: "copilot".to_string(),
                url: Some(format!("http://127.0.0.1:{port}")),
                chat_path: None,
                api_key_env: None,
                secret_key_env: None,
                anthropic_version: None,
                model: None,
                max_tokens: None,
                supports_system: None,
            },
        );

        let config = AppConfig {
            default_phase: "coding".to_string(),
            agents,
            flow: FlowConfig {
                name: "coding".to_string(),
                phases: vec!["coding".to_string()],
                workflow_type: crate::config::WorkflowType::Auto,
            },
            phases: HashMap::new(),
            runtime: None,
            cache: None,
            vector: None,
            autotune: None,
            model_selection_mode: "auto".to_string(),
            compliance: None,
            startup_context: None,
            scheduler: None,
            reputation: None,
            role_registry: HashMap::new(),
        };

        let readiness =
            probe_agent_runtime_readiness(&config, "copilot", Duration::from_millis(50)).await;
        assert_eq!(readiness, AgentRuntimeReadiness::Ready);

        drop(listener);
    }
}
