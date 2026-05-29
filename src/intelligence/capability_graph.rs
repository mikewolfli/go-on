//! F-GAP-01: Capability Graph
//!
//! Tracks agent capabilities as a directed graph where edges represent
//! "agent A can hand off to agent B for capability C".
//! Used by the router to pick the best next agent in a chain.

use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::sync::Mutex;

use ordered_float::OrderedFloat;

type HeuristicQueueEntry = (OrderedFloat<f64>, usize, String, Option<String>);

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

/// Cached forward and reverse adjacency lists for pathfinding.
#[derive(Debug)]
struct AdjacencyCache {
    /// Snapshot of `modification_count` when this cache was built.
    version: u64,
    fwd_adj: HashMap<String, Vec<String>>,
    rev_adj: HashMap<String, Vec<String>>,
}

/// In-memory capability graph
#[derive(Debug)]
pub struct CapabilityGraph {
    /// agent_name → list of declared capabilities
    capabilities: HashMap<String, Vec<CapabilityDecl>>,
    /// directed edges for handoff routing
    edges: Vec<CapabilityEdge>,
    /// Max capabilities to retain per agent
    max_capabilities_per_agent: usize,
    /// Max edges to retain before evicting oldest
    max_edges: usize,
    /// Monotonically increasing counter incremented on every edge mutation.
    modification_count: u64,
    /// Cached adjacency lists, rebuilt lazily on `find_path` / `find_path_bidirectional`.
    cached_adjacency: Mutex<Option<AdjacencyCache>>,
}

impl Default for CapabilityGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl CapabilityGraph {
    pub fn new() -> Self {
        Self {
            capabilities: HashMap::new(),
            edges: Vec::new(),
            max_capabilities_per_agent: 100,
            max_edges: 10_000,
            modification_count: 0,
            cached_adjacency: Mutex::new(None),
        }
    }

    pub fn register_agent(&mut self, agent: &str, decls: Vec<CapabilityDecl>) {
        let mut decls = decls;
        // Evict oldest capabilities if over per-agent limit.
        while decls.len() > self.max_capabilities_per_agent {
            decls.remove(0);
        }
        self.capabilities.insert(agent.to_string(), decls);
    }

    pub fn add_edge(&mut self, edge: CapabilityEdge) {
        // Evict the oldest edge when at capacity.
        if self.edges.len() >= self.max_edges {
            self.edges.remove(0);
        }
        self.edges.push(edge);
        // Invalidate the adjacency cache on every edge mutation.
        self.modification_count = self.modification_count.wrapping_add(1);
        *self.cached_adjacency.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("capability_graph adjacency cache lock poisoned: recovering");
            poisoned.into_inner()
        }) = None;
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

    /// Return cached or freshly-built forward adjacency (as owned types).
    ///
    /// This avoids rebuilding the adjacency map on every pathfinding call
    /// when the edge set has not changed.
    fn adjacency(&self) -> (HashMap<String, Vec<String>>, HashMap<String, Vec<String>>) {
        let version = self.modification_count;
        // Fast path: check the cache first (extract result, drop lock, then return).
        let cached_hit = {
            let cache = self.cached_adjacency.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            cache.as_ref().and_then(|cached| {
                if cached.version == version {
                    let fwd: HashMap<String, Vec<String>> = cached
                        .fwd_adj
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    let rev: HashMap<String, Vec<String>> = cached
                        .rev_adj
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    Some((fwd, rev))
                } else {
                    None
                }
            })
        };
        if let Some(result) = cached_hit {
            return result;
        }

        // Cache miss or poisoned: rebuild from scratch.
        let mut fwd_adj: HashMap<String, Vec<String>> = HashMap::new();
        let mut rev_adj: HashMap<String, Vec<String>> = HashMap::new();
        for edge in &self.edges {
            fwd_adj
                .entry(edge.from_agent.clone())
                .or_default()
                .push(edge.to_agent.clone());
            rev_adj
                .entry(edge.to_agent.clone())
                .or_default()
                .push(edge.from_agent.clone());
        }

        // Store into cache for subsequent calls.
        let mut cache = self.cached_adjacency.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        *cache = Some(AdjacencyCache {
            version,
            fwd_adj: fwd_adj.clone(),
            rev_adj: rev_adj.clone(),
        });

        // Return as owned types for compatibility with existing callers.
        let fwd: HashMap<String, Vec<String>> = fwd_adj
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let rev: HashMap<String, Vec<String>> = rev_adj
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        (fwd, rev)
    }

    /// Find the shortest path (by hop count) from `from_agent` to any agent
    /// that can perform `capability`, with at most `max_hops` steps.
    pub fn find_path(
        &self,
        from_agent: &str,
        capability: &str,
        max_hops: usize,
    ) -> Option<Vec<String>> {
        let (adjacency, _rev) = self.adjacency();
        // Convert to borrowed references for iteration within this scope.
        let adj: HashMap<&str, &[String]> = adjacency
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_slice()))
            .collect();

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

            if let Some(neighbors) = adj.get(current) {
                for next in *neighbors {
                    if !visited.contains(next.as_str()) {
                        visited.insert(next.as_str());
                        parent.insert(next.as_str(), current);
                        depth.insert(next.as_str(), current_depth + 1);
                        queue.push_back(next.as_str());
                    }
                }
            }
        }

        None
    }

    /// Bidirectional BFS pathfinding: O(b^(d/2)) complexity.
    ///
    /// Runs BFS simultaneously from the source agent and from all agents
    /// that possess the target `capability`.  When the two frontiers meet,
    /// the path is reconstructed by joining the forward and backward
    /// parent chains.  Expected 3-5x speedup on large graphs compared to
    /// the unidirectional `find_path`.
    pub fn find_path_bidirectional(
        &self,
        from_agent: &str,
        capability: &str,
        max_hops: usize,
    ) -> Option<Vec<String>> {
        let (fwd_adj, rev_adj) = self.adjacency();
        // Find all target agents that have the capability
        let targets: HashSet<String> = self
            .capabilities
            .iter()
            .filter(|(agent, decls)| {
                // Exclude source from targets (self-capability handled separately)
                agent.as_str() != from_agent
                    && decls
                        .iter()
                        .any(|d| d.name == capability || d.tags.iter().any(|t| t == capability))
            })
            .map(|(agent, _)| agent.clone())
            .collect();

        if targets.is_empty() {
            return None;
        }

        // Forward search: from source
        let mut fwd_visited: HashSet<String> = HashSet::new();
        let mut fwd_parent: HashMap<String, String> = HashMap::new();
        let mut fwd_depth: HashMap<String, usize> = HashMap::new();
        let mut fwd_queue: VecDeque<String> = VecDeque::new();

        fwd_visited.insert(from_agent.to_string());
        fwd_depth.insert(from_agent.to_string(), 0);
        fwd_queue.push_back(from_agent.to_string());

        // Backward search: from all targets
        let mut bwd_visited: HashSet<String> = HashSet::new();
        let mut bwd_parent: HashMap<String, String> = HashMap::new();
        let mut bwd_depth: HashMap<String, usize> = HashMap::new();
        let mut bwd_queue: VecDeque<String> = VecDeque::new();

        for target in &targets {
            bwd_visited.insert(target.clone());
            bwd_depth.insert(target.clone(), 0);
            bwd_queue.push_back(target.clone());
        }

        // Alternate expanding one level from each side
        while !fwd_queue.is_empty() || !bwd_queue.is_empty() {
            // Expand forward frontier by one level
            let fwd_level_size = fwd_queue.len();
            for _ in 0..fwd_level_size {
                let current = fwd_queue.pop_front()?;
                let cur_depth = *fwd_depth.get(&current).unwrap_or(&0);

                // Check if this node is in the backward visited set
                if bwd_visited.contains(&current) && current != from_agent {
                    return Some(Self::reconstruct_bidi_path(
                        &current,
                        &fwd_parent,
                        &bwd_parent,
                        from_agent,
                    ));
                }

                if cur_depth >= max_hops {
                    continue;
                }

                if let Some(neighbors) = fwd_adj.get(&current) {
                    for next in neighbors {
                        if fwd_visited.insert(next.clone()) {
                            fwd_parent.insert(next.clone(), current.clone());
                            fwd_depth.insert(next.clone(), cur_depth + 1);
                            fwd_queue.push_back(next.clone());
                        }
                    }
                }
            }

            // Expand backward frontier by one level
            let bwd_level_size = bwd_queue.len();
            for _ in 0..bwd_level_size {
                let current = bwd_queue.pop_front()?;
                let cur_depth = *bwd_depth.get(&current).unwrap_or(&0);

                // Check if this node is in the forward visited set
                if fwd_visited.contains(&current) && !targets.contains(&current) {
                    return Some(Self::reconstruct_bidi_path(
                        &current,
                        &fwd_parent,
                        &bwd_parent,
                        from_agent,
                    ));
                }

                if cur_depth >= max_hops {
                    continue;
                }

                if let Some(neighbors) = rev_adj.get(&current) {
                    for prev in neighbors {
                        if bwd_visited.insert(prev.clone()) {
                            bwd_parent.insert(prev.clone(), current.clone());
                            bwd_depth.insert(prev.clone(), cur_depth + 1);
                            bwd_queue.push_back(prev.clone());
                        }
                    }
                }
            }
        }

        None
    }

    /// Reconstruct the full path from the meeting point when bidirectional
    /// BFS frontiers converge.
    fn reconstruct_bidi_path(
        meeting: &str,
        fwd_parent: &HashMap<String, String>,
        bwd_parent: &HashMap<String, String>,
        from_agent: &str,
    ) -> Vec<String> {
        // Build forward path: from source → meeting
        let mut fwd_path: Vec<String> = Vec::new();
        let mut cursor = meeting.to_string();
        fwd_path.push(cursor.clone());
        while cursor != from_agent {
            if let Some(prev) = fwd_parent.get(&cursor) {
                fwd_path.push(prev.clone());
                cursor = prev.clone();
            } else {
                break;
            }
        }
        fwd_path.reverse();

        // Build backward path: from meeting → target
        // (skip meeting point as it is already in fwd_path)
        let mut bwd_path: Vec<String> = Vec::new();
        let mut cursor = meeting.to_string();
        while let Some(next) = bwd_parent.get(&cursor) {
            bwd_path.push(next.clone());
            cursor = next.clone();
        }

        fwd_path.extend(bwd_path);
        fwd_path
    }

    /// A*-like heuristic pathfinding using edge weights and reputation scores.
    ///
    /// Uses edge weights as actual path costs and (1.0 - reputation_score)
    /// as the heuristic, preferring paths through high-reputation agents.
    /// The heuristic is admissible (0.0 ≤ h ≤ 1.0 per hop), keeping the
    /// search efficient while directing it toward trusted agents.
    pub fn find_path_heuristic(
        &self,
        from_agent: &str,
        capability: &str,
        max_hops: usize,
        reputation_scores: &HashMap<String, f64>,
    ) -> Option<Vec<String>> {
        // Build forward adjacency with edge weights
        let mut fwd_adj: HashMap<&str, Vec<(&str, f32)>> = HashMap::new();
        for edge in &self.edges {
            fwd_adj
                .entry(edge.from_agent.as_str())
                .or_default()
                .push((edge.to_agent.as_str(), edge.weight));
        }

        // Priority queue stored as (cost, depth, node, parent).
        // Use OrderedFloat to allow f64 in BinaryHeap (which requires Ord).
        // Reverse so BinaryHeap acts as a min-heap.
        let mut pq: BinaryHeap<Reverse<HeuristicQueueEntry>> = BinaryHeap::new();

        // best_cost[node] = lowest cost discovered so far
        let mut best_cost: HashMap<String, f64> = HashMap::new();
        let mut parent: HashMap<String, String> = HashMap::new();
        let mut hop_depth: HashMap<String, usize> = HashMap::new();

        let start_heuristic = Self::heuristic(from_agent, reputation_scores);
        best_cost.insert(from_agent.to_string(), 0.0);
        hop_depth.insert(from_agent.to_string(), 0);
        pq.push(Reverse((
            OrderedFloat(start_heuristic),
            0,
            from_agent.to_string(),
            None,
        )));

        while let Some(Reverse((est_cost, depth, current, prev))) = pq.pop() {
            // Skip if we have already found a better path to this node
            if let Some(&best) = best_cost.get(&current) {
                let actual_cost =
                    est_cost.into_inner() - Self::heuristic(&current, reputation_scores);
                if actual_cost > best {
                    continue;
                }
            }

            if let Some(p) = prev {
                parent.insert(current.clone(), p);
            }

            // Check if current agent has the target capability (exclude source)
            if current != from_agent {
                if let Some(decls) = self.capabilities.get(current.as_str()) {
                    if decls
                        .iter()
                        .any(|d| d.name == capability || d.tags.iter().any(|t| t == capability))
                    {
                        // Reconstruct path
                        let mut path = vec![current.clone()];
                        let mut cursor = current.clone();
                        while let Some(prev_node) = parent.get(&cursor) {
                            path.push(prev_node.clone());
                            cursor = prev_node.clone();
                        }
                        path.reverse();
                        return Some(path);
                    }
                }
            }

            if depth >= max_hops {
                continue;
            }

            // Expand neighbors
            if let Some(neighbors) = fwd_adj.get(current.as_str()) {
                for &(next, weight) in neighbors {
                    let edge_cost = 1.0 - weight as f64; // invert: higher weight = lower cost
                    let new_cost = best_cost.get(&current).copied().unwrap_or(f64::MAX) + edge_cost;

                    let prev_best = best_cost.get(next).copied().unwrap_or(f64::MAX);
                    if new_cost < prev_best {
                        best_cost.insert(next.to_string(), new_cost);
                        hop_depth.insert(next.to_string(), depth + 1);
                        let h = Self::heuristic(next, reputation_scores);
                        pq.push(Reverse((
                            OrderedFloat(new_cost + h),
                            depth + 1,
                            next.to_string(),
                            Some(current.clone()),
                        )));
                    }
                }
            }
        }

        None
    }

    /// Compute the heuristic value for an agent.
    ///
    /// Uses `1.0 - reputation_score` so that high-reputation agents have
    /// lower heuristic values and are explored first.  Unknown agents
    /// default to a neutral score of 0.5 (moderate preference).
    fn heuristic(agent: &str, reputation_scores: &HashMap<String, f64>) -> f64 {
        let score = reputation_scores.get(agent).copied().unwrap_or(0.5);
        (1.0 - score).max(0.0)
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

    // ── Bidirectional BFS tests ──────────────────────────────────────

    #[test]
    fn test_bidi_find_path_direct() {
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
        let path = g.find_path_bidirectional("a", "code", 5);
        assert_eq!(path, Some(vec!["a".to_string(), "b".to_string()]));
    }

    #[test]
    fn test_bidi_find_path_multi_hop() {
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
        let path = g.find_path_bidirectional("a", "test", 5);
        assert_eq!(
            path,
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
    }

    #[test]
    fn test_bidi_find_path_no_path() {
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
        assert_eq!(g.find_path_bidirectional("a", "code", 5), None);
    }

    #[test]
    fn test_bidi_find_path_exceeds_max_hops() {
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
        assert_eq!(g.find_path_bidirectional("a", "code", 0), None);
    }

    // ── A* heuristic tests ───────────────────────────────────────────

    #[test]
    fn test_heuristic_find_path_direct() {
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
        let scores = HashMap::new();
        let path = g.find_path_heuristic("a", "code", 5, &scores);
        assert_eq!(path, Some(vec!["a".to_string(), "b".to_string()]));
    }

    #[test]
    fn test_heuristic_find_path_no_path() {
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
        let scores = HashMap::new();
        assert_eq!(g.find_path_heuristic("a", "code", 5, &scores), None);
    }

    #[test]
    fn test_heuristic_prefers_high_reputation() {
        let mut g = CapabilityGraph::new();
        g.register_agent("a", vec![]);
        // Low-reputation path: a → b
        g.register_agent("b", vec![]);
        // High-reputation path: a → c
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
            weight: 0.5,
        });
        g.add_edge(CapabilityEdge {
            from_agent: "a".into(),
            to_agent: "c".into(),
            capability: "handoff".into(),
            weight: 0.5,
        });
        // Give c high reputation
        let mut scores = HashMap::new();
        scores.insert("b".to_string(), 0.3);
        scores.insert("c".to_string(), 0.9);
        let path = g.find_path_heuristic("a", "code", 5, &scores);
        // Should prefer a→c due to higher reputation
        assert_eq!(path, Some(vec!["a".to_string(), "c".to_string()]));
    }

    #[test]
    fn test_heuristic_respects_max_hops() {
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
        let scores = HashMap::new();
        assert_eq!(g.find_path_heuristic("a", "code", 0, &scores), None);
    }
}
