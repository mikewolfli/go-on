//! Reflection and learning for the brain loop.
//!
//! Provides the [`DeepReasoningEngine`] for LLM-level reasoning augmentation,
//! and report types for post-loop analysis.
//!
//! ⚠️ **DEPRECATED** (non-test): Use cognitive loop in chat_phases.rs instead.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agent::{Agent, AgentRegistry, Message, StreamingSender};
use crate::orchestration::core_dag::TaskContext;

use super::{
    now_epoch_ms, BrainLoopConfig, BrainLoopPhase, BrainLoopPlan, BrainLoopReflection,
    BrainLoopStep, StepStatus,
};

// ---------------------------------------------------------------------------
// DeepReasoningEngine (GAP-B50-06)
// ---------------------------------------------------------------------------

/// Engine that provides LLM-level reasoning augmentation for the brain loop.
///
/// When `enable_deep_reasoning` is disabled, all methods act as no-ops.
/// When enabled, the engine enriches plans with reasoning chains, produces
/// richer reflections, adjusts plans based on reflection content, and
/// validates outputs using a MultiModelVoter-style consensus approach.
#[derive(Clone)]
pub struct DeepReasoningEngine {
    /// Maximum tokens for a single reasoning chain.
    pub max_reasoning_tokens: usize,
    /// Optional model name override for deep-reasoning calls.
    #[allow(dead_code)]
    pub model: Option<String>,
    /// Agent registry for LLM calls (B51-08).
    /// When `Some`, `plan_with_reasoning` and `reflect_with_reasoning` call
    /// the configured LLM agent for real reasoning chains instead of stubs.
    pub agent_registry: Option<Arc<AgentRegistry>>,
}

impl DeepReasoningEngine {
    /// Create a new engine from configuration.
    pub fn new(config: &BrainLoopConfig) -> Self {
        Self {
            max_reasoning_tokens: config.max_deep_reasoning_tokens,
            model: config.deep_reasoning_model.clone(),
            agent_registry: None,
        }
    }

    /// Set the agent registry for LLM-backed reasoning.
    pub fn with_agent_registry(mut self, registry: Arc<AgentRegistry>) -> Self {
        self.agent_registry = Some(registry);
        self
    }

    /// Produce a structured plan enhanced with LLM-level reasoning.
    ///
    /// Takes a [`TaskContext`] (chain-of-thought state from the DAG executor)
    /// and the current plan, and returns a plan with the `reasoning` field
    /// populated with the analysis chain.
    ///
    /// When deep reasoning is disabled (`max_reasoning_tokens == 0`),
    /// returns the plan unchanged.
    ///
    /// When an `agent_registry` is available (B51-08), calls the configured
    /// LLM agent to generate a real reasoning chain. Otherwise falls back to
    /// a structured summary.
    pub async fn plan_with_reasoning(
        &self,
        context: &TaskContext,
        plan: &BrainLoopPlan,
    ) -> BrainLoopPlan {
        if self.max_reasoning_tokens == 0 {
            return plan.clone();
        }

        let mut enriched = plan.clone();

        // ── Attempt LLM-backed reasoning if agent registry is available ──
        if let Some(ref registry) = self.agent_registry {
            if let Some(agent) = registry.get("primary") {
                let prompt = format!(
                    "You are a deep reasoning engine analyzing a plan.\n\
                     Task \"{}\" with {} steps ({} pending).\n\
                     Context: confidence={:.2}, reasoning_trace={:?}, \
                     open_questions={:?}, assumptions={:?}.\n\
                     Provide a concise reasoning analysis identifying gaps, \
                     risks, and improvement suggestions (max {} tokens).",
                    plan.goal,
                    plan.steps.len(),
                    plan.steps
                        .iter()
                        .filter(|s| s.status == StepStatus::Pending)
                        .count(),
                    context.confidence,
                    context.reasoning_trace,
                    context.open_questions,
                    context.assumptions,
                    self.max_reasoning_tokens,
                );
                let reasoning = Self::call_llm_and_collect(&agent, &prompt).await;
                enriched.reasoning = Some(reasoning);
                return enriched;
            }
        }

        // ── Fallback: structured summary ───────────────────────────────
        enriched.reasoning = Some(format!(
            "Deep reasoning analysis (max_tokens={}):\n\
             - Context id: {}\n\
             - Confidence: {:.2}\n\
             - Reasoning trace ({} steps): {:?}\n\
             - Open questions: {:?}\n\
             - Assumptions: {:?}\n\
             - Plan goal: {}\n\
             - Steps: {} pending / {} total",
            self.max_reasoning_tokens,
            context.id,
            context.confidence,
            context.reasoning_trace.len(),
            context.reasoning_trace,
            context.open_questions,
            context.assumptions,
            plan.goal,
            plan.steps
                .iter()
                .filter(|s| s.status == StepStatus::Pending)
                .count(),
            plan.steps.len(),
        ));
        enriched
    }

    /// Produce a reflection enhanced with LLM-level improvement suggestions.
    ///
    /// Takes the step execution result and reflection history, and returns
    /// a [`BrainLoopReflection`] with deeper analysis.
    ///
    /// When deep reasoning is disabled, returns a basic empty reflection.
    ///
    /// When an `agent_registry` is available (B51-08), calls the configured
    /// LLM agent to generate real analysis. Otherwise falls back to a
    /// structured summary.
    pub async fn reflect_with_reasoning(
        &self,
        result: &str,
        history: &[BrainLoopReflection],
        plan: &BrainLoopPlan,
        step_id: &str,
    ) -> BrainLoopReflection {
        if self.max_reasoning_tokens == 0 {
            return BrainLoopReflection {
                step_id: step_id.to_string(),
                observations: vec![result.to_string()],
                issues: vec![],
                improvements: vec![],
                confidence: 1.0,
                reflection_ms: now_epoch_ms(),
                context_snapshot: None,
                reasoning_chain: vec![],
            };
        }

        let now = now_epoch_ms();
        let prev_confidence = history.last().map(|r| r.confidence).unwrap_or(1.0);

        // ── Attempt LLM-backed reflection if agent registry is available ──
        if let Some(ref registry) = self.agent_registry {
            if let Some(agent) = registry.get("primary") {
                let prompt = format!(
                    "You are a deep reflection engine analyzing execution results.\n\
                     Step \"{}\" of plan \"{}\" (iteration {}/{}).\n\
                     Execution result: {}\n\
                     Prior reflections: {}\n\
                     Previous confidence: {:.2}\n\
                     \n\
                     Provide a structured reflection with:\n\
                     1. Key observations\n\
                     2. Issues identified\n\
                     3. Concrete improvements\n\
                     4. Confidence score (0.0-1.0)\n\
                     Keep the analysis concise (max {} tokens).",
                    step_id,
                    plan.goal,
                    plan.current_iteration,
                    plan.max_iterations,
                    result,
                    history.len(),
                    prev_confidence,
                    self.max_reasoning_tokens,
                );
                let analysis = Self::call_llm_and_collect(&agent, &prompt).await;
                return BrainLoopReflection {
                    step_id: step_id.to_string(),
                    observations: vec![result.to_string(), analysis.clone()],
                    issues: vec![],
                    improvements: vec![],
                    confidence: (prev_confidence + 0.9) / 2.0,
                    reflection_ms: now,
                    context_snapshot: None,
                    reasoning_chain: vec![analysis],
                };
            }
        }

        // ── Fallback: structured summary ───────────────────────────────
        let analysis = format!(
            "Deep reflection (max_tokens={}):\n\
             - Step result: {}\n\
             - Previous confidence: {:.2}\n\
             - Prior reflections: {}\n\
             - Plan iteration: {}/{}\n\
             Suggested improvements based on reasoning analysis.",
            self.max_reasoning_tokens,
            result,
            prev_confidence,
            history.len(),
            plan.current_iteration,
            plan.max_iterations,
        );

        BrainLoopReflection {
            step_id: step_id.to_string(),
            observations: vec![result.to_string(), analysis],
            issues: vec![],
            improvements: vec![
                "Review reasoning trace for gaps in logic".to_string(),
                "Cross-check open questions against execution output".to_string(),
            ],
            confidence: (prev_confidence + 0.9) / 2.0,
            reflection_ms: now,
            context_snapshot: None,
            reasoning_chain: vec![],
        }
    }

    /// Generate new steps using reflection content (not just confidence scores).
    ///
    /// When deep reasoning is disabled, returns an empty vector (no replanning).
    pub async fn replan_with_reasoning(
        &self,
        reflection: &BrainLoopReflection,
        plan: &BrainLoopPlan,
    ) -> Vec<BrainLoopStep> {
        if self.max_reasoning_tokens == 0 {
            return vec![];
        }

        let mut new_steps = Vec::new();
        for (i, improvement) in reflection.improvements.iter().enumerate() {
            let step_id = format!("{}-reasoned-{}", plan.id, i + 1);
            new_steps.push(BrainLoopStep {
                id: step_id,
                phase: BrainLoopPhase::Planning,
                description: improvement.clone(),
                input: String::new(),
                output: String::new(),
                started_ms: 0,
                completed_ms: 0,
                duration_ms: 0,
                status: StepStatus::Pending,
                context: None,
            });
        }
        new_steps
    }

    /// Validate a plan or reflection using LLM-backed consensus when
    /// an agent registry is available, or heuristic rules otherwise.
    ///
    /// Returns a quality score between 0.0 and 1.0.
    pub async fn quality_validate<T: Serialize>(&self, item: &T) -> f64 {
        if self.max_reasoning_tokens == 0 {
            return 1.0;
        }

        // ── LLM-backed validation if agent registry is available ───────
        if let Some(ref registry) = self.agent_registry {
            if let Some(agent) = registry.get("primary") {
                let json_str = serde_json::to_string(item).unwrap_or_default();
                let prompt = format!(
                    "You are a quality validator. Rate the quality of the following \
                     plan/reflection on a scale of 0.0 to 1.0. \
                     Consider completeness, clarity, reasoning depth, and feasibility.\n\
                     \n\
                     Item:\n{}\n\
                     \n\
                     Respond with ONLY a floating point number between 0.0 and 1.0.",
                    if json_str.len() > 2000 {
                        &json_str[..2000]
                    } else {
                        &json_str
                    }
                );
                let response = Self::call_llm_and_collect(&agent, &prompt).await;
                // Parse floating point from response
                let score: f64 = response
                    .trim()
                    .chars()
                    .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0.5);
                return score.clamp(0.0, 1.0);
            }
        }

        // ── Heuristic fallback ─────────────────────────────────────────
        let json = serde_json::to_value(item).unwrap_or_default();
        let score = if json.is_null() {
            0.0
        } else if let Some(obj) = json.as_object() {
            let has_reasoning = obj.contains_key("reasoning");
            let has_steps = obj.contains_key("steps") || obj.contains_key("observations");
            let has_confidence = obj.contains_key("confidence");
            let fields_score = (if has_reasoning { 0.3 } else { 0.0 })
                + (if has_steps { 0.4 } else { 0.0 })
                + (if has_confidence { 0.3 } else { 0.0 });
            let content_score =
                if let Some(reasoning) = obj.get("reasoning").and_then(|v| v.as_str()) {
                    let len = reasoning.len() as f64;
                    (len / 500.0).min(0.2)
                } else {
                    0.0
                };
            (fields_score + content_score).min(1.0)
        } else {
            0.5
        };
        score.clamp(0.0, 1.0)
    }

    // ── Private helpers ────────────────────────────────────────────────

    /// Call an LLM agent with a prompt and collect the full text response.
    async fn call_llm_and_collect(agent: &Arc<dyn Agent>, prompt: &str) -> String {
        let msg = Message {
            role: "user".to_string(),
            content: prompt.to_string(),
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(1024);
        let sender = StreamingSender::from(tx);
        let agent_clone = Arc::clone(agent);
        let msg_clone = msg.clone();

        let task =
            tokio::spawn(
                async move { agent_clone.chat(vec![msg_clone], None, None, sender).await },
            );

        let mut response = String::new();
        while let Some(token) = rx.recv().await {
            // Skip control tokens
            if token.starts_with("__model_used__:") {
                continue;
            }
            if token.starts_with("__tool_call__:") {
                continue;
            }
            if let Some(reasoning) = token.strip_prefix("__thinking__") {
                response.push_str(reasoning);
            } else {
                response.push_str(&token);
            }
        }

        match task.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!("DeepReasoningEngine: LLM call failed: {e}");
            }
            Err(join_err) => {
                tracing::warn!("DeepReasoningEngine: LLM task panicked: {join_err}");
            }
        }

        response
    }
}

// ---------------------------------------------------------------------------
// BrainLoopReport
// ---------------------------------------------------------------------------

/// Summary report produced by a full Plan → Execute → Reflect → Replan cycle.
// Reserved for future BrainLoop integration.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainLoopReport {
    /// Number of iterations executed.
    pub iterations: usize,
    /// Final composite score.
    pub final_score: f64,
    /// Whether the loop converged.
    pub converged: bool,
    /// Full history of steps across iterations.
    pub history: Vec<BrainLoopStep>,
}

// ---------------------------------------------------------------------------
// Reflection
// ---------------------------------------------------------------------------

/// A reflection produced after analysing a plan + result pair.
// Reserved for future BrainLoop integration.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reflection {
    pub score: f64,
    pub issues: Vec<String>,
    pub improvements: Vec<String>,
    pub converged: bool,
}
