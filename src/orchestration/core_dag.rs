//! Orchestration DAG types.
//!
//! This module provides the **ExecutionGraph** — a DAG used to express
//! planner-produced execution plans as a node graph whose readiness can be
//! queried (`get_ready_nodes`) for observability payloads and future
//! DAG-driven execution.
//!
//! Fan-out/join, conditional branching (`ExCondition`), `TaskContext` and
//! `TaskGraph` were removed in the 2026-08-09 deep-scan cleanup: the fan-out
//! and condition machinery had zero production callers (only the basic
//! add_node/add_edge/readiness surface is consumed by
//! `planner_execution_graph`), and `TaskContext` was superseded by the
//! `core_dag::TaskContext`-independent chain-of-thought handling in
//! `workflow_registry`. Keeping only the consumed surface eliminates dead code
//! (principle §11).

#[cfg(test)]
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};

use crate::orchestration::planner_execution_graph::PlannerExecutionBridge;

// =========================================================================
// Execution Graph — DAG with readiness tracking
// =========================================================================

/// Execution graph node ID
pub type ExNodeId = String;

/// Kind of execution graph node
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ExNodeKind {
    /// Standard execution step
    Task,
    /// Entry point (single root)
    Start,
    /// Terminal node (single end)
    End,
}

/// Node execution state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExNodeState {
    Pending,
    Running,
    Completed,
    Failed(String),
    Skipped,
}

/// A node in the execution graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExNode {
    pub id: ExNodeId,
    pub kind: ExNodeKind,
    pub name: String,
    pub state: ExNodeState,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    /// Node cost estimate (for scheduling decisions)
    pub cost_estimate: f64,
    /// Error message if failed
    pub error: Option<String>,
    /// Execution duration in ms
    pub duration_ms: Option<u64>,
}

impl ExNode {
    pub fn new(id: &str, kind: ExNodeKind, name: &str) -> Self {
        Self {
            id: id.to_string(),
            kind,
            name: name.to_string(),
            state: ExNodeState::Pending,
            input: serde_json::Value::Null,
            output: None,
            cost_estimate: 1.0,
            error: None,
            duration_ms: None,
        }
    }
}

/// Directed edge between two nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExEdge {
    pub from: ExNodeId,
    pub to: ExNodeId,
    /// Optional label (e.g. "true" / "false" for Condition nodes)
    pub label: Option<String>,
}

/// Execution graph — a DAG whose nodes can be queried for execution readiness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionGraph {
    pub nodes: HashMap<ExNodeId, ExNode>,
    pub edges: Vec<ExEdge>,
    pub start_node: ExNodeId,
    pub end_node: ExNodeId,
    pub name: String,
}

impl ExecutionGraph {
    /// Create a new execution graph with Start and End nodes.
    pub fn new(name: &str) -> Self {
        let start_id = "start".to_string();
        let end_id = "end".to_string();
        let mut nodes = HashMap::with_capacity(16);
        let mut start_node = ExNode::new(&start_id, ExNodeKind::Start, "Start");
        start_node.state = ExNodeState::Completed;
        nodes.insert(start_id.clone(), start_node);
        nodes.insert(end_id.clone(), ExNode::new(&end_id, ExNodeKind::End, "End"));

        Self {
            nodes,
            edges: Vec::with_capacity(8),
            start_node: start_id,
            end_node: end_id,
            name: name.to_string(),
        }
    }

    /// Add a node to the graph, returning its ID.
    pub fn add_node(&mut self, node: ExNode) -> ExNodeId {
        let id = node.id.clone();
        self.nodes.insert(id.clone(), node);
        id
    }

    /// Add a directed edge.
    pub fn add_edge(&mut self, from: &str, to: &str, label: Option<&str>) {
        self.edges.push(ExEdge {
            from: from.to_string(),
            to: to.to_string(),
            label: label.map(|s| s.to_string()),
        });
    }

    /// Get nodes whose dependencies are all satisfied (ready to execute).
    /// Returns `Task` nodes whose dependencies are satisfied.
    pub fn get_ready_nodes(&self) -> Vec<ExNodeId> {
        let completed: HashSet<&ExNodeId> = self
            .nodes
            .iter()
            .filter(|(_, n)| matches!(n.state, ExNodeState::Completed))
            .map(|(id, _)| id)
            .collect();

        let mut ready = Vec::with_capacity(self.nodes.len() / 4);
        for (id, node) in &self.nodes {
            if node.kind != ExNodeKind::Task {
                continue;
            }
            if node.state != ExNodeState::Pending {
                continue;
            }
            let all_deps_satisfied = self
                .edges
                .iter()
                .filter(|e| e.to == *id)
                .map(|e| &e.from)
                .all(|d| completed.contains(d));
            if all_deps_satisfied {
                ready.push(id.clone());
            }
        }
        ready
    }

    /// Set a node's state.
    ///
    /// Test-only: used by `PlannerExecutionBridge::fail_step` (cfg(test)) to
    /// simulate failure propagation in DAG readiness tests.
    #[cfg(test)]
    pub fn set_node_state(&mut self, id: &str, state: ExNodeState) -> Result<()> {
        let node = self
            .nodes
            .get_mut(id)
            .ok_or_else(|| anyhow!("Node {id} not found"))?;
        node.state = state;
        Ok(())
    }

    /// Mark a task as completed.
    ///
    /// Test-only: used by `PlannerExecutionBridge::complete_step` (cfg(test)).
    #[cfg(test)]
    pub fn complete_task(&mut self, task_id: &str, output: serde_json::Value) -> Result<()> {
        let node = self
            .nodes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("Task {task_id} not found"))?;
        node.state = ExNodeState::Completed;
        node.output = Some(output);
        Ok(())
    }

    /// Check if the entire graph is complete (End node reached).
    pub fn is_complete(&self) -> bool {
        self.nodes
            .get(&self.end_node)
            .map(|n| matches!(n.state, ExNodeState::Completed))
            .unwrap_or(false)
    }
}

/// Build a phased execution order from the planner DAG.
///
/// Returns `Vec<Vec<String>>` where each inner vec is a phase of ready-to-run
/// node IDs.
pub fn dag_execution_order(bridge: &PlannerExecutionBridge) -> Vec<Vec<String>> {
    let plan_step_ids: HashSet<String> = bridge
        .plan
        .steps
        .iter()
        .map(|step| step.step_id.clone())
        .collect();

    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    for step in &bridge.plan.steps {
        deps.insert(step.step_id.clone(), step.depends_on.clone());
    }

    let mut remaining: BTreeSet<String> = plan_step_ids.iter().cloned().collect();
    let mut completed: HashSet<String> = HashSet::new();
    let mut phases: Vec<Vec<String>> = Vec::new();

    while !remaining.is_empty() {
        let ready_phase: Vec<String> = remaining
            .iter()
            .filter(|id| {
                deps.get(*id)
                    .map(|requirements| {
                        requirements
                            .iter()
                            .all(|dep| !plan_step_ids.contains(dep) || completed.contains(dep))
                    })
                    .unwrap_or(true)
            })
            .cloned()
            .collect();

        if ready_phase.is_empty() {
            phases.push(remaining.iter().cloned().collect());
            break;
        }

        for step_id in &ready_phase {
            let _ = remaining.remove(step_id);
            completed.insert(step_id.clone());
        }
        phases.push(ready_phase);
    }

    phases
}

/// Extract progress information from the DAG as a serializable value.
pub fn dag_progress_with_suggested_next(bridge: &PlannerExecutionBridge) -> Value {
    let snapshot = bridge.progress_snapshot();
    let ready = bridge.ready_nodes();
    let completed = snapshot["completed"].as_u64().unwrap_or(0);
    let failed = snapshot["failed"].as_u64().unwrap_or(0);
    let total = snapshot["total_steps"].as_u64().unwrap_or(0);

    let next_step_hint = if !ready.is_empty() {
        format!("ready to execute: {}", ready.join(", "),)
    } else if completed >= total {
        "all steps completed".to_string()
    } else if failed > 0 {
        format!("{} steps failed, repair may be needed", failed)
    } else {
        "waiting for dependencies".to_string()
    };

    serde_json::json!({
        "progress": snapshot,
        "ready_nodes": ready,
        "next_step_hint": next_step_hint,
        "is_complete": bridge.is_complete(),
    })
}

/// Check whether the bridge DAG indicates a stalled state (no progress possible).
pub fn dag_is_stalled(bridge: &PlannerExecutionBridge) -> bool {
    let completed = bridge
        .graph
        .nodes
        .iter()
        .filter(|(_, n)| matches!(n.state, ExNodeState::Completed))
        .count();
    let failed = bridge
        .graph
        .nodes
        .iter()
        .filter(|(_, n)| matches!(n.state, ExNodeState::Failed(..)))
        .count();
    let total = bridge.total_steps;

    !bridge.is_complete() && completed + failed < total && bridge.ready_nodes().is_empty()
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_graph(name: &str) -> ExecutionGraph {
        ExecutionGraph::new(name)
    }

    #[test]
    fn test_new_graph_has_start_and_end() {
        let g = make_graph("test");
        assert_eq!(g.nodes.len(), 2);
        assert!(g.nodes.contains_key("start"));
        assert!(g.nodes.contains_key("end"));
        assert_eq!(g.nodes["start"].kind, ExNodeKind::Start);
        assert_eq!(g.nodes["start"].state, ExNodeState::Completed);
        assert_eq!(g.nodes["end"].kind, ExNodeKind::End);
    }

    #[test]
    fn test_add_node_and_edge() {
        let mut g = make_graph("test");
        g.add_node(ExNode::new("step1", ExNodeKind::Task, "Step 1"));
        g.add_edge("start", "step1", None);
        let ready = g.get_ready_nodes();
        assert!(ready.contains(&"step1".to_string()));
    }

    #[test]
    fn test_get_ready_nodes() {
        let mut g = make_graph("test");
        g.add_node(ExNode::new("step1", ExNodeKind::Task, "Step 1"));
        g.add_node(ExNode::new("step2", ExNodeKind::Task, "Step 2"));
        g.add_edge("start", "step1", None);
        g.add_edge("step1", "step2", None);

        // Only step1 should be ready (start completed, step1 pending)
        let ready = g.get_ready_nodes();
        assert_eq!(ready.len(), 1);
        assert!(ready.contains(&"step1".to_string()));

        // Complete step1
        g.set_node_state("step1", ExNodeState::Completed).unwrap();
        let ready = g.get_ready_nodes();
        assert_eq!(ready.len(), 1);
        assert!(ready.contains(&"step2".to_string()));
    }

    #[test]
    fn test_complete_task() {
        let mut g = make_graph("test");
        g.add_node(ExNode::new("step1", ExNodeKind::Task, "Step 1"));
        assert!(g
            .complete_task("step1", serde_json::json!({"ok": true}))
            .is_ok());
        assert!(matches!(g.nodes["step1"].state, ExNodeState::Completed));
    }

    #[test]
    fn test_is_complete() {
        let mut g = make_graph("test");
        assert!(!g.is_complete());
        g.set_node_state("end", ExNodeState::Completed).unwrap();
        assert!(g.is_complete());
    }
}
