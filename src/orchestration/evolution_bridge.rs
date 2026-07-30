//! Evolution-BrainLoop bridge — shares observations between self-evolution and brain loop.
//!
//! Converts `EvolutionTrigger` values (from the self-evolution system) into
//! `BrainLoopReflection` observations, and vice versa. This enables the evolution
//! loop's metric-driven observations to feed into the brain loop's reflection phase,
//! and brain loop reflections to trigger evolution cycles.
//!
//! # Wiring status
//! These bridge functions are ready for integration but not yet called from
//! production paths. They will be wired once the self-evolution loop's observation
//! pipeline is connected to the brain loop's reflection phase (planned for Phase 11).

use crate::orchestration::brain_loop::BrainLoopReflection;
use crate::orchestration::self_evolution::evolution_loop::observe::EvolutionTrigger;

/// Convert an `EvolutionTrigger` into a list of observation strings
/// that can be added to a `BrainLoopReflection`.
///
/// This bridges the evolution loop's metric-driven observations into the
/// brain loop's reflection phase, enabling plan-level awareness of system
/// health trends.
///
#[cfg_attr(not(test), allow(dead_code))]
pub fn evolution_trigger_to_reflections(trigger: &EvolutionTrigger) -> Vec<String> {
    match trigger {
        EvolutionTrigger::PerformanceRegression {
            metric,
            threshold,
            direction,
        } => {
            vec![format!(
                "[evolution] Performance regression detected: {} crossed {} (direction: {:?})",
                metric, threshold, direction
            )]
        }
        EvolutionTrigger::RepeatedError { pattern, count } => {
            vec![format!(
                "[evolution] Repeated error pattern '{}' observed {} times",
                pattern, count
            )]
        }
        EvolutionTrigger::DeadCodeDetected { module, ratio } => {
            vec![format!(
                "[evolution] Dead code detected in module '{}' (ratio: {:.1}%)",
                module,
                ratio * 100.0
            )]
        }
        EvolutionTrigger::ManualRequest { instruction } => {
            vec![format!(
                "[evolution] Manual evolution request: {}",
                instruction
            )]
        }
        EvolutionTrigger::ConfigDrift {
            key,
            expected,
            actual,
        } => {
            vec![format!(
                "[evolution] Configuration drift: '{}' expected '{}' but found '{}'",
                key, expected, actual
            )]
        }
        EvolutionTrigger::DegradationDetected {
            capability_id,
            trend_slope,
        } => {
            vec![format!(
                "[evolution] Capability '{}' degrading (trend slope: {:.2})",
                capability_id, trend_slope
            )]
        }
    }
}

/// Convert a `BrainLoopReflection` into a list of `EvolutionTrigger` values.
///
/// Observations containing "[evolution]" were already sourced from the evolution
/// system and are skipped. Other observations are packaged as `ManualRequest`
/// triggers so the evolution loop can process them.
///
#[cfg_attr(not(test), allow(dead_code))]
pub fn brain_reflection_to_evolution_triggers(
    reflection: &BrainLoopReflection,
) -> Vec<EvolutionTrigger> {
    let mut triggers = Vec::new();

    for obs in &reflection.observations {
        // Skip observations that originated from the evolution system.
        if obs.starts_with("[evolution]") {
            continue;
        }
        triggers.push(EvolutionTrigger::ManualRequest {
            instruction: format!(
                "BrainLoop reflection on step '{}': {}",
                reflection.step_id, obs
            ),
        });
    }

    for issue in &reflection.issues {
        triggers.push(EvolutionTrigger::RepeatedError {
            pattern: issue.clone(),
            count: 1,
        });
    }

    triggers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::brain_loop::BrainLoopReflection;

    fn sample_reflection() -> BrainLoopReflection {
        BrainLoopReflection {
            step_id: "test-step".to_string(),
            observations: vec![
                "Step completed successfully".to_string(),
                "[evolution] Performance regression in module X".to_string(),
            ],
            issues: vec!["Timeout occurred".to_string()],
            improvements: vec!["Add retry logic".to_string()],
            confidence: 0.8,
            reflection_ms: 100,
            context_snapshot: None,
            reasoning_chain: vec![],
        }
    }

    #[test]
    fn test_performance_regression_to_observation() {
        let trigger = EvolutionTrigger::PerformanceRegression {
            metric: "latency_p50".to_string(),
            threshold: 200.0,
            direction: crate::orchestration::self_evolution::evolution_loop::observe::RegressionDirection::Increasing,
        };
        let obs = evolution_trigger_to_reflections(&trigger);
        assert_eq!(obs.len(), 1);
        assert!(obs[0].contains("latency_p50"));
        assert!(obs[0].contains("[evolution]"));
    }

    #[test]
    fn test_repeated_error_to_observation() {
        let trigger = EvolutionTrigger::RepeatedError {
            pattern: "connection refused".to_string(),
            count: 5,
        };
        let obs = evolution_trigger_to_reflections(&trigger);
        assert_eq!(obs.len(), 1);
        assert!(obs[0].contains("connection refused"));
        assert!(obs[0].contains("5"));
    }

    #[test]
    fn test_reflection_to_triggers_skips_evolution_observations() {
        let reflection = sample_reflection();
        let triggers = brain_reflection_to_evolution_triggers(&reflection);
        // The "[evolution]" observation should be skipped.
        let evolution_obs_count = triggers
            .iter()
            .filter(|t| matches!(t, EvolutionTrigger::ManualRequest { .. }))
            .count();
        assert_eq!(evolution_obs_count, 1); // Only "Step completed"
                                            // The issue becomes a RepeatedError.
        let error_count = triggers
            .iter()
            .filter(|t| matches!(t, EvolutionTrigger::RepeatedError { .. }))
            .count();
        assert_eq!(error_count, 1);
    }

    #[test]
    fn test_dead_code_trigger_format() {
        let trigger = EvolutionTrigger::DeadCodeDetected {
            module: "src/legacy.rs".to_string(),
            ratio: 0.15,
        };
        let obs = evolution_trigger_to_reflections(&trigger);
        assert!(obs[0].contains("15.0%"));
    }

    #[test]
    fn test_config_drift_trigger_format() {
        let trigger = EvolutionTrigger::ConfigDrift {
            key: "timeout".to_string(),
            expected: "30".to_string(),
            actual: "60".to_string(),
        };
        let obs = evolution_trigger_to_reflections(&trigger);
        assert!(obs[0].contains("timeout"));
        assert!(obs[0].contains("30"));
        assert!(obs[0].contains("60"));
    }

    #[test]
    fn test_empty_reflection_produces_no_triggers() {
        let reflection = BrainLoopReflection {
            step_id: "empty".to_string(),
            observations: vec![],
            issues: vec![],
            improvements: vec![],
            confidence: 1.0,
            reflection_ms: 0,
            context_snapshot: None,
            reasoning_chain: vec![],
        };
        let triggers = brain_reflection_to_evolution_triggers(&reflection);
        assert!(triggers.is_empty());
    }
}
