//! S11: Capability Graph
//!
//! Tracks agent capabilities as a directed graph where edges represent
//! "agent A can hand off to agent B for capability C".
//! Used by the router to pick the best next agent in a chain.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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
    pub fn new() -> Self { Self::default() }

    pub fn register_agent(&mut self, agent: &str, decls: Vec<CapabilityDecl>) {
        self.capabilities.insert(agent.to_string(), decls);
    }

    pub fn add_edge(&mut self, edge: CapabilityEdge) {
        self.edges.push(edge);
    }

    /// Find the best handoff target from `from_agent` that supports `capability`.
    pub fn best_handoff(&self, from_agent: &str, capability: &str) -> Option<&str> {
        self.edges.iter()
            .filter(|e| e.from_agent == from_agent && e.capability == capability)
            .max_by(|a, b| a.weight.partial_cmp(&b.weight).unwrap_or(std::cmp::Ordering::Equal))
            .map(|e| e.to_agent.as_str())
    }

    /// Return all agents that declare a given capability tag
    pub fn agents_with_tag(&self, tag: &str) -> Vec<&str> {
        self.capabilities.iter()
            .filter(|(_, decls)| decls.iter().any(|d| d.tags.contains(&tag.to_string())))
            .map(|(agent, _)| agent.as_str())
            .collect()
    }

    /// All declared capabilities for an agent
    pub fn agent_capabilities(&self, agent: &str) -> Vec<&CapabilityDecl> {
        self.capabilities.get(agent).map(|v| v.iter().collect()).unwrap_or_default()
    }

    pub fn total_agents(&self) -> usize { self.capabilities.len() }
    pub fn total_edges(&self) -> usize { self.edges.len() }

    /// Set of all declared capability names across all agents
    pub fn all_capability_names(&self) -> HashSet<&str> {
        self.capabilities.values()
            .flat_map(|decls| decls.iter().map(|d| d.name.as_str()))
            .collect()
    }
}
