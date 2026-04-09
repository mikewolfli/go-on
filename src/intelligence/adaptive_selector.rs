//! Adaptive Model Selection - Learning-based model selection (Phase 10+)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Performance metrics for a model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetrics {
    pub model_id: String,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub success_rate: f32,
}

/// Adaptive model selector with learning
pub struct AdaptiveModelSelector {
    metrics: HashMap<String, ModelMetrics>,
}

impl AdaptiveModelSelector {
    pub fn new() -> Self {
        Self {
            metrics: HashMap::new(),
        }
    }

    pub fn record_result(&mut self, model_id: &str, success: bool) {
        let entry = self
            .metrics
            .entry(model_id.to_string())
            .or_insert_with(|| ModelMetrics {
                model_id: model_id.to_string(),
                total_requests: 0,
                successful_requests: 0,
                success_rate: 0.5,
            });

        entry.total_requests += 1;
        if success {
            entry.successful_requests += 1;
        }
        entry.success_rate = entry.successful_requests as f32 / entry.total_requests as f32;
    }

    pub fn get_best_model(&self, candidates: &[String]) -> Option<String> {
        let mut best = None;
        let mut best_score = 0.0f32;

        for candidate in candidates {
            if let Some(metrics) = self.metrics.get(candidate) {
                if metrics.success_rate > best_score {
                    best_score = metrics.success_rate;
                    best = Some(candidate.clone());
                }
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
}
