//! Risk assessment and vote policy for chat requests
//!
//! This module contains types and functions for assessing risk levels
//! of chat requests and building vote policies for high-risk scenarios.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::agent::Message;

use super::helpers::{option_bool, option_keywords, option_usize};

/// Policy for conducting risk votes across agents/models.
#[derive(Debug, Clone)]
pub struct RiskVotePolicy {
    pub enabled: bool,
    pub threshold: usize,
    pub domain_keywords: Vec<String>,
    pub decision_keywords: Vec<String>,
}

/// Assessment of risk level for a chat request.
#[derive(Debug, Clone)]
pub struct RiskAssessment {
    pub score: usize,
    pub is_high_risk: bool,
    pub reasons: Vec<String>,
}

/// Outcome of a strong-model vote by a single agent.
#[derive(Debug, Clone)]
pub struct AgentStrongVoteOutcome {
    pub agent: String,
    pub model: Option<String>,
    pub response: String,
    pub reasoning: String,
}

/// A tuple of (agent_name, agent_ref, agent_options) used during vote escalation.
pub type AgentVoteSource = (String, Arc<dyn crate::agent::Agent>, HashMap<String, Value>);

/// Build a risk vote policy from phase options
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

/// Assess the risk level of a chat request
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

/// Normalize a vote key text for comparison
pub(crate) fn normalize_vote_key(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}
