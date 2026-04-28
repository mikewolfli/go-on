//! Execution graph — DAG with fan-out/join and conditional branching support.
//!
//! Extends the basic TaskGraph with Branch nodes (fan-out to parallel paths),
//! Join nodes (sync point after parallel execution), and Condition nodes
//! (branching based on condition evaluation). Designed for F-GAP-04.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ── Node types ──────────────────────────────────────────────────────────────

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

// ── Conditions ──────────────────────────────────────────────────────────────

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

// ── Edges ───────────────────────────────────────────────────────────────────

/// Directed edge between two nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExEdge {
    pub from: ExNodeId,
    pub to: ExNodeId,
    /// Optional label (e.g. "true" / "false" for Condition nodes)
    pub label: Option<String>,
}

// ── Fan-out groups ──────────────────────────────────────────────────────────

/// Tracks a fan-out group: branch → parallel tasks → join
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanOutGroup {
    pub group_id: String,
    pub branch_node_id: ExNodeId,
    pub join_node_id: ExNodeId,
    pub parallel_task_ids: Vec<ExNodeId>,
    pub completed_count: usize,
    pub total_count: usize,
}

// ── Execution graph ─────────────────────────────────────────────────────────

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
        let mut nodes = HashMap::new();
        let mut start_node = ExNode::new(&start_id, ExNodeKind::Start, "Start");
        start_node.state = ExNodeState::Completed;
        nodes.insert(start_id.clone(), start_node);
        nodes.insert(end_id.clone(), ExNode::new(&end_id, ExNodeKind::End, "End"));

        Self {
            nodes,
            edges: Vec::new(),
            start_node: start_id,
            end_node: end_id,
            fan_out_groups: Vec::new(),
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

    /// Create a fan-out: branch → parallel tasks → join.
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

        // Connect predecessor → branch
        self.add_edge(predecessor, &branch_id, None);

        // Connect branch → each parallel task → join
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
    /// Only returns `Task` or `Branch` nodes — structural nodes (Start/End/Join) are excluded.
    pub fn get_ready_nodes(&self) -> Vec<ExNodeId> {
        let completed: HashSet<&ExNodeId> = self
            .nodes
            .iter()
            .filter(|(_, n)| matches!(n.state, ExNodeState::Completed))
            .map(|(id, _)| id)
            .collect();

        self.nodes
            .iter()
            .filter(|(id, node)| {
                // Only Task and Branch nodes can be "ready" for execution
                if !matches!(node.kind, ExNodeKind::Task | ExNodeKind::Branch) {
                    return false;
                }
                if node.state != ExNodeState::Pending {
                    return false;
                }
                let deps: Vec<&ExNodeId> = self
                    .edges
                    .iter()
                    .filter(|e| e.to == **id)
                    .map(|e| &e.from)
                    .collect();
                deps.is_empty() || deps.iter().all(|d| completed.contains(d))
            })
            .map(|(id, _)| id.clone())
            .collect()
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
    pub fn reset(&mut self) {
        for node in self.nodes.values_mut() {
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

// ── Tests ───────────────────────────────────────────────────────────────────

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
        // start is already Completed by default in new() — step1 should be ready immediately
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
        // After reset: start→Pending, step1→Pending, end→Pending = 3
        assert_eq!(g.count_by_state(&ExNodeState::Pending), 3);
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

        // Verify edges: step1 → cond1 → true, cond1 → false
        let cond_edges: Vec<&ExEdge> = g
            .edges
            .iter()
            .filter(|e| e.from == "cond1" || e.to == "cond1")
            .collect();
        assert_eq!(cond_edges.len(), 3); // step1→cond1, cond1→true, cond1→false
    }
}
