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

use crate::intelligence::federated_rl::FederatedRL;
use crate::intelligence::matcher::ScenarioMatcher;
use crate::intelligence::metacognitive::MetacognitiveController;
use crate::intelligence::reinforcement::learning::{
    ExperienceKnowledgeBase, QLearningAgent, RewardFunction, RlTaskExecutionMetrics, SuccessCase,
};
use crate::intelligence::reputation::ReputationStore;
use crate::intelligence::self_model::SelfModelCore;
use crate::intelligence::world_model::WorldModel;
use crate::observability::provenance::{make_entry, ProvenanceLedger};
#[cfg(any(
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
use std::sync::{Arc, Mutex};
use std::time::Instant;
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
            council_pending_proposals: 0,
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
        feature = "profile-simple-server",
        feature = "profile-multi-users-server"
    ))]
    pub agent_factory: Arc<Mutex<AgentFactory>>,
    /// Orchestration council — multi-agent voting governance (F-GAP-15)
    #[cfg(any(
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
}

impl CapabilityBus {
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
            learning_bus: Arc::new(Mutex::new(WorkflowLearningBus::new(1000))),
            knowledge_bus: Arc::new(Mutex::new(KnowledgeBus::default())),
            reputation: Arc::new(Mutex::new(reputation)),
            capability_graph: Arc::new(Mutex::new(capability_graph)),
            q_learning: Arc::new(Mutex::new(q_learning)),
            experience: Arc::new(Mutex::new(experience)),
            reward_fn: Arc::new(Mutex::new(reward_fn)),
            event_history: Arc::new(Mutex::new(VecDeque::with_capacity(100))),
            profile: Arc::new(Mutex::new(CapabilityBusProfile::default())),
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
            memory_bus: MemoryBus::new(None, None, None, None),
            #[cfg(feature = "sub-bus-protocol")]
            protocol_bus: ProtocolBus::new(),
            #[cfg(feature = "sub-bus-orchestration")]
            orchestration_bus: OrchestrationBus::new(None),
            #[cfg(feature = "sub-bus-distributed-memory")]
            distributed_memory_bus: DistributedMemoryBus::new(5000),
            max_event_history: 100,
            consciousness: ConsciousnessMetrics::new(Default::default()),
            metacognitive: MetacognitiveController::new(Default::default()),
            world_model: WorldModel::new(Default::default()),
            self_model: SelfModelCore::new(Default::default()),
            federated_rl: FederatedRL::new(Default::default()),
            matcher: ScenarioMatcher::default(),
            discovery: DiscoveryCenter::new(),
            consensus: ConsensusEngine::new(Default::default()),
            #[cfg(any(
                feature = "profile-simple-server",
                feature = "profile-multi-users-server"
            ))]
            agent_factory: Arc::new(Mutex::new(AgentFactory::new(AgentFactoryConfig::default()))),
            #[cfg(any(
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

    pub fn sense(&self, task: &TaskContext) -> SensingOutput {
        // Include task risk score in heartbeat so `task` is unconditionally referenced
        // across all feature configurations.
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
        if let Ok(transport) = self.transport.lock() {
            let heartbeat = format!(
                "{{\"status\":\"alive\",\"risk_score\":{}}}",
                task.risk_score
            );
            let _ = transport.send_heartbeat("capability-bus", "harness-bus", &heartbeat);
        }

        SensingOutput {
            capability_agent_count: cap_agents,
            reputation_snapshot: rep_snapshot,
            recent_agents: _learning_rates,
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

        // In server profiles, merge runtime-created sub-agent templates from AgentFactory.
        #[cfg(any(
            feature = "profile-simple-server",
            feature = "profile-multi-users-server"
        ))]
        let candidate_agents = {
            let mut agents = candidate_agents;
            if let Ok(factory) = self.agent_factory.lock() {
                for inst in factory.find_agents_by_capability("general") {
                    if !agents.iter().any(|name| name == &inst.template_name) {
                        agents.push(inst.template_name);
                    }
                }
            }
            agents
        };

        let selected_agent = self.select_best_agent(&candidate_agents, sensing);

        // Step B2: Consult WorkflowRegistry for workflow-based routing metadata
        let workflow_preset = self.workflow_registry.as_ref().and_then(|wr| {
            wr.lock().ok().and_then(|registry| {
                let task_type_str = format!("{:?}", task.task_type).to_lowercase();
                let mapped_name = match task_type_str.as_str() {
                    "bugfix" | "featureadd" | "refactor" | "securitypatch" => "dev",
                    _ => "general",
                };
                registry.find(mapped_name).cloned()
            })
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

        let confidence = sensing
            .reputation_snapshot
            .iter()
            .find(|r| Some(r.agent.as_str()) == selected_agent.as_deref())
            .map(|r| r.score)
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
            }),
        );

        #[cfg(feature = "sub-bus-observability")]
        let _healthy_agents_count = sensing.healthy_agents.len();

        if let Ok(mut p) = self.profile.lock() {
            p.routing_count = p.routing_count.saturating_add(1);
            p.last_route_duration_ms = start.elapsed().as_millis() as u64;
        }

        // Send a control message through the transport layer if an agent was selected
        if let Some(agent) = &selected_agent {
            if let Ok(transport) = self.transport.lock() {
                let msg = serde_json::json!({ "selected_tool": agent, "agent": agent });
                let _ = transport.send_control("capability-bus", "tool-bus", &msg.to_string());
            }
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
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(b.0))
        });
        scored.first().map(|(name, _)| (*name).clone())
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
                serde_json::json!({"tool": tool_name, "reason": "HarnessBus denied"}),
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
        let success = result.is_ok();

        // Step 3: Record execution in ObservabilityBus
        #[cfg(feature = "sub-bus-observability")]
        self.observability_bus.record_trace(
            "capability_bus",
            "tool_call",
            duration_ms,
            success,
            result.as_ref().err().map(|e| e.to_string()),
            0,
        );

        // Step 4: Record outcome in ToolBus
        #[cfg(feature = "sub-bus-tool")]
        self.tool_bus
            .record_tool_call(tool_name, success, duration_ms);

        // Step 5: Record event
        let outcome = if success { "success" } else { "failure" };
        self.record_event(
            "action",
            None,
            None,
            outcome,
            serde_json::json!({"tool": tool_name, "duration_ms": duration_ms}),
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
        let _flow_guard = FlowGuard {
            bus: &self.orchestration_bus,
            flow_id: &flow_id,
            task_id,
        };
        #[cfg(feature = "sub-bus-orchestration")]
        let _ = self.orchestration_bus.start_flow(&flow_id, task_id);

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

        // `complete_flow` is called automatically by `FlowGuard` RAII guard.
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
            if let Err(e) = self.experience.lock().map(|mut exp| {
                exp.add_success_case(SuccessCase {
                    objective: format!("state_{:?}", state),
                    strategy: format!("action_{}", action),
                    confidence: quality_score,
                })
            }) {
                warn!("failed to record success case for state {:?}: {}", state, e);
            }
        }

        // --- Cognitive module integration ---

        let now = now_ms();

        // 1. FederatedRL: submit local policy update
        if success {
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
            // Try to contribute to the current round if one exists
            if let Err(e) = self
                .federated_rl
                .contribute_to_round(&format!("round_{}", state.0), &frl)
            {
                warn!("evolve: federated_rl.contribute_to_round failed: {}", e);
            }
        }

        // 2. ContinuousLearning: consolidate experience to prevent forgetting
        if let Err(e) = self.continuous_learning.lock().map(|cl| {
            cl.consolidate_experience(
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
            )
        }) {
            warn!(
                "evolve: continuous_learning.consolidate_experience failed: {}",
                e
            );
        }

        // 3. Metacognitive: record observation for self-reflection
        if let Err(e) = self.metacognitive.record_observation(
            &format!("evolve_{}_{}", state.0, action),
            "capability_bus",
            "evolution",
            if success { "success" } else { "failure" },
            &format!("reward={}, quality={}", reward, quality_score),
        ) {
            warn!("evolve: metacognitive.record_observation failed: {}", e);
        }

        // 4. DiscoveryCenter: record successful patterns
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

        // 5. EvolutionGraph: update capability evolution trajectory
        if let Ok(mut eg) = self.evolution_graph.lock() {
            let cap_name = format!("evolve_{}", action);

            // Register the capability if it doesn't exist yet
            if let Err(e) = eg.register_capability(&state.0, &cap_name, EvolutionStage::New) {
                warn!("evolve: evolution_graph.register_capability failed: {}", e);
            }

            // Record a new version snapshot
            if let Err(e) = eg.record_version(
                &state.0,
                &cap_name,
                if success { quality_score } else { 0.0 },
                0.0,
            ) {
                warn!("evolve: evolution_graph.record_version failed: {}", e);
            }

            // Promote if consistently successful (high quality, multiple versions)
            if success && quality_score > 0.8 {
                if let Some(rec) = eg.get_history(&state.0, &cap_name) {
                    let next_stage = match rec.current_stage {
                        EvolutionStage::New => Some(EvolutionStage::Learning),
                        EvolutionStage::Learning
                            if rec.versions.len() >= 3
                                && rec.trend == TrendDirection::Improving =>
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

        // 6. WorldModel: update environmental state cognition
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

        // Send an event through the transport layer with evolve summary
        // Compute q_value for both the transport event and consensus
        let q_value;
        let exploration_rate;
        if let Ok(ql) = self.q_learning.lock() {
            q_value = ql
                .q_table
                .get(state)
                .and_then(|m| m.get(action))
                .copied()
                .unwrap_or(0.0);
            exploration_rate = ql.exploration_rate;
        } else {
            q_value = 0.0;
            exploration_rate = 0.0;
        }

        // Send an event through the transport layer with evolve summary
        if let Ok(transport) = self.transport.lock() {
            let summary =
                serde_json::json!({ "q_value": q_value, "exploration_rate": exploration_rate });
            if let Err(e) = transport.send_event("capability-bus", "monitor", &summary.to_string())
            {
                warn!("evolve: transport.send_event failed: {}", e);
            }
        }

        // 7. ConsensusEngine: record the evolve result as a round in consensus
        {
            use crate::intelligence::consensus::{ConsensusNode, ConsensusVote, NodeRole};
            if let Err(e) = self.consensus.register_node(ConsensusNode {
                id: "capability-bus".to_string(),
                address: "internal://capability_bus".to_string(),
                weight: 1,
                role: NodeRole::Leader,
                is_online: true,
                last_heartbeat_ms: 0,
            }) {
                warn!("evolve: consensus.register_node failed: {}", e);
            }
            match self.consensus.start_round(
                "capability-bus",
                vec![serde_json::json!({
                    "action": action,
                    "state": state,
                    "reward": reward,
                    "q_value": q_value,
                    "success": success,
                })],
            ) {
                Ok(rid) => {
                    if let Err(e) = self.consensus.cast_vote(ConsensusVote {
                        node_id: "capability-bus".to_string(),
                        round_id: rid,
                        proposal_id: String::new(),
                        approve: success,
                        weight: 1,
                        vote_ms: 0,
                    }) {
                        warn!("evolve: consensus.cast_vote failed: {}", e);
                    }
                }
                Err(e) => warn!("evolve: consensus.start_round failed: {}", e),
            }
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
            p.q_learning_table_size = self
                .q_learning
                .lock()
                .map(|ql| ql.q_table.values().map(|m| m.len()).sum())
                .unwrap_or(0);
            p.experience_case_count = self
                .experience
                .lock()
                .map(|exp| exp.success_cases.len() + exp.failure_patterns.len())
                .unwrap_or(0);
            p.event_history_len = self.event_history.lock().map(|h| h.len()).unwrap_or(0);
            p.workflow_presets_count = self
                .workflow_registry
                .as_ref()
                .and_then(|wr| wr.lock().ok())
                .map(|r| r.list().len())
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

            // Skill evolution metrics
            #[cfg(feature = "sub-bus-tool")]
            if let Ok(skills) = self.tool_bus.skill_registry_ref().lock() {
                p.skill_evolution_count = skills
                    .evolution_history
                    .values()
                    .map(|v| v.len())
                    .sum::<usize>() as u32;
            }

            #[cfg(any(
                feature = "profile-simple-server",
                feature = "profile-multi-users-server"
            ))]
            {
                if let Ok(factory) = self.agent_factory.lock() {
                    let fp = factory.profile();
                    p.agent_factory_active_instances = fp.active_instances as u32;
                    p.agent_factory_templates = fp.total_templates as u32;
                }
                if let Ok(council) = self.council.lock() {
                    let cp = council.profile();
                    p.council_active_members = cp.active_members;
                    p.council_pending_proposals = cp.pending_count;
                }
            }

            p.clone()
        } else {
            CapabilityBusProfile::default()
        }
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

/// Current wall-clock time in milliseconds since Unix epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
