//! Decision subsystem — stage 2 of the capability bus lifecycle
//!
//! Selects the best agent/strategy for a given task using multi-factor scoring
//! (reputation, recency, task-fit, recent outcomes) and Q-Learning integration.
//!
//! Extracted from `core.rs` to isolate the `decide()` method and all agent
//! selection logic. (BLUE38 ARCH-13)

use super::core::CapabilityBus;
use super::sense::SensingOutput;
use crate::governance::harness_bus::{AgentExecutionPolicy, PolicyVerdict};
use crate::governance::pua::TaskContext;

use crate::intelligence::capability_bus::learning_optimization_bus::LearningEvent;

use serde::Serialize;
use std::collections::HashMap;
use std::env;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Candidate scoring types
// ---------------------------------------------------------------------------

/// Weight of the Q-learning preference factor (the 6th scoring factor).
///
/// Deliberately NOT part of the five normalized `CandidateScoreWeights`:
/// the five factors stay normalized to 1.0 (see
/// `configured_candidate_score_weights`), and the Q-learned preference is an
/// additive nudge on top — it can inform routing but never override the real
/// five-factor evidence.
const Q_LEARNING_SCORE_WEIGHT: f64 = 0.05;

/// Maximum additive contribution of the semantic capability match (I7) to
/// `task_fit_score`. Small by design: the keyword-based task-fit
/// classification stays the primary signal; the semantic matcher's token-
/// overlap score can raise the fit by at most this amount.
const SEMANTIC_SUPPLEMENT_WEIGHT: f64 = 0.10;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CandidateScoreWeights {
    pub(crate) reputation: f64,
    pub(crate) recency: f64,
    pub(crate) task_fit: f64,
    pub(crate) recent_outcome: f64,
    pub(crate) discovery: f64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CandidateScoreBreakdown {
    pub(crate) agent: String,
    pub(crate) reputation_score: f64,
    pub(crate) recency_score: f64,
    pub(crate) task_fit_score: f64,
    pub(crate) recent_outcome_score: f64,
    pub(crate) discovery_score: f64,
    /// Semantic capability-match score (I7) — a small supplement to
    /// `task_fit_score`, real matcher output (0.0 when unmatched).
    pub(crate) semantic_score: f64,
    /// Q-learning preference factor (6th factor): 1.0 when this candidate is
    /// the Q-table's preferred action for the task type, else 0.0. Applied in
    /// `decide()` after the five-factor score is computed.
    pub(crate) q_learning_score: f64,
    pub(crate) total_score: f64,
}

// ---------------------------------------------------------------------------
// Stage output type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct DecisionOutput {
    pub verdict: PolicyVerdict,
    pub selected_agent: Option<String>,
    pub agent_policy: Option<AgentExecutionPolicy>,
    pub confidence: f64,
    pub duration_ms: u64,
    /// Phase 4: recommended execution mode
    pub recommended_mode: String,
    /// Phase 4: tools available for the selected agent
    #[cfg(feature = "sub-bus-tool")]
    pub available_tools: Vec<String>,
}

// ---------------------------------------------------------------------------
// Scoring helpers
// ---------------------------------------------------------------------------

pub(crate) fn configured_candidate_score_weights() -> CandidateScoreWeights {
    // Cache: env weights are startup-time configuration; re-reading 5 env vars
    // on every decide() (up to ~30 lookups/request) is wasted work.
    static CACHED: std::sync::OnceLock<CandidateScoreWeights> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        fn read_weight(key: &str, fallback: f64) -> f64 {
            env::var(key)
                .ok()
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0)
                .unwrap_or(fallback)
        }

        let weights = CandidateScoreWeights {
            reputation: read_weight("GO_ON_CAPABILITY_WEIGHT_REPUTATION", 0.38),
            recency: read_weight("GO_ON_CAPABILITY_WEIGHT_RECENCY", 0.11),
            task_fit: read_weight("GO_ON_CAPABILITY_WEIGHT_TASK_FIT", 0.22),
            recent_outcome: read_weight("GO_ON_CAPABILITY_WEIGHT_RECENT_OUTCOME", 0.14),
            discovery: read_weight("GO_ON_CAPABILITY_WEIGHT_DISCOVERY", 0.05),
        };
        let total = weights.reputation
            + weights.recency
            + weights.task_fit
            + weights.recent_outcome
            + weights.discovery;
        if total <= f64::EPSILON {
            CandidateScoreWeights {
                reputation: 0.38,
                recency: 0.11,
                task_fit: 0.22,
                recent_outcome: 0.14,
                discovery: 0.05,
            }
        } else {
            CandidateScoreWeights {
                reputation: weights.reputation / total,
                recency: weights.recency / total,
                task_fit: weights.task_fit / total,
                recent_outcome: weights.recent_outcome / total,
                discovery: weights.discovery / total,
            }
        }
    })
}

pub(crate) fn task_fit_score(task: &TaskContext, agent_name: &str) -> f64 {
    let normalized = agent_name.to_ascii_lowercase();
    let prefers =
        |needles: &[&str]| -> bool { needles.iter().any(|needle| normalized.contains(needle)) };

    match task.task_type {
        crate::governance::pua::TaskType::BugFix => {
            if prefers(&["fix", "debug", "coder", "review"]) {
                0.95
            } else {
                0.60
            }
        }
        crate::governance::pua::TaskType::FeatureAdd => {
            if prefers(&["feature", "builder", "planner", "coder"]) {
                0.95
            } else {
                0.65
            }
        }
        crate::governance::pua::TaskType::Refactor => {
            if prefers(&["refactor"]) {
                1.00
            } else if prefers(&["planner", "review"]) {
                0.80
            } else if prefers(&["coder"]) {
                0.55
            } else {
                0.25
            }
        }
        crate::governance::pua::TaskType::SecurityPatch => {
            if prefers(&["security", "audit", "review", "guard"]) {
                1.0
            } else {
                0.50
            }
        }
        crate::governance::pua::TaskType::Other => 0.60,
    }
}

pub(crate) fn recency_score(recent_agents: &[String], agent_name: &str) -> f64 {
    if recent_agents.is_empty() {
        return 0.50;
    }

    recent_agents
        .iter()
        .rev()
        .position(|recent| recent == agent_name)
        .map(|index| {
            let rank = index as f64 / recent_agents.len().max(1) as f64;
            (1.0 - rank).clamp(0.0, 1.0)
        })
        .unwrap_or(0.40)
}

pub(crate) fn recent_outcome_score(
    events: &[LearningEvent],
    task: &TaskContext,
    agent_name: &str,
) -> f64 {
    let mut weighted_total = 0.0;
    let mut weighted_success = 0.0;
    let target_task = format!("{:?}", task.task_type);

    for (idx, event) in events
        .iter()
        .rev()
        .filter(|event| event.agent == agent_name)
        .take(20)
        .enumerate()
    {
        let freshness_weight = 1.0 / ((idx + 1) as f64);
        let task_weight = if event.task_type == target_task {
            1.0
        } else {
            0.6
        };
        let weight = freshness_weight * task_weight;
        weighted_total += weight;
        if event.success {
            weighted_success += weight;
        }
    }

    if weighted_total <= f64::EPSILON {
        0.50
    } else {
        (weighted_success / weighted_total).clamp(0.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

/// Query candidate agent capabilities using SemanticCapabilityMatcher (I7).
///
/// Scores each candidate agent (by name) against the task description with
/// the semantic matcher. Consumed inside `select_best_agent` so the matcher's
/// token-overlap score supplements `task_fit_score` (small weight) instead of
/// only producing a log event. Scans the candidates only — the former
/// full-graph iteration (`all_capability_names()`) returned capability-decl
/// names, not agent names, so its scores could never be attributed to a
/// candidate.
fn query_capabilities_semantic(
    task_description: &str,
    candidates: &[String],
) -> Vec<crate::intelligence::semantic_matcher::ScoredModel> {
    if candidates.is_empty() {
        return Vec::new();
    }

    let capabilities: Vec<crate::intelligence::semantic_matcher::ModelCapability> = candidates
        .iter()
        .map(
            |name| crate::intelligence::semantic_matcher::ModelCapability {
                model_id: name.to_string(),
                description: format!("Capability: {}", name),
                tags: vec![name.to_lowercase()],
            },
        )
        .collect();

    crate::intelligence::semantic_matcher::SemanticCapabilityMatcher::match_task_to_models(
        task_description,
        &capabilities,
    )
}

/// Apply the Q-learning preference as the 6th candidate-scoring factor.
///
/// The five normalized factors are computed first (via `select_best_agent`);
/// this nudge is deliberately small and additive (`q_weight`) so a learned
/// preference can inform routing without overriding the real five-factor
/// evidence. The preferred agent's breakdown shows `q_learning_score = 1.0`
/// and its `total_score` is raised by `q_weight`; every other entry keeps its
/// real factor values. Returns the re-sorted breakdown using the same
/// tie-break as `select_best_agent` (total descending, then agent name
/// ascending).
fn apply_q_learning_factor(
    mut breakdown: Vec<CandidateScoreBreakdown>,
    q_preferred: Option<&str>,
    q_weight: f64,
) -> Vec<CandidateScoreBreakdown> {
    if let Some(preferred) = q_preferred {
        for entry in breakdown.iter_mut() {
            if entry.agent == preferred {
                entry.q_learning_score = 1.0;
                entry.total_score += q_weight;
            }
        }
        breakdown.sort_by(|a, b| {
            b.total_score
                .partial_cmp(&a.total_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.agent.cmp(&b.agent))
        });
    }
    breakdown
}

impl CapabilityBus {
    pub(crate) fn select_best_agent(
        &self,
        task: &TaskContext,
        candidates: &[String],
        sensing: &SensingOutput,
    ) -> (Option<String>, Vec<CandidateScoreBreakdown>) {
        if candidates.is_empty() {
            return (None, Vec::new());
        }
        let weights = configured_candidate_score_weights();
        // Semantic capability match (I7): real matcher output per candidate,
        // consumed below as a small supplement to `task_fit_score` (the
        // former per-request full-graph scan only produced a log event).
        let semantic_scores: HashMap<String, f64> =
            query_capabilities_semantic(&format!("{:?}", task.task_type), candidates)
                .into_iter()
                .map(|m| (m.model_id, m.score))
                .collect();
        // Historical solution knowledge: DiscoveryCenter entries written by
        // `evolve_discovery` use `problem_pattern = "state_{agent}"`, so a
        // per-agent lookup surfaces that agent's past success rate for the
        // same task type. Computed for all candidates in ONE lock acquisition
        // and ONE pass (previously this was an N× full scan via `search`).
        let discovery_patterns: Vec<String> =
            candidates.iter().map(|name| format!("state_{}", name)).collect();
        let discovery_rates = self.discovery.best_success_rates(&discovery_patterns, 0.5);
        let mut scored: Vec<CandidateScoreBreakdown> = candidates
            .iter()
            .map(|name| {
                let reputation_score = sensing
                    .reputation_snapshot
                    .iter()
                    .find(|r| r.agent == *name)
                    .map(|r| r.score)
                    .unwrap_or(crate::acp::helpers::agent_selector::DEFAULT_REPUTATION_SCORE);
                let recency_score = recency_score(&sensing.recent_agents, name);
                let task_fit_score = {
                    let keyword_fit = task_fit_score(task, name);
                    // Semantic supplement: the matcher's token-overlap score can
                    // raise the keyword-based fit by at most
                    // SEMANTIC_SUPPLEMENT_WEIGHT, keeping keyword classification
                    // dominant while the semantic signal participates in real
                    // scoring.
                    let semantic = semantic_scores
                        .get(name)
                        .copied()
                        .unwrap_or(0.0)
                        .clamp(0.0, 1.0);
                    (keyword_fit + semantic * SEMANTIC_SUPPLEMENT_WEIGHT).min(1.0)
                };
                let semantic_score = semantic_scores.get(name).copied().unwrap_or(0.0);
                let recent_outcome_score =
                    recent_outcome_score(&sensing.learning_snapshot, task, name);
                // Historical solution knowledge: DiscoveryCenter entries written by
                // `evolve_discovery` use `problem_pattern = "state_{agent}"`, so a
                // per-agent lookup surfaces that agent's past success rate for the
                // same task type. High-rate agents get a real score boost instead of
                // the knowledge being recorded-and-discarded.
                let discovery_score = discovery_rates
                    .get(&format!("state_{}", name))
                    .copied()
                    .unwrap_or(0.0);
                let total_score = (reputation_score * weights.reputation)
                    + (recency_score * weights.recency)
                    + (task_fit_score * weights.task_fit)
                    + (recent_outcome_score * weights.recent_outcome)
                    + (discovery_score * weights.discovery);
                CandidateScoreBreakdown {
                    agent: name.clone(),
                    reputation_score,
                    recency_score,
                    task_fit_score,
                    recent_outcome_score,
                    discovery_score,
                    // Q-learning factor is applied in `decide()` after the
                    // five-factor score is computed (see
                    // `apply_q_learning_factor`); direct callers of
                    // `select_best_agent` score without a Q preference.
                    semantic_score,
                    q_learning_score: 0.0,
                    total_score,
                }
            })
            .collect();
        scored.sort_by(|a, b| {
            b.total_score
                .partial_cmp(&a.total_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.agent.cmp(&b.agent))
        });
        (scored.first().map(|entry| entry.agent.clone()), scored)
    }

    // ------------------------------------------------------------------
    // Stage 2: Decision — select agent / strategy
    // ------------------------------------------------------------------

    pub async fn decide(&self, task: &TaskContext, sensing: &SensingOutput) -> DecisionOutput {
        let start = Instant::now();

        // Step A: HarnessBus policy evaluation (compliance gate)
        let verdict = self.harness.evaluate(task).await;
        match &verdict {
            PolicyVerdict::Deny(v) => {
                self.record_event(
                    "decision",
                    None,
                    None,
                    "blocked",
                    serde_json::json!({"reason": v.detail}),
                );
                return DecisionOutput {
                    verdict,
                    selected_agent: None,
                    agent_policy: None,
                    confidence: 0.0,
                    duration_ms: start.elapsed().as_millis() as u64,
                    recommended_mode: "ask".to_string(),
                    // When policy denies, provide system health tools for degraded-mode access:
                    // health check, diagnostics, and audit review.
                    #[cfg(feature = "sub-bus-tool")]
                    available_tools: vec![
                        "health".to_string(),
                        "diagnostics".to_string(),
                        "audit".to_string(),
                    ],
                };
            }
            PolicyVerdict::Escalate(r) => {
                self.record_event(
                    "decision",
                    None,
                    None,
                    "degraded",
                    serde_json::json!({"reason": r.reason}),
                );
                return DecisionOutput {
                    verdict,
                    selected_agent: None,
                    agent_policy: None,
                    confidence: 0.0,
                    duration_ms: start.elapsed().as_millis() as u64,
                    recommended_mode: "ask".to_string(),
                    // Same fallback tools available during escalation for manual review:
                    #[cfg(feature = "sub-bus-tool")]
                    available_tools: vec![
                        "health".to_string(),
                        "diagnostics".to_string(),
                        "audit".to_string(),
                    ],
                };
            }
            PolicyVerdict::Allow | PolicyVerdict::Review(_) => {
                // Allowed — continue to agent selection.
            }
        }

        // Step C: pick best agent from capability graph + reputation
        // BLUE56-B11: Also query QLearningAgent for learned routing preferences
        // First build candidate agent list, then use Q-learning to inform selection.
        // (The former P2-1 token-cache fast path was removed in round 32: it
        // shared the LLM response cache and its no-TTL task→agent entries
        // would freeze agent routing; full selection runs every time.)
        let task_type_str = format!("{:?}", task.task_type);
        // Lock ordering (core.rs): evolution_graph precedes capability_graph.
        // Snapshot degrading agents first (evolution_graph scope), then acquire
        // capability_graph — never the reverse, or a future caller following
        // the documented order would deadlock against this path.
        let degrading_agents: Vec<String> = self
            .evolution_graph
            .lock()
            .map(|eg| {
                eg.find_degrading_capabilities()
                    .into_iter()
                    .map(|(agent, _, _)| agent)
                    .collect()
            })
            .unwrap_or_default();
        let candidate_agents = self
            .capability_graph
            .lock()
            .map(|g| {
                let mut candidates: Vec<String> = g
                    .agents_with_tag("general")
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect();
                if candidates.is_empty() {
                    let all: Vec<String> = g
                        .all_capability_names()
                        .into_iter()
                        .map(|s| s.to_string())
                        .collect();
                    candidates = all;
                }

                // Exclude agents that are degrading according to EvolutionGraph
                candidates.retain(|name| !degrading_agents.contains(name));

                candidates
            })
            .unwrap_or_default();

        // Step D: Historical solution knowledge is consumed inside
        // `select_best_agent` — per-agent DiscoveryCenter lookups (problem_pattern
        // `state_{agent}`) boost candidates with proven success. The former
        // task-type query here matched nothing (entries are keyed by agent, not
        // by TaskType) and its result was only recorded to an event.

        // Step C2: BLUE70: Query ReinforcementBus for learned routing preferences.
        let q_preferred_action = {
            let rb = crate::read_or_recover!(&self.reinforcement_bus, "intelligence");
            rb.select_action(&task_type_str, &candidate_agents)
        };
        if let Some(ref preferred) = q_preferred_action {
            self.record_event(
                "decision",
                None,
                None,
                "reinforcement_preference",
                serde_json::json!({
                    "preferred_agent": preferred,
                    "state": task_type_str,
                }),
            );
        }

        // Step E: SemanticCapabilityMatcher (I7) is consumed inside
        // `select_best_agent` — its per-candidate score supplements
        // `task_fit_score` (small weight), so the per-request full-graph scan
        // feeds real scoring instead of a log-only event. Observability for
        // the semantic signal is recorded below from the computed breakdown.

        // Five-factor scoring runs for every candidate; the Q-learning
        // preference is applied afterwards as a small 6th factor (see
        // `apply_q_learning_factor`), so the breakdown always reflects real
        // computed values instead of a fabricated all-1.0 override.
        //
        // The former P2-3 adaptive re-rank fed `candidate_agents` through the
        // shared AdaptiveModelSelector (UCB) before scoring — a no-op:
        // `select_best_agent` re-sorts by `total_score` anyway, so the input
        // order never affected the outcome, and every request paid an O(n log
        // n) UCB computation plus a lock acquisition for nothing. The shared
        // selector is still fed real outcomes by the execution path
        // (exec_pack/task.rs `record_result`).
        let mut score_breakdown = self.select_best_agent(task, &candidate_agents, sensing).1;
        score_breakdown = apply_q_learning_factor(
            score_breakdown,
            q_preferred_action.as_deref(),
            Q_LEARNING_SCORE_WEIGHT,
        );
        let selected_agent = score_breakdown.first().map(|entry| entry.agent.clone());
        tracing::info!(
            candidates = ?candidate_agents,
            selected = ?selected_agent,
            "capability_bus agent selection"
        );

        // Observability: surface the top semantic matches (derived from the
        // breakdown computed above; the matcher itself now feeds task-fit).
        let mut by_semantic: Vec<&CandidateScoreBreakdown> = score_breakdown.iter().collect();
        by_semantic.sort_by(|a, b| b.semantic_score.total_cmp(&a.semantic_score));
        let top_semantic: Vec<serde_json::Value> = by_semantic
            .iter()
            .take(3)
            .filter(|e| e.semantic_score > 0.0)
            .map(|e| serde_json::json!({ "agent": e.agent, "score": e.semantic_score }))
            .collect();
        if !top_semantic.is_empty() {
            self.record_event(
                "decision",
                None,
                None,
                "semantic_match",
                serde_json::json!({
                    "top_matches": top_semantic,
                    "total_scored": score_breakdown.len(),
                }),
            );
        }

        // ── No decision-time adaptive-learning record here ──
        // The adaptive selector's UCB statistics are fed exclusively by the
        // execution path (exec_pack/task.rs records `record_result` with the
        // REAL execution outcome). Recording a self-confirming `success=true`
        // at decision time would feed the statistics with outcomes that were
        // never observed, biasing per-agent ranking.

        // ── P2-6: LivePerformanceFeed — consumed at model-selection time ──
        // The feed's EMA cost/latency estimates are real but are read by the
        // orchestrator's model selection (`select_model_for_task` →
        // `estimate_model_cost` / `estimate_model_latency`), where a model
        // choice is actually made. Reading them here produced only debug logs
        // that took no part in scoring; the block was removed rather than
        // wired into ranking because per-agent estimates are sparse (EMA over
        // observed outcomes) and adding them as a decision factor would inject
        // a weak, noisy signal on top of the five real factors.

        // Step B2: Consult WorkflowRegistry for workflow-based routing metadata
        let workflow_preset = self.workflow_registry.as_ref().and_then(|wr| {
            let registry = wr.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            let task_type_str = format!("{:?}", task.task_type).to_lowercase();
            let mapped_name = match task_type_str.as_str() {
                "bugfix" | "featureadd" | "refactor" | "securitypatch" => "dev",
                _ => "general",
            };
            registry.find(mapped_name).cloned()
        });

        if let Some(ref preset) = workflow_preset {
            self.record_event(
                "decision",
                selected_agent.clone(),
                None,
                "workflow_matched",
                serde_json::json!({
                    "preset_name": preset.name,
                    "workflow_type": format!("{:?}", preset.workflow_type),
                    "phases": preset.phases,
                }),
            );
        }

        // Step C: build agent execution policy from HarnessBus
        let agent_policy = Some(self.harness.get_agent_policy(
            selected_agent.as_deref().unwrap_or("unknown"),
            &format!("{:?}", task.task_type),
        ));

        let confidence = score_breakdown
            .iter()
            .find(|entry| Some(entry.agent.as_str()) == selected_agent.as_deref())
            .map(|entry| entry.total_score)
            // No score for the selected agent — neutral confidence (same
            // convention as DEFAULT_REPUTATION_SCORE).
            .unwrap_or(crate::acp::helpers::agent_selector::DEFAULT_REPUTATION_SCORE);

        // Phase 4: Get recommended execution mode from OrchestrationBus
        #[cfg(any(feature = "sub-bus-orchestration", feature = "sub-bus-tool"))]
        let task_type_str = format!("{:?}", task.task_type);
        #[cfg(feature = "sub-bus-orchestration")]
        let recommended_mode = self
            .orchestration_bus
            .recommend_mode(&task_type_str, task.risk_score);
        #[cfg(not(feature = "sub-bus-orchestration"))]
        let recommended_mode = "auto".to_string();

        // Phase 4: Get available tools for the selected agent via ToolBus
        #[cfg(feature = "sub-bus-tool")]
        let available_tools = selected_agent
            .as_ref()
            .map(|agent| self.tool_bus.agent_tool_match(agent, &task_type_str))
            .unwrap_or_default();
        #[cfg(not(feature = "sub-bus-tool"))]
        let available_tools = Vec::<String>::new();

        self.record_event(
            "decision",
            selected_agent.clone(),
            None,
            "success",
            serde_json::json!({
                "confidence": confidence,
                "recommended_mode": recommended_mode,
                "available_tools": available_tools.len(),
                "candidate_agents": candidate_agents.len(),
                "score_weights": {
                    "reputation": configured_candidate_score_weights().reputation,
                            "recency": configured_candidate_score_weights().recency,
                            "task_fit": configured_candidate_score_weights().task_fit,
                            "recent_outcome": configured_candidate_score_weights().recent_outcome,
                },
                "candidate_scores": score_breakdown,
            }),
        );

        {
            let mut p = crate::write_or_recover!(&self.profile, "intelligence");
            p.routing_count = p.routing_count.saturating_add(1);
            p.last_route_duration_ms = start.elapsed().as_millis() as u64;
        }

        // Log agent selection — previously sent via MultiChannelTransport
        // which was removed as dead code (~740 lines, only 1 usage).
        if let Some(agent) = &selected_agent {
            tracing::debug!(
                agent = %agent,
                "decide: agent selected (MultiChannelTransport removed)"
            );
        }

        DecisionOutput {
            verdict,
            selected_agent,
            agent_policy,
            confidence,
            duration_ms: start.elapsed().as_millis() as u64,
            recommended_mode,
            #[cfg(feature = "sub-bus-tool")]
            available_tools,
        }
    }
}

#[cfg(all(test, feature = "sub-bus-tool"))]
mod tests {
    use super::*;
    use crate::governance::harness_bus::default_harness_bus;
    use crate::governance::pua::{TaskContext, TaskType};
    use crate::intelligence::capability_graph::CapabilityDecl;
    use std::sync::Arc;

    fn register_test_agent(
        graph: &mut crate::intelligence::capability_graph::CapabilityGraph,
        name: &str,
        tags: Vec<&str>,
    ) {
        let decls: Vec<CapabilityDecl> = tags
            .into_iter()
            .map(|t| CapabilityDecl {
                name: t.to_string(),
                description: String::new(),
                tags: vec![t.to_string()],
            })
            .collect();
        graph.register_agent(name, decls);
    }

    fn make_sensing(bus: &CapabilityBus, recent_agents: Vec<String>) -> SensingOutput {
        let snapshot = bus
            .unified_knowledge_bus
            .read()
            .map(|ukb| {
                ukb.all_reputations()
                    .into_iter()
                    .map(|r| crate::intelligence::reputation::ReputationRecord {
                        agent: r.agent.clone(),
                        score: r.score,
                        total_tasks: r.total_tasks,
                        success_count: r.successful_tasks,
                        failure_count: r.total_tasks.saturating_sub(r.successful_tasks),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        SensingOutput {
            capability_agent_count: 0,
            reputation_snapshot: snapshot,
            recent_agents,
            learning_snapshot: Vec::new(),
        }
    }

    /// F2: the Q-learning preference must be a real 6th factor, not a
    /// fabricated all-1.0 breakdown override. The pure helper keeps the
    /// five-factor evidence dominant (small additive nudge) and records the
    /// real `q_learning_score` on the preferred candidate.
    #[test]
    fn apply_q_learning_factor_is_a_small_nudge_not_an_override() {
        let mk = |agent: &str, base: f64| CandidateScoreBreakdown {
            agent: agent.to_string(),
            reputation_score: 0.8,
            recency_score: 0.5,
            task_fit_score: 0.7,
            recent_outcome_score: 0.5,
            discovery_score: 0.0,
            semantic_score: 0.0,
            q_learning_score: 0.0,
            total_score: base,
        };

        let breakdown = vec![mk("agent-x", 0.9), mk("agent-y", 0.5)];
        let ranked = apply_q_learning_factor(breakdown, Some("agent-y"), Q_LEARNING_SCORE_WEIGHT);

        // Real factor values are kept — nothing is fabricated to 1.0.
        let x = ranked.iter().find(|e| e.agent == "agent-x").unwrap();
        assert_eq!(x.q_learning_score, 0.0);
        assert!((x.total_score - 0.9).abs() < 1e-9, "agent-x unchanged");
        let y = ranked.iter().find(|e| e.agent == "agent-y").unwrap();
        assert_eq!(y.q_learning_score, 1.0);
        assert!(
            (y.total_score - (0.5 + Q_LEARNING_SCORE_WEIGHT)).abs() < 1e-9,
            "agent-y nudged by exactly the Q weight"
        );
        assert!(
            y.reputation_score < 1.0 || y.task_fit_score < 1.0,
            "five-factor scores must stay real (not all 1.0)"
        );
        // The nudge must not override a clearly better five-factor candidate.
        assert_eq!(ranked[0].agent, "agent-x");
    }

    /// F2 (E2E): through `decide()`, a Q-preferred agent must not be forced
    /// through with a fabricated breakdown — the five factors still decide,
    /// and the preferred agent's breakdown shows a real `q_learning_score`
    /// on top of real factor values.
    #[tokio::test]
    async fn decide_q_learning_preference_is_factor_not_override() {
        let harness = Arc::new(default_harness_bus());
        let bus = CapabilityBus::new_default(harness, None);
        {
            let mut graph = bus
                .capability_graph
                .lock()
                .expect("capability_graph lock should not be poisoned");
            register_test_agent(&mut graph, "a-star", vec!["general"]);
            register_test_agent(&mut graph, "b-weak", vec!["general"]);
        }
        {
            let mut ukb = bus
                .unified_knowledge_bus
                .write()
                .expect("unified_knowledge_bus lock should not be poisoned");
            for _ in 0..5 {
                ukb.record_outcome("a-star", "test", true, "test setup".to_string());
            }
            ukb.record_outcome("b-weak", "test", true, "test setup".to_string());
        }
        // Deterministic Q-table: b-weak is the learned preference for BugFix.
        {
            let mut rb = bus
                .reinforcement_bus
                .write()
                .expect("reinforcement_bus lock should not be poisoned");
            rb.set_exploration_rate(0.0);
            for _ in 0..3 {
                rb.record_reward("BugFix", "b-weak", 1.0, "BugFix/next");
            }
        }

        let task = TaskContext {
            task_type: TaskType::BugFix,
            file_count: 2,
            risk_score: 0.5,
        };
        let sensing = bus.sense(&task);
        let decision = bus.decide(&task, &sensing).await;

        // Five-factor evidence dominates: a-star's reputation (5 wins) beats
        // b-weak even after the Q nudge. The old override would have forced
        // b-weak through with a fabricated all-1.0 breakdown.
        assert_eq!(decision.selected_agent.as_deref(), Some("a-star"));
        assert_ne!(decision.selected_agent.as_deref(), Some("b-weak"));

        // The production selection path (the exact functions `decide()` calls)
        // yields a real breakdown: b-weak carries the Q signal on top of its
        // real factor values — nothing is fabricated to 1.0.
        let candidates: Vec<String> = vec!["a-star".to_string(), "b-weak".to_string()];
        let breakdown = apply_q_learning_factor(
            bus.select_best_agent(&task, &candidates, &sensing).1,
            Some("b-weak"),
            Q_LEARNING_SCORE_WEIGHT,
        );
        let b = breakdown.iter().find(|e| e.agent == "b-weak").unwrap();
        assert_eq!(b.q_learning_score, 1.0);
        assert!(
            b.reputation_score < 1.0 || b.task_fit_score < 1.0,
            "five-factor scores must be real, not fabricated 1.0"
        );
    }

    /// F3: the semantic matcher output must participate in real scoring — it
    /// supplements `task_fit_score` (small weight) and the breakdown exposes
    /// the real matcher score.
    #[tokio::test]
    async fn semantic_match_supplements_task_fit_score() {
        let harness = Arc::new(default_harness_bus());
        let bus = CapabilityBus::new_default(harness, None);
        {
            let mut graph = bus
                .capability_graph
                .lock()
                .expect("capability_graph lock should not be poisoned");
            // "bugfix-pro" shares a token with the "BugFix" task type, so the
            // semantic matcher scores it well above the unknown-tag floor.
            register_test_agent(&mut graph, "bugfix-pro", vec!["general"]);
            register_test_agent(&mut graph, "generic-agent", vec!["general"]);
        }
        let task = TaskContext {
            task_type: TaskType::BugFix,
            file_count: 3,
            risk_score: 0.6,
        };
        let sensing = make_sensing(&bus, Vec::new());
        let (_, breakdown) = bus.select_best_agent(
            &task,
            &["bugfix-pro".to_string(), "generic-agent".to_string()],
            &sensing,
        );

        let pro = breakdown.iter().find(|e| e.agent == "bugfix-pro").unwrap();
        let generic = breakdown
            .iter()
            .find(|e| e.agent == "generic-agent")
            .unwrap();
        // Real matcher output, direction: token-overlap candidate scores higher.
        assert!(pro.semantic_score > generic.semantic_score);
        assert!(pro.semantic_score > 0.0);
        // Small-weight supplement to keyword fit (generic: 0.60 keyword fit +
        // ~0.02 semantic × 0.10 weight → above the keyword-only baseline).
        assert!(generic.task_fit_score > 0.60);
        assert!(pro.task_fit_score > generic.task_fit_score);
        // Keyword classification stays primary: the supplement is bounded.
        assert!(pro.task_fit_score <= 1.0);
        assert!(generic.task_fit_score < 0.61);
    }
}
