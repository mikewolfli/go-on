//! OrchestrationBus — Unified coordination sub-bus (BLUE38 §1, ARCH-13)
//!
//! OrchestrationBus provides unified coordination across FlowManager, TaskRouter,
//! ExecutionGraph, ModeRuntime, and Scheduler. It serves as the central orchestration
//! hub within the multi-bus architecture, enabling flow tracking, mode management,
//! and intelligent execution mode recommendation based on task characteristics.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, RwLock,
};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::orchestration::core_dag::{ExCondition, ExNode, ExNodeKind, ExecutionGraph};
use crate::orchestration::flow::FlowManager;

// ── Supporting types ────────────────────────────────────────────────────────

/// Profile snapshot of the OrchestrationBus at a given point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrationBusProfile {
    /// Whether orchestration is enabled
    pub enabled: bool,
    /// Number of currently active flows
    pub active_flows: u32,
    /// Number of registered execution modes
    pub available_modes: u32,
    /// Total routes processed across all flows
    pub total_routes: u64,
    /// Number of active execution graphs
    pub active_graphs: u32,
}

/// Status information for an active flow.
#[derive(Debug, Clone)]
pub struct FlowStatus {
    /// Name of the flow
    pub flow_name: String,
    /// Current execution phase
    pub phase: String,
    /// Optional agent currently handling the flow
    pub agent: Option<String>,
    /// Whether the flow is still active
    pub is_active: bool,
    /// Timestamp (epoch ms) when the flow started
    pub started_ms: u64,
}

/// Execution modes supported by the orchestration layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrchestrationMode {
    /// Interactive question-and-answer mode
    Ask,
    /// Direct edit mode for modifying existing code
    Edit,
    /// Autonomous agent execution mode
    Agent,
    /// Fully automated end-to-end execution
    FullAuto,
    /// Safety-constrained execution with guardrails
    SafeGuard,
}

impl OrchestrationMode {
    /// Returns the string representation of this mode.
    pub fn as_str(&self) -> &'static str {
        match self {
            OrchestrationMode::Ask => "ask",
            OrchestrationMode::Edit => "edit",
            OrchestrationMode::Agent => "agent",
            OrchestrationMode::FullAuto => "full_auto",
            OrchestrationMode::SafeGuard => "safe_guard",
        }
    }

    /// Parse an `OrchestrationMode` from its string representation.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "ask" => Some(OrchestrationMode::Ask),
            "edit" => Some(OrchestrationMode::Edit),
            "agent" => Some(OrchestrationMode::Agent),
            "full_auto" => Some(OrchestrationMode::FullAuto),
            "safe_guard" => Some(OrchestrationMode::SafeGuard),
            _ => None,
        }
    }
}

impl std::str::FromStr for OrchestrationMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match <Self>::from_str(s) {
            Some(mode) => Ok(mode),
            None => Err(format!("unknown OrchestrationMode: {s}")),
        }
    }
}

// ── Internal flow tracker ───────────────────────────────────────────────────

/// A single tracked flow entry used internally by the OrchestrationBus.
#[derive(Debug, Clone)]
struct FlowEntry {
    flow_name: String,
    task_id: String,
    phase: String,
    agent: Option<String>,
    started_ms: u64,
}

// ── OrchestrationBus ────────────────────────────────────────────────────────

/// OrchestrationBus provides unified coordination across FlowManager, TaskRouter,
/// ExecutionGraph, ModeRuntime, and Scheduler.
///
/// This sub-bus tracks active flows, manages execution mode registration, and
/// recommends appropriate execution modes based on task type and complexity.
pub struct OrchestrationBus {
    /// Flow manager reference
    flow_manager: Option<Arc<FlowManager>>,
    /// Execution graph
    execution_graph: Arc<Mutex<ExecutionGraph>>,
    /// Available modes
    available_modes: Arc<RwLock<Vec<String>>>,
    /// Profile metrics
    profile: Arc<Mutex<OrchestrationBusProfile>>,
    /// Active flow tracking (flow_name -> FlowEntry)
    active_flow_map: Arc<Mutex<HashMap<String, FlowEntry>>>,
    /// Total routes counter (persisted across resets)
    total_routes: Arc<AtomicU64>,
    /// Maximum number of tracked flows before FIFO eviction
    max_flows: usize,
}

impl OrchestrationBus {
    /// Create a new `OrchestrationBus`.
    ///
    /// # Arguments
    /// * `flow_manager` - Optional shared reference to a `FlowManager`
    ///
    /// # Returns
    /// * `Self` - A new `OrchestrationBus` instance
    pub fn new(flow_manager: Option<Arc<FlowManager>>) -> Self {
        // Create a minimal default ExecutionGraph for initialization.
        let graph = ExecutionGraph::new("orchestration_root");

        Self {
            flow_manager,
            execution_graph: Arc::new(Mutex::new(graph)),
            available_modes: Arc::new(RwLock::new(Vec::new())),
            profile: Arc::new(Mutex::new(OrchestrationBusProfile {
                enabled: true,
                active_flows: 0,
                available_modes: 0,
                total_routes: 0,
                active_graphs: 0,
            })),
            active_flow_map: Arc::new(Mutex::new(HashMap::new())),
            total_routes: Arc::new(AtomicU64::new(0)),
            max_flows: 500,
        }
    }

    /// Register an execution mode string.
    ///
    /// # Arguments
    /// * `mode` - The mode name to register
    pub fn register_mode(&self, mode: &str) {
        // Parse known standard modes to keep enum conversion path active.
        let _ = OrchestrationMode::from_str(mode);
        let mut modes = crate::write_or_recover!(self.available_modes.as_ref(), "intelligence");
        let mode_str = mode.to_string();
        if !modes.contains(&mode_str) {
            modes.push(mode_str);
        }
        // Update profile
        let mut prof = crate::lock_or_recover!(self.profile.as_ref(), "intelligence");
        prof.available_modes = modes.len() as u32;
    }

    /// List all available execution modes.
    ///
    /// # Returns
    /// * `Vec<String>` - A sorted copy of registered mode names
    pub fn available_modes(&self) -> Vec<String> {
        let modes = crate::read_or_recover!(self.available_modes.as_ref(), "intelligence");
        let mut result = modes.clone();
        result.sort();
        result
    }

    /// Recommend an execution mode based on task type and complexity.
    ///
    /// Uses a simple heuristic:
    /// - High complexity (> 7.0) or safety concerns -> SafeGuard
    /// - Bug fixes / medium complexity (4.0–7.0) -> Agent
    /// - Simple questions / low complexity (< 2.0) -> Ask
    /// - Feature implementation / moderate complexity (2.0–4.0) -> Edit
    /// - Everything else -> FullAuto
    ///
    /// # Arguments
    /// * `task_type` - The type of task to route
    /// * `complexity` - Task complexity score (0.0–10.0)
    ///
    /// # Returns
    /// * `String` - The recommended mode name
    pub fn recommend_mode(&self, task_type: &str, complexity: f64) -> String {
        let lower_type = task_type.to_lowercase();

        // Safety-critical or highly complex tasks
        if complexity > 7.0
            || lower_type.contains("security")
            || lower_type.contains("safety")
            || lower_type.contains("critical")
        {
            return OrchestrationMode::SafeGuard.as_str().to_string();
        }

        // Bug fixes and troubleshooting
        if lower_type.contains("bug")
            || lower_type.contains("fix")
            || lower_type.contains("error")
            || lower_type.contains("defect")
        {
            if complexity > 4.0 {
                return OrchestrationMode::Agent.as_str().to_string();
            }
            return OrchestrationMode::Edit.as_str().to_string();
        }

        // Simple queries
        if complexity < 2.0
            || lower_type.contains("question")
            || lower_type.contains("explain")
            || lower_type.contains("what is")
        {
            return OrchestrationMode::Ask.as_str().to_string();
        }

        // Feature implementation or moderate complexity
        if lower_type.contains("feature")
            || lower_type.contains("implement")
            || lower_type.contains("add")
            || (2.0..4.0).contains(&complexity)
        {
            return OrchestrationMode::Edit.as_str().to_string();
        }

        // Medium complexity — delegate to agent
        if (4.0..7.0).contains(&complexity) {
            return OrchestrationMode::Agent.as_str().to_string();
        }

        // Default to full auto for everything else
        OrchestrationMode::FullAuto.as_str().to_string()
    }

    /// Start tracking a new flow.
    ///
    /// # Arguments
    /// * `flow_name` - The name of the flow to start
    /// * `task_id` - The task identifier for this flow instance
    ///
    /// # Returns
    /// * `Result<()>` - Ok if the flow was started, Err if it is already active
    pub fn start_flow(&self, flow_name: &str, task_id: &str) -> Result<()> {
        let mut flow_map = crate::lock_or_recover!(self.active_flow_map.as_ref(), "intelligence");

        if flow_map.contains_key(flow_name) {
            return Err(anyhow!(
                "Flow '{}' is already active with task '{}'",
                flow_name,
                flow_map
                    .get(flow_name)
                    .map(|e| e.task_id.as_str())
                    .unwrap_or("unknown")
            ));
        }

        // Evict oldest flow when at capacity.
        if flow_map.len() >= self.max_flows {
            if let Some(oldest) = flow_map.keys().next().cloned() {
                flow_map.remove(&oldest);
            }
        }

        let phase = self
            .flow_manager
            .as_ref()
            .map(|fm| fm.default_phase().to_string())
            .unwrap_or_else(|| "default".to_string());

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let entry = FlowEntry {
            flow_name: flow_name.to_string(),
            task_id: task_id.to_string(),
            phase,
            agent: None,
            started_ms: now_ms,
        };

        flow_map.insert(flow_name.to_string(), entry);

        // Update profile
        let mut prof = crate::lock_or_recover!(self.profile.as_ref(), "intelligence");
        prof.active_flows = flow_map.len() as u32;

        Ok(())
    }

    /// Complete (remove) a tracked flow.
    ///
    /// # Arguments
    /// * `flow_name` - The name of the flow to complete
    /// * `task_id` - The task identifier to verify against
    pub fn complete_flow(&self, flow_name: &str, task_id: &str) {
        let mut flow_map = crate::lock_or_recover!(self.active_flow_map.as_ref(), "intelligence");

        if let Some(entry) = flow_map.get(flow_name) {
            if entry.task_id == task_id {
                flow_map.remove(flow_name);
            }
        }

        // Update profile
        let mut prof = crate::lock_or_recover!(self.profile.as_ref(), "intelligence");
        prof.active_flows = flow_map.len() as u32;

        // Increment total routes (lock-free atomic counter)
        self.total_routes.fetch_add(1, Ordering::Relaxed);
    }

    /// List all active flows with their status.
    ///
    /// # Returns
    /// * `Vec<FlowStatus>` - Status information for each active flow
    pub fn active_flows(&self) -> Vec<FlowStatus> {
        let flow_map = crate::lock_or_recover!(self.active_flow_map.as_ref(), "intelligence");

        flow_map
            .values()
            .map(|entry| FlowStatus {
                flow_name: entry.flow_name.clone(),
                phase: entry.phase.clone(),
                agent: entry.agent.clone(),
                is_active: true,
                started_ms: entry.started_ms,
            })
            .collect()
    }

    /// Return a snapshot of the current orchestration profile.
    ///
    /// # Returns
    /// * `OrchestrationBusProfile` - A copy of the current profile metrics
    pub fn profile(&self) -> OrchestrationBusProfile {
        let mut prof = crate::lock_or_recover!(self.profile.as_ref(), "intelligence");
        prof.total_routes = self.total_routes.load(Ordering::Relaxed);
        prof.clone()
    }

    /// Queue a task node in the execution graph, connected from `predecessor`.
    ///
    /// Returns the IDs of nodes that are now ready to run.
    pub fn queue_graph_task(
        &self,
        task_id: &str,
        task_name: &str,
        predecessor: &str,
    ) -> Vec<String> {
        let mut graph = crate::lock_or_recover!(self.execution_graph.as_ref(), "intelligence");
        let node = ExNode::new(task_id, ExNodeKind::Task, task_name);
        graph.add_node(node);
        graph.add_edge(predecessor, task_id, None);
        graph.get_ready_nodes()
    }

    /// Add a conditional branch in the execution graph.
    ///
    /// The `ExCondition::Always` guard is used for unconditional pass-through.
    pub fn add_graph_condition(
        &self,
        cond_id: &str,
        predecessor: &str,
        true_target: &str,
        false_target: &str,
    ) {
        let mut graph = crate::lock_or_recover!(self.execution_graph.as_ref(), "intelligence");
        graph.add_condition(
            cond_id,
            cond_id,
            ExCondition::Always,
            predecessor,
            true_target,
            false_target,
        );
    }

    /// Evaluate a condition against current node outputs.
    ///
    /// Returns true if the condition passes; always true for `ExCondition::Always`.
    pub fn evaluate_condition(&self, condition: &ExCondition) -> bool {
        condition.evaluate(&std::collections::HashMap::new())
    }

    /// Mark a task node as complete and record its output.
    pub fn complete_graph_task(&self, task_id: &str, output: serde_json::Value) -> Result<bool> {
        let mut graph = crate::lock_or_recover!(self.execution_graph.as_ref(), "intelligence");
        graph.complete_task(task_id, output)?;
        Ok(graph.is_complete())
    }

    /// Create a fan-out group in the execution graph.
    pub fn add_graph_fan_out(
        &self,
        branch_name: &str,
        join_name: &str,
        parallel_tasks: Vec<(String, String)>,
        predecessor: &str,
    ) -> Result<(String, String)> {
        let mut graph = crate::lock_or_recover!(self.execution_graph.as_ref(), "intelligence");
        graph.add_fan_out(branch_name, join_name, parallel_tasks, predecessor)
    }

    /// Set a node's state in the execution graph.
    pub fn set_graph_node_state(
        &self,
        id: &str,
        state: crate::orchestration::core_dag::ExNodeState,
    ) -> Result<()> {
        crate::lock_or_recover!(self.execution_graph.as_ref(), "intelligence").set_node_state(id, state)
    }

    /// Check if a fan-out group is complete.
    pub fn is_fan_out_complete(&self, group_id: &str) -> bool {
        crate::lock_or_recover!(self.execution_graph.as_ref(), "intelligence").is_fan_out_complete(group_id)
    }

    /// Count graph nodes in a given state.
    pub fn count_graph_nodes_by_state(
        &self,
        state: &crate::orchestration::core_dag::ExNodeState,
    ) -> usize {
        crate::lock_or_recover!(self.execution_graph.as_ref(), "intelligence").count_by_state(state)
    }

    /// Summary of fan-out groups: (group_id, completed_count, total_count).
    pub fn graph_fan_out_summary(&self) -> Vec<(String, usize, usize)> {
        crate::lock_or_recover!(self.execution_graph.as_ref(), "intelligence").fan_out_summary()
    }

    /// Reset the execution graph for reuse.
    pub fn reset_graph(&self) {
        crate::lock_or_recover!(self.execution_graph.as_ref(), "intelligence").reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_list_modes() {
        let bus = OrchestrationBus::new(None);
        assert!(bus.available_modes().is_empty());

        bus.register_mode("ask");
        bus.register_mode("agent");
        bus.register_mode("ask"); // duplicate — should be ignored

        let modes = bus.available_modes();
        assert_eq!(modes.len(), 2);
        assert!(modes.contains(&"agent".to_string()));
        assert!(modes.contains(&"ask".to_string()));
    }

    #[test]
    fn test_start_and_complete_flow() {
        let bus = OrchestrationBus::new(None);

        assert!(bus.start_flow("test-flow", "task-001").is_ok());
        assert_eq!(bus.active_flows().len(), 1);

        // Duplicate start should fail
        assert!(bus.start_flow("test-flow", "task-002").is_err());
        assert_eq!(bus.active_flows().len(), 1);

        // Complete with wrong task_id should not remove the flow
        bus.complete_flow("test-flow", "wrong-task");
        assert_eq!(bus.active_flows().len(), 1);

        // Complete with correct task_id should remove it
        bus.complete_flow("test-flow", "task-001");
        assert_eq!(bus.active_flows().len(), 0);
    }

    #[test]
    fn test_recommend_mode() {
        let bus = OrchestrationBus::new(None);

        // Safety-critical tasks
        let mode = bus.recommend_mode("security audit", 3.0);
        assert_eq!(mode, "safe_guard");

        // High complexity
        let mode = bus.recommend_mode("refactor", 8.5);
        assert_eq!(mode, "safe_guard");

        // Bug fixes
        let mode = bus.recommend_mode("bug fix", 3.0);
        assert_eq!(mode, "edit");

        // Complex bug fix
        let mode = bus.recommend_mode("bug", 5.0);
        assert_eq!(mode, "agent");

        // Simple questions
        let mode = bus.recommend_mode("question", 1.0);
        assert_eq!(mode, "ask");

        // Feature implementation
        let mode = bus.recommend_mode("feature", 3.0);
        assert_eq!(mode, "edit");

        // Default fallback
        let mode = bus.recommend_mode("unknown", 5.5);
        assert_eq!(mode, "agent");

        // Full auto for high-complexity unknown
        let mode = bus.recommend_mode("maintenance", 7.5);
        assert_eq!(mode, "safe_guard");
    }

    #[test]
    fn test_profile_updates() {
        let bus = OrchestrationBus::new(None);

        assert_eq!(bus.profile().available_modes, 0);
        bus.register_mode("ask");
        bus.register_mode("agent");
        bus.register_mode("edit");
        assert_eq!(bus.profile().available_modes, 3);

        bus.start_flow("flow-a", "t1").expect("start flow-a");
        bus.start_flow("flow-b", "t2").expect("start flow-b");
        assert_eq!(bus.profile().active_flows, 2);

        bus.complete_flow("flow-a", "t1");
        assert_eq!(bus.profile().active_flows, 1);
        assert_eq!(bus.profile().total_routes, 1);
    }

    #[test]
    fn test_active_flows_output() {
        let bus = OrchestrationBus::new(None);

        bus.start_flow("alpha", "t1").expect("start flow alpha");
        bus.start_flow("beta", "t2").expect("start flow beta");

        let flows = bus.active_flows();
        assert_eq!(flows.len(), 2);

        for flow in &flows {
            assert!(flow.is_active);
            assert!(flow.started_ms > 0);
            assert!(!flow.phase.is_empty());
        }

        let names: Vec<&str> = flows.iter().map(|f| f.flow_name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
    }
}
