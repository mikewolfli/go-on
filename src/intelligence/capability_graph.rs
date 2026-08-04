//! F-GAP-01: Capability Graph
//!
//! Tracks agent capability declarations. Used by the capability bus
//! (`sense`/`decide`) to enumerate candidate agents and by the agent registry
//! to register per-agent capability declarations.
//!
//! The former handoff-edge subsystem (edges, `find_path`/`find_path_bidirectional`/
//! `find_path_heuristic`/`detect_cycles`/`is_reachable`/`best_handoff`) was removed:
//! it had zero production callers — only `register_agent`, `agents_with_tag`,
//! `all_capability_names`, and `total_agents` are consumed by the routing chain.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A single capability declaration by an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityDecl {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
}

/// In-memory capability graph
#[derive(Debug)]
pub struct CapabilityGraph {
    /// agent_name → list of declared capabilities
    capabilities: HashMap<String, Vec<CapabilityDecl>>,
    /// Max capabilities to retain per agent
    max_capabilities_per_agent: usize,
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
            max_capabilities_per_agent: 100,
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

    /// Return all agents that declare a given capability tag
    pub fn agents_with_tag(&self, tag: &str) -> Vec<&str> {
        self.capabilities
            .iter()
            .filter(|(_, decls)| decls.iter().any(|d| d.tags.iter().any(|t| t == tag)))
            .map(|(agent, _)| agent.as_str())
            .collect()
    }

    pub fn total_agents(&self) -> usize {
        self.capabilities.len()
    }

    /// Set of all declared capability names across all agents
    pub fn all_capability_names(&self) -> HashSet<&str> {
        self.capabilities
            .values()
            .flat_map(|decls| decls.iter().map(|d| d.name.as_str()))
            .collect()
    }
}
