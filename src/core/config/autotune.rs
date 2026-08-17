use std::fs;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};

/// Autotune configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AutoTuneConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_autotune_evaluate_interval")]
    pub evaluate_interval: usize,
    #[serde(default = "default_autotune_min_query_chars_step")]
    pub min_query_chars_step: usize,
    #[serde(default = "default_autotune_min_query_chars_min")]
    pub min_query_chars_min: usize,
    #[serde(default = "default_autotune_min_query_chars_max")]
    pub min_query_chars_max: usize,
    #[serde(default = "default_autotune_max_top_k")]
    pub max_top_k: usize,
    #[serde(default = "default_autotune_low_precision")]
    pub low_precision_threshold: f32,
    #[serde(default = "default_autotune_high_precision")]
    pub high_precision_threshold: f32,
    #[serde(default = "default_autotune_state_path")]
    pub state_path: String,
    #[serde(default = "default_autotune_cooldown_windows")]
    pub cooldown_windows: usize,
    #[serde(default = "default_autotune_min_vector_searches")]
    pub min_vector_searches: usize,
    #[serde(default = "default_autotune_summary_trigger_min")]
    pub summary_trigger_min: usize,
    #[serde(default = "default_autotune_summary_trigger_max")]
    pub summary_trigger_max: usize,
}

pub(crate) fn default_autotune_evaluate_interval() -> usize {
    20
}
pub(crate) fn default_autotune_min_query_chars_step() -> usize {
    20
}
pub(crate) fn default_autotune_min_query_chars_min() -> usize {
    40
}
pub(crate) fn default_autotune_min_query_chars_max() -> usize {
    300
}
pub(crate) fn default_autotune_max_top_k() -> usize {
    4
}
pub(crate) fn default_autotune_low_precision() -> f32 {
    0.35
}
pub(crate) fn default_autotune_high_precision() -> f32 {
    0.75
}
pub(crate) fn default_autotune_state_path() -> String {
    "sqlite3/acp_autotune_state.json".to_string()
}
pub(crate) fn default_autotune_cooldown_windows() -> usize {
    2
}
pub(crate) fn default_autotune_min_vector_searches() -> usize {
    5
}
pub(crate) fn default_autotune_summary_trigger_min() -> usize {
    3
}
pub(crate) fn default_autotune_summary_trigger_max() -> usize {
    20
}

/// Runtime autotune state: tracks current parameter values and precision feedback metrics.
/// Persisted to JSON file at state_path to survive across server restarts.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AutoTuneState {
    /// Current minimum query character threshold for vector searches.
    pub current_min_query_chars: usize,
    /// Current top-k value for vector result limiting.
    pub current_top_k: usize,
    /// Which evaluation window we're in (incremented every evaluate_interval searches).
    pub window_phase: usize,
    /// Number of vector searches with high precision (above high_precision_threshold).
    pub high_precision_count: usize,
    /// Number of vector searches with low precision (below low_precision_threshold).
    pub low_precision_count: usize,
    /// Total vector searches in current window.
    pub vector_search_count: usize,
    /// Windows remaining before next adjustment is allowed (cooldown logic).
    pub cooldown_remaining: usize,
}

impl AutoTuneState {
    /// Create new state from AutoTuneConfig defaults.
    pub fn new(config: &AutoTuneConfig) -> Self {
        Self {
            current_min_query_chars: config.min_query_chars_min,
            current_top_k: 2, // Conservative initial value
            window_phase: 0,
            high_precision_count: 0,
            low_precision_count: 0,
            vector_search_count: 0,
            cooldown_remaining: 0,
        }
    }

    /// Load state from JSON file, or return new default if file doesn't exist.
    pub fn load_or_default(path: &str, config: &AutoTuneConfig) -> Self {
        match fs::read_to_string(path) {
            Ok(content) => match serde_json::from_str::<AutoTuneState>(&content) {
                Ok(state) => state,
                Err(e) => {
                    warn!(
                        "failed to parse autotune state from {}: {}, using defaults",
                        path, e
                    );
                    Self::new(config)
                }
            },
            Err(_) => Self::new(config),
        }
    }

    /// Save state to JSON file.
    pub fn save(&self, path: &str) -> Result<()> {
        let json =
            serde_json::to_string_pretty(self).context("failed to serialize autotune state")?;
        fs::write(path, json).context("failed to write autotune state to file")?;
        Ok(())
    }

    /// Record a vector search result with precision score.
    /// Called after each vector search to update metrics.
    pub fn record_vector_search(&mut self, precision: f32, config: &AutoTuneConfig) {
        if precision >= config.high_precision_threshold {
            self.high_precision_count += 1;
        } else if precision < config.low_precision_threshold {
            self.low_precision_count += 1;
        }
        self.vector_search_count += 1;
    }

    /// Advance one evaluation window while cooling down.
    /// This prevents the tuner from getting stuck with a non-zero cooldown.
    pub fn advance_cooldown_window(&mut self, config: &AutoTuneConfig) -> bool {
        if self.cooldown_remaining == 0 || self.vector_search_count < config.evaluate_interval {
            return false;
        }

        self.vector_search_count = 0;
        self.high_precision_count = 0;
        self.low_precision_count = 0;
        self.window_phase += 1;
        self.cooldown_remaining -= 1;
        true
    }

    /// Determine if it's time to evaluate and possibly adjust parameters.
    /// Returns true if adjustment window reached and cooldown expired.
    pub fn should_evaluate(&self, config: &AutoTuneConfig) -> bool {
        self.vector_search_count >= config.evaluate_interval && self.cooldown_remaining == 0
    }

    /// Evaluate precision metrics and adjust parameters if needed.
    /// Returns true if parameters were adjusted.
    pub fn evaluate_and_adjust(&mut self, config: &AutoTuneConfig) -> bool {
        if !self.should_evaluate(config) {
            return false;
        }

        if self.vector_search_count < config.min_vector_searches {
            // Not enough data, reset counters and proceed to next window
            self.vector_search_count = 0;
            self.high_precision_count = 0;
            self.low_precision_count = 0;
            self.window_phase += 1;
            return false;
        }

        let high_precision_ratio =
            self.high_precision_count as f32 / self.vector_search_count as f32;
        let low_precision_ratio = self.low_precision_count as f32 / self.vector_search_count as f32;

        let adjusted = if high_precision_ratio > 0.6 {
            // Most results are good - we can be more selective
            self.increase_min_query_chars(config)
        } else if low_precision_ratio > 0.4 {
            // Many poor results - relax the filter
            self.decrease_min_query_chars(config)
        } else {
            false
        };

        // Reset counters and move to next window
        self.vector_search_count = 0;
        self.high_precision_count = 0;
        self.low_precision_count = 0;
        self.window_phase += 1;

        if adjusted {
            self.cooldown_remaining = config.cooldown_windows;
        } else {
            self.cooldown_remaining = 0;
        }

        adjusted
    }

    /// Increase min_query_chars to be more selective (fewer but better results).
    fn increase_min_query_chars(&mut self, config: &AutoTuneConfig) -> bool {
        let new_value = (self.current_min_query_chars + config.min_query_chars_step)
            .min(config.min_query_chars_max);
        if new_value != self.current_min_query_chars {
            info!(
                "autotune: increasing min_query_chars from {} to {}",
                self.current_min_query_chars, new_value
            );
            self.current_min_query_chars = new_value;
            true
        } else {
            false
        }
    }

    /// Decrease min_query_chars to be more permissive (more results).
    fn decrease_min_query_chars(&mut self, config: &AutoTuneConfig) -> bool {
        let new_value = self
            .current_min_query_chars
            .saturating_sub(config.min_query_chars_step)
            .max(config.min_query_chars_min);
        if new_value != self.current_min_query_chars {
            info!(
                "autotune: decreasing min_query_chars from {} to {}",
                self.current_min_query_chars, new_value
            );
            self.current_min_query_chars = new_value;
            true
        } else {
            false
        }
    }

    /// Return current tuning state as JSON for RPC responses.
    pub fn snapshot(&self) -> Value {
        serde_json::json!({
            "current_min_query_chars": self.current_min_query_chars,
            "current_top_k": self.current_top_k,
            "window_phase": self.window_phase,
            "high_precision_count": self.high_precision_count,
            "low_precision_count": self.low_precision_count,
            "vector_search_count": self.vector_search_count,
            "cooldown_remaining": self.cooldown_remaining,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn autotune_state_initializes_with_config_defaults() {
        let config = AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let state = AutoTuneState::new(&config);
        assert_eq!(state.current_min_query_chars, 40);
        assert_eq!(state.current_top_k, 2);
        assert_eq!(state.window_phase, 0);
        assert_eq!(state.vector_search_count, 0);
    }

    #[test]
    fn autotune_state_records_vector_search_metrics() {
        let config = AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = AutoTuneState::new(&config);
        state.record_vector_search(0.9, &config); // high precision
        state.record_vector_search(0.3, &config); // low precision
        state.record_vector_search(0.5, &config); // medium (no increment)

        assert_eq!(state.vector_search_count, 3);
        assert_eq!(state.high_precision_count, 1);
        assert_eq!(state.low_precision_count, 1);
    }

    #[test]
    fn autotune_state_adjusts_on_high_precision() {
        let config = AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = AutoTuneState::new(&config);
        // Record 20 searches: 15 high precision (75%)
        for _ in 0..15 {
            state.record_vector_search(0.9, &config);
        }
        for _ in 0..5 {
            state.record_vector_search(0.5, &config);
        }

        let adjusted = state.evaluate_and_adjust(&config);
        assert!(adjusted, "should adjust when precision is high");
        assert_eq!(
            state.current_min_query_chars, 60,
            "should increase min_query_chars"
        );
        assert_eq!(state.vector_search_count, 0, "should reset counters");
        assert_eq!(state.window_phase, 1);
    }

    #[test]
    fn autotune_state_adjusts_on_low_precision() {
        let config = AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = AutoTuneState::new(&config);
        state.current_min_query_chars = 100; // start higher
                                             // Record 20 searches: 10 low precision (50%)
        for _ in 0..10 {
            state.record_vector_search(0.2, &config);
        }
        for _ in 0..10 {
            state.record_vector_search(0.5, &config);
        }

        let adjusted = state.evaluate_and_adjust(&config);
        assert!(adjusted, "should adjust when precision is low");
        assert_eq!(
            state.current_min_query_chars, 80,
            "should decrease min_query_chars"
        );
    }

    #[test]
    fn autotune_state_respects_cooldown() {
        let config = AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = AutoTuneState::new(&config);
        // Fill evaluation window with high precision
        for _ in 0..15 {
            state.record_vector_search(0.9, &config);
        }
        for _ in 0..5 {
            state.record_vector_search(0.5, &config);
        }

        // First adjustment should succeed
        let adjusted1 = state.evaluate_and_adjust(&config);
        assert!(adjusted1);
        assert_eq!(state.cooldown_remaining, 2);
        let min_query_chars_after_first = state.current_min_query_chars;

        // Fill next evaluation window
        for _ in 0..15 {
            state.record_vector_search(0.9, &config);
        }
        for _ in 0..5 {
            state.record_vector_search(0.5, &config);
        }

        // Second adjustment attempt should fail due to cooldown
        let adjusted2 = state.evaluate_and_adjust(&config);
        assert!(!adjusted2, "should not adjust during cooldown");
        assert_eq!(
            state.current_min_query_chars, min_query_chars_after_first,
            "parameters should not change"
        );

        // Advance the cooldown via the production path
        // (`advance_cooldown_window`, called from apply_autotune_feedback in
        // vector_context.rs). tick_cooldown was removed — it only decremented
        // the counter without advancing the window or resetting counters, so
        // it duplicated advance_cooldown_window with weaker semantics and had
        // zero production callers.
        assert!(
            state.advance_cooldown_window(&config),
            "first cooldown window should advance"
        );
        // Fill the next window and advance the second cooldown window.
        for _ in 0..15 {
            state.record_vector_search(0.9, &config);
        }
        for _ in 0..5 {
            state.record_vector_search(0.5, &config);
        }
        assert!(
            state.advance_cooldown_window(&config),
            "second cooldown window should advance"
        );

        // Now should be able to adjust again (cooldown expired and new window filled)
        for _ in 0..15 {
            state.record_vector_search(0.9, &config);
        }
        for _ in 0..5 {
            state.record_vector_search(0.5, &config);
        }
        let adjusted3 = state.evaluate_and_adjust(&config);
        assert!(adjusted3, "should adjust after cooldown expires");
    }

    #[test]
    fn autotune_cooldown_advances_across_windows() {
        let config = AutoTuneConfig {
            enabled: true,
            evaluate_interval: 4,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 2,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = AutoTuneState::new(&config);
        state.cooldown_remaining = 2;
        state.vector_search_count = 4;
        state.high_precision_count = 3;
        state.low_precision_count = 1;

        let advanced = state.advance_cooldown_window(&config);
        assert!(
            advanced,
            "cooldown window should advance once interval is reached"
        );
        assert_eq!(state.cooldown_remaining, 1);
        assert_eq!(state.vector_search_count, 0);
        assert_eq!(state.high_precision_count, 0);
        assert_eq!(state.low_precision_count, 0);
        assert_eq!(state.window_phase, 1);
    }

    #[test]
    fn autotune_state_load_and_save_roundtrip() {
        let config = AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let temp_file = NamedTempFile::new().expect("failed to create temp file");
        let path = temp_file
            .path()
            .to_str()
            .expect("failed to get path")
            .to_string();

        // Create, modify, and save state
        let mut state = AutoTuneState::new(&config);
        state.current_min_query_chars = 120;
        state.current_top_k = 3;
        state.window_phase = 5;
        state.vector_search_count = 10;
        state.high_precision_count = 8;
        state.low_precision_count = 1;

        state.save(&path).expect("failed to save state");

        // Load and verify
        let loaded = AutoTuneState::load_or_default(&path, &config);
        assert_eq!(loaded.current_min_query_chars, 120);
        assert_eq!(loaded.current_top_k, 3);
        assert_eq!(loaded.window_phase, 5);
        assert_eq!(loaded.vector_search_count, 10);
        assert_eq!(loaded.high_precision_count, 8);
        assert_eq!(loaded.low_precision_count, 1);
    }
}
