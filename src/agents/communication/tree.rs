//! AgentTree — lightweight hierarchical agent index (BLUE70 §4)
//!
//! Flat HashMap-based agent tree with parent pointers and BFS traversal.
//! Avoids recursive structures for O(1) clone and no stack overflow risk.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::watch;

use crate::agents::communication::budget::AgentExecutionBudget;
use crate::agents::communication::lifecycle::{AgentLifecycle, AgentLifecycleBuilder};
use crate::agents::communication::message::AgentTarget;
use crate::agents::communication::path::AgentPath;

/// Metadata for an agent tree node (BLUE70 §3.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentNodeMetadata {
    pub created_at_ms: u64,
    pub role: Option<String>,
    pub model: Option<String>,
    pub token_budget: Option<u64>,
    pub depth_limit: Option<u32>,
    /// Whether to automatically fork context to children.
    pub fork_context: bool,
}

impl AgentNodeMetadata {
    /// Create metadata with default values (created now, fork_context=true).
    pub fn new() -> Self {
        Self {
            created_at_ms: crate::shared::timestamps::now_ts_ms_u64(),
            role: None,
            model: None,
            token_budget: None,
            depth_limit: None,
            fork_context: true,
        }
    }

    /// Set the agent role.
    pub fn with_role(mut self, role: &str) -> Self {
        self.role = Some(role.to_string());
        self
    }

    /// Set the model name.
    pub fn with_model(mut self, model: &str) -> Self {
        self.model = Some(model.to_string());
        self
    }

    /// Set token budget.
    pub fn with_token_budget(mut self, budget: u64) -> Self {
        self.token_budget = Some(budget);
        self
    }
}

impl Default for AgentNodeMetadata {
    fn default() -> Self {
        Self::new()
    }
}

/// Agent tree node — lightweight with parent pointer (BLUE70 §3.2).
///
/// Design notes:
/// - Uses `Vec<AgentPath>` for children instead of recursive HashMap.
/// - The flat `AgentTree.nodes` map contains ALL nodes; `children` is just an index.
/// - Clone is O(1) since AgentPath is a small `Vec<String>`.
/// - `lifecycle_tx` is a watch channel sender for event-driven status propagation (BLUE71 §7).
#[derive(Debug, Clone)]
pub struct AgentNode {
    /// Path in the agent tree.
    pub path: AgentPath,
    /// Agent name (key in AgentRegistry).
    pub agent_name: String,
    /// Parent path (None = root).
    pub parent_path: Option<AgentPath>,
    /// Child path list (traversal index only).
    pub children: Vec<AgentPath>,
    /// Node metadata.
    pub metadata: AgentNodeMetadata,
    /// Execution budget for this sub-tree.
    pub budget: AgentExecutionBudget,
    /// Lifecycle state watch channel sender (BLUE71 §7).
    pub lifecycle_tx: watch::Sender<AgentLifecycle>,
}

impl AgentNode {
    /// Create a new agent node with default lifecycle (Registered).
    pub fn new(path: AgentPath, agent_name: String, metadata: AgentNodeMetadata) -> Self {
        let (lifecycle_tx, _) = watch::channel(AgentLifecycleBuilder::registered());
        Self {
            path,
            agent_name,
            parent_path: None,
            children: Vec::new(),
            metadata,
            budget: AgentExecutionBudget::new(),
            lifecycle_tx,
        }
    }

    /// Whether this node is a leaf (no children).
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }

    /// Get the current lifecycle state.
    pub fn lifecycle(&self) -> AgentLifecycle {
        self.lifecycle_tx.borrow().clone()
    }

    /// Subscribe to lifecycle changes (watch receiver for event-driven waiting).
    pub fn subscribe_lifecycle(&self) -> watch::Receiver<AgentLifecycle> {
        self.lifecycle_tx.subscribe()
    }

    /// Update the lifecycle state and notify all watchers.
    pub fn set_lifecycle(&self, new_state: AgentLifecycle) {
        self.lifecycle_tx.send_replace(new_state);
    }
}

/// Lightweight hierarchical agent index (BLUE70 §4).
///
/// Flat HashMap storage + parent pointer indexing.
/// Traversal uses BFS iteration instead of recursion.
#[derive(Debug, Clone)]
pub struct AgentTree {
    /// Flat registry: path → AgentNode.
    nodes: HashMap<AgentPath, AgentNode>,
    /// Cached root path.
    root_path: Option<AgentPath>,
}

impl AgentTree {
    /// Create an empty agent tree.
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            root_path: None,
        }
    }

    /// Register an agent node with the given path.
    ///
    /// If the parent already exists, the node is linked as a child.
    pub fn register(
        &mut self,
        path: &AgentPath,
        agent_name: &str,
        metadata: AgentNodeMetadata,
    ) -> Result<(), String> {
        if self.nodes.contains_key(path) {
            return Err(format!("agent already registered at path: {}", path));
        }

        // Determine parent path
        let parent_path = path.parent();

        // Create the node
        let mut node = AgentNode::new(path.clone(), agent_name.to_string(), metadata);
        node.parent_path = parent_path.clone();

        // Insert into nodes map
        self.nodes.insert(path.clone(), node);

        // Link to parent
        if let Some(ref pp) = parent_path {
            if let Some(parent) = self.nodes.get_mut(pp) {
                if !parent.children.contains(path) {
                    parent.children.push(path.clone());
                }
            }
            // If parent doesn't exist yet, that's okay — the link will be
            // established when the parent is registered later.
        }

        // Cache root if this is the first node
        if self.root_path.is_none() && parent_path.is_none() {
            self.root_path = Some(path.clone());
        }

        Ok(())
    }

    /// Resolve a path to its node (O(1) lookup).
    pub fn resolve(&self, path: &AgentPath) -> Option<&AgentNode> {
        self.nodes.get(path)
    }

    /// Resolve a path to its mutable node (O(1) lookup).
    pub fn resolve_mut(&mut self, path: &AgentPath) -> Option<&mut AgentNode> {
        self.nodes.get_mut(path)
    }

    /// Resolve multiple nodes matching a pattern target.
    pub fn resolve_target(&self, target: &AgentTarget) -> Vec<&AgentNode> {
        match target {
            AgentTarget::Direct(path) => self.nodes.get(path).into_iter().collect(),
            AgentTarget::Broadcast => {
                // For broadcast from root, return all nodes
                self.nodes.values().collect()
            }
            AgentTarget::ToParent => {
                // ToParent needs a specific call context — empty by default.
                Vec::new()
            }
            AgentTarget::Pattern { prefix, suffix } => {
                let pat = crate::agents::communication::path::AgentPathPattern {
                    prefix: prefix.clone(),
                    suffix: suffix.clone(),
                };
                self.nodes
                    .values()
                    .filter(|n| n.path.matches_simple(&pat))
                    .collect()
            }
        }
    }

    /// Get all ancestors of a path (bottom-up, excluding self).
    pub fn ancestors(&self, path: &AgentPath) -> Vec<&AgentNode> {
        let mut result = Vec::new();
        let mut current = path.parent();
        while let Some(ref cp) = current {
            if let Some(node) = self.nodes.get(cp) {
                result.push(node);
                current = node.path.parent();
            } else {
                break;
            }
        }
        result
    }

    /// Get all descendants of a path (BFS iteration, excluding self).
    ///
    /// Uses iterative BFS instead of recursion to avoid stack overflow
    /// on deep agent trees.
    pub fn descendants(&self, path: &AgentPath) -> Vec<&AgentNode> {
        let mut result = Vec::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(path.clone());

        while let Some(current) = queue.pop_front() {
            if let Some(node) = self.nodes.get(&current) {
                for child_path in &node.children {
                    if let Some(child) = self.nodes.get(child_path) {
                        result.push(child);
                        queue.push_back(child_path.clone());
                    }
                }
            }
        }
        result
    }

    /// Get all descendants of a path (owned, for mutation).
    pub fn descendant_paths(&self, path: &AgentPath) -> Vec<AgentPath> {
        let mut result = Vec::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(path.clone());

        while let Some(current) = queue.pop_front() {
            if let Some(node) = self.nodes.get(&current) {
                for child_path in &node.children {
                    result.push(child_path.clone());
                    queue.push_back(child_path.clone());
                }
            }
        }
        result
    }

    /// Get the root node, if any.
    pub fn root(&self) -> Option<&AgentNode> {
        self.root_path.as_ref().and_then(|p| self.nodes.get(p))
    }

    /// Remove a sub-tree from the tree.
    ///
    /// Returns the list of removed paths (BFS collected).
    pub fn remove_subtree(&mut self, path: &AgentPath) -> Vec<AgentPath> {
        // Collect all descendant paths (BFS)
        let to_remove = self.descendant_paths(path);

        // Also remove the node itself
        let mut all_removed = vec![path.clone()];
        all_removed.extend(to_remove);

        // Remove from parent's children list — clone parent_path first to avoid borrow conflict
        let parent_path_opt = self.nodes.get(path).and_then(|n| n.parent_path.clone());
        if let Some(ref parent_path) = parent_path_opt {
            if let Some(parent) = self.nodes.get_mut(parent_path) {
                parent.children.retain(|c| c != path);
            }
        }

        // Remove all nodes from the map
        for p in &all_removed {
            self.nodes.remove(p);
        }

        // Update root cache if root was removed
        if self.root_path.as_ref() == Some(path) {
            self.root_path = self.nodes.keys().next().cloned();
        }

        all_removed
    }

    /// Total number of nodes in the tree.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Get all nodes (for iteration).
    pub fn all_nodes(&self) -> impl Iterator<Item = &AgentNode> {
        self.nodes.values()
    }
}

impl Default for AgentTree {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_path(s: &str) -> AgentPath {
        AgentPath::parse(s).unwrap()
    }

    #[test]
    fn test_register_and_resolve() {
        let mut tree = AgentTree::new();
        let path = make_path("root");
        tree.register(&path, "main", AgentNodeMetadata::new())
            .unwrap();
        assert!(tree.resolve(&path).is_some());
        assert_eq!(tree.resolve(&path).unwrap().agent_name, "main");
    }

    #[test]
    fn test_register_duplicate_fails() {
        let mut tree = AgentTree::new();
        let path = make_path("root");
        tree.register(&path, "main", AgentNodeMetadata::new())
            .unwrap();
        assert!(tree
            .register(&path, "main2", AgentNodeMetadata::new())
            .is_err());
    }

    #[test]
    fn test_parent_child_relationship() {
        let mut tree = AgentTree::new();
        let root = make_path("root");
        let child = make_path("root/research");

        tree.register(&root, "main", AgentNodeMetadata::new())
            .unwrap();
        tree.register(&child, "researcher", AgentNodeMetadata::new())
            .unwrap();

        let child_node = tree.resolve(&child).unwrap();
        assert_eq!(child_node.parent_path, Some(root.clone()));
        assert!(child_node.is_leaf());

        let root_node = tree.resolve(&root).unwrap();
        assert!(!root_node.children.is_empty());
        assert!(root_node.children.contains(&child));
    }

    #[test]
    fn test_descendants_bfs() {
        let mut tree = AgentTree::new();
        let paths = [
            make_path("root"),
            make_path("root/a"),
            make_path("root/a/a1"),
            make_path("root/b"),
            make_path("root/b/b1"),
            make_path("root/b/b1/b2"),
        ];
        for p in &paths {
            tree.register(p, "agent", AgentNodeMetadata::new()).unwrap();
        }

        let desc = tree.descendants(&make_path("root"));
        assert_eq!(desc.len(), 5); // all except root
    }

    #[test]
    fn test_ancestors() {
        let mut tree = AgentTree::new();
        tree.register(&make_path("root"), "main", AgentNodeMetadata::new())
            .unwrap();
        tree.register(&make_path("root/a"), "a", AgentNodeMetadata::new())
            .unwrap();
        tree.register(&make_path("root/a/a1"), "a1", AgentNodeMetadata::new())
            .unwrap();

        let ancestors = tree.ancestors(&make_path("root/a/a1"));
        assert_eq!(ancestors.len(), 2);
        assert_eq!(ancestors[0].agent_name, "a");
        assert_eq!(ancestors[1].agent_name, "main");
    }

    #[test]
    fn test_remove_subtree() {
        let mut tree = AgentTree::new();
        tree.register(&make_path("root"), "main", AgentNodeMetadata::new())
            .unwrap();
        tree.register(&make_path("root/a"), "a", AgentNodeMetadata::new())
            .unwrap();
        tree.register(&make_path("root/a/a1"), "a1", AgentNodeMetadata::new())
            .unwrap();
        tree.register(&make_path("root/b"), "b", AgentNodeMetadata::new())
            .unwrap();

        let removed = tree.remove_subtree(&make_path("root/a"));
        assert_eq!(removed.len(), 2); // a + a1
        assert!(tree.resolve(&make_path("root/a")).is_none());
        assert!(tree.resolve(&make_path("root/a/a1")).is_none());
        assert!(tree.resolve(&make_path("root/b")).is_some());
    }

    #[test]
    fn test_resolve_target_direct() {
        let mut tree = AgentTree::new();
        tree.register(&make_path("root"), "main", AgentNodeMetadata::new())
            .unwrap();
        let target = AgentTarget::Direct(make_path("root"));
        let results = tree.resolve_target(&target);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_resolve_target_broadcast() {
        let mut tree = AgentTree::new();
        tree.register(&make_path("root"), "main", AgentNodeMetadata::new())
            .unwrap();
        tree.register(&make_path("root/a"), "a", AgentNodeMetadata::new())
            .unwrap();
        let target = AgentTarget::Broadcast;
        let results = tree.resolve_target(&target);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_agent_node_leaf() {
        let node = AgentNode::new(
            make_path("root/leaf"),
            "leaf".to_string(),
            AgentNodeMetadata::new(),
        );
        assert!(node.is_leaf());
    }

    #[test]
    fn test_len_and_empty() {
        let mut tree = AgentTree::new();
        assert!(tree.is_empty());
        assert_eq!(tree.len(), 0);
        tree.register(&make_path("root"), "main", AgentNodeMetadata::new())
            .unwrap();
        assert!(!tree.is_empty());
        assert_eq!(tree.len(), 1);
    }
}
