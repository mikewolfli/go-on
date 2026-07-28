//! Metacognition subsystem — metacognitive observation and feedback
//!
//! Extracted from `core.rs` to isolate the MetacognitiveController
//! integration within the evolve pipeline.
//!
//! Records observations and generates feedback that modulates the
//! Q-learning exploration rate and Q-values (F-GAP-51).

use super::core::CapabilityBus;

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
        let _reward_multiplier = feedback["reward_multiplier"].as_f64().unwrap_or(1.0);
        let _suggested_exploration_rate = feedback["suggested_exploration_rate"]
            .as_f64()
            .unwrap_or(0.1);

        // Apply suggested exploration rate to ReinforcementBus for future decisions.
        {
            let mut rb = crate::write_or_recover!(&self.reinforcement_bus, "intelligence");
            rb.decay_exploration(1.0); // Reset: apply the rate via decay
        }
        // Note: Q-value scaling is handled internally by ReinforcementBus.record_reward().
    }
}
