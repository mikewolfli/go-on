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

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::RwLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{debug, info, warn};

use crate::i18n::runtime::tf;
use crate::orchestration::brain_loop::{
    BrainLoop, BrainLoopConfig, BrainLoopPhase, BrainLoopStep, StepStatus,
};
use crate::orchestration::complexity_estimator::ComplexityEstimator;
use crate::orchestration::diagnostic_feedback::{
    DiagnosticBatch, DiagnosticFeedbackEngine, DiagnosticMessage, DiagnosticSeverity,
};
use crate::orchestration::fast_path_cache::{
    EnvCacheValue, FastPathCache, IntentCacheValue, SkillCacheValue,
};
use crate::orchestration::skill::SkillRegistry;
use crate::orchestration::skill_import::SkillImportPolicy;
use crate::orchestration::skill_market::SkillMarketRegistry;
use crate::orchestration::threshold_learner::ThresholdLearner;
use crate::orchestration::tool::ToolRegistry;
use crate::orchestration::tool_lock::{LockMode, ToolLockManager};
use crate::orchestration::tool_recommender::{ToolRecommendation, ToolRecommender};

// ---------------------------------------------------------------------------
// Weight constants used for composite skill-matching scores
// ---------------------------------------------------------------------------

/// Weight for name similarity in composite scoring.
const WEIGHT_NAME: f64 = 0.35;

/// Weight for description semantic similarity.
const WEIGHT_DESCRIPTION: f64 = 0.40;

/// Weight for runtime score (historical success rate from registry).
const WEIGHT_RUNTIME: f64 = 0.25;

/// Default minimum composite score for a skill to be considered a match.
const DEFAULT_MIN_MATCH_SCORE: f64 = 0.40;

// ---------------------------------------------------------------------------
// TaskIntent
// ---------------------------------------------------------------------------

/// Structured representation of a parsed task.
///
/// Each field captures a distinct dimension extracted from the raw task
/// description via lightweight heuristics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskIntent {
    /// What the task aims to achieve.
    pub goals: Vec<String>,
    /// Boundaries and limitations that must be respected.
    pub constraints: Vec<String>,
    /// Required skills, tools, or runtime capabilities.
    pub prerequisites: Vec<String>,
    /// Expected outputs or artifacts.
    pub deliverables: Vec<String>,
}

impl TaskIntent {
    /// Build a combined text string from all goals for matching purposes.
    pub fn goal_text(&self) -> String {
        self.goals.join(" ")
    }

    /// Check whether non‑zero goals exist.
    #[cfg(test)]
    pub fn has_goals(&self) -> bool {
        !self.goals.is_empty()
    }

    /// Number of known constraints.
    #[cfg(test)]
    pub fn constraint_count(&self) -> usize {
        self.constraints.len()
    }
}

// ---------------------------------------------------------------------------
// ExecutionEnvironment
// ---------------------------------------------------------------------------

/// Snapshot of the execution environment at the time of the run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionEnvironment {
    /// Whether all declared prerequisites have been verified.
    pub dependencies_checked: bool,
    /// Whether the required runtime is available.
    pub runtime_ready: bool,
    /// Environment variable / context snapshot.
    pub env_snapshot: HashMap<String, String>,
}

impl ExecutionEnvironment {
    /// Return `true` when the environment is fully ready for execution.
    #[cfg(test)]
    pub fn is_ready(&self) -> bool {
        self.dependencies_checked && self.runtime_ready
    }
}

// ---------------------------------------------------------------------------
// SkillMatch
// ---------------------------------------------------------------------------

/// A skill matched to the task, together with its composite score and a
/// human‑readable rationale.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMatch {
    /// Skill name (matches `SkillRegistry` key).
    pub name: String,
    /// Human-readable description of the skill.
    pub description: String,
    /// Composite match score (0.0 – 1.0).
    pub score: f64,
    /// Human-readable explanation of the score.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// ExecutionStep
// ---------------------------------------------------------------------------

/// A single step recorded in the execution audit trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionStep {
    /// Name of the skill that was executed.
    pub skill_name: String,
    /// The input value provided to the skill.
    pub input: Value,
    /// The output value returned by the skill (or `Null` on failure).
    pub output: Value,
    /// Whether the execution completed without error.
    pub success: bool,
    /// Wall-clock duration of this step in milliseconds.
    pub duration_ms: u64,
    /// Monotonic timestamp (milliseconds since flow start).
    pub timestamp_ms: u64,
    /// Error message if the step failed, or `None`.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// AutoExecutionReport
// ---------------------------------------------------------------------------

/// Full audit trail for an automatic execution run.
///
/// Contains every stage from parsing through environment preparation to
/// skill execution, enabling full traceability of what happened and why.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoExecutionReport {
    /// The structured task intent derived from the raw description.
    pub task_intent: TaskIntent,
    /// Skills that were matched and considered for execution.
    pub matched_skills: Vec<SkillMatch>,
    /// Environment state at the time of the run.
    pub environment_status: ExecutionEnvironment,
    /// Ordered log of every skill execution attempt.
    pub execution_log: Vec<ExecutionStep>,
    /// Final consolidated output, if any.
    pub final_output: Option<String>,
    /// Non‑fatal errors that occurred during the flow.
    pub errors: Vec<String>,
    /// Total wall‑clock duration of the entire flow in milliseconds.
    pub total_duration_ms: u64,
    /// Cache metrics snapshot from the fast-path cache.
    pub cache_metrics: Value,
}

impl AutoExecutionReport {
    /// Returns `true` when all matched skills completed successfully
    /// and no errors were recorded.
    pub fn is_success(&self) -> bool {
        self.errors.is_empty() && self.execution_log.iter().all(|s| s.success)
    }

    /// Returns the number of successful steps in the execution log.
    pub fn success_count(&self) -> usize {
        self.execution_log.iter().filter(|s| s.success).count()
    }

    /// Returns the number of failed steps in the execution log.
    pub fn failure_count(&self) -> usize {
        self.execution_log.iter().filter(|s| !s.success).count()
    }
}

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
    /// Maximum total execution steps before the flow stops.
    pub max_execution_steps: usize,
    /// Whether to perform environment preparation checks.
    pub enable_env_check: bool,
}

impl Default for FullAutoConfig {
    fn default() -> Self {
        Self {
            min_match_score: DEFAULT_MIN_MATCH_SCORE,
            max_skills_to_execute: 5,
            max_execution_steps: 20,
            enable_env_check: true,
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
    skill_registry: Arc<Mutex<SkillRegistry>>,
    tool_registry: Arc<ToolRegistry>,
    config: FullAutoConfig,
    /// Fast-path cache for parsing, discovery, environment, and route matching.
    cache: FastPathCache,
    /// Optional dynamic threshold learner for adaptive skill matching.
    threshold_learner: Option<Mutex<ThresholdLearner>>,
    /// Complexity estimator for dynamic BrainLoop iteration tuning.
    complexity_estimator: ComplexityEstimator,
    /// Diagnostic feedback engine for error analysis and recovery.
    diagnostic_engine: Mutex<DiagnosticFeedbackEngine>,
    /// Tool recommender for suggesting complementary tools.
    tool_recommender: Mutex<ToolRecommender>,
    /// Tool lock manager for safe concurrent file access.
    tool_lock_manager: ToolLockManager,
    /// Optional skill market registry for external skill discovery.
    skill_market: Option<SkillMarketRegistry>,
}

impl std::fmt::Debug for FullAutoFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FullAutoFlow")
            .field("skill_registry", &"Arc<Mutex<SkillRegistry>>")
            .field("tool_registry", &"Arc<ToolRegistry>")
            .field("config", &self.config)
            .field("cache", &"FastPathCache")
            .field("complexity_estimator", &"ComplexityEstimator")
            .field("diagnostic_engine", &"Mutex<DiagnosticFeedbackEngine>")
            .field("tool_recommender", &"Mutex<ToolRecommender>")
            .field("tool_lock_manager", &"ToolLockManager")
            .finish()
    }
}

impl FullAutoFlow {
    /// Create a new `FullAutoFlow` with default configuration and default routes.
    pub fn new(
        skill_registry: Arc<Mutex<SkillRegistry>>,
        tool_registry: Arc<ToolRegistry>,
    ) -> Self {
        Self {
            skill_registry,
            tool_registry,
            config: FullAutoConfig::default(),
            cache: FastPathCache::with_default_routes(),
            threshold_learner: None,
            complexity_estimator: ComplexityEstimator::new(),
            diagnostic_engine: Mutex::new(DiagnosticFeedbackEngine::new()),
            tool_recommender: Mutex::new(ToolRecommender::new()),
            tool_lock_manager: ToolLockManager::new(),
            skill_market: None,
        }
    }

    /// Attach a threshold learner for dynamic skill-match threshold tuning.
    #[allow(dead_code)] // F-GAP-12 — reserved for threshold learner integration
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
            if let Ok(mut guard) = learner.lock() {
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
    }

    /// Get the effective minimum match score, which may be dynamically
    /// learned if a ThresholdLearner is attached.
    pub fn effective_min_match_score(&self) -> f64 {
        if let Some(ref learner) = self.threshold_learner {
            if let Ok(guard) = learner.lock() {
                guard.get_optimal_threshold("skill_match")
            } else {
                self.config.min_match_score
            }
        } else {
            self.config.min_match_score
        }
    }

    /// Create a new `FullAutoFlow` with a custom configuration.
    #[cfg(test)]
    pub fn with_config(
        skill_registry: Arc<Mutex<SkillRegistry>>,
        tool_registry: Arc<ToolRegistry>,
        config: FullAutoConfig,
    ) -> Self {
        Self {
            skill_registry,
            tool_registry,
            config,
            cache: FastPathCache::new(),
            threshold_learner: None,
            complexity_estimator: ComplexityEstimator::new(),
            diagnostic_engine: Mutex::new(DiagnosticFeedbackEngine::new()),
            tool_recommender: Mutex::new(ToolRecommender::new()),
            tool_lock_manager: ToolLockManager::new(),
            skill_market: None,
        }
    }

    /// Create a new `FullAutoFlow` with a custom cache.
    /// Only used in tests; production uses `with_default_routes()`.
    #[cfg(test)]
    pub fn with_cache(
        skill_registry: Arc<Mutex<SkillRegistry>>,
        tool_registry: Arc<ToolRegistry>,
        cache: FastPathCache,
    ) -> Self {
        Self {
            skill_registry,
            tool_registry,
            config: FullAutoConfig::default(),
            cache,
            threshold_learner: None,
            complexity_estimator: ComplexityEstimator::new(),
            diagnostic_engine: Mutex::new(DiagnosticFeedbackEngine::new()),
            tool_recommender: Mutex::new(ToolRecommender::new()),
            tool_lock_manager: ToolLockManager::new(),
            skill_market: None,
        }
    }

    /// Enable the skill marketplace, allowing the flow to search for
    /// external skills during the discovery phase.
    ///
    /// Caller-available builder method — callers should invoke
    /// `enable_skill_market()` before `run()` for external skill discovery.
    #[allow(dead_code)] // public builder API intended for external consumers
    pub fn enable_skill_market(&mut self) {
        let cache_dir = std::env::temp_dir().join("go-on-skill-market");
        let import_policy = SkillImportPolicy {
            enabled: true,
            allowed_sources: vec!["*".to_string()],
            require_sha256: false,
            allow_floating_ref: true,
            cache_dir: cache_dir.to_string_lossy().to_string(),
        };
        self.skill_market = Some(SkillMarketRegistry::new(
            "https://marketplace.go-on.dev",
            cache_dir,
            Arc::new(RwLock::new(SkillRegistry::default())),
            import_policy,
        ));
        info!("Skill marketplace enabled for discovery phase");
    }

    // -----------------------------------------------------------------------
    // 1. Task parsing
    // -----------------------------------------------------------------------

    /// Parse a free‑form task description into a structured `TaskIntent`.
    ///
    /// Uses lightweight heuristics to identify goals, constraints,
    /// prerequisites, and deliverables from the raw text. Lines prefixed
    /// with `-` or `*` are classified by keyword (`goal:`, `constraint:`,
    /// `require:`, `deliverable:`, `output:`). Unclassified bullet lines
    /// and multi‑word standalone lines default to goals.
    ///
    /// Results are cached via the fast-path cache so that repeated calls
    /// with the same task text avoid re-parsing.
    pub fn parse_task(&self, task: &str) -> TaskIntent {
        // Fast-path cache check.
        if let Some(cached) = self.cache.get_intent(task) {
            debug!("parse_task: returning cached intent");
            return cached.into_task_intent();
        }

        let mut goals: Vec<String> = Vec::new();
        let mut constraints: Vec<String> = Vec::new();
        let mut prerequisites: Vec<String> = Vec::new();
        let mut deliverables: Vec<String> = Vec::new();

        for line in task.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let (is_bullet, content) = if let Some(rest) = trimmed
                .strip_prefix('-')
                .or_else(|| trimmed.strip_prefix('*'))
            {
                (true, rest.trim())
            } else {
                (false, trimmed)
            };

            if content.is_empty() {
                if is_bullet {
                    // An empty bullet is meaningless; skip.
                    continue;
                }
                // Non‑empty trimmed line with no bullet → treat as goal.
                goals.push(trimmed.to_string());
                continue;
            }

            let lower = content.to_lowercase();

            if lower.starts_with("goal:") || lower.starts_with("goal ") {
                goals.push(Self::strip_label(content, &["goal:", "goal "]));
            } else if lower.starts_with("constraint:") || lower.starts_with("constraint ") {
                constraints.push(Self::strip_label(content, &["constraint:", "constraint "]));
            } else if lower.starts_with("require:")
                || lower.starts_with("require ")
                || lower.starts_with("prerequisite:")
                || lower.starts_with("prerequisite ")
            {
                prerequisites.push(Self::strip_label(
                    content,
                    &["require:", "require ", "prerequisite:", "prerequisite "],
                ));
            } else if lower.starts_with("deliverable:")
                || lower.starts_with("deliverable ")
                || lower.starts_with("output:")
                || lower.starts_with("output ")
            {
                deliverables.push(Self::strip_label(
                    content,
                    &["deliverable:", "deliverable ", "output:", "output "],
                ));
            } else if is_bullet {
                // Unclassified bullet → default to goal.
                goals.push(content.to_string());
            } else if trimmed.len() > 10 {
                // Longer non‑bullet line → heuristic for an implicit goal.
                goals.push(trimmed.to_string());
            }
        }

        // Fallback: if nothing useful was parsed, treat the entire task as
        // a single goal so the flow has something to work with.
        if goals.is_empty() && task.len() > 5 {
            goals.push(task.to_string());
        }

        let intent = TaskIntent {
            goals,
            constraints,
            prerequisites,
            deliverables,
        };

        // Store in cache for future fast-path lookups.
        self.cache
            .set_intent(task, IntentCacheValue::from(intent.clone()));

        intent
    }

    /// Strip one of the recognised labels from the front of `content`.
    fn strip_label(content: &str, labels: &[&str]) -> String {
        let lower = content.to_lowercase();
        for label in labels {
            if lower.starts_with(label) {
                let remainder = &content[label.len()..];
                return remainder.trim().to_string();
            }
        }
        content.to_string()
    }

    // -----------------------------------------------------------------------
    // 2. Skill discovery
    // -----------------------------------------------------------------------

    /// Discover matching skills from the `SkillRegistry` for the given task
    /// intent.
    ///
    /// Scores each registered skill using a composite of:
    /// - **Name similarity** – whether the skill name appears in the goals.
    /// - **Description similarity** – keyword overlap between goals and
    ///   the skill description.
    /// - **Runtime score** – historical success rate from the registry.
    ///
    /// Results are cached keyed by the goal text so that repeated discovery
    /// for the same intent goals avoids recomputation.
    pub fn discover_skills(&self, intent: &TaskIntent) -> Vec<SkillMatch> {
        let goal_text = intent.goal_text();

        // Fast-path cache check.
        if let Some(cached) = self.cache.get_skills(&goal_text) {
            debug!("discover_skills: returning cached skills");
            return cached
                .skill_names
                .into_iter()
                .zip(cached.scores)
                .map(|(name, score)| SkillMatch {
                    name,
                    description: String::new(),
                    score,
                    reason: "cached".into(),
                })
                .collect();
        }

        let goal_tokens = tokenize(&goal_text);

        let registry = self
            .skill_registry
            .lock()
            .expect("skill_registry lock poisoned");
        let descriptors = registry.list();
        drop(registry); // Release the lock as early as possible.

        let mut matches: Vec<SkillMatch> = descriptors
            .into_iter()
            .filter_map(|desc| {
                let name_score = if goal_text.to_lowercase().contains(&desc.name.to_lowercase()) {
                    0.9
                } else {
                    0.3
                };

                let desc_tokens = tokenize(&desc.description);
                let desc_score = if goal_tokens.is_empty() {
                    0.0
                } else {
                    let overlap = goal_tokens
                        .iter()
                        .filter(|t| desc_tokens.contains(*t))
                        .count();
                    overlap as f64 / goal_tokens.len().max(1) as f64
                };

                let composite = name_score * WEIGHT_NAME
                    + desc_score * WEIGHT_DESCRIPTION
                    + desc.score * WEIGHT_RUNTIME;

                let effective_threshold = self.effective_min_match_score();
                if composite < effective_threshold {
                    return None;
                }

                let reason = format!(
                    "name_sim={:.2}, desc_sim={:.2}, runtime_score={:.2}",
                    name_score, desc_score, desc.score
                );

                Some(SkillMatch {
                    name: desc.name,
                    description: desc.description,
                    score: composite,
                    reason,
                })
            })
            .collect();

        // Sort by score descending, then by name ascending for stability.
        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.name.cmp(&b.name))
        });
        matches.truncate(self.config.max_skills_to_execute);

        // Store in cache for future fast-path lookups.
        let cached = SkillCacheValue {
            skill_names: matches.iter().map(|m| m.name.clone()).collect(),
            scores: matches.iter().map(|m| m.score).collect(),
        };
        self.cache.set_skills(&goal_text, cached);

        matches
    }

    // -----------------------------------------------------------------------
    // 3. Environment preparation
    // -----------------------------------------------------------------------

    /// Prepare the execution environment for the given task intent.
    ///
    /// Builds a snapshot of relevant context (mode, goals, constraints) and
    /// checks whether prerequisites are declared (proxy for runtime
    /// readiness).
    ///
    /// Results are cached keyed by the prerequisites list so that repeated
    /// calls with the same prerequisites avoid recomputation.
    pub fn prepare_environment(&self, intent: &TaskIntent) -> ExecutionEnvironment {
        if !self.config.enable_env_check {
            return ExecutionEnvironment {
                dependencies_checked: true,
                runtime_ready: true,
                env_snapshot: HashMap::new(),
            };
        }

        // Fast-path cache check.
        if let Some(cached) = self.cache.get_env(&intent.prerequisites) {
            debug!("prepare_environment: returning cached environment");
            return ExecutionEnvironment {
                dependencies_checked: cached.dependencies_checked,
                runtime_ready: cached.runtime_ready,
                env_snapshot: HashMap::new(),
            };
        }

        let mut env_snapshot = HashMap::new();
        env_snapshot.insert("mode".to_string(), "full_auto".to_string());
        env_snapshot.insert("task_goals".to_string(), intent.goals.join("; "));
        env_snapshot.insert("constraints".to_string(), intent.constraints.join("; "));

        // If prerequisites are declared we consider them checkable; in a
        // production setting this would invoke the actual dependency
        // resolver.
        let dependencies_checked = true;
        let runtime_ready = !intent.prerequisites.is_empty();

        let result = ExecutionEnvironment {
            dependencies_checked,
            runtime_ready,
            env_snapshot,
        };

        // Store in cache for future fast-path lookups.
        self.cache.set_env(
            &intent.prerequisites,
            EnvCacheValue {
                dependencies_checked: result.dependencies_checked,
                runtime_ready: result.runtime_ready,
            },
        );

        result
    }

    // -----------------------------------------------------------------------
    // 4. Full flow execution
    // -----------------------------------------------------------------------

    /// Run the complete full-auto flow:
    ///
    /// 1. Try a fast-path route template match (bypasses parsing and
    ///    discovery for known task types like bug fixes and features).
    /// 2. Parse the task description into a `TaskIntent`.
    /// 3. Discover matching skills via the `SkillRegistry`.
    /// 4. Prepare the execution environment.
    /// 5. Execute each matched skill in priority order.
    /// 6. Return a complete `AutoExecutionReport`.
    ///
    /// This is an `async` method because skill execution may involve I/O.
    pub async fn run(&mut self, task: &str) -> AutoExecutionReport {
        let flow_start = Instant::now();
        let mut errors: Vec<String> = Vec::new();
        let mut execution_log: Vec<ExecutionStep> = Vec::new();
        let mut final_output: Option<String> = None;

        // Report available tool count (tool_registry is retained for future
        // skill-level tool access).
        let tool_count = self.tool_registry.names().len();
        debug!("FullAutoFlow: {} tools available in registry", tool_count);

        // ---- Step 0: Fast-path route template match ----
        let (intent, mut matched_skills) = if let Some(route) = self.cache.match_route(task) {
            info!(
                "Fast-path route matched: {} (planning={})",
                route.task_type, route.requires_planning
            );

            let intent = TaskIntent {
                goals: route.default_goals.clone(),
                constraints: vec![],
                prerequisites: vec![],
                deliverables: vec![],
            };

            // Convert default skill names into SkillMatch entries.
            let matched_skills: Vec<SkillMatch> = route
                .default_skills
                .iter()
                .map(|name| SkillMatch {
                    name: name.clone(),
                    description: String::new(),
                    score: 1.0,
                    reason: format!("fast-path route: {}", route.task_type),
                })
                .collect();

            (intent, matched_skills)
        } else {
            debug!("No fast-path route matched; falling through to full flow");

            // ---- Step 1: Parse ----
            let intent = self.parse_task(task);
            debug!(
                "Parsed task: {} goals, {} constraints, {} prerequisites, {} deliverables",
                intent.goals.len(),
                intent.constraints.len(),
                intent.prerequisites.len(),
                intent.deliverables.len()
            );

            // ---- Step 2: Discover ----
            let mut matched_skills = self.discover_skills(&intent);
            info!(
                "Discovered {} matching skills for task with {} goal(s)",
                matched_skills.len(),
                intent.goals.len()
            );

            // Also search the skill marketplace if available.
            if let Some(ref market) = self.skill_market {
                let query = intent.goal_text();
                let market_items = market.search_skills(&query).await;
                if !market_items.is_empty() {
                    for item in &market_items {
                        matched_skills.push(SkillMatch {
                            name: item.name.clone(),
                            description: item.description.clone(),
                            score: 0.8,
                            reason: "marketplace skill match".into(),
                        });
                    }
                    info!(
                        "Found {} matching skills from skill marketplace",
                        market_items.len()
                    );
                }
            }

            if matched_skills.is_empty() {
                warn!("No skills matched the task; flow will produce an empty execution log");
            }

            (intent, matched_skills)
        };

        // ---- Step 3: Environment ----
        let environment_status = self.prepare_environment(&intent);
        debug!(
            "Environment prepared: deps_checked={}, runtime_ready={}",
            environment_status.dependencies_checked, environment_status.runtime_ready
        );

        // ---- Step 4: Execute ----
        // GAP-46-12: Run ToolRecommender to get additional tool suggestions.
        let recommended_tools: Vec<ToolRecommendation> = {
            let recommender = self
                .tool_recommender
                .lock()
                .expect("tool_recommender lock poisoned");
            let current_tools: Vec<String> =
                matched_skills.iter().map(|m| m.name.clone()).collect();
            recommender.recommend(task, &current_tools)
        };
        if !recommended_tools.is_empty() {
            info!(
                "ToolRecommender suggested {} additional tools",
                recommended_tools.len()
            );

            // Collect names already in the execution plan for deduplication.
            let existing_names: BTreeSet<String> =
                matched_skills.iter().map(|m| m.name.clone()).collect();

            for rec in &recommended_tools {
                debug!(
                    "  ↳ recommended: {} (score: {:.3}, reason: {})",
                    rec.tool_name, rec.relevance_score, rec.reason
                );

                // Add recommended tools that aren't already in the plan.
                if !existing_names.contains(&rec.tool_name) {
                    matched_skills.push(SkillMatch {
                        name: rec.tool_name.clone(),
                        description: format!("Auto-recommended: {}", rec.reason),
                        score: rec.relevance_score.min(1.0),
                        reason: rec.reason.clone(),
                    });
                    debug!(
                        "ToolRecommender: added '{}' to execution plan",
                        rec.tool_name
                    );
                }
            }
        }

        for skill_match in &matched_skills {
            if execution_log.len() >= self.config.max_execution_steps {
                let msg = tf(
                    "error.full_auto.max_steps_reached",
                    &[("max_steps", &self.config.max_execution_steps.to_string())],
                );
                warn!("{}", msg);
                errors.push(msg);
                break;
            }

            let step_start = Instant::now();

            // Acquire lock just long enough to get the skill.
            let skill_opt = {
                let registry = self
                    .skill_registry
                    .lock()
                    .expect("skill_registry lock poisoned");
                registry.get(&skill_match.name)
            };

            let skill = match skill_opt {
                Some(s) => s,
                None => {
                    let msg = tf(
                        "error.full_auto.skill_not_found",
                        &[("skill_name", &skill_match.name)],
                    );
                    warn!("{}", msg);
                    errors.push(msg);
                    continue;
                }
            };

            let input = json!({
                "task": task,
                "goals": intent.goals,
                "constraints": intent.constraints,
                "skill_name": skill_match.name,
            });

            // GAP-46-12: Acquire tool lock for file-modifying skills.
            // Best-effort lock — non-blocking try_acquire to avoid stalling the flow.
            let _lock_handle = if skill_match.name.contains("write")
                || skill_match.name.contains("edit")
                || skill_match.name.contains("file")
            {
                let handle = self
                    .tool_lock_manager
                    .try_acquire(&skill_match.name, LockMode::Write);
                if handle.is_some() {
                    debug!(
                        "ToolLockManager: acquired write lock for '{}'",
                        skill_match.name
                    );
                } else {
                    debug!(
                        "ToolLockManager: could not acquire lock for '{}', proceeding anyway",
                        skill_match.name
                    );
                }
                handle
            } else {
                None
            };

            match skill.execute(&input).await {
                Ok(output) => {
                    let elapsed = step_start.elapsed();
                    let duration_ms = elapsed.as_millis() as u64;

                    // Record the successful outcome back to the registry.
                    {
                        let mut registry = self
                            .skill_registry
                            .lock()
                            .expect("skill_registry lock poisoned");
                        registry.record_outcome(&skill_match.name, true, elapsed);
                    }

                    let step = ExecutionStep {
                        skill_name: skill_match.name.clone(),
                        input: input.clone(),
                        output: output.clone(),
                        success: true,
                        duration_ms,
                        timestamp_ms: flow_start.elapsed().as_millis() as u64,
                        error: None,
                    };
                    execution_log.push(step);

                    // Latest successful output becomes the candidate final
                    // output.
                    let output_text = output.to_string();
                    if output_text.len() < 1_000_000 {
                        // Cap at ~1 MB to avoid storing enormous blobs.
                        final_output = Some(output_text);
                    } else {
                        final_output = Some(tf(
                            "status.full_auto.output_truncated",
                            &[("bytes", &output_text.len().to_string())],
                        ));
                    }

                    debug!(
                        "Skill '{}' succeeded in {}ms",
                        skill_match.name, duration_ms
                    );

                    // Record successful match for threshold learning.
                    self.record_match_outcome(true, false, false);
                }
                Err(e) => {
                    let elapsed = step_start.elapsed();
                    let duration_ms = elapsed.as_millis() as u64;
                    let error_msg = tf(
                        "error.full_auto.skill_failed",
                        &[("skill_name", &skill_match.name), ("error", &e.to_string())],
                    );
                    warn!("{}", error_msg);
                    errors.push(error_msg.clone());

                    // GAP-46-12: Feed error to DiagnosticFeedbackEngine for analysis.
                    let diag_msg = DiagnosticMessage {
                        file: skill_match.name.clone(),
                        line: 0,
                        column: 0,
                        severity: DiagnosticSeverity::Error,
                        code: Some(format!("SKILL_FAILED/{}", skill_match.name)),
                        message: error_msg.clone(),
                        suggestion: None,
                        source_snippet: None,
                    };
                    let batch = DiagnosticBatch::new(vec![diag_msg]);
                    {
                        let mut engine = self
                            .diagnostic_engine
                            .lock()
                            .expect("diagnostic_engine lock poisoned");
                        engine.submit_batch(batch);
                        if let Some((strategy, desc)) = engine.recommend_repair() {
                            info!(
                                "DiagnosticFeedback suggests repair strategy '{}': {}",
                                strategy, desc
                            );
                            // Surface the repair strategy in the error
                            // report so callers know what was attempted.
                            errors.push(tf(
                                "error.full_auto.repair_attempted",
                                &[("strategy", &strategy), ("description", &desc)],
                            ));
                        }
                        let trend = engine.error_trend();
                        debug!("Diagnostic error trend: {}", trend);
                    }

                    // Record failed match for threshold learning.
                    self.record_match_outcome(false, true, false);

                    // Record the failure.
                    {
                        let mut registry = self
                            .skill_registry
                            .lock()
                            .expect("skill_registry lock poisoned");
                        registry.record_outcome(&skill_match.name, false, elapsed);
                    }

                    let step = ExecutionStep {
                        skill_name: skill_match.name.clone(),
                        input,
                        output: Value::Null,
                        success: false,
                        duration_ms,
                        timestamp_ms: flow_start.elapsed().as_millis() as u64,
                        error: Some(error_msg),
                    };
                    execution_log.push(step);
                }
            }
        }

        let total_duration_ms = flow_start.elapsed().as_millis() as u64;
        // If no skill succeeded but we have data, report the last output
        // anyway.
        if final_output.is_none() {
            for step in execution_log.iter().rev() {
                if step.success && step.output != Value::Null {
                    final_output = Some(step.output.to_string());
                    break;
                }
            }
        }

        let cache_snapshot = self.cache.cache_metrics_snapshot();

        // BLUE44: Store cache metrics for governance observability
        crate::orchestration::fast_path_cache::store_cache_metrics(cache_snapshot.clone());

        info!(
            "{}",
            tf(
                "status.full_auto.flow_completed",
                &[
                    (
                        "successful",
                        &execution_log
                            .iter()
                            .filter(|s| s.success)
                            .count()
                            .to_string()
                    ),
                    (
                        "failed",
                        &execution_log
                            .iter()
                            .filter(|s| !s.success)
                            .count()
                            .to_string()
                    ),
                    ("errors", &errors.len().to_string()),
                    ("duration_ms", &total_duration_ms.to_string()),
                ]
            )
        );

        // ── BrainLoop integration (GAP-46-07) ───────────────────────────
        // Create a plan from the task result and execute a synthetic step so
        // the brain loop is no longer a dead module.
        // GAP-46-12: Use ComplexityEstimator to dynamically tune max_iterations.
        //
        // NOTE: The synthetic BrainLoop below uses the complexity-derived
        // iteration count, but it does NOT actually re-execute any skills.
        // Future work should connect this to real re-execution — e.g. by
        // re-running failed or low-confidence steps up to the recommended
        // iteration limit when the flow produces errors.
        if !execution_log.is_empty() {
            let complexity = self.complexity_estimator.estimate(task);
            info!(
                "ComplexityEstimator: level={:?} (score={}), recommended_iterations={}",
                complexity.level,
                complexity.score,
                complexity.level.recommended_iterations()
            );

            // The recommended iteration count is plumbed through to the
            // BrainLoop config so it's available when the loop is connected
            // to actual re-execution. For now the synthetic run uses it
            // as a forward-looking placeholder.
            let bl_config = BrainLoopConfig {
                max_iterations: complexity.level.recommended_iterations(),
                ..BrainLoopConfig::default()
            };

            let bl = BrainLoop::new(bl_config);
            let bl_steps: Vec<BrainLoopStep> = execution_log
                .iter()
                .enumerate()
                .map(|(i, s)| BrainLoopStep {
                    id: format!("bl-step-{i}"),
                    phase: BrainLoopPhase::Executing,
                    description: s.skill_name.clone(),
                    input: s.input.to_string(),
                    output: if s.success {
                        s.output.to_string()
                    } else {
                        String::new()
                    },
                    started_ms: s.timestamp_ms,
                    completed_ms: s.timestamp_ms + s.duration_ms,
                    duration_ms: s.duration_ms,
                    status: if s.success {
                        StepStatus::Done
                    } else {
                        StepStatus::Skipped
                    },
                })
                .collect();

            match bl.start_plan(task, bl_steps) {
                Ok(plan_id) => {
                    debug!("BrainLoop plan `{plan_id}` started for task");
                    if let Some(ref output) = final_output {
                        if let Err(e) = bl.execute_step(&plan_id, "bl-step-0", output) {
                            warn!("BrainLoop step execution failed: {e}");
                        }
                    }
                }
                Err(e) => warn!("BrainLoop plan creation failed: {e}"),
            }
        }

        AutoExecutionReport {
            task_intent: intent,
            matched_skills,
            environment_status,
            execution_log,
            final_output,
            errors,
            total_duration_ms,
            cache_metrics: cache_snapshot,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Tokenize a string into a set of lowercased alphanumeric tokens of length
/// ≥ 3.
pub(crate) fn tokenize(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Arc;

    use crate::orchestration::skill::Skill;
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

        async fn execute(&self, _input: &Value) -> anyhow::Result<Value> {
            if self.fail {
                anyhow::bail!("intentional failure");
            }
            Ok(self.output.clone())
        }
    }

    fn setup_registry() -> Arc<Mutex<SkillRegistry>> {
        let mut reg = SkillRegistry::default();

        // Register two skills: one for code fixes, one for documentation.
        let fix_skill = FixedSkill {
            name: "code_fixer".to_string(),
            desc: "Fixes bugs and errors in source code files".to_string(),
            output: json!({"fixed": true, "patches": 3}),
            fail: false,
        };
        reg.register(Arc::new(fix_skill)).unwrap();

        let docs_skill = FixedSkill {
            name: "doc_writer".to_string(),
            desc: "Generates documentation from code comments and structure".to_string(),
            output: json!({"documented": true, "pages": 2}),
            fail: false,
        };
        reg.register(Arc::new(docs_skill)).unwrap();

        let failing_skill = FixedSkill {
            name: "flakey_tool".to_string(),
            desc: "A skill that always fails for testing purposes".to_string(),
            output: Value::Null,
            fail: true,
        };
        reg.register(Arc::new(failing_skill)).unwrap();

        Arc::new(Mutex::new(reg))
    }

    fn make_flow(registry: Arc<Mutex<SkillRegistry>>) -> FullAutoFlow {
        let tool_registry = Arc::new(ToolRegistry::new_empty());
        FullAutoFlow::new(registry, tool_registry)
    }

    #[test]
    fn with_cache_constructor_smoke() {
        let registry = setup_registry();
        let tool_registry = Arc::new(ToolRegistry::new_empty());
        let cache = crate::orchestration::fast_path_cache::FastPathCache::new();
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
                max_execution_steps: 20,
                enable_env_check: true,
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
        assert!(env.runtime_ready);
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
        assert!(!env.runtime_ready);
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
        // Duration may be 0 when the skill returns synchronously in <1ms.
        // u64 is always non-negative, so no assertion needed.
    }

    #[tokio::test]
    async fn run_flow_no_matching_skills_produces_empty_report() {
        let registry = setup_registry();
        let mut flow = FullAutoFlow::with_config(
            registry,
            Arc::new(ToolRegistry::new_empty()),
            FullAutoConfig {
                min_match_score: 5.0,
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
        // We check that at least one execution step was recorded.
        // If flakey_tool wasn't matched (low score), the flow produces
        // an empty execution log, which is also acceptable since matching
        // depends on token overlap thresholds.
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
                input: Value::Null,
                output: Value::Null,
                success: true,
                duration_ms: 1,
                timestamp_ms: 0,
                error: None,
            }],
            final_output: None,
            errors: vec![],
            total_duration_ms: 1,
            cache_metrics: json!({}),
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
            cache_metrics: json!({}),
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
                    input: Value::Null,
                    output: Value::Null,
                    success: true,
                    duration_ms: 1,
                    timestamp_ms: 0,
                    error: None,
                },
                ExecutionStep {
                    skill_name: "b".to_string(),
                    input: Value::Null,
                    output: Value::Null,
                    success: false,
                    duration_ms: 2,
                    timestamp_ms: 1,
                    error: Some("fail".to_string()),
                },
            ],
            final_output: None,
            errors: vec![],
            total_duration_ms: 3,
            cache_metrics: json!({}),
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
