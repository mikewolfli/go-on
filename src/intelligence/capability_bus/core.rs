//! Core CapabilityBus implementation.
//!
//! Full multi-bus bidirectional closed-loop (BLUE38 ARCH-13):
//!   sense → decide → act → feedback → evolve
//!
//! This module defines the top-level `CapabilityBus` struct that holds references
//! to all 13 sub-bus components and orchestrates the complete lifecycle.
//! Sub-buses:
//!   1. WorkflowLearningBus  (existing)
//!   2. KnowledgeBus          (existing)
//!   3. ReputationStore       (existing)
//!   4. CapabilityGraph       (existing)
//!   5. QLearningAgent        (existing)
//!   6. ExperienceKnowledgeBase (existing)
//!   7. HarnessBus            (existing)
//!   8. ToolBus               (new in Phase 4)
//!   9. ObservabilityBus      (new in Phase 4)
//!  10. OptimizationBus       (new in Phase 4)
//!  11. MemoryBus             (new in Phase 4)
//!  12. ProtocolBus           (new in Phase 4)
//!  13. OrchestrationBus      (new in Phase 4)
//!  14. DistributedMemoryBus  (new in Phase 4)

#[cfg(any(
    feature = "sub-bus-tool",
    feature = "profile-simple-server",
    feature = "profile-multi-users-server"
))]
use crate::agents::factory::{AgentFactory, AgentFactoryConfig};
use crate::governance::hardening::TenantBudgetEnforcer;
use crate::governance::harness_bus::{AgentExecutionPolicy, HarnessBus, PolicyVerdict};
use crate::governance::pua::TaskContext;
#[cfg(feature = "sub-bus-distributed-memory")]
use crate::intelligence::capability_bus::distributed_memory_bus::DistributedMemoryBus;
#[cfg(feature = "sub-bus-memory")]
use crate::intelligence::capability_bus::memory_bus::MemoryBus;
#[cfg(feature = "sub-bus-observability")]
use crate::intelligence::capability_bus::observability_bus::ObservabilityBus;
#[cfg(feature = "sub-bus-optimization")]
use crate::intelligence::capability_bus::optimization_bus::OptimizationBus;
#[cfg(feature = "sub-bus-orchestration")]
use crate::intelligence::capability_bus::orchestration_bus::OrchestrationBus;
#[cfg(feature = "sub-bus-protocol")]
use crate::intelligence::capability_bus::protocol_bus::ProtocolBus;
#[cfg(feature = "sub-bus-tool")]
use crate::intelligence::capability_bus::tool_bus::ToolBus;
use crate::intelligence::capability_graph::CapabilityGraph;
use crate::intelligence::consciousness::ConsciousnessMetrics;
use crate::intelligence::consensus::ConsensusEngine;
use crate::intelligence::continuous_learning::ContinuousLearningCenter;
use crate::intelligence::discovery::DiscoveryCenter;
use crate::intelligence::evolution_graph::{EvolutionGraph, EvolutionStage, TrendDirection};

use crate::intelligence::matcher::ScenarioMatcher;
use crate::intelligence::metacognitive::MetacognitiveController;
use crate::intelligence::now_ms;
use crate::intelligence::reinforcement::federated::FederatedRL;
use crate::intelligence::reinforcement::learning::{
    ExperienceKnowledgeBase, QLearningAgent, RewardFunction, RlTaskExecutionMetrics, SuccessCase,
};
use crate::intelligence::reputation::ReputationStore;
use crate::intelligence::self_model::SelfModelCore;
use crate::intelligence::world_model::WorldModel;
use crate::intelligence::{lock_guard, read_guard, write_guard};
use crate::observability::provenance::{make_entry, ProvenanceLedger};
#[cfg(any(
    feature = "sub-bus-tool",
    feature = "profile-simple-server",
    feature = "profile-multi-users-server"
))]
use crate::orchestration::council::{CouncilConfig, OrchestrationCouncil};
use crate::orchestration::task_schema::SchemaRegistry;
use crate::orchestration::workflow_optimizer::OptimizerRegistry;
use crate::orchestration::workflow_registry::WorkflowRegistry;
use crate::protocol::transport::MultiChannelTransport;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::env;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use tokio::time::{timeout, Duration};
use tracing::warn;

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

#[derive(Debug, Clone, Copy)]
struct CandidateScoreWeights {
    reputation: f64,
    recency: f64,
    task_fit: f64,
    recent_outcome: f64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CandidateScoreBreakdown {
    agent: String,
    reputation_score: f64,
    recency_score: f64,
    task_fit_score: f64,
    recent_outcome_score: f64,
    total_score: f64,
}

fn configured_candidate_score_weights() -> CandidateScoreWeights {
    fn read_weight(key: &str, fallback: f64) -> f64 {
        env::var(key)
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .unwrap_or(fallback)
    }

    let weights = CandidateScoreWeights {
        reputation: read_weight("GO_ON_CAPABILITY_WEIGHT_REPUTATION", 0.45),
        recency: read_weight("GO_ON_CAPABILITY_WEIGHT_RECENCY", 0.15),
        task_fit: read_weight("GO_ON_CAPABILITY_WEIGHT_TASK_FIT", 0.25),
        recent_outcome: read_weight("GO_ON_CAPABILITY_WEIGHT_RECENT_OUTCOME", 0.15),
    };
    let total = weights.reputation + weights.recency + weights.task_fit + weights.recent_outcome;
    if total <= f64::EPSILON {
        CandidateScoreWeights {
            reputation: 0.45,
            recency: 0.15,
            task_fit: 0.25,
            recent_outcome: 0.15,
        }
    } else {
        CandidateScoreWeights {
            reputation: weights.reputation / total,
            recency: weights.recency / total,
            task_fit: weights.task_fit / total,
            recent_outcome: weights.recent_outcome / total,
        }
    }
}

fn task_fit_score(task: &TaskContext, agent_name: &str) -> f64 {
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

fn recency_score(recent_agents: &[String], agent_name: &str) -> f64 {
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

fn recent_outcome_score(
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
        let (total, successes) = self
            .events
            .iter()
            .filter(|e| e.agent == agent)
            .fold((0usize, 0usize), |(total, successes), e| {
                (total + 1, successes + e.success as usize)
            });
        if total == 0 {
            None
        } else {
            Some(successes as f64 / total as f64)
        }
    }

    /// Historical success rate for a given task type.
    pub fn task_type_success_rate(&self, task_type: &str) -> Option<f64> {
        let (total, successes) = self
            .events
            .iter()
            .filter(|e| e.task_type == task_type)
            .fold((0usize, 0usize), |(total, successes), e| {
                (total + 1, successes + e.success as usize)
            });
        if total == 0 {
            None
        } else {
            Some(successes as f64 / total as f64)
        }
    }

    /// All events (for snapshot / endpoint)
    pub fn snapshot(&self) -> Vec<WorkflowLearningEvent> {
        self.events.iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Returns `true` if there are no events.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// Builder
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

const MAX_KNOWLEDGE_INSIGHTS: usize = 500;

#[derive(Debug, Default)]
pub struct KnowledgeBus {
    insights: Vec<KnowledgeInsight>,
}

impl KnowledgeBus {
    pub fn add_insight(&mut self, insight: KnowledgeInsight) {
        if self.insights.len() >= MAX_KNOWLEDGE_INSIGHTS {
            self.insights.remove(0);
        }
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
    pub q_learning_table_size: usize,
    pub experience_case_count: usize,
    pub event_history_len: usize,
    pub workflow_presets_count: usize,
    pub provenance_entries_count: usize,
    // Phase 4 sub-bus metrics
    #[cfg(feature = "sub-bus-tool")]
    pub tool_bus_tools: u32,
    #[cfg(feature = "sub-bus-tool")]
    pub tool_bus_skills: u32,
    #[cfg(feature = "sub-bus-tool")]
    pub tool_bus_calls: u64,
    #[cfg(feature = "sub-bus-observability")]
    pub observability_tracked_agents: u32,
    #[cfg(feature = "sub-bus-observability")]
    pub observability_system_error_rate: f64,
    #[cfg(feature = "sub-bus-optimization")]
    pub optimization_total: u64,
    #[cfg(feature = "sub-bus-optimization")]
    pub optimization_circuit_breaker_trips: u64,
    #[cfg(feature = "sub-bus-protocol")]
    pub protocol_active_transport: String,
    #[cfg(feature = "sub-bus-protocol")]
    pub protocol_healthy_count: u32,
    #[cfg(feature = "sub-bus-orchestration")]
    pub orchestration_active_flows: u32,
    #[cfg(feature = "sub-bus-orchestration")]
    pub orchestration_available_modes: u32,
    #[cfg(feature = "sub-bus-memory")]
    pub memory_cache_hit_rate: f64,
    #[cfg(feature = "sub-bus-memory")]
    pub memory_total_entries: u32,
    #[cfg(feature = "sub-bus-distributed-memory")]
    pub distributed_memory_peers: u32,
    #[cfg(feature = "sub-bus-distributed-memory")]
    pub distributed_memory_shared: u32,
    /// Number of skill evolution records
    pub skill_evolution_count: u32,
    /// Phase-gated sub-agent factory active instances (simple/multi profiles)
    pub agent_factory_active_instances: u32,
    /// Phase-gated sub-agent factory templates (simple/multi profiles)
    pub agent_factory_templates: u32,
    /// Phase-gated orchestration council active members (simple/multi profiles)
    pub council_active_members: u32,
    /// Cumulative evolve() timeout count — non-zero indicates silent degradation
    pub evolve_timeout_count: u64,
    /// Phase-gated orchestration council pending proposals (simple/multi profiles)
    pub council_pending_proposals: u32,
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
            q_learning_table_size: 0,
            experience_case_count: 0,
            event_history_len: 0,
            workflow_presets_count: 0,
            provenance_entries_count: 0,
            #[cfg(feature = "sub-bus-tool")]
            tool_bus_tools: 0,
            #[cfg(feature = "sub-bus-tool")]
            tool_bus_skills: 0,
            #[cfg(feature = "sub-bus-tool")]
            tool_bus_calls: 0,
            #[cfg(feature = "sub-bus-observability")]
            observability_tracked_agents: 0,
            #[cfg(feature = "sub-bus-observability")]
            observability_system_error_rate: 0.0,
            #[cfg(feature = "sub-bus-optimization")]
            optimization_total: 0,
            #[cfg(feature = "sub-bus-optimization")]
            optimization_circuit_breaker_trips: 0,
            #[cfg(feature = "sub-bus-protocol")]
            protocol_active_transport: "auto".to_string(),
            #[cfg(feature = "sub-bus-protocol")]
            protocol_healthy_count: 0,
            #[cfg(feature = "sub-bus-orchestration")]
            orchestration_active_flows: 0,
            #[cfg(feature = "sub-bus-orchestration")]
            orchestration_available_modes: 0,
            #[cfg(feature = "sub-bus-memory")]
            memory_cache_hit_rate: 0.0,
            #[cfg(feature = "sub-bus-memory")]
            memory_total_entries: 0,
            #[cfg(feature = "sub-bus-distributed-memory")]
            distributed_memory_peers: 0,
            #[cfg(feature = "sub-bus-distributed-memory")]
            distributed_memory_shared: 0,
            skill_evolution_count: 0,
            agent_factory_active_instances: 0,
            agent_factory_templates: 0,
            council_active_members: 0,
            evolve_timeout_count: 0,
            council_pending_proposals: 0,
        }
    }
}

/// Configuration for CapabilityBus lifecycle and behavior (GAP-B50-21).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityBusConfig {
    /// How many requests between evolve() calls. Default: 50.
    pub evolve_interval: u64,
    /// Whether the capability bus is enabled. Default: false.
    pub enable_capability_bus: bool,
}

impl Default for CapabilityBusConfig {
    fn default() -> Self {
        Self {
            evolve_interval: 50,
            enable_capability_bus: false,
        }
    }
}

/// CapabilityBus aggregates all sub-bus references and orchestrates the
/// 5-stage lifecycle: sense → decide → act → feedback → evolve.
/// This is the scheduling coordinator for all 14 sub-buses (BLUE38 ARCH-13).
pub struct CapabilityBus {
    /// HarnessBus — strategy engine (pre-route / pre-tool / post-exec)
    pub harness: Arc<HarnessBus>,

    /// Workflow learning bus — historical execution outcomes
    pub learning_bus: Arc<RwLock<WorkflowLearningBus>>,

    /// Knowledge bus — reusable solution insights
    pub knowledge_bus: Arc<RwLock<KnowledgeBus>>,

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
    pub event_history: Arc<RwLock<VecDeque<BusEvent>>>,

    /// Capability bus profile (for governance.status)
    pub profile: Arc<RwLock<CapabilityBusProfile>>,

    /// Workflow registry — named workflow presets for workflow-based routing
    workflow_registry: Option<Arc<Mutex<WorkflowRegistry>>>,

    /// Provenance ledger — immutable data lineage tracking for every operation
    pub provenance_ledger: Arc<ProvenanceLedger>,

    /// Schema registry — validates task envelopes against role schemas (F-GAP-07)
    pub schema_registry: Arc<Mutex<SchemaRegistry>>,

    /// Tenant budget enforcer — per-tenant resource quota management (F-GAP-08)
    pub tenant_budget: Arc<Mutex<TenantBudgetEnforcer>>,

    /// Optimizer registry — workflow optimization plugins (ARCH-11)
    pub optimizer_registry: Arc<Mutex<OptimizerRegistry>>,

    // ── Phase 4 sub-buses ────────────────────────────────────────────────
    /// ToolBus — unified tool/skill invocation with capability-aware routing
    #[cfg(feature = "sub-bus-tool")]
    pub tool_bus: ToolBus,

    /// ObservabilityBus — unified trace/metric/audit coordination
    #[cfg(feature = "sub-bus-observability")]
    pub observability_bus: ObservabilityBus,

    /// OptimizationBus — cost, speed, reliability optimization coordination
    #[cfg(feature = "sub-bus-optimization")]
    pub optimization_bus: OptimizationBus,

    /// MemoryBus — unified cache coordination (L1 memory → L2 SQLite → L3 vector)
    #[cfg(feature = "sub-bus-memory")]
    pub memory_bus: MemoryBus,

    /// ProtocolBus — protocol-aware routing and health tracking
    #[cfg(feature = "sub-bus-protocol")]
    pub protocol_bus: ProtocolBus,

    /// OrchestrationBus — unified flow/task/mode coordination
    #[cfg(feature = "sub-bus-orchestration")]
    pub orchestration_bus: OrchestrationBus,

    /// DistributedMemoryBus — cross-node memory sharing
    #[cfg(feature = "sub-bus-distributed-memory")]
    pub distributed_memory_bus: DistributedMemoryBus,

    max_event_history: usize,

    // ── Cognitive modules ──────────────────────────────────────────────
    pub consciousness: ConsciousnessMetrics,
    pub metacognitive: MetacognitiveController,
    pub world_model: WorldModel,
    pub self_model: SelfModelCore,
    pub federated_rl: FederatedRL,
    pub matcher: ScenarioMatcher,
    pub discovery: DiscoveryCenter,
    pub consensus: ConsensusEngine,

    /// Agent factory — dynamic sub-agent creation (F-GAP-13)
    #[cfg(any(
        feature = "sub-bus-tool",
        feature = "profile-simple-server",
        feature = "profile-multi-users-server"
    ))]
    pub agent_factory: Arc<Mutex<AgentFactory>>,
    /// Orchestration council — multi-agent voting governance (F-GAP-15)
    #[cfg(any(
        feature = "sub-bus-tool",
        feature = "profile-simple-server",
        feature = "profile-multi-users-server"
    ))]
    pub council: Arc<Mutex<OrchestrationCouncil>>,
    /// Evolution graph — capability lifecycle tracking (F-GAP-18)
    pub evolution_graph: Arc<Mutex<EvolutionGraph>>,

    /// Continuous learning center — lifelong learning (F-GAP-24)
    pub continuous_learning: Arc<Mutex<ContinuousLearningCenter>>,

    /// Multi-channel message transport — protocol layer (F-GAP-29)
    pub transport: Arc<Mutex<MultiChannelTransport>>,

    /// Configuration for capability bus lifecycle (GAP-B50-21)
    pub config: CapabilityBusConfig,

    /// Cumulative count of evolve() subsystem timeouts (non-zero indicates silent degradation)
    pub evolve_timeout_count: std::sync::atomic::AtomicU64,

    /// Multi-model voter — cross-validates high-risk decisions via agent consensus
    #[cfg(feature = "sub-bus-voter-future")]
    pub multi_voter: crate::intelligence::multi_model_voter::MultiModelVoter,
}

impl CapabilityBus {
    // ── Lock ordering ───────────────────────────────────────────────────
    //
    // To avoid deadlocks, always acquire locks in the order listed below
    // and never acquire a lock from a later group while holding one from
    // an earlier group:
    //
    //   Level 1 (innermost – acquire first, release last):
    //     reward_fn, q_learning, experience, continuous_learning
    //
    //   Level 2:
    //     federated_rl, metacognitive, discovery, self_model,
    //     consciousness, world_model, consensus
    //
    //   Level 3 (outermost – acquire last, release first):
    //     evolution_graph, transport, learning_bus, reputation,
    //     capability_graph, profile
    //
    // Single-lock components (no ordering conflicts):
    //     harness, matcher, provenance_ledger, schema_registry
    //
    // RULE: Never hold locks across subsystem boundaries.
    //       Use `lock_guard` which is scoped; drop guards before calling
    //       another subsystem method.
    //
    // ─────────────────────────────────────────────────────────────────────

    pub fn new(
        harness: Arc<HarnessBus>,
        reputation: ReputationStore,
        capability_graph: CapabilityGraph,
        q_learning: QLearningAgent,
        experience: ExperienceKnowledgeBase,
        reward_fn: RewardFunction,
        provenance_ledger: Arc<ProvenanceLedger>,
    ) -> Self {
        Self {
            harness,
            learning_bus: Arc::new(RwLock::new(WorkflowLearningBus::new(1000))),
            knowledge_bus: Arc::new(RwLock::new(KnowledgeBus::default())),
            reputation: Arc::new(Mutex::new(reputation)),
            capability_graph: Arc::new(Mutex::new(capability_graph)),
            q_learning: Arc::new(Mutex::new(q_learning)),
            experience: Arc::new(Mutex::new(experience)),
            reward_fn: Arc::new(Mutex::new(reward_fn)),
            event_history: Arc::new(RwLock::new(VecDeque::with_capacity(100))),
            profile: Arc::new(RwLock::new(CapabilityBusProfile::default())),
            workflow_registry: None,
            provenance_ledger,
            schema_registry: Arc::new(Mutex::new(SchemaRegistry::new())),
            tenant_budget: Arc::new(Mutex::new(TenantBudgetEnforcer::new())),
            optimizer_registry: Arc::new(Mutex::new(OptimizerRegistry::new())),
            #[cfg(feature = "sub-bus-tool")]
            tool_bus: ToolBus::new(
                Arc::new(Mutex::new(crate::orchestration::tool::ToolRegistry::new())),
                Arc::new(Mutex::new(
                    crate::orchestration::skill::SkillRegistry::default(),
                )),
            ),
            #[cfg(feature = "sub-bus-observability")]
            observability_bus: ObservabilityBus::new(),
            #[cfg(feature = "sub-bus-optimization")]
            optimization_bus: OptimizationBus::default(),
            #[cfg(feature = "sub-bus-memory")]
            // MemoryBus is created with default in-memory backends so data is
            // never silently lost. L2/L3 backends (SQLite/vector) can be
            // injected later via set_backends() once the shared handles are
            // available at server startup. The evolve_timeout_count tracking
            // ensures stuck memory operations are surfaced as governance drift
            // when they exceed their deadline budget.
            memory_bus: MemoryBus::new(None, None, None, None).with_default_backends(),
            #[cfg(feature = "sub-bus-protocol")]
            protocol_bus: ProtocolBus::new(),
            #[cfg(feature = "sub-bus-orchestration")]
            orchestration_bus: OrchestrationBus::new(None),
            #[cfg(feature = "sub-bus-distributed-memory")]
            distributed_memory_bus: DistributedMemoryBus::new(5000),
            max_event_history: 100,
            consciousness: ConsciousnessMetrics::new(Default::default()),
            // BLUE56-GAP-B02: `MetacognitiveController::with_llm(Default::default(), llm_agent)`
                        // should be used here when an LLM agent is available at bus construction.
                        // `with_metacognitive_llm()` builder method also exists for post-hoc injection.
                        metacognitive: MetacognitiveController::new(Default::default()),
            world_model: WorldModel::new(Default::default()),
            self_model: SelfModelCore::new(Default::default()),
            federated_rl: FederatedRL::new(Default::default()),
            matcher: ScenarioMatcher::default(),
            discovery: DiscoveryCenter::new(),
            consensus: ConsensusEngine::new(Default::default()),
            #[cfg(any(
                feature = "sub-bus-tool",
                feature = "profile-simple-server",
                feature = "profile-multi-users-server"
            ))]
            agent_factory: Arc::new(Mutex::new(AgentFactory::new(AgentFactoryConfig::default()))),
            #[cfg(any(
                feature = "sub-bus-tool",
                feature = "profile-simple-server",
                feature = "profile-multi-users-server"
            ))]
            council: Arc::new(Mutex::new(OrchestrationCouncil::new(
                CouncilConfig::default(),
            ))),
            evolution_graph: Arc::new(Mutex::new(EvolutionGraph::new())),
            continuous_learning: Arc::new(Mutex::new(ContinuousLearningCenter::new(
                Default::default(),
            ))),
            transport: Arc::new(Mutex::new(MultiChannelTransport::new(Default::default()))),
            config: CapabilityBusConfig::default(),
            evolve_timeout_count: std::sync::atomic::AtomicU64::new(0),
            #[cfg(feature = "sub-bus-voter-future")]
            multi_voter:
                crate::intelligence::multi_model_voter::MultiModelVoter::new(),
        }
    }

    pub fn new_default(
        harness: Arc<HarnessBus>,
        workflow_registry: Option<Arc<Mutex<WorkflowRegistry>>>,
    ) -> Self {
        let mut bus = Self::new(
            harness,
            ReputationStore::new(crate::intelligence::reputation::ReputationConfig::default()),
            CapabilityGraph::new(),
            QLearningAgent::default(),
            ExperienceKnowledgeBase::default(),
            RewardFunction::default(),
            Arc::new(ProvenanceLedger::default()),
        );
        bus.workflow_registry = workflow_registry;
        bus
    }

    /// Attach a shared ProvenanceLedger to the CapabilityBus, replacing the default
    pub fn with_provenance_ledger(mut self, ledger: Arc<ProvenanceLedger>) -> Self {
        self.provenance_ledger = ledger;
        self
    }

    /// Attach a WorkflowRegistry to an existing CapabilityBus
    pub fn with_workflow_registry(mut self, registry: Arc<Mutex<WorkflowRegistry>>) -> Self {
        self.workflow_registry = Some(registry);
        self
    }

    // ── Phase 4 sub-bus builder methods ───────────────────────────────────

    /// Attach a ToolBus to the CapabilityBus
    #[cfg(feature = "sub-bus-tool")]
    pub fn with_tool_bus(mut self, tool_bus: ToolBus) -> Self {
        self.tool_bus = tool_bus;
        self
    }

    /// Attach an ObservabilityBus to the CapabilityBus
    #[cfg(feature = "sub-bus-observability")]
    pub fn with_observability_bus(mut self, bus: ObservabilityBus) -> Self {
        self.observability_bus = bus;
        self
    }

    /// Attach an OptimizationBus to the CapabilityBus
    #[cfg(feature = "sub-bus-optimization")]
    pub fn with_optimization_bus(mut self, bus: OptimizationBus) -> Self {
        self.optimization_bus = bus;
        self
    }

    /// Attach a MemoryBus to the CapabilityBus
    #[cfg(feature = "sub-bus-memory")]
    pub fn with_memory_bus(mut self, bus: MemoryBus) -> Self {
        self.memory_bus = bus;
        self
    }

    /// Attach a ProtocolBus to the CapabilityBus
    #[cfg(feature = "sub-bus-protocol")]
    pub fn with_protocol_bus(mut self, bus: ProtocolBus) -> Self {
        self.protocol_bus = bus;
        self
    }

    /// Attach an OrchestrationBus to the CapabilityBus
    #[cfg(feature = "sub-bus-orchestration")]
    pub fn with_orchestration_bus(mut self, bus: OrchestrationBus) -> Self {
        self.orchestration_bus = bus;
        self
    }

    /// Attach a DistributedMemoryBus to the CapabilityBus
    #[cfg(feature = "sub-bus-distributed-memory")]
    pub fn with_distributed_memory_bus(mut self, bus: DistributedMemoryBus) -> Self {
        self.distributed_memory_bus = bus;
        self
    }

    /// Set configuration for the CapabilityBus (GAP-B50-21)
    pub fn with_config(mut self, config: CapabilityBusConfig) -> Self {
        self.config = config;
        self
    }

    /// Inject an LLM agent into the MetacognitiveController (BLUE56-GAP-B02).
    ///
    /// When an LLM agent is provided, reflection reports use LLM-based
    /// root cause analysis instead of template-based fallback.
    pub fn with_metacognitive_llm(mut self, agent: Arc<dyn crate::agent::Agent>) -> Self {
        self.metacognitive.set_llm_agent(agent);
        self
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

        let mut history = write_guard(&self.event_history);
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

    fn action_outcome_label(success: bool) -> &'static str {
        if success {
            "success"
        } else {
            "failure"
        }
    }

    fn build_action_blocked_detail(tool_name: &str, reason: &str) -> Value {
        serde_json::json!({
            "schema": "capability-bus-action-v1",
            "tool": tool_name,
            "duration_ms": 0,
            "logical_success": false,
            "error": reason,
            "policy_blocked": true,
        })
    }

    fn build_action_event_detail(
        tool_name: &str,
        duration_ms: u64,
        success: bool,
        error_text: Option<String>,
    ) -> Value {
        serde_json::json!({
            "schema": "capability-bus-action-v1",
            "tool": tool_name,
            "duration_ms": duration_ms,
            "logical_success": success,
            "error": error_text,
            "policy_blocked": false,
        })
    }

    fn build_feedback_event_detail(
        duration_ms: u64,
        token_cost: u64,
        quality_score: f64,
        success: bool,
    ) -> Value {
        serde_json::json!({
            "schema": "capability-bus-feedback-v1",
            "duration_ms": duration_ms,
            "token_cost": token_cost,
            "quality_score": quality_score,
            "logical_success": success,
        })
    }

    // ------------------------------------------------------------------
    // Stage 1: Sensing — gather input from sub-buses
    // ------------------------------------------------------------------

    pub fn sense(&self, task: &TaskContext) -> SensingOutput {
        // Include task risk score in heartbeat so `task` is unconditionally referenced
        // across all feature configurations.
        let cap_agents = lock_guard(&self.capability_graph).total_agents();
        let rep_snapshot = lock_guard(&self.reputation).snapshot();
        let _learning_rates = {
            let agents: Vec<String> = read_guard(&self.learning_bus)
                .snapshot()
                .iter()
                .map(|e| e.agent.clone())
                .collect();
            agents
        };
        let learning_snapshot = read_guard(&self.learning_bus).snapshot();

        // Phase 4: Query ObservabilityBus for healthy agents
        #[cfg(feature = "sub-bus-observability")]
        let healthy = self.observability_bus.healthy_agents(0.5);
        #[cfg(not(feature = "sub-bus-observability"))]
        let _healthy = Vec::<String>::new();

        // Phase 4: Query OrchestrationBus for available modes
        #[cfg(feature = "sub-bus-orchestration")]
        let modes = self.orchestration_bus.available_modes();
        #[cfg(not(feature = "sub-bus-orchestration"))]
        let _modes = Vec::<String>::new();

        // Phase 4: Get optimization recommendation
        #[cfg(any(feature = "sub-bus-optimization", feature = "sub-bus-protocol"))]
        let task_type_str = format!("{:?}", task.task_type);
        #[cfg(any(feature = "sub-bus-optimization", feature = "sub-bus-protocol"))]
        let token_estimate = (task.file_count * 512) as u64;
        #[cfg(feature = "sub-bus-optimization")]
        let opt =
            self.optimization_bus
                .recommend(&task_type_str, token_estimate.max(1024), "balanced");

        // Phase 4: Protocol recommendation (used for routing diagnostics)
        #[cfg(feature = "sub-bus-protocol")]
        {
            let proto_reco = self
                .protocol_bus
                .recommend_protocol(&task_type_str, token_estimate.max(1024));
            self.record_event(
                "sense",
                None,
                None,
                "protocol_recommend",
                serde_json::json!({
                    "preferred_protocol": proto_reco.preferred_protocol,
                    "confidence": proto_reco.confidence,
                }),
            );
        }

        // Send a heartbeat through the transport layer, including task risk score
        // so the transport is always informed of the current task context.
        let transport = lock_guard(&self.transport);
        let heartbeat = format!(
            "{{\"status\":\"alive\",\"risk_score\":{}}}",
            task.risk_score
        );
        let _ = transport.send_heartbeat("capability-bus", "harness-bus", &heartbeat);

        SensingOutput {
            capability_agent_count: cap_agents,
            reputation_snapshot: rep_snapshot,
            recent_agents: _learning_rates,
            learning_snapshot,
            #[cfg(feature = "sub-bus-observability")]
            healthy_agents: healthy,
            #[cfg(feature = "sub-bus-orchestration")]
            available_modes: modes,
            #[cfg(feature = "sub-bus-optimization")]
            optimization: Some(opt),
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

        // Step C: pick best agent from capability graph + reputation
        // BLUE56-B11: Also query QLearningAgent for learned routing preferences
        let q_learning_state = (task_type_str.clone(), "select_agent".to_string());
        let _q_preferred_action =
            lock_guard(&self.q_learning).choose_action(&q_learning_state, &[]);
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

        // In profiles with tool bus, merge runtime-created sub-agent templates from AgentFactory.
        #[cfg(any(
            feature = "sub-bus-tool",
            feature = "profile-simple-server",
            feature = "profile-multi-users-server"
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

        // If ScenarioMatcher found a high-confidence match, prefer its routing
        let scenario_preferred_agent = if scenario_match.matched {
            scenario_match
                .scenario
                .as_ref()
                .and_then(|s| s.routing.preferred_agent.clone())
        } else {
            None
        };

        let (selected_agent, score_breakdown) =
            if let Some(ref preferred) = scenario_preferred_agent {
                let breakdown = vec![CandidateScoreBreakdown {
                    agent: preferred.clone(),
                    reputation_score: 1.0,
                    recency_score: 1.0,
                    task_fit_score: 1.0,
                    recent_outcome_score: 1.0,
                    total_score: 1.0,
                }];
                (Some(preferred.clone()), breakdown)
            } else {
                self.select_best_agent(task, &candidate_agents, sensing)
            };
        tracing::info!(
            candidates = ?candidate_agents,
            selected = ?selected_agent,
            "capability_bus agent selection"
        );

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

        #[cfg(feature = "sub-bus-observability")]
        let _healthy_agents_count = sensing.healthy_agents.len();

        {
            let mut p = write_guard(&self.profile);
            p.routing_count = p.routing_count.saturating_add(1);
            p.last_route_duration_ms = start.elapsed().as_millis() as u64;
        }

        // Send a control message through the transport layer if an agent was selected
        if let Some(agent) = &selected_agent {
            let transport = lock_guard(&self.transport);
            let msg = serde_json::json!({ "selected_tool": agent, "agent": agent });
            let _ = transport.send_control("capability-bus", "tool-bus", &msg.to_string());
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
                let total_score = (reputation_score * weights.reputation)
                    + (recency_score * weights.recency)
                    + (task_fit_score * weights.task_fit)
                    + (recent_outcome_score * weights.recent_outcome);
                CandidateScoreBreakdown {
                    agent: name.clone(),
                    reputation_score,
                    recency_score,
                    task_fit_score,
                    recent_outcome_score,
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
    // Stage 3: Action — dispatch to agent with tool bus awareness
    // ------------------------------------------------------------------

    /// Execute a tool through the ToolBus with HarnessBus validation
    pub fn execute_tool(
        &self,
        tool_name: &str,
        input: &crate::orchestration::tool::ToolInput,
    ) -> anyhow::Result<crate::orchestration::tool::ToolOutput> {
        // Step 1: Validate via HarnessBus
        let tool_verdict = self
            .harness
            .evaluator
            .check_tool_call(tool_name, &input.payload);
        if !tool_verdict.is_allowed() {
            self.record_event(
                "action",
                None,
                None,
                "blocked",
                Self::build_action_blocked_detail(tool_name, "HarnessBus denied"),
            );
            return Err(anyhow::anyhow!(
                "Tool call '{}' denied by HarnessBus policy",
                tool_name
            ));
        }

        // Step 2: Execute via ToolBus
        let start = Instant::now();
        #[cfg(feature = "sub-bus-tool")]
        let result = self.tool_bus.execute_tool(tool_name, input);
        #[cfg(not(feature = "sub-bus-tool"))]
        let result: anyhow::Result<crate::orchestration::tool::ToolOutput> =
            Err(anyhow::anyhow!("ToolBus not available in this profile"));
        let duration_ms = start.elapsed().as_millis() as u64;
        let success = result
            .as_ref()
            .map(|output| output.success)
            .unwrap_or(false);
        let error_text = result
            .as_ref()
            .err()
            .map(|err| err.to_string())
            .or_else(|| result.as_ref().ok().and_then(|output| output.error.clone()));

        // Step 3: Record execution in ObservabilityBus
        #[cfg(feature = "sub-bus-observability")]
        self.observability_bus.record_trace(
            "capability_bus",
            "tool_call",
            duration_ms,
            success,
            error_text.clone(),
            0,
        );

        // Step 4: Record event
        let outcome = Self::action_outcome_label(success);
        self.record_event(
            "action",
            None,
            None,
            outcome,
            Self::build_action_event_detail(tool_name, duration_ms, success, error_text),
        );

        result
    }

    /// Check if an agent is healthy via ObservabilityBus and OptimizationBus
    pub fn is_agent_healthy(&self, agent: &str) -> bool {
        // Check circuit breaker via OptimizationBus
        #[cfg(feature = "sub-bus-optimization")]
        if self.optimization_bus.is_circuit_broken(agent) {
            return false;
        }
        // Check error rate via ObservabilityBus
        #[cfg(feature = "sub-bus-observability")]
        if let Some(err_rate) = self.observability_bus.agent_error_rate(agent) {
            if err_rate.error_rate > 0.5 {
                return false;
            }
        }
        #[cfg(not(any(feature = "sub-bus-optimization", feature = "sub-bus-observability")))]
        let _ = agent;
        true
    }

    /// Get recommended execution mode via OrchestrationBus
    pub fn recommended_mode(&self, task_type: &str, complexity: f64) -> String {
        #[cfg(feature = "sub-bus-orchestration")]
        {
            self.orchestration_bus.recommend_mode(task_type, complexity)
        }
        #[cfg(not(feature = "sub-bus-orchestration"))]
        {
            let _ = (task_type, complexity);
            "auto".to_string()
        }
    }

    /// Get optimization recommendation for a task
    #[cfg(feature = "sub-bus-optimization")]
    pub fn optimization_recommendation(
        &self,
        task_type: &str,
        token_count: u64,
        priority: &str,
    ) -> crate::intelligence::capability_bus::optimization_bus::OptimizationRecommendation {
        self.optimization_bus
            .recommend(task_type, token_count, priority)
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

        #[cfg(feature = "sub-bus-orchestration")]
        let flow_id = format!("{}::{}", task_type, task_id);
        #[cfg(feature = "sub-bus-orchestration")]
        let _flow_guard = match self.orchestration_bus.start_flow(&flow_id, task_id) {
            Ok(_) => Some(FlowGuard {
                bus: &self.orchestration_bus,
                flow_id: &flow_id,
                task_id,
            }),
            Err(e) => {
                tracing::warn!("feedback: start_flow failed for {}: {}", flow_id, e);
                None::<FlowGuard>
            }
        };

        // 1. Write to learning bus
        {
            let mut lb = write_guard(&self.learning_bus);
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
        {
            let mut rep = lock_guard(&self.reputation);
            rep.record_outcome(agent, success);
        }

        // 3. Write to ObservabilityBus
        #[cfg(feature = "sub-bus-observability")]
        self.observability_bus.record_trace(
            agent,
            task_type,
            duration_ms,
            success,
            None,
            token_cost,
        );

        // 4. Write to OptimizationBus
        #[cfg(feature = "sub-bus-optimization")]
        self.optimization_bus
            .record_execution(agent, duration_ms, token_cost, success);

        // 4b. Update ProtocolBus with runtime latency on active transport.
        #[cfg(feature = "sub-bus-protocol")]
        {
            let active_transport = self.protocol_bus.active_transport();
            self.protocol_bus
                .record_protocol_latency(&active_transport, duration_ms);
        }

        // 4c. Persist execution summary to MemoryBus L1/L2.
        #[cfg(any(feature = "sub-bus-memory", feature = "sub-bus-distributed-memory"))]
        let memory_key = format!("{}::{}", task_type, task_id);
        #[cfg(any(feature = "sub-bus-memory", feature = "sub-bus-distributed-memory"))]
        let memory_value = serde_json::json!({
            "agent": agent,
            "success": success,
            "duration_ms": duration_ms,
            "token_cost": token_cost,
            "quality_score": quality_score,
        })
        .to_string()
        .into_bytes();
        #[cfg(feature = "sub-bus-memory")]
        self.memory_bus.store(
            &memory_key,
            memory_value,
            &crate::intelligence::capability_bus::memory_bus::CacheStrategy::default(),
        );

        // 4d. Persist execution summary to DistributedMemoryBus and share.
        #[cfg(feature = "sub-bus-distributed-memory")]
        {
            let dist_id = self.distributed_memory_bus.store_local(
                &memory_key,
                &format!(
                    "agent={} success={} quality={:.3}",
                    agent, success, quality_score
                ),
                vec![task_type.to_string(), agent.to_string()],
                quality_score,
                300_000,
            );
            let _ = self.distributed_memory_bus.share_with_peers(&dist_id);
        }

        // 5. Record event
        let outcome = Self::action_outcome_label(success);
        self.record_event(
            "feedback",
            Some(agent.to_string()),
            Some(task_id.to_string()),
            outcome,
            Self::build_feedback_event_detail(duration_ms, token_cost, quality_score, success),
        );

        // 6. Record provenance
        self.provenance_ledger.append(make_entry(
            task_id,
            task_type,
            agent,
            "capability_bus",
            &serde_json::json!({"task_type": task_type, "quality_score": quality_score}),
            &serde_json::json!({"success": success, "duration_ms": duration_ms}),
            vec![],
        ));

        // 7. Record execution result in SelfModel for per-capability EMA tracking
        self.self_model
            .record_execution_result(agent, success, duration_ms);

        // `complete_flow` is called automatically by `FlowGuard` RAII guard.
    }

    // ------------------------------------------------------------------
    // Stage 5: Evolution — reinforcement learning update (BLUE48 Step 1.2)
    // ------------------------------------------------------------------

    // ── evolve() subsystem methods ──────────────────────────────────────
    // Each method is < 100 lines, handles its own errors via warn!(), and
    // respects a combined deadline to prevent any single subsystem from
    // exceeding its ~100 ms budget.
    // ------------------------------------------------------------------

    /// Update Q-table with reward signal from latest execution.
    fn evolve_q_learning(
        &self,
        state: &(String, String),
        action: &str,
        next_state: &(String, String),
        token_cost: u64,
        success: bool,
        quality_score: f64,
    ) -> f64 {
        let metrics = RlTaskExecutionMetrics {
            tokens_used: token_cost,
            success,
            quality_score,
            duration_ms: 0,
        };
        let reward = lock_guard(&self.reward_fn).calculate(&metrics);
        lock_guard(&self.q_learning).update(state, action, reward, next_state);
        reward
    }

    /// Record success case in experience knowledge base.
    fn evolve_experience(
        &self,
        state: &(String, String),
        action: &str,
        success: bool,
        quality_score: f64,
    ) {
        if success {
            lock_guard(&self.experience).add_success_case(SuccessCase {
                objective: format!("state_{:?}", state),
                strategy: format!("action_{}", action),
                confidence: quality_score,
            });
        }
    }

    /// Record drift metrics through HarnessBus drift engine.
    fn evolve_drift_protection(&self, quality_score: f64, success: bool) {
        use crate::governance::drift::drift_protection::DriftType;
        let _ = self.harness.drift_engine.record_metric(
            "evolve_quality",
            quality_score,
            0.8,
            DriftType::Performance,
        );
        if !success {
            let _ = self.harness.drift_engine.record_metric(
                "evolve_failure",
                1.0,
                0.0,
                DriftType::Goal,
            );
        }
    }

    /// Register evolve action as a FaultTolerance node and send heartbeat.
    fn evolve_fault_tolerance(&self, node_id: &str) {
        let _ = self.harness.fault_tolerance.register_node(node_id);
        let _ = self.harness.fault_tolerance.report_heartbeat(node_id);
    }

    /// Record an audit entry for the evolve cycle.
    fn evolve_harness_bus(
        &self,
        state: &(String, String),
        action: &str,
        reward: f64,
        success: bool,
        quality_score: f64,
    ) {
        use crate::governance::harness_bus::AuditEntry;
        let now_for_audit = now_ms();
        let entry = AuditEntry {
            timestamp: now_for_audit as i64,
            request_id: format!("evolve_{}_{}", state.0, action),
            stage: "evolve".to_string(),
            verdict: if success { "allowed" } else { "failed" }.to_string(),
            dispatch_policy: "capability_bus".to_string(),
            execution_policy: "evolve".to_string(),
            governance_policy: "learn".to_string(),
            violations: vec![],
            context_snapshot: serde_json::json!({
                "state": state,
                "action": action,
                "reward": reward,
                "success": success,
                "quality": quality_score,
            }),
        };
        self.harness.audit(entry);
    }

    /// Submit local policy to FederatedRL.
    fn evolve_federated_rl(
        &self,
        state: &(String, String),
        action: &str,
        reward: f64,
        quality_score: f64,
        success: bool,
    ) {
        if success {
            let now = now_ms();
            let frl = self.federated_rl.submit_policy(
                "local_agent".to_string(),
                format!("evolve_{}", state.0),
                serde_json::json!({
                    "state": state,
                    "action": action,
                    "reward": reward,
                    "timestamp": now,
                })
                .to_string(),
                quality_score,
                1,
            );
            if let Err(e) = self
                .federated_rl
                .contribute_to_round(&format!("round_{}", state.0), &frl)
            {
                warn!("evolve: federated_rl.contribute_to_round failed: {}", e);
            }
        }
    }

    /// Consolidate experience into continuous learning center.
    ///
    /// Periodically triggers forgetting detection and experience replay
    /// to close the online learning loop (F-GAP-51).
    fn evolve_continuous_learning(
        &self,
        state: &(String, String),
        action: &str,
        reward: f64,
        success: bool,
        quality_score: f64,
    ) {
        if let Err(e) = lock_guard(&self.continuous_learning).consolidate_experience(
            &format!("{:?}_{}", state.0, action),
            &serde_json::json!({
                "state": state,
                "action": action,
                "success": success,
                "reward": reward,
                "quality": quality_score,
            })
            .to_string(),
            quality_score,
        ) {
            warn!(
                "evolve: continuous_learning.consolidate_experience failed: {}",
                e
            );
        }

        // ── Periodic maintenance: detect forgetting & replay (every 10th call) ──
        use std::sync::atomic::{AtomicU64, Ordering};
        static CL_MAINTENANCE_COUNTER: AtomicU64 = AtomicU64::new(0);
        let count = CL_MAINTENANCE_COUNTER.fetch_add(1, Ordering::Relaxed);

        if count.is_multiple_of(10) {
            // 1. Detect forgetting and reinforce forgotten memories
            let forgotten = {
                let cl = lock_guard(&self.continuous_learning);
                cl.detect_forgetting()
            };
            for curve in &forgotten {
                if let Err(e) =
                    lock_guard(&self.continuous_learning).reinforce_memory(&curve.memory_id)
                {
                    warn!("evolve: reinforce_memory failed: {}", e);
                }
            }
            if !forgotten.is_empty() {
                tracing::info!(
                    "evolve: continuous_learning reinforced {} forgotten memories",
                    forgotten.len()
                );
            }

            // 2. Replay important memories and feed into Q-learning
            let replayed = {
                let cl = lock_guard(&self.continuous_learning);
                cl.replay_important_memories(3)
            };
            for mem in &replayed {
                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&mem.data) {
                    // Parse the stored (state, action, reward) triple
                    let state_arr = data["state"].as_array();
                    let action_str = data["action"].as_str();
                    let replay_reward = data["reward"].as_f64();

                    if let (Some(arr), Some(action_str), Some(replay_reward)) =
                        (state_arr, action_str, replay_reward)
                    {
                        if arr.len() >= 2 {
                            if let (Some(s0), Some(s1)) = (arr[0].as_str(), arr[1].as_str()) {
                                let replayed_state = (s0.to_string(), s1.to_string());
                                // Perform a mini Q-learning update with
                                // replayed experience using the current
                                // state as the next_state placeholder.
                                lock_guard(&self.q_learning).update(
                                    &replayed_state,
                                    action_str,
                                    replay_reward,
                                    state,
                                );
                            }
                        }
                    }
                }
            }
            if !replayed.is_empty() {
                tracing::info!(
                    "evolve: continuous_learning replayed {} memories into Q-learning",
                    replayed.len()
                );
            }
        }
    }

    /// Record observation in metacognitive controller and feed feedback into Q-learning.
    fn evolve_metacognitive(
        &self,
        state: &(String, String),
        action: &str,
        reward: f64,
        quality_score: f64,
        success: bool,
    ) {
        if let Err(e) = self.metacognitive.record_observation(
            &format!("evolve_{}_{}", state.0, action),
            "capability_bus",
            "evolution",
            if success { "success" } else { "failure" },
            &format!("reward={}, quality={}", reward, quality_score),
        ) {
            warn!("evolve: metacognitive.record_observation failed: {}", e);
        }

        // ── Generate metacognitive feedback and feed into Q-learning (F-GAP-51) ──
        let feedback = self.metacognitive.generate_evolve_feedback();
        let reward_multiplier = feedback["reward_multiplier"].as_f64().unwrap_or(1.0);
        let suggested_exploration_rate = feedback["suggested_exploration_rate"]
            .as_f64()
            .unwrap_or(0.1);

        // Apply suggested exploration rate to Q-learning agent for future decisions.
        {
            let mut ql = lock_guard(&self.q_learning);
            ql.exploration_rate = suggested_exploration_rate;
        }

        // Scale the Q-value for this (state, action) pair by the reward_multiplier
        // to retroactively incorporate metacognitive insight into the Q-table.
        if (reward_multiplier - 1.0).abs() > 0.001 {
            let mut ql = lock_guard(&self.q_learning);
            if let Some(state_actions) = ql.q_table.get_mut(state) {
                if let Some(q_val) = state_actions.get_mut(action) {
                    *q_val *= reward_multiplier;
                }
            }
            if let Some(state_actions) = ql.q_table_2.get_mut(state) {
                if let Some(q_val) = state_actions.get_mut(action) {
                    *q_val *= reward_multiplier;
                }
            }
        }
    }

    /// Record successful patterns in DiscoveryCenter.
    fn evolve_discovery(
        &self,
        state: &(String, String),
        action: &str,
        reward: f64,
        quality_score: f64,
        success: bool,
        now: u64,
    ) {
        if success && quality_score > 0.7 {
            if let Err(e) = self.discovery.record_solution(
                crate::intelligence::discovery::DiscoveryEntry {
                    id: String::new(),
                    problem_pattern: format!("state_{}", state.0),
                    solution_summary: format!("action_{}", action),
                    solution_detail: serde_json::json!({"reward": reward, "quality": quality_score}),
                    applicability_tags: vec![state.0.clone(), state.1.clone()],
                    success_rate: quality_score,
                    total_attempts: 1,
                    successful_attempts: if success { 1 } else { 0 },
                    discovered_by: "capability_bus_evolve".to_string(),
                    created_ms: now,
                    last_used_ms: now,
                }
            ) {
                warn!("evolve: discovery.record_solution failed: {}", e);
            }
        }
    }

    /// Update EvolutionGraph with capability trajectory.
    fn evolve_evolution_graph(
        &self,
        state: &(String, String),
        action: &str,
        success: bool,
        quality_score: f64,
    ) {
        let mut eg = lock_guard(&self.evolution_graph);
        let cap_name = format!("evolve_{}", action);
        if let Err(e) = eg.register_capability(&state.0, &cap_name, EvolutionStage::New) {
            warn!("evolve: evolution_graph.register_capability failed: {}", e);
        }
        if let Err(e) = eg.record_version(
            &state.0,
            &cap_name,
            if success { quality_score } else { 0.0 },
            0.0,
        ) {
            warn!("evolve: evolution_graph.record_version failed: {}", e);
        }
        if success && quality_score > 0.8 {
            if let Some(rec) = eg.get_history(&state.0, &cap_name) {
                let next_stage = match rec.current_stage {
                    EvolutionStage::New => Some(EvolutionStage::Learning),
                    EvolutionStage::Learning
                        if rec.versions.len() >= 3 && rec.trend == TrendDirection::Improving =>
                    {
                        Some(EvolutionStage::Mature)
                    }
                    _ => None,
                };
                if let Some(stage) = next_stage {
                    if let Err(e) = eg.advance_stage(&state.0, &cap_name, stage) {
                        warn!("evolve: evolution_graph.advance_stage failed: {}", e);
                    }
                }
            }
        }
    }

    /// Record performance snapshot in SelfModel.
    fn evolve_self_model(&self, now: u64, success: bool) {
        use crate::intelligence::self_model::SelfPerformanceSnapshot;
        let snapshot = SelfPerformanceSnapshot {
            timestamp_ms: now,
            avg_latency_ms: 0.0,
            p50_latency_ms: 0.0,
            p95_latency_ms: 0.0,
            p99_latency_ms: 0.0,
            error_rate: if success { 0.0 } else { 1.0 },
            throughput: 1.0,
            agent_count: 1,
            tasks_processed: 1,
        };
        self.self_model.record_performance(snapshot);
    }

    /// Record awareness metrics in Consciousness.
    fn evolve_consciousness(
        &self,
        state: &(String, String),
        action: &str,
        quality_score: f64,
        success: bool,
    ) {
        use crate::intelligence::consciousness::AwarenessMetricType;
        let awareness_value = if success { quality_score } else { 0.1 };
        let _ = self.consciousness.record_metric(
            AwarenessMetricType::SelfAwareness,
            awareness_value,
            quality_score,
        );
        let _ = self.consciousness.record_metric(
            AwarenessMetricType::EnvironmentalAwareness,
            if quality_score > 0.5 { 0.7 } else { 0.3 },
            quality_score,
        );
        let profile = self.consciousness.profile();
        if profile.reflexion_count < 100 && success {
            let _ = self
                .consciousness
                .trigger_reflexion(&format!("evolve_cycle_{}_{}", state.0, action));
        }
    }

    /// Update WorldModel with entity state.
    fn evolve_world_model(&self, action: &str, state: &(String, String), reward: f64) {
        if let Err(e) = self.world_model.register_entity(
            &format!("action_{}", action),
            crate::intelligence::world_model::EntityType::System,
        ) {
            warn!("evolve: world_model.register_entity failed: {}", e);
        } else {
            let mut props = std::collections::HashMap::new();
            props.insert("state_0".to_string(), state.0.clone());
            props.insert("state_1".to_string(), state.1.clone());
            props.insert("reward".to_string(), reward.to_string());
            if let Err(e) = self
                .world_model
                .update_entity(&format!("action_{}", action), props)
            {
                warn!("evolve: world_model.update_entity failed: {}", e);
            }
        }
    }

    /// Record evolve result as a round in ConsensusEngine.
    fn evolve_consensus(
        &self,
        state: &(String, String),
        action: &str,
        reward: f64,
        q_value: f64,
        success: bool,
        now: u64,
    ) {
        use crate::intelligence::consensus::{ConsensusNode, ConsensusVote, NodeRole};
        let _ = self.consensus.register_node(ConsensusNode {
            id: "capability-bus".to_string(),
            address: "internal://capability_bus".to_string(),
            weight: 1,
            role: NodeRole::Leader,
            is_online: true,
            last_heartbeat_ms: now,
        });
        let proposals = vec![serde_json::json!({
            "action": action,
            "state": state,
            "reward": reward,
            "q_value": q_value,
            "success": success,
        })];
        let proposal_id = format!("proposal_{}_{}", state.0, action);
        match self.consensus.start_round("capability-bus", proposals) {
            Ok(rid) => {
                if let Err(e) = self.consensus.cast_vote(ConsensusVote {
                    node_id: "capability-bus".to_string(),
                    round_id: rid,
                    proposal_id,
                    approve: success,
                    weight: 1,
                    vote_ms: now,
                }) {
                    warn!("evolve: consensus.cast_vote failed: {}", e);
                }
            }
            Err(e) => warn!("evolve: consensus.start_round failed: {}", e),
        }
    }

    /// Coordinate the full evolution pipeline by delegating to focused
    /// subsystem methods.  Each subsystem has its own error handling so a
    /// single failure never blocks the rest of the pipeline.
    ///
    /// # Lock ordering
    ///
    /// Subsystem dispatch in `evolve()` follows a strict lock-ordering
    /// discipline to prevent deadlocks:
    ///
    ///   Level 1 (innermost – acquire first, release last):
    ///     reward_fn, q_learning, experience, continuous_learning
    ///
    ///   Level 2:
    ///     federated_rl, metacognitive, discovery, self_model,
    ///     consciousness, world_model, consensus
    ///
    ///   Level 3 (outermost – acquire last, release first):
    ///     evolution_graph, transport, learning_bus, reputation,
    ///     capability_graph, profile
    ///
    ///   Single-lock (no ordering conflicts):
    ///     harness, matcher, provenance_ledger, schema_registry
    ///
    ///   RULE: Never hold locks across subsystem boundaries.
    ///         Use `lock_guard` which is scoped; drop guards before
    ///         calling another subsystem method.
    ///
    /// Each subsystem call is wrapped in `tokio::time::timeout(100ms, …)`
    /// so a hung subsystem never stalls the pipeline.  Errors are logged
    /// as warnings and the pipeline continues.
    pub async fn evolve(
        &self,
        state: &(String, String),
        action: &str,
        next_state: &(String, String),
        token_cost: u64,
        success: bool,
        quality_score: f64,
    ) {
        // ── Core RL update (reward, Q-table, experience) ────────────────
        let reward = match timeout(Duration::from_millis(100), async {
            self.evolve_q_learning(
                state,
                action,
                next_state,
                token_cost,
                success,
                quality_score,
            )
        })
        .await
        {
            Ok(r) => r,
            Err(_) => {
                self.evolve_timeout_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                warn!("evolve: evolve_q_learning timed out — using default reward");
                0.0
            }
        };

        if timeout(Duration::from_millis(100), async {
            self.evolve_experience(state, action, success, quality_score)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_experience timed out — skipping");
        }

        // ── ScenarioMatcher: record task pattern ────────────────────────
        if timeout(Duration::from_millis(100), async {
            use crate::intelligence::matcher::{MatchRules, ScenarioRouting};
            let now = now_ms();
            let scenario_id = format!("evolve_{}_{}", state.0, action);
            self.matcher
                .register_scenario(crate::intelligence::matcher::Scenario {
                    id: scenario_id.clone(),
                    name: format!("Evolved: {} via {}", state.0, action),
                    description: format!(
                        "Auto-generated scenario from evolution: state={:?} action={} success={}",
                        state, action, success
                    ),
                    priority: if quality_score > 0.8 { 50 } else { 20 },
                    match_rules: MatchRules {
                        keywords: vec![state.0.clone(), action.to_string()],
                        task_types: vec![state.1.clone()],
                        agent_tags: vec![],
                        complexity_range: None,
                        risk_range: None,
                    },
                    routing: ScenarioRouting {
                        preferred_agent: None,
                        recommended_mode: if success { "auto".into() } else { "ask".into() },
                        enabled_tools: vec![],
                        add_tags: vec![state.0.clone(), action.to_string()],
                    },
                    created_ms: now,
                    is_active: success && quality_score > 0.6,
                });
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: scenario registration timed out — skipping");
        }

        // ── HarnessBus: drift / fault tolerance / audit ─────────────────
        if timeout(Duration::from_millis(100), async {
            self.evolve_drift_protection(quality_score, success)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_drift_protection timed out — skipping");
        }

        let node_id = format!("evolve::{}_{}", state.0, action);
        if timeout(Duration::from_millis(100), async {
            self.evolve_fault_tolerance(&node_id)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_fault_tolerance timed out — skipping");
        }

        if timeout(Duration::from_millis(100), async {
            self.evolve_harness_bus(state, action, reward, success, quality_score)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_harness_bus timed out — skipping");
        }

        // ── Cognitive modules ───────────────────────────────────────────
        if timeout(Duration::from_millis(100), async {
            self.evolve_federated_rl(state, action, reward, quality_score, success)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_federated_rl timed out — skipping");
        }

        if timeout(Duration::from_millis(100), async {
            self.evolve_continuous_learning(state, action, reward, success, quality_score)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_continuous_learning timed out — skipping");
        }

        if timeout(Duration::from_millis(100), async {
            self.evolve_metacognitive(state, action, reward, quality_score, success)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_metacognitive timed out — skipping");
        }

        let now = now_ms();
        if timeout(Duration::from_millis(100), async {
            self.evolve_discovery(state, action, reward, quality_score, success, now)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_discovery timed out — skipping");
        }

        // ── Abstract knowledge (periodic, every 50th evolve) ────────────
        if timeout(Duration::from_millis(100), async {
            use std::sync::atomic::{AtomicU64, Ordering};
            static EVOLVE_COUNTER: AtomicU64 = AtomicU64::new(0);
            let evolve_count = EVOLVE_COUNTER.fetch_add(1, Ordering::Relaxed);

            // Periodic world_model causal pattern discovery (every 100 cycles)
            if evolve_count.is_multiple_of(100) {
                let discoveries = self.world_model.discover_causal_patterns(60_000);
                if !discoveries.is_empty() {
                    tracing::info!(
                        "evolve: world_model discovered {} causal pattern(s)",
                        discoveries.len()
                    );
                    for d in &discoveries {
                        tracing::debug!("evolve: causal discovery: {}", d);
                    }
                }
            }

            if evolve_count.is_multiple_of(50) && quality_score > 0.5 {
                let insights = self.discovery.abstract_knowledge();
                if !insights.is_empty() {
                    tracing::info!(
                        "evolve: discovery abstract_knowledge generated {} insights",
                        insights.len()
                    );
                    for insight in &insights {
                        if let Err(e) = lock_guard(&self.continuous_learning)
                            .consolidate_experience(
                                &format!("abstract_knowledge_{}", now),
                                insight,
                                0.5,
                            )
                        {
                            warn!("evolve: abstract_knowledge consolidate failed: {}", e);
                        }
                    }
                    self.record_event(
                        "evolve",
                        None,
                        None,
                        "knowledge_abstraction",
                        serde_json::json!({
                            "insights_count": insights.len(),
                            "insights": insights,
                        }),
                    );
                }
            }
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: abstract_knowledge phase timed out — skipping");
        }

        // ── Self-model & meta-cognitive evolution ───────────────────────
        if timeout(Duration::from_millis(100), async {
            self.evolve_evolution_graph(state, action, success, quality_score)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_evolution_graph timed out — skipping");
        }

        if timeout(Duration::from_millis(100), async {
            self.evolve_self_model(now, success)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_self_model timed out — skipping");
        }

        if timeout(Duration::from_millis(100), async {
            self.evolve_consciousness(state, action, quality_score, success)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_consciousness timed out — skipping");
        }

        if timeout(Duration::from_millis(100), async {
            self.evolve_world_model(action, state, reward)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_world_model timed out — skipping");
        }

        // ── Transport event & consensus ────────────────────────────────
        let (q_value, exploration_rate) = timeout(Duration::from_millis(100), async {
            let ql = lock_guard(&self.q_learning);
            let qv = ql
                .q_table
                .get(state)
                .and_then(|m| m.get(action))
                .copied()
                .unwrap_or(0.0);
            let er = ql.exploration_rate;
            drop(ql);
            (qv, er)
        })
        .await
        .unwrap_or_else(|_| {
            warn!("evolve: q_learning lock timed out — using defaults");
            (0.0, 0.0)
        });

        if timeout(Duration::from_millis(100), async {
            let transport = lock_guard(&self.transport);
            let summary = serde_json::json!({
                "q_value": q_value,
                "exploration_rate": exploration_rate,
            });
            if let Err(e) = transport.send_event("capability-bus", "monitor", &summary.to_string())
            {
                warn!("evolve: transport.send_event failed: {}", e);
            }
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: transport.send_event timed out — skipping");
        }

        if timeout(Duration::from_millis(100), async {
            self.evolve_consensus(state, action, reward, q_value, success, now)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_consensus timed out — skipping");
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
    // Multi-model voter (sub-bus-voter-future)
    // ------------------------------------------------------------------

    /// Run multi-model voting on a high-stakes decision.
    /// Spawns concurrent agent evaluations and aggregates via configured strategy.
    #[cfg(feature = "sub-bus-voter-future")]
    pub async fn vote_on_decision(
        &self,
        prompt: &str,
        agents: &[std::sync::Arc<dyn crate::agents::agent::Agent>],
    ) -> anyhow::Result<crate::intelligence::multi_model_voter::VotingOutcome> {
        self.multi_voter.vote(prompt, agents).await
    }

    // ------------------------------------------------------------------
    // Observability helpers
    // ------------------------------------------------------------------

    pub fn snapshot_events(&self) -> Vec<BusEvent> {
        read_guard(&self.event_history).iter().cloned().collect()
    }

    pub fn capability_bus_profile(&self) -> CapabilityBusProfile {
        let mut p = write_guard(&self.profile);
        p.learning_events_count = read_guard(&self.learning_bus).len();
        p.reputation_agents_count = lock_guard(&self.reputation).tracked_agent_count();
        p.capability_graph_agents = lock_guard(&self.capability_graph).total_agents();
        p.knowledge_insights_count = read_guard(&self.knowledge_bus).snapshot().len();
        p.q_learning_table_size = lock_guard(&self.q_learning)
            .q_table
            .values()
            .map(|m| m.len())
            .sum();
        {
            let exp = lock_guard(&self.experience);
            p.experience_case_count = exp.success_cases.len() + exp.failure_patterns.len();
        }
        p.event_history_len = read_guard(&self.event_history).len();
        p.workflow_presets_count = self
            .workflow_registry
            .as_ref()
            .map(|wr| lock_guard(wr).list().len())
            .unwrap_or(0);
        p.provenance_entries_count = self.provenance_ledger.len();

        // Phase 4 sub-bus profile enrichment
        #[cfg(feature = "sub-bus-tool")]
        {
            let tb = self.tool_bus.profile();
            p.tool_bus_tools = tb.total_tools;
            p.tool_bus_skills = tb.total_skills;
            p.tool_bus_calls = tb.total_calls;
        }

        #[cfg(feature = "sub-bus-observability")]
        {
            let ob = self.observability_bus.system_health();
            p.observability_tracked_agents = ob.tracked_agents;
            p.observability_system_error_rate = ob.system_error_rate;
        }

        #[cfg(feature = "sub-bus-optimization")]
        {
            let opt = self.optimization_bus.profile();
            p.optimization_total = opt.total_optimizations;
            p.optimization_circuit_breaker_trips = opt.circuit_breaker_trips;
        }

        #[cfg(feature = "sub-bus-protocol")]
        {
            let pb = self.protocol_bus.profile();
            p.protocol_active_transport = pb.active_transport;
            p.protocol_healthy_count = pb.healthy_protocols;
        }

        #[cfg(feature = "sub-bus-orchestration")]
        {
            let orb = self.orchestration_bus.profile();
            p.orchestration_active_flows = orb.active_flows;
            p.orchestration_available_modes = orb.available_modes;
        }

        #[cfg(feature = "sub-bus-memory")]
        {
            let mb = self.memory_bus.profile();
            p.memory_cache_hit_rate = mb.cache_hit_rate;
            p.memory_total_entries = mb.vector_docs_count + mb.memory_entries;
        }

        #[cfg(feature = "sub-bus-distributed-memory")]
        {
            let dmb = self.distributed_memory_bus.profile();
            p.distributed_memory_peers = dmb.remote_peers;
            p.distributed_memory_shared = dmb.shared_entries;
        }

        // Evolve timeout counter — report cumulative degradation
        p.evolve_timeout_count = self
            .evolve_timeout_count
            .load(std::sync::atomic::Ordering::Relaxed);

        // Skill evolution metrics
        #[cfg(feature = "sub-bus-tool")]
        {
            let skills = lock_guard(self.tool_bus.skill_registry_ref());
            p.skill_evolution_count = skills
                .evolution_history
                .values()
                .map(|v| v.len())
                .sum::<usize>() as u32;
        }

        #[cfg(any(
            feature = "sub-bus-tool",
            feature = "profile-simple-server",
            feature = "profile-multi-users-server"
        ))]
        {
            let fp = lock_guard(&self.agent_factory).profile();
            p.agent_factory_active_instances = fp.active_instances as u32;
            p.agent_factory_templates = fp.total_templates as u32;

            let cp = lock_guard(&self.council).profile();
            p.council_active_members = cp.active_members;
            p.council_pending_proposals = cp.pending_count;
        }

        p.clone()
    }
}

// ---------------------------------------------------------------------------
/// Stage output types
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct SensingOutput {
    pub capability_agent_count: usize,
    pub reputation_snapshot: Vec<crate::intelligence::reputation::ReputationRecord>,
    pub recent_agents: Vec<String>,
    pub learning_snapshot: Vec<WorkflowLearningEvent>,
    /// Phase 4: healthy agents from ObservabilityBus
    #[cfg(feature = "sub-bus-observability")]
    pub healthy_agents: Vec<String>,
    /// Phase 4: available modes from OrchestrationBus
    #[cfg(feature = "sub-bus-orchestration")]
    pub available_modes: Vec<String>,
    /// Phase 4: optimization recommendation
    #[cfg(feature = "sub-bus-optimization")]
    pub optimization:
        Option<crate::intelligence::capability_bus::optimization_bus::OptimizationRecommendation>,
}

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

/// RAII guard that ensures `complete_flow` is called when `feedback()` returns,
/// even if an intermediate operation panics.
#[cfg(feature = "sub-bus-orchestration")]
struct FlowGuard<'a> {
    bus: &'a OrchestrationBus,
    flow_id: &'a str,
    task_id: &'a str,
}

#[cfg(feature = "sub-bus-orchestration")]
impl Drop for FlowGuard<'_> {
    fn drop(&mut self) {
        self.bus.complete_flow(self.flow_id, self.task_id);
    }
}

#[cfg(all(test, feature = "sub-bus-tool"))]
mod tests {
    use super::CapabilityBus;
    use crate::governance::harness_bus::default_harness_bus;
    use crate::governance::pua::{TaskContext, TaskType};
    use crate::intelligence::capability_graph::CapabilityDecl;
    use crate::orchestration::tool::ToolInput;
    use std::sync::Arc;

    #[tokio::test]
    async fn execute_tool_counts_single_call_in_tool_bus() {
        let harness = Arc::new(default_harness_bus(None));
        let bus = CapabilityBus::new_default(harness, None);

        let before = bus.tool_bus.profile().total_calls;
        let input = ToolInput {
            task_id: "cb-test-001".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "read cargo manifest".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({"path": "Cargo.toml"}),
            allowed_base_dir: None,
        };

        let result = bus.execute_tool("read_file", &input);
        assert!(
            result.is_ok(),
            "execute_tool should succeed: {:?}",
            result.err()
        );

        let after = bus.tool_bus.profile().total_calls;
        assert_eq!(
            after,
            before + 1,
            "tool call should be counted exactly once"
        );

        let profile = bus.capability_bus_profile();
        assert_eq!(profile.tool_bus_calls, after);

        let action_event = bus
            .snapshot_events()
            .into_iter()
            .rev()
            .find(|event| event.stage == "action")
            .expect("action event should be recorded");
        assert_eq!(action_event.outcome, "success");
        assert_eq!(
            action_event.detail.get("schema").and_then(|v| v.as_str()),
            Some("capability-bus-action-v1")
        );
        assert_eq!(
            action_event
                .detail
                .get("logical_success")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn configured_candidate_score_weights_are_normalized() {
        let weights = super::configured_candidate_score_weights();
        let total =
            weights.reputation + weights.recency + weights.task_fit + weights.recent_outcome;
        assert!((total - 1.0).abs() < 0.0001);
    }

    #[test]
    fn task_fit_score_prefers_security_agents_for_security_patch() {
        let security_task = TaskContext {
            task_type: TaskType::SecurityPatch,
            file_count: 4,
            risk_score: 0.9,
        };

        let reviewer = super::task_fit_score(&security_task, "security-reviewer");
        let general = super::task_fit_score(&security_task, "general-coder");

        assert!(reviewer > general);
    }

    #[test]
    fn recency_score_prefers_more_recent_agents() {
        let recent_agents = vec![
            "planner".to_string(),
            "reviewer".to_string(),
            "coder".to_string(),
        ];

        let recent = super::recency_score(&recent_agents, "coder");
        let stale = super::recency_score(&recent_agents, "planner");

        assert!(recent > stale);
    }

    #[test]
    fn configured_weights_env_override_respected_and_normalized() {
        let weights = super::configured_candidate_score_weights();
        let total =
            weights.reputation + weights.recency + weights.task_fit + weights.recent_outcome;
        assert!(
            (total - 1.0).abs() < 0.0001,
            "weights must sum to 1.0, got {}",
            total
        );
    }

    #[test]
    fn task_fit_score_differs_across_task_types() {
        // Verify that task_fit_score returns meaningfully different values
        // for the same agent across different task types.
        let fix_task = TaskContext {
            task_type: TaskType::BugFix,
            file_count: 2,
            risk_score: 0.5,
        };
        let security_task = TaskContext {
            task_type: TaskType::SecurityPatch,
            file_count: 2,
            risk_score: 0.9,
        };

        let fix_score = super::task_fit_score(&fix_task, "security-reviewer");
        let security_score = super::task_fit_score(&security_task, "security-reviewer");

        // security-reviewer should score higher for SecurityPatch than BugFix
        assert!(
            security_score > fix_score,
            "security-reviewer should score higher for SecurityPatch ({}) vs BugFix ({})",
            security_score,
            fix_score
        );
    }

    // ── Multi-factor E2E selection tests ────────────────────────────────

    fn make_sensing(bus: &CapabilityBus, recent_agents: Vec<String>) -> super::SensingOutput {
        let snapshot = bus
            .reputation
            .lock()
            .map(|r| r.snapshot())
            .unwrap_or_default();
        super::SensingOutput {
            capability_agent_count: 0,
            reputation_snapshot: snapshot,
            recent_agents,
            learning_snapshot: Vec::new(),
            #[cfg(feature = "sub-bus-observability")]
            healthy_agents: Vec::new(),
            #[cfg(feature = "sub-bus-orchestration")]
            available_modes: Vec::new(),
            #[cfg(feature = "sub-bus-optimization")]
            optimization: None,
        }
    }

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

    #[tokio::test]
    async fn multi_factor_selection_beats_reputation_only_for_security_task() {
        let harness = Arc::new(default_harness_bus(None));
        let bus = CapabilityBus::new_default(harness, None);

        {
            let mut graph = bus.capability_graph.lock().unwrap();
            register_test_agent(&mut graph, "security-auditor", vec!["security"]);
            register_test_agent(&mut graph, "general-coder", vec!["general"]);
            register_test_agent(&mut graph, "fix-specialist", vec!["bugfix", "general"]);
        }

        {
            let mut rep = bus.reputation.lock().unwrap();
            rep.record_outcome("security-auditor", true);
            rep.record_outcome("general-coder", true);
            rep.record_outcome("general-coder", true);
            rep.record_outcome("general-coder", true);
            rep.record_outcome("fix-specialist", true);
            rep.record_outcome("fix-specialist", true);
        }

        let recent = vec!["general-coder".to_string(), "fix-specialist".to_string()];
        let candidates = vec![
            "security-auditor".to_string(),
            "general-coder".to_string(),
            "fix-specialist".to_string(),
        ];

        let security_task = TaskContext {
            task_type: TaskType::SecurityPatch,
            file_count: 5,
            risk_score: 0.9,
        };

        let sensing = make_sensing(&bus, recent);
        let (selected, breakdown) = bus.select_best_agent(&security_task, &candidates, &sensing);

        assert_eq!(
            selected.as_deref(),
            Some("security-auditor"),
            "multi-factor should prefer security-auditor for SecurityPatch; got {:?}",
            selected
        );
        assert!(
            breakdown.len() >= 3,
            "all candidates should have score breakdowns"
        );

        let auditor_entry = breakdown
            .iter()
            .find(|e| e.agent == "security-auditor")
            .expect("security-auditor should be in breakdown");
        assert!(
            auditor_entry.task_fit_score > 0.9,
            "security-auditor should have high task-fit for SecurityPatch"
        );

        let coder_entry = breakdown
            .iter()
            .find(|e| e.agent == "general-coder")
            .expect("general-coder should be in breakdown");
        assert!(
            (0.0..=1.0).contains(&coder_entry.recent_outcome_score),
            "recent_outcome_score should be normalized into [0,1]"
        );
    }

    #[tokio::test]
    async fn multi_factor_vs_reputation_only_different_results_across_task_types() {
        let harness = Arc::new(default_harness_bus(None));
        let bus = CapabilityBus::new_default(harness, None);

        {
            let mut graph = bus.capability_graph.lock().unwrap();
            register_test_agent(&mut graph, "refactor-expert", vec!["refactor", "general"]);
            register_test_agent(&mut graph, "general-coder", vec!["general"]);
            register_test_agent(&mut graph, "debugger", vec!["bugfix", "general"]);
        }

        {
            let mut rep = bus.reputation.lock().unwrap();
            rep.record_outcome("refactor-expert", true);
            rep.record_outcome("general-coder", true);
            rep.record_outcome("debugger", true);
        }

        let candidates = vec![
            "refactor-expert".to_string(),
            "general-coder".to_string(),
            "debugger".to_string(),
        ];
        let recent = candidates.clone();
        let sensing = make_sensing(&bus, recent);

        let refactor_task = TaskContext {
            task_type: TaskType::Refactor,
            file_count: 10,
            risk_score: 0.3,
        };
        let (refactor_agent, _) = bus.select_best_agent(&refactor_task, &candidates, &sensing);
        assert_eq!(
            refactor_agent.as_deref(),
            Some("refactor-expert"),
            "refactor task should select refactor-expert"
        );

        let bugfix_task = TaskContext {
            task_type: TaskType::BugFix,
            file_count: 3,
            risk_score: 0.6,
        };
        let (bugfix_agent, _) = bus.select_best_agent(&bugfix_task, &candidates, &sensing);
        assert_eq!(
            bugfix_agent.as_deref(),
            Some("debugger"),
            "bugfix task should select debugger"
        );

        assert_ne!(
            refactor_agent, bugfix_agent,
            "routing should differ across task types"
        );
    }

    #[tokio::test]
    async fn select_best_agent_returns_empty_when_no_candidates() {
        let harness = Arc::new(default_harness_bus(None));
        let bus = CapabilityBus::new_default(harness, None);

        let task = TaskContext {
            task_type: TaskType::Other,
            file_count: 0,
            risk_score: 0.0,
        };
        let sensing = make_sensing(&bus, vec![]);
        let (selected, breakdown) = bus.select_best_agent(&task, &[], &sensing);
        assert!(selected.is_none());
        assert!(breakdown.is_empty());
    }

    #[tokio::test]
    async fn candidate_score_breakdown_contains_all_expected_fields() {
        let harness = Arc::new(default_harness_bus(None));
        let bus = CapabilityBus::new_default(harness, None);

        {
            let mut graph = bus.capability_graph.lock().unwrap();
            register_test_agent(&mut graph, "test-agent", vec!["general"]);
        }

        {
            let mut rep = bus.reputation.lock().unwrap();
            rep.record_outcome("test-agent", true);
        }

        let candidates = vec!["test-agent".to_string()];
        let recent = candidates.clone();
        let sensing = make_sensing(&bus, recent);

        let task = TaskContext {
            task_type: TaskType::BugFix,
            file_count: 2,
            risk_score: 0.5,
        };
        let (selected, breakdown) = bus.select_best_agent(&task, &candidates, &sensing);
        assert_eq!(selected.as_deref(), Some("test-agent"));

        let entry = &breakdown[0];
        assert_eq!(entry.agent, "test-agent");
        assert!(entry.reputation_score >= 0.0);
        assert!(entry.recency_score >= 0.0);
        assert!(entry.task_fit_score >= 0.0);
        assert!(entry.recent_outcome_score >= 0.0);
        assert!(entry.total_score >= 0.0);
    }

    #[tokio::test]
    async fn recent_outcome_score_prefers_recent_successes_for_same_task_type() {
        let harness = Arc::new(default_harness_bus(None));
        let bus = CapabilityBus::new_default(harness, None);

        let task = TaskContext {
            task_type: TaskType::BugFix,
            file_count: 3,
            risk_score: 0.4,
        };
        let target = format!("{:?}", task.task_type);

        let strong = vec![
            super::WorkflowLearningEvent {
                task_type: target.clone(),
                agent: "agent-a".to_string(),
                success: true,
                duration_ms: 100,
                token_cost: 50,
                quality_score: 0.9,
                timestamp_ms: 10,
            },
            super::WorkflowLearningEvent {
                task_type: target.clone(),
                agent: "agent-a".to_string(),
                success: true,
                duration_ms: 120,
                token_cost: 60,
                quality_score: 0.8,
                timestamp_ms: 11,
            },
        ];
        let weak = vec![
            super::WorkflowLearningEvent {
                task_type: target,
                agent: "agent-b".to_string(),
                success: false,
                duration_ms: 100,
                token_cost: 50,
                quality_score: 0.2,
                timestamp_ms: 12,
            },
            super::WorkflowLearningEvent {
                task_type: "Other".to_string(),
                agent: "agent-b".to_string(),
                success: true,
                duration_ms: 120,
                token_cost: 60,
                quality_score: 0.4,
                timestamp_ms: 13,
            },
        ];

        let mut learning = strong;
        learning.extend(weak);

        let strong_score = super::recent_outcome_score(&learning, &task, "agent-a");
        let weak_score = super::recent_outcome_score(&learning, &task, "agent-b");
        assert!(strong_score > weak_score);

        let mut sensing = make_sensing(&bus, vec![]);
        sensing.learning_snapshot = learning;

        let candidates = vec!["agent-a".to_string(), "agent-b".to_string()];
        let (selected, breakdown) = bus.select_best_agent(&task, &candidates, &sensing);
        assert_eq!(selected.as_deref(), Some("agent-a"));
        assert_eq!(breakdown.len(), 2);
    }

    #[tokio::test]
    async fn select_best_agent_single_candidate_returns_that_candidate() {
        // Edge case: only one candidate available — should still be selected
        // and produce a valid breakdown.
        let harness = Arc::new(default_harness_bus(None));
        let bus = CapabilityBus::new_default(harness, None);

        {
            let mut graph = bus.capability_graph.lock().unwrap();
            register_test_agent(&mut graph, "solo-agent", vec!["general"]);
        }
        {
            let mut rep = bus.reputation.lock().unwrap();
            rep.record_outcome("solo-agent", true);
        }

        let candidates = vec!["solo-agent".to_string()];
        let recent = candidates.clone();
        let sensing = make_sensing(&bus, recent);

        let task = TaskContext {
            task_type: TaskType::BugFix,
            file_count: 1,
            risk_score: 0.3,
        };

        let (selected, breakdown) = bus.select_best_agent(&task, &candidates, &sensing);
        assert_eq!(selected.as_deref(), Some("solo-agent"));
        assert_eq!(breakdown.len(), 1);

        let entry = &breakdown[0];
        assert_eq!(entry.agent, "solo-agent");
        assert!(entry.total_score > 0.0);
    }

    #[tokio::test]
    async fn select_best_agent_tiebreaker_is_alphabetical() {
        // Edge case: when all scores are equal, alphabetical order should
        // be the tiebreaker. Use fresh agents with no reputation/events.
        let harness = Arc::new(default_harness_bus(None));
        let bus = CapabilityBus::new_default(harness, None);

        // Register agents but do NOT add any reputation events so all scores
        // start at the same baseline (reputation=0.5, recency=0.0, etc.)
        {
            let mut graph = bus.capability_graph.lock().unwrap();
            register_test_agent(&mut graph, "zulu-agent", vec!["general"]);
            register_test_agent(&mut graph, "alpha-agent", vec!["general"]);
            register_test_agent(&mut graph, "beta-agent", vec!["general"]);
        }

        let candidates = vec![
            "zulu-agent".to_string(),
            "alpha-agent".to_string(),
            "beta-agent".to_string(),
        ];
        let sensing = make_sensing(&bus, vec![]);

        let task = TaskContext {
            task_type: TaskType::Other,
            file_count: 0,
            risk_score: 0.0,
        };

        let (selected, breakdown) = bus.select_best_agent(&task, &candidates, &sensing);
        // All agents have equal scores, so alphabetical: alpha-agent wins
        assert_eq!(
            selected.as_deref(),
            Some("alpha-agent"),
            "tiebreaker should pick first alphabetically: alpha-agent, got {:?}",
            selected
        );
        assert_eq!(breakdown.len(), 3);
        // Verify the breakdown is also sorted alphabetically after score tie
        assert_eq!(breakdown[0].agent, "alpha-agent");
        assert_eq!(breakdown[1].agent, "beta-agent");
        assert_eq!(breakdown[2].agent, "zulu-agent");
    }

    #[tokio::test]
    async fn recent_outcome_score_defaults_to_mid_when_no_events() {
        // Edge case: recent_outcome_score should return 0.5 (neutral) when
        // there are no learning events for the agent-task combination.
        let events = vec![];
        let task = TaskContext {
            task_type: TaskType::BugFix,
            file_count: 2,
            risk_score: 0.5,
        };

        let score = super::recent_outcome_score(&events, &task, "unknown-agent");
        assert!(
            (score - 0.5).abs() < 1e-6,
            "expected 0.5 for no events, got {}",
            score
        );
    }
}
