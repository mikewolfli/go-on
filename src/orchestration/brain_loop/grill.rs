//! GRILL reflection hooks — integrates GRILL-style interrogation into BrainLoop reflection.
//!
//! During the reflection phase, these hooks can optionally invoke GRILL skills
//! to deepen analysis: asking probing questions, diagnosing root causes, and
//! suggesting structural improvements.

use crate::orchestration::brain_loop::BrainLoopReflection;
use serde::{Deserialize, Serialize};

/// GRILL interrogation mode for the reflection phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GrillMode {
    /// No GRILL interrogation (standard reflection only).
    Disabled,
    /// Lightweight: add probing follow-up questions to reflections.
    Light,
    /// Full: add root-cause diagnosis, counterfactuals, and improvement suggestions.
    Full,
}

impl Default for GrillMode {
    fn default() -> Self {
        Self::Disabled
    }
}

/// Enhance a `BrainLoopReflection` with GRILL-style probing questions.
///
/// When `GrillMode` is enabled, this function appends additional observations
/// and improvements to the reflection, simulating the "grilling" interaction
/// pattern — asking deeper "why" and "what if" questions.
pub fn enhance_reflection_with_grill(
    reflection: &mut BrainLoopReflection,
    mode: GrillMode,
    task_context: &str,
) {
    match mode {
        GrillMode::Disabled => { /* no-op */ }
        GrillMode::Light => {
            reflection.observations.push(format!(
                "[GRILL-light] Why was this approach chosen for: {}?",
                task_context
            ));
            reflection.improvements.push(
                "[GRILL-light] Consider if there's a simpler alternative that achieves the same goal."
                    .to_string(),
            );
        }
        GrillMode::Full => {
            reflection.observations.push(format!(
                "[GRILL-full] Root cause probe: what assumptions were made about: {}?",
                task_context
            ));
            reflection.observations.push(
                "[GRILL-full] Counterfactual: what would happen if we reversed the dependency order?"
                    .to_string(),
            );
            reflection.improvements.push(
                "[GRILL-full] Structural suggestion: extract this logic into a reusable module."
                    .to_string(),
            );
            reflection
                .improvements
                .push("[GRILL-full] Add explicit error handling for edge cases uncovered during execution."
                    .to_string());
            reflection.issues.push(
                "[GRILL-full] Potential blind spot: verify that all side effects are documented."
                    .to_string(),
            );
        }
    }
}

/// Build a GRILL prompt string for the reflection phase.
///
/// This can be used as input to a GRILL SKILL.md execution when the
/// LLM-based skill system is active.
// Reserved for future LLM-based GRILL skill system integration;
// tested but not yet wired into production reflection flow.
#[allow(dead_code)]
pub fn build_grill_prompt(step_description: &str, mode: GrillMode) -> String {
    match mode {
        GrillMode::Disabled => String::new(),
        GrillMode::Light => format!(
            "Lightly grill the step: '{}'. Ask one probing question about the approach.",
            step_description
        ),
        GrillMode::Full => format!(
            "Deeply grill the step: '{}'. Analyze assumptions, counterfactuals, \
             and suggest structural improvements. Identify potential blind spots.",
            step_description
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_reflection() -> BrainLoopReflection {
        BrainLoopReflection {
            step_id: "test-step".to_string(),
            observations: vec!["Step completed".to_string()],
            issues: vec![],
            improvements: vec![],
            confidence: 0.8,
            reflection_ms: 100,
            context_snapshot: None,
            reasoning_chain: vec![],
        }
    }

    #[test]
    fn test_grill_disabled_no_changes() {
        let mut reflection = make_reflection();
        let initial_len = reflection.observations.len();
        enhance_reflection_with_grill(&mut reflection, GrillMode::Disabled, "test task");
        assert_eq!(reflection.observations.len(), initial_len);
    }

    #[test]
    fn test_grill_light_adds_observation_and_improvement() {
        let mut reflection = make_reflection();
        enhance_reflection_with_grill(&mut reflection, GrillMode::Light, "test task");
        assert_eq!(reflection.observations.len(), 2);
        assert!(reflection.observations[1].contains("[GRILL-light]"));
        assert!(!reflection.improvements.is_empty());
    }

    #[test]
    fn test_grill_full_adds_multiple_items() {
        let mut reflection = make_reflection();
        enhance_reflection_with_grill(&mut reflection, GrillMode::Full, "test task");
        assert!(reflection.observations.len() >= 3);
        assert!(reflection.improvements.len() >= 2);
        assert!(!reflection.issues.is_empty());
    }

    #[test]
    fn test_build_grill_prompt_disabled_returns_empty() {
        assert!(build_grill_prompt("test", GrillMode::Disabled).is_empty());
    }

    #[test]
    fn test_build_grill_prompt_light_returns_probe() {
        let prompt = build_grill_prompt("sort the array", GrillMode::Light);
        assert!(prompt.contains("Lightly grill"));
        assert!(prompt.contains("sort the array"));
    }

    #[test]
    fn test_build_grill_prompt_full_returns_deep_analysis() {
        let prompt = build_grill_prompt("implement cache", GrillMode::Full);
        assert!(prompt.contains("Deeply grill"));
        assert!(prompt.contains("assumptions"));
        assert!(prompt.contains("counterfactuals"));
    }
}
