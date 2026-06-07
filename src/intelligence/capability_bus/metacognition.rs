//! Metacognition subsystem — metacognitive observation and feedback
//!
//! Extracted from `core.rs` to isolate the MetacognitiveController
//! integration within the evolve pipeline.
//!
//! Records observations and generates feedback that modulates the
//! Q-learning exploration rate and Q-values (F-GAP-51).

use super::core::CapabilityBus;
use crate::intelligence::lock_guard;
use tracing::warn;

impl CapabilityBus {
    /// Record observation in metacognitive controller and feed feedback into Q-learning.
    pub(crate) fn evolve_metacognitive(
        &self,
        state: &(String, String),
        action: &str,
        reward: f64,
        quality_score: f64,
        success: bool,
    ) {
        if let Err(e) = self.metacognitive.record_observation(
            &format!("evolve_{}_{}", state.0, action),
            "capability_bus",
            "evolution",
            if success { "success" } else { "failure" },
            &format!("reward={}, quality={}", reward, quality_score),
        ) {
            warn!("evolve: metacognitive.record_observation failed: {}", e);
        }

        // ── Generate metacognitive feedback and feed into Q-learning (F-GAP-51) ──
        let feedback = self.metacognitive.generate_evolve_feedback();
        let reward_multiplier = feedback["reward_multiplier"].as_f64().unwrap_or(1.0);
        let suggested_exploration_rate = feedback["suggested_exploration_rate"]
            .as_f64()
            .unwrap_or(0.1);

        // Apply suggested exploration rate to Q-learning agent for future decisions.
        {
            let mut ql = lock_guard(&self.q_learning);
            ql.exploration_rate = suggested_exploration_rate;
        }

        // Scale the Q-value for this (state, action) pair by the reward_multiplier
        // to retroactively incorporate metacognitive insight into the Q-table.
        if (reward_multiplier - 1.0).abs() > 0.001 {
            let mut ql = lock_guard(&self.q_learning);
            if let Some(state_actions) = ql.q_table.get_mut(state) {
                if let Some(q_val) = state_actions.get_mut(action) {
                    *q_val *= reward_multiplier;
                }
            }
            if let Some(state_actions) = ql.q_table_2.get_mut(state) {
                if let Some(q_val) = state_actions.get_mut(action) {
                    *q_val *= reward_multiplier;
                }
            }
        }
    }
}
