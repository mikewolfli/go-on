//! S11: Capability Graph
//!
//! Tracks agent capabilities as a directed graph where edges represent
//! "agent A can hand off to agent B for capability C".
//! Used by the router to pick the best next agent in a chain.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

/// A single capability declaration by an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityDecl {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
}

/// Directed edge: agent A → agent B has handoff weight for capability C
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityEdge {
    pub from_agent: String,
    pub to_agent: String,
    pub capability: String,
    /// Weight 0.0–1.0; higher = preferred handoff path
    pub weight: f32,
}

/// In-memory capability graph
#[derive(Debug, Default)]
pub struct CapabilityGraph {
    /// agent_name → list of declared capabilities
    capabilities: HashMap<String, Vec<CapabilityDecl>>,
    /// directed edges for handoff routing
    edges: Vec<CapabilityEdge>,
}

impl CapabilityGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_agent(&mut self, agent: &str, decls: Vec<CapabilityDecl>) {
        self.capabilities.insert(agent.to_string(), decls);
    }

    pub fn add_edge(&mut self, edge: CapabilityEdge) {
        self.edges.push(edge);
    }

    /// Find the best handoff target from `from_agent` that supports `capability`.
    pub fn best_handoff(&self, from_agent: &str, capability: &str) -> Option<&str> {
        self.edges
            .iter()
            .filter(|e| e.from_agent == from_agent && e.capability == capability)
            .max_by(|a, b| {
                a.weight
                    .partial_cmp(&b.weight)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|e| e.to_agent.as_str())
    }

    /// Return all agents that declare a given capability tag
    pub fn agents_with_tag(&self, tag: &str) -> Vec<&str> {
        self.capabilities
            .iter()
            .filter(|(_, decls)| decls.iter().any(|d| d.tags.iter().any(|t| t == tag)))
            .map(|(agent, _)| agent.as_str())
            .collect()
    }

    /// All declared capabilities for an agent
    pub fn agent_capabilities(&self, agent: &str) -> Vec<&CapabilityDecl> {
        self.capabilities
            .get(agent)
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    pub fn total_agents(&self) -> usize {
        self.capabilities.len()
    }
    pub fn total_edges(&self) -> usize {
        self.edges.len()
    }

    /// Count agents tagged "high_risk" (or "high-risk").
    pub fn high_risk_count(&self) -> usize {
        self.capabilities
            .values()
            .filter(|decls| {
                decls
                    .iter()
                    .any(|d| d.tags.iter().any(|t| t == "high_risk" || t == "high-risk"))
            })
            .count()
    }

    /// Count agents tagged "deprecated".
    pub fn deprecated_count(&self) -> usize {
        self.capabilities
            .values()
            .filter(|decls| {
                decls
                    .iter()
                    .any(|d| d.tags.iter().any(|t| t == "deprecated"))
            })
            .count()
    }

    /// Set of all declared capability names across all agents
    pub fn all_capability_names(&self) -> HashSet<&str> {
        self.capabilities
            .values()
            .flat_map(|decls| decls.iter().map(|d| d.name.as_str()))
            .collect()
    }

    /// Find the shortest path (by hop count) from `from_agent` to any agent
    /// that can perform `capability`, with at most `max_hops` steps.
    pub fn find_path(
        &self,
        from_agent: &str,
        capability: &str,
        max_hops: usize,
    ) -> Option<Vec<String>> {
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
        for edge in &self.edges {
            adjacency
                .entry(edge.from_agent.as_str())
                .or_default()
                .push(edge.to_agent.as_str());
        }

        let mut visited: HashSet<&str> = HashSet::new();
        let mut parent: HashMap<&str, &str> = HashMap::new();
        let mut depth: HashMap<&str, usize> = HashMap::new();
        let mut queue: VecDeque<&str> = VecDeque::new();

        visited.insert(from_agent);
        depth.insert(from_agent, 0);
        queue.push_back(from_agent);

        while let Some(current) = queue.pop_front() {
            let current_depth = *depth.get(current).unwrap_or(&0);

            if current != from_agent {
                if let Some(decls) = self.capabilities.get(current) {
                    if decls
                        .iter()
                        .any(|d| d.name == capability || d.tags.iter().any(|t| t == capability))
                    {
                        let mut path = vec![current.to_string()];
                        let mut cursor = current;
                        while let Some(prev) = parent.get(cursor).copied() {
                            path.push(prev.to_string());
                            cursor = prev;
                        }
                        path.reverse();
                        return Some(path);
                    }
                }
            }

            if current_depth >= max_hops {
                continue;
            }

            if let Some(neighbors) = adjacency.get(current) {
                for &next in neighbors {
                    if visited.insert(next) {
                        parent.insert(next, current);
                        depth.insert(next, current_depth + 1);
                        queue.push_back(next);
                    }
                }
            }
        }

        None
    }

    /// Detect cycles in the graph using DFS.
    pub fn detect_cycles(&self) -> Vec<Vec<String>> {
        let mut cycles: Vec<Vec<String>> = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();
        let mut in_stack: HashSet<String> = HashSet::new();
        let mut path: Vec<String> = Vec::new();
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
        for edge in &self.edges {
            adjacency
                .entry(edge.from_agent.as_str())
                .or_default()
                .push(edge.to_agent.as_str());
        }

        fn dfs(
            node: &str,
            adjacency: &HashMap<&str, Vec<&str>>,
            visited: &mut HashSet<String>,
            in_stack: &mut HashSet<String>,
            path: &mut Vec<String>,
            cycles: &mut Vec<Vec<String>>,
        ) {
            visited.insert(node.to_string());
            in_stack.insert(node.to_string());
            path.push(node.to_string());

            if let Some(neighbors) = adjacency.get(node) {
                for &next in neighbors {
                    if !visited.contains(next) {
                        dfs(next, adjacency, visited, in_stack, path, cycles);
                    } else if in_stack.contains(next) {
                        if let Some(pos) = path.iter().position(|n| n.as_str() == next) {
                            cycles.push(path[pos..].to_vec());
                        }
                    }
                }
            }

            path.pop();
            in_stack.remove(node);
        }

        let all_nodes: Vec<String> = self.capabilities.keys().cloned().collect();
        for node in &all_nodes {
            if !visited.contains(node) {
                dfs(
                    node,
                    &adjacency,
                    &mut visited,
                    &mut in_stack,
                    &mut path,
                    &mut cycles,
                );
            }
        }

        cycles
    }

    /// Check if `target` is reachable from `source` through handoff edges.
    pub fn is_reachable(&self, source: &str, target: &str) -> bool {
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
        for edge in &self.edges {
            adjacency
                .entry(edge.from_agent.as_str())
                .or_default()
                .push(edge.to_agent.as_str());
        }

        let mut visited: HashSet<&str> = HashSet::new();
        let mut stack: Vec<&str> = vec![source];
        visited.insert(source);

        while let Some(current) = stack.pop() {
            if current == target {
                return true;
            }
            if let Some(neighbors) = adjacency.get(current) {
                for &next in neighbors {
                    if visited.insert(next) {
                        stack.push(next);
                    }
                }
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_graph_empty() {
        let g = CapabilityGraph::new();
        assert_eq!(g.total_agents(), 0);
    }

    #[test]
    fn test_register_agent() {
        let mut g = CapabilityGraph::new();
        g.register_agent(
            "agent_a",
            vec![CapabilityDecl {
                name: "code".to_string(),
                description: "".to_string(),
                tags: vec!["code".to_string()],
            }],
        );
        assert_eq!(g.total_agents(), 1);
    }

    #[test]
    fn test_add_edge_and_best_handoff() {
        let mut g = CapabilityGraph::new();
        g.register_agent("agent_a", vec![]);
        g.register_agent("agent_b", vec![]);
        g.add_edge(CapabilityEdge {
            from_agent: "agent_a".to_string(),
            to_agent: "agent_b".to_string(),
            capability: "code_review".to_string(),
            weight: 0.8,
        });
        assert_eq!(g.total_edges(), 1);
        assert_eq!(g.best_handoff("agent_a", "code_review"), Some("agent_b"));
    }

    #[test]
    fn test_best_handoff_returns_highest_weight() {
        let mut g = CapabilityGraph::new();
        g.register_agent("a", vec![]);
        g.register_agent("b", vec![]);
        g.register_agent("c", vec![]);
        g.add_edge(CapabilityEdge {
            from_agent: "a".into(),
            to_agent: "b".into(),
            capability: "code".into(),
            weight: 0.5,
        });
        g.add_edge(CapabilityEdge {
            from_agent: "a".into(),
            to_agent: "c".into(),
            capability: "code".into(),
            weight: 0.9,
        });
        assert_eq!(g.best_handoff("a", "code"), Some("c"));
    }

    #[test]
    fn test_best_handoff_none_when_no_edge() {
        let g = CapabilityGraph::new();
        assert_eq!(g.best_handoff("a", "missing"), None);
    }

    #[test]
    fn test_agents_with_tag() {
        let mut g = CapabilityGraph::new();
        g.register_agent(
            "agent_a",
            vec![CapabilityDecl {
                name: "code".into(),
                description: "".into(),
                tags: vec!["code".into(), "test".into()],
            }],
        );
        g.register_agent(
            "agent_b",
            vec![CapabilityDecl {
                name: "code".into(),
                description: "".into(),
                tags: vec!["code".into()],
            }],
        );
        let code_agents = g.agents_with_tag("code");
        assert_eq!(code_agents.len(), 2);
    }

    #[test]
    fn test_all_capability_names() {
        let mut g = CapabilityGraph::new();
        g.register_agent(
            "a1",
            vec![CapabilityDecl {
                name: "code".into(),
                description: "".into(),
                tags: vec![],
            }],
        );
        g.register_agent(
            "a2",
            vec![CapabilityDecl {
                name: "test".into(),
                description: "".into(),
                tags: vec![],
            }],
        );
        let names = g.all_capability_names();
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn test_agent_capabilities_returns_decls() {
        let mut g = CapabilityGraph::new();
        g.register_agent(
            "a1",
            vec![CapabilityDecl {
                name: "code".into(),
                description: "desc".into(),
                tags: vec![],
            }],
        );
        let decls = g.agent_capabilities("a1");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].description, "desc");
    }

    #[test]
    fn test_find_path_direct() {
        let mut g = CapabilityGraph::new();
        g.register_agent("a", vec![]);
        g.register_agent(
            "b",
            vec![CapabilityDecl {
                name: "code".into(),
                description: "".into(),
                tags: vec![],
            }],
        );
        g.add_edge(CapabilityEdge {
            from_agent: "a".into(),
            to_agent: "b".into(),
            capability: "handoff".into(),
            weight: 1.0,
        });
        let path = g.find_path("a", "code", 5);
        assert_eq!(path, Some(vec!["a".to_string(), "b".to_string()]));
    }

    #[test]
    fn test_find_path_multi_hop() {
        let mut g = CapabilityGraph::new();
        g.register_agent("a", vec![]);
        g.register_agent("b", vec![]);
        g.register_agent(
            "c",
            vec![CapabilityDecl {
                name: "test".into(),
                description: "".into(),
                tags: vec![],
            }],
        );
        g.add_edge(CapabilityEdge {
            from_agent: "a".into(),
            to_agent: "b".into(),
            capability: "handoff".into(),
            weight: 1.0,
        });
        g.add_edge(CapabilityEdge {
            from_agent: "b".into(),
            to_agent: "c".into(),
            capability: "handoff".into(),
            weight: 1.0,
        });
        let path = g.find_path("a", "test", 5);
        assert_eq!(
            path,
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
    }

    #[test]
    fn test_find_path_no_path() {
        let mut g = CapabilityGraph::new();
        g.register_agent("a", vec![]);
        g.register_agent(
            "b",
            vec![CapabilityDecl {
                name: "code".into(),
                description: "".into(),
                tags: vec![],
            }],
        );
        // no edge from a to b
        assert_eq!(g.find_path("a", "code", 5), None);
    }

    #[test]
    fn test_find_path_exceeds_max_hops() {
        let mut g = CapabilityGraph::new();
        g.register_agent("a", vec![]);
        g.register_agent("b", vec![]);
        g.register_agent(
            "c",
            vec![CapabilityDecl {
                name: "code".into(),
                description: "".into(),
                tags: vec![],
            }],
        );
        g.add_edge(CapabilityEdge {
            from_agent: "a".into(),
            to_agent: "b".into(),
            capability: "handoff".into(),
            weight: 1.0,
        });
        g.add_edge(CapabilityEdge {
            from_agent: "b".into(),
            to_agent: "c".into(),
            capability: "handoff".into(),
            weight: 1.0,
        });
        // max_hops=0 means only direct neighbor (b), not b->c
        assert_eq!(g.find_path("a", "code", 0), None);
    }

    #[test]
    fn test_find_path_self_capability_not_counted() {
        // The start agent should not be considered as a valid target
        // because the path must go through at least one edge.
        let mut g = CapabilityGraph::new();
        g.register_agent(
            "a",
            vec![CapabilityDecl {
                name: "code".into(),
                description: "".into(),
                tags: vec![],
            }],
        );
        assert_eq!(g.find_path("a", "code", 5), None);
    }

    #[test]
    fn test_detect_cycles_no_cycle() {
        let mut g = CapabilityGraph::new();
        g.register_agent("a", vec![]);
        g.register_agent("b", vec![]);
        g.register_agent("c", vec![]);
        g.add_edge(CapabilityEdge {
            from_agent: "a".into(),
            to_agent: "b".into(),
            capability: "x".into(),
            weight: 1.0,
        });
        g.add_edge(CapabilityEdge {
            from_agent: "b".into(),
            to_agent: "c".into(),
            capability: "x".into(),
            weight: 1.0,
        });
        assert!(g.detect_cycles().is_empty());
    }

    #[test]
    fn test_detect_cycles_with_cycle() {
        let mut g = CapabilityGraph::new();
        g.register_agent("a", vec![]);
        g.register_agent("b", vec![]);
        g.add_edge(CapabilityEdge {
            from_agent: "a".into(),
            to_agent: "b".into(),
            capability: "x".into(),
            weight: 1.0,
        });
        g.add_edge(CapabilityEdge {
            from_agent: "b".into(),
            to_agent: "a".into(),
            capability: "y".into(),
            weight: 1.0,
        });
        let cycles = g.detect_cycles();
        assert_eq!(cycles.len(), 1);
    }

    #[test]
    fn test_is_reachable_true() {
        let mut g = CapabilityGraph::new();
        g.register_agent("a", vec![]);
        g.register_agent("b", vec![]);
        g.register_agent("c", vec![]);
        g.add_edge(CapabilityEdge {
            from_agent: "a".into(),
            to_agent: "b".into(),
            capability: "x".into(),
            weight: 1.0,
        });
        g.add_edge(CapabilityEdge {
            from_agent: "b".into(),
            to_agent: "c".into(),
            capability: "x".into(),
            weight: 1.0,
        });
        assert!(g.is_reachable("a", "c"));
    }

    #[test]
    fn test_is_reachable_false() {
        let mut g = CapabilityGraph::new();
        g.register_agent("a", vec![]);
        g.register_agent("b", vec![]);
        assert!(!g.is_reachable("a", "b"));
    }
}
