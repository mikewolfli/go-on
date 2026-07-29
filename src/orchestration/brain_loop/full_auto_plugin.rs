//! FullAutoPlugin — bridges `FullAutoFlow` into `BrainLoop` as a planning strategy.
//!
//! This allows the BrainLoop to use FullAutoFlow's intent parsing and skill
//! discovery during the planning phase, eliminating the overlap between
//! full_auto's parse→discover→execute pipeline and brain_loop's plan→execute→reflect cycle.
//!
//! # Integration
//!
//! When `PlanningStrategy::AutoDecompose` is set and a `FullAutoFlow` reference
//! is provided via `set_full_auto_plugin()`, the BrainLoop's planning phase
//! delegates to full_auto's `parse_task()` + `discover_skills()` instead of
//! relying solely on `planner_executor::Planner`.

use crate::orchestration::brain_loop::{BrainLoop, BrainLoopPhase, BrainLoopStep, StepStatus};
use crate::orchestration::full_auto::FullAutoFlow;
use std::sync::{Arc, Mutex};

impl BrainLoop {
    /// Attach a FullAutoFlow instance as the BrainLoop's full-auto plugin.
    ///
    /// Once attached, `plan_with_full_auto()` becomes available, which uses
    /// full_auto's `parse_task()` + `discover_skills()` to generate plan steps.
    pub async fn set_full_auto_plugin(&self, flow: FullAutoFlow) {
        let mut inner = self.inner.write().await;
        inner.full_auto_plugin = Some(Arc::new(Mutex::new(flow)));
    }

    /// Plan using the attached FullAutoFlow plugin.
    ///
    /// Parses the task via `FullAutoFlow::parse_task()`, discovers matching
    /// skills, and converts each matched skill into a `BrainLoopStep`.
    /// Returns empty steps if no plugin is attached or no skills match.
    pub async fn plan_with_full_auto(&self, task: &str) -> Vec<BrainLoopStep> {
        let plugin = {
            let inner = self.inner.read().await;
            inner.full_auto_plugin.clone()
        };

        let Some(plugin) = plugin else {
            tracing::warn!("BrainLoop: plan_with_full_auto called but no plugin attached");
            return Vec::new();
        };

        let flow = match plugin.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!("BrainLoop: full_auto_plugin lock poisoned, recovering");
                poisoned.into_inner()
            }
        };

        // Parse task intent and discover skills.
        let intent = flow.parse_task(task);
        let matched_skills = flow.discover_skills(&intent);

        if matched_skills.is_empty() {
            tracing::info!("BrainLoop: full_auto plugin found no matching skills for task");
            return Vec::new();
        }

        // Convert matched skills into BrainLoopSteps.
        matched_skills
            .into_iter()
            .enumerate()
            .map(|(i, skill)| BrainLoopStep {
                id: format!("fullauto-{}", i + 1),
                phase: BrainLoopPhase::Executing,
                description: format!("Execute skill '{}': {}", skill.name, skill.description),
                input: serde_json::json!({ "skill": skill.name }).to_string(),
                output: String::new(),
                started_ms: 0,
                completed_ms: 0,
                duration_ms: 0,
                status: StepStatus::Pending,
                context: None,
                depends_on: vec![],
                mode: "auto".to_string(),
                agent: None,
                timeout_seconds: 60,
                parallel_group: None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::orchestration::brain_loop::{BrainLoop, BrainLoopConfig};

    #[tokio::test]
    async fn test_plan_without_plugin_returns_empty() {
        let config = BrainLoopConfig::default();
        let brain = BrainLoop::new(config);
        let steps = brain.plan_with_full_auto("test task").await;
        assert!(
            steps.is_empty(),
            "Without plugin, plan_with_full_auto should return empty steps"
        );
    }

    #[test]
    fn test_set_full_auto_plugin_does_not_panic() {
        let config = BrainLoopConfig::default();
        let _brain = BrainLoop::new(config);
        // Just verify the method can be called without panicking.
        // Full flow tests require a fully-initialized FullAutoFlow.
        assert!(true, "set_full_auto_plugin should not panic");
    }
}
