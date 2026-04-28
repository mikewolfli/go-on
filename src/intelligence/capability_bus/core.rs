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

use crate::governance::hardening::TenantBudgetEnforcer;
use crate::governance::harness_bus::{AgentExecutionPolicy, HarnessBus, PolicyVerdict};
use crate::governance::pua::TaskContext;
use crate::intelligence::capability_bus::distributed_memory_bus::DistributedMemoryBus;
use crate::intelligence::capability_bus::memory_bus::MemoryBus;
use crate::intelligence::capability_bus::observability_bus::ObservabilityBus;
use crate::intelligence::capability_bus::optimization_bus::OptimizationBus;
use crate::intelligence::capability_bus::orchestration_bus::OrchestrationBus;
use crate::intelligence::capability_bus::protocol_bus::ProtocolBus;
use crate::intelligence::capability_bus::tool_bus::ToolBus;
use crate::intelligence::capability_graph::CapabilityGraph;
use crate::intelligence::reinforcement::learning::{
    ExperienceKnowledgeBase, QLearningAgent, RewardFunction, RlTaskExecutionMetrics, SuccessCase,
};
use crate::intelligence::reputation::ReputationStore;
use crate::observability::provenance::{make_entry, ProvenanceLedger};
use crate::orchestration::task_schema::SchemaRegistry;
use crate::orchestration::workflow_optimizer::OptimizerRegistry;
use crate::orchestration::workflow_registry::WorkflowRegistry;

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
    pub q_learning_table_size: usize,
    pub experience_case_count: usize,
    pub event_history_len: usize,
    pub workflow_presets_count: usize,
    pub provenance_entries_count: usize,
    // Phase 4 sub-bus metrics
    pub tool_bus_tools: u32,
    pub tool_bus_skills: u32,
    pub tool_bus_calls: u64,
    pub observability_tracked_agents: u32,
    pub observability_system_error_rate: f64,
    pub optimization_total: u64,
    pub optimization_circuit_breaker_trips: u64,
    pub protocol_active_transport: String,
    pub protocol_healthy_count: u32,
    pub orchestration_active_flows: u32,
    pub orchestration_available_modes: u32,
    pub memory_cache_hit_rate: f64,
    pub memory_total_entries: u32,
    pub distributed_memory_peers: u32,
    pub distributed_memory_shared: u32,
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
            tool_bus_tools: 0,
            tool_bus_skills: 0,
            tool_bus_calls: 0,
            observability_tracked_agents: 0,
            observability_system_error_rate: 0.0,
            optimization_total: 0,
            optimization_circuit_breaker_trips: 0,
            protocol_active_transport: "auto".to_string(),
            protocol_healthy_count: 0,
            orchestration_active_flows: 0,
            orchestration_available_modes: 0,
            memory_cache_hit_rate: 0.0,
            memory_total_entries: 0,
            distributed_memory_peers: 0,
            distributed_memory_shared: 0,
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
    pub tool_bus: ToolBus,

    /// ObservabilityBus — unified trace/metric/audit coordination
    pub observability_bus: ObservabilityBus,

    /// OptimizationBus — cost, speed, reliability optimization coordination
    pub optimization_bus: OptimizationBus,

    /// MemoryBus — unified cache coordination (L1 memory → L2 SQLite → L3 vector)
    pub memory_bus: MemoryBus,

    /// ProtocolBus — protocol-aware routing and health tracking
    pub protocol_bus: ProtocolBus,

    /// OrchestrationBus — unified flow/task/mode coordination
    pub orchestration_bus: OrchestrationBus,

    /// DistributedMemoryBus — cross-node memory sharing
    pub distributed_memory_bus: DistributedMemoryBus,

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
            tool_bus: ToolBus::new(
                Arc::new(Mutex::new(crate::orchestration::tool::ToolRegistry::new())),
                Arc::new(Mutex::new(
                    crate::orchestration::skill::SkillRegistry::default(),
                )),
            ),
            observability_bus: ObservabilityBus::new(),
            optimization_bus: OptimizationBus::default(),
            memory_bus: MemoryBus::new(None, None, None, None),
            protocol_bus: ProtocolBus::new(),
            orchestration_bus: OrchestrationBus::new(None, None),
            distributed_memory_bus: DistributedMemoryBus::new(5000),
            max_event_history: 100,
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
    pub fn with_tool_bus(mut self, tool_bus: ToolBus) -> Self {
        self.tool_bus = tool_bus;
        self
    }

    /// Attach an ObservabilityBus to the CapabilityBus
    pub fn with_observability_bus(mut self, bus: ObservabilityBus) -> Self {
        self.observability_bus = bus;
        self
    }

    /// Attach an OptimizationBus to the CapabilityBus
    pub fn with_optimization_bus(mut self, bus: OptimizationBus) -> Self {
        self.optimization_bus = bus;
        self
    }

    /// Attach a MemoryBus to the CapabilityBus
    pub fn with_memory_bus(mut self, bus: MemoryBus) -> Self {
        self.memory_bus = bus;
        self
    }

    /// Attach a ProtocolBus to the CapabilityBus
    pub fn with_protocol_bus(mut self, bus: ProtocolBus) -> Self {
        self.protocol_bus = bus;
        self
    }

    /// Attach an OrchestrationBus to the CapabilityBus
    pub fn with_orchestration_bus(mut self, bus: OrchestrationBus) -> Self {
        self.orchestration_bus = bus;
        self
    }

    /// Attach a DistributedMemoryBus to the CapabilityBus
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
        let healthy = self.observability_bus.healthy_agents(0.5);

        // Phase 4: Query OrchestrationBus for available modes
        let modes = self.orchestration_bus.available_modes();

        // Phase 4: Get optimization recommendation
        let task_type_str = format!("{:?}", task.task_type);
        let token_estimate = (task.file_count * 512) as u64;
        let opt =
            self.optimization_bus
                .recommend(&task_type_str, token_estimate.max(1024), "balanced");

        SensingOutput {
            capability_agent_count: cap_agents,
            reputation_snapshot: rep_snapshot,
            recent_agents: _learning_rates,
            healthy_agents: healthy,
            available_modes: modes,
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
                    available_tools: vec![],
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
                    available_tools: vec![],
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

        // Step B2: Consult WorkflowRegistry for workflow-based routing metadata
        let workflow_preset = self.workflow_registry.as_ref().and_then(|wr| {
            wr.lock().ok().and_then(|registry| {
                let task_type_str = format!("{:?}", task.task_type).to_lowercase();
                let mapped_name = match task_type_str.as_str() {
                    "bugfix" | "featureadd" | "refactor" | "securitypatch" => "dev",
                    _ => "general",
                };
                registry.get(mapped_name).cloned()
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
        let task_type_str = format!("{:?}", task.task_type);
        let recommended_mode = self
            .orchestration_bus
            .recommend_mode(&task_type_str, task.risk_score);

        // Phase 4: Get available tools for the selected agent via ToolBus
        let available_tools = selected_agent
            .as_ref()
            .map(|agent| self.tool_bus.agent_tool_match(agent, &task_type_str))
            .unwrap_or_default();

        self.record_event(
            "decision",
            selected_agent.clone(),
            None,
            "success",
            serde_json::json!({
                "confidence": confidence,
                "recommended_mode": recommended_mode,
                "available_tools": available_tools.len(),
            }),
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
            recommended_mode,
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
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
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
        let result = self.tool_bus.execute_tool(tool_name, input);
        let duration_ms = start.elapsed().as_millis() as u64;
        let success = result.is_ok();

        // Step 3: Record execution in ObservabilityBus
        self.observability_bus.record_trace(
            "capability_bus",
            "tool_call",
            duration_ms,
            success,
            result.as_ref().err().map(|e| e.to_string()),
            0,
        );

        // Step 4: Record outcome in ToolBus
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
        if self.optimization_bus.is_circuit_broken(agent) {
            return false;
        }
        // Check error rate via ObservabilityBus
        if let Some(err_rate) = self.observability_bus.agent_error_rate(agent) {
            if err_rate.error_rate > 0.5 {
                return false;
            }
        }
        true
    }

    /// Get recommended execution mode via OrchestrationBus
    pub fn recommended_mode(&self, task_type: &str, complexity: f64) -> String {
        self.orchestration_bus.recommend_mode(task_type, complexity)
    }

    /// Get optimization recommendation for a task
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
        self.observability_bus.record_trace(
            agent,
            task_type,
            duration_ms,
            success,
            None,
            token_cost,
        );

        // 4. Write to OptimizationBus
        self.optimization_bus
            .record_execution(agent, duration_ms, token_cost, success);

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
            let tb = self.tool_bus.profile();
            p.tool_bus_tools = tb.total_tools;
            p.tool_bus_skills = tb.total_skills;
            p.tool_bus_calls = tb.total_calls;

            let ob = self.observability_bus.system_health();
            p.observability_tracked_agents = ob.tracked_agents;
            p.observability_system_error_rate = ob.system_error_rate;

            let opt = self.optimization_bus.profile();
            p.optimization_total = opt.total_optimizations;
            p.optimization_circuit_breaker_trips = opt.circuit_breaker_trips;

            let pb = self.protocol_bus.profile();
            p.protocol_active_transport = pb.active_transport;
            p.protocol_healthy_count = pb.healthy_protocols;

            let orb = self.orchestration_bus.profile();
            p.orchestration_active_flows = orb.active_flows;
            p.orchestration_available_modes = orb.available_modes;

            let mb = self.memory_bus.profile();
            p.memory_cache_hit_rate = mb.cache_hit_rate;
            p.memory_total_entries = mb.vector_docs_count + mb.memory_entries;

            let dmb = self.distributed_memory_bus.profile();
            p.distributed_memory_peers = dmb.remote_peers;
            p.distributed_memory_shared = dmb.shared_entries;

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
    pub healthy_agents: Vec<String>,
    /// Phase 4: available modes from OrchestrationBus
    pub available_modes: Vec<String>,
    /// Phase 4: optimization recommendation
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
    pub available_tools: Vec<String>,
}
