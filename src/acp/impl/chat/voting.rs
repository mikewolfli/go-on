//! Risk assessment, high-risk voting, and agent voting types
//!
//! Contains the data structures and pure helper functions for assessing
//! risk levels in chat requests. Extracted from the parent `chat.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::agent::Message;
pub(crate) use crate::shared::{option_bool, option_usize};

/// Policy for configuring high-risk voting behavior.
#[derive(Debug, Clone)]
pub(crate) struct RiskVotePolicy {
    pub(crate) enabled: bool,
    threshold: usize,
    domain_keywords: Vec<String>,
    decision_keywords: Vec<String>,
}

/// Result of a risk assessment.
#[derive(Debug, Clone)]
pub(crate) struct RiskAssessment {
    pub(crate) score: usize,
    pub(crate) is_high_risk: bool,
    pub(crate) reasons: Vec<String>,
}

/// Outcome of a single agent's strong-model vote.
#[derive(Debug, Clone)]
pub(crate) struct AgentStrongVoteOutcome {
    pub(crate) agent: String,
    pub(crate) model: Option<String>,
    pub(crate) response: String,
    pub(crate) reasoning: String,
}

pub(crate) type AgentVoteSource = (String, Arc<dyn crate::agent::Agent>, HashMap<String, Value>);

fn option_keywords(options: &HashMap<String, Value>, key: &str) -> Vec<String> {
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

pub(crate) fn build_risk_vote_policy(options: &HashMap<String, Value>) -> RiskVotePolicy {
    const DEFAULT_DOMAIN_KEYWORDS: &[&str] = &[
        "medical",
        "diagnosis",
        "clinical",
        "prescription",
        "treatment",
        "surgery",
        "healthcare",
        "legal",
        "contract",
        "compliance",
        "regulation",
        "litigation",
        "finance",
        "financial",
        "investment",
        "portfolio",
        "credit",
        "loan",
        "underwriting",
        "fraud",
        "aml",
        "tax",
        "audit",
        "insurance",
        "privacy",
        "security incident",
        "safety-critical",
    ];
    const DEFAULT_DECISION_KEYWORDS: &[&str] = &[
        "approve",
        "reject",
        "deny",
        "diagnose",
        "prescribe",
        "recommendation",
        "risk control",
        "decision",
        "compliance decision",
        "legal advice",
        "medical advice",
        "financial advice",
    ];

    let mut domain_keywords = DEFAULT_DOMAIN_KEYWORDS
        .iter()
        .map(|item| (*item).to_string())
        .collect::<Vec<_>>();
    domain_keywords.extend(option_keywords(options, "high_risk_domain_keywords"));
    domain_keywords.sort();
    domain_keywords.dedup();

    let mut decision_keywords = DEFAULT_DECISION_KEYWORDS
        .iter()
        .map(|item| (*item).to_string())
        .collect::<Vec<_>>();
    decision_keywords.extend(option_keywords(options, "high_risk_decision_keywords"));
    decision_keywords.sort();
    decision_keywords.dedup();

    RiskVotePolicy {
        enabled: option_bool(options, "high_risk_vote_enabled", true),
        threshold: option_usize(options, "high_risk_vote_threshold", 2).clamp(1, 10),
        domain_keywords,
        decision_keywords,
    }
}

pub(crate) fn assess_high_risk(
    messages: &[Message],
    mode: &str,
    policy: &RiskVotePolicy,
) -> RiskAssessment {
    let corpus = messages
        .iter()
        .filter(|message| message.role.eq_ignore_ascii_case("user"))
        .map(|message| message.content.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("\n");

    let mut score = 0usize;
    let mut reasons = Vec::new();

    for keyword in &policy.domain_keywords {
        if corpus.contains(keyword) {
            score += 2;
            reasons.push(format!("domain:{keyword}"));
        }
    }
    for keyword in &policy.decision_keywords {
        if corpus.contains(keyword) {
            score += 1;
            reasons.push(format!("decision:{keyword}"));
        }
    }
    if matches!(mode, "safeguard" | "full_auto") {
        score += 1;
        reasons.push(format!("mode:{mode}"));
    }

    reasons.sort();
    reasons.dedup();

    RiskAssessment {
        score,
        is_high_risk: score >= policy.threshold,
        reasons,
    }
}

pub(crate) fn normalize_vote_key(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// Result of a single agent's high-risk vote attempt.
pub(crate) struct HighRiskVoteAttemptResult {
    pub(crate) attempt_log: Value,
    pub(crate) candidate: Option<AgentStrongVoteOutcome>,
    pub(crate) source: Option<AgentVoteSource>,
    pub(crate) failure: Option<Value>,
}

/// Select the strongest model from an agent's available models.
///
/// Equivalent to `select_top_models(agent, 1).first()` — delegates to the
/// shared ranking implementation (single sort, no duplicated logic).
pub(crate) fn select_strong_model_id(agent: &dyn crate::agent::Agent) -> Option<String> {
    select_top_models(agent, 1).into_iter().next()
}

/// Select the top N models from an agent by capability ranking.
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
