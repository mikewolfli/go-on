//! Execution orchestration for the full-auto flow.
//!
//! Contains the main [`FullAutoFlow::run`] method and the skill discovery
//! pipeline that matches task intents to registered skills.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use tracing::{debug, info, warn};

use crate::i18n::runtime::tf;
use crate::orchestration::brain_loop::{
    BrainLoop, BrainLoopConfig, BrainLoopPhase, BrainLoopStep, StepStatus,
};
use crate::orchestration::skill::Skill;
use crate::orchestration::tool_recommender::ToolRecommendation;

use super::intent::{tokenize, TaskIntent};
use super::report::{AutoExecutionReport, ExecutionStep, SkillMatch};
use super::FullAutoFlow;

// ---------------------------------------------------------------------------
// Weight constants used for composite skill-matching scores
// ---------------------------------------------------------------------------

/// Weight for name similarity in composite scoring.
const WEIGHT_NAME: f64 = 0.35;

/// Weight for description semantic similarity.
const WEIGHT_DESCRIPTION: f64 = 0.40;

/// Weight for runtime score (historical success rate from registry).
const WEIGHT_RUNTIME: f64 = 0.25;

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

impl FullAutoFlow {
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
        // O(1) lookup set for goal tokens — avoids O(N×M) nested iteration
        // over goal_tokens × desc_tokens for every skill descriptor.
        let goal_token_set: std::collections::HashSet<&str> =
            goal_tokens.iter().map(|s| s.as_str()).collect();

        let registry = self.skill_registry.lock().unwrap_or_else(|poisoned| {
            warn!("skill_registry lock poisoned – recovered data");
            poisoned.into_inner()
        });
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
                    // O(M) instead of O(G×M): count descriptor tokens that
                    // appear in the goal token set (HashSet provides O(1) lookup).
                    let overlap = desc_tokens
                        .iter()
                        .filter(|t| goal_token_set.contains(t.as_str()))
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
        let cached = super::SkillCacheValue {
            skill_names: matches.iter().map(|m| m.name.clone()).collect(),
            scores: matches.iter().map(|m| m.score).collect(),
        };
        self.cache.set_skills(&goal_text, cached);

        matches
    }
}

// ---------------------------------------------------------------------------
// Execute a single skill (free function for Send)
// ---------------------------------------------------------------------------

/// Execute a single skill with a held semaphore permit.
/// This is a free function (not a method on FullAutoFlow) to guarantee
/// that the returned future is `Send`, which is required by `tokio::spawn`.
pub(crate) async fn execute_skill(
    skill: Arc<dyn Skill>,
    input: Value,
    _permit: tokio::sync::OwnedSemaphorePermit,
) -> (anyhow::Result<Value>, Duration) {
    let step_start = Instant::now();
    let result = skill.execute(&input).await;
    let elapsed = step_start.elapsed();
    (result, elapsed)
}

// ---------------------------------------------------------------------------
// Main run method
// ---------------------------------------------------------------------------

impl FullAutoFlow {
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
                // Fallback to universal tools when no skills match
                if self.config.fallback_to_universal_tools {
                    let universal_tools = vec![
                        "read_file",
                        "write_file",
                        "list_directory",
                        "grep",
                        "search_files",
                    ];
                    info!("Falling back to {} universal tools", universal_tools.len());
                    for tool_name in &universal_tools {
                        matched_skills.push(SkillMatch {
                            name: tool_name.to_string(),
                            description: format!("Universal fallback tool: {}", tool_name),
                            score: 0.5,
                            reason: "universal tool fallback (no skill match)".into(),
                        });
                    }
                }
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
            let recommender = self.tool_recommender.lock().unwrap_or_else(|poisoned| {
                warn!("tool_recommender lock poisoned – recovered data");
                poisoned.into_inner()
            });
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

        // Execute skills with bounded parallelism.
        // Skills that need file write locks are still serialized by ToolLockManager.
        let semaphore = Arc::clone(&self.semaphore);

        // Build a list of (skill_match, skill) pairs, filtering out missing skills
        let mut skills_to_run: Vec<(SkillMatch, Arc<dyn Skill>, Value)> = Vec::new();
        for skill_match in &matched_skills {
            if execution_log.len() + skills_to_run.len() >= self.config.max_execution_steps {
                let remaining = self
                    .config
                    .max_execution_steps
                    .saturating_sub(execution_log.len() + skills_to_run.len());
                if skills_to_run.len() >= remaining {
                    break;
                }
            }

            let skill_opt = {
                let registry = self.skill_registry.lock().unwrap_or_else(|poisoned| {
                    warn!("skill_registry lock poisoned – recovered data");
                    poisoned.into_inner()
                });
                registry.get(&skill_match.name)
            };

            match skill_opt {
                Some(skill) => {
                    let input = json!({
                        "task": task,
                        "goals": intent.goals,
                        "constraints": intent.constraints,
                        "skill_name": skill_match.name,
                    });
                    skills_to_run.push((skill_match.clone(), skill, input));
                }
                None => {
                    let msg = tf(
                        "error.full_auto.skill_not_found",
                        &[("skill_name", &skill_match.name)],
                    );
                    warn!("{}", msg);
                    errors.push(msg);
                }
            }
        }

        // Execute skills in parallel with bounded concurrency via Semaphore.
        // Permits are acquired inside each spawned task so the loop can
        // dispatch all tasks immediately — they'll compete for permits
        // asynchronously rather than serializing on sequential acquisition.
        let mut handles = Vec::with_capacity(skills_to_run.len());
        let mut skill_names: Vec<String> = Vec::with_capacity(skills_to_run.len());

        for (skill_match, skill, input) in skills_to_run {
            let skill_name = skill_match.name.clone(); // clone before move
            skill_names.push(skill_name.clone());
            let sem_clone = semaphore.clone();
            handles.push(tokio::spawn(async move {
                let _permit = match sem_clone.acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => {
                        warn!("Semaphore closed — skipping skill '{}'", skill_name);
                        return (Err(anyhow::anyhow!("semaphore closed")), Duration::ZERO);
                    }
                };
                execute_skill(skill, input, _permit).await
            }));
        }

        // Collect results via join_all on the JoinHandles
        for (idx, handle) in handles.into_iter().enumerate() {
            let skill_name = skill_names
                .get(idx)
                .map(|s| s.as_str())
                .unwrap_or("unknown");
            match handle.await {
                Ok((Ok(output), elapsed)) => {
                    let duration_ms = elapsed.as_millis() as u64;

                    let step = ExecutionStep {
                        skill_name: "parallel-execution".to_string(),
                        input: json!(null),
                        output: output.clone(),
                        success: true,
                        duration_ms,
                        timestamp_ms: flow_start.elapsed().as_millis() as u64,
                        error: None,
                    };
                    execution_log.push(step);

                    let output_text = output.to_string();
                    if output_text.len() < 1_000_000 {
                        final_output = Some(output_text);
                    } else {
                        final_output = Some(tf(
                            "status.full_auto.output_truncated",
                            &[("bytes", &output_text.len().to_string())],
                        ));
                    }

                    self.model_selector.record_result(skill_name, true);
                    self.record_match_outcome(true, false, false);
                }
                Ok((Err(e), elapsed)) => {
                    let duration_ms = elapsed.as_millis() as u64;
                    let err_str = e.to_string();
                    let msg = tf("error.full_auto.skill_execution", &[("error", &err_str)]);
                    warn!("{}", msg);
                    errors.push(msg);

                    let step = ExecutionStep {
                        skill_name: "parallel-execution".to_string(),
                        input: json!(null),
                        output: json!(null),
                        success: false,
                        duration_ms,
                        timestamp_ms: flow_start.elapsed().as_millis() as u64,
                        error: Some(err_str),
                    };
                    execution_log.push(step);

                    self.model_selector.record_result(skill_name, false);
                    self.record_match_outcome(false, true, false);
                }
                Err(join_err) => {
                    warn!("Parallel skill execution panicked: {}", join_err);
                    if let Some(name) = skill_names.get(idx) {
                        self.model_selector.record_result(name, false);
                    }
                    errors.push(format!("Skill panicked: {}", join_err));
                }
            }

            if execution_log.len() >= self.config.max_execution_steps {
                let msg = tf(
                    "error.full_auto.max_steps_reached",
                    &[("max_steps", &self.config.max_execution_steps.to_string())],
                );
                warn!("{}", msg);
                errors.push(msg);
                break;
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

        // ── BrainLoop re-execution (GAP-46-07) ─────────────────────────
        // Re-run failed or skipped steps through the BrainLoop so that
        // the brain loop receives real execution data rather than being a
        // dead module. The complexity estimator dynamically tunes the
        // iteration budget for each task.
        if !execution_log.is_empty() {
            let complexity = self.complexity_estimator.estimate(task);
            info!(
                "ComplexityEstimator: level={:?} (score={}), recommended_iterations={}",
                complexity.level,
                complexity.score,
                complexity.level.recommended_iterations()
            );

            let bl_config = BrainLoopConfig {
                max_iterations: complexity.level.recommended_iterations(),
                world_model_integration: true,
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
                    context: None,
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

                    // Execute every step through the BrainLoop, not just the first one.
                    // Failed/skipped steps receive empty outputs; successful steps
                    // propagate their prior output through the loop.
                    for i in 0..execution_log.len() {
                        let step_id = format!("bl-step-{i}");
                        let step_output = execution_log
                            .get(i)
                            .filter(|s| s.success)
                            .map(|s| s.output.to_string())
                            .unwrap_or_default();

                        if let Err(e) = bl.execute_step(&plan_id, &step_id, &step_output).await {
                            warn!("BrainLoop step `{step_id}` execution failed: {e}");
                            errors.push(format!("BrainLoop re-execution failed for step {i}: {e}"));
                        }
                    }

                    // Mark the plan as completed so the BrainLoop has a clean
                    // terminal state for profiling / persistence.
                    if let Err(e) = bl.complete_plan(&plan_id).await {
                        // Non-fatal — the steps were already recorded.
                        warn!("BrainLoop complete_plan failed: {e}");
                    }

                    let profile = bl.profile().await;
                    debug!(
                        "BrainLoop plan `{plan_id}` completed: {} cycles, {} plans",
                        profile.total_cycles, profile.total_plans
                    );
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
