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
    Arc, Mutex,
};

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
    task_id: String,
}

// ── OrchestrationBus ────────────────────────────────────────────────────────

/// OrchestrationBus provides unified coordination across FlowManager, TaskRouter,
/// ExecutionGraph, ModeRuntime, and Scheduler.
///
/// This sub-bus tracks active flows, manages execution mode registration, and
/// recommends appropriate execution modes based on task type and complexity.
pub struct OrchestrationBus {
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
    /// * `flow_manager` - Optional shared reference to a `FlowManager`; retained
    ///   for API compatibility (the bus no longer reads flow-manager state)
    ///
    /// # Returns
    /// * `Self` - A new `OrchestrationBus` instance
    pub fn new(_flow_manager: Option<Arc<FlowManager>>) -> Self {
        Self {
            profile: Arc::new(Mutex::new(OrchestrationBusProfile {
                enabled: true,
                active_flows: 0,
                available_modes: 0,
                total_routes: 0,
            })),
            active_flow_map: Arc::new(Mutex::new(HashMap::new())),
            total_routes: Arc::new(AtomicU64::new(0)),
            max_flows: 500,
        }
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

        let entry = FlowEntry {
            task_id: task_id.to_string(),
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

    /// Return a snapshot of the current orchestration profile.
    ///
    /// # Returns
    /// * `OrchestrationBusProfile` - A copy of the current profile metrics
    pub fn profile(&self) -> OrchestrationBusProfile {
        let mut prof = crate::lock_or_recover!(self.profile.as_ref(), "intelligence");
        prof.total_routes = self.total_routes.load(Ordering::Relaxed);
        prof.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
