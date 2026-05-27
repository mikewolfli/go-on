//! Flow management
//!
//! This module handles the flow management logic, including phase resolution and agent routing.
//!
//! Phase 0/1 discipline:
//! - All phase/agent routes must support `AgentTaskEnvelope`, `AgentTaskResult`, and `AgentAuditLog` structures.
//! - Audit logs should be emitted at phase/agent entry points and decision nodes to support trace/replay/audit.
//! - The mode/phase/provider capability matrix is extensible (see design.md).

use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::warn;

use crate::agent::{Agent, AgentRegistry};
use crate::config::{AppConfig, PhaseConfig, PhaseOptions};
use crate::error::ProxyError;
use crate::pua::merge_phase_principles;

/// Resolved phase information
#[derive(Clone, Debug)]
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
#[allow(missing_debug_implementations)]
pub struct ResolvedRouting {
    /// Resolved phase information
    pub phase: ResolvedPhase,
    /// List of resolved agents (name, agent instance)
    pub agents: Vec<(String, Arc<dyn Agent>)>,
}

impl std::fmt::Debug for ResolvedRouting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedRouting")
            .field("phase", &self.phase)
            .field(
                "agents",
                &self.agents.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Flow manager for handling phase resolution and routing
#[derive(Debug)]
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

    /// Get the underlying application configuration.
    pub fn config(&self) -> Arc<AppConfig> {
        Arc::clone(&self.config)
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
        let mut phase_name = self
            .forced_phase
            .clone()
            .or(requested_phase)
            .unwrap_or_else(|| self.config.default_phase.clone());

        // Unknown phase: silently fall back to the configured default phase.
        // This prevents errors when a GUI session has a stale phase value
        // (e.g. "act" from an older config) that no longer exists in [flow].phases.
        if !self
            .config
            .flow
            .phases
            .iter()
            .any(|name| name == &phase_name)
        {
            let fallback = self.config.default_phase.clone();
            warn!(
                "phase '{}' not in [flow].phases ({:?}), falling back to default '{}'",
                phase_name, self.config.flow.phases, fallback
            );
            phase_name = fallback;
        }

        let phase_cfg = self
            .config
            .phases
            .get(&phase_name)
            .with_context(|| format!("phase '{}' not found in [phases]", phase_name))?;

        let resolved_phase = build_phase(&self.config.flow.name, &phase_name, phase_cfg);
        let mut resolved_agents: Vec<(String, Arc<dyn Agent>)> = Vec::new();

        if resolved_phase.agent_names.is_empty() {
            // Path B: no agents configured — auto-map by using all registered agents.
            // Fallback is always enabled in this path.
            for agent_name in registry.names() {
                if let Some(agent) = registry.get(&agent_name) {
                    resolved_agents.push((agent_name, agent));
                }
            }
            if resolved_agents.is_empty() {
                return Err(ProxyError::AgentNotFound("(auto)".to_string()).into());
            }
        } else {
            // Path A: explicit agent list configured — deterministic path.
            // If fallback is disabled, only the first configured agent can be used.
            // If fallback is enabled, iterate in order and keep all currently available agents.
            for (idx, agent_name) in resolved_phase.agent_names.iter().enumerate() {
                if let Some(agent) = registry.get(agent_name) {
                    resolved_agents.push((agent_name.clone(), agent));
                } else if idx == 0 && !resolved_phase.fallback {
                    return Err(ProxyError::AgentNotFound(agent_name.clone()).into());
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
                return Err(ProxyError::AgentNotFound(first).into());
            }

            // NOTE: The `break` inside the loop above already limits iteration to one agent
            // when fallback is disabled, so `resolved_agents` will only contain at most one
            // entry. The old `truncate(1)` call here was redundant and has been removed.
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
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use crate::agent::AgentRegistry;
    use crate::config::{
        AgentConfig, AppConfig, FlowConfig, PhaseConfig, PhaseOptions, RuntimeConfig,
    };
    use crate::intelligence::capability_graph::CapabilityGraph;

    use super::FlowManager;

    fn make_registry(config: Arc<AppConfig>) -> AgentRegistry {
        AgentRegistry::from_config(
            Arc::clone(&config),
            reqwest::Client::new(),
            Arc::new(Mutex::new(CapabilityGraph::new())),
        )
        .expect("registry should build")
    }

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
            supports_vision: None,
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
                supports_vision: None,
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
            schema_version: "1.0.0".to_string(),
            default_phase: "coding".to_string(),
            agents,
            flow: FlowConfig {
                name: "flow".to_string(),
                phases: vec!["coding".to_string(), "review".to_string()],
                workflow_type: crate::config::WorkflowType::Auto,
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
    fn resolve_uses_default_phase_and_keeps_fallback_agents() {
        let config = Arc::new(build_test_config(Some(true)));
        let registry = make_registry(Arc::clone(&config));
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
        let registry = make_registry(Arc::clone(&config));
        let flow = FlowManager::new(Arc::clone(&config), None);

        let resolved = flow.resolve(None, &registry).expect("phase should resolve");

        assert_eq!(resolved.agents.len(), 1);
        assert_eq!(resolved.agents[0].0, "copilot");
    }

    #[test]
    fn resolve_unknown_phase_falls_back_to_default() {
        let config = Arc::new(build_test_config(Some(true)));
        let registry = make_registry(Arc::clone(&config));
        let flow = FlowManager::new(Arc::clone(&config), None);

        // Unknown phase should fall back to default instead of error.
        let result = flow.resolve(Some("delivery".to_string()), &registry);
        assert!(
            result.is_ok(),
            "unknown phase should fall back to default, got: {:?}",
            result.err()
        );
        let routing = result.unwrap();
        assert_eq!(
            routing.phase.phase_name, "coding",
            "should fall back to default phase 'coding'"
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
        let registry = make_registry(Arc::clone(&config));
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
        let registry = make_registry(Arc::clone(&config));
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
        let registry = make_registry(Arc::clone(&config));
        let flow = FlowManager::new(Arc::clone(&config), None);

        let resolved = flow
            .resolve(Some("review".to_string()), &registry)
            .expect("review phase should resolve");

        assert!(resolved.phase.fallback);
        assert_eq!(resolved.agents.len(), 1);
        assert_eq!(resolved.agents[0].0, "copilot");
    }

    #[test]
    fn resolve_empty_agents_auto_maps_all_registry_agents() {
        // Path B: phase with no configured agents should resolve to all registered agents.
        let mut config = build_test_config(Some(true));
        config
            .phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .agents = vec![];

        let config = Arc::new(config);
        let registry = make_registry(Arc::clone(&config));
        let flow = FlowManager::new(Arc::clone(&config), None);

        let resolved = flow
            .resolve(None, &registry)
            .expect("empty-agents phase should auto-map");

        // All agents registered in the config must appear in the resolved list.
        let resolved_names: Vec<&str> = resolved.agents.iter().map(|(n, _)| n.as_str()).collect();
        for name in config.agents.keys() {
            assert!(
                resolved_names.contains(&name.as_str()),
                "auto-map should include agent '{name}'"
            );
        }
        assert!(
            !resolved.agents.is_empty(),
            "auto-map must not return empty agent list"
        );
    }
}
