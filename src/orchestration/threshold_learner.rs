//! Dynamic threshold learner for skill matching.
//!
//! Learns the optimal match-score threshold for each metric over time
//! using EMA-smoothed adjustments based on trial outcomes. This replaces
//! a static `DEFAULT_MIN_MATCH_SCORE` with an online learning system
//! that reduces false positives and missed matches.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single trial recording a threshold decision and its outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThresholdTrial {
    /// The metric being tuned (e.g., "skill_match", "tool_match").
    pub metric: String,
    /// The threshold value that was used for this trial.
    pub threshold: f64,
    /// Whether the outcome was successful.
    pub success: bool,
    /// Whether this was a false positive (matched but shouldn't have).
    pub false_positive: bool,
    /// Whether this was a missed match (should have matched but didn't).
    pub missed_match: bool,
}

/// Online learner that maintains an EMA-smoothed optimal threshold
/// per metric.
///
/// # How it works
///
/// - Starts at a configurable initial threshold (default 0.40).
/// - Records each (threshold, outcome) trial.
/// - **False positives** push the threshold **up** (be more selective).
/// - **Missed matches** push the threshold **down** (be more inclusive).
/// - The adjustment uses EMA with a configurable learning rate.
#[derive(Debug, Clone)]
pub struct ThresholdLearner {
    /// EMA-smoothed current best threshold per metric.
    thresholds: HashMap<String, f64>,
    /// History of recent (threshold, outcome) pairs for diagnostics.
    history: Vec<ThresholdTrial>,
    /// Learning rate for EMA updates (0.0 – 1.0).
    learning_rate: f64,
    /// Initial threshold for new metrics.
    initial_threshold: f64,
    /// Maximum history size before eviction.
    max_history: usize,
}

impl ThresholdLearner {
    /// Create a new learner with the given learning rate and initial threshold.
    pub fn new(learning_rate: f64, initial_threshold: f64) -> Self {
        Self {
            thresholds: HashMap::new(),
            history: Vec::new(),
            learning_rate,
            initial_threshold,
            max_history: 500,
        }
    }

    /// Create a learner with default settings: learning_rate = 0.15, initial = 0.40.
    pub fn default_learner() -> Self {
        Self::new(0.15, 0.40)
    }

    /// Record a trial outcome and adjust the threshold for the given metric.
    ///
    /// - `success`: true if the matched skill/tool produced a good result.
    /// - `false_positive`: true if the match was incorrect (matched a skill
    ///   that turned out to be inappropriate).
    /// - `missed_match`: true if a skill that should have matched was missed
    ///   (reflects threshold being too high).
    pub fn record_trial(
        &mut self,
        metric: &str,
        threshold: f64,
        success: bool,
        false_positive: bool,
        missed_match: bool,
    ) {
        // Push history.
        let trial = ThresholdTrial {
            metric: metric.to_string(),
            threshold,
            success,
            false_positive,
            missed_match,
        };
        self.history.push(trial);
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }

        // Adjust threshold.
        let current = self
            .thresholds
            .get(metric)
            .copied()
            .unwrap_or(self.initial_threshold);

        let new = if false_positive {
            // Too many false positives — raise threshold to be more selective.
            // Adjustment is proportional to how far below 1.0 we are.
            let gap = 1.0 - current;
            (current + self.learning_rate * gap).min(0.95)
        } else if missed_match {
            // Too many missed matches — lower threshold to be more inclusive.
            let gap = current - 0.10;
            (current - self.learning_rate * gap).max(0.10)
        } else if success {
            // Successful match — reinforce current threshold lightly.
            current
        } else {
            // Failure without clear signal — slightly relax threshold.
            let gap = current - 0.10;
            (current - self.learning_rate * 0.3 * gap).max(0.10)
        };

        self.thresholds.insert(metric.to_string(), new);
    }

    /// Get the current EMA-smoothed optimal threshold for a metric.
    ///
    /// Returns `initial_threshold` if no trials have been recorded yet.
    pub fn get_optimal_threshold(&self, metric: &str) -> f64 {
        self.thresholds
            .get(metric)
            .copied()
            .unwrap_or(self.initial_threshold)
    }

    /// Manually adjust the threshold by a delta value.
    ///
    /// This can be used for offline tuning or admin overrides.
    /// The threshold is clamped to [0.10, 0.95].
    pub fn adjust_threshold(&mut self, metric: &str, delta: f64) {
        let current = self.get_optimal_threshold(metric);
        let new = (current + delta).clamp(0.10, 0.95);
        self.thresholds.insert(metric.to_string(), new);
    }

    /// Get the learning rate.
    pub fn learning_rate(&self) -> f64 {
        self.learning_rate
    }

    /// Set a new learning rate.
    pub fn set_learning_rate(&mut self, rate: f64) {
        self.learning_rate = rate.clamp(0.01, 0.50);
    }

    /// Get a copy of the trial history for diagnostics.
    pub fn history(&self) -> &[ThresholdTrial] {
        &self.history
    }

    /// Number of trials recorded so far.
    pub fn trial_count(&self) -> usize {
        self.history.len()
    }

    /// Return a snapshot of all learned thresholds.
    pub fn all_thresholds(&self) -> HashMap<String, f64> {
        self.thresholds.clone()
    }
}

impl Default for ThresholdLearner {
    fn default() -> Self {
        Self::default_learner()
    }
}

// ── tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_initial_threshold() {
        let learner = ThresholdLearner::default_learner();
        assert_eq!(learner.get_optimal_threshold("skill_match"), 0.40);
    }

    #[test]
    fn false_positive_raises_threshold() {
        let mut learner = ThresholdLearner::new(0.20, 0.40);
        learner.record_trial("skill_match", 0.40, false, true, false);
        // After false positive, threshold should increase.
        assert!(learner.get_optimal_threshold("skill_match") > 0.40);
    }

    #[test]
    fn missed_match_lowers_threshold() {
        let mut learner = ThresholdLearner::new(0.20, 0.50);
        learner.record_trial("skill_match", 0.50, false, false, true);
        // After missed match, threshold should decrease.
        assert!(learner.get_optimal_threshold("skill_match") < 0.50);
    }

    #[test]
    fn success_reinforces_current_threshold() {
        let mut learner = ThresholdLearner::new(0.20, 0.40);
        let before = learner.get_optimal_threshold("skill_match");
        learner.record_trial("skill_match", 0.40, true, false, false);
        assert_eq!(learner.get_optimal_threshold("skill_match"), before);
    }

    #[test]
    fn threshold_is_clamped() {
        let mut learner = ThresholdLearner::new(0.50, 0.40);
        // Simulate many missed matches to try to push below 0.10
        for _ in 0..50 {
            learner.record_trial("skill_match", 0.40, false, false, true);
        }
        assert!(learner.get_optimal_threshold("skill_match") >= 0.10);

        // Simulate many false positives to try to push above 0.95
        for _ in 0..50 {
            learner.record_trial("skill_match", 0.40, false, true, false);
        }
        assert!(learner.get_optimal_threshold("skill_match") <= 0.95);
    }

    #[test]
    fn manual_adjust_works() {
        let mut learner = ThresholdLearner::default_learner();
        assert_eq!(learner.get_optimal_threshold("skill_match"), 0.40);
        learner.adjust_threshold("skill_match", 0.10);
        assert_eq!(learner.get_optimal_threshold("skill_match"), 0.50);
        learner.adjust_threshold("skill_match", -0.05);
        assert_eq!(learner.get_optimal_threshold("skill_match"), 0.45);
    }

    #[test]
    fn history_is_tracked() {
        let mut learner = ThresholdLearner::new(0.10, 0.40);
        learner.record_trial("a", 0.40, true, false, false);
        learner.record_trial("b", 0.40, false, true, false);
        assert_eq!(learner.trial_count(), 2);
        assert_eq!(learner.history()[0].metric, "a");
        assert_eq!(learner.history()[1].metric, "b");
    }

    #[test]
    fn multiple_metrics_are_independent() {
        let mut learner = ThresholdLearner::default_learner();
        learner.record_trial("skill_match", 0.40, false, true, false);
        assert_ne!(
            learner.get_optimal_threshold("skill_match"),
            learner.get_optimal_threshold("tool_match")
        );
    }
}
