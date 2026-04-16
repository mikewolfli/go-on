//! Phase 6: Execution Graph with Branching and Conditional Transitions
//!
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! ExecutionGraph provides branching and conditional execution that will be
//! traversed by the orchestrator once node completion callbacks are wired.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExecutionNodeKind {
    Plan,
    Act,
    Verify,
    Review,
    Summarize,
    Finalize,
    Branch,
    Join,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionNode {
    pub id: String,
    pub kind: ExecutionNodeKind,
    pub prerequisites: HashSet<String>,
    pub successors: Vec<(String, String)>, // next node, condition (e.g., "pass", "fail")
    pub state: String,                     // "pending", "running", "done", "failed"
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub checkpoint: Option<String>,
    pub retries: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionGraph {
    pub id: String,
    pub root: String,
    pub nodes: HashMap<String, ExecutionNode>,
    pub current_node: String,
    pub completed_nodes: HashSet<String>,
    pub failed_nodes: HashSet<String>,
}

impl ExecutionGraph {
    pub fn new(root: ExecutionNode) -> Self {
        let root_id = root.id.clone();
        let mut nodes = HashMap::new();
        nodes.insert(root_id.clone(), root);
        Self {
            id: format!(
                "graph-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
            ),
            root: root_id.clone(),
            nodes,
            current_node: root_id,
            completed_nodes: HashSet::new(),
            failed_nodes: HashSet::new(),
        }
    }

    pub fn add_node(&mut self, node: ExecutionNode) {
        self.nodes.insert(node.id.clone(), node);
    }

    pub fn add_transition(&mut self, from: String, to: String, condition: String) {
        if let Some(node) = self.nodes.get_mut(&from) {
            node.successors.push((to, condition));
        }
    }

    pub fn complete_node(&mut self, node_id: &str, output: serde_json::Value) {
        if let Some(node) = self.nodes.get_mut(node_id) {
            node.state = "done".to_string();
            node.output = Some(output);
            self.completed_nodes.insert(node_id.to_string());
        }
    }

    pub fn fail_node(&mut self, node_id: &str, _error: String) {
        if let Some(node) = self.nodes.get_mut(node_id) {
            node.state = "failed".to_string();
            self.failed_nodes.insert(node_id.to_string());
        }
    }

    pub fn is_complete(&self) -> bool {
        self.failed_nodes.is_empty()
            && self
                .nodes
                .values()
                .all(|n| n.state == "done" || n.state == "pending")
    }
}
