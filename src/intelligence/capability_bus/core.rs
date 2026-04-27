//! Core CapabilityBus implementation.
//!
//! Phased implementation — all types are public and ready for HarnessBus
//! integration. dead_code & unused warnings will resolve once wired into
//! the main request lifecycle in Phase 1.
//!
//! This module defines the top-level `CapabilityBus` struct that holds references
//! to all sub-bus components and orchestrates the sensing → decision → action →
//! feedback → evolution loop.

#![allow(dead_code, unused_variables)]

use crate::governance::harness_bus::{AgentExecutionPolicy, HarnessBus, PolicyVerdict};
use crate::governance::pua::TaskContext;
use crate::intelligence::capability_graph::CapabilityGraph;
use crate::intelligence::reinforcement::learning::{
    ExperienceKnowledgeBase, QLearningAgent, RewardFunction, RlTaskExecutionMetrics, SuccessCase,
};
use crate::intelligence::reputation::ReputationStore;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Bus event record — each operation produces a traceable event
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusEvent {
    pub timestamp_ms: u64,
    pub stage: String, // "sense" | "decision" | "action" | "feedback" | "evolve"
    pub agent: Option<String>,
    pub task_id: Option<String>,
    pub outcome: String, // "success" | "failure" | "blocked" | "degraded"
    pub detail: Value,
}

// ---------------------------------------------------------------------------
// WorkflowLearningBus — in-memory runtime bus
// ---------------------------------------------------------------------------

/// Runtime event stored in the WorkflowLearningBus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowLearningEvent {
    pub task_type: String,
    pub agent: String,
    pub success: bool,
    pub duration_ms: u64,
    pub token_cost: u64,
    pub quality_score: f64,
    pub timestamp_ms: u64,
}

/// In-memory WorkflowLearningBus — replaces the file-only artifact.
#[derive(Debug)]
pub struct WorkflowLearningBus {
    events: VecDeque<WorkflowLearningEvent>,
    max_events: usize,
}

impl WorkflowLearningBus {
    pub fn new(max_events: usize) -> Self {
        Self {
            events: VecDeque::with_capacity(max_events.min(100)),
            max_events,
        }
    }

    /// Push a new event, evicting oldest if at capacity.
    pub fn push(&mut self, event: WorkflowLearningEvent) {
        if self.events.len() >= self.max_events {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    /// Historical success rate for a given agent, over the entire window.
    pub fn agent_success_rate(&self, agent: &str) -> Option<f64> {
        let agent_events: Vec<_> = self.events.iter().filter(|e| e.agent == agent).collect();
        if agent_events.is_empty() {
            return None;
        }
        let successes = agent_events.iter().filter(|e| e.success).count();
        Some(successes as f64 / agent_events.len() as f64)
    }

    /// Historical success rate for a given task type.
    pub fn task_type_success_rate(&self, task_type: &str) -> Option<f64> {
        let matching: Vec<_> = self
            .events
            .iter()
            .filter(|e| e.task_type == task_type)
            .collect();
        if matching.is_empty() {
            return None;
        }
        let successes = matching.iter().filter(|e| e.success).count();
        Some(successes as f64 / matching.len() as f64)
    }

    /// All events (for snapshot / endpoint)
    pub fn snapshot(&self) -> Vec<WorkflowLearningEvent> {
        self.events.iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }
}

// ---------------------------------------------------------------------------
// KnowledgeBus — in-memory runtime bus for reusable insights
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KnowledgeInsight {
    pub id: String,
    pub pattern: String,
    pub solution_summary: String,
    pub applicability_tags: Vec<String>,
    pub confidence: f64,
    pub created_ms: u64,
}

#[derive(Debug, Default)]
pub struct KnowledgeBus {
    insights: Vec<KnowledgeInsight>,
}

impl KnowledgeBus {
    pub fn add_insight(&mut self, insight: KnowledgeInsight) {
        self.insights.push(insight);
    }

    pub fn find_matching(&self, tags: &[String]) -> Vec<&KnowledgeInsight> {
        self.insights
            .iter()
            .filter(|i| tags.iter().any(|t| i.applicability_tags.contains(t)))
            .collect()
    }

    pub fn snapshot(&self) -> Vec<KnowledgeInsight> {
        self.insights.clone()
    }
}

// ---------------------------------------------------------------------------
// CapabilityBus — the top-level scheduling coordinator
// ---------------------------------------------------------------------------

/// Bus profile for governance.status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityBusProfile {
    pub enabled: bool,
    pub routing_count: u64,
    pub learning_events_count: usize,
    pub reputation_agents_count: usize,
    pub capability_graph_agents: usize,
    pub knowledge_insights_count: usize,
    pub last_route_duration_ms: u64,
}

impl Default for CapabilityBusProfile {
    fn default() -> Self {
        Self {
            enabled: true,
            routing_count: 0,
            learning_events_count: 0,
            reputation_agents_count: 0,
            capability_graph_agents: 0,
            knowledge_insights_count: 0,
            last_route_duration_ms: 0,
        }
    }
}

/// CapabilityBus aggregates all sub-bus references and orchestrates the
/// 5-stage lifecycle: sense → decide → act → feedback → evolve.
pub struct CapabilityBus {
    /// HarnessBus — strategy engine (pre-route / pre-tool / post-exec)
    pub harness: Arc<HarnessBus>,

    /// Workflow learning bus — historical execution outcomes
    pub learning_bus: Arc<Mutex<WorkflowLearningBus>>,

    /// Knowledge bus — reusable solution insights
    pub knowledge_bus: Arc<Mutex<KnowledgeBus>>,

    /// Reputation store — per-agent EMA reliability scores
    pub reputation: Arc<Mutex<ReputationStore>>,

    /// Capability graph — agent capability declarations and handoff edges
    pub capability_graph: Arc<Mutex<CapabilityGraph>>,

    /// Q-Learning agent — reinforcement learning for routing decisions
    pub q_learning: Arc<Mutex<QLearningAgent>>,

    /// Experience knowledge base — success/failure case library
    pub experience: Arc<Mutex<ExperienceKnowledgeBase>>,

    /// Reward function — calculates reward from execution metrics
    pub reward_fn: Arc<Mutex<RewardFunction>>,

    /// Bus event history (for observability / tracing)
    pub event_history: Arc<Mutex<VecDeque<BusEvent>>>,

    /// Capability bus profile (for governance.status)
    pub profile: Arc<Mutex<CapabilityBusProfile>>,

    max_event_history: usize,
}

impl CapabilityBus {
    pub fn new(
        harness: Arc<HarnessBus>,
        reputation: ReputationStore,
        capability_graph: CapabilityGraph,
        q_learning: QLearningAgent,
        experience: ExperienceKnowledgeBase,
        reward_fn: RewardFunction,
    ) -> Self {
        Self {
            harness,
            learning_bus: Arc::new(Mutex::new(WorkflowLearningBus::new(1000))),
            knowledge_bus: Arc::new(Mutex::new(KnowledgeBus::default())),
            reputation: Arc::new(Mutex::new(reputation)),
            capability_graph: Arc::new(Mutex::new(capability_graph)),
            q_learning: Arc::new(Mutex::new(q_learning)),
            experience: Arc::new(Mutex::new(experience)),
            reward_fn: Arc::new(Mutex::new(reward_fn)),
            event_history: Arc::new(Mutex::new(VecDeque::new())),
            profile: Arc::new(Mutex::new(CapabilityBusProfile::default())),
            max_event_history: 500,
        }
    }

    pub fn new_default(harness: Arc<HarnessBus>) -> Self {
        Self::new(
            harness,
            ReputationStore::new(crate::intelligence::reputation::ReputationConfig::default()),
            CapabilityGraph::new(),
            QLearningAgent::default(),
            ExperienceKnowledgeBase::default(),
            RewardFunction::default(),
        )
    }

    // ------------------------------------------------------------------
    // Event recording
    // ------------------------------------------------------------------

    pub fn record_event(
        &self,
        stage: &str,
        agent: Option<String>,
        task_id: Option<String>,
        outcome: &str,
        detail: Value,
    ) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        if let Ok(mut history) = self.event_history.lock() {
            history.push_back(BusEvent {
                timestamp_ms: now_ms,
                stage: stage.to_string(),
                agent,
                task_id,
                outcome: outcome.to_string(),
                detail,
            });
            while history.len() > self.max_event_history {
                history.pop_front();
            }
        }
    }

    // ------------------------------------------------------------------
    // Stage 1: Sensing — gather input from sub-buses
    // ------------------------------------------------------------------

    pub fn sense(&self, _task: &TaskContext) -> SensingOutput {
        let cap_agents = self
            .capability_graph
            .lock()
            .map(|g| g.total_agents())
            .unwrap_or(0);
        let rep_snapshot = self
            .reputation
            .lock()
            .map(|r| r.snapshot())
            .unwrap_or_default();
        let _learning_rates = self
            .learning_bus
            .lock()
            .map(|lb| {
                let agents: Vec<String> = lb.snapshot().iter().map(|e| e.agent.clone()).collect();
                agents
            })
            .unwrap_or_default();

        SensingOutput {
            capability_agent_count: cap_agents,
            reputation_snapshot: rep_snapshot,
            recent_agents: _learning_rates,
        }
    }

    // ------------------------------------------------------------------
    // Stage 2: Decision — select agent / strategy
    // ------------------------------------------------------------------

    pub fn decide(&self, task: &TaskContext, sensing: &SensingOutput) -> DecisionOutput {
        let start = Instant::now();

        // Step A: HarnessBus policy evaluation (compliance gate)
        let verdict = self.harness.evaluate(task);
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
                };
            }
            _ => {}
        }

        // Step B: pick best agent from capability graph + reputation
        let candidate_agents = self
            .capability_graph
            .lock()
            .map(|g| {
                // Use agents_with_tag for broad matching, or fallback to all
                let mut candidates: Vec<String> = g
                    .agents_with_tag("general")
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect();
                if candidates.is_empty() {
                    // No tagged agents; use all registered agents
                    let all: Vec<String> = g
                        .all_capability_names()
                        .into_iter()
                        .map(|s| s.to_string())
                        .collect();
                    candidates = all;
                }
                candidates
            })
            .unwrap_or_default();

        let selected_agent = self.select_best_agent(&candidate_agents, sensing);

        // Step C: build agent execution policy from HarnessBus
        let agent_policy = Some(self.harness.get_agent_policy(
            selected_agent.as_deref().unwrap_or("unknown"),
            &format!("{:?}", task.task_type),
        ));

        let confidence = sensing
            .reputation_snapshot
            .iter()
            .find(|r| Some(r.agent.as_str()) == selected_agent.as_deref())
            .map(|r| r.score)
            .unwrap_or(0.5);

        self.record_event(
            "decision",
            selected_agent.clone(),
            None,
            "success",
            serde_json::json!({"confidence": confidence}),
        );

        if let Ok(mut p) = self.profile.lock() {
            p.routing_count = p.routing_count.saturating_add(1);
            p.last_route_duration_ms = start.elapsed().as_millis() as u64;
        }

        DecisionOutput {
            verdict,
            selected_agent,
            agent_policy,
            confidence,
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }

    fn select_best_agent(&self, candidates: &[String], sensing: &SensingOutput) -> Option<String> {
        if candidates.is_empty() {
            return None;
        }
        // Score each candidate by reputation (higher is better)
        let mut scored: Vec<(&String, f64)> = candidates
            .iter()
            .map(|name| {
                let score = sensing
                    .reputation_snapshot
                    .iter()
                    .find(|r| r.agent == *name)
                    .map(|r| r.score)
                    .unwrap_or(1.0);
                (name, score)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.first().map(|(name, _)| (*name).clone())
    }

    // ------------------------------------------------------------------
    // Stage 4: Feedback — write results to sub-buses
    // ------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn feedback(
        &self,
        agent: &str,
        task_type: &str,
        task_id: &str,
        success: bool,
        duration_ms: u64,
        token_cost: u64,
        quality_score: f64,
    ) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // 1. Write to learning bus
        if let Ok(mut lb) = self.learning_bus.lock() {
            lb.push(WorkflowLearningEvent {
                task_type: task_type.to_string(),
                agent: agent.to_string(),
                success,
                duration_ms,
                token_cost,
                quality_score,
                timestamp_ms: now_ms,
            });
        }

        // 2. Write to reputation store
        if let Ok(mut rep) = self.reputation.lock() {
            rep.record_outcome(agent, success);
        }

        // 3. Record event
        let outcome = if success { "success" } else { "failure" };
        self.record_event(
            "feedback",
            Some(agent.to_string()),
            Some(task_id.to_string()),
            outcome,
            serde_json::json!({
                "duration_ms": duration_ms,
                "token_cost": token_cost,
                "quality_score": quality_score,
            }),
        );
    }

    // ------------------------------------------------------------------
    // Stage 5: Evolution — reinforcement learning update
    // ------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn evolve(
        &self,
        state: &(String, String),
        action: &str,
        next_state: &(String, String),
        token_cost: u64,
        success: bool,
        quality_score: f64,
    ) {
        // Build metrics for reward calculation
        let metrics = RlTaskExecutionMetrics {
            tokens_used: token_cost,
            success,
            quality_score,
            duration_ms: 0,
        };

        // Calculate reward
        let reward = self
            .reward_fn
            .lock()
            .map(|rf| rf.calculate(&metrics))
            .unwrap_or(0.0);

        // Update Q table
        if let Ok(mut ql) = self.q_learning.lock() {
            ql.update(state, action, reward, next_state);
        }

        // Record success/failure knowledge
        if success {
            let _ = self.experience.lock().map(|mut exp| {
                exp.add_success_case(SuccessCase {
                    objective: format!("state_{:?}", state),
                    strategy: format!("action_{}", action),
                    confidence: quality_score,
                })
            });
        }

        self.record_event(
            "evolve",
            None,
            None,
            "success",
            serde_json::json!({"reward": reward, "state": state, "action": action}),
        );
    }

    // ------------------------------------------------------------------
    // Observability helpers
    // ------------------------------------------------------------------

    pub fn snapshot_events(&self) -> Vec<BusEvent> {
        self.event_history
            .lock()
            .map(|h| h.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn capability_bus_profile(&self) -> CapabilityBusProfile {
        if let Ok(mut p) = self.profile.lock() {
            p.learning_events_count = self.learning_bus.lock().map(|lb| lb.len()).unwrap_or(0);
            p.reputation_agents_count = self
                .reputation
                .lock()
                .map(|r| r.tracked_agent_count())
                .unwrap_or(0);
            p.capability_graph_agents = self
                .capability_graph
                .lock()
                .map(|g| g.total_agents())
                .unwrap_or(0);
            p.knowledge_insights_count = self
                .knowledge_bus
                .lock()
                .map(|kb| kb.snapshot().len())
                .unwrap_or(0);
            p.clone()
        } else {
            CapabilityBusProfile::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Stage output types
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct SensingOutput {
    pub capability_agent_count: usize,
    pub reputation_snapshot: Vec<crate::intelligence::reputation::ReputationRecord>,
    pub recent_agents: Vec<String>,
}

#[derive(Debug)]
pub struct DecisionOutput {
    pub verdict: PolicyVerdict,
    pub selected_agent: Option<String>,
    pub agent_policy: Option<AgentExecutionPolicy>,
    pub confidence: f64,
    pub duration_ms: u64,
}
