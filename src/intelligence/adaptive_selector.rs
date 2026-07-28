//! Adaptive Model Selection - Learning-based model selection (Phase 10+)
//!
//! # I13: Context-Aware Contextual Bandit
//!
//! The UCB algorithm is extended with context features (time-of-day bucket,
//! task type category, and model latency tier) so that the selector learns
//! context-dependent policies.  Metrics are keyed by (model_id, context_hash)
//! so each arm has separate statistics per context.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::debug;

use crate::intelligence::model_selector::{
    ModelCharacteristics, ModelSelectionStrategy, ModelSelector, SelectionCriteria,
};

const DEFAULT_EXPLORATION_BIAS: f32 = 0.8;
const DEFAULT_MAX_MODELS: usize = 1000;

/// Minimum total observations before UCB selection is considered reliable.
/// Below this threshold, the static fallback strategy is used.
const COLD_START_OBSERVATION_THRESHOLD: u64 = 10;

// ---------------------------------------------------------------------------
// Context Features
// ---------------------------------------------------------------------------

/// Discretised context features that the bandit can use to learn
/// context-dependent arm policies.
#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub struct ContextFeatures {
    /// Hour-of-day bucket: "morning" (6-12), "afternoon" (12-18),
    /// "evening" (18-24), "night" (0-6).
    pub time_bucket: String,
    /// High-level task category (e.g. "chat", "code", "reasoning",
    /// "embedding").
    pub task_type: String,
    /// Model latency tier: "low" (<500ms), "medium" (500-2000ms),
    /// "high" (>2000ms).
    pub latency_tier: String,
}

impl ContextFeatures {
    /// Build features from raw values, discretising automatically.
    pub fn new(time_bucket: &str, task_type: &str, latency_tier: &str) -> Self {
        Self {
            time_bucket: time_bucket.to_string(),
            task_type: task_type.to_string(),
            latency_tier: latency_tier.to_string(),
        }
    }

    /// Produce a deterministic context hash string used as a key suffix.
    pub fn context_key(&self) -> String {
        format!(
            "ctx:{}|{}|{}",
            self.time_bucket, self.task_type, self.latency_tier
        )
    }

    /// Convenience: build context from current system time (UTC hour).
    pub fn from_time_and_task(task_type: &str) -> Self {
        let hour = chrono_hour();
        let time_bucket = match hour {
            0..=5 => "night",
            6..=11 => "morning",
            12..=17 => "afternoon",
            _ => "evening",
        };
        Self {
            time_bucket: time_bucket.to_string(),
            task_type: task_type.to_string(),
            latency_tier: "unknown".to_string(),
        }
    }
}

impl Default for ContextFeatures {
    fn default() -> Self {
        Self {
            time_bucket: "unknown".to_string(),
            task_type: "unknown".to_string(),
            latency_tier: "unknown".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Performance metrics for a model in a specific context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetrics {
    pub model_id: String,
    /// Context key that these metrics apply to (empty = global).
    pub context_key: String,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub success_rate: f32,
    pub last_updated_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelScoreSnapshot {
    pub model_id: String,
    pub context_key: String,
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

// ---------------------------------------------------------------------------
// Adaptive Model Selector
// ---------------------------------------------------------------------------

/// Adaptive model selector with context-aware UCB bandit learning.
///
/// When UCB exploration data is insufficient (cold start), falls back to a
/// static [`ModelSelectionStrategy`] provided at construction time.
#[derive(Debug)]
pub struct AdaptiveModelSelector {
    /// Metrics keyed by `"<model_id>::<context_key>"` for context-dependent stats.
    metrics: HashMap<String, ModelMetrics>,
    exploration_bias: f32,
    max_models: usize,
    /// Optional static strategy for fallback during cold start.
    static_strategy: Option<ModelSelectionStrategy>,
}

impl AdaptiveModelSelector {
    pub fn new() -> Self {
        Self {
            metrics: HashMap::new(),
            exploration_bias: DEFAULT_EXPLORATION_BIAS,
            max_models: DEFAULT_MAX_MODELS,
            static_strategy: None,
        }
    }

    /// Create a new selector with a static fallback strategy for cold starts.
    ///
    /// When UCB data is insufficient, [`select_with_static_fallback`] will
    /// delegate to [`ModelSelector::select_model`] using the given strategy.
    pub fn with_static_strategy(strategy: ModelSelectionStrategy) -> Self {
        Self {
            static_strategy: Some(strategy),
            ..Self::new()
        }
    }

    /// Returns `true` when total observations are below the cold-start threshold.
    pub fn is_cold_start(&self) -> bool {
        self.total_observations() < COLD_START_OBSERVATION_THRESHOLD
    }

    /// Returns the static fallback strategy, if configured.
    pub fn static_strategy(&self) -> Option<ModelSelectionStrategy> {
        self.static_strategy.clone()
    }

    pub fn exploration_bias(&self) -> f32 {
        self.exploration_bias
    }

    pub fn set_exploration_bias(&mut self, bias: f32) {
        self.exploration_bias = bias.max(0.0);
    }

    /// Internal key: `"<model_id>::<context_key>"`.
    fn metrics_key(model_id: &str, context_key: &str) -> String {
        format!("{}::{}", model_id, context_key)
    }

    // ── Record results (with optional context) ────────────────────────────

    /// Record a result with context features.
    ///
    /// When `context` is provided the bandit updates separate per-context
    /// statistics for the model, enabling context-dependent arm selection.
    /// When `None`, only the legacy global statistics are updated.
    pub fn record_result_with_context(
        &mut self,
        model_id: &str,
        success: bool,
        context: Option<&ContextFeatures>,
    ) {
        // Update global (legacy) metrics
        self.record_result(model_id, success);

        // Update per-context metrics if features are provided
        if let Some(ctx) = context {
            let ck = ctx.context_key();
            let key = Self::metrics_key(model_id, &ck);
            self.update_metrics(&key, model_id, &ck, success);
        }
    }

    /// Legacy: record result without context (global-only).
    pub fn record_result(&mut self, model_id: &str, success: bool) {
        let ck = String::new();
        let key = Self::metrics_key(model_id, &ck);
        self.update_metrics(&key, model_id, &ck, success);
    }

    fn update_metrics(&mut self, key: &str, model_id: &str, context_key: &str, success: bool) {
        // Evict the oldest entry when at capacity (model not already tracked).
        if !self.metrics.contains_key(key) && self.metrics.len() >= self.max_models {
            if let Some(oldest_key) = self
                .metrics
                .iter()
                .min_by_key(|(_, m)| m.last_updated_ms)
                .map(|(k, _)| k.clone())
            {
                self.metrics.remove(&oldest_key);
            }
        }

        let now = crate::shared::timestamps::now_ts_ms() as u64;
        let entry = self.metrics.entry(key.to_string()).or_insert_with(|| {
            let now = crate::shared::timestamps::now_ts_ms() as u64;
            ModelMetrics {
                model_id: model_id.to_string(),
                context_key: context_key.to_string(),
                total_requests: 0,
                successful_requests: 0,
                success_rate: 0.5,
                last_updated_ms: now,
            }
        });

        entry.total_requests += 1;
        if success {
            entry.successful_requests += 1;
        }
        entry.success_rate = entry.successful_requests as f32 / entry.total_requests as f32;
        entry.last_updated_ms = now;
    }

    // ── Context-aware selection ───────────────────────────────────────────

    /// Get the best model for a given context.
    pub fn get_best_model_with_context(
        &self,
        candidates: &[String],
        context: &ContextFeatures,
    ) -> Option<String> {
        let mut best = None;
        let mut best_score = f32::MIN;
        let ck = context.context_key();

        for candidate in candidates {
            let score = self.ucb_score_for_model_in_context(Some(candidate), &ck);
            if score > best_score {
                best_score = score;
                best = Some(candidate.clone());
            }
        }

        best
    }

    /// Legacy: get best model (global-only).
    pub fn get_best_model(&self, candidates: &[String]) -> Option<String> {
        self.get_best_model_with_context(candidates, &ContextFeatures::default())
    }

    /// Check if a model is degraded (global success rate < 0.7).
    pub fn is_degraded(&self, model_id: &str) -> bool {
        self.metrics
            .get(&Self::metrics_key(model_id, ""))
            .map(|m| m.success_rate < 0.7)
            .unwrap_or(false)
    }

    /// Rank candidates with context features.
    pub fn rank_candidates_with_context(
        &self,
        candidates: &[(String, Option<String>)],
        context: &ContextFeatures,
    ) -> Vec<String> {
        let ck = context.context_key();
        let mut ranked = candidates
            .iter()
            .map(|(agent_name, model_id)| {
                (
                    agent_name.clone(),
                    self.ucb_score_for_model_in_context(model_id.as_deref(), &ck),
                )
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked
            .into_iter()
            .map(|(agent_name, _)| agent_name)
            .collect()
    }

    /// Legacy: rank candidates (global-only).
    pub fn rank_candidates(&self, candidates: &[(String, Option<String>)]) -> Vec<String> {
        self.rank_candidates_with_context(candidates, &ContextFeatures::default())
    }

    // ── Static fallback selection ─────────────────────────────────────────

    /// Select the best model using UCB when data is sufficient, falling back
    /// to static [`ModelSelector::select_model`] during cold start.
    ///
    /// # Arguments
    /// * `criteria` - Selection criteria (used by static fallback).
    /// * `available_models` - All available model characteristics (used by
    ///   static fallback and as UCB candidate pool).
    /// * `context` - Optional context for context-aware UCB selection.
    ///
    /// # Returns
    /// * `Option<String>` - Selected model ID, or `None` if no suitable model.
    pub fn select_with_static_fallback(
        &self,
        criteria: &SelectionCriteria,
        available_models: &[ModelCharacteristics],
        context: Option<&ContextFeatures>,
    ) -> Option<String> {
        // Cold start: use static fallback strategy when insufficient UCB data
        if self.is_cold_start() {
            if let Some(ref strategy) = self.static_strategy {
                debug!(
                    total_observations = self.total_observations(),
                    threshold = COLD_START_OBSERVATION_THRESHOLD,
                    strategy = ?strategy,
                    "cold start: static model selection fallback"
                );
                return ModelSelector::select_model(criteria, available_models, strategy.clone());
            }
        }

        // Warm: use UCB on available model IDs as candidates
        let candidates: Vec<String> = available_models.iter().map(|m| m.id.clone()).collect();
        match context {
            Some(ctx) => self.get_best_model_with_context(&candidates, ctx),
            None => self.get_best_model(&candidates),
        }
    }

    pub fn snapshot(&self) -> AdaptiveSelectorSnapshot {
        let mut models = self
            .metrics
            .values()
            .map(|entry| ModelScoreSnapshot {
                model_id: entry.model_id.clone(),
                context_key: entry.context_key.clone(),
                total_requests: entry.total_requests,
                successful_requests: entry.successful_requests,
                success_rate: entry.success_rate,
                ucb_score: self
                    .ucb_score_for_model_in_context(Some(&entry.model_id), &entry.context_key),
            })
            .collect::<Vec<_>>();
        models.sort_by(|a, b| {
            b.ucb_score
                .total_cmp(&a.ucb_score)
                .then_with(|| a.model_id.cmp(&b.model_id))
                .then_with(|| a.context_key.cmp(&b.context_key))
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

    /// UCB score for a model in a specific context.
    /// Falls back to global metrics when no per-context metrics exist.
    fn ucb_score_for_model_in_context(&self, model_id: Option<&str>, context_key: &str) -> f32 {
        let total = self.total_observations();
        let log_total = ((total + 1) as f32).ln();
        let exploration = self.exploration_bias;

        let Some(model_id) = model_id else {
            return 0.0;
        };

        // Try per-context metrics first
        let ctx_key = Self::metrics_key(model_id, context_key);
        if let Some(metrics) = self.metrics.get(&ctx_key) {
            if metrics.total_requests > 0 {
                let pulls = metrics.total_requests as f32;
                let bonus = if log_total > 0.0 {
                    exploration * (log_total / pulls).sqrt()
                } else {
                    0.0
                };
                return metrics.success_rate + bonus;
            }
        }

        // Fall back to global metrics
        let global_key = Self::metrics_key(model_id, "");
        match self.metrics.get(&global_key) {
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

/// Helper: get the current hour (UTC) for context feature computation.
/// Returns 0..=23.
fn chrono_hour() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    // Days since epoch * 86400 + hour offset
    ((secs % 86400) / 3600) as u32
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_track_metrics() {
        let mut selector = AdaptiveModelSelector::new();
        selector.record_result("model-a", true);
        selector.record_result("model-a", true);
        selector.record_result("model-a", false);

        let m = selector
            .metrics
            .get(&AdaptiveModelSelector::metrics_key("model-a", ""));
        let metrics = m.expect("metrics should exist for model-a after recording results");
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

    // ── I13: Context-aware tests ──────────────────────────────────────────

    #[test]
    fn test_context_features_creates_deterministic_key() {
        let ctx = ContextFeatures::new("morning", "code", "low");
        let key = ctx.context_key();
        assert_eq!(key, "ctx:morning|code|low");
    }

    #[test]
    fn test_context_aware_record_separates_stats() {
        let mut selector = AdaptiveModelSelector::new();

        let morning_ctx = ContextFeatures::new("morning", "chat", "low");
        let evening_ctx = ContextFeatures::new("evening", "code", "high");

        // model-a: 10/10 success in morning, 1/10 success in evening
        for _ in 0..10 {
            selector.record_result_with_context("model-a", true, Some(&morning_ctx));
        }
        for _ in 0..9 {
            selector.record_result_with_context("model-a", false, Some(&evening_ctx));
        }
        selector.record_result_with_context("model-a", true, Some(&evening_ctx));

        // In morning context: model-a should be preferred
        let candidates = vec!["model-a".to_string()];
        let best_morning = selector.get_best_model_with_context(&candidates, &morning_ctx);
        assert_eq!(best_morning, Some("model-a".to_string()));

        let morning_score =
            selector.ucb_score_for_model_in_context(Some("model-a"), &morning_ctx.context_key());
        let evening_score =
            selector.ucb_score_for_model_in_context(Some("model-a"), &evening_ctx.context_key());

        // Morning should have higher success rate
        assert!(
            morning_score > evening_score,
            "morning score ({}) should exceed evening score ({}) \
             because model-a succeeds more often in the morning",
            morning_score,
            evening_score
        );
    }

    #[test]
    fn test_context_fallback_to_global_when_no_per_context_data() {
        let mut selector = AdaptiveModelSelector::new();

        // Only record global (legacy) results
        for _ in 0..10 {
            selector.record_result("model-a", true);
        }

        let ctx = ContextFeatures::new("afternoon", "reasoning", "medium");
        let candidates = vec!["model-a".to_string()];
        let best = selector.get_best_model_with_context(&candidates, &ctx);
        // Should still find model-a via global fallback
        assert_eq!(best, Some("model-a".to_string()));
    }

    #[test]
    fn test_rank_candidates_with_context() {
        let mut selector = AdaptiveModelSelector::new();
        selector.set_exploration_bias(0.8);

        let morning_ctx = ContextFeatures::new("morning", "chat", "low");
        let evening_ctx = ContextFeatures::new("evening", "code", "high");

        // model-a excels in morning, model-b excels in evening
        for _ in 0..20 {
            selector.record_result_with_context("model-a", true, Some(&morning_ctx));
            selector.record_result_with_context("model-b", false, Some(&morning_ctx));
            selector.record_result_with_context("model-a", false, Some(&evening_ctx));
            selector.record_result_with_context("model-b", true, Some(&evening_ctx));
        }

        let candidates = vec![
            ("agent-a".to_string(), Some("model-a".to_string())),
            ("agent-b".to_string(), Some("model-b".to_string())),
        ];

        let ranked_morning = selector.rank_candidates_with_context(&candidates, &morning_ctx);
        let ranked_evening = selector.rank_candidates_with_context(&candidates, &evening_ctx);

        // In morning: model-a should rank first
        assert_eq!(
            ranked_morning.first(),
            Some(&"agent-a".to_string()),
            "model-a should win in morning context"
        );
        // In evening: model-b should rank first
        assert_eq!(
            ranked_evening.first(),
            Some(&"agent-b".to_string()),
            "model-b should win in evening context"
        );
    }

    #[test]
    fn test_context_features_from_time() {
        let ctx = ContextFeatures::from_time_and_task("code");
        assert_eq!(ctx.task_type, "code");
        // time_bucket depends on actual wall clock, just verify it's one of the buckets
        assert!(
            ["morning", "afternoon", "evening", "night"].contains(&ctx.time_bucket.as_str()),
            "unexpected time bucket: {}",
            ctx.time_bucket
        );
    }
}
