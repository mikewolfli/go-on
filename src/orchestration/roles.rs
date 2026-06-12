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

/// Re-exported from [`crate::shared::role_types::RoleDefinition`].
///
/// This type was moved to `shared::role_types` to break the circular
/// dependency between `core::config::types` and `orchestration::roles`.
pub use crate::shared::role_types::RoleDefinition;

/// Runtime registry of custom role definitions.
/// Populated from `[[agents.custom_roles]]` config at startup.
#[derive(Debug, Default)]
pub struct RoleRegistry {
    roles: HashMap<String, RoleDefinition>,
}

static ROLE_REGISTRY: OnceLock<RwLock<RoleRegistry>> = OnceLock::new();

/// Returns the role registry if it has been installed.
/// Returns `None` if `install_role_registry` has not been called yet.
///
/// This avoids a race condition between installation and first read:
/// using `get_or_init` would eagerly initialize an empty registry before
/// installation data is available, creating a window where readers see
/// stale state. By returning `Option`, callers naturally fall back to
/// empty/default responses instead of racing with the installer.
pub fn role_registry() -> Option<&'static RwLock<RoleRegistry>> {
    ROLE_REGISTRY.get()
}

pub fn role_registry_keywords_for(name: &str) -> Vec<String> {
    role_registry()
        .and_then(|lock| lock.read().ok())
        .map(|registry| registry.keywords_for(name))
        .unwrap_or_default()
}

pub fn role_registry_count() -> usize {
    role_registry()
        .and_then(|lock| lock.read().ok())
        .map(|registry| registry.roles.len())
        .unwrap_or(0)
}

pub fn role_registry_industry_for(name: &str) -> Option<String> {
    role_registry()
        .and_then(|lock| lock.read().ok())
        .and_then(|registry| {
            registry
                .get(name)
                .map(|definition| definition.industry.clone())
        })
}

pub fn install_role_registry(definitions: HashMap<String, RoleDefinition>) {
    let registry = ROLE_REGISTRY.get_or_init(|| RwLock::new(RoleRegistry::new()));
    if let Ok(mut guard) = registry.write() {
        guard.roles = definitions;
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

/// Built-in role specifications, annotated for integration with the dynamic
/// [`RoleRegistry`].
///
/// When a role is not found in the registry, these hardcoded specifications
/// serve as the fallback defaults. The dynamic registry (populated from
/// `[[agents.custom_roles]]` config) can override or extend these.
///
/// # Integration
/// - [`role_registry()`] provides runtime access to dynamic role definitions
/// - [`role_registry_keywords_for()`] resolves keywords from the registry
/// - Custom roles in the registry take priority over built-in specifications
pub struct RoleSpecifications;
impl RoleSpecifications {
    /// Returns the `planner` role specification.
    /// Falls back to built-in defaults if no custom role is registered.
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

    /// Returns the `researcher` role specification.
    /// Falls back to built-in defaults if no custom role is registered.
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

    /// Returns the `coder` role specification.
    /// Falls back to built-in defaults if no custom role is registered.
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

    /// Returns the `tester` role specification.
    /// Falls back to built-in defaults if no custom role is registered.
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

    /// Returns the `reviewer` role specification.
    /// Falls back to built-in defaults if no custom role is registered.
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
