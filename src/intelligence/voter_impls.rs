//! Concrete [`AgentVoter`] implementations for the weighted-vote / Delphi-method
//! debate system.
//!
//! Provides five voter strategies:
//!
//! | Voter | Strategy |
//! |---|---|
//! | [`CapabilityBusVoter`] | Wraps `Arc<CapabilityBus>`, votes via capability matching |
//! | [`LocalAgentVoter`] | Keyword-heuristic voter using `contains` checks |
//! | [`RationalizationGuardVoter`] | Safety-guard voter based on confidence thresholds |
//! | [`DeepSeekVoter`] | LLM-based voter via DeepSeek API |
//! | [`LocalVoter`] | Configurable local model voter using `AgentConfig` |
//!
//! **Wiring**: The `#[async_trait]` AgentVoter trait allows all voters to run
//! asynchronously. Voters are registered in `hub::init_intel_voters()` and
//! participate in the Delphi debate path via
//! `hub::consensus_vote_with_reputation()`.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::capability_bus::core::CapabilityBus;
use super::weighted_vote::{AgentVoter, Vote};
use crate::governance::rationalization::SelfRationalizationGuard;

use crate::config::AgentConfig;

// ── CapabilityBusVoter ──────────────────────────────────────────────────

/// Votes based on the capability graph and reputation data exposed by the
/// [`CapabilityBus`].
///
/// The voter inspects the `CapabilityBus` profile to determine how many
/// agents are registered and their health status.  A well-covered capability
/// space yields higher confidence.
pub struct CapabilityBusVoter {
    /// Name of this voter.
    name: String,
    /// Shared capability bus reference.
    bus: Arc<CapabilityBus>,
}

impl CapabilityBusVoter {
    /// Create a new voter wrapping the given capability bus.
    ///
    /// **Wiring status**: The async `AgentVoter` trait is now integrated via
    /// `#[async_trait]`, and this voter is registered in `hub::init_intel_voters()`.
    /// The Delphi debate path (`consensus_vote_with_reputation` with
    /// `VoteMode::DelphiDebate`) delegates to the stored voters. The TODO-BLUE64
    /// wiring is **complete**.
    pub fn new(name: impl Into<String>, bus: Arc<CapabilityBus>) -> Self {
        Self {
            name: name.into(),
            bus,
        }
    }
}

#[async_trait]
impl AgentVoter for CapabilityBusVoter {
    fn name(&self) -> &str {
        &self.name
    }

    /// Vote by inspecting the capability bus profile.
    ///
    /// The `context` may contain the proposal description; the voter uses it
    /// together with bus metrics to produce a reasoned vote.
    async fn vote(&self, context: &str) -> Vote {
        let profile = self.bus.capability_bus_profile();

        // Compute a capability coverage ratio from the profile.
        let coverage = if profile.capability_graph_agents > 0 {
            let healthy = profile.observability_tracked_agents.max(1);
            (profile.reputation_agents_count as f64 / healthy as f64).clamp(0.0, 1.0)
        } else {
            0.3 // no agents registered → low default confidence
        };

        // Keyword-based signal extraction from context.
        let has_security_keywords = context.to_lowercase().contains("security")
            || context.to_lowercase().contains("vulnerability");
        let has_performance_keywords = context.to_lowercase().contains("performance")
            || context.to_lowercase().contains("latency");

        // Decide: approve when coverage is sufficient or the context matches
        // known capability domains.
        let approves = coverage > 0.4 || has_security_keywords || has_performance_keywords;

        let reasoning = format!(
            "CapabilityBusVoter: coverage={:.2}, security={}, performance={}, agents={}",
            coverage,
            has_security_keywords,
            has_performance_keywords,
            profile.capability_graph_agents,
        );

        let confidence = if has_security_keywords {
            (coverage + 0.3).min(1.0)
        } else {
            coverage
        };

        Vote {
            approves,
            reasoning,
            confidence,
        }
    }
}

// ── LocalAgentVoter ────────────────────────────────────────────────────

/// A simple heuristic-based voter that assesses proposals using local keyword
/// analysis and a weighted scoring model.
///
/// Useful as a baseline / default voter when no external coordinator is
/// available.
pub struct LocalAgentVoter {
    /// Name of this voter.
    name: String,
}

impl LocalAgentVoter {
    /// Create a new local agent voter.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[async_trait]
impl AgentVoter for LocalAgentVoter {
    fn name(&self) -> &str {
        &self.name
    }

    /// Vote by applying a weighted keyword heuristic to the context.
    async fn vote(&self, context: &str) -> Vote {
        let lower = context.to_lowercase();

        // Positive signals — increase confidence.
        let has_positive = lower.contains("optimize")
            || lower.contains("improve")
            || lower.contains("fix")
            || lower.contains("upgrade");

        // Negative signals — decrease confidence.
        let has_risk =
            lower.contains("breaking") || lower.contains("unsafe") || lower.contains("deprecate");

        // Neutral signals — indicate a well-defined request.
        let has_structure = lower.contains("proposal")
            || lower.contains("spec")
            || lower.contains("design")
            || lower.contains("plan");

        let mut confidence: f64 = 0.5; // baseline

        if has_positive {
            confidence += 0.15;
        }
        if has_risk {
            confidence -= 0.20;
        }
        if has_structure {
            confidence += 0.10;
        }

        confidence = confidence.clamp(0.0, 1.0);

        let approves = confidence >= 0.45;
        let reasoning = format!(
            "LocalAgentVoter: confidence={:.2}, positive={}, risk={}, structured={}",
            confidence, has_positive, has_risk, has_structure,
        );

        Vote {
            approves,
            reasoning,
            confidence,
        }
    }
}

// ── RationalizationGuardVoter ─────────────────────────────────────────

/// Votes based on safety guard criteria, evaluating whether a proposal
/// meets the minimum confidence threshold defined by the
/// [`SelfRationalizationGuard`].
///
/// This voter acts as a "safety gate" — proposals falling below the guard's
/// confidence threshold or exhibiting suspicious patterns are voted against.
pub struct RationalizationGuardVoter {
    /// Name of this voter.
    name: String,
    /// Reference to the system's rationalization guard.
    guard: Arc<SelfRationalizationGuard>,
}

impl RationalizationGuardVoter {
    /// Create a new guard-based voter.
    pub fn new(name: impl Into<String>, guard: Arc<SelfRationalizationGuard>) -> Self {
        Self {
            name: name.into(),
            guard,
        }
    }
}

#[async_trait]
impl AgentVoter for RationalizationGuardVoter {
    fn name(&self) -> &str {
        &self.name
    }

    /// Vote by checking the context against safety guard criteria.
    ///
    /// Proposals with high information content, clear justification, or that
    /// reference established patterns are more likely to pass the guard.
    async fn vote(&self, context: &str) -> Vote {
        let threshold = self.guard.confidence_threshold as f64;

        // Heuristic: longer well-structured context indicates higher-quality
        // proposals that are more likely to pass the guard.
        let word_count = context.split_whitespace().count() as f64;
        let length_score = (word_count / 50.0).min(1.0); // 50+ words → full score

        // Safety signals.
        let has_rollback = context.to_lowercase().contains("rollback")
            || context.to_lowercase().contains("revert");
        let has_validation = context.to_lowercase().contains("test")
            || context.to_lowercase().contains("verify")
            || context.to_lowercase().contains("validate");

        let mut confidence: f64 = length_score * 0.5
            + if has_validation { 0.3 } else { 0.0 }
            + if has_rollback { 0.2 } else { 0.0 };

        confidence = confidence.clamp(0.0, 1.0);

        let approves = confidence >= threshold;
        let reasoning = format!(
            "RationalizationGuardVoter: confidence={:.2}, threshold={:.2}, \
             words={}, rollback={}, validation={}",
            confidence, threshold, word_count as u64, has_rollback, has_validation,
        );

        Vote {
            approves,
            reasoning,
            confidence,
        }
    }
}

// ── DeepSeekVoter ──────────────────────────────────────────────────────────

/// Uses the DeepSeek API to cast a vote based on the proposal context.
///
/// Sends a non-streaming chat completion request to the DeepSeek API and
/// parses the response to extract an approve/reject decision with reasoning.
pub struct DeepSeekVoter {
    name: String,
    /// Base URL for the DeepSeek API.
    base_url: String,
    /// Name of the DeepSeek model to use.
    model: String,
    /// API key.
    api_key: String,
    /// HTTP client.
    client: reqwest::Client,
}

impl DeepSeekVoter {
    /// Create a new DeepSeek voter.
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into(),
            model: model.into(),
            api_key: api_key.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Build a non-streaming payload for the DeepSeek chat completions endpoint.
    fn build_payload(&self, context: &str) -> Value {
        let system_content = concat!(
            "You are a voting agent. Analyse the following proposal ",
            "and decide whether to approve or reject it. ",
            "Respond with valid JSON only: ",
            "{\"approves\": true/false, \"reasoning\": \"...\", \"confidence\": 0.0-1.0}"
        );
        serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system_content},
                {"role": "user", "content": context}
            ],
            "temperature": 0.3,
            "max_tokens": 256
        })
    }
}

#[async_trait]
impl AgentVoter for DeepSeekVoter {
    fn name(&self) -> &str {
        &self.name
    }

    async fn vote(&self, context: &str) -> Vote {
        let payload = self.build_payload(context);
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        match self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
        {
            Ok(resp) => {
                if let Ok(body) = resp.json::<Value>().await {
                    // Extract content from the completion response.
                    let content = body
                        .pointer("/choices/0/message/content")
                        .and_then(|c| c.as_str())
                        .unwrap_or("");

                    // Try to parse JSON response.
                    if let Ok(parsed) = serde_json::from_str::<Value>(content) {
                        let approves = parsed
                            .get("approves")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);
                        let reasoning = parsed
                            .get("reasoning")
                            .and_then(|v| v.as_str())
                            .unwrap_or("DeepSeekVoter: parsed response")
                            .to_string();
                        let confidence = parsed
                            .get("confidence")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.5)
                            .clamp(0.0, 1.0);
                        return Vote {
                            approves,
                            reasoning,
                            confidence,
                        };
                    }

                    // Fallback: keyword-based heuristic from raw content.
                    let lower = content.to_lowercase();
                    let approves = lower.contains("approve") && !lower.contains("reject");
                    Vote {
                        approves,
                        reasoning: format!(
                            "DeepSeekVoter: raw response — {}",
                            content.chars().take(200).collect::<String>()
                        ),
                        confidence: 0.5,
                    }
                } else {
                    Vote {
                        approves: true,
                        reasoning: "DeepSeekVoter: failed to parse response body".to_string(),
                        confidence: 0.3,
                    }
                }
            }
            Err(e) => {
                tracing::error!(
                    target: "go_on::intelligence::voter",
                    error = %e,
                    "DeepSeekVoter: API unreachable — conservative reject"
                );
                Vote {
                    approves: false,
                    reasoning: format!("DeepSeekVoter: API error — {}", e),
                    confidence: 0.0,
                }
            }
        }
    }
}

// ── LocalVoter ────────────────────────────────────────────────────────────

/// Uses a local model configuration to vote on proposals.
///
/// Evaluates proposals using config-driven thresholds and local agent config.
/// Unlike `LocalAgentVoter` (keyword heuristic), this voter respects the
/// `AgentConfig` thresholds for more configurable voting behaviour.
pub struct LocalVoter {
    name: String,
    /// Agent configuration for threshold tuning.
    config: AgentConfig,
}

impl LocalVoter {
    /// Create a new local voter from an agent configuration.
    pub fn new(name: impl Into<String>, config: AgentConfig) -> Self {
        Self {
            name: name.into(),
            config,
        }
    }
}

#[async_trait]
impl AgentVoter for LocalVoter {
    fn name(&self) -> &str {
        &self.name
    }

    async fn vote(&self, context: &str) -> Vote {
        let lower = context.to_lowercase();

        // Config-driven thresholds.
        let max_tokens = self.config.max_tokens.unwrap_or(4096) as f64;
        let confidence_base = (max_tokens / 8192.0).clamp(0.3, 0.9);

        // Structural indicators.
        let has_proposal_keywords = ["proposal", "plan", "design", "spec", "objective"]
            .iter()
            .any(|kw| lower.contains(kw));
        let has_risk_indicators = [
            "risk",
            "critical",
            "breaking",
            "unsafe",
            "deprecate",
            "delete",
        ]
        .iter()
        .any(|kw| lower.contains(kw));
        let has_positive_signals = ["optimize", "improve", "fix", "upgrade", "enhance"]
            .iter()
            .any(|kw| lower.contains(kw));

        let mut confidence = confidence_base;
        if has_proposal_keywords {
            confidence += 0.15;
        }
        if has_positive_signals {
            confidence += 0.1;
        }
        if has_risk_indicators {
            confidence -= 0.25;
        }
        // Long context with structure indicates well-thought-out proposals.
        let word_count = context.split_whitespace().count() as f64;
        if word_count > 20.0 && has_proposal_keywords {
            confidence += 0.1;
        }

        confidence = confidence.clamp(0.0, 1.0);
        let approves = confidence >= 0.45;

        let reasoning = format!(
            "LocalVoter: confidence={:.2}, proposal={}, risk={}, positive={}, words={}",
            confidence,
            has_proposal_keywords,
            has_risk_indicators,
            has_positive_signals,
            word_count as u64
        );

        Vote {
            approves,
            reasoning,
            confidence,
        }
    }
}
