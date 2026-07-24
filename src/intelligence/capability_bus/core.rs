//! Core CapabilityBus implementation.
//!
//! Full multi-bus bidirectional closed-loop (BLUE38 ARCH-13):
//!   sense → decide → act → feedback → evolve
//!
//! This module defines the top-level `CapabilityBus` struct that holds references
//! to all sub-bus components and orchestrates the complete lifecycle.
//! Sub-buses (BLUE70 consolidated):
//!   1. UnifiedKnowledgeBus  (merged KnowledgeBus + ReputationStore + ExperienceKnowledgeBase)
//!   2. ReinforcementBus     (merged QLearningAgent + FederatedRL)
//!   3. LearningOptimizationBus (merged WorkflowLearningBus + OptimizationBus)
//!   4. CapabilityGraph       (existing)
//!   5. HarnessBus            (existing)
//!   6.  ToolBus
//!   7.  ObservabilityBus
//!   8.  OptimizationBus
//!   9.  MemoryBus
//!  10.  ProtocolBus
//!  11.  OrchestrationBus
//!  12.  DistributedMemoryBus
//!
//! # Module structure
//!
//! The five lifecycle stages are split into focused sub-modules:
//! - `sense`    — stage 1: gather input from sub-buses
//! - `decide`   — stage 2: select agent / strategy
//! - `act`      — stage 3: dispatch tool execution
//! - `feedback` — stage 4: write results back to sub-buses
//!
//! The `evolve()` method (stage 5) and all private evolve helpers remain
//! here in `core.rs`, alongside the struct definition, constructors, and
//! profile helpers.

#[cfg(any(
    feature = "sub-bus-tool",
    feature = "simple-server",
    feature = "multi-users-server"
))]
use crate::agents::factory::{AgentFactory, AgentFactoryConfig};
use crate::governance::hardening::TenantBudgetEnforcer;
use crate::governance::harness_bus::HarnessBus;
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
use crate::intelligence::evolution_graph::EvolutionGraph;

// BLUE70: Consolidated buses
use crate::intelligence::capability_bus::learning_optimization_bus::LearningOptimizationBus;
use crate::intelligence::capability_bus::reinforcement_bus::ReinforcementBus;
use crate::intelligence::capability_bus::unified_knowledge_bus::UnifiedKnowledgeBus;

use crate::intelligence::adaptive_selector::AdaptiveModelSelector;
use crate::intelligence::hot_failover::HotFailover;
use crate::intelligence::matcher::ScenarioMatcher;
use crate::intelligence::metacognitive::MetacognitiveController;
use crate::intelligence::now_ms;
use crate::intelligence::reinforcement::federated::FederatedLearning;
use crate::intelligence::reinforcement::federated::FederatedRL;
use crate::intelligence::reinforcement::learning::RewardFunction;
use crate::intelligence::self_model::SelfModelCore;
use crate::intelligence::token_cache::TokenMultiLevelCache;
use crate::observability::live_performance::LivePerformanceFeed;

use crate::intelligence::world_model::WorldModel;
use crate::intelligence::{lock_guard, read_guard, write_guard};
use crate::observability::provenance::ProvenanceLedger;
#[cfg(any(
    feature = "sub-bus-tool",
    feature = "simple-server",
    feature = "multi-users-server"
))]
use crate::orchestration::council::{CouncilConfig, OrchestrationCouncil};
use crate::orchestration::task_schema::SchemaRegistry;
use crate::orchestration::workflow_optimizer::OptimizerRegistry;
use crate::orchestration::workflow_registry::WorkflowRegistry;
use crate::protocol::transport::MultiChannelTransport;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, RwLock};
use tokio::time::{timeout, Duration};
use tracing::warn;

// ---------------------------------------------------------------------------
// Re-exports from sibling sub-modules so that existing paths like
// `crate::intelligence::capability_bus::core::SensingOutput` still work.
// ---------------------------------------------------------------------------

// Used only in tests — conditionally imported to avoid unused-import warnings
#[cfg(test)]
pub(crate) use super::decide::{
    configured_candidate_score_weights, recency_score, recent_outcome_score, task_fit_score,
};
#[cfg(test)]
pub(crate) use super::sense::SensingOutput;

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
// WorkflowLearningEvent — shared event type
// ---------------------------------------------------------------------------

/// Runtime execution event (used by SharedLearning and the BLUE70 LearningOptimizationBus).
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

// (Builder section below)
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
    // BLUE70: Consolidated bus metrics
    pub unified_knowledge_insight_count: usize,
    pub unified_knowledge_experience_count: usize,
    pub reinforcement_table_size: usize,
    pub learning_optimization_event_count: usize,
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
            unified_knowledge_insight_count: 0,
            unified_knowledge_experience_count: 0,
            reinforcement_table_size: 0,
            learning_optimization_event_count: 0,
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
    /// Timeout (ms) for each subsystem call inside evolve(). Default: 100.
    pub subsystem_timeout_ms: u64,
}

impl Default for CapabilityBusConfig {
    fn default() -> Self {
        Self {
            evolve_interval: 50,
            enable_capability_bus: false,
            subsystem_timeout_ms: 100,
        }
    }
}

/// CapabilityBus aggregates all sub-bus references and orchestrates the
/// 5-stage lifecycle: sense → decide → act → feedback → evolve.
/// This is the scheduling coordinator for all 14 sub-buses (BLUE38 ARCH-13).
pub struct CapabilityBus {
    /// HarnessBus — strategy engine (pre-route / pre-tool / post-exec)
    pub harness: Arc<HarnessBus>,

    /// Capability graph — agent capability declarations and handoff edges
    pub capability_graph: Arc<Mutex<CapabilityGraph>>,

    /// Reward function — calculates reward from execution metrics
    pub reward_fn: Arc<Mutex<RewardFunction>>,

    /// Bus event history (for observability / tracing)
    pub event_history: Arc<RwLock<VecDeque<BusEvent>>>,

    /// Capability bus profile (for governance.status)
    pub profile: Arc<RwLock<CapabilityBusProfile>>,

    /// Workflow registry — named workflow presets for workflow-based routing
    pub(crate) workflow_registry: Option<Arc<Mutex<WorkflowRegistry>>>,

    /// Provenance ledger — immutable data lineage tracking for every operation
    pub provenance_ledger: Arc<ProvenanceLedger>,

    /// Schema registry — validates task envelopes against role schemas (F-GAP-07)
    pub schema_registry: Arc<Mutex<SchemaRegistry>>,

    /// Tenant budget enforcer — per-tenant resource quota management (F-GAP-08)
    pub tenant_budget: Arc<Mutex<TenantBudgetEnforcer>>,

    /// Optimizer registry — workflow optimization plugins (ARCH-11)
    pub optimizer_registry: Arc<Mutex<OptimizerRegistry>>,

    // ── BLUE70 consolidated buses ──
    /// UnifiedKnowledgeBus — merged KnowledgeBus + ReputationStore + ExperienceKnowledgeBase
    pub unified_knowledge_bus: Arc<RwLock<UnifiedKnowledgeBus>>,
    /// ReinforcementBus — merged QLearningAgent + FederatedRL
    pub reinforcement_bus: Arc<RwLock<ReinforcementBus>>,
    /// LearningOptimizationBus — merged WorkflowLearningBus + OptimizationBus
    pub learning_optimization_bus: Arc<RwLock<LearningOptimizationBus>>,

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
        feature = "simple-server",
        feature = "multi-users-server"
    ))]
    pub agent_factory: Arc<Mutex<AgentFactory>>,
    /// Orchestration council — multi-agent voting governance (F-GAP-15)
    #[cfg(any(
        feature = "sub-bus-tool",
        feature = "simple-server",
        feature = "multi-users-server"
    ))]
    pub council: Arc<Mutex<OrchestrationCouncil>>,
    /// Evolution graph — capability lifecycle tracking (F-GAP-18)
    pub evolution_graph: Arc<Mutex<EvolutionGraph>>,

    /// Continuous learning center — lifelong learning (F-GAP-24)
    pub continuous_learning: Arc<Mutex<ContinuousLearningCenter>>,

    /// Multi-channel message transport — protocol layer (F-GAP-29)
    pub transport: Arc<Mutex<MultiChannelTransport>>,

    /// Token multi-level cache for LLM response caching (P2-1)
    pub token_cache: Option<Arc<TokenMultiLevelCache>>,

    /// Adaptive model selector for context-aware model routing (P2-3)
    pub model_selector: Option<Mutex<AdaptiveModelSelector>>,

    /// Federated learning coordinator — cross-node policy aggregation (P2-4)
    pub federated_learning: Option<Arc<Mutex<FederatedLearning>>>,

    /// Live performance feed — EMA-smoothed model cost estimates (P2-6)
    pub live_performance: Option<Arc<LivePerformanceFeed>>,

    /// Hot failover manager — transparent model failover with cooldown (P2-7)
    pub hot_failover: Option<Arc<HotFailover>>,

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
    //     reward_fn, q_learning, experience, continuous_learning,
    //     reinforcement_bus          (BLUE70: replaces q_learning)
    //
    //   Level 2:
    //     federated_rl, metacognitive, discovery, self_model,
    //     consciousness, world_model, consensus
    //
    //   Level 3 (outermost – acquire last, release first):
    //     evolution_graph, transport, learning_bus, reputation,
    //     capability_graph, profile,
    //     unified_knowledge_bus       (BLUE70: replaces knowledge_bus+reputation)
    //     learning_optimization_bus   (BLUE70: replaces learning_bus)
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
        capability_graph: CapabilityGraph,
        reward_fn: RewardFunction,
        provenance_ledger: Arc<ProvenanceLedger>,
    ) -> Self {
        Self {
            harness,
            capability_graph: Arc::new(Mutex::new(capability_graph)),
            reward_fn: Arc::new(Mutex::new(reward_fn)),
            event_history: Arc::new(RwLock::new(VecDeque::with_capacity(100))),
            profile: Arc::new(RwLock::new(CapabilityBusProfile::default())),
            workflow_registry: None,
            provenance_ledger,
            schema_registry: Arc::new(Mutex::new(SchemaRegistry::new())),
            tenant_budget: Arc::new(Mutex::new(TenantBudgetEnforcer::new())),
            optimizer_registry: Arc::new(Mutex::new(OptimizerRegistry::new())),
            // BLUE70: Consolidated buses
            unified_knowledge_bus: Arc::new(RwLock::new(UnifiedKnowledgeBus::new())),
            reinforcement_bus: Arc::new(RwLock::new(ReinforcementBus::new())),
            learning_optimization_bus: Arc::new(RwLock::new(LearningOptimizationBus::new())),
            #[cfg(feature = "sub-bus-tool")]
            tool_bus: ToolBus::new(
                crate::acp::r#impl::request::tools_pack::global_tool_registry(),
                Arc::new(RwLock::new(
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
            // BLUE56-GAP-B02: Uses the global-shared singleton so inner state
                        // (observations, actions, reports) is shared across the system.
                        // `with_metacognitive_llm()` builder method sets the per-instance llm_agent.
                        metacognitive: crate::intelligence::metacognitive::shared_metacognitive_controller(),
            world_model: WorldModel::new(Default::default()),
            self_model: SelfModelCore::new(Default::default()),
            federated_rl: FederatedRL::new(Default::default()),
            matcher: ScenarioMatcher::default(),
            discovery: DiscoveryCenter::new(),
            consensus: ConsensusEngine::new(Default::default()),
            #[cfg(any(
                feature = "sub-bus-tool",
                feature = "simple-server",
                feature = "multi-users-server"
            ))]
            agent_factory: Arc::new(Mutex::new(AgentFactory::new(AgentFactoryConfig::default()))),
            #[cfg(any(
                feature = "sub-bus-tool",
                feature = "simple-server",
                feature = "multi-users-server"
            ))]
            council: {
                let council = Arc::new(Mutex::new(OrchestrationCouncil::new(
                    CouncilConfig::default(),
                )));
                OrchestrationCouncil::start_auto_ejection(council.clone());
                council
            },
            evolution_graph: Arc::new(Mutex::new(EvolutionGraph::new())),
            continuous_learning: Arc::new(Mutex::new(ContinuousLearningCenter::new(
                Default::default(),
            ))),
            transport: Arc::new(Mutex::new(MultiChannelTransport::new(Default::default()))),
            token_cache: None,
            model_selector: None,
            federated_learning: None,
            live_performance: None,
            hot_failover: None,
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
            harness.clone(),
            CapabilityGraph::new(),
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

    /// Import remote skills from the given endpoint/skill-name pairs.
    ///
    /// Each entry is a `(endpoint, skill_name)` tuple. This is only available
    /// under the `multi-users-server` feature flag.
    #[cfg(feature = "multi-users-server")]
    pub fn with_remote_skills(self, skills: &[(&str, &str)]) -> Self {
        for (endpoint, skill_name) in skills {
            crate::intelligence::capability_bus::tool_bus::import_remote_skill(
                &self.tool_bus,
                endpoint,
                skill_name,
            )
            .unwrap_or_else(|e| {
                tracing::warn!(
                    "Failed to import remote skill {} from {}: {}",
                    skill_name,
                    endpoint,
                    e
                );
            });
        }
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

    /// Attach a TokenMultiLevelCache to the CapabilityBus (P2-1).
    pub fn with_token_cache(mut self, cache: Arc<TokenMultiLevelCache>) -> Self {
        self.token_cache = Some(cache);
        self
    }

    /// Attach an AdaptiveModelSelector to the CapabilityBus (P2-3).
    pub fn with_model_selector(mut self, selector: AdaptiveModelSelector) -> Self {
        self.model_selector = Some(Mutex::new(selector));
        self
    }

    /// Attach a FederatedLearning coordinator to the CapabilityBus (P2-4).
    pub fn with_federated_learning(mut self, fl: Arc<Mutex<FederatedLearning>>) -> Self {
        self.federated_learning = Some(fl);
        self
    }

    /// Attach a pre-populated capability graph (shared with AgentRegistry)
    /// so the capability bus sees all registered agents for candidate selection.
    /// Without this, agents_from_config are invisible to decide().
    pub fn with_capability_graph(mut self, graph: Arc<Mutex<CapabilityGraph>>) -> Self {
        self.capability_graph = graph;
        self
    }

    /// Attach a LivePerformanceFeed to the CapabilityBus
    pub fn with_live_performance(mut self, feed: Arc<LivePerformanceFeed>) -> Self {
        self.live_performance = Some(feed);
        self
    }

    /// Attach a HotFailover manager to the CapabilityBus (P2-7).
    pub fn with_hot_failover(mut self, hf: Arc<HotFailover>) -> Self {
        self.hot_failover = Some(hf);
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

    pub(crate) fn action_outcome_label(success: bool) -> &'static str {
        if success {
            "success"
        } else {
            "failure"
        }
    }

    pub(crate) fn build_action_blocked_detail(tool_name: &str, reason: &str) -> Value {
        serde_json::json!({
            "schema": "capability-bus-action-v1",
            "tool": tool_name,
            "duration_ms": 0,
            "logical_success": false,
            "error": reason,
            "policy_blocked": true,
        })
    }

    pub(crate) fn build_action_event_detail(
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

    pub(crate) fn build_feedback_event_detail(
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
    // Observability helpers
    // ------------------------------------------------------------------

    pub fn snapshot_events(&self) -> Vec<BusEvent> {
        read_guard(&self.event_history).iter().cloned().collect()
    }

    pub fn capability_bus_profile(&self) -> CapabilityBusProfile {
        let mut p = write_guard(&self.profile);
        // BLUE70: Read from consolidated buses
        {
            let ukb = read_guard(&self.unified_knowledge_bus);
            p.reputation_agents_count = ukb.reputation_count();
            p.knowledge_insights_count = ukb.insight_count();
            p.experience_case_count = ukb.experience_count();
        }
        {
            let rb = read_guard(&self.reinforcement_bus);
            p.q_learning_table_size = rb.table_size();
        }
        {
            let lob = read_guard(&self.learning_optimization_bus);
            p.learning_events_count = lob.event_count();
        }
        p.capability_graph_agents = lock_guard(&self.capability_graph).total_agents();
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

        // BLUE70: Consolidated bus profile metrics
        {
            let ukb = read_guard(&self.unified_knowledge_bus);
            p.unified_knowledge_insight_count = ukb.insight_count();
            p.unified_knowledge_experience_count = ukb.experience_count();
        }
        {
            let rb = read_guard(&self.reinforcement_bus);
            p.reinforcement_table_size = rb.table_size();
        }
        {
            let lob = read_guard(&self.learning_optimization_bus);
            p.learning_optimization_event_count = lob.event_count();
        }

        // Skill evolution metrics
        #[cfg(feature = "sub-bus-tool")]
        {
            let skills = read_guard(self.tool_bus.skill_registry_ref());
            p.skill_evolution_count = skills
                .evolution_history
                .values()
                .map(|v| v.len())
                .sum::<usize>() as u32;
        }

        #[cfg(any(
            feature = "sub-bus-tool",
            feature = "simple-server",
            feature = "multi-users-server"
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

    // ------------------------------------------------------------------
    // Stage 5: Evolution — reinforcement learning update (BLUE48 Step 1.2)
    // ------------------------------------------------------------------

    // ── evolve() subsystem methods ──────────────────────────────────────
    // Each method is < 100 lines, handles its own errors via warn!(), and
    // respects a combined deadline to prevent any single subsystem from
    // exceeding its ~100 ms budget.
    // ------------------------------------------------------------------

    /// Record drift metrics through HarnessBus drift engine.
    pub(crate) fn evolve_drift_protection(&self, quality_score: f64, success: bool) {
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
    pub(crate) async fn evolve_fault_tolerance(&self, node_id: &str) {
        if let Err(e) = self.harness.fault_tolerance.register_node(node_id).await {
            warn!(
                "evolve_fault_tolerance: register_node failed for {}: {:?}",
                node_id, e
            );
        }
        if let Err(e) = self.harness.fault_tolerance.report_heartbeat(node_id).await {
            warn!(
                "evolve_fault_tolerance: report_heartbeat failed for {}: {:?}",
                node_id, e
            );
        }
    }

    /// Record an audit entry for the evolve cycle.
    pub(crate) fn evolve_harness_bus(
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
        let timeout_dur = Duration::from_millis(self.config.subsystem_timeout_ms);
        let reward = match timeout(timeout_dur, async {
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

        if timeout(timeout_dur, async {
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
        if timeout(timeout_dur, async {
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
        if timeout(timeout_dur, async {
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
        if timeout(timeout_dur, self.evolve_fault_tolerance(&node_id))
            .await
            .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_fault_tolerance timed out — skipping");
        }

        if timeout(timeout_dur, async {
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
        if timeout(timeout_dur, async {
            self.evolve_federated_rl(state, action, reward, quality_score, success)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_federated_rl timed out — skipping");
        }

        if timeout(timeout_dur, async {
            self.evolve_continuous_learning(state, action, reward, success, quality_score)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_continuous_learning timed out — skipping");
        }

        if timeout(timeout_dur, async {
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
        if timeout(timeout_dur, async {
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
        if timeout(timeout_dur, async {
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
        if timeout(timeout_dur, async {
            self.evolve_evolution_graph(state, action, success, quality_score)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_evolution_graph timed out — skipping");
        }

        if timeout(timeout_dur, async { self.evolve_self_model(now, success) })
            .await
            .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_self_model timed out — skipping");
        }

        if timeout(timeout_dur, async {
            self.evolve_consciousness(state, action, quality_score, success)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_consciousness timed out — skipping");
        }

        if timeout(timeout_dur, async {
            self.evolve_world_model(action, state, reward)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_world_model timed out — skipping");
        }

        // ── BLUE70: Read Q-value and exploration rate from ReinforcementBus ──
        let (q_value, exploration_rate) = timeout(timeout_dur, async {
            let rb = read_guard(&self.reinforcement_bus);
            let qv = rb.best_q_value(&state.0);
            let er = 0.1; // default exploration rate; ReinforcementBus manages its own
            drop(rb);
            (qv, er)
        })
        .await
        .unwrap_or_else(|_| {
            warn!("evolve: reinforcement_bus lock timed out — using defaults");
            (0.0, 0.0)
        });

        if timeout(timeout_dur, async {
            self.evolve_send_transport_event(q_value, exploration_rate)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: transport.send_event timed out — skipping");
        }

        if timeout(timeout_dur, async {
            self.evolve_consensus(state, action, reward, q_value, success, now)
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: evolve_consensus timed out — skipping");
        }

        // ── P2-4: Federated learning aggregation — merge client policies ──
        if let Some(ref federated) = self.federated_learning {
            if timeout(timeout_dur, async {
                match federated.lock() {
                    Ok(mut fl) => {
                        // Only aggregate if enough clients have contributed
                        if fl.pending_weights_count() >= fl.min_clients_required() {
                            match fl.aggregate_round() {
                                Ok(round) => {
                                    tracing::info!(
                                        "evolve: federated aggregation round {} completed with {} clients",
                                        round.round_id,
                                        round.clients_participated.len(),
                                    );
                                }
                                Err(e) => {
                                    tracing::debug!(
                                        "evolve: federated aggregation skipped: {}",
                                        e
                                    );
                                }
                            }
                        }
                    }
                    Err(poisoned) => {
                        warn!("evolve: federated_learning lock poisoned");
                        drop(poisoned.into_inner());
                    }
                }
            })
            .await
            .is_err()
            {
                self.evolve_timeout_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                warn!("evolve: federated_learning timed out — skipping");
            }
        }

        // ── P2-5: Metacognitive autoreflect ──────────────────────────────
        if timeout(timeout_dur, async {
            let report_ids = self.metacognitive.autoreflect();
            if !report_ids.is_empty() {
                tracing::info!(
                    "evolve: metacognitive autoreflect generated {} report(s): {:?}",
                    report_ids.len(),
                    report_ids
                );
            }
        })
        .await
        .is_err()
        {
            self.evolve_timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!("evolve: metacognitive.autoreflect timed out — skipping");
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
}

// ---------------------------------------------------------------------------
// Stage output types
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "sub-bus-tool"))]
pub(crate) mod tests {
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

        let result = bus.execute_tool("read_file", &input).await;
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
        let total = weights.reputation
            + weights.recency
            + weights.task_fit
            + weights.recent_outcome
            + weights.causal_insight;
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
        let total = weights.reputation
            + weights.recency
            + weights.task_fit
            + weights.recent_outcome
            + weights.causal_insight;
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
                        consecutive_failures: 0,
                        last_updated_ms: 0,
                    })
                    .collect::<Vec<_>>()
            })
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
            let mut graph = bus
                .capability_graph
                .lock()
                .expect("capability_graph lock should not be poisoned");
            register_test_agent(&mut graph, "security-auditor", vec!["security"]);
            register_test_agent(&mut graph, "general-coder", vec!["general"]);
            register_test_agent(&mut graph, "fix-specialist", vec!["bugfix", "general"]);
        }

        {
            let mut ukb = bus
                .unified_knowledge_bus
                .write()
                .expect("unified_knowledge_bus lock should not be poisoned");
            ukb.record_outcome("security-auditor", "test", true, "test setup".to_string());
            ukb.record_outcome("general-coder", "test", true, "test setup".to_string());
            ukb.record_outcome("general-coder", "test", true, "test setup".to_string());
            ukb.record_outcome("general-coder", "test", true, "test setup".to_string());
            ukb.record_outcome("fix-specialist", "test", true, "test setup".to_string());
            ukb.record_outcome("fix-specialist", "test", true, "test setup".to_string());
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
            let mut graph = bus
                .capability_graph
                .lock()
                .expect("capability_graph lock should not be poisoned");
            register_test_agent(&mut graph, "refactor-expert", vec!["refactor", "general"]);
            register_test_agent(&mut graph, "general-coder", vec!["general"]);
            register_test_agent(&mut graph, "debugger", vec!["bugfix", "general"]);
        }

        {
            let mut ukb = bus
                .unified_knowledge_bus
                .write()
                .expect("unified_knowledge_bus lock should not be poisoned");
            ukb.record_outcome("refactor-expert", "test", true, "test setup".to_string());
            ukb.record_outcome("general-coder", "test", true, "test setup".to_string());
            ukb.record_outcome("debugger", "test", true, "test setup".to_string());
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
            let mut graph = bus
                .capability_graph
                .lock()
                .expect("capability_graph lock should not be poisoned");
            register_test_agent(&mut graph, "test-agent", vec!["general"]);
        }

        {
            let mut ukb = bus
                .unified_knowledge_bus
                .write()
                .expect("unified_knowledge_bus lock should not be poisoned");
            ukb.record_outcome("test-agent", "test", true, "test setup".to_string());
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
            let mut graph = bus
                .capability_graph
                .lock()
                .expect("capability_graph lock should not be poisoned");
            register_test_agent(&mut graph, "solo-agent", vec!["general"]);
        }
        {
            let mut ukb = bus
                .unified_knowledge_bus
                .write()
                .expect("unified_knowledge_bus lock should not be poisoned");
            ukb.record_outcome("solo-agent", "test", true, "test setup".to_string());
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
            let mut graph = bus
                .capability_graph
                .lock()
                .expect("capability_graph lock should not be poisoned");
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
