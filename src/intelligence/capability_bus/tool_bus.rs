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
use crate::orchestration::tool::ToolRegistry;

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
