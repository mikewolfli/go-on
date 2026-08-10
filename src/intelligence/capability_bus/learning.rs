//! Learning subsystem — RL integration (Q-learning, experience, federated RL, continuous learning)
//!
//! Extracted from `core.rs` to isolate the reinforcement learning loop and
//! lifelong learning (F-GAP-24, F-GAP-51).
//!
//! Each method handles its own errors via `warn!()` and respects the
//! lock ordering discipline documented in `core::CapabilityBus`.

use super::core::CapabilityBus;

use crate::intelligence::reinforcement::learning::RlTaskExecutionMetrics;
use tracing::warn;

impl CapabilityBus {
    /// Update Q-table with reward signal from latest execution.
    pub(crate) fn evolve_q_learning(
        &self,
        state: &(String, String),
        action: &str,
        next_state: &(String, String),
        token_cost: u64,
        success: bool,
        quality_score: f64,
    ) -> f64 {
        let metrics = RlTaskExecutionMetrics {
            tokens_used: token_cost,
            success,
            quality_score,
            duration_ms: 0,
        };
        let reward = crate::lock_or_recover!(&self.reward_fn, "intelligence").calculate(&metrics);
        // BLUE70: Use ReinforcementBus (replaces legacy QLearningAgent)
        let mut rb = crate::write_or_recover!(&self.reinforcement_bus, "intelligence");
        rb.record_reward(&state.0, action, reward, &next_state.0);
        reward
    }

    /// Record success case in experience knowledge base.
    pub(crate) fn evolve_experience(
        &self,
        state: &(String, String),
        _action: &str,
        success: bool,
        quality_score: f64,
    ) {
        if success {
            // BLUE70: Record in UnifiedKnowledgeBus (replaces legacy ExperienceKnowledgeBase)
            crate::write_or_recover!(&self.unified_knowledge_bus, "intelligence").record_outcome(
                &state.0,
                &state.1,
                true,
                format!("quality={:.2}", quality_score),
            );
        }
    }

    /// Consolidate experience into continuous learning center.
    ///
    /// Periodically triggers forgetting detection and experience replay
    /// to close the online learning loop (F-GAP-51).
    pub(crate) fn evolve_continuous_learning(
        &self,
        state: &(String, String),
        action: &str,
        reward: f64,
        success: bool,
        quality_score: f64,
    ) {
        if let Err(e) = crate::lock_or_recover!(&self.continuous_learning, "intelligence")
            .consolidate_experience(
                &format!("{:?}_{}", state.0, action),
                &serde_json::json!({
                    "state": state,
                    "action": action,
                    "success": success,
                    "reward": reward,
                    "quality": quality_score,
                })
                .to_string(),
                quality_score,
            )
        {
            warn!(
                "evolve: continuous_learning.consolidate_experience failed: {}",
                e
            );
        }

        // Periodic maintenance (forgetting detection + spaced replay) is owned
        // by the 10-minute `ContinuousLearningCenter::review_cycle` background
        // task in acp/server.rs, which runs the full loop (LLM distillation,
        // reinforce, replay, risk eviction). Keeping a second copy here would
        // double-write the same center.
    }
}
