//! Flow management
//!
//! This module handles the flow management logic, including phase resolution and agent routing.
//!
//! Phase 0/1 discipline:
//! - 所有 phase/agent 路由都应支持 AgentTaskEnvelope/AgentTaskResult/AgentAuditLog 结构
//! - 推荐在 phase/agent 入口和决策点生成审计日志，便于 trace/replay/audit
//! - 可扩展 mode/phase/provider 能力兼容矩阵（见 design.md）

use std::sync::Arc;

use anyhow::{Context, Result};

use crate::agent::{Agent, AgentRegistry};
use crate::config::{AppConfig, PhaseConfig, PhaseOptions};
use crate::error::ProxyError;
use crate::pua::merge_phase_principles;

/// Resolved phase information
#[derive(Clone)]
pub struct ResolvedPhase {
    /// Flow name
    pub flow_name: String,
    /// Phase name
    pub phase_name: String,
    /// Phase description
    pub phase_description: String,
    /// Optional list of principles
    pub principles: Option<Vec<String>>,
    /// Optional phase options
    pub options: Option<PhaseOptions>,
    /// Whether fallback to other agents is enabled
    pub fallback: bool,
    /// List of agent names in order of preference
    pub agent_names: Vec<String>,
}

/// Resolved routing information
pub struct ResolvedRouting {
    /// Resolved phase information
    pub phase: ResolvedPhase,
    /// List of resolved agents (name, agent instance)
    pub agents: Vec<(String, Arc<dyn Agent>)>,
}

/// Flow manager for handling phase resolution and routing
pub struct FlowManager {
    /// Application configuration
    config: Arc<AppConfig>,
    /// Forced phase (overrides requested phase)
    forced_phase: Option<String>,
}

impl FlowManager {
    /// Create a new flow manager
    ///
    /// # Arguments
    /// * `config` - Application configuration
    /// * `forced_phase` - Optional forced phase (overrides requested phase)
    ///
    /// # Returns
    /// * `Self` - New flow manager instance
    pub fn new(config: Arc<AppConfig>, forced_phase: Option<String>) -> Self {
        Self {
            config,
            forced_phase,
        }
    }

    /// Check if a phase exists in the flow
    ///
    /// # Arguments
    /// * `phase_name` - Phase name to check
    ///
    /// # Returns
    /// * `bool` - True if the phase exists, false otherwise
    pub fn has_phase(&self, phase_name: &str) -> bool {
        self.config
            .flow
            .phases
            .iter()
            .any(|name| name == phase_name)
    }

    /// Get the default phase
    ///
    /// # Returns
    /// * `&str` - Default phase name
    pub fn default_phase(&self) -> &str {
        &self.config.default_phase
    }

    /// Resolve a phase and its agents
    ///
    /// This method resolves the requested phase (or uses the default if not specified),
    /// and returns the resolved phase information along with the available agents.
    ///
    /// # Arguments
    /// * `requested_phase` - Optional requested phase
    /// * `registry` - Agent registry
    ///
    /// # Returns
    /// * `Result<ResolvedRouting>` - Returns Ok(ResolvedRouting) if resolution succeeds, or an error if something goes wrong
    pub fn resolve(
        &self,
        requested_phase: Option<String>,
        registry: &AgentRegistry,
    ) -> Result<ResolvedRouting> {
        let phase_name = self
            .forced_phase
            .clone()
            .or(requested_phase)
            .unwrap_or_else(|| self.config.default_phase.clone());

        if !self
            .config
            .flow
            .phases
            .iter()
            .any(|name| name == &phase_name)
        {
            return Err(ProxyError::UnknownPhase(phase_name).into());
        }

        let phase_cfg = self
            .config
            .phases
            .get(&phase_name)
            .with_context(|| format!("phase '{}' not found in [phases]", phase_name))?;

        let resolved_phase = build_phase(&self.config.flow.name, &phase_name, phase_cfg);
        let mut resolved_agents: Vec<(String, Arc<dyn Agent>)> = Vec::new();

        // If fallback is disabled, only the first configured agent can be used.
        // If fallback is enabled, iterate in order and keep all currently available agents.
        for (idx, agent_name) in resolved_phase.agent_names.iter().enumerate() {
            if let Some(agent) = registry.get(agent_name) {
                resolved_agents.push((agent_name.clone(), agent));
            } else if idx == 0 && !resolved_phase.fallback {
                return Err(ProxyError::UnknownAgent(agent_name.clone()).into());
            }

            if !resolved_phase.fallback {
                break;
            }
        }

        if resolved_agents.is_empty() {
            let first = resolved_phase
                .agent_names
                .first()
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());
            return Err(ProxyError::UnknownAgent(first).into());
        }

        if !resolved_phase.fallback {
            resolved_agents.truncate(1);
        }

        Ok(ResolvedRouting {
            phase: resolved_phase,
            agents: resolved_agents,
        })
    }
}

/// Build a resolved phase from configuration
///
/// # Arguments
/// * `flow_name` - Flow name
/// * `name` - Phase name
/// * `cfg` - Phase configuration
///
/// # Returns
/// * `ResolvedPhase` - Resolved phase information
fn build_phase(flow_name: &str, name: &str, cfg: &PhaseConfig) -> ResolvedPhase {
    ResolvedPhase {
        flow_name: flow_name.to_string(),
        phase_name: name.to_string(),
        phase_description: cfg.description.clone(),
        principles: merge_phase_principles(cfg.principles.clone(), name),
        options: cfg.options.clone(),
        fallback: cfg.fallback.unwrap_or(true),
        agent_names: cfg.agents.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use serde_json::json;

    use crate::agent::AgentRegistry;
    use crate::config::{
        AgentConfig, AppConfig, FlowConfig, PhaseConfig, PhaseOptions, RuntimeConfig,
    };

    use super::FlowManager;

    fn test_agent(agent_type: &str, url: Option<&str>) -> AgentConfig {
        AgentConfig {
            agent_type: agent_type.to_string(),
            url: url.map(std::string::ToString::to_string),
            chat_path: None,
            api_key_env: None,
            secret_key_env: None,
            anthropic_version: None,
            model: None,
            max_tokens: None,
            supports_system: None,
        }
    }

    fn build_test_config(fallback: Option<bool>) -> AppConfig {
        let mut agents = HashMap::new();
        agents.insert(
            "copilot".to_string(),
            test_agent("copilot", Some("http://127.0.0.1:8080")),
        );
        agents.insert(
            "deepseek".to_string(),
            AgentConfig {
                agent_type: "deepseek".to_string(),
                url: None,
                chat_path: None,
                api_key_env: Some("DEEPSEEK_API_KEY".to_string()),
                secret_key_env: None,
                anthropic_version: None,
                model: Some("deepseek-chat".to_string()),
                max_tokens: None,
                supports_system: None,
            },
        );

        let mut phases = HashMap::new();
        phases.insert(
            "coding".to_string(),
            PhaseConfig {
                description: "coding".to_string(),
                agents: vec!["copilot".to_string(), "deepseek".to_string()],
                fallback,
                principles: Some(vec!["use tests".to_string()]),
                options: Some(PhaseOptions {
                    extra: HashMap::from([(String::from("stage"), json!("strict"))]),
                    ..PhaseOptions::default()
                }),
            },
        );
        phases.insert(
            "review".to_string(),
            PhaseConfig {
                description: "review".to_string(),
                agents: vec!["copilot".to_string()],
                fallback: None,
                principles: Some(vec!["be strict".to_string()]),
                options: Some(PhaseOptions {
                    extra: HashMap::from([(String::from("approval_mode"), json!("dual"))]),
                    ..PhaseOptions::default()
                }),
            },
        );

        AppConfig {
            default_phase: "coding".to_string(),
            agents,
            flow: FlowConfig {
                name: "flow".to_string(),
                phases: vec!["coding".to_string(), "review".to_string()],
            },
            phases,
            runtime: Some(RuntimeConfig::default()),
            cache: None,
            vector: None,
            autotune: None,
            model_selection_mode: "adaptive".to_string(),
        }
    }

    #[test]
    fn resolve_uses_default_phase_and_keeps_fallback_agents() {
        let config = Arc::new(build_test_config(Some(true)));
        let registry = AgentRegistry::from_config(Arc::clone(&config), reqwest::Client::new())
            .expect("registry should build");
        let flow = FlowManager::new(Arc::clone(&config), None);

        let resolved = flow.resolve(None, &registry).expect("phase should resolve");

        assert_eq!(resolved.phase.phase_name, "coding");
        assert_eq!(resolved.agents.len(), 2);
        assert_eq!(resolved.agents[0].0, "copilot");
        assert_eq!(resolved.agents[1].0, "deepseek");
    }

    #[test]
    fn resolve_without_fallback_only_returns_first_agent() {
        let config = Arc::new(build_test_config(Some(false)));
        let registry = AgentRegistry::from_config(Arc::clone(&config), reqwest::Client::new())
            .expect("registry should build");
        let flow = FlowManager::new(Arc::clone(&config), None);

        let resolved = flow.resolve(None, &registry).expect("phase should resolve");

        assert_eq!(resolved.agents.len(), 1);
        assert_eq!(resolved.agents[0].0, "copilot");
    }

    #[test]
    fn resolve_unknown_phase_fails() {
        let config = Arc::new(build_test_config(Some(true)));
        let registry = AgentRegistry::from_config(Arc::clone(&config), reqwest::Client::new())
            .expect("registry should build");
        let flow = FlowManager::new(Arc::clone(&config), None);

        let result = flow.resolve(Some("delivery".to_string()), &registry);
        assert!(result.is_err(), "unknown phase must fail");
        let err_text = result.err().expect("error must exist").to_string();
        assert!(
            err_text.contains("phase not found: delivery"),
            "unexpected error: {err_text}"
        );
    }

    #[test]
    fn has_phase_and_default_phase_report_configured_values() {
        let config = Arc::new(build_test_config(Some(true)));
        let flow = FlowManager::new(config, None);

        assert!(flow.has_phase("coding"));
        assert!(flow.has_phase("review"));
        assert!(!flow.has_phase("delivery"));
        assert_eq!(flow.default_phase(), "coding");
    }

    #[test]
    fn resolve_requested_phase_returns_review_metadata() {
        let config = Arc::new(build_test_config(Some(true)));
        let registry = AgentRegistry::from_config(Arc::clone(&config), reqwest::Client::new())
            .expect("registry should build");
        let flow = FlowManager::new(Arc::clone(&config), None);

        let resolved = flow
            .resolve(Some("review".to_string()), &registry)
            .expect("requested review phase should resolve");

        assert_eq!(resolved.phase.flow_name, "flow");
        assert_eq!(resolved.phase.phase_name, "review");
        assert_eq!(resolved.phase.phase_description, "review");
        let principles = resolved
            .phase
            .principles
            .clone()
            .expect("review phase principles should exist");
        assert!(principles.contains(&"be strict".to_string()));
        assert!(principles.iter().any(|item| item.contains("PUA red line")));
        assert!(resolved.phase.fallback);
        assert_eq!(resolved.phase.agent_names, vec!["copilot".to_string()]);
        assert_eq!(resolved.agents.len(), 1);
        assert_eq!(resolved.agents[0].0, "copilot");
        assert_eq!(
            resolved
                .phase
                .options
                .as_ref()
                .and_then(|options| options.extra.get("approval_mode"))
                .and_then(|value| value.as_str()),
            Some("dual")
        );
    }

    #[test]
    fn forced_phase_overrides_requested_phase() {
        let config = Arc::new(build_test_config(Some(true)));
        let registry = AgentRegistry::from_config(Arc::clone(&config), reqwest::Client::new())
            .expect("registry should build");
        let flow = FlowManager::new(Arc::clone(&config), Some("review".to_string()));

        let resolved = flow
            .resolve(Some("coding".to_string()), &registry)
            .expect("forced phase should win");

        assert_eq!(resolved.phase.phase_name, "review");
        assert_eq!(resolved.agents.len(), 1);
        assert_eq!(resolved.agents[0].0, "copilot");
    }

    #[test]
    fn phase_without_explicit_fallback_defaults_to_true() {
        let config = Arc::new(build_test_config(Some(false)));
        let registry = AgentRegistry::from_config(Arc::clone(&config), reqwest::Client::new())
            .expect("registry should build");
        let flow = FlowManager::new(Arc::clone(&config), None);

        let resolved = flow
            .resolve(Some("review".to_string()), &registry)
            .expect("review phase should resolve");

        assert!(resolved.phase.fallback);
        assert_eq!(resolved.agents.len(), 1);
        assert_eq!(resolved.agents[0].0, "copilot");
    }
}
