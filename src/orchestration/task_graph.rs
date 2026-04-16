//! Task graph and durable plan state for go-on (Phase 3)
//!
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! TaskGraph provides DAG-based task orchestration that will be integrated into
//! the execution engine once persistence and traversal logic is implemented.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Task node ID
type NodeId = String;

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
    pub fn add_edge(&mut self, from: String, to: String) {
        self.edges.entry(from).or_default().insert(to);
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
}
