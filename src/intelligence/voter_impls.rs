//! Concrete [`AgentVoter`] implementations for the weighted-vote / Delphi-method
//! debate system.
//!
//! Provides three voter strategies:
//!
//! | Voter | Strategy |
//! |---|---|
//! | [`CapabilityBusVoter`] | Wraps `Arc<CapabilityBus>`, votes via capability matching |
//! | [`LocalAgentVoter`] | Keyword-heuristic voter using `contains` checks |
//! | [`RationalizationGuardVoter`] | Safety-guard voter based on confidence thresholds |

use std::sync::Arc;

use async_trait::async_trait;

use super::capability_bus::core::CapabilityBus;
use super::weighted_vote::{AgentVoter, Vote};
use crate::governance::rationalization::SelfRationalizationGuard;

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
    /// TODO-BLUE64: Wire into hub.rs Delphi debate path once async AgentVoter is integrated.
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
    /// TODO-BLUE64: Wire into hub.rs Delphi debate path once async AgentVoter is integrated.
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
    /// TODO-BLUE64: Wire into hub.rs Delphi debate path once async AgentVoter is integrated.
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
