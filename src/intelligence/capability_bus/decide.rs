//! Decision subsystem — stage 2 of the capability bus lifecycle
//!
//! Selects the best agent/strategy for a given task using multi-factor scoring
//! (reputation, recency, task-fit, recent outcomes) and Q-Learning integration.
//!
//! Extracted from `core.rs` to isolate the `decide()` method and all agent
//! selection logic. (BLUE38 ARCH-13)

use super::core::CapabilityBus;
use super::core::WorkflowLearningEvent;
use super::sense::SensingOutput;
use crate::governance::harness_bus::{AgentExecutionPolicy, PolicyVerdict};
use crate::governance::pua::TaskContext;
use crate::intelligence::adaptive_selector::ContextFeatures;
use crate::intelligence::token_cache::{estimate_token_count, ContextLengthClass};

use serde::Serialize;
use std::env;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Candidate scoring types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub(crate) struct CandidateScoreWeights {
    pub(crate) reputation: f64,
    pub(crate) recency: f64,
    pub(crate) task_fit: f64,
    pub(crate) recent_outcome: f64,
    pub(crate) causal_insight: f64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CandidateScoreBreakdown {
    pub(crate) agent: String,
    pub(crate) reputation_score: f64,
    pub(crate) recency_score: f64,
    pub(crate) task_fit_score: f64,
    pub(crate) recent_outcome_score: f64,
    pub(crate) causal_insight_score: f64,
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
    /// BLUE67-I2: Counterfactual score — probability that NOT selecting this
    /// agent would lead to a worse outcome, computed from the Bayesian causal graph.
    pub counterfactual_score: f64,
}

// ---------------------------------------------------------------------------
// Scoring helpers
// ---------------------------------------------------------------------------

pub(crate) fn configured_candidate_score_weights() -> CandidateScoreWeights {
    fn read_weight(key: &str, fallback: f64) -> f64 {
        env::var(key)
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .unwrap_or(fallback)
    }

    let weights = CandidateScoreWeights {
        reputation: read_weight("GO_ON_CAPABILITY_WEIGHT_REPUTATION", 0.40),
        recency: read_weight("GO_ON_CAPABILITY_WEIGHT_RECENCY", 0.12),
        task_fit: read_weight("GO_ON_CAPABILITY_WEIGHT_TASK_FIT", 0.23),
        recent_outcome: read_weight("GO_ON_CAPABILITY_WEIGHT_RECENT_OUTCOME", 0.15),
        causal_insight: read_weight("GO_ON_CAPABILITY_WEIGHT_CAUSAL_INSIGHT", 0.10),
    };
    let total = weights.reputation
        + weights.recency
        + weights.task_fit
        + weights.recent_outcome
        + weights.causal_insight;
    if total <= f64::EPSILON {
        CandidateScoreWeights {
            reputation: 0.40,
            recency: 0.12,
            task_fit: 0.23,
            recent_outcome: 0.15,
            causal_insight: 0.10,
        }
    } else {
        CandidateScoreWeights {
            reputation: weights.reputation / total,
            recency: weights.recency / total,
            task_fit: weights.task_fit / total,
            recent_outcome: weights.recent_outcome / total,
            causal_insight: weights.causal_insight / total,
        }
    }
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
    events: &[WorkflowLearningEvent],
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

/// Query agent capabilities using SemanticCapabilityMatcher (I7).
///
/// Uses the semantic matcher to find the best-matching agents for a
/// given task description, boosting scores when task keywords align
/// with capability tags. Wired into the `decide()` hot path so that
/// the matcher influences agent routing alongside reputation and
/// recency scores.
fn query_capabilities_semantic(
    bus: &CapabilityBus,
    task_description: &str,
) -> Vec<crate::intelligence::semantic_matcher::ScoredModel> {
    let graph = crate::lock_or_recover!(&bus.capability_graph, "intelligence");
    let capabilities: Vec<crate::intelligence::semantic_matcher::ModelCapability> = graph
        .all_capability_names()
        .into_iter()
        .map(
            |name| crate::intelligence::semantic_matcher::ModelCapability {
                model_id: name.to_string(),
                description: format!("Capability: {}", name),
                tags: vec![name.to_lowercase()],
            },
        )
        .collect();
    drop(graph);

    if capabilities.is_empty() {
        return Vec::new();
    }

    crate::intelligence::semantic_matcher::SemanticCapabilityMatcher::match_task_to_models(
        task_description,
        &capabilities,
    )
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
        let task_type_str = format!("{:?}", task.task_type);
        let mut scored: Vec<CandidateScoreBreakdown> = candidates
            .iter()
            .map(|name| {
                let reputation_score = sensing
                    .reputation_snapshot
                    .iter()
                    .find(|r| r.agent == *name)
                    .map(|r| r.score)
                    .unwrap_or(0.5);
                let recency_score = recency_score(&sensing.recent_agents, name);
                let task_fit_score = task_fit_score(task, name);
                let recent_outcome_score =
                    recent_outcome_score(&sensing.learning_snapshot, task, name);
                // BLUE67-I1: Query causal Bayesian graph for agent-task effectiveness
                let causal_insight_score =
                    self.world_model.causal_agent_insight(name, &task_type_str);
                let total_score = (reputation_score * weights.reputation)
                    + (recency_score * weights.recency)
                    + (task_fit_score * weights.task_fit)
                    + (recent_outcome_score * weights.recent_outcome)
                    + (causal_insight_score * weights.causal_insight);
                CandidateScoreBreakdown {
                    agent: name.clone(),
                    reputation_score,
                    recency_score,
                    task_fit_score,
                    recent_outcome_score,
                    causal_insight_score,
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
                    counterfactual_score: 0.0,
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
                    counterfactual_score: 0.0,
                };
            }
            PolicyVerdict::Allow
            | PolicyVerdict::Review(_)
            | PolicyVerdict::AllowWithConstraints(_) => {
                // Allowed — continue to agent selection.
            }
        }

        // Step B: consult ScenarioMatcher for pre-configured routing
        let task_type_str = format!("{:?}", task.task_type);
        let scenario_match =
            self.matcher
                .match_task(&task_type_str, &task_type_str, 0.5, task.risk_score, &[]);

        // ── P2-1: TokenCache check ────────────────────────────────────────
        // If a cached decision exists for this task type, return it directly
        // to skip full agent selection (hot path optimization).
        if let Some(ref cache) = self.token_cache {
            let context_class = ContextLengthClass::from_token_count(task_type_str.len());
            if let Some((_level, entry)) = cache.lookup(&task_type_str, context_class).await {
                tracing::info!(
                    "decide: token_cache hit for task_type={}, cached_agent={}",
                    task_type_str,
                    entry.agent_name.as_deref().unwrap_or("unknown")
                );
                let agent_policy = entry
                    .agent_name
                    .as_ref()
                    .map(|agent| self.harness.get_agent_policy(agent, &task_type_str));
                self.record_event(
                    "decision",
                    entry.agent_name.clone(),
                    None,
                    "cache_hit",
                    serde_json::json!({
                        "task_type": task_type_str,
                        "cached_agent": entry.agent_name,
                    }),
                );
                return DecisionOutput {
                    verdict: PolicyVerdict::Allow,
                    selected_agent: entry.agent_name.clone(),
                    agent_policy,
                    confidence: 0.9,
                    duration_ms: start.elapsed().as_millis() as u64,
                    recommended_mode: "auto".to_string(),
                    #[cfg(feature = "sub-bus-tool")]
                    available_tools: vec![],
                    counterfactual_score: 0.5,
                };
            }
        }

        // Step C: pick best agent from capability graph + reputation
        // BLUE56-B11: Also query QLearningAgent for learned routing preferences
        // First build candidate agent list, then use Q-learning to inform selection.
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
                candidates.retain(|name| !degrading_agents.contains(name));

                candidates
            })
            .unwrap_or_default();

        // Step D: Query DiscoveryCenter for prior solutions matching this task (I6).
        //         If a matching solution exists, prefer its associated agent.
        let discovery_query = crate::intelligence::discovery::DiscoveryQuery {
            problem_pattern: Some(task_type_str.clone()),
            tags: None,
            category: None,
            min_success_rate: Some(0.5),
            limit: Some(5),
        };
        let discovery_result = self.discovery.search(&discovery_query);
        if !discovery_result.entries.is_empty() {
            self.record_event(
                "decision",
                None,
                None,
                "discovery_match",
                serde_json::json!({
                    "total_matches": discovery_result.total_matches,
                    "best_match": discovery_result.best_match,
                }),
            );
        }

        // In profiles with tool bus, merge runtime-created sub-agent templates from AgentFactory.
        #[cfg(any(
            feature = "sub-bus-tool",
            feature = "simple-server",
            feature = "multi-users-server"
        ))]
        let candidate_agents = {
            let mut agents = candidate_agents;
            let factory = self.agent_factory.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            for inst in factory.find_agents_by_capability("general") {
                if !agents.iter().any(|name| name == &inst.template_name) {
                    agents.push(inst.template_name);
                }
            }
            agents
        };

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

        // Step E: Query SemanticCapabilityMatcher for semantic agent-task fit (I7).
        //         Logs the top-3 matches for observability.
        let semantic_matches = query_capabilities_semantic(self, &task_type_str);
        if !semantic_matches.is_empty() {
            let top_n: Vec<serde_json::Value> = semantic_matches
                .iter()
                .take(3)
                .map(|m| {
                    serde_json::json!({
                        "agent": m.model_id,
                        "score": m.score,
                        "reasons": m.match_reasons,
                    })
                })
                .collect();
            self.record_event(
                "decision",
                None,
                None,
                "semantic_match",
                serde_json::json!({
                    "top_matches": top_n,
                    "total_scored": semantic_matches.len(),
                }),
            );
        }

        // If ScenarioMatcher found a high-confidence match, prefer its routing
        let scenario_preferred_agent = if scenario_match.matched {
            scenario_match
                .scenario
                .as_ref()
                .and_then(|s| s.routing.preferred_agent.clone())
        } else {
            None
        };

        // If Q-learning has a strong preference and no scenario override, prefer the learned action.
        let q_learning_override = match (
            scenario_preferred_agent.as_ref(),
            q_preferred_action.as_ref(),
        ) {
            (Some(_), _) => None, // Scenario takes priority over Q-learning
            (None, Some(q_agent)) => {
                if candidate_agents.contains(q_agent) {
                    Some(q_agent.clone())
                } else {
                    None
                }
            }
            (None, None) => None,
        };

        // ── P2-3: AdaptiveModelSelector — re-rank candidates by performance context ──
        let model_selector_ranked: Option<Vec<String>> =
            self.model_selector.as_ref().map(|selector| {
                let context = ContextFeatures::from_time_and_task(&task_type_str);
                let candidates_with_models: Vec<(String, Option<String>)> = candidate_agents
                    .iter()
                    .map(|name| (name.clone(), None))
                    .collect();
                selector
                    .lock()
                    .map(|sel| sel.rank_candidates_with_context(&candidates_with_models, &context))
                    .unwrap_or_else(|_| {
                        tracing::warn!(
                            "decide: model_selector lock poisoned, using original order"
                        );
                        candidate_agents.clone()
                    })
            });
        let selection_candidates: &[String] = match model_selector_ranked {
            Some(ref ranked) => ranked.as_slice(),
            None => &candidate_agents,
        };

        // ── P2-7: HotFailover — check if preferred agents are blacklisted ──
        let effective_override =
            q_learning_override
                .or(scenario_preferred_agent)
                .and_then(|agent| {
                    if let Some(ref hf) = self.hot_failover {
                        if hf.is_blacklisted(&agent) {
                            tracing::warn!(
                        "decide: preferred agent {} is blacklisted by hot_failover, falling back",
                        agent
                    );
                            return None;
                        }
                    }
                    Some(agent)
                });

        let (selected_agent, score_breakdown) = if let Some(ref preferred) = effective_override {
            let breakdown = vec![CandidateScoreBreakdown {
                agent: preferred.clone(),
                reputation_score: 1.0,
                recency_score: 1.0,
                task_fit_score: 1.0,
                recent_outcome_score: 1.0,
                causal_insight_score: 1.0,
                total_score: 1.0,
            }];
            (Some(preferred.clone()), breakdown)
        } else {
            self.select_best_agent(task, selection_candidates, sensing)
        };
        tracing::info!(
            candidates = ?selection_candidates,
            selected = ?selected_agent,
            "capability_bus agent selection"
        );

        // ── P2-3: Record model selection outcome for adaptive learning ──
        if let Some(ref selector) = self.model_selector {
            let context = ContextFeatures::from_time_and_task(&task_type_str);
            if let Ok(mut sel) = selector.lock() {
                sel.record_result_with_context(
                    selected_agent.as_deref().unwrap_or("unknown"),
                    true,
                    Some(&context),
                );
            }
        }

        // ── P2-1: Store decision result in token_cache for future lookups ──
        if let Some(ref cache) = self.token_cache {
            if let Some(ref agent) = selected_agent {
                let token_count = estimate_token_count(&task_type_str);
                cache
                    .store(
                        &task_type_str,
                        agent,
                        token_count,
                        Some(agent.clone()),
                        None,
                    )
                    .await;
            }
        }

        // ── P2-6: LivePerformanceFeed — get real-time model cost estimates ──
        if let Some(ref perf) = self.live_performance {
            if let Some(ref agent) = selected_agent {
                if let Some(cost) = perf.get_cost_estimate(agent) {
                    tracing::debug!(
                        "decide: live performance cost estimate for {}: {:.2}",
                        agent,
                        cost
                    );
                }
                if let Some(latency) = perf.get_latency_estimate(agent) {
                    tracing::debug!(
                        "decide: live performance latency estimate for {}: {:.1}ms",
                        agent,
                        latency
                    );
                }
            }
        }

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
            .unwrap_or(0.5);

        // Phase 4: Get recommended execution mode from OrchestrationBus
        #[cfg(any(feature = "sub-bus-orchestration", feature = "sub-bus-tool"))]
        let task_type_str = format!("{:?}", task.task_type);
        #[cfg(feature = "sub-bus-orchestration")]
        let recommended_mode = self
            .orchestration_bus
            .recommend_mode(&task_type_str, task.risk_score);
        #[cfg(not(feature = "sub-bus-orchestration"))]
        let recommended_mode = "auto".to_string();

        // BLUE67-I2: Compute counterfactual score for the selected agent
        // Answers: "How much worse would the outcome be if we had NOT selected this agent?"
        let counterfactual_score = selected_agent.as_ref().map_or(0.5, |agent| {
            // Evaluate P(success | ¬agent) — the probability of success WITHOUT this agent
            let p_without = self
                .world_model
                .counterfactual_probability(agent, &task_type_str);
            // Counterfactual score: 1.0 - P(success | ¬agent)
            // Higher means the agent is more critical (harder to replace)
            (1.0 - p_without).clamp(0.0, 1.0)
        });

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
                "counterfactual_score": counterfactual_score,
                "recommended_mode": recommended_mode,
                "available_tools": available_tools.len(),
                "candidate_agents": candidate_agents.len(),
                "score_weights": {
                    "reputation": configured_candidate_score_weights().reputation,
                            "recency": configured_candidate_score_weights().recency,
                            "task_fit": configured_candidate_score_weights().task_fit,
                            "recent_outcome": configured_candidate_score_weights().recent_outcome,
                            "causal_insight": configured_candidate_score_weights().causal_insight,
                },
                "candidate_scores": score_breakdown,
            }),
        );

        #[cfg(feature = "sub-bus-observability")]
        let _healthy_agents_count = sensing.healthy_agents.len();

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
            counterfactual_score,
        }
    }
}
