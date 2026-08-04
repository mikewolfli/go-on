//! Planning, reflection, replanning, and lifecycle methods for [`BrainLoop`](super::BrainLoop).
//!
//! Actively wired in production: HarnessBus, autonomy_loop_adapter, chat_phases.rs.
//! The cognitive loop in chat_phases.rs is an additional path; the brain loop
//! remains the primary iterative orchestration driver.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use serde_json::Value;

use crate::agent::AgentRegistry;
use crate::intelligence::metacognitive::CorrectiveStatus;
use crate::intelligence::world_model::{EntityType, WorldModel, WorldModelConfig};
use crate::orchestration::brain_loop::grill::enhance_reflection_with_grill;
use crate::orchestration::brain_loop::planner_bridge::{auto_decompose_task, PlanningStrategy};
use crate::orchestration::core_dag::TaskContext;

use super::{
    now_epoch_ms, tf, BrainLoop, BrainLoopInner, BrainLoopPhase, BrainLoopPlan, BrainLoopProfile,
    BrainLoopReflection, BrainLoopStep, DeepReasoningEngine, PlannerHint, StepStatus,
};

// ---------------------------------------------------------------------------
// Plan lifecycle (async lock acquisition)
// ---------------------------------------------------------------------------

impl BrainLoop {
    /// Start a new plan with the given `goal` and initial `steps`.
    ///
    /// Returns the assigned plan id on success.
    pub async fn start_plan(
        &self,
        goal: &str,
        steps: Vec<BrainLoopStep>,
    ) -> anyhow::Result<String> {
        let id_num = self.next_plan_id.fetch_add(1, Ordering::AcqRel);
        let id = format!("plan-{id_num}");

        let now = now_epoch_ms();
        let mut inner = self.inner.write().await;
        let max_iterations = inner.config.max_iterations;
        let plan = BrainLoopPlan {
            id: id.clone(),
            goal: goal.to_string(),
            steps,
            max_iterations,
            current_iteration: 0,
            created_ms: now,
            phase: BrainLoopPhase::Planning,
            fail_reason: String::new(),
            reasoning: None,
            world_model_data: None,
            parallel_groups: vec![],
            dag_metrics: None,
        };
        inner.plans.insert(id.clone(), plan);
        inner.total_plans_started += 1;
        Ok(id)
    }

    /// Get a clone of a plan by its id.
    pub async fn get_plan(&self, id: &str) -> anyhow::Result<BrainLoopPlan> {
        self.inner
            .read()
            .await
            .plans
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("plan `{id}` not found"))
    }

    /// Attach a metacognitive controller for self-correction feedback.
    ///
    /// When set, `run_async` will query the controller for historical
    /// corrective actions and inject preventive measures as constraints
    /// into the planning loop.
    pub async fn set_metacognitive(
        &self,
        mc: crate::intelligence::metacognitive::MetacognitiveController,
    ) {
        let mut inner = self.inner.write().await;
        inner.metacognitive = Some(mc);
    }

    /// Set the agent registry for LLM-backed deep reasoning (B51-08).
    pub async fn set_agent_registry(&self, registry: std::sync::Arc<AgentRegistry>) {
        let mut inner = self.inner.write().await;
        inner.agent_registry = Some(registry);
    }

    /// Return accumulated planner hints (e.g. from metacognitive feedback).
    pub async fn get_planner_hints(&self) -> Vec<PlannerHint> {
        self.inner.read().await.planner_hints.clone()
    }

    /// Return a list of all known plan ids.
    pub async fn list_plans(&self) -> Vec<String> {
        self.inner.read().await.plans.keys().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// Reflection (async)
// ---------------------------------------------------------------------------

impl BrainLoop {
    /// Record a reflection on a completed step.
    ///
    /// Moves the plan into the `Reflecting` phase.
    pub async fn reflect(
        &self,
        plan_id: &str,
        step_id: &str,
        observations: Vec<String>,
        issues: Vec<String>,
        improvements: Vec<String>,
    ) -> anyhow::Result<BrainLoopReflection> {
        let now = now_epoch_ms();
        let mut inner = self.inner.write().await;

        // Compute reflection inside a scope so the mutable plan borrow is
        // dropped before we push to `inner.reflections`.
        let reflection = {
            let plan = inner.plans.get_mut(plan_id).ok_or_else(|| {
                anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)]))
            })?;

            if plan.phase.is_terminal() {
                anyhow::bail!("{}", tf("error.plan_already_terminal", &[("id", plan_id)]));
            }

            let step_idx = plan
                .steps
                .iter()
                .position(|s| s.id == step_id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "{}",
                        tf(
                            "error.step_not_found",
                            &[("id", step_id), ("plan_id", plan_id)]
                        )
                    )
                })?;

            let started = plan.steps[step_idx].started_ms;
            // Capture context from the step before marking it done.
            let step_context = plan.steps[step_idx].context.clone();
            let accumulated_reasoning = step_context
                .as_ref()
                .map(|c| c.reasoning_trace.clone())
                .unwrap_or_default();

            plan.steps[step_idx].status = StepStatus::Done;
            plan.steps[step_idx].completed_ms = now;
            plan.steps[step_idx].duration_ms = now.saturating_sub(started);
            plan.phase = BrainLoopPhase::Reflecting;

            let confidence = if issues.is_empty() {
                1.0
            } else {
                // Each issue reduces confidence by 0.2, with a max penalty cap of 5 issues
                let penalty = (issues.len() as f64 * 0.2).min(1.0);
                (1.0 - penalty).max(0.1)
            };

            BrainLoopReflection {
                step_id: step_id.to_string(),
                observations,
                issues,
                improvements,
                confidence,
                reflection_ms: now,
                context_snapshot: step_context,
                reasoning_chain: accumulated_reasoning,
            }
        };

        const MAX_REFLECTIONS: usize = 1000;
        if inner.reflections.len() >= MAX_REFLECTIONS {
            inner.reflections.remove(0);
        }
        inner.reflections.push(reflection.clone());

        Ok(reflection)
    }

    /// Record a reflection, enhanced with GRILL-style interrogation if enabled.
    ///
    /// Wraps [`reflect`](Self::reflect) with optional GRILL probing questions
    /// based on the configured `grill_mode`. The grill mode is read from the
    /// runtime config at call time.
    pub async fn reflect_with_grill(
        &self,
        plan_id: &str,
        step_id: &str,
        observations: Vec<String>,
        issues: Vec<String>,
        improvements: Vec<String>,
    ) -> anyhow::Result<BrainLoopReflection> {
        // Read the GRILL mode under a read lock.
        let (grill_mode, step_description) = {
            let inner = self.inner.read().await;
            let mode = inner.config.grill_mode;
            let desc = inner
                .plans
                .get(plan_id)
                .and_then(|p| {
                    p.steps
                        .iter()
                        .find(|s| s.id == step_id)
                        .map(|s| s.description.clone())
                })
                .unwrap_or_default();
            (mode, desc)
        };

        let mut reflection = self
            .reflect(plan_id, step_id, observations, issues, improvements)
            .await?;

        enhance_reflection_with_grill(&mut reflection, grill_mode, &step_description);

        Ok(reflection)
    }

    /// Run the BrainLoop with automatic task decomposition.
    ///
    /// If `BrainLoopConfig.planning_strategy` is `AutoDecompose`, the task
    /// string is decomposed into steps via `planner_executor::Planner`.
    /// Otherwise behaves identically to [`run_async`](Self::run_async).
    pub async fn run_async_with_strategy(
        &self,
        task: &str,
        steps: Vec<BrainLoopStep>,
    ) -> anyhow::Result<BrainLoopProfile> {
        let strategy = {
            let inner = self.inner.read().await;
            inner.config.planning_strategy
        };

        let final_steps = match strategy {
            PlanningStrategy::ExplicitSteps => steps,
            PlanningStrategy::AutoDecompose => {
                if steps.is_empty() {
                    auto_decompose_task(task).await
                } else {
                    // Explicit steps override auto-decompose.
                    steps
                }
            }
        };

        self.run_async(task, final_steps).await
    }
}

// ---------------------------------------------------------------------------
// Replanning (async)
// ---------------------------------------------------------------------------

impl BrainLoop {
    /// Replace the remaining pending steps with a new set of steps.
    ///
    /// Existing completed / in-progress steps are preserved.
    /// The plan phase is set to `Replanning`.
    ///
    /// When TaskContexts exist on completed steps, they are merged and
    /// assigned to new steps for reasoning chain continuity.
    pub async fn replan(&self, plan_id: &str, new_steps: Vec<BrainLoopStep>) -> anyhow::Result<()> {
        let mut inner = self.inner.write().await;

        let plan = inner
            .plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)])))?;

        if plan.phase.is_terminal() {
            anyhow::bail!("{}", tf("error.plan_already_terminal", &[("id", plan_id)]));
        }

        // Collect parent TaskContexts from completed steps for merging.
        let parent_contexts: Vec<TaskContext> = plan
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::Done)
            .filter_map(|s| s.context.clone())
            .collect();

        // Keep only steps that are not pending (they are either done or in progress).
        plan.steps.retain(|s| s.status != StepStatus::Pending);

        // Merge parent contexts into a single merged context for new steps.
        let merged_context = if !parent_contexts.is_empty() {
            Some(TaskContext::merge(&parent_contexts))
        } else {
            None
        };

        // Append the new steps, each receiving the merged context.
        // Assign the merged context (if any) to all new steps.
        let merged = merged_context;
        for mut step in new_steps {
            if let Some(ref ctx) = merged {
                step.context = Some(ctx.clone());
            }
            plan.steps.push(step);
        }
        plan.phase = BrainLoopPhase::Replanning;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Terminal transitions (async)
// ---------------------------------------------------------------------------

impl BrainLoop {
    /// Mark a plan as completed.
    pub async fn complete_plan(&self, plan_id: &str) -> anyhow::Result<()> {
        let mut inner = self.inner.write().await;
        let plan = inner
            .plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)])))?;

        if plan.phase.is_terminal() {
            anyhow::bail!("{}", tf("error.plan_already_terminal", &[("id", plan_id)]));
        }
        plan.phase = BrainLoopPhase::Completed;
        inner.completed_plans_total += 1;

        Self::evict_oldest_terminal_plan(&mut inner.plans);
        Ok(())
    }

    /// Mark a plan as failed with a reason.
    pub async fn fail_plan(&self, plan_id: &str, reason: &str) -> anyhow::Result<()> {
        let mut inner = self.inner.write().await;
        let plan = inner
            .plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)])))?;

        if plan.phase.is_terminal() {
            anyhow::bail!("{}", tf("error.plan_already_terminal", &[("id", plan_id)]));
        }
        plan.phase = BrainLoopPhase::Failed;
        plan.fail_reason = reason.to_string();
        inner.failed_plans_total += 1;

        Self::evict_oldest_terminal_plan(&mut inner.plans);
        Ok(())
    }

    /// Cancel a plan.
    pub async fn cancel_plan(&self, plan_id: &str) -> anyhow::Result<()> {
        let mut inner = self.inner.write().await;
        let plan = inner
            .plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)])))?;

        if plan.phase.is_terminal() {
            anyhow::bail!("{}", tf("error.plan_already_terminal", &[("id", plan_id)]));
        }
        plan.phase = BrainLoopPhase::Cancelled;
        inner.cancelled_plans_total += 1;

        Self::evict_oldest_terminal_plan(&mut inner.plans);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Queries (async)
// ---------------------------------------------------------------------------

impl BrainLoop {
    /// The current phase of a plan.
    pub async fn current_phase(&self, plan_id: &str) -> anyhow::Result<BrainLoopPhase> {
        self.inner
            .read()
            .await
            .plans
            .get(plan_id)
            .map(|p| p.phase)
            .ok_or_else(|| anyhow::anyhow!("plan `{plan_id}` not found"))
    }

    /// Return a snapshot of runtime metrics.
    pub async fn profile(&self) -> BrainLoopProfile {
        let inner = self.inner.read().await;
        let total_plans = inner.total_plans_started;
        let active_plans = inner
            .plans
            .values()
            .filter(|p| !p.phase.is_terminal())
            .count() as u64;
        let avg = if total_plans > 0 {
            inner.total_cycles as f64 / total_plans as f64
        } else {
            0.0
        };

        // Compute convergence info and avg step score from reflections.
        let total_steps: u64 = inner.plans.values().map(|p| p.steps.len() as u64).sum();
        let avg_step_score = if inner.reflections.is_empty() {
            0.0
        } else {
            inner.reflections.iter().map(|r| r.confidence).sum::<f64>()
                / inner.reflections.len() as f64
        };

        let convergence_info = if active_plans == 0 && total_plans > 0 {
            let converged = self.check_convergence(&inner);
            if converged {
                format!("converged after {} plans", total_plans)
            } else {
                "not converged".to_string()
            }
        } else {
            "in progress".to_string()
        };

        BrainLoopProfile {
            total_plans,
            active_plans,
            completed_plans: inner.completed_plans_total,
            failed_plans: inner.failed_plans_total,
            total_cycles: inner.total_cycles,
            avg_cycles_per_plan: avg,
            convergence_info,
            avg_step_score,
            total_steps,
        }
    }

    /// Check whether the loop has converged based on recent reflection confidence scores.
    ///
    /// Convergence is detected when:
    /// - At least two reflections exist, AND
    /// - The latest confidence score is >= `min_score`, OR
    /// - The score delta between the last two reflections is <= `convergence_threshold`.
    fn check_convergence(&self, inner: &BrainLoopInner) -> bool {
        let config = &inner.config;
        let reflections = &inner.reflections;

        if reflections.len() < 2 {
            return false;
        }

        let latest = &reflections[reflections.len() - 1];
        let previous = &reflections[reflections.len() - 2];

        if latest.confidence >= config.min_score {
            return true;
        }

        let delta = (latest.confidence - previous.confidence).abs();
        delta <= config.convergence_threshold && latest.confidence > 0.3
    }
}

// ---------------------------------------------------------------------------
// Persistence (async)
// ---------------------------------------------------------------------------

impl BrainLoop {
    /// Serialize and write a plan to a JSON file in the configured `plans_directory`.
    ///
    /// Returns `Ok(())` if the plan exists and serialization succeeds, or if no
    /// directory is configured (silent no-op).
    pub async fn persist_plan(&self, plan_id: &str) -> anyhow::Result<()> {
        let (plan, dir) = {
            let inner = self.inner.read().await;
            let plan = inner
                .plans
                .get(plan_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("plan `{plan_id}` not found"))?;
            let dir = inner.config.plans_directory.clone();
            (plan, dir)
        };

        let dir = match dir {
            Some(d) => d,
            None => return Ok(()), // no directory configured, skip
        };

        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| anyhow::anyhow!("failed to create plans directory {:?}: {e}", dir))?;

        let path = dir.join(format!("{plan_id}.json"));
        let json = serde_json::to_string_pretty(&plan)
            .map_err(|e| anyhow::anyhow!("failed to serialize plan `{plan_id}`: {e}"))?;
        tokio::fs::write(&path, &json)
            .await
            .map_err(|e| anyhow::anyhow!("failed to write plan `{plan_id}` to {:?}: {e}", path))?;
        tracing::debug!("persisted plan `{plan_id}` to {:?}", path);
        Ok(())
    }

    /// Load a plan from a JSON file in the configured `plans_directory`.
    ///
    /// Returns `None` if no directory is configured or the file does not exist.
    pub async fn load_plan(&self, plan_id: &str) -> Option<BrainLoopPlan> {
        let dir = {
            let inner = self.inner.read().await;
            inner.config.plans_directory.clone()
        };

        let dir = dir?;
        let path = dir.join(format!("{plan_id}.json"));
        if !path.exists() {
            return None;
        }
        match tokio::fs::read_to_string(&path).await {
            Ok(content) => match serde_json::from_str::<BrainLoopPlan>(&content) {
                Ok(plan) => Some(plan),
                Err(e) => {
                    tracing::warn!(
                        "failed to deserialize plan `{plan_id}` from {:?}: {e}",
                        path
                    );
                    None
                }
            },
            Err(e) => {
                tracing::warn!("failed to read plan `{plan_id}` from {:?}: {e}", path);
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// World model integration (GAP-B50-06, B51-08)
// ---------------------------------------------------------------------------

impl BrainLoop {
    /// Query the world model for environment entities relevant to the plan.
    ///
    /// When `world_model_integration` is enabled in the config, this queries
    /// the [`WorldModel`] for real entity data and populates the plan's
    /// `world_model_data` field.
    pub async fn query_world_model(&self, plan_id: &str) {
        let world_model_enabled = {
            let inner = self.inner.read().await;
            inner.config.world_model_integration
        };

        if !world_model_enabled {
            return;
        }

        // ── Query real world model data (B51-08) ───────────────────────
        let wm = WorldModel::new(WorldModelConfig::default());

        // Register the current plan goal as a tracked entity and summarize the
        // just-registered entity directly (the fresh model contains exactly the
        // entity registered below; the former `query_entities` round-trip was
        // removed with the world-model query API).
        let goal = {
            let inner = self.inner.read().await;
            inner
                .plans
                .get(plan_id)
                .map(|p| p.goal.clone())
                .unwrap_or_default()
        };

        let entity_name = format!("brain-loop-plan-{plan_id}");
        let entity_summary: Vec<Value> = match wm.register_entity(&entity_name, EntityType::System)
        {
            Ok(entity_id) => vec![serde_json::json!({
                "id": entity_id,
                "name": entity_name,
                "entity_type": "System",
                "confidence": 1.0,
                "properties": {},
            })],
            Err(e) => {
                tracing::warn!("query_world_model: failed to register plan entity: {e}");
                Vec::new()
            }
        };

        let mut data = HashMap::new();
        data.insert(
            "environment".to_string(),
            Value::String("world-model-v1".to_string()),
        );
        data.insert("goal".to_string(), Value::String(goal));
        data.insert("entities".to_string(), Value::Array(entity_summary));
        data.insert(
            "query_timestamp_ms".to_string(),
            Value::Number(serde_json::Number::from(now_epoch_ms())),
        );

        let mut inner = self.inner.write().await;
        if let Some(plan) = inner.plans.get_mut(plan_id) {
            plan.world_model_data = Some(data);
        }
    }
}

// ---------------------------------------------------------------------------
// Metacognitive feedback integration
// ---------------------------------------------------------------------------

impl BrainLoop {
    /// Query the metacognitive controller for historical corrective actions
    /// matching the given task type, and inject preventive measures as
    /// [`PlannerHint`]s into [`BrainLoopInner`].
    ///
    /// Detects repeated error patterns (3+ occurrences of the same error
    /// type) and generates warning hints.
    async fn integrate_metacognitive_feedback(&self, task_type: &str) {
        // Snapshot the metacognitive controller (if any) outside the write
        // lock to avoid a lock ordering inversion (sync Mutex inside async
        // RwLock).
        let mc = {
            let inner = self.inner.read().await;
            inner.metacognitive.clone()
        };

        let Some(mc) = mc else { return };

        // Query historical actions matching the task type.
        let historical = mc.get_historical_actions(task_type);
        if historical.is_empty() {
            return;
        }

        let mut hints: Vec<PlannerHint> = Vec::new();

        // Collect preventive measures from completed corrective results.
        for action in &historical {
            if let Some(ref result) = action.result {
                if !result.preventive_measures.is_empty() {
                    let hint = PlannerHint {
                        hint_type: if result.success {
                            "Info".to_string()
                        } else {
                            "Warning".to_string()
                        },
                        message: format!(
                            "Corrective action `{}`: {}. Root cause: {}",
                            action.action_type, action.description, result.root_cause,
                        ),
                        source: "metacognitive".to_string(),
                        preventive_measures: result.preventive_measures.clone(),
                    };
                    hints.push(hint);
                }
            }
        }

        // Detect repeated error patterns from historical failed actions.
        let mut error_type_counts: HashMap<String, u32> = HashMap::new();
        for action in &historical {
            if action.status == CorrectiveStatus::Failed {
                let et = action.action_type.clone();
                *error_type_counts.entry(et).or_insert(0) += 1;
            }
        }
        for (et, count) in &error_type_counts {
            if *count >= 3 {
                hints.push(PlannerHint {
                    hint_type: "Warning".to_string(),
                    message: format!(
                        "Action type `{et}` failed {count} times historically; consider a different strategy"
                    ),
                    source: "metacognitive".to_string(),
                    preventive_measures: vec![],
                });
            }
        }

        // Write hints into inner state.
        if !hints.is_empty() {
            let mut inner = self.inner.write().await;
            inner.planner_hints.extend(hints);
        }
    }
}

// ---------------------------------------------------------------------------
// High-level orchestration (async)
// ---------------------------------------------------------------------------

impl BrainLoop {
    /// Run the full Plan → Execute → Reflect → Replan cycle asynchronously.
    ///
    /// Starts a plan with the given `task` and `steps`, then iterates
    /// through pending steps — executing, reflecting, and optionally
    /// replanning — until the plan reaches a terminal phase.
    /// Returns a [`BrainLoopProfile`] snapshot at the end.
    ///
    /// # Serial step execution
    ///
    /// Steps in the `pending` list are executed **sequentially** in a
    /// `for` loop (line ~832).  Although `BrainLoopPlan.parallel_groups`
    /// exists (populated from `ExecutionPlan` for complex tasks), this
    /// method does **not** use it for concurrent fan-out.  The
    /// `parallel_groups` are currently consumed downstream by
    /// `ToolPipeline` (see [`tool::pipeline::ToolPipeline`]).
    ///
    /// Adding cross-group parallelism here would require:
    /// 1. Dependency graph analysis to ensure groups are independent.
    /// 2. Coordinated error handling across concurrent step failures.
    /// 3. Ordered reflection when some steps complete before others.
    ///
    /// This is a future enhancement opportunity — the data (step IDs
    /// partitioned by group) is already available on the plan.
    pub async fn run_async(
        &self,
        task: &str,
        steps: Vec<BrainLoopStep>,
    ) -> anyhow::Result<BrainLoopProfile> {
        let plan_id = self.start_plan(task, steps).await?;
        let task_type = task.to_string();

        // ── Check deep-reasoning configuration ────────────────────────
        let (enable_deep, engine, world_model_int) = {
            let inner = self.inner.read().await;
            let mut engine = DeepReasoningEngine::new(&inner.config);
            if let Some(ref registry) = inner.agent_registry {
                engine = engine.with_agent_registry(std::sync::Arc::clone(registry));
            }
            (
                inner.config.enable_deep_reasoning,
                engine,
                inner.config.world_model_integration,
            )
        };

        // ── Deep-reasoning planning pass ──────────────────────────────
        if enable_deep {
            let plan = self.get_plan(&plan_id).await?;
            let context = TaskContext {
                id: plan_id.clone(),
                reasoning_trace: vec!["Initial planning via BrainLoop run_async".to_string()],
                intermediate_findings: HashMap::new(),
                confidence: 0.8,
                open_questions: vec![],
                assumptions: vec![],
                parent_context_id: None,
            };
            let enriched = engine.plan_with_reasoning(&context, &plan).await;
            // Write back reasoning and world model data to the plan.
            {
                let mut inner = self.inner.write().await;
                if let Some(p) = inner.plans.get_mut(&plan_id) {
                    p.reasoning = enriched.reasoning;
                    p.world_model_data = enriched.world_model_data;
                }
            }
        }

        // ── World-model context query (runs regardless of deep reasoning) ──
        if world_model_int {
            self.query_world_model(&plan_id).await;
        }

        // ── Main Plan → Execute → Reflect → Replan loop ──────────────
        loop {
            // Collect pending step ids under a read lock.
            let pending: Vec<String> = {
                let inner = self.inner.read().await;
                inner
                    .plans
                    .get(&plan_id)
                    .map(|p| {
                        p.steps
                            .iter()
                            .filter(|s| s.status == StepStatus::Pending)
                            .map(|s| s.id.clone())
                            .collect()
                    })
                    .unwrap_or_default()
            };

            if pending.is_empty() {
                // ── Validate plan quality (deep mode) ─────────────────
                if enable_deep {
                    let plan = self.get_plan(&plan_id).await?;
                    let quality = engine.quality_validate(&plan).await;
                    tracing::debug!(
                        "BrainLoop: deep quality validation score = {:.2} for plan `{plan_id}`",
                        quality
                    );
                }

                let phase = self.current_phase(&plan_id).await?;
                if !phase.is_terminal() {
                    self.complete_plan(&plan_id).await?;
                }

                // ── Metacognitive feedback integration ───────────────
                self.integrate_metacognitive_feedback(&task_type).await;

                return Ok(self.profile().await);
            }

            // Execute and reflect on each pending step.
            for step_id in &pending {
                if let Err(e) = self.execute_step(&plan_id, step_id, "").await {
                    // ── Track error for repeated-error detection ────────
                    let err_msg = e.to_string();
                    let error_type = super::extract_error_type(&err_msg);
                    {
                        let mut inner = self.inner.write().await;
                        *inner.error_counts.entry(error_type.clone()).or_insert(0) += 1;
                        let count = inner.error_counts[&error_type];
                        if count >= 3 && count % 3 == 0 {
                            let hint = PlannerHint {
                                hint_type: "Warning".to_string(),
                                message: format!(
                                    "Error type `{error_type}` occurred {count} times; consider a different approach"
                                ),
                                source: "metacognitive".to_string(),
                                preventive_measures: vec![],
                            };
                            inner.planner_hints.push(hint);
                        }
                    }

                    tracing::warn!(
                        "BrainLoop: step `{step_id}` execution failed: {e} — failing plan"
                    );
                    self.fail_plan(&plan_id, &err_msg).await?;
                    // Still integrate metacognitive feedback on failure.
                    self.integrate_metacognitive_feedback(&task_type).await;
                    return Ok(self.profile().await);
                }

                if enable_deep {
                    // Use deep-reasoning reflection.
                    let plan = self.get_plan(&plan_id).await?;
                    let history = {
                        let inner = self.inner.read().await;
                        inner.reflections.clone()
                    };
                    let deep_reflection = engine
                        .reflect_with_reasoning("", &history, &plan, step_id)
                        .await;
                    if let Err(e) = self
                        .reflect_with_grill(
                            &plan_id,
                            step_id,
                            deep_reflection.observations,
                            deep_reflection.issues,
                            deep_reflection.improvements,
                        )
                        .await
                    {
                        tracing::warn!("BrainLoop: deep reflection for `{step_id}` failed: {e}");
                    }
                } else {
                    // Standard reflection (with optional GRILL enhancement).
                    if let Err(e) = self
                        .reflect_with_grill(&plan_id, step_id, vec![], vec![], vec![])
                        .await
                    {
                        tracing::warn!("BrainLoop: reflection for `{step_id}` failed: {e}");
                    }
                }
            }

            // Auto-replan if configured and within iteration limits.
            let should_continue = {
                let inner = self.inner.read().await;
                let config = &inner.config;
                config.auto_replan
                    && inner
                        .plans
                        .get(&plan_id)
                        .map(|p| !p.phase.is_terminal() && p.current_iteration < p.max_iterations)
                        .unwrap_or(false)
            };

            if should_continue {
                if enable_deep {
                    // Use deep-reasoning replanning based on reflection content.
                    let reflections = {
                        let inner = self.inner.read().await;
                        inner.reflections.clone()
                    };
                    if let Some(latest_reflection) = reflections.last() {
                        let plan = self.get_plan(&plan_id).await?;
                        let new_steps =
                            engine.replan_with_reasoning(latest_reflection, &plan).await;
                        if !new_steps.is_empty() {
                            // Bump iteration counter BEFORE continue to prevent
                            // infinite re-planning when current_iteration never
                            // advances (the deep-reasoning branch skips the normal
                            // execute_step path that normally increments it).
                            {
                                let mut inner = self.inner.write().await;
                                if let Some(p) = inner.plans.get_mut(&plan_id) {
                                    p.current_iteration = p.current_iteration.saturating_add(1);
                                }
                            }
                            let _ = self.replan(&plan_id, new_steps).await;
                            continue;
                        }
                    }
                }

                // Fallback: complete the plan to avoid an infinite loop.
                let phase = self.current_phase(&plan_id).await?;
                if !phase.is_terminal() {
                    self.complete_plan(&plan_id).await?;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

impl BrainLoop {
    // Evict the oldest terminal plan when the cap is exceeded.
    pub(crate) fn evict_oldest_terminal_plan(
        plans: &mut std::collections::HashMap<String, BrainLoopPlan>,
    ) {
        const MAX_TERMINAL_PLANS: usize = 200;
        let terminal_count = plans.values().filter(|p| p.phase.is_terminal()).count();
        if terminal_count > MAX_TERMINAL_PLANS {
            if let Some(oldest_id) = plans
                .iter()
                .filter(|(_, p)| p.phase.is_terminal())
                .min_by_key(|(_, p)| p.created_ms)
                .map(|(id, _)| id.clone())
            {
                plans.remove(&oldest_id);
            }
        }
    }
}
