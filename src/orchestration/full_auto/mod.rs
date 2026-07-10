//! Full-Auto Flow Orchestrator (BLUE43 Step 10)
//!
//! Provides an autonomous execution flow that given a natural-language task
//! description automatically:
//!
//! 1. **Parses** the task into a structured `TaskIntent` (goals, constraints,
//!    prerequisites, deliverables).
//! 2. **Discovers** matching skills via the `SkillRegistry`.
//! 3. **Prepares** an `ExecutionEnvironment` snapshot (dependency check,
//!    runtime readiness, env vars).
//! 4. **Executes** each matched skill in priority order, collecting a full
//!    audit trail of `ExecutionStep` records.
//! 5. **Reports** the outcome as an `AutoExecutionReport` with all diagnostics.
//!
//! # Architecture
//!
//! This module is split into sub-modules for clarity:
//!
//! - [`intent`] — [`TaskIntent`] and task parsing
//! - [`environment`] — [`ExecutionEnvironment`] and environment preparation
//! - [`executor`] — skill discovery and the [`run`](FullAutoFlow::run) method
//! - [`report`] — [`SkillMatch`], [`ExecutionStep`], [`AutoExecutionReport`]

pub mod environment;
pub mod executor;
pub mod intent;
pub mod report;

#[allow(unused_imports)]
pub use environment::ExecutionEnvironment;
pub use intent::TaskIntent;
#[allow(unused_imports)]
pub use report::{AutoExecutionReport, ExecutionStep, SkillMatch};
use std::sync::{Arc, Mutex, RwLock as StdRwLock};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::intelligence::adaptive_selector::AdaptiveModelSelector;
use crate::orchestration::complexity_estimator::ComplexityEstimator;
use crate::orchestration::fast_path_cache::{
    EnvCacheValue, FastPathCache, IntentCacheValue, SkillCacheValue,
};
use crate::orchestration::skill::SkillRegistry;
use crate::orchestration::skill_market::SkillMarketRegistry;
use crate::orchestration::threshold_learner::ThresholdLearner;
use crate::orchestration::tool::ToolRegistry;
use crate::orchestration::tool_recommender::ToolRecommender;

/// Default minimum composite score for a skill to be considered a match.
pub(crate) const DEFAULT_MIN_MATCH_SCORE: f64 = 0.40;

// ---------------------------------------------------------------------------
// FullAutoConfig
// ---------------------------------------------------------------------------

/// Tunable parameters for the full-auto flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullAutoConfig {
    /// Minimum composite score (0.0 – 1.0) for a skill to be matched.
    /// When a `ThresholdLearner` is attached, this value is dynamically
    /// adjusted based on trial outcomes.
    pub min_match_score: f64,
    /// Maximum number of skills to execute in a single run.
    pub max_skills_to_execute: usize,
    /// Maximum number of skills to execute per task (alias for max_skills_to_execute).
    #[serde(default = "default_max_skills_per_task")]
    pub max_skills_per_task: usize,
    /// Maximum total execution steps before the flow stops.
    pub max_execution_steps: usize,
    /// Whether to perform environment preparation checks.
    pub enable_env_check: bool,
    /// Whether to fall back to universal tools when no skills match.
    #[serde(default)]
    pub fallback_to_universal_tools: bool,
    /// Maximum number of skills to execute in parallel.
    /// Skills that require write locks on the same resource are serialized.
    /// Default: 3 (balances speed vs. resource contention).
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
}

fn default_max_skills_per_task() -> usize {
    5
}

fn default_max_concurrency() -> usize {
    3
}

impl Default for FullAutoConfig {
    fn default() -> Self {
        Self {
            min_match_score: DEFAULT_MIN_MATCH_SCORE,
            max_skills_to_execute: 5,
            max_skills_per_task: 5,
            max_execution_steps: 20,
            enable_env_check: true,
            fallback_to_universal_tools: true,
            max_concurrency: default_max_concurrency(),
        }
    }
}

// ---------------------------------------------------------------------------
// FullAutoFlow
// ---------------------------------------------------------------------------

/// Main orchestrator for the full-auto execution flow.
///
/// Owns references to the `SkillRegistry` and `ToolRegistry` and coordinates
/// the parse → discover → prepare → execute → report pipeline.
pub struct FullAutoFlow {
    pub(crate) skill_registry: Arc<StdRwLock<SkillRegistry>>,
    pub(crate) tool_registry: Arc<ToolRegistry>,
    pub(crate) config: FullAutoConfig,
    /// Fast-path cache for parsing, discovery, environment, and route matching.
    pub(crate) cache: Arc<FastPathCache>,
    /// Optional dynamic threshold learner for adaptive skill matching.
    pub(crate) threshold_learner: Option<Mutex<ThresholdLearner>>,
    /// Complexity estimator for dynamic BrainLoop iteration tuning.
    pub(crate) complexity_estimator: ComplexityEstimator,
    /// Tool recommender for suggesting complementary tools.
    pub(crate) tool_recommender: Mutex<ToolRecommender>,
    /// Optional skill market registry for external skill discovery.
    pub(crate) skill_market: Option<SkillMarketRegistry>,
    /// Semaphore for limiting concurrent skill execution.
    pub(crate) semaphore: Arc<tokio::sync::Semaphore>,
    /// Adaptive model selector for tracking skill execution outcomes.
    pub(crate) model_selector: AdaptiveModelSelector,
}

impl std::fmt::Debug for FullAutoFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FullAutoFlow")
            .field("skill_registry", &"Arc<StdRwLock<SkillRegistry>>")
            .field("tool_registry", &"Arc<ToolRegistry>")
            .field("config", &self.config)
            .field("cache", &"FastPathCache")
            .field("complexity_estimator", &"ComplexityEstimator")
            .field("tool_recommender", &"Mutex<ToolRecommender>")
            .field("model_selector", &"AdaptiveModelSelector")
            .finish()
    }
}

impl FullAutoFlow {
    /// Create a new `FullAutoFlow` with default configuration and default routes.
    pub fn new(
        skill_registry: Arc<StdRwLock<SkillRegistry>>,
        tool_registry: Arc<ToolRegistry>,
    ) -> Self {
        let max_concurrency = FullAutoConfig::default().max_concurrency;
        Self {
            skill_registry,
            tool_registry,
            config: FullAutoConfig::default(),
            cache: Arc::new(FastPathCache::with_default_routes()),
            threshold_learner: None,
            complexity_estimator: ComplexityEstimator::new(),
            tool_recommender: Mutex::new(ToolRecommender::new()),
            skill_market: None,
            semaphore: Arc::new(tokio::sync::Semaphore::new(max_concurrency)),
            model_selector: AdaptiveModelSelector::new(),
        }
    }

    /// Create a new `FullAutoFlow` with explicit skill and tool registries.
    ///
    /// This is an alias for `new()` with an explicit name that makes it clear
    /// real registries are being injected (as opposed to default/empty ones).
    pub fn new_with_registries(
        skill_registry: Arc<StdRwLock<SkillRegistry>>,
        tool_registry: Arc<ToolRegistry>,
    ) -> Self {
        Self::new(skill_registry, tool_registry)
    }

    /// Return a reference to the internal FastPathCache, so callers can
    /// wire it to the CacheWarmingEngine for unified hit/miss tracking.
    pub fn fast_path_cache(&self) -> Arc<FastPathCache> {
        Arc::clone(&self.cache)
    }

    /// Attach a threshold learner for dynamic skill-match threshold tuning.
    pub fn with_threshold_learner(mut self, learner: ThresholdLearner) -> Self {
        self.threshold_learner = Some(Mutex::new(learner));
        self
    }

    /// Record a trial outcome for the threshold learner.
    ///
    /// Called after a skill match is executed to tune future thresholds.
    /// `false_positive` — the matched skill was inappropriate.
    /// `missed_match` — a relevant skill was not matched.
    pub fn record_match_outcome(&self, success: bool, false_positive: bool, missed_match: bool) {
        if let Some(ref learner) = self.threshold_learner {
            let mut guard = learner.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("threshold_learner lock poisoned, recovering");
                poisoned.into_inner()
            });
            let current = guard.get_optimal_threshold("skill_match");
            guard.record_trial(
                "skill_match",
                current,
                success,
                false_positive,
                missed_match,
            );
        }
    }

    /// Get the effective minimum match score, which may be dynamically
    /// learned if a ThresholdLearner is attached.
    pub fn effective_min_match_score(&self) -> f64 {
        if let Some(ref learner) = self.threshold_learner {
            let guard = match learner.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::warn!("[B48] learner lock poisoned, recovering");
                    poisoned.into_inner()
                }
            };
            guard.get_optimal_threshold("skill_match")
        } else {
            self.config.min_match_score
        }
    }

    /// Create a new `FullAutoFlow` with a custom configuration.
    #[cfg(test)]
    pub fn with_config(
        skill_registry: Arc<StdRwLock<SkillRegistry>>,
        tool_registry: Arc<ToolRegistry>,
        config: FullAutoConfig,
    ) -> Self {
        let max_concurrency = config.max_concurrency;
        Self {
            skill_registry,
            tool_registry,
            config,
            cache: Arc::new(FastPathCache::new()),
            threshold_learner: None,
            complexity_estimator: ComplexityEstimator::new(),
            tool_recommender: Mutex::new(ToolRecommender::new()),
            skill_market: None,
            semaphore: Arc::new(tokio::sync::Semaphore::new(max_concurrency)),
            model_selector: AdaptiveModelSelector::new(),
        }
    }

    /// Create a new `FullAutoFlow` with a custom cache.
    /// Only used in tests; production uses `with_default_routes()`.
    #[cfg(test)]
    pub fn with_cache(
        skill_registry: Arc<StdRwLock<SkillRegistry>>,
        tool_registry: Arc<ToolRegistry>,
        cache: Arc<FastPathCache>,
    ) -> Self {
        Self {
            skill_registry,
            tool_registry,
            config: FullAutoConfig::default(),
            cache,
            threshold_learner: None,
            complexity_estimator: ComplexityEstimator::new(),
            tool_recommender: Mutex::new(ToolRecommender::new()),
            skill_market: None,
            semaphore: Arc::new(tokio::sync::Semaphore::new(
                FullAutoConfig::default().max_concurrency,
            )),
            model_selector: AdaptiveModelSelector::new(),
        }
    }

    /// Enable the skill marketplace, allowing the flow to search for
    /// external skills during the discovery phase.
    ///
    /// Returns an error if the marketplace registry cannot be created
    /// (e.g. temp directory unwritable, DNS resolution failure).
    pub fn enable_skill_market(&mut self) -> Result<()> {
        let cache_dir = std::env::temp_dir().join("go-on-skill-market");
        self.skill_market = Some(
            SkillMarketRegistry::new(
                "https://marketplace.go-on.dev",
                cache_dir,
                self.skill_registry.clone(),
            )
            .map_err(|e| anyhow::anyhow!("failed to create skill market registry: {e}"))?,
        );
        tracing::info!("Skill marketplace enabled for discovery phase");
        Ok(())
    }
}

// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Arc;

    use serde_json::Value;

    use crate::orchestration::fast_path_cache::FastPathCache;
    use crate::orchestration::skill::Skill;
    use crate::orchestration::threshold_learner::ThresholdLearner;
    use crate::orchestration::tool::ToolRegistry;

    // ── Helpers ────────────────────────────────────────────────────────

    /// A simple skill that returns a fixed value.
    struct FixedSkill {
        name: String,
        desc: String,
        output: Value,
        fail: bool,
    }

    #[async_trait]
    impl Skill for FixedSkill {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            &self.desc
        }

        async fn execute(&self, _input: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
            if self.fail {
                anyhow::bail!("intentional failure");
            }
            Ok(self.output.clone())
        }
    }

    fn setup_registry() -> Arc<StdRwLock<SkillRegistry>> {
        let mut reg = SkillRegistry::default();

        // Register two skills: one for code fixes, one for documentation.
        let fix_skill = FixedSkill {
            name: "code_fixer".to_string(),
            desc: "Fixes bugs and errors in source code files".to_string(),
            output: serde_json::json!({"fixed": true, "patches": 3}),
            fail: false,
        };
        reg.register(Arc::new(fix_skill)).unwrap();

        let docs_skill = FixedSkill {
            name: "doc_writer".to_string(),
            desc: "Generates documentation from code comments and structure".to_string(),
            output: serde_json::json!({"documented": true, "pages": 2}),
            fail: false,
        };
        reg.register(Arc::new(docs_skill)).unwrap();

        let failing_skill = FixedSkill {
            name: "flakey_tool".to_string(),
            desc: "Unreliable tool for testing failure handling".to_string(),
            output: serde_json::json!({}),
            fail: true,
        };
        reg.register(Arc::new(failing_skill)).unwrap();

        Arc::new(StdRwLock::new(reg))
    }

    fn make_flow(registry: Arc<StdRwLock<SkillRegistry>>) -> FullAutoFlow {
        let tool_registry = Arc::new(ToolRegistry::new_empty());
        FullAutoFlow::new(registry, tool_registry)
    }

    #[test]
    fn with_cache_constructor_smoke() {
        let registry = setup_registry();
        let tool_registry = Arc::new(ToolRegistry::new_empty());
        let cache = Arc::new(FastPathCache::new());
        let flow = FullAutoFlow::with_cache(registry, tool_registry, cache);

        let intent = flow.parse_task("- goal: validate custom cache constructor");
        assert!(intent.has_goals());
    }

    // ── TaskIntent tests ───────────────────────────────────────────────

    #[test]
    fn task_intent_default_goal_text() {
        let intent = TaskIntent {
            goals: vec!["fix bugs".to_string(), "add tests".to_string()],
            constraints: vec![],
            prerequisites: vec![],
            deliverables: vec![],
        };
        assert_eq!(intent.goal_text(), "fix bugs add tests");
        assert!(intent.has_goals());
        assert_eq!(intent.constraint_count(), 0);
    }

    #[test]
    fn task_intent_empty_goals() {
        let intent = TaskIntent {
            goals: vec![],
            constraints: vec![],
            prerequisites: vec![],
            deliverables: vec![],
        };
        assert!(!intent.has_goals());
        assert!(intent.goal_text().is_empty());
    }

    // ── Parse tests ────────────────────────────────────────────────────

    #[test]
    fn parse_task_with_bullet_labels() {
        let flow = make_flow(setup_registry());
        let task = "\
- goal: fix the login timeout bug
- constraint: must not break existing tests
- require: rust compiler
- deliverable: patched main.rs
";
        let intent = flow.parse_task(task);
        assert_eq!(intent.goals.len(), 1);
        assert!(intent.goals[0].contains("login timeout bug"));
        assert_eq!(intent.constraints.len(), 1);
        assert!(intent.constraints[0].contains("not break"));
        assert_eq!(intent.prerequisites.len(), 1);
        assert!(intent.prerequisites[0].contains("rust compiler"));
        assert_eq!(intent.deliverables.len(), 1);
        assert!(intent.deliverables[0].contains("patched main.rs"));
    }

    #[test]
    fn parse_task_plain_sentence_falls_back_to_goal() {
        let flow = make_flow(setup_registry());
        let task = "Refactor the authentication module to use JWT tokens";
        let intent = flow.parse_task(task);
        assert_eq!(intent.goals.len(), 1);
        assert!(intent.goals[0].contains("Refactor"));
        assert!(intent.constraints.is_empty());
    }

    #[test]
    fn parse_task_unclassified_bullets_become_goals() {
        let flow = make_flow(setup_registry());
        let task = "- implement user login\n- add rate limiting";
        let intent = flow.parse_task(task);
        assert_eq!(intent.goals.len(), 2);
        assert!(intent.goals.iter().any(|g| g.contains("login")));
        assert!(intent.goals.iter().any(|g| g.contains("rate limiting")));
    }

    #[test]
    fn parse_task_empty_string() {
        let flow = make_flow(setup_registry());
        let intent = flow.parse_task("");
        assert!(intent.goals.is_empty());
    }

    #[test]
    fn parse_task_short_string_falls_back() {
        let flow = make_flow(setup_registry());
        let intent = flow.parse_task("hi");
        // "hi" is only 2 chars, below the 5-char fallback threshold.
        assert!(intent.goals.is_empty());
    }

    // ── Discovery tests ────────────────────────────────────────────────

    #[test]
    fn discover_skills_matches_code_fixer() {
        let registry = setup_registry();
        let flow = make_flow(registry);

        let intent = TaskIntent {
            goals: vec!["fix bugs in source code".to_string()],
            constraints: vec![],
            prerequisites: vec![],
            deliverables: vec![],
        };

        let matches = flow.discover_skills(&intent);
        assert!(!matches.is_empty(), "Expected at least one match");

        // code_fixer should outrank the others for this goal.
        let top = &matches[0];
        assert_eq!(top.name, "code_fixer");
        assert!(top.score >= DEFAULT_MIN_MATCH_SCORE);
    }

    #[test]
    fn discover_skills_respects_min_score() {
        let registry = setup_registry();
        let flow = FullAutoFlow::with_config(
            registry,
            Arc::new(ToolRegistry::new_empty()),
            FullAutoConfig {
                min_match_score: 5.0, // unreachable
                max_skills_to_execute: 5,
                max_skills_per_task: 5,
                max_execution_steps: 20,
                enable_env_check: true,
                fallback_to_universal_tools: true,
                max_concurrency: 3,
            },
        );

        let intent = TaskIntent {
            goals: vec!["fix bugs".to_string()],
            constraints: vec![],
            prerequisites: vec![],
            deliverables: vec![],
        };

        let matches = flow.discover_skills(&intent);
        assert!(matches.is_empty(), "Expected no matches with score > 5.0");
    }

    // ── Environment tests ──────────────────────────────────────────────

    #[test]
    fn prepare_environment_with_prerequisites() {
        let registry = setup_registry();
        let flow = make_flow(registry);

        let intent = TaskIntent {
            goals: vec!["build project".to_string()],
            constraints: vec![],
            prerequisites: vec!["rust".to_string(), "cargo".to_string()],
            deliverables: vec![],
        };

        let env = flow.prepare_environment(&intent);
        assert!(env.dependencies_checked);
        // Prerequisites exist but haven't been resolved yet; runtime not ready
        assert!(!env.runtime_ready);
        assert_eq!(env.env_snapshot.get("mode").unwrap(), "full_auto");
    }

    #[test]
    fn prepare_environment_without_prerequisites() {
        let registry = setup_registry();
        let flow = make_flow(registry);

        let intent = TaskIntent {
            goals: vec!["do something".to_string()],
            constraints: vec![],
            prerequisites: vec![],
            deliverables: vec![],
        };

        let env = flow.prepare_environment(&intent);
        assert!(env.dependencies_checked);
        // Empty prerequisites means trivially ready
        assert!(env.runtime_ready);
    }

    #[test]
    fn prepare_environment_skipped_when_disabled() {
        let registry = setup_registry();
        let flow = FullAutoFlow::with_config(
            registry,
            Arc::new(ToolRegistry::new_empty()),
            FullAutoConfig {
                enable_env_check: false,
                ..Default::default()
            },
        );

        let intent = TaskIntent {
            goals: vec!["x".to_string()],
            constraints: vec![],
            prerequisites: vec![],
            deliverables: vec![],
        };

        let env = flow.prepare_environment(&intent);
        assert!(env.is_ready()); // both true when check is disabled
    }

    // ── Execution tests ────────────────────────────────────────────────

    #[tokio::test]
    async fn run_flow_with_matching_skills() {
        let registry = setup_registry();
        let mut flow = make_flow(registry);

        let report = flow
            .run("- goal: fix bugs in the source code\n- constraint: keep it simple")
            .await;

        assert!(report.task_intent.has_goals());
        assert!(!report.matched_skills.is_empty());
        assert!(report.environment_status.dependencies_checked);

        // At least one skill should have been executed successfully.
        let successful: Vec<_> = report.execution_log.iter().filter(|s| s.success).collect();
        assert!(
            !successful.is_empty(),
            "Expected at least one successful execution step"
        );

        assert!(report.final_output.is_some());
    }

    #[tokio::test]
    async fn run_flow_no_matching_skills_produces_empty_report() {
        let registry = setup_registry();
        let mut flow = FullAutoFlow::with_config(
            registry,
            Arc::new(ToolRegistry::new_empty()),
            FullAutoConfig {
                min_match_score: 5.0,
                fallback_to_universal_tools: false,
                ..Default::default()
            },
        );

        let report = flow
            .run("- goal: do something extremely obscure and unrelated")
            .await;

        assert!(report.matched_skills.is_empty());
        assert!(report.execution_log.is_empty());
        // No errors for empty matches — that is not an error condition.
        assert!(report.final_output.is_none());
    }

    #[tokio::test]
    async fn run_flow_records_failed_skills() {
        let registry = setup_registry();

        // Use the flow with the flakey_tool registered.
        let tool_registry = Arc::new(ToolRegistry::new_empty());
        let mut flow = FullAutoFlow::new(registry, tool_registry);

        let report = flow
            .run("- goal: test the flakey tool execution path")
            .await;

        // At least one step should have a failure entry for flakey_tool.
        // The flakey_tool should be matched and executed, failing.
        if !report.execution_log.is_empty() {
            let has_failure = report
                .execution_log
                .iter()
                .any(|s| s.skill_name == "flakey_tool" && !s.success);
            assert!(
                has_failure,
                "Expected flakey_tool to fail when it was matched by discovery"
            );
        }
    }

    // ── Report tests ───────────────────────────────────────────────────

    #[test]
    fn report_is_success_checks_errors_and_steps() {
        let report = AutoExecutionReport {
            task_intent: TaskIntent {
                goals: vec![],
                constraints: vec![],
                prerequisites: vec![],
                deliverables: vec![],
            },
            matched_skills: vec![],
            environment_status: ExecutionEnvironment {
                dependencies_checked: true,
                runtime_ready: true,
                env_snapshot: HashMap::new(),
            },
            execution_log: vec![ExecutionStep {
                skill_name: "test".to_string(),
                input: serde_json::Value::Null,
                output: serde_json::Value::Null,
                success: true,
                duration_ms: 1,
                timestamp_ms: 0,
                error: None,
            }],
            final_output: None,
            errors: vec![],
            total_duration_ms: 1,
            cache_metrics: serde_json::json!({}),
            brain_loop_metrics: None,
        };
        assert!(report.is_success());
        assert_eq!(report.success_count(), 1);
        assert_eq!(report.failure_count(), 0);
    }

    #[test]
    fn report_is_success_false_when_errors_exist() {
        let report = AutoExecutionReport {
            task_intent: TaskIntent {
                goals: vec![],
                constraints: vec![],
                prerequisites: vec![],
                deliverables: vec![],
            },
            matched_skills: vec![],
            environment_status: ExecutionEnvironment {
                dependencies_checked: true,
                runtime_ready: true,
                env_snapshot: HashMap::new(),
            },
            execution_log: vec![],
            final_output: None,
            errors: vec!["something went wrong".to_string()],
            total_duration_ms: 0,
            cache_metrics: serde_json::json!({}),
            brain_loop_metrics: None,
        };
        assert!(!report.is_success());
    }

    #[test]
    fn report_counts_success_and_failure() {
        let report = AutoExecutionReport {
            task_intent: TaskIntent {
                goals: vec![],
                constraints: vec![],
                prerequisites: vec![],
                deliverables: vec![],
            },
            matched_skills: vec![],
            environment_status: ExecutionEnvironment {
                dependencies_checked: true,
                runtime_ready: true,
                env_snapshot: HashMap::new(),
            },
            execution_log: vec![
                ExecutionStep {
                    skill_name: "a".to_string(),
                    input: serde_json::Value::Null,
                    output: serde_json::Value::Null,
                    success: true,
                    duration_ms: 1,
                    timestamp_ms: 0,
                    error: None,
                },
                ExecutionStep {
                    skill_name: "b".to_string(),
                    input: serde_json::Value::Null,
                    output: serde_json::Value::Null,
                    success: false,
                    duration_ms: 2,
                    timestamp_ms: 1,
                    error: Some("fail".to_string()),
                },
            ],
            final_output: None,
            errors: vec![],
            total_duration_ms: 3,
            cache_metrics: serde_json::json!({}),
            brain_loop_metrics: None,
        };
        assert!(!report.is_success());
        assert_eq!(report.success_count(), 1);
        assert_eq!(report.failure_count(), 1);
    }

    // ── Serialization tests ────────────────────────────────────────────

    #[test]
    fn task_intent_roundtrip_json() {
        let intent = TaskIntent {
            goals: vec!["g1".to_string()],
            constraints: vec!["c1".to_string()],
            prerequisites: vec!["p1".to_string()],
            deliverables: vec!["d1".to_string()],
        };
        let json = serde_json::to_value(&intent).unwrap();
        let deserialized: TaskIntent = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.goals, intent.goals);
        assert_eq!(deserialized.constraints, intent.constraints);
        assert_eq!(deserialized.prerequisites, intent.prerequisites);
        assert_eq!(deserialized.deliverables, intent.deliverables);
    }

    #[test]
    fn full_auto_config_default() {
        let config = FullAutoConfig::default();
        assert!((config.min_match_score - DEFAULT_MIN_MATCH_SCORE).abs() < 1e-9);
        assert_eq!(config.max_skills_to_execute, 5);
        assert_eq!(config.max_execution_steps, 20);
        assert!(config.enable_env_check);
    }

    #[test]
    fn execution_environment_is_ready() {
        let env = ExecutionEnvironment {
            dependencies_checked: true,
            runtime_ready: true,
            env_snapshot: HashMap::new(),
        };
        assert!(env.is_ready());
    }

    #[test]
    fn execution_environment_not_ready() {
        let env = ExecutionEnvironment {
            dependencies_checked: false,
            runtime_ready: true,
            env_snapshot: HashMap::new(),
        };
        assert!(!env.is_ready());
    }

    #[test]
    fn threshold_learner_integration_smoke() {
        let tool_registry = Arc::new(ToolRegistry::default());
        let skill_registry = setup_registry();

        let learner = ThresholdLearner::default_learner();
        let flow = FullAutoFlow::new(skill_registry, tool_registry).with_threshold_learner(learner);

        // Initially at 0.40.
        let initial = flow.effective_min_match_score();
        assert!((initial - 0.40).abs() < 0.001);

        // Record a false positive — should raise threshold.
        flow.record_match_outcome(false, true, false);
        let after_fp = flow.effective_min_match_score();
        assert!(after_fp > initial, "False positive should raise threshold");

        // Record a missed match — should lower threshold.
        flow.record_match_outcome(false, false, true);
        let after_miss = flow.effective_min_match_score();
        assert!(after_miss < after_fp, "Missed match should lower threshold");

        // Record a success — should keep threshold stable.
        let before_success = flow.effective_min_match_score();
        flow.record_match_outcome(true, false, false);
        assert_eq!(flow.effective_min_match_score(), before_success);
    }

    #[test]
    fn effective_min_match_score_falls_back_to_config_when_no_learner() {
        let tool_registry = Arc::new(ToolRegistry::default());
        let skill_registry = setup_registry();
        let flow = FullAutoFlow::new(skill_registry, tool_registry);

        // Without a learner, should fall back to config value.
        assert_eq!(flow.effective_min_match_score(), DEFAULT_MIN_MATCH_SCORE);
    }

    #[test]
    fn discover_skills_uses_dynamic_threshold() {
        let tool_registry = Arc::new(ToolRegistry::default());
        let skill_registry = setup_registry();

        // Create a flow with a learner that has a very high threshold.
        let mut learner = ThresholdLearner::new(1.0, 0.95);
        learner.adjust_threshold("skill_match", 0.0); // reset to exactly 0.95
        let flow = FullAutoFlow::new(skill_registry.clone(), tool_registry.clone())
            .with_threshold_learner(learner);

        // With threshold 0.95, only very good matches pass.
        let intent = TaskIntent {
            goals: vec!["fix bugs in source code".to_string()],
            constraints: vec![],
            prerequisites: vec![],
            deliverables: vec![],
        };

        let matches = flow.discover_skills(&intent);
        // The point is that discover_skills uses the dynamic threshold.
        let score = flow.effective_min_match_score();
        assert!((score - 0.95).abs() < 0.001);

        // All returned matches should have score >= effective threshold.
        for m in &matches {
            assert!(m.score >= 0.95, "{} scored {}", m.name, m.score);
        }
    }
}
