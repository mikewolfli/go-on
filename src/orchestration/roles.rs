//! Phase 5: Role-Specialized Multi-Agent Collaboration
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! Agent roles and handoff contracts define multi-agent delegation patterns,
//! to be integrated into the agent orchestrator once role routing is implemented.
//!
//! S1 (blue35): Added `AgentRole::Custom(String)` + `RoleDefinition` + `RoleRegistry`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// Built-in + extensible agent roles.
/// `Custom(name)` allows user-defined roles declared in `[[agents.roles]]` config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum AgentRole {
    Planner,
    Researcher,
    Coder,
    Tester,
    Reviewer,
    /// User-defined role with a lowercase name, e.g. "security_auditor"
    Custom(String),
}

impl AgentRole {
    /// Canonical lower-case string representation
    pub fn as_str(&self) -> &str {
        match self {
            AgentRole::Planner => "planner",
            AgentRole::Researcher => "researcher",
            AgentRole::Coder => "coder",
            AgentRole::Tester => "tester",
            AgentRole::Reviewer => "reviewer",
            AgentRole::Custom(n) => n.as_str(),
        }
    }
}

// ───────────────────────────────────────────────
// S1: RoleDefinition + RoleRegistry
// ───────────────────────────────────────────────

/// Full definition for a custom agent role
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub industry: String,
    /// Keywords used by rank_execution_agents; mirrors the built-in role keyword lists
    pub keywords: Vec<String>,
    /// Allowed tool names for this role
    pub allowed_tools: Vec<String>,
    /// Max tool calls per turn
    pub max_tool_calls: usize,
    /// Token budget per turn
    pub token_budget: usize,
    /// Timeout in seconds
    pub timeout_seconds: u64,
}

/// Runtime registry of custom role definitions.
/// Populated from `[[agents.custom_roles]]` config at startup.
#[derive(Debug, Default)]
pub struct RoleRegistry {
    roles: HashMap<String, RoleDefinition>,
}

static ROLE_REGISTRY: OnceLock<RwLock<RoleRegistry>> = OnceLock::new();

pub fn role_registry() -> &'static RwLock<RoleRegistry> {
    ROLE_REGISTRY.get_or_init(|| RwLock::new(RoleRegistry::new()))
}

pub fn role_registry_keywords_for(name: &str) -> Vec<String> {
    role_registry()
        .read()
        .map(|registry| registry.keywords_for(name))
        .unwrap_or_default()
}

pub fn role_registry_count() -> usize {
    role_registry()
        .read()
        .map(|registry| registry.roles.len())
        .unwrap_or(0)
}

pub fn role_registry_industry_for(name: &str) -> Option<String> {
    role_registry().read().ok().and_then(|registry| {
        registry
            .get(name)
            .map(|definition| definition.industry.clone())
    })
}

pub fn install_role_registry(definitions: HashMap<String, RoleDefinition>) {
    let lock = role_registry();
    if let Ok(mut registry) = lock.write() {
        registry.roles = definitions;
    }
}

impl RoleRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, def: RoleDefinition) {
        self.roles.insert(def.name.clone(), def);
    }

    pub fn get(&self, name: &str) -> Option<&RoleDefinition> {
        self.roles.get(name)
    }

    pub fn keywords_for(&self, name: &str) -> Vec<String> {
        self.roles
            .get(name)
            .map(|d| d.keywords.clone())
            .unwrap_or_default()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.roles.contains_key(name)
    }

    pub fn all(&self) -> Vec<&RoleDefinition> {
        let mut v: Vec<_> = self.roles.values().collect();
        v.sort_by_key(|d| d.name.as_str());
        v
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleSpecification {
    pub role: AgentRole,
    pub tier: String, // "primary", "fallback"
    pub allowed_tools: Vec<String>,
    pub max_tool_calls: usize,
    pub token_budget: usize,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffContract {
    pub from_role: AgentRole,
    pub to_role: AgentRole,
    pub objective: String,
    pub constraints: Vec<String>,
    pub evidence_pointers: Vec<String>,
    pub failure_modes: Vec<String>,
    pub expected_outputs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffContext {
    pub contract: HandoffContract,
    pub project_state: serde_json::Value,
    pub episodic_memory: serde_json::Value,
    pub prior_results: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleOutput {
    pub role: AgentRole,
    pub success: bool,
    pub deliverables: serde_json::Value,
    pub confidence: f32,
    pub failure_signals: Option<Vec<String>>,
    pub artifacts: Vec<String>,
}

pub struct RoleSpecifications;
impl RoleSpecifications {
    pub fn planner() -> RoleSpecification {
        RoleSpecification {
            role: AgentRole::Planner,
            tier: "primary".to_string(),
            allowed_tools: vec!["read_file".to_string(), "search_files".to_string()],
            max_tool_calls: 10,
            token_budget: 4000,
            timeout_seconds: 60,
        }
    }

    pub fn researcher() -> RoleSpecification {
        RoleSpecification {
            role: AgentRole::Researcher,
            tier: "primary".to_string(),
            allowed_tools: vec!["read_file".to_string(), "search_files".to_string()],
            max_tool_calls: 20,
            token_budget: 6000,
            timeout_seconds: 120,
        }
    }

    pub fn coder() -> RoleSpecification {
        RoleSpecification {
            role: AgentRole::Coder,
            tier: "primary".to_string(),
            allowed_tools: vec!["read_file".to_string(), "apply_patch".to_string()],
            max_tool_calls: 15,
            token_budget: 6000,
            timeout_seconds: 120,
        }
    }

    pub fn tester() -> RoleSpecification {
        RoleSpecification {
            role: AgentRole::Tester,
            tier: "primary".to_string(),
            allowed_tools: vec!["run_tests".to_string(), "inspect_git_diff".to_string()],
            max_tool_calls: 10,
            token_budget: 3000,
            timeout_seconds: 180,
        }
    }

    pub fn reviewer() -> RoleSpecification {
        RoleSpecification {
            role: AgentRole::Reviewer,
            tier: "primary".to_string(),
            allowed_tools: vec!["read_file".to_string(), "inspect_git_diff".to_string()],
            max_tool_calls: 5,
            token_budget: 4000,
            timeout_seconds: 90,
        }
    }
}
