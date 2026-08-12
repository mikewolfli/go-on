//! GAP-B53-55: Metacognitive → Consciousness → Evolution triple fusion.
//!
//! Bridges the three self-referential subsystems so that:
//! - **Metacognitive** observations inform **Consciousness** awareness metrics
//! - **Consciousness** reflexion insights drive **Evolution** triggers
//! - **Evolution** outcomes feed back into **Metacognitive** corrective learning
//!
//! This creates a closed-loop self-improvement cycle that spans sessions.

use crate::intelligence::consciousness::{AwarenessMetricType, ConsciousnessMetrics};
use crate::intelligence::metacognitive::MetacognitiveController;
use crate::orchestration::self_evolution::evolution_loop::{EvolutionTrigger, RegressionDirection};

use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;

/// Configuration for the triple fusion bridge.
#[derive(Debug, Clone)]
pub struct TripleFusionConfig {
    /// Minimum consciousness awareness threshold before evolution triggers fire.
    pub min_awareness_for_evolution: f64,
    /// How often (in ms) to push metacognitive data into consciousness.
    pub metacognitive_sync_interval_ms: u64,
    /// Whether to auto-generate evolution triggers from consciousness insights.
    pub auto_evolve_from_reflexion: bool,
}

impl Default for TripleFusionConfig {
    fn default() -> Self {
        // Design constants (no config surface today — the bridge runs with
        // fixed tuning): awareness must cross 0.3 before evolution triggers
        // fire (keeps low-signal reflexion out); metacognitive data syncs
        // into consciousness every 10s; auto-evolution from reflexion is on.
        Self {
            min_awareness_for_evolution: 0.3,
            metacognitive_sync_interval_ms: 10_000,
            auto_evolve_from_reflexion: true,
        }
    }
}

/// The triple fusion bridge that synchronises data across the three systems.
pub struct TripleFusionBridge {
    config: TripleFusionConfig,
    /// Running count of fusion cycles executed.
    fusion_cycles: u64,
    /// Timestamp (ms since epoch) of the last metacognitive sync.
    last_sync_time_ms: u64,
    /// Timestamp (ms since epoch) of the last consciousness→evolution trigger
    /// emission, so the static "awareness is high" signal cannot drive a real
    /// auto-patch cycle on every request when evolution is enabled.
    last_evolution_trigger_ms: u64,
}

/// Minimum interval between consciousness→evolution trigger emissions.
const EVOLUTION_TRIGGER_MIN_INTERVAL_MS: u64 = 10 * 60 * 1000; // 10 minutes

// ── Global singleton ──────────────────────────────────────────────────────

/// Global singleton bridge shared across all requests so fusion cycles accumulate.
///
/// # Locking notes
///
/// Uses `tokio::sync::Mutex` (not `std::sync::Mutex`) because the singleton
/// is accessed from async contexts (`tokio::spawn` in `chat_phases.rs`,
/// `EvolutionLoop::run` in `evolution_loop/mod.rs`).  A `std::sync::Mutex`
/// held across an `.await` point would be a runtime error.
///
/// **Read/write ratio**: The primary entry point is `run_fusion_cycle()`
/// (&mut self — write), which calls the read-only helper
/// `consciousness_to_evolution_triggers()` internally.  Standalone reads
/// (`fusion_cycles()`, `consciousness_to_evolution_triggers()`) are rare.
/// Switching to `tokio::sync::RwLock` is not beneficial because the
/// write path dominates and the additional complexity does not justify
/// the marginal gain.
static GLOBAL_TRIPLE_FUSION: OnceLock<Arc<Mutex<TripleFusionBridge>>> = OnceLock::new();

/// Returns a reference to the global `TripleFusionBridge` singleton, initialising
/// it once with default configuration.
pub fn global_triple_fusion_bridge() -> &'static Arc<Mutex<TripleFusionBridge>> {
    GLOBAL_TRIPLE_FUSION.get_or_init(|| {
        Arc::new(Mutex::new(TripleFusionBridge::new(
            TripleFusionConfig::default(),
        )))
    })
}

impl std::fmt::Debug for TripleFusionBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TripleFusionBridge")
            .field("fusion_cycles", &self.fusion_cycles())
            .field("config", &self.config)
            .finish()
    }
}

impl TripleFusionBridge {
    /// Return the number of fusion cycles executed so far.
    pub fn fusion_cycles(&self) -> u64 {
        self.fusion_cycles
    }

    /// Create a new triple fusion bridge.
    pub fn new(config: TripleFusionConfig) -> Self {
        Self {
            config,
            fusion_cycles: 0,
            last_sync_time_ms: 0,
            last_evolution_trigger_ms: 0,
        }
    }

    /// Phase 1: Push metacognitive observations into Consciousness metrics.
    ///
    /// Each unresolved observation becomes an EnvironmentalAwareness metric
    /// so the consciousness system can track the system's self-awareness of issues.
    ///
    /// The sync is gated by `metacognitive_sync_interval_ms` — consecutive calls
    /// within the interval are no-ops to avoid overwhelming the consciousness system.
    pub fn sync_metacognitive_to_consciousness(
        &mut self,
        metacognitive: &MetacognitiveController,
        consciousness: &ConsciousnessMetrics,
    ) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let last_sync = self.last_sync_time_ms;
        if now_ms.saturating_sub(last_sync) < self.config.metacognitive_sync_interval_ms {
            return;
        }
        self.last_sync_time_ms = now_ms;

        let observations = metacognitive.list_observations(false);
        let unresolved_count = observations.iter().filter(|o| !o.is_resolved).count() as f64;

        if unresolved_count > 0.0 {
            let awareness_value = (unresolved_count / 20.0).clamp(0.0, 1.0);
            let _ = consciousness.record_metric(
                AwarenessMetricType::EnvironmentalAwareness,
                awareness_value,
                0.7,
            );
        }

        // Push a MetaAwareness metric reflecting the metacognitive insight depth.
        let profile = metacognitive.profile();
        let meta_value = (profile.action_effectiveness_ratio * 0.5
            + (profile.successful_actions as f64 / profile.total_actions_taken.max(1) as f64)
                * 0.5)
            .clamp(0.0, 1.0);
        let _ = consciousness.record_metric(AwarenessMetricType::MetaAwareness, meta_value, 0.8);
    }

    /// Phase 2: Convert Consciousness reflexion insights into Evolution triggers.
    ///
    /// When consciousness reflexion produces insights and awareness is high enough,
    /// generate evolution triggers that the EvolutionLoop can process.
    pub fn consciousness_to_evolution_triggers(
        &mut self,
        consciousness: &ConsciousnessMetrics,
    ) -> Vec<EvolutionTrigger> {
        if !self.config.auto_evolve_from_reflexion {
            return Vec::new();
        }

        let profile = consciousness.profile();
        if profile.overall_awareness < self.config.min_awareness_for_evolution {
            return Vec::new();
        }

        let mut triggers = Vec::new();

        // Generate PerformanceRegression trigger if awareness of self is high
        // (suggesting the system detects its own performance). The trigger is
        // a static threshold signal, not a measured regression, so it is
        // rate-limited: emitting it per request would start a real auto-patch
        // cycle on every chat once evolution is enabled.
        if profile.overall_awareness > 0.6 {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            if now_ms.saturating_sub(self.last_evolution_trigger_ms)
                >= EVOLUTION_TRIGGER_MIN_INTERVAL_MS
            {
                self.last_evolution_trigger_ms = now_ms;
                triggers.push(EvolutionTrigger::PerformanceRegression {
                    metric: "consciousness_awareness".to_string(),
                    threshold: 0.5,
                    direction: RegressionDirection::Decreasing,
                });
            }
        }

        // NOTE: The former ConfigDrift trigger was removed — it emitted
        // `expected == actual` (both derived from the same profile state), a
        // constant self-identical signal that claimed "drift" where none
        // existed. Real configuration drift needs an external baseline to
        // compare against; none is wired here.

        triggers
    }

    /// Phase 3: Feed Evolution outcomes back into Metacognitive learning.
    ///
    /// Records the evolution outcome as a corrective result so the metacognitive
    /// system can learn from past evolution attempts.
    ///
    /// Wired into EvolutionLoop::run() after each evolution cycle completes.
    pub fn record_evolution_outcome(
        &self,
        metacognitive: &MetacognitiveController,
        trigger: &EvolutionTrigger,
        success: bool,
    ) {
        let obs_id = match metacognitive.record_observation(
            &format!("evolution-{}", trigger.label()),
            "evolution",
            "evolution_cycle",
            if success { "info" } else { "error" },
            &format!(
                "Evolution cycle for trigger '{}' {}",
                trigger.description(),
                if success { "succeeded" } else { "failed" }
            ),
        ) {
            Ok(id) => id,
            Err(_) => return,
        };

        // Record the outcome as an action so total_actions_taken reflects it.
        let _ = metacognitive.record_action_outcome(
            "evolution",
            &obs_id,
            &format!("Evolution outcome for trigger '{}'", trigger.label()),
            success,
        );
    }

    /// Run a full fusion cycle: sync → convert → learn.
    ///
    /// Returns the number of evolution triggers generated.
    pub fn run_fusion_cycle(
        &mut self,
        metacognitive: &MetacognitiveController,
        consciousness: &ConsciousnessMetrics,
    ) -> Vec<EvolutionTrigger> {
        self.fusion_cycles += 1;

        // Phase 1
        self.sync_metacognitive_to_consciousness(metacognitive, consciousness);

        // Phase 2
        let triggers = self.consciousness_to_evolution_triggers(consciousness);

        // Phase 3 is triggered externally via `record_evolution_outcome`
        // when the EvolutionLoop completes a cycle.

        triggers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intelligence::consciousness::ConsciousnessConfig;
    use crate::intelligence::metacognitive::MetacognitiveConfig;

    #[test]
    fn test_fusion_cycle_increments_counter() {
        let config = TripleFusionConfig::default();
        let mut bridge = TripleFusionBridge::new(config);
        let mc = MetacognitiveController::new(MetacognitiveConfig::default());
        let consciousness = ConsciousnessMetrics::new(Default::default());

        let triggers = bridge.run_fusion_cycle(&mc, &consciousness);
        assert_eq!(bridge.fusion_cycles(), 1);
        // At default config with empty state, triggers may be empty
        // (awareness too low).
        assert!(triggers.is_empty());
    }

    #[test]
    fn test_consciousness_to_evolution_triggers_low_awareness() {
        let config = TripleFusionConfig {
            min_awareness_for_evolution: 0.3,
            ..Default::default()
        };
        let mut bridge = TripleFusionBridge::new(config);
        let consciousness = ConsciousnessMetrics::new(ConsciousnessConfig::default());
        let triggers = bridge.consciousness_to_evolution_triggers(&consciousness);
        // Fresh consciousness has low awareness → no triggers.
        assert!(triggers.is_empty());
    }

    #[test]
    fn test_record_evolution_outcome() {
        let mc = MetacognitiveController::new(MetacognitiveConfig::default());
        let bridge = TripleFusionBridge::new(TripleFusionConfig::default());

        let trigger = EvolutionTrigger::ManualRequest {
            instruction: "test evolution".to_string(),
        };

        // Record a successful outcome.
        bridge.record_evolution_outcome(&mc, &trigger, true);
        let profile = mc.profile();
        assert!(
            profile.total_actions_taken > 0,
            "Evolution outcome should be recorded as a metacognitive action"
        );
    }
}
