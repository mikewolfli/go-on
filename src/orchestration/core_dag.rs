//! Unified core DAG data structure for the orchestration subsystem.
//!
//! # Purpose
//!
//! This module provides a single file that consolidates:
//!
//! - **CoreDag** — generic multi-purpose DAG
//! - **ExecutionGraph** — DAG with fan-out/join and conditional branching
//! - **TaskGraph** — DAG for checkpoint and restore workflows
//!
//! New code should prefer `CoreDag<T>` and use the conversion traits below
//! to bridge into existing APIs that still expect the legacy types.
//!
//! # Future work
//!
//! Once all call sites have migrated, `CoreDag<T>` will become the sole DAG type
//! and the ExecutionGraph / TaskGraph types will be deprecated.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use uuid::Uuid;

use crate::orchestration::planner_execution_graph::PlannerExecutionBridge;

// =========================================================================
// Execution Graph — DAG with fan-out/join and conditional branching support
// =========================================================================

/// Execution graph node ID
pub type ExNodeId = String;

/// Kind of execution graph node
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ExNodeKind {
    /// Standard execution step
    Task,
    /// Fan-out to multiple parallel branches
    Branch,
    /// Sync point after parallel execution
    Join,
    /// Conditional branching based on evaluation
    Condition,
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

/// Condition evaluated by a Condition node to determine branching
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExCondition {
    /// True when a node's output matches an expected value
    OutputMatches {
        node_id: ExNodeId,
        expected: serde_json::Value,
    },
    /// Numeric comparison on a node's output field
    NumericCompare {
        node_id: ExNodeId,
        field: String,
        op: String, // "==", "!=", ">", "<", ">=", "<="
        value: f64,
    },
    /// All sub-conditions must be true (AND)
    All(Vec<ExCondition>),
    /// Any sub-condition must be true (OR)
    Any(Vec<ExCondition>),
    /// Always evaluates to true
    Always,
}

impl ExCondition {
    /// Evaluate this condition against the current node outputs.
    pub fn evaluate(&self, node_outputs: &HashMap<ExNodeId, &ExNode>) -> bool {
        match self {
            ExCondition::OutputMatches { node_id, expected } => node_outputs
                .get(node_id)
                .and_then(|n| n.output.as_ref())
                .map(|o| o == expected)
                .unwrap_or(false),
            ExCondition::NumericCompare {
                node_id,
                field,
                op,
                value,
            } => {
                let actual = node_outputs
                    .get(node_id)
                    .and_then(|n| n.output.as_ref())
                    .and_then(|o| o.get(field))
                    .and_then(|v| v.as_f64());
                match (actual, op.as_str()) {
                    (Some(a), "==") => (a - value).abs() < f64::EPSILON,
                    (Some(a), "!=") => (a - value).abs() >= f64::EPSILON,
                    (Some(a), ">") => a > *value,
                    (Some(a), "<") => a < *value,
                    (Some(a), ">=") => a >= *value,
                    (Some(a), "<=") => a <= *value,
                    _ => false,
                }
            }
            ExCondition::All(conds) => conds.iter().all(|c| c.evaluate(node_outputs)),
            ExCondition::Any(conds) => conds.iter().any(|c| c.evaluate(node_outputs)),
            ExCondition::Always => true,
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

/// Tracks a fan-out group: branch -> parallel tasks -> join
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanOutGroup {
    pub group_id: String,
    pub branch_node_id: ExNodeId,
    pub join_node_id: ExNodeId,
    pub parallel_task_ids: Vec<ExNodeId>,
    pub completed_count: usize,
    pub total_count: usize,
}

/// Execution graph — a DAG supporting fan-out/join and conditional branching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionGraph {
    pub nodes: HashMap<ExNodeId, ExNode>,
    pub edges: Vec<ExEdge>,
    pub start_node: ExNodeId,
    pub end_node: ExNodeId,
    pub fan_out_groups: Vec<FanOutGroup>,
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
            fan_out_groups: Vec::with_capacity(4),
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

    /// Create a fan-out: branch -> parallel tasks -> join.
    /// Returns (branch_id, join_id) on success.
    pub fn add_fan_out(
        &mut self,
        branch_name: &str,
        join_name: &str,
        parallel_tasks: Vec<(String, String)>, // (task_id, task_name)
        predecessor: &str,
    ) -> Result<(ExNodeId, ExNodeId)> {
        let branch_id = format!("branch-{}", branch_name);
        let join_id = format!("join-{}", join_name);

        self.add_node(ExNode::new(&branch_id, ExNodeKind::Branch, branch_name));
        self.add_node(ExNode::new(&join_id, ExNodeKind::Join, join_name));

        // Connect predecessor -> branch
        self.add_edge(predecessor, &branch_id, None);

        // Connect branch -> each parallel task -> join
        let mut task_ids = Vec::new();
        for (tid, tname) in &parallel_tasks {
            self.add_node(ExNode::new(tid, ExNodeKind::Task, tname));
            self.add_edge(&branch_id, tid, None);
            self.add_edge(tid, &join_id, None);
            task_ids.push(tid.clone());
        }

        // Register fan-out group
        self.fan_out_groups.push(FanOutGroup {
            group_id: format!("fanout-{}", branch_name),
            branch_node_id: branch_id.clone(),
            join_node_id: join_id.clone(),
            parallel_task_ids: task_ids.clone(),
            completed_count: 0,
            total_count: parallel_tasks.len(),
        });

        Ok((branch_id, join_id))
    }

    /// Add a Condition node with true/false branches.
    pub fn add_condition(
        &mut self,
        cond_id: &str,
        cond_name: &str,
        condition: ExCondition,
        predecessor: &str,
        true_target: &str,
        false_target: &str,
    ) -> ExNodeId {
        let id = cond_id.to_string();
        let mut node = ExNode::new(&id, ExNodeKind::Condition, cond_name);
        node.input = serde_json::to_value(&condition).unwrap_or_default();
        self.add_node(node);
        self.add_edge(predecessor, &id, None);
        self.add_edge(&id, true_target, Some("true"));
        self.add_edge(&id, false_target, Some("false"));
        id
    }

    /// Get nodes whose dependencies are all satisfied (ready to execute).
    /// Returns `Task`, `Branch`, and `Condition` nodes whose dependencies are satisfied.
    /// Condition nodes are included so they can be evaluated for branch selection.
    pub fn get_ready_nodes(&self) -> Vec<ExNodeId> {
        let completed: HashSet<&ExNodeId> = self
            .nodes
            .iter()
            .filter(|(_, n)| matches!(n.state, ExNodeState::Completed))
            .map(|(id, _)| id)
            .collect();

        let mut ready = Vec::with_capacity(self.nodes.len() / 4);
        for (id, node) in &self.nodes {
            if !matches!(
                node.kind,
                ExNodeKind::Task | ExNodeKind::Branch | ExNodeKind::Condition
            ) {
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
    pub fn set_node_state(&mut self, id: &str, state: ExNodeState) -> Result<()> {
        let node = self
            .nodes
            .get_mut(id)
            .ok_or_else(|| anyhow!("Node {id} not found"))?;
        node.state = state;
        Ok(())
    }

    /// Mark a task as completed and update fan-out progress.
    pub fn complete_task(&mut self, task_id: &str, output: serde_json::Value) -> Result<()> {
        let node = self
            .nodes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("Task {task_id} not found"))?;
        node.state = ExNodeState::Completed;
        node.output = Some(output);

        // Update fan-out progress
        for group in &mut self.fan_out_groups {
            if group.parallel_task_ids.iter().any(|id| id == task_id) {
                group.completed_count += 1;
            }
        }

        Ok(())
    }

    /// Check if a fan-out group is fully complete.
    pub fn is_fan_out_complete(&self, group_id: &str) -> bool {
        self.fan_out_groups
            .iter()
            .find(|g| g.group_id == group_id)
            .map(|g| g.completed_count >= g.total_count)
            .unwrap_or(false)
    }

    /// Check if the entire graph is complete (End node reached).
    pub fn is_complete(&self) -> bool {
        self.nodes
            .get(&self.end_node)
            .map(|n| matches!(n.state, ExNodeState::Completed))
            .unwrap_or(false)
    }

    /// Count nodes matching a given state.
    pub fn count_by_state(&self, state: &ExNodeState) -> usize {
        self.nodes.values().filter(|n| n.state == *state).count()
    }

    /// Get progress summary for all fan-out groups.
    pub fn fan_out_summary(&self) -> Vec<(String, usize, usize)> {
        self.fan_out_groups
            .iter()
            .map(|g| (g.group_id.clone(), g.completed_count, g.total_count))
            .collect()
    }

    /// Reset all nodes to Pending (for re-execution).
    /// The Start node is kept in Completed state to avoid deadlocking the graph.
    pub fn reset(&mut self) {
        for node in self.nodes.values_mut() {
            if node.kind == ExNodeKind::Start {
                continue;
            }
            node.state = ExNodeState::Pending;
            node.output = None;
            node.error = None;
            node.duration_ms = None;
        }
        for group in &mut self.fan_out_groups {
            group.completed_count = 0;
        }
    }
}

// =========================================================================
// Core DAG — generic multi-purpose directed acyclic graph
// =========================================================================

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
/// forward edges (`edges: parent -> children`) and backward edges
/// (`dependencies` stored on each node).
#[derive(Clone, Serialize, Deserialize)]
pub struct CoreDag<T> {
    /// All nodes in the graph, keyed by their `id`.
    pub nodes: HashMap<String, DagNode<T>>,
    /// Forward edges: parent_id -> set of child_ids.
    pub edges: HashMap<String, HashSet<String>>,
    /// Nodes that have no dependencies (entry points for topological sort).
    pub entry_points: Vec<String>,
}

impl<T> std::fmt::Debug for CoreDag<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut node_debug: Vec<String> = Vec::new();
        for (id, _) in &self.nodes {
            let parents = self.parents(id);
            node_debug.push(format!("{} -> parents: {:?}", id, parents));
        }
        f.debug_struct("CoreDag")
            .field("nodes", &node_debug)
            .field("edges", &self.edges)
            .field("entry_points", &self.entry_points)
            .finish()
    }
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

    /// Return the children (direct successors) of a node.
    pub fn children(&self, id: &str) -> Vec<&str> {
        self.edges
            .get(id)
            .map(|children| children.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    /// Return the parents (direct dependencies) of a node.
    pub fn parents(&self, id: &str) -> Vec<&str> {
        self.nodes
            .get(id)
            .map(|node| node.dependencies.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    // -- Topological sort ----------------------------------------------------

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

    /// Compute the width (maximum number of nodes at any depth level) and
    /// depth (longest path length) of the DAG.
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

    // -- Private helpers -----------------------------------------------------

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

// -- Metrics -----------------------------------------------------------------

/// Metrics computed from a DAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DagMetrics {
    /// Maximum number of nodes at a single depth level.
    pub width: usize,
    /// Longest path length (number of levels).
    pub depth: usize,
}

// -- Conversion traits (reserved for future use) -----------------------------

/// Trait for converting from another DAG type into a `CoreDag<T>`.
#[cfg_attr(not(test), allow(dead_code))] // used in tests (F-GAP-49)
pub trait IntoCoreDag<T, Source> {
    /// Convert `Source` into a `CoreDag<T>`.
    fn into_core_dag(source: Source) -> CoreDag<T>;
}

// -- Iterators ---------------------------------------------------------------

/// An iterator over the nodes of a `CoreDag` in topological order.
#[cfg_attr(not(test), allow(dead_code))] // used in tests (F-GAP-49)
pub struct TopoIter<'a, T> {
    dag: &'a CoreDag<T>,
    order: Vec<&'a str>,
    index: usize,
}

impl<'a, T> TopoIter<'a, T> {
    #[cfg_attr(not(test), allow(dead_code))] // used in tests (F-GAP-49)
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
    #[cfg_attr(not(test), allow(dead_code))] // used in tests (F-GAP-49)
    pub fn iter_topo(&self) -> Result<TopoIter<'_, T>, String> {
        TopoIter::new(self)
    }
}

// -- TaskContext -------------------------------------------------------------

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

// -- DagNodeResult / DagExecutionTrace ---------------------------------------

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
// Task Graph — DAG for checkpoint and restore workflows
// =========================================================================

/// Task node ID
type NodeId = String;

/// Record of a planned subtask for checkpoint/restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedSubtaskRecord {
    pub subtask_id: String,
    pub description: String,
    pub phase: String,
    pub outcome: Option<String>,
    pub result_summary: Option<String>,
    /// IDs of subtasks this subtask depends on.
    /// Preserved across checkpoint/restore to reconstruct the DAG.
    #[serde(default)]
    pub dependencies: Vec<String>,
}

/// Checkpoint artifact for task graph serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskGraphCheckpointArtifact {
    pub checkpoint_id: String,
    pub schema_version: String,
    pub created_at: i64,
    pub task: String,
    pub phases_completed: usize,
    pub subtask_records: Vec<PlannedSubtaskRecord>,
    pub resume_eligible: bool,
    pub resume_reason: Option<String>,
}

/// Task graph node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNode {
    pub id: NodeId,
    pub kind: String,  // e.g., "plan", "edit", "review"
    pub state: String, // e.g., "pending", "running", "done", "failed"
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub dependencies: HashSet<NodeId>,
    pub retries: u32,
}

/// Task graph (DAG)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskGraph {
    pub nodes: HashMap<NodeId, TaskNode>,
    pub edges: HashMap<NodeId, HashSet<NodeId>>, // from -> to
    pub root: NodeId,
}

impl TaskGraph {
    pub fn new(root: TaskNode) -> Self {
        let root_id = root.id.clone();
        let mut nodes = HashMap::new();
        nodes.insert(root_id.clone(), root);
        Self {
            nodes,
            edges: HashMap::new(),
            root: root_id,
        }
    }
    pub fn add_node(&mut self, node: TaskNode) {
        self.nodes.insert(node.id.clone(), node);
    }
    /// Add a directed edge from `from` to `to`.
    /// Returns `Err` if adding this edge would create a cycle.
    pub fn add_edge(&mut self, from: String, to: String) -> Result<(), String> {
        // Quick self-loop check
        if from == to {
            return Err(format!("self-loop edge not allowed: {} -> {}", from, to));
        }
        // Check if edge already exists
        if self
            .edges
            .get(&from)
            .is_some_and(|edges| edges.contains(&to))
        {
            return Ok(()); // Edge already exists, no-op
        }
        // Temporarily add the edge and check for cycles using DFS
        self.edges
            .entry(from.clone())
            .or_default()
            .insert(to.clone());
        if self.has_cycle() {
            // Rollback: remove the edge we just added
            if let Some(edges) = self.edges.get_mut(&from) {
                edges.remove(&to);
                if edges.is_empty() {
                    self.edges.remove(&from);
                }
            }
            return Err(format!(
                "adding edge {} -> {} would create a cycle",
                from, to
            ));
        }
        Ok(())
    }

    /// Check whether the graph contains a cycle using DFS.
    /// Returns `true` if a cycle is detected.
    pub fn has_cycle(&self) -> bool {
        let mut visited: HashSet<String> = HashSet::new();
        let mut recursion_stack: HashSet<String> = HashSet::new();

        for node_id in self.nodes.keys() {
            if !visited.contains(node_id)
                && self.dfs_cycle_check(node_id, &mut visited, &mut recursion_stack)
            {
                return true;
            }
        }
        false
    }

    fn dfs_cycle_check(
        &self,
        node_id: &str,
        visited: &mut HashSet<String>,
        recursion_stack: &mut HashSet<String>,
    ) -> bool {
        visited.insert(node_id.to_string());
        recursion_stack.insert(node_id.to_string());

        if let Some(neighbors) = self.edges.get(node_id) {
            for neighbor in neighbors {
                if !visited.contains(neighbor) {
                    if self.dfs_cycle_check(neighbor, visited, recursion_stack) {
                        return true;
                    }
                } else if recursion_stack.contains(neighbor) {
                    return true;
                }
            }
        }

        recursion_stack.remove(node_id);
        false
    }
    pub fn get_node(&self, id: &str) -> Option<&TaskNode> {
        self.nodes.get(id)
    }
    pub fn set_node_state(&mut self, id: &str, state: String) -> bool {
        if let Some(node) = self.nodes.get_mut(id) {
            node.state = state;
            true
        } else {
            false
        }
    }
    pub fn set_node_output(&mut self, id: &str, output: serde_json::Value) -> bool {
        if let Some(node) = self.nodes.get_mut(id) {
            node.output = Some(output);
            true
        } else {
            false
        }
    }
    pub fn is_complete(&self) -> bool {
        self.nodes
            .values()
            .all(|n| n.state == "done" || n.state == "failed")
    }

    /// B26-S11: Snapshot the current graph state into a checkpoint artifact.
    pub fn snapshot(
        &self,
        task: &str,
        phases_completed: usize,
        subtask_records: Vec<PlannedSubtaskRecord>,
    ) -> TaskGraphCheckpointArtifact {
        let failed_count = subtask_records
            .iter()
            .filter(|r| r.outcome.as_deref() == Some("failed"))
            .count();
        let resume_eligible = failed_count < subtask_records.len();
        let resume_reason = if failed_count > 0 {
            Some(format!(
                "{} subtasks failed, resume will retry them",
                failed_count
            ))
        } else {
            None
        };
        TaskGraphCheckpointArtifact {
            checkpoint_id: format!("ckpt-{}", crate::acp::prelude::now_ts()),
            schema_version: "blue26-taskgraph-checkpoint-v1".to_string(),
            created_at: crate::acp::prelude::now_ts(),
            task: task.to_string(),
            phases_completed,
            subtask_records,
            resume_eligible,
            resume_reason,
        }
    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- ExecutionGraph tests -----------------------------------------------

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
    fn test_fan_out_creation() {
        let mut g = make_graph("test");
        let tasks = vec![
            ("t1".to_string(), "Task 1".to_string()),
            ("t2".to_string(), "Task 2".to_string()),
            ("t3".to_string(), "Task 3".to_string()),
        ];
        let result = g.add_fan_out("analysis", "analysis-join", tasks, "start");
        assert!(result.is_ok());
        let (branch_id, join_id) = result.unwrap();

        assert!(g.nodes.contains_key(&branch_id));
        assert!(g.nodes.contains_key(&join_id));
        assert_eq!(g.nodes[&branch_id].kind, ExNodeKind::Branch);
        assert_eq!(g.nodes[&join_id].kind, ExNodeKind::Join);
        assert_eq!(g.fan_out_groups.len(), 1);
        assert_eq!(g.fan_out_groups[0].total_count, 3);
    }

    #[test]
    fn test_fan_out_progress() {
        let mut g = make_graph("test");
        let tasks = vec![
            ("t1".to_string(), "Task 1".to_string()),
            ("t2".to_string(), "Task 2".to_string()),
        ];
        let _ = g.add_fan_out("build", "build-join", tasks, "start");
        let group_id = g.fan_out_groups[0].group_id.clone();

        // Complete one task
        assert!(g
            .complete_task("t1", serde_json::json!({"ok": true}))
            .is_ok());
        assert!(!g.is_fan_out_complete(&group_id));
        assert!(!g.is_fan_out_complete(&group_id));

        // Complete second task
        assert!(g
            .complete_task("t2", serde_json::json!({"ok": true}))
            .is_ok());
        assert!(g.is_fan_out_complete(&group_id));
    }

    #[test]
    fn test_condition_evaluation_output_matches() {
        let mut node_outputs: HashMap<ExNodeId, &ExNode> = HashMap::new();
        let mut node = ExNode::new("step1", ExNodeKind::Task, "Step 1");
        node.output = Some(serde_json::json!({"status": "ok"}));
        node_outputs.insert("step1".to_string(), &node);

        let cond = ExCondition::OutputMatches {
            node_id: "step1".to_string(),
            expected: serde_json::json!({"status": "ok"}),
        };
        assert!(cond.evaluate(&node_outputs));

        let cond_fail = ExCondition::OutputMatches {
            node_id: "step1".to_string(),
            expected: serde_json::json!({"status": "fail"}),
        };
        assert!(!cond_fail.evaluate(&node_outputs));
    }

    #[test]
    fn test_condition_evaluation_numeric() {
        let mut node_outputs: HashMap<ExNodeId, &ExNode> = HashMap::new();
        let mut node = ExNode::new("step1", ExNodeKind::Task, "Step 1");
        node.output = Some(serde_json::json!({"score": 42.0}));
        node_outputs.insert("step1".to_string(), &node);

        let cond_gt = ExCondition::NumericCompare {
            node_id: "step1".to_string(),
            field: "score".to_string(),
            op: ">".to_string(),
            value: 10.0,
        };
        assert!(cond_gt.evaluate(&node_outputs));

        let cond_eq = ExCondition::NumericCompare {
            node_id: "step1".to_string(),
            field: "score".to_string(),
            op: "==".to_string(),
            value: 42.0,
        };
        assert!(cond_eq.evaluate(&node_outputs));

        let cond_lt = ExCondition::NumericCompare {
            node_id: "step1".to_string(),
            field: "score".to_string(),
            op: "<".to_string(),
            value: 100.0,
        };
        assert!(cond_lt.evaluate(&node_outputs));
    }

    #[test]
    fn test_condition_evaluation_all_any() {
        let mut node_outputs: HashMap<ExNodeId, &ExNode> = HashMap::new();
        let mut node = ExNode::new("step1", ExNodeKind::Task, "Step 1");
        node.output = Some(serde_json::json!({"x": 1.0, "y": 2.0}));
        node_outputs.insert("step1".to_string(), &node);

        // All: x == 1 AND y == 2
        let all_cond = ExCondition::All(vec![
            ExCondition::NumericCompare {
                node_id: "step1".to_string(),
                field: "x".to_string(),
                op: "==".to_string(),
                value: 1.0,
            },
            ExCondition::NumericCompare {
                node_id: "step1".to_string(),
                field: "y".to_string(),
                op: "==".to_string(),
                value: 2.0,
            },
        ]);
        assert!(all_cond.evaluate(&node_outputs));

        // Any: x == 99 OR y == 2
        let any_cond = ExCondition::Any(vec![
            ExCondition::NumericCompare {
                node_id: "step1".to_string(),
                field: "x".to_string(),
                op: "==".to_string(),
                value: 99.0,
            },
            ExCondition::NumericCompare {
                node_id: "step1".to_string(),
                field: "y".to_string(),
                op: "==".to_string(),
                value: 2.0,
            },
        ]);
        assert!(any_cond.evaluate(&node_outputs));

        // Always
        assert!(ExCondition::Always.evaluate(&node_outputs));
    }

    #[test]
    fn test_get_ready_nodes() {
        let mut g = make_graph("test");
        g.add_node(ExNode::new("step1", ExNodeKind::Task, "Step 1"));
        g.add_node(ExNode::new("step2", ExNodeKind::Task, "Step 2"));
        g.add_edge("start", "step1", None);
        g.add_edge("step1", "step2", None);

        // Only step1 should be ready (start completed, step1 pending)
        // Since start defaults to Pending, we need to mark it completed
        // start is already Completed by default in new() -- step1 should be ready immediately
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
    fn test_complete_task_updates_fan_out() {
        let mut g = make_graph("test");
        let tasks = vec![("a".to_string(), "A".to_string())];
        let _ = g.add_fan_out("single", "single-join", tasks, "start");

        assert!(g
            .complete_task("a", serde_json::json!({"ok": true}))
            .is_ok());
        assert_eq!(g.fan_out_groups[0].completed_count, 1);
        let summary = g.fan_out_summary();
        assert_eq!(summary[0].1, 1);
        assert_eq!(summary[0].2, 1);
    }

    #[test]
    fn test_is_complete() {
        let mut g = make_graph("test");
        assert!(!g.is_complete());
        g.set_node_state("end", ExNodeState::Completed).unwrap();
        assert!(g.is_complete());
    }

    #[test]
    fn test_reset() {
        let mut g = make_graph("test");
        g.add_node(ExNode::new("step1", ExNodeKind::Task, "Step 1"));
        // start is already Completed by default in new()
        assert_eq!(g.count_by_state(&ExNodeState::Completed), 1); // start

        g.reset();
        // After reset: start->Completed (preserved), step1->Pending, end->Pending = 2 Pending + 1 Completed
        assert_eq!(g.count_by_state(&ExNodeState::Pending), 2);
        assert_eq!(g.count_by_state(&ExNodeState::Completed), 1);
    }

    #[test]
    fn test_count_by_state() {
        let mut g = make_graph("test");
        g.add_node(ExNode::new("step1", ExNodeKind::Task, "Step 1"));
        // start=Completed, step1=Pending, end=Pending
        assert_eq!(g.count_by_state(&ExNodeState::Pending), 2);
        assert_eq!(g.count_by_state(&ExNodeState::Completed), 1);
        g.set_node_state("step1", ExNodeState::Completed).unwrap();
        assert_eq!(g.count_by_state(&ExNodeState::Completed), 2); // start + step1
        assert_eq!(g.count_by_state(&ExNodeState::Pending), 1); // end only
    }

    #[test]
    fn test_add_condition() {
        let mut g = make_graph("test");
        g.add_node(ExNode::new("step1", ExNodeKind::Task, "Step 1"));
        g.add_node(ExNode::new("true_branch", ExNodeKind::Task, "True"));
        g.add_node(ExNode::new("false_branch", ExNodeKind::Task, "False"));
        g.add_edge("start", "step1", None);

        g.add_condition(
            "cond1",
            "Check result",
            ExCondition::OutputMatches {
                node_id: "step1".to_string(),
                expected: serde_json::json!({"status": "ok"}),
            },
            "step1",
            "true_branch",
            "false_branch",
        );

        assert!(g.nodes.contains_key("cond1"));
        assert_eq!(g.nodes["cond1"].kind, ExNodeKind::Condition);

        // Verify edges: step1 -> cond1 -> true, cond1 -> false
        let cond_edges: Vec<&ExEdge> = g
            .edges
            .iter()
            .filter(|e| e.from == "cond1" || e.to == "cond1")
            .collect();
        assert_eq!(cond_edges.len(), 3); // step1->cond1, cond1->true, cond1->false
    }

    // -- CoreDag tests ------------------------------------------------------

    #[test]
    fn test_empty_dag() {
        let dag: CoreDag<String> = CoreDag::new();
        assert!(dag.topological_sort().unwrap().is_empty());
        assert!(dag.nodes.is_empty());
    }

    #[test]
    fn test_single_node() {
        let mut dag: CoreDag<String> = CoreDag::new();
        dag.add_node("a".into(), "task A".into(), vec![]);
        assert_eq!(dag.nodes.len(), 1);
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
        // Register the forward edge b -> a
        dag.edges.entry("b".into()).or_default().insert("a".into());
        dag.recompute_entry_points();

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
        // width: level 1 (a) = 1, level 2 (b, c) = 2, level 3 (d) = 1 -> max = 2
        assert_eq!(metrics.width, 2);
        // depth: a -> b -> d = 3, a -> c -> d = 3
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
    fn test_into_core_dag() {
        struct SimpleGraph {
            nodes: Vec<String>,
            edges: Vec<(String, String)>,
        }

        impl IntoCoreDag<String, SimpleGraph> for SimpleGraph {
            fn into_core_dag(source: SimpleGraph) -> CoreDag<String> {
                let mut dag = CoreDag::new();
                for n in &source.nodes {
                    dag.add_node(n.clone(), n.clone(), vec![]);
                }
                for (from, to) in &source.edges {
                    // Add dependency edges: `to` depends on `from`
                    if let Some(node) = dag.nodes.get_mut(to) {
                        node.dependencies.push(from.clone());
                    }
                    dag.edges
                        .entry(from.clone())
                        .or_default()
                        .insert(to.clone());
                }
                dag.recompute_entry_points();
                dag
            }
        }

        let graph = SimpleGraph {
            nodes: vec!["a".to_string(), "b".to_string()],
            edges: vec![("a".to_string(), "b".to_string())],
        };
        let dag: CoreDag<String> = SimpleGraph::into_core_dag(graph);
        assert!(dag.nodes.contains_key("a"));
        assert!(dag.nodes.contains_key("b"));
    }
}
