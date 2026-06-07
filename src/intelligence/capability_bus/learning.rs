//! Learning subsystem — RL integration (Q-learning, experience, federated RL, continuous learning)
//!
//! Extracted from `core.rs` to isolate the reinforcement learning loop and
//! lifelong learning (F-GAP-24, F-GAP-51).
//!
//! Each method handles its own errors via `warn!()` and respects the
//! lock ordering discipline documented in `core::CapabilityBus`.

use super::core::CapabilityBus;
use crate::intelligence::now_ms;
use crate::intelligence::reinforcement::learning::{
    RlTaskExecutionMetrics, SuccessCase,
};
use crate::intelligence::lock_guard;
use std::sync::atomic::{AtomicU64, Ordering};
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
        let reward = lock_guard(&self.reward_fn).calculate(&metrics);
        lock_guard(&self.q_learning).update(state, action, reward, next_state);
        reward
    }

    /// Record success case in experience knowledge base.
    pub(crate) fn evolve_experience(
        &self,
        state: &(String, String),
        action: &str,
        success: bool,
        quality_score: f64,
    ) {
        if success {
            lock_guard(&self.experience).add_success_case(SuccessCase {
                objective: format!("state_{:?}", state),
                strategy: format!("action_{}", action),
                confidence: quality_score,
            });
        }
    }

    /// Submit local policy to FederatedRL.
    pub(crate) fn evolve_federated_rl(
        &self,
        state: &(String, String),
        action: &str,
        reward: f64,
        quality_score: f64,
        success: bool,
    ) {
        if success {
            let now = now_ms();
            let frl = self.federated_rl.submit_policy(
                "local_agent".to_string(),
                format!("evolve_{}", state.0),
                serde_json::json!({
                    "state": state,
                    "action": action,
                    "reward": reward,
                    "timestamp": now,
                })
                .to_string(),
                quality_score,
                1,
            );
            if let Err(e) = self
                .federated_rl
                .contribute_to_round(&format!("round_{}", state.0), &frl)
            {
                warn!("evolve: federated_rl.contribute_to_round failed: {}", e);
            }
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
        if let Err(e) = lock_guard(&self.continuous_learning).consolidate_experience(
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
        ) {
            warn!(
                "evolve: continuous_learning.consolidate_experience failed: {}",
                e
            );
        }

        // ── Periodic maintenance: detect forgetting & replay (every 10th call) ──
        static CL_MAINTENANCE_COUNTER: AtomicU64 = AtomicU64::new(0);
        let count = CL_MAINTENANCE_COUNTER.fetch_add(1, Ordering::Relaxed);

        if count.is_multiple_of(10) {
            // 1. Detect forgetting and reinforce forgotten memories
            let forgotten = {
                let cl = lock_guard(&self.continuous_learning);
                cl.detect_forgetting()
            };
            for curve in &forgotten {
                if let Err(e) =
                    lock_guard(&self.continuous_learning).reinforce_memory(&curve.memory_id)
                {
                    warn!("evolve: reinforce_memory failed: {}", e);
                }
            }
            if !forgotten.is_empty() {
                tracing::info!(
                    "evolve: continuous_learning reinforced {} forgotten memories",
                    forgotten.len()
                );
            }

            // 2. Replay important memories and feed into Q-learning
            let replayed = {
                let cl = lock_guard(&self.continuous_learning);
                cl.replay_important_memories(3)
            };
            for mem in &replayed {
                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&mem.data) {
                    // Parse the stored (state, action, reward) triple
                    let state_arr = data["state"].as_array();
                    let action_str = data["action"].as_str();
                    let replay_reward = data["reward"].as_f64();

                    if let (Some(arr), Some(action_str), Some(replay_reward)) =
                        (state_arr, action_str, replay_reward)
                    {
                        if arr.len() >= 2 {
                            if let (Some(s0), Some(s1)) = (arr[0].as_str(), arr[1].as_str()) {
                                let replayed_state = (s0.to_string(), s1.to_string());
                                // Perform a mini Q-learning update with
                                // replayed experience using the current
                                // state as the next_state placeholder.
                                lock_guard(&self.q_learning).update(
                                    &replayed_state,
                                    action_str,
                                    replay_reward,
                                    state,
                                );
                            }
                        }
                    }
                }
            }
            if !replayed.is_empty() {
                tracing::info!(
                    "evolve: continuous_learning replayed {} memories into Q-learning",
                    replayed.len()
                );
            }
        }
    }
}
