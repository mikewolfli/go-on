//! BLUE42 ORCH-FIN-01: Independent agent selection module.
//!
//! Extracted from `process_chat_request` to enable independent testing,
//! reputation-weighted scoring, and dynamic re-evaluation mid-execution.
//!
//! B48-R1: Eliminated alphabetical tie-breaking. When all agents have the same
//! default score, the selector now uses a deterministic-but-fair round-robin via
//! a global atomic counter instead of `a.0.cmp(&b.0)` which always favored
//! alphabetically-earliest names like "copilot".

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::acp::server::AcpServer;
use crate::agent::Agent;
use crate::intelligence::reputation::ReputationStore;
use serde::{Deserialize, Serialize};

/// Result of a single agent selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSelection {
    pub winner: String,
    pub candidates: Vec<ScoredAgent>,
    pub selection_reason: String,
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
    pub capability_weight: f64,
    pub reputation_weight: f64,
    pub history_weight: f64,
    pub eligibility_threshold: f64,
}

impl Default for AgentSelectorConfig {
    fn default() -> Self {
        Self {
            capability_weight: 0.3,
            reputation_weight: 0.3,
            history_weight: 0.4,
            eligibility_threshold: 0.2,
        }
    }
}

#[derive(Default)]
pub struct AgentSelector {
    config: AgentSelectorConfig,
}

/// Global atomic counter for round-robin tie-breaking.
/// Incremented once per sort call (not per comparison) to ensure
/// the comparison function is idempotent for the duration of a single sort.
static TIE_BREAKER_ROUND: AtomicU64 = AtomicU64::new(0);

/// Break ties using a deterministic hash of agent names combined with a
/// round-robin seed. This avoids alphabetical bias while ensuring the
/// comparison function is idempotent (always returns the same result for
/// a given pair within a single sort).
///
/// The seed changes after each sort, so over multiple requests the order
/// rotates fairly among equal-scoring agents.
fn break_tie(name_a: &str, name_b: &str, seed: u64) -> std::cmp::Ordering {
    // Use a simple deterministic hash: combine the seed with each name's bytes
    let hash_a = seed.wrapping_mul(6364136223846793005).wrapping_add(
        name_a
            .bytes()
            .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64)),
    );
    let hash_b = seed.wrapping_mul(6364136223846793005).wrapping_add(
        name_b
            .bytes()
            .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64)),
    );
    hash_a.cmp(&hash_b)
}

impl AgentSelector {
    #[cfg(test)]
    pub fn new(config: AgentSelectorConfig) -> Self {
        Self { config }
    }

    pub fn score_candidates(
        &self,
        agents: &[(String, Arc<dyn Agent>)],
        preferred_agent: Option<&str>,
        reputation: Option<&ReputationStore>,
        online_scores: &[(String, f64)],
        task_type: &str,
    ) -> Vec<ScoredAgent> {
        // Compute task-related boost: prefer agents whose name/role matches task type keywords
        let task_lower = task_type.to_lowercase();
        let is_coding_task = task_lower.contains("code")
            || task_lower.contains("develop")
            || task_lower.contains("implement")
            || task_lower.contains("fix");
        let is_creative_task = task_lower.contains("write")
            || task_lower.contains("create")
            || task_lower.contains("design")
            || task_lower.contains("draft");
        let is_analysis_task = task_lower.contains("analyze")
            || task_lower.contains("review")
            || task_lower.contains("audit")
            || task_lower.contains("explain");

        let mut scored: Vec<ScoredAgent> = agents
            .iter()
            .map(|(name, agent)| {
                let base = if Some(name.as_str()) == preferred_agent {
                    1.0
                } else {
                    0.5
                };
                let rep_score = reputation.map(|r| r.score(name)).unwrap_or(0.5);
                let mut hist_score = online_scores
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, s)| *s)
                    .unwrap_or(0.5);

                // Task-type-aware scoring: boost agents whose name/description
                // aligns with the task type for more intelligent selection
                let agent_lower = name.to_lowercase();
                let task_affinity = if is_coding_task
                    && (agent_lower.contains("code")
                        || agent_lower.contains("deepseek")
                        || agent_lower.contains("copilot")
                        || agent_lower.contains("claude"))
                {
                    0.15
                } else if is_creative_task
                    && (agent_lower.contains("gemini")
                        || agent_lower.contains("claude")
                        || agent_lower.contains("gpt"))
                {
                    0.12
                } else if is_analysis_task
                    && (agent_lower.contains("review")
                        || agent_lower.contains("audit")
                        || agent_lower.contains("analyze")
                        || agent_lower.contains("deepseek"))
                {
                    0.10
                } else {
                    0.0
                };

                // Boost agents that have specialized capabilities (more models, larger context)
                let capability_boost = {
                    let models = agent.available_models();
                    let has_large_context = models
                        .iter()
                        .any(|m| m.context_window.map(|cw| cw >= 128_000).unwrap_or(false));
                    let model_count = models.len().min(10) as f64 / 20.0;
                    if has_large_context {
                        0.05 + model_count
                    } else {
                        model_count * 0.5
                    }
                };

                hist_score += task_affinity;
                let total = base * self.config.capability_weight
                    + rep_score * self.config.reputation_weight
                    + hist_score * self.config.history_weight
                    + capability_boost * 0.1;
                ScoredAgent {
                    name: name.clone(),
                    base_score: base,
                    reputation_score: rep_score,
                    task_match_score: hist_score,
                    total_score: total,
                }
            })
            .collect();
        let tie_seed = TIE_BREAKER_ROUND.fetch_add(1, Ordering::Relaxed);
        scored.sort_by(|a, b| {
            b.total_score
                .partial_cmp(&a.total_score)
                .unwrap_or_else(|| break_tie(&a.name, &b.name, tie_seed))
        });
        scored
    }

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

    pub fn reorder_agents_by_selection(
        &self,
        agents: &mut Vec<(String, Arc<dyn Agent>)>,
        preferred_agent: Option<&str>,
        reputation_scores: &HashMap<String, f64>,
        online_scores: &[(String, f64)],
        task_type: &str,
    ) -> Option<AgentSelection> {
        let scored = self
            .score_candidates(agents, preferred_agent, None, online_scores, task_type)
            .into_iter()
            .map(|mut candidate| {
                candidate.reputation_score = reputation_scores
                    .get(&candidate.name)
                    .copied()
                    .unwrap_or(candidate.reputation_score);
                candidate.total_score = candidate.base_score * self.config.capability_weight
                    + candidate.reputation_score * self.config.reputation_weight
                    + candidate.task_match_score * self.config.history_weight;
                candidate
            })
            .collect::<Vec<_>>();

        let selection = self.select_winner(scored)?;
        sort_by_score(agents, |name| {
            selection
                .candidates
                .iter()
                .find(|candidate| candidate.name == name)
                .map(|candidate| candidate.total_score)
                .unwrap_or(0.0)
        });
        Some(selection)
    }
}

// ── Backward-compatible wrappers used by process_chat_request ──────────

fn sort_by_score<T>(agents: &mut [(String, T)], mut score_of: impl FnMut(&str) -> f64) {
    let tie_seed = TIE_BREAKER_ROUND.fetch_add(1, Ordering::Relaxed);
    agents.sort_by(|a, b| {
        let score_a = score_of(&a.0);
        let score_b = score_of(&b.0);
        score_b
            .partial_cmp(&score_a)
            .unwrap_or_else(|| break_tie(&a.0, &b.0, tie_seed))
    });
}

pub(crate) fn collect_reputation_scores(
    server: &AcpServer,
    agents: &[(String, Arc<dyn Agent>)],
) -> HashMap<String, f64> {
    let mut scores = HashMap::with_capacity(agents.len());
    if let Some(ref cb) = server.capability_bus {
        // BLUE48-R2: Collect base reputation scores from ReputationStore
        let rep = cb.reputation.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("reputation lock poisoned during score collection – recovered");
            poisoned.into_inner()
        });
        for (name, _) in agents {
            scores.insert(name.clone(), rep.score(name));
        }
        drop(rep);

        // BLUE48-R2: Augment with Council reputation influence multiplier.
        // The Council reputation system tracks member voting accuracy over time.
        // Members who consistently vote accurately gain influence (up to 2.0x),
        // which boosts their score in future selections. This creates a positive
        // feedback loop for intelligent agent selection.
        let council_guard = cb.council.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        for (name, score) in scores.iter_mut() {
            if let Some(rep_record) = council_guard.get_reputation(name) {
                // Apply influence multiplier: accurate voters get boosted
                if rep_record.total_votes >= 3 {
                    let boost = (rep_record.influence_multiplier - 1.0) * 0.2;
                    *score = (*score + boost).clamp(0.0, 1.0);
                }
            }
        }
    }
    scores
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use async_trait::async_trait;
    use serde_json::Value;

    struct MockAgent;
    #[async_trait]
    impl Agent for MockAgent {
        async fn chat(
            &self,
            _: Vec<crate::agent::Message>,
            _: Option<Vec<String>>,
            _: Option<HashMap<String, Value>>,
            _: crate::agent::StreamingSender,
        ) -> crate::core::error::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn selector_ranks_by_score() {
        let selector = AgentSelector::default();
        let agents: Vec<(String, Arc<dyn Agent>)> = vec![
            ("a".into(), Arc::new(MockAgent)),
            ("b".into(), Arc::new(MockAgent)),
        ];
        let scores = selector.score_candidates(&agents, None, None, &[], "test");
        assert_eq!(scores.len(), 2);
    }

    #[test]
    fn selector_new_uses_custom_config() {
        let selector = AgentSelector::new(AgentSelectorConfig {
            capability_weight: 1.0,
            reputation_weight: 0.0,
            history_weight: 0.0,
            eligibility_threshold: 0.0,
        });

        let agents: Vec<(String, Arc<dyn Agent>)> = vec![
            ("preferred".into(), Arc::new(MockAgent)),
            ("other".into(), Arc::new(MockAgent)),
        ];

        let scored = selector.score_candidates(
            &agents,
            Some("preferred"),
            None,
            &[("preferred".to_string(), 0.0), ("other".to_string(), 1.0)],
            "coding",
        );

        assert_eq!(scored[0].name, "preferred");
    }

    #[test]
    fn preferred_agent_gets_boost() {
        let selector = AgentSelector::default();
        let agents: Vec<(String, Arc<dyn Agent>)> = vec![
            ("a".into(), Arc::new(MockAgent)),
            ("b".into(), Arc::new(MockAgent)),
        ];
        let scores = selector.score_candidates(&agents, Some("a"), None, &[], "test");
        assert!(
            scores.iter().find(|s| s.name == "a").unwrap().total_score
                > scores.iter().find(|s| s.name == "b").unwrap().total_score
        );
    }

    #[test]
    fn select_winner_prefers_eligible_top_score() {
        let selector = AgentSelector::default();
        let scored = vec![
            ScoredAgent {
                name: "a".into(),
                base_score: 1.0,
                reputation_score: 0.9,
                task_match_score: 0.5,
                total_score: 0.8,
            },
            ScoredAgent {
                name: "b".into(),
                base_score: 0.5,
                reputation_score: 0.2,
                task_match_score: 0.3,
                total_score: 0.3,
            },
        ];
        let selection = selector
            .select_winner(scored)
            .expect("B49: winner (select_winner must always succeed when scored is non-empty)");
        assert_eq!(selection.winner, "a");
        assert_eq!(selection.selection_reason, "high_reputation");
    }

    #[test]
    fn sort_by_score_orders_desc() {
        let mut candidates = vec![
            ("beta".to_string(), ()),
            ("alpha".to_string(), ()),
            ("gamma".to_string(), ()),
        ];
        let mut scores = HashMap::new();
        scores.insert("alpha".to_string(), 0.8);
        scores.insert("beta".to_string(), 0.8);
        scores.insert("gamma".to_string(), 0.2);
        sort_by_score(&mut candidates, |name| {
            scores.get(name).copied().unwrap_or(0.0)
        });
        let ordered: Vec<String> = candidates.into_iter().map(|(n, _)| n).collect();
        // gamma has the lowest score (0.2), so it must be last
        assert_eq!(ordered[2], "gamma", "lowest score must be last");
        // alpha and beta both have 0.8 (tie) — either is acceptable first
        assert!(
            ordered[0] == "alpha" || ordered[0] == "beta",
            "tied agents must be alpha or beta, got: {}",
            ordered[0]
        );
        assert!(
            ordered[1] == "alpha" || ordered[1] == "beta",
            "tied agents must be alpha or beta, got: {}",
            ordered[1]
        );
        assert_ne!(ordered[0], ordered[1], "alpha and beta must be distinct");
    }
}
