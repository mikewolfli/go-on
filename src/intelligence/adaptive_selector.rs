//! Adaptive Model Selection - Learning-based model selection (Phase 10+)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const DEFAULT_EXPLORATION_BIAS: f32 = 0.8;
const DEFAULT_MAX_MODELS: usize = 1000;

/// Performance metrics for a model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetrics {
    pub model_id: String,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub success_rate: f32,
    pub last_updated_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelScoreSnapshot {
    pub model_id: String,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub success_rate: f32,
    pub ucb_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AdaptiveSelectorSnapshot {
    pub exploration_bias: f32,
    pub tracked_models: usize,
    pub total_observations: u64,
    pub models: Vec<ModelScoreSnapshot>,
}

/// Adaptive model selector with learning
pub struct AdaptiveModelSelector {
    metrics: HashMap<String, ModelMetrics>,
    exploration_bias: f32,
    max_models: usize,
}

impl AdaptiveModelSelector {
    pub fn new() -> Self {
        Self {
            metrics: HashMap::new(),
            exploration_bias: DEFAULT_EXPLORATION_BIAS,
            max_models: DEFAULT_MAX_MODELS,
        }
    }

    pub fn exploration_bias(&self) -> f32 {
        self.exploration_bias
    }

    pub fn set_exploration_bias(&mut self, bias: f32) {
        self.exploration_bias = bias.max(0.0);
    }

    pub fn record_result(&mut self, model_id: &str, success: bool) {
        // Evict the oldest entry when at capacity (model not already tracked).
        if !self.metrics.contains_key(model_id) && self.metrics.len() >= self.max_models {
            if let Some(oldest_key) = self
                .metrics
                .iter()
                .min_by_key(|(_, m)| m.last_updated_ms)
                .map(|(k, _)| k.clone())
            {
                self.metrics.remove(&oldest_key);
            }
        }

        let now = crate::intelligence::now_ms();
        let entry = self
            .metrics
            .entry(model_id.to_string())
            .or_insert_with(|| ModelMetrics {
                model_id: model_id.to_string(),
                total_requests: 0,
                successful_requests: 0,
                success_rate: 0.5,
                last_updated_ms: now,
            });

        entry.total_requests += 1;
        if success {
            entry.successful_requests += 1;
        }
        entry.success_rate = entry.successful_requests as f32 / entry.total_requests as f32;
        entry.last_updated_ms = now;
    }

    pub fn get_best_model(&self, candidates: &[String]) -> Option<String> {
        let mut best = None;
        let mut best_score = f32::MIN;

        for candidate in candidates {
            let score = self.ucb_score_for_model(Some(candidate));
            if score > best_score {
                best_score = score;
                best = Some(candidate.clone());
            }
        }

        best
    }

    pub fn is_degraded(&self, model_id: &str) -> bool {
        self.metrics
            .get(model_id)
            .map(|m| m.success_rate < 0.7)
            .unwrap_or(false)
    }

    pub fn rank_candidates(&self, candidates: &[(String, Option<String>)]) -> Vec<String> {
        let mut ranked = candidates
            .iter()
            .map(|(agent_name, model_id)| {
                (
                    agent_name.clone(),
                    self.ucb_score_for_model(model_id.as_deref()),
                )
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked
            .into_iter()
            .map(|(agent_name, _)| agent_name)
            .collect()
    }

    pub fn snapshot(&self) -> AdaptiveSelectorSnapshot {
        let mut models = self
            .metrics
            .values()
            .map(|entry| ModelScoreSnapshot {
                model_id: entry.model_id.clone(),
                total_requests: entry.total_requests,
                successful_requests: entry.successful_requests,
                success_rate: entry.success_rate,
                ucb_score: self.ucb_score_for_model(Some(&entry.model_id)),
            })
            .collect::<Vec<_>>();
        models.sort_by(|a, b| {
            b.ucb_score
                .total_cmp(&a.ucb_score)
                .then_with(|| a.model_id.cmp(&b.model_id))
        });

        AdaptiveSelectorSnapshot {
            exploration_bias: self.exploration_bias,
            tracked_models: models.len(),
            total_observations: self.total_observations(),
            models,
        }
    }

    fn total_observations(&self) -> u64 {
        self.metrics.values().map(|item| item.total_requests).sum()
    }

    fn ucb_score_for_model(&self, model_id: Option<&str>) -> f32 {
        let total = self.total_observations();
        let log_total = ((total + 1) as f32).ln();
        let exploration = self.exploration_bias;

        let Some(model_id) = model_id else {
            return 0.0;
        };

        match self.metrics.get(model_id) {
            Some(metrics) if metrics.total_requests > 0 => {
                let pulls = metrics.total_requests as f32;
                let bonus = if log_total > 0.0 {
                    exploration * (log_total / pulls).sqrt()
                } else {
                    0.0
                };
                metrics.success_rate + bonus
            }
            _ => {
                let unseen_bonus = if log_total > 0.0 {
                    exploration * log_total.sqrt()
                } else {
                    exploration
                };
                0.5 + unseen_bonus
            }
        }
    }
}

impl Default for AdaptiveModelSelector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_track_metrics() {
        let mut selector = AdaptiveModelSelector::new();
        selector.record_result("model-a", true);
        selector.record_result("model-a", true);
        selector.record_result("model-a", false);

        let metrics = selector.metrics.get("model-a").unwrap();
        assert_eq!(metrics.total_requests, 3);
        assert_eq!(metrics.successful_requests, 2);
    }

    #[test]
    fn test_best_model_selection() {
        let mut selector = AdaptiveModelSelector::new();

        for _ in 0..9 {
            selector.record_result("model-a", true);
        }
        selector.record_result("model-a", false);

        for _ in 0..5 {
            selector.record_result("model-b", true);
        }
        for _ in 0..5 {
            selector.record_result("model-b", false);
        }

        let best = selector.get_best_model(&["model-a".to_string(), "model-b".to_string()]);
        assert_eq!(best, Some("model-a".to_string()));
    }

    #[test]
    fn test_rank_candidates_uses_model_level_ucb_scores() {
        let mut selector = AdaptiveModelSelector::new();
        selector.set_exploration_bias(0.8);

        for _ in 0..10 {
            selector.record_result("stable-model", true);
        }
        selector.record_result("new-model", true);

        let ranked = selector.rank_candidates(&[
            ("agent-a".to_string(), Some("stable-model".to_string())),
            ("agent-b".to_string(), Some("new-model".to_string())),
        ]);

        assert_eq!(ranked.first(), Some(&"agent-b".to_string()));
    }

    #[test]
    fn test_snapshot_contains_sorted_ucb_scores() {
        let mut selector = AdaptiveModelSelector::new();
        selector.record_result("model-a", true);
        selector.record_result("model-b", false);

        let snapshot = selector.snapshot();
        assert_eq!(snapshot.tracked_models, 2);
        assert_eq!(snapshot.total_observations, 2);
        assert_eq!(snapshot.models.len(), 2);
        assert!(snapshot.models[0].ucb_score >= snapshot.models[1].ucb_score);
    }
}
