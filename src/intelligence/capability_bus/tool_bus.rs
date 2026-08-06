//! ToolBus — Unified tool & skill sub-bus (BLUE38 §1, ARCH-13 multi-bus architecture)
//!
//! The ToolBus wraps the existing `ToolRegistry` and `SkillRegistry` into a
//! single sub-bus that the `CapabilityBus` can query for agent-aware tool
//! assignment and capability introspection.
//!
//! The execution side (a second tool-execution path) was removed: production
//! tool execution goes exclusively through
//! `orchestration::tool::executor::execute_tools_concurrent`. This bus is the
//! read-only capability view (capability matrix, agent-tool matching, skill
//! registry access, profile).
//!
//! # Integration
//!
//! ```text
//!  CapabilityBus
//!      │
//!      ├── WorkflowLearningBus
//!      ├── KnowledgeBus
//!      ├── ReputationStore
//!      ├── CapabilityGraph
//!      ├── ...
//!      └── ToolBus  ←  this module
//!              │
//!              ├── ToolRegistry  (orchestration::tool)
//!              └── SkillRegistry (orchestration::skill)
//! ```

use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

use crate::orchestration::skill::SkillRegistry;
use crate::orchestration::tool::{ToolRegistry, ToolRiskLevel};

// ---------------------------------------------------------------------------
// Descriptor – one item in the combined capability matrix
// ---------------------------------------------------------------------------

/// A unified descriptor for both tools and skills.
///
/// Returned by `capability_matrix()` so callers see a homogeneous list.
#[derive(Debug, Clone)]
pub struct ToolDescriptor {
    pub name: String,
    pub capability: String,
    pub risk_level: String,
    pub timeout_ms: u64,
    pub fallback_chain: Vec<String>,
    pub is_skill: bool,
}

// ---------------------------------------------------------------------------
// ToolBus profile
// ---------------------------------------------------------------------------

/// High-level health / status snapshot of the ToolBus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolBusProfile {
    pub total_tools: u32,
    pub total_skills: u32,
}

// ---------------------------------------------------------------------------
// ToolBus
// ---------------------------------------------------------------------------

/// Read-only capability view over the tool and skill registries.
///
/// The `CapabilityBus` holds one instance of `ToolBus` and delegates all
/// capability-lookup, agent-tool-matching and profiling to it.
pub struct ToolBus {
    tool_registry: &'static ToolRegistry,
    skill_registry: Arc<RwLock<SkillRegistry>>,
}

impl ToolBus {
    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    /// Create a new `ToolBus` wrapping the given registries.
    pub fn new(
        tool_registry: &'static ToolRegistry,
        skill_registry: Arc<RwLock<SkillRegistry>>,
    ) -> Self {
        Self {
            tool_registry,
            skill_registry,
        }
    }

    // -----------------------------------------------------------------------
    // Capability matrix
    // -----------------------------------------------------------------------

    /// Return a combined list of all tools and skills with their capability
    /// profiles.  Skills are always listed with a risk level of `"medium"` and
    /// an empty fallback chain because those concepts are not part of the
    /// `Skill` trait.
    pub fn capability_matrix(&self) -> Vec<ToolDescriptor> {
        let mut descriptors: Vec<ToolDescriptor> = Vec::new();

        // Tools
        for name in self.tool_registry.names() {
            let profile = self.tool_registry.profile(name);
            descriptors.push(ToolDescriptor {
                name: name.to_string(),
                capability: profile
                    .map(|p| p.capability.clone())
                    .unwrap_or_else(|| "unknown".to_string()),
                risk_level: profile
                    .map(|p| match p.risk_level {
                        ToolRiskLevel::Low => "low",
                        ToolRiskLevel::Medium => "medium",
                        ToolRiskLevel::High => "high",
                    })
                    .unwrap_or("medium")
                    .to_string(),
                timeout_ms: profile.map(|p| p.timeout_budget_ms).unwrap_or(30_000),
                fallback_chain: profile
                    .map(|p| p.fallback_chain.clone())
                    .unwrap_or_default(),
                is_skill: false,
            });
        }

        // Skills
        let reg = self.skill_registry.read().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        for desc in reg.list(false) {
            descriptors.push(ToolDescriptor {
                name: desc.name.clone(),
                capability: format!("skill:{}", desc.name),
                risk_level: "medium".to_string(),
                timeout_ms: 30_000,
                fallback_chain: Vec::new(),
                is_skill: true,
            });
        }

        descriptors
    }

    // -----------------------------------------------------------------------
    // Agent-tool matching
    // -----------------------------------------------------------------------

    /// Return names of tools (and skills) that are appropriate for the given
    /// `agent_role` and `task_type`.
    ///
    /// The matching heuristic is deliberately simple:
    ///
    /// * **Tools** – the tool's `capability` field is compared against both
    ///   `agent_role` and `task_type` via substring / prefix matching.  When the
    ///   agent is `"coder"` the tool `"filesystem_write"` is considered a match
    ///   because `"filesystem"` overlaps with common coding tasks.
    /// * **Skills** – the skill's name and description are matched against
    ///   `task_type` using the same `SkillRegistry::best_match_with_input`
    ///   semantics.
    ///
    /// This method will be refined as the RL feedback loop matures.
    pub fn agent_tool_match(&self, agent_role: &str, task_type: &str) -> Vec<String> {
        let mut matches: Vec<String> = Vec::new();

        let role_lower = agent_role.to_lowercase();
        let task_lower = task_type.to_lowercase();

        // Match tools by capability field.
        let reg = self.tool_registry;
        for name in reg.names() {
            let profile = reg.profile(name);
            if let Some(prof) = profile {
                let cap_lower = prof.capability.to_lowercase();
                // A tool matches if its capability overlaps with the agent
                // role or the task type.
                if cap_lower.contains(&role_lower)
                    || role_lower.contains(&cap_lower)
                    || cap_lower.contains(&task_lower)
                    || task_lower.contains(&cap_lower)
                {
                    matches.push(name.to_string());
                }
            }
        }

        // Match skills via the skill-registry's best-match logic.
        let reg = self.skill_registry.read().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        if let Some(best) = reg.best_match_with_input(
            task_type,
            &serde_json::json!({"task": task_type, "objective": task_type}),
        ) {
            if !matches.contains(&best) {
                matches.push(best);
            }
        }

        matches
    }

    // -----------------------------------------------------------------------
    // Profile
    // -----------------------------------------------------------------------

    /// Access the inner SkillRegistry for profiling / evolution tracking.
    pub fn skill_registry_ref(&self) -> &Arc<RwLock<SkillRegistry>> {
        &self.skill_registry
    }

    /// Produce a high-level profile snapshot of the ToolBus.
    pub fn profile(&self) -> ToolBusProfile {
        let total_tools = self.tool_registry.names().len() as u32;

        let total_skills = self
            .skill_registry
            .read()
            .map(|reg| reg.list(false).len() as u32)
            .unwrap_or(0);

        ToolBusProfile {
            total_tools,
            total_skills,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::skill::EchoSkill;

    fn make_bus() -> ToolBus {
        let tool_registry = ToolRegistry::new();
        let tool_registry: &'static ToolRegistry = Box::leak(Box::new(tool_registry));
        let skill_registry = Arc::new(RwLock::new(SkillRegistry::default()));
        {
            let mut skill_guard = skill_registry.write().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            let _ = skill_guard.register(Arc::new(EchoSkill));
        }
        ToolBus::new(tool_registry, skill_registry)
    }

    #[test]
    fn capability_matrix_includes_tools_and_skills() {
        let bus = make_bus();
        let matrix = bus.capability_matrix();

        // At least the 6 built-in tools.
        assert!(
            matrix.len() >= 6,
            "expected at least 6 tools, got {}",
            matrix.len()
        );

        let tool_names: Vec<&str> = matrix.iter().map(|d| d.name.as_str()).collect();
        assert!(tool_names.contains(&"read_file"), "read_file missing");
        assert!(tool_names.contains(&"write_file"), "write_file missing");
        assert!(tool_names.contains(&"search_files"), "search_files missing");
        assert!(tool_names.contains(&"apply_patch"), "apply_patch missing");
        assert!(tool_names.contains(&"run_tests"), "run_tests missing");
        assert!(
            tool_names.contains(&"inspect_git_diff"),
            "inspect_git_diff missing"
        );

        // Also includes the echo skill.
        assert!(tool_names.contains(&"builtin.echo"), "builtin.echo missing");

        // Non-skills are marked correctly.
        for desc in &matrix {
            if desc.name == "read_file" {
                assert!(!desc.is_skill, "read_file should not be a skill");
                assert_eq!(desc.risk_level, "low");
            }
        }
    }

    #[test]
    fn agent_tool_match_returns_relevant_tools() {
        let bus = make_bus();
        let matched = bus.agent_tool_match("coder", "filesystem_read");

        // At minimum "read_file" and "search_files" should match.
        assert!(
            matched.contains(&"read_file".to_string()),
            "expected read_file in matches for coder/filesystem_read, got {:?}",
            matched
        );
    }

    #[test]
    fn profile_reflects_registries() {
        let bus = make_bus();

        let prof = bus.profile();
        assert!(prof.total_tools >= 6);
        assert!(prof.total_skills >= 1);
    }
}
