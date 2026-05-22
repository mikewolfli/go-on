//! BLUE42 ORCH-FIN-01: Independent agent selection module.
//!
//! Extracted from `process_chat_request` to enable independent testing,
//! reputation-weighted scoring, and dynamic re-evaluation mid-execution.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::acp::server::AcpServer;
use crate::agent::Agent;
use crate::intelligence::reputation::ReputationStore;

/// Result of a single agent selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSelection {
    /// The selected agent name
    pub winner: String,
    /// All candidates considered (ordered by score)
    pub candidates: Vec<ScoredAgent>,
    /// Why this agent was selected
    pub selection_reason: String,
    /// Confidence in this selection (0.0–1.0)
    pub confidence: f64,
}

/// A candidate agent with its computed score
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredAgent {
    pub name: String,
    pub base_score: f64,
    pub reputation_score: f64,
    pub task_match_score: f64,
    pub total_score: f64,
}

/// Configuration for the agent selector
#[derive(Debug, Clone)]
pub struct AgentSelectorConfig {
    /// Weight of capability-bus recommendation (0.0–1.0)
    pub capability_weight: f64,
    /// Weight of reputation score (0.0–1.0)
    pub reputation_weight: f64,
    /// Weight of online-controller history (0.0–1.0)
    pub history_weight: f64,
    /// Minimum score for an agent to be eligible
    pub eligibility_threshold: f64,
}

// ── Backward-compatible wrappers used by process_chat_request ──────────mpl Default for AgentSelectorConfig {
    fn default() -> Self {
        Self {
            capability_weight: 0.3,
            reputation_weight: 0.3,
            history_weight: 0.4,
            eligibility_threshold: 0.2,
        }
    }
}

/// The agent selector, responsible for candidate collection, scoring, and ranking.
pub struct AgentSelector {
    config: AgentSelectorConfig,
}

impl AgentSelector {
    pub fn new(config: AgentSelectorConfig) -> Self {
        Self { config }
    }

    /// Score a list of candidate agents using capability, reputation, and history.
    pub fn score_candidates(
        &self,
        agents: &[(String, Arc<dyn Agent>)],
        preferred_agent: Option<&str>,
        reputation: Option<&ReputationStore>,
        online_scores: &[(String, f64)],
        task_type: &str,
    ) -> Vec<ScoredAgent> {
        let mut scored: Vec<ScoredAgent> = agents
            .iter()
            .map(|(name, _)| {
                let base = if Some(name.as_str()) == preferred_agent {
                    1.0
                } else {
                    0.5
                };
                let rep_score = reputation.and_then(|r| r.score_of(name)).unwrap_or(0.5);
                let hist_score = online_scores
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, s)| *s)
                    .unwrap_or(0.5);
                let total = base * self.config.capability_weight
                    + rep_score * self.config.reputation_weight
                    + hist_score * self.config.history_weight;

                ScoredAgent {
                    name: name.clone(),
                    base_score: base,
                    reputation_score: rep_score,
                    task_match_score: hist_score,
                    total_score: total,
                }
            })
            .collect();

        scored.sort_by(|a, b| {
            b.total_score
                .partial_cmp(&a.total_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored
    }

    /// Select the best agent from scored candidates.
    pub fn select_winner(&self, scored: Vec<ScoredAgent>) -> Option<AgentSelection> {
        let eligible: Vec<ScoredAgent> = scored
            .into_iter()
            .filter(|a| a.total_score >= self.config.eligibility_threshold)
            .collect();

        let winner = eligible.first()?;
        let reason = if winner.reputation_score >= 0.8 {
            "high_reputation"
        } else if winner.base_score > 0.9 {
            "capability_preferred"
        } else {
            "balanced_score"
        };

        Some(AgentSelection {
            winner: winner.name.clone(),
            candidates: eligible.clone(),
            selection_reason: reason.to_string(),
            confidence: winner.total_score,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use async_trait::async_trait;

    struct MockAgent;

    #[async_trait]
    impl Agent for MockAgent {
        async fn chat(
            &self,
            _: Vec<crate::agent::Message>,
            _: Option<Vec<String>>,
            _: Option<std::collections::HashMap<String, Value>>,
            _: crate::agent::StreamingSender,
        ) -> crate::core::error::Result<()> {
            Ok(())
        }
        fn name(&self) -> &str {
            "mock"
        }
    }

    #[test]
    fn selector_ranks_by_score() {
        let selector = AgentSelector::default();
        let agents: Vec<(String, Arc<dyn Agent>)> = vec![
            ("agent_a".into(), Arc::new(MockAgent)),
            ("agent_b".into(), Arc::new(MockAgent)),
        ];
        let scores = selector.score_candidates(&agents, None, None, &[], "test");
        assert_eq!(scores.len(), 2);
        assert!(scores[0].total_score > 0.0);
    }

    #[test]
    fn preferred_agent_gets_boost() {
        let selector = AgentSelector::default();
        let agents: Vec<(String, Arc<dyn Agent>)> = vec![
            ("agent_a".into(), Arc::new(MockAgent)),
            ("agent_b".into(), Arc::new(MockAgent)),
        ];
        let scores = selector.score_candidates(&agents, Some("agent_a"), None, &[], "test");
        let a = scores.iter().find(|s| s.name == "agent_a").unwrap();
        let b = scores.iter().find(|s| s.name == "agent_b").unwrap();
        assert!(a.total_score > b.total_score);
    }

    #[test]
    fn select_winner_filters_below_threshold() {
        let selector = AgentSelector::new(AgentSelectorConfig {
            eligibility_threshold: 0.8,
            ..Default::default()
        });
        let agents: Vec<(String, Arc<dyn Agent>)> = vec![("agent_a".into(), Arc::new(MockAgent))];
        let scores = selector.score_candidates(&agents, None, None, &[], "test");
        let result = selector.select_winner(scores);
        assert!(result.is_none()); // Score too low
    }
}
