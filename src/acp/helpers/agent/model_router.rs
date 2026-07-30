//! Model-based agent routing and high-risk vote configuration
//!
//! This module extracts the Model-Based Agent Routing / Filtering logic
//! from the chat request processing pipeline (`process_chat_request`).
//!
//! ## Functions
//!
//! * [`filter_agents_by_model`] — filters the agent list to match a user-specified model option.
//! * [`build_high_risk_vote_config`] — computes all high-risk vote and escalation parameters
//!   from the risk assessment and agent options.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tracing::{debug, warn};

use crate::acp::r#impl::chat::{RiskAssessment, RiskVotePolicy};
use crate::agent::Agent;

/// Result of filtering agents by model option.
#[derive(Debug)]
pub(crate) struct FilterResult {
    /// Whether the model option is specific (not `"auto"` or empty).
    pub(crate) model_is_specific: bool,
}

/// High-risk vote configuration computed from agent options and risk assessment.
#[derive(Debug)]
pub(crate) struct HighRiskVoteConfig {
    /// Whether multi-agent voting is enabled (derived from policy + risk + model specificity).
    pub(crate) enable_high_risk_multi_agent_vote: bool,
    /// Minimum number of vote agents (clamped 1..=6).
    pub(crate) min_vote_agents: usize,
    /// Maximum number of vote agents (clamped between min_vote_agents and 8).
    pub(crate) max_vote_agents: usize,
    /// Whether multi-model escalation is enabled.
    pub(crate) escalation_enabled: bool,
    /// Number of models per agent for escalation (clamped 2..=6).
    pub(crate) escalation_models_per_agent: usize,
    /// Maximum number of agents for escalation (clamped 1..=max_vote_agents).
    pub(crate) escalation_max_agents: usize,
}

// ---------------------------------------------------------------------------
// Helper option readers (mirrored from chat.rs for self-containment)
// ---------------------------------------------------------------------------

fn option_bool(options: &HashMap<String, Value>, key: &str, default: bool) -> bool {
    options.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn option_usize(options: &HashMap<String, Value>, key: &str, default: usize) -> usize {
    options
        .get(key)
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(default)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Filter agents by model option.
///
/// When the user picks a specific model (e.g. `"deepseek-v4-flash"`), this
/// function replaces `agents` with only the matching agent(s) so unrelated
/// providers are skipped.  When the model is `"auto"` or empty the phase
/// agent list is kept intact.
///
/// Special cases:
/// * `"copilot"`, `"copilot/auto"`, `"copilot-auto"` — retain only the copilot agent.
/// * Qualified model IDs containing `/` (e.g. `"siliconflow/deepseek-v3"`)
///   use a more conservative match to avoid false prefix matches.
///
/// If the filter eliminates all agents, the original list is restored and
/// a warning is logged.
pub(crate) fn filter_agents_by_model(
    agents: &mut Vec<(String, Arc<dyn Agent>)>,
    options: &HashMap<String, Value>,
) -> FilterResult {
    let model_is_specific = options
        .get("model")
        .and_then(|v| v.as_str())
        .is_some_and(|m| !m.is_empty() && m != "auto");

    let model_str = options
        .get("model")
        .and_then(|v| v.as_str())
        .map(String::from);

    // Handle Copilot variants – keep only the copilot agent.
    if let Some(ref model_val) = model_str {
        if model_val == "copilot/auto" || model_val == "copilot-auto" || model_val == "copilot" {
            agents.retain(|(name, _)| name.eq_ignore_ascii_case("copilot"));
        }
    }

    // Prefix-match filtering for specific model strings.
    if model_is_specific {
        let model = model_str.as_deref().unwrap_or("");
        let model_lower = model.to_ascii_lowercase();

        // Track the original agent count before filtering.
        // If filtering removes all agents, we restore the originals
        // so the caller still has phase candidate agents to use.
        let before = std::mem::take(agents);

        agents.extend(
            before
                .iter()
                .filter(|(name, _)| {
                    let name_lower = name.to_ascii_lowercase();

                    if model_lower.starts_with(&name_lower) && model_lower.contains('/') {
                        // Qualified model ID like "siliconflow/deepseek-..." —
                        // only match if the agent name also appears after '/',
                        // or the agent name IS the full model string.
                        name_lower.starts_with(&model_lower)
                            || model_lower.ends_with(&format!("/{}", name_lower))
                    } else {
                        model_lower.starts_with(&name_lower) || name_lower.starts_with(&model_lower)
                    }
                })
                .cloned(),
        );

        if agents.is_empty() {
            // When model is specific but no agent matches, DO NOT restore all
            // agents. The caller (select_and_score_agents) will return a clear
            // error telling the user the model didn't match any configured agent.
            warn!(
                model = %model,
                "model filter did not match any agent — will report error"
            );
        } else {
            let removed = before.len() - agents.len();
            if removed > 0 {
                debug!("filter_agents_by_model: removed {} agent(s)", removed);
            }
        }
    }

    FilterResult { model_is_specific }
}

/// Build high-risk vote configuration from agent options and risk assessment.
///
/// Computes all vote-related configuration values:
/// * `enable_high_risk_vote` — voting is gated by policy, assessment, and model specificity.
/// * `enable_high_risk_multi_agent_vote` — multi-agent variant.
/// * `min_vote_agents` / `max_vote_agents` — agent count bounds.
/// * Escalation parameters (`escalation_enabled`, `escalation_models_per_agent`,
///   `escalation_max_agents`).
pub(crate) fn build_high_risk_vote_config(
    options: &HashMap<String, Value>,
    risk_policy: &RiskVotePolicy,
    risk_assessment: &RiskAssessment,
    model_is_specific: bool,
) -> HighRiskVoteConfig {
    let enable_high_risk_vote =
        risk_policy.enabled && risk_assessment.is_high_risk && !model_is_specific;

    let min_vote_agents = option_usize(options, "high_risk_vote_min_agents", 2).clamp(1, 6);

    let max_vote_agents = option_usize(options, "high_risk_vote_max_agents", 3)
        .max(min_vote_agents)
        .clamp(min_vote_agents, 8);

    let escalation_enabled = option_bool(options, "high_risk_escalate_multi_model_enabled", true);

    let escalation_models_per_agent =
        option_usize(options, "high_risk_escalate_models_per_agent", 2).clamp(2, 6);

    let escalation_max_agents =
        option_usize(options, "high_risk_escalate_max_agents", max_vote_agents)
            .clamp(1, max_vote_agents);

    HighRiskVoteConfig {
        enable_high_risk_multi_agent_vote: enable_high_risk_vote
            && option_bool(options, "high_risk_multi_agent_vote_enabled", true),
        min_vote_agents,
        max_vote_agents,
        escalation_enabled,
        escalation_models_per_agent,
        escalation_max_agents,
    }
}
