//! Helper functions for chat handling
//!
//! This module contains standalone utility/helper functions used by
//! the chat request processing pipeline. These are not part of the
//! main pipeline but provide shared logic across multiple modules.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{Mutex as StdMutex, OnceLock};

use serde_json::Value;

use crate::agent::Message;
use crate::config::PhaseOptions;

use super::params::ChatParams;

// ── Agent switch state ───────────────────────────────────────────────

#[derive(Default)]
pub(crate) struct AgentSwitchState {
    pub(crate) forced_agent_by_phase: HashMap<String, String>,
    pub(crate) primary_agent_by_phase: HashMap<String, String>,
}

static AGENT_SWITCH_STATE: OnceLock<StdMutex<AgentSwitchState>> = OnceLock::new();

pub(crate) fn agent_switch_state() -> &'static StdMutex<AgentSwitchState> {
    AGENT_SWITCH_STATE.get_or_init(|| StdMutex::new(AgentSwitchState::default()))
}

// ── Numeric helpers ──────────────────────────────────────────────────

pub(crate) fn round_metric(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

// ── Cache short-circuit logic ────────────────────────────────────────

pub(crate) fn cache_short_circuit_allowed(params: &ChatParams) -> bool {
    let mode = params.mode.trim().to_ascii_lowercase();
    if matches!(mode.as_str(), "full_auto" | "safeguard" | "agent" | "edit") {
        return false;
    }

    if params
        .options
        .as_ref()
        .and_then(|opts| opts.extra.get("disable_cache_short_circuit"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return false;
    }

    let latest_user_content = params
        .messages
        .iter()
        .rev()
        .find(|message| message.role.eq_ignore_ascii_case("user"))
        .map(|message| message.content.to_ascii_lowercase())
        .unwrap_or_default();

    if latest_user_content.is_empty() {
        return true;
    }

    let execution_keywords = [
        "fix",
        "implement",
        "write",
        "edit",
        "modify",
        "refactor",
        "run",
        "execute",
        "debug",
        "diagnose",
        "test",
        "build",
        "compile",
        "deploy",
        "tool",
        "workflow",
        "task.execute",
        "workflow.execute",
        "patch",
        "code change",
    ];

    !execution_keywords
        .iter()
        .any(|keyword| latest_user_content.contains(keyword))
}

// ── Quota / token-limit error detection ──────────────────────────────

pub(crate) fn is_quota_or_token_limit_error(error_text: &str) -> bool {
    let text = error_text.to_ascii_lowercase();
    text.contains("429")
        || text.contains("rate limit")
        || text.contains("quota")
        || text.contains("insufficient_quota")
        || text.contains("token") && text.contains("limit")
        || text.contains("token") && text.contains("exhaust")
        || text.contains("billing")
        || text.contains("credit") && text.contains("insufficient")
}

// ── Option extraction helpers ────────────────────────────────────────

pub(crate) fn option_bool(options: &HashMap<String, Value>, key: &str, default: bool) -> bool {
    options.get(key).and_then(Value::as_bool).unwrap_or(default)
}

pub(crate) fn option_usize(options: &HashMap<String, Value>, key: &str, default: usize) -> usize {
    options
        .get(key)
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(default)
}

pub(crate) fn option_keywords(options: &HashMap<String, Value>, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(value) = options.get(key) {
        if let Some(items) = value.as_array() {
            for item in items {
                if let Some(text) = item.as_str() {
                    let trimmed = text.trim().to_ascii_lowercase();
                    if !trimmed.is_empty() {
                        out.push(trimmed);
                    }
                }
            }
        } else if let Some(text) = value.as_str() {
            for token in text.split(',') {
                let trimmed = token.trim().to_ascii_lowercase();
                if !trimmed.is_empty() {
                    out.push(trimmed);
                }
            }
        }
    }
    out
}

// ── Model selection ──────────────────────────────────────────────────

pub(crate) fn select_strong_model_id(agent: &dyn crate::agent::Agent) -> Option<String> {
    let mut models = agent
        .available_models()
        .into_iter()
        .filter(|model| !model.id.trim().is_empty())
        .collect::<Vec<_>>();

    if models.is_empty() {
        return agent.default_model().map(|model| model.id);
    }

    models.sort_by(|left, right| {
        right
            .context_window
            .unwrap_or(0)
            .cmp(&left.context_window.unwrap_or(0))
            .then_with(|| right.capabilities.len().cmp(&left.capabilities.len()))
            .then_with(|| right.is_default.cmp(&left.is_default))
    });

    models.first().map(|model| model.id.clone())
}

pub(crate) fn select_top_models(agent: &dyn crate::agent::Agent, max_models: usize) -> Vec<String> {
    let mut models = agent
        .available_models()
        .into_iter()
        .filter(|model| !model.id.trim().is_empty())
        .collect::<Vec<_>>();

    if models.is_empty() {
        return agent
            .default_model()
            .map(|model| vec![model.id])
            .unwrap_or_default();
    }

    models.sort_by(|left, right| {
        right
            .context_window
            .unwrap_or(0)
            .cmp(&left.context_window.unwrap_or(0))
            .then_with(|| right.capabilities.len().cmp(&left.capabilities.len()))
            .then_with(|| right.is_default.cmp(&left.is_default))
    });

    let ordered = models.into_iter().map(|model| model.id).collect::<Vec<_>>();
    let mut selected = Vec::new();
    for model_id in ordered {
        if !selected.iter().any(|existing| existing == &model_id) {
            selected.push(model_id);
        }
    }
    selected.truncate(max_models.max(1));
    selected
}

// ── Agent reordering ─────────────────────────────────────────────────

pub(crate) fn reorder_agents_with_priority(
    agents: &mut Vec<(String, Arc<dyn crate::agent::Agent>)>,
    preferred: &str,
) -> bool {
    if let Some(index) = agents.iter().position(|(name, _)| name == preferred) {
        if index > 0 {
            let selected = agents.remove(index);
            agents.insert(0, selected);
        }
        return true;
    }
    false
}

// ── Flow phase detection ─────────────────────────────────────────────

pub(crate) fn has_flow_phase(config: &crate::config::AppConfig, phase: &str) -> bool {
    config
        .flow
        .phases
        .iter()
        .any(|candidate| candidate == phase)
        || config.phases.contains_key(phase)
}

#[cfg(test)]
mod tests {
    use crate::agent::Message;

    use super::cache_short_circuit_allowed;
    use crate::acp::r#impl::chat::ChatParams;

    #[test]
    fn cache_short_circuit_allowed_for_read_only_ask() {
        let params = ChatParams {
            mode: "ask".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "What is this architecture about?".to_string(),
            }],
            conversation_id: None,
            branch_id: None,
            phase: None,
            options: None,
            requirement_contract: None,
            plan: None,
            vector_hits: None,
            execution_decision_candidate: None,
        };

        assert!(cache_short_circuit_allowed(&params));
    }

    #[test]
    fn cache_short_circuit_disallowed_for_execution_intent() {
        let params = ChatParams {
            mode: "ask".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "Please fix this bug and run tests".to_string(),
            }],
            conversation_id: None,
            branch_id: None,
            phase: None,
            options: None,
            requirement_contract: None,
            plan: None,
            vector_hits: None,
            execution_decision_candidate: None,
        };

        assert!(!cache_short_circuit_allowed(&params));
    }
}
