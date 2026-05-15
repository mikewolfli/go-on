//! ARCH-10: Promotion Plugin System
//!
//! Pluggable promotion strategies that can be registered with CapabilityBus
//! and influence routing decisions based on agent performance, cost, or
//! other heuristic criteria.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::time::SystemTime;

/// Outcome of a promotion check
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PromotionDecision {
    /// Promote this agent (increase routing weight)
    Promote,
    /// Demote this agent (decrease routing weight)
    Demote,
    /// No change
    Neutral,
    /// Escalate for human review
    Escalate(String),
}

/// Context provided to a promotion plugin for evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionContext {
    /// The name of the agent being evaluated.
    pub agent_name: String,
    /// The type of task being routed.
    pub task_type: String,
    /// Quality score of the evidence produced (0.0–1.0).
    pub evidence_quality: f64,
    /// Historical success rate of the agent (0.0–1.0).
    pub success_rate: f64,
}

impl PromotionContext {
    pub fn new(
        agent_name: impl Into<String>,
        task_type: impl Into<String>,
        evidence_quality: f64,
        success_rate: f64,
    ) -> Self {
        Self {
            agent_name: agent_name.into(),
            task_type: task_type.into(),
            evidence_quality: evidence_quality.clamp(0.0, 1.0),
            success_rate: success_rate.clamp(0.0, 1.0),
        }
    }
}

/// The result of running a single promotion plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionResult {
    /// The agent that was evaluated.
    pub agent_name: String,
    /// The name of the plugin that produced this result.
    pub plugin_name: String,
    /// Whether the agent should be promoted.
    pub promoted: bool,
    /// The multiplier applied (1.0 means no change).
    pub multiplier: f64,
    /// Human-readable reason for the decision.
    pub reason: String,
}

impl PromotionResult {
    pub fn new(
        agent_name: impl Into<String>,
        plugin_name: impl Into<String>,
        promoted: bool,
        multiplier: f64,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            agent_name: agent_name.into(),
            plugin_name: plugin_name.into(),
            promoted,
            multiplier,
            reason: reason.into(),
        }
    }
}

/// Plugin trait: each promotion strategy implements this
pub trait PromotionPlugin: Send + Sync {
    fn name(&self) -> &'static str;

    /// Evaluate using the legacy per-field interface.
    fn evaluate(
        &self,
        agent: &str,
        success_rate: f64,
        avg_latency_ms: f64,
        cost_score: f64,
    ) -> PromotionDecision;

    /// Evaluate using a rich `PromotionContext`.
    /// The default implementation maps the context fields to the legacy interface.
    fn promote(&self, ctx: &PromotionContext) -> PromotionResult {
        let agent = &ctx.agent_name;
        let decision = self.evaluate(agent, ctx.success_rate, 0.0, 0.0);
        let (promoted, multiplier, reason) = match &decision {
            PromotionDecision::Promote => (true, 1.25, "Agent outperformed thresholds".into()),
            PromotionDecision::Demote => (false, 0.75, "Agent underperformed thresholds".into()),
            PromotionDecision::Neutral => (false, 1.0, "No action needed".into()),
            PromotionDecision::Escalate(msg) => (false, 1.0, format!("Escalated: {msg}")),
        };
        PromotionResult::new(agent, self.name(), promoted, multiplier, reason)
    }
}

/// A simple threshold-based promotion plugin
pub struct ThresholdPromotion {
    pub min_success_rate: f64,
    pub max_latency_ms: f64,
    pub max_cost_score: f64,
}

impl ThresholdPromotion {
    pub fn new(min_success_rate: f64, max_latency_ms: f64, max_cost_score: f64) -> Self {
        Self {
            min_success_rate,
            max_latency_ms,
            max_cost_score,
        }
    }
}

impl Default for ThresholdPromotion {
    fn default() -> Self {
        Self {
            min_success_rate: 0.8,
            max_latency_ms: 5000.0,
            max_cost_score: 0.7,
        }
    }
}

impl PromotionPlugin for ThresholdPromotion {
    fn name(&self) -> &'static str {
        "threshold_promotion"
    }

    fn evaluate(
        &self,
        _agent: &str,
        success_rate: f64,
        avg_latency_ms: f64,
        cost_score: f64,
    ) -> PromotionDecision {
        let mut flags = Vec::new();
        if success_rate < self.min_success_rate {
            flags.push("low_success_rate");
        }
        if avg_latency_ms > self.max_latency_ms {
            flags.push("high_latency");
        }
        if cost_score > self.max_cost_score {
            flags.push("high_cost");
        }

        if flags.len() >= 2 {
            PromotionDecision::Demote
        } else if flags.is_empty() && success_rate >= self.min_success_rate + 0.15 {
            PromotionDecision::Promote
        } else {
            PromotionDecision::Neutral
        }
    }

    fn promote(&self, ctx: &PromotionContext) -> PromotionResult {
        let agent = &ctx.agent_name;
        // Map context into the legacy interface fields.
        let decision = self.evaluate(agent, ctx.success_rate, 0.0, 0.0);
        let (promoted, multiplier, reason) = match decision {
            PromotionDecision::Promote => (true, 1.25, "Agent outperformed thresholds".into()),
            PromotionDecision::Demote => (false, 0.75, "Agent underperformed thresholds".into()),
            PromotionDecision::Neutral => (false, 1.0, "No action needed".into()),
            PromotionDecision::Escalate(msg) => (false, 1.0, format!("Escalated: {msg}")),
        };
        PromotionResult::new(agent, self.name(), promoted, multiplier, reason)
    }
}

#[cfg(test)]
/// Evidence-weighted promotion plugin.
/// Promotes agents based on the quality of evidence they produce.
///
/// When `evidence_quality` exceeds a configurable threshold, the promotion
/// multiplier is scaled linearly up to a configurable maximum.  All promotion
/// events are tracked in an internal history map.
pub struct EvidenceWeightedPromotion {
    /// Minimum evidence quality required to trigger promotion (0.0–1.0).
    pub threshold: f64,
    /// Maximum multiplier applied when evidence quality is 1.0.
    pub max_multiplier: f64,
    /// Promotion history keyed by agent name.
    history: Mutex<HashMap<String, Vec<PromotionHistoryEntry>>>,
}

#[cfg(test)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PromotionHistoryEntry {
    timestamp: String,
    evidence_quality: f64,
    multiplier: f64,
    reason: String,
}

#[cfg(test)]
impl EvidenceWeightedPromotion {
    pub fn new(threshold: f64, max_multiplier: f64) -> Self {
        Self {
            threshold: threshold.clamp(0.0, 1.0),
            max_multiplier,
            history: Mutex::new(HashMap::new()),
        }
    }
}

#[cfg(test)]
impl Default for EvidenceWeightedPromotion {
    fn default() -> Self {
        Self {
            threshold: 0.6,
            max_multiplier: 1.5,
            history: Mutex::new(HashMap::new()),
        }
    }
}

#[cfg(test)]
impl PromotionPlugin for EvidenceWeightedPromotion {
    fn name(&self) -> &'static str {
        "evidence_weighted_promotion"
    }

    fn evaluate(
        &self,
        _agent: &str,
        _success_rate: f64,
        _avg_latency_ms: f64,
        _cost_score: f64,
    ) -> PromotionDecision {
        // Legacy evaluate is not the primary interface for this plugin;
        // use promote() instead.  Return Neutral by default.
        PromotionDecision::Neutral
    }

    fn promote(&self, ctx: &PromotionContext) -> PromotionResult {
        let eq = ctx.evidence_quality;
        let agent = &ctx.agent_name;

        if eq > self.threshold {
            // Scale multiplier linearly from 1.0 at threshold to max_multiplier at 1.0.
            let range = 1.0 - self.threshold; // > 0 because eq > threshold
            let fraction = (eq - self.threshold) / range;
            let multiplier = 1.0 + fraction * (self.max_multiplier - 1.0);
            let reason = format!(
                "Evidence quality {:.2} exceeds threshold {:.2}; multiplier {:.2}",
                eq, self.threshold, multiplier
            );

            // Track in history.
            if let Ok(mut hist) = self.history.lock() {
                hist.entry(agent.clone())
                    .or_default()
                    .push(PromotionHistoryEntry {
                        timestamp: format!("{:?}", SystemTime::now())
                            .trim_matches('"')
                            .to_string(),
                        evidence_quality: eq,
                        multiplier,
                        reason: reason.clone(),
                    });
                // Cap history at 100 entries per agent
                if let Some(entries) = hist.get_mut(agent.as_str()) {
                    if entries.len() > 100 {
                        entries.drain(0..entries.len() - 100);
                    }
                }
            }

            PromotionResult::new(agent, self.name(), true, multiplier, reason)
        } else {
            let reason = format!(
                "Evidence quality {:.2} does not meet threshold {:.2}",
                eq, self.threshold
            );
            PromotionResult::new(agent, self.name(), false, 1.0, reason)
        }
    }
}

#[cfg(test)]
impl EvidenceWeightedPromotion {
    /// Return a snapshot of the promotion history for a given agent.
    pub fn history_for(&self, agent: &str) -> Vec<PromotionHistoryEntry> {
        self.history
            .lock()
            .ok()
            .and_then(|h| h.get(agent).cloned())
            .unwrap_or_default()
    }

    /// Return a snapshot of the full promotion history.
    #[allow(dead_code)] // F-GAP-13 — reserved for diagnostic / observability endpoint
    pub fn all_history(&self) -> HashMap<String, Vec<PromotionHistoryEntry>> {
        self.history
            .lock()
            .ok()
            .map(|h| h.clone())
            .unwrap_or_default()
    }
}

/// Thread-safe registry of promotion plugins.
///
/// Wraps plugins in `Arc<Mutex<...>>` so the registry can be shared across
/// threads (e.g. with `CapabilityBus`).
pub struct PluginRegistry {
    plugins: Vec<Box<dyn PromotionPlugin>>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginRegistry {
    pub fn new() -> Self {
        let mut reg = Self {
            plugins: Vec::new(),
        };
        reg.register(Box::new(ThresholdPromotion::new(0.8, 5000.0, 0.7)));
        reg
    }

    pub fn register(&mut self, plugin: Box<dyn PromotionPlugin>) {
        self.plugins.push(plugin);
    }

    pub fn promote_all(&self, ctx: &PromotionContext) -> Vec<PromotionResult> {
        self.plugins.iter().map(|p| p.promote(ctx)).collect()
    }

    pub fn list_plugins(&self) -> Vec<String> {
        self.plugins.iter().map(|p| p.name().to_string()).collect()
    }

    pub fn evaluate_all(
        &self,
        agent: &str,
        success_rate: f64,
        avg_latency_ms: f64,
        cost_score: f64,
    ) -> Vec<PromotionDecision> {
        self.plugins
            .iter()
            .map(|p| p.evaluate(agent, success_rate, avg_latency_ms, cost_score))
            .collect()
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    /// Wrap the registry in an `Arc<Mutex<Self>>` for thread-safe sharing.
    pub fn into_shared(self) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(self))
    }
}

/// Alias for backward compatibility.
pub type PromotionRegistry = PluginRegistry;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_promotion_context_clamps_values() {
        let ctx = PromotionContext::new("agent-a", "code_review", 1.5, -0.3);
        assert_eq!(ctx.evidence_quality, 1.0);
        assert_eq!(ctx.success_rate, 0.0);
    }

    #[test]
    fn test_promotion_result_new() {
        let r = PromotionResult::new("agent-a", "test_plugin", true, 1.5, "high quality");
        assert_eq!(r.agent_name, "agent-a");
        assert!(r.promoted);
        assert!((r.multiplier - 1.5).abs() < 1e-10);
    }

    #[test]
    fn test_threshold_promotion_promotes_high_performer() {
        let plugin = ThresholdPromotion::default();
        let decision = plugin.evaluate("agent-a", 0.98, 500.0, 0.2);
        assert_eq!(decision, PromotionDecision::Promote);
    }

    #[test]
    fn test_threshold_promotion_demotes_low_performer() {
        let plugin = ThresholdPromotion::default();
        let decision = plugin.evaluate("agent-b", 0.5, 10000.0, 0.9);
        assert_eq!(decision, PromotionDecision::Demote);
    }

    #[test]
    fn test_threshold_promotion_neutral_on_single_flag() {
        let plugin = ThresholdPromotion::default();
        let decision = plugin.evaluate("agent-c", 0.7, 500.0, 0.3);
        assert_eq!(decision, PromotionDecision::Neutral);
    }

    #[test]
    fn test_registry_evaluates_all_plugins() {
        let reg = PluginRegistry::new();
        let decisions = reg.evaluate_all("agent-a", 0.95, 1000.0, 0.3);
        assert_eq!(decisions.len(), 1);
    }

    #[test]
    fn test_evidence_weighted_promotion_promotes_above_threshold() {
        let plugin = EvidenceWeightedPromotion::new(0.5, 2.0);
        let ctx = PromotionContext::new("agent-a", "code_review", 0.9, 0.8);
        let result = plugin.promote(&ctx);
        assert!(result.promoted);
        assert!(result.multiplier > 1.0);
        assert_eq!(result.plugin_name, "evidence_weighted_promotion");
    }

    #[test]
    fn test_evidence_weighted_promotion_does_not_promote_below_threshold() {
        let plugin = EvidenceWeightedPromotion::new(0.7, 2.0);
        let ctx = PromotionContext::new("agent-b", "qa", 0.4, 0.9);
        let result = plugin.promote(&ctx);
        assert!(!result.promoted);
        assert!((result.multiplier - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_evidence_weighted_promotion_tracks_history() {
        let plugin = EvidenceWeightedPromotion::new(0.5, 2.0);
        let ctx = PromotionContext::new("agent-c", "research", 0.85, 0.7);
        let _ = plugin.promote(&ctx);
        let history = plugin.history_for("agent-c");
        assert_eq!(history.len(), 1);
        assert!((history[0].evidence_quality - 0.85).abs() < 1e-10);
    }

    #[test]
    fn test_plugin_registry_promote_all() {
        let mut reg = PluginRegistry::new();
        reg.register(Box::new(EvidenceWeightedPromotion::new(0.5, 1.5)));
        let ctx = PromotionContext::new("agent-d", "analysis", 0.92, 0.95);
        let results = reg.promote_all(&ctx);
        // threshold_promotion + evidence_weighted_promotion
        assert_eq!(results.len(), 2);
        let ew_result = results
            .iter()
            .find(|r| r.plugin_name == "evidence_weighted_promotion")
            .unwrap();
        assert!(ew_result.promoted);
    }

    #[test]
    fn test_plugin_registry_list_plugins() {
        let reg = PluginRegistry::new();
        let names = reg.list_plugins();
        assert!(names.contains(&"threshold_promotion".to_string()));
    }

    #[test]
    fn test_plugin_registry_into_shared() {
        let reg = PluginRegistry::new();
        let shared = reg.into_shared();
        let guard = shared.lock().unwrap();
        assert_eq!(guard.plugin_count(), 1);
    }

    #[test]
    fn test_threshold_promotion_promote_uses_promotion_context() {
        let plugin = ThresholdPromotion::default();
        let ctx = PromotionContext::new("agent-a", "code", 0.0, 0.98);
        let result = plugin.promote(&ctx);
        assert!(result.promoted);
        assert_eq!(result.agent_name, "agent-a");
    }
}
