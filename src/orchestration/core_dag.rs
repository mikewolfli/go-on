//! Unified core DAG data structure for the orchestration subsystem.
//!
//! # Purpose
//!
//! This module provides a single generic [`CoreDag<T>`] that replaces the
//! three separate DAG graph implementations:
//!
//! - `dag_executor.rs` → [`DagGraph`](crate::orchestration::dag_executor::DagGraph)
//! - `task_graph.rs` → [`TaskGraph`](crate::orchestration::task_graph::TaskGraph)
//! - `execution_graph.rs` → [`ExecutionGraph`](crate::orchestration::execution_graph::ExecutionGraph)
//!
//! New code should prefer `CoreDag<T>` and use the conversion traits below
//! to bridge into existing APIs that still expect the legacy types.
//!
//! # Future work
//!
//! Once all call sites have migrated, the three legacy implementations
//! will be removed and `CoreDag<T>` will become the sole DAG type.
//!
//! # Migration
//!
//! ## Active usages
//! - Exported via `crate::orchestration::mod.rs` for new code
//! - Referenced in doc comments by `dag_executor.rs`, `execution_graph.rs`, `task_graph.rs`
//!
//! ## Migration plan
//! 1. Implement `FromCoreDag` / `IntoCoreDag` for all legacy DAG types
//! 2. Update call sites in `dag_executor.rs`, `task_graph.rs`, `execution_graph.rs`
//! 3. Remove legacy modules once call sites are migrated
//!
//! This module is **active** — do not delete. Prefer `CoreDag<T>` for new DAG code.

// TODO-BLUE64: Wire these utility APIs once consumers migrate to CoreDag:
//   remove_node, get, get_mut, contains, len, is_empty, parents,
//   has_cycle, metrics, DagMetrics, FromCoreDag, IntoCoreDag, iter_topo,
//   TaskContext (struct + impl).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use uuid::Uuid;

use crate::orchestration::execution_graph::{ExNodeId, ExNodeState};
use crate::orchestration::planner_execution_graph::PlannerExecutionBridge;

// ── Core types ──────────────────────────────────────────────────────────────

/// A generic directed acyclic graph (DAG) node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNode<T> {
    /// Unique identifier for this node.
    pub id: String,
    /// User-defined payload carried by the node.
    pub data: T,
    /// IDs of nodes that this node depends on (incoming edges).
    pub dependencies: Vec<String>,
}

/// A generic directed acyclic graph (DAG).
///
/// Stores nodes in a `HashMap` keyed by node ID, and maintains both
/// forward edges (`edges: parent → children`) and backward edges
/// (`dependencies` stored on each node).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreDag<T> {
    /// All nodes in the graph, keyed by their `id`.
    pub nodes: HashMap<String, DagNode<T>>,
    /// Forward edges: parent_id → set of child_ids.
    pub edges: HashMap<String, HashSet<String>>,
    /// Nodes that have no dependencies (entry points for topological sort).
    pub entry_points: Vec<String>,
}

impl<T> CoreDag<T> {
    /// Create a new, empty DAG.
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
            entry_points: Vec::new(),
        }
    }

    /// Add a node to the DAG. If a node with the same `id` already exists,
    /// it is replaced.
    pub fn add_node(&mut self, id: String, data: T, dependencies: Vec<String>) {
        let node = DagNode {
            id: id.clone(),
            data,
            dependencies: dependencies.clone(),
        };
        self.nodes.insert(id.clone(), node);

        // Register forward edges
        for dep in &dependencies {
            self.edges
                .entry(dep.clone())
                .or_default()
                .insert(id.clone());
        }

        // Recompute entry points
        self.recompute_entry_points();
    }

    /// Remove a node and all its edges from the DAG.
    #[allow(dead_code)]
    pub fn remove_node(&mut self, id: &str) -> Option<DagNode<T>> {
        let node = self.nodes.remove(id)?;

        // Remove forward edges from this node's dependencies
        for dep in &node.dependencies {
            if let Some(children) = self.edges.get_mut(dep) {
                children.remove(id);
                if children.is_empty() {
                    self.edges.remove(dep);
                }
            }
        }

        // Remove forward edges where this node is a parent
        self.edges.remove(id);

        self.recompute_entry_points();
        Some(node)
    }

    /// Get a reference to a node by ID.
    #[allow(dead_code)]
    pub fn get(&self, id: &str) -> Option<&DagNode<T>> {
        self.nodes.get(id)
    }

    /// Get a mutable reference to a node by ID.
    #[allow(dead_code)]
    pub fn get_mut(&mut self, id: &str) -> Option<&mut DagNode<T>> {
        self.nodes.get_mut(id)
    }

    /// Return `true` if the graph contains a node with the given ID.
    #[allow(dead_code)]
    pub fn contains(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    /// Return the number of nodes in the graph.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Return `true` if the graph has no nodes.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Return the children (direct successors) of a node.
    pub fn children(&self, id: &str) -> Vec<&str> {
        self.edges
            .get(id)
            .map(|children| children.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    /// Return the parents (direct dependencies) of a node.
    #[allow(dead_code)]
    pub fn parents(&self, id: &str) -> Vec<&str> {
        self.nodes
            .get(id)
            .map(|node| node.dependencies.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    // ── Topological sort ────────────────────────────────────────────────────

    /// Perform a topological sort of the graph.
    ///
    /// Returns `Ok(Vec<&str>)` with node IDs in topological order, or
    /// `Err(String)` if a cycle is detected.
    pub fn topological_sort(&self) -> Result<Vec<&str>, String> {
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        for (id, node) in &self.nodes {
            in_degree.entry(id.as_str()).or_insert(0);
            for dep in &node.dependencies {
                if self.nodes.contains_key(dep) {
                    *in_degree.entry(id.as_str()).or_insert(0) += 1;
                }
            }
        }

        let mut queue: VecDeque<&str> = VecDeque::new();
        for (id, degree) in &in_degree {
            if *degree == 0 {
                queue.push_back(id);
            }
        }

        let mut sorted: Vec<&str> = Vec::with_capacity(self.nodes.len());
        while let Some(node_id) = queue.pop_front() {
            sorted.push(node_id);
            for child in self.children(node_id) {
                if let Some(degree) = in_degree.get_mut(child) {
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(child);
                    }
                }
            }
        }

        if sorted.len() != self.nodes.len() {
            return Err(format!(
                "cycle detected: sorted {} of {} nodes",
                sorted.len(),
                self.nodes.len()
            ));
        }

        Ok(sorted)
    }

    /// Detect whether the graph contains a cycle.
    #[allow(dead_code)]
    pub fn has_cycle(&self) -> bool {
        self.topological_sort().is_err()
    }

    /// Compute the width (maximum number of nodes at any depth level) and
    /// depth (longest path length) of the DAG.
    #[allow(dead_code)]
    pub fn metrics(&self) -> DagMetrics {
        let depth = self.compute_depth();
        let width = self.compute_width();
        DagMetrics { width, depth }
    }

    fn compute_depth(&self) -> usize {
        let sorted = match self.topological_sort() {
            Ok(s) => s,
            Err(_) => return 0,
        };

        let mut depths: HashMap<&str, usize> = HashMap::new();
        let mut max_depth = 0usize;

        for &node_id in &sorted {
            let node = match self.nodes.get(node_id) {
                Some(n) => n,
                None => continue,
            };

            let depth = if node.dependencies.is_empty() {
                1usize
            } else {
                let parent_depth = node
                    .dependencies
                    .iter()
                    .filter_map(|dep| depths.get(dep.as_str()))
                    .max()
                    .copied()
                    .unwrap_or(0);
                parent_depth + 1
            };

            depths.insert(node_id, depth);
            max_depth = max_depth.max(depth);
        }

        max_depth
    }

    fn compute_width(&self) -> usize {
        // Since compute_depth already computed depths, we use a simpler
        // approach: group nodes by their depth via level-order traversal.
        let sorted = match self.topological_sort() {
            Ok(s) => s,
            Err(_) => return 0,
        };

        let mut depths: HashMap<&str, usize> = HashMap::new();
        let mut max_depth = 0usize;

        for &node_id in &sorted {
            let node = match self.nodes.get(node_id) {
                Some(n) => n,
                None => continue,
            };

            let depth = if node.dependencies.is_empty() {
                1usize
            } else {
                let parent_depth = node
                    .dependencies
                    .iter()
                    .filter_map(|dep| depths.get(dep.as_str()))
                    .max()
                    .copied()
                    .unwrap_or(0);
                parent_depth + 1
            };

            depths.insert(node_id, depth);
            max_depth = max_depth.max(depth);
        }

        // Count nodes at each depth level
        let mut level_counts: HashMap<usize, usize> = HashMap::new();
        for &depth in depths.values() {
            *level_counts.entry(depth).or_insert(0) += 1;
        }

        level_counts.values().max().copied().unwrap_or(0)
    }

    // ── Private helpers ─────────────────────────────────────────────────────

    fn recompute_entry_points(&mut self) {
        self.entry_points = self
            .nodes
            .values()
            .filter(|n| n.dependencies.is_empty())
            .map(|n| n.id.clone())
            .collect();
    }
}

impl<T> Default for CoreDag<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ── Metrics ─────────────────────────────────────────────────────────────────

/// Metrics computed from a DAG.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DagMetrics {
    /// Maximum number of nodes at a single depth level.
    pub width: usize,
    /// Longest path length (number of levels).
    pub depth: usize,
}

// ── Conversion traits (reserved for future use) ─────────────────────────────

/// Trait for converting from a `CoreDag<T>` to another DAG type.
///
/// Implement this trait for each legacy DAG type to enable migration.
///
/// ```ignore
/// impl From<CoreDag<MyNodeType>> for LegacyGraphType {
///     fn from(dag: CoreDag<MyNodeType>) -> Self {
///         // ... conversion logic ...
///     }
/// }
/// ```
#[allow(dead_code)]
pub trait FromCoreDag<T, Target> {
    /// Convert a `CoreDag<T>` into `Target`.
    fn from_core_dag(dag: CoreDag<T>) -> Target;
}

/// Trait for converting from another DAG type into a `CoreDag<T>`.
#[allow(dead_code)]
pub trait IntoCoreDag<T, Source> {
    /// Convert `Source` into a `CoreDag<T>`.
    fn into_core_dag(source: Source) -> CoreDag<T>;
}

// ── Iterators ───────────────────────────────────────────────────────────────

/// An iterator over the nodes of a `CoreDag` in topological order.
pub struct TopoIter<'a, T> {
    dag: &'a CoreDag<T>,
    order: Vec<&'a str>,
    index: usize,
}

impl<'a, T> TopoIter<'a, T> {
    #[allow(dead_code)]
    fn new(dag: &'a CoreDag<T>) -> Result<Self, String> {
        let order = dag.topological_sort()?;
        Ok(Self {
            dag,
            order,
            index: 0,
        })
    }
}

impl<'a, T> Iterator for TopoIter<'a, T> {
    type Item = (&'a str, &'a DagNode<T>);

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.order.get(self.index)?;
        self.index += 1;
        self.dag.nodes.get(*id).map(|node| (*id, node))
    }
}

impl<T> CoreDag<T> {
    /// Return an iterator over nodes in topological order.
    #[allow(dead_code)]
    pub fn iter_topo(&self) -> Result<TopoIter<'_, T>, String> {
        TopoIter::new(self)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────
// ===========================================================================
// Deprecated DAG types — consolidated from dag_executor, dag_driver,
// and dag_execution for eventual removal of those legacy modules.
// ===========================================================================

// ---------------------------------------------------------------------------
// TaskContext — Chain-of-Thought context propagated between DAG nodes
// ---------------------------------------------------------------------------

/// Chain-of-Thought context propagated between DAG nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContext {
    pub id: String,
    pub reasoning_trace: Vec<String>,
    pub intermediate_findings: HashMap<String, Value>,
    pub confidence: f64,
    pub open_questions: Vec<String>,
    pub assumptions: Vec<String>,
    pub parent_context_id: Option<String>,
}

impl TaskContext {
    /// Create a new TaskContext with the given id.
    pub fn new(id: String) -> Self {
        Self {
            id,
            reasoning_trace: Vec::new(),
            intermediate_findings: HashMap::new(),
            confidence: 1.0,
            open_questions: Vec::new(),
            assumptions: Vec::new(),
            parent_context_id: None,
        }
    }

    /// Merge multiple parent contexts into a single child context.
    /// Generates a new UUID for the merged context's id.
    pub fn merge(parents: &[TaskContext]) -> Self {
        let mut reasoning_trace = Vec::new();
        let mut intermediate_findings = HashMap::new();
        let mut confidences_sum = 0.0;
        let mut open_questions = Vec::new();
        let mut assumptions = Vec::new();

        for parent in parents {
            reasoning_trace.extend(parent.reasoning_trace.clone());
            intermediate_findings.extend(parent.intermediate_findings.clone());
            confidences_sum += parent.confidence;
            open_questions.extend(parent.open_questions.clone());
            assumptions.extend(parent.assumptions.clone());
        }

        let parent_context_id = parents.first().map(|p| p.id.clone());

        Self {
            id: Uuid::new_v4().to_string(),
            reasoning_trace,
            intermediate_findings,
            confidence: if parents.is_empty() {
                1.0
            } else {
                confidences_sum / parents.len() as f64
            },
            open_questions,
            assumptions,
            parent_context_id,
        }
    }
}

// ---------------------------------------------------------------------------
// DagNodeResult / DagExecutionTrace — consolidated from dag_driver
// ---------------------------------------------------------------------------

/// Result of a single DAG node execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNodeResult {
    pub node_id: ExNodeId,
    pub tool_name: String,
    pub state: ExNodeState,
    pub duration_ms: u64,
    /// Preserved tool output payload for observe/replan evidence.
    pub tool_output: Option<Value>,
    /// Preserved error payload for diagnostic use.
    pub error_payload: Option<String>,
}

/// Complete DAG execution trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagExecutionTrace {
    pub nodes: Vec<DagNodeResult>,
    pub total_duration_ms: u64,
    pub branch_count: u32,
    pub join_count: u32,
}

/// Convert DAG execution results into a governance.status-observable payload.
pub fn dag_trace_to_observability(trace: &DagExecutionTrace) -> Value {
    let completed = trace
        .nodes
        .iter()
        .filter(|n| matches!(n.state, ExNodeState::Completed))
        .count();
    let failed = trace
        .nodes
        .iter()
        .filter(|n| matches!(n.state, ExNodeState::Failed(_)))
        .count();
    let total = trace.nodes.len();

    serde_json::json!({
        "dag_execution": {
            "total_nodes": total,
            "completed": completed,
            "failed": failed,
            "branch_count": trace.branch_count,
            "join_count": trace.join_count,
            "total_duration_ms": trace.total_duration_ms,
            "dag_width": trace.join_count,
            "dag_depth": trace.branch_count,
            "has_tool_evidence": trace.nodes.iter().any(|n| n.tool_output.is_some()),
            "node_details": trace.nodes.iter().map(|n| serde_json::json!({
                "node_id": n.node_id,
                "tool": n.tool_name,
                "state": format!("{:?}", n.state),
                "duration_ms": n.duration_ms,
                "has_output": n.tool_output.is_some(),
                "has_error": n.error_payload.is_some(),
            })).collect::<Vec<_>>(),
        }
    })
}

// ---------------------------------------------------------------------------
// DAG execution order / progress — consolidated from dag_execution
// ---------------------------------------------------------------------------

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

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_dag() {
        let dag: CoreDag<String> = CoreDag::new();
        assert!(dag.is_empty());
        assert_eq!(dag.len(), 0);
        assert!(dag.topological_sort().unwrap().is_empty());
    }

    #[test]
    fn test_single_node() {
        let mut dag: CoreDag<String> = CoreDag::new();
        dag.add_node("a".into(), "task A".into(), vec![]);
        assert!(!dag.is_empty());
        assert_eq!(dag.len(), 1);
        let sorted = dag.topological_sort().unwrap();
        assert_eq!(sorted, vec!["a"]);
    }

    #[test]
    fn test_linear_chain() {
        let mut dag: CoreDag<String> = CoreDag::new();
        dag.add_node("a".into(), "start".into(), vec![]);
        dag.add_node("b".into(), "middle".into(), vec!["a".into()]);
        dag.add_node("c".into(), "end".into(), vec!["b".into()]);
        let sorted = dag.topological_sort().unwrap();
        assert_eq!(sorted, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_diamond_dag() {
        let mut dag: CoreDag<String> = CoreDag::new();
        dag.add_node("root".into(), "root".into(), vec![]);
        dag.add_node("left".into(), "left".into(), vec!["root".into()]);
        dag.add_node("right".into(), "right".into(), vec!["root".into()]);
        dag.add_node(
            "merge".into(),
            "merge".into(),
            vec!["left".into(), "right".into()],
        );
        let sorted = dag.topological_sort().unwrap();
        assert_eq!(sorted[0], "root");
        assert!(sorted.contains(&"left"));
        assert!(sorted.contains(&"right"));
        assert_eq!(sorted[3], "merge");
    }

    #[test]
    fn test_cycle_detection() {
        let mut dag: CoreDag<String> = CoreDag::new();
        dag.add_node("a".into(), "A".into(), vec![]);
        dag.add_node("b".into(), "B".into(), vec!["a".into()]);
        // Manually create a cycle by adding "a" as a dependency of "b"
        // and "b" as a dependency of "a"
        if let Some(node_b) = dag.nodes.get_mut("b") {
            node_b.dependencies.push("a".into());
        }
        // Now add "a" depends on "b" by using raw HashMap manipulation
        if let Some(node_a) = dag.nodes.get_mut("a") {
            node_a.dependencies.push("b".into());
        }
        // Register the forward edge b → a
        dag.edges.entry("b".into()).or_default().insert("a".into());
        dag.recompute_entry_points();

        assert!(dag.has_cycle());
        assert!(dag.topological_sort().is_err());
    }

    #[test]
    fn test_children_and_parents() {
        let mut dag: CoreDag<String> = CoreDag::new();
        dag.add_node("a".into(), "A".into(), vec![]);
        dag.add_node("b".into(), "B".into(), vec!["a".into()]);
        dag.add_node("c".into(), "C".into(), vec!["a".into()]);

        let children = dag.children("a");
        assert_eq!(children.len(), 2);
        assert!(children.contains(&"b"));
        assert!(children.contains(&"c"));

        let parents_b = dag.parents("b");
        assert_eq!(parents_b, vec!["a"]);

        let parents_a: Vec<&str> = dag.parents("a");
        assert!(parents_a.is_empty());
    }

    #[test]
    fn test_metrics() {
        let mut dag: CoreDag<String> = CoreDag::new();
        dag.add_node("a".into(), "A".into(), vec![]);
        dag.add_node("b".into(), "B".into(), vec!["a".into()]);
        dag.add_node("c".into(), "C".into(), vec!["a".into()]);
        dag.add_node("d".into(), "D".into(), vec!["b".into(), "c".into()]);

        let metrics = dag.metrics();
        // width: level 1 (a) = 1, level 2 (b, c) = 2, level 3 (d) = 1 → max = 2
        assert_eq!(metrics.width, 2);
        // depth: a → b → d = 3, a → c → d = 3
        assert_eq!(metrics.depth, 3);
    }

    #[test]
    fn test_topo_iterator() {
        let mut dag: CoreDag<String> = CoreDag::new();
        dag.add_node("a".into(), "first".into(), vec![]);
        dag.add_node("b".into(), "second".into(), vec!["a".into()]);
        dag.add_node("c".into(), "third".into(), vec!["b".into()]);

        let ids: Vec<&str> = dag.iter_topo().unwrap().map(|(id, _)| id).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_remove_node() {
        let mut dag: CoreDag<String> = CoreDag::new();
        dag.add_node("a".into(), "A".into(), vec![]);
        dag.add_node("b".into(), "B".into(), vec!["a".into()]);

        let removed = dag.remove_node("a");
        assert!(removed.is_some());
        assert_eq!(dag.len(), 1);
        assert!(!dag.contains("a"));
        assert!(dag.contains("b"));
    }
}
