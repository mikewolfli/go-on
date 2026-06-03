//! Concrete implementation of `OrchestrationProvider` for the orchestration module.
//!
//! This file bridges the `core::provider::OrchestrationProvider` trait with the
//! actual orchestration types (SkillRegistry, ToolRegistry).
//! ACP depends on the trait; orchestration implements the trait.
//!
//! BLUE56-GAP-A07: Architecture dependency inversion boundary.

use std::sync::{Arc, Mutex};

use crate::core::provider::OrchestrationProvider;
use crate::orchestration::skill::{Skill, SkillRegistry};
use crate::orchestration::tool::ToolRegistry;

/// Wraps orchestration dependencies behind the `OrchestrationProvider` trait.
///
/// This struct is the concrete adapter that the orchestration module provides
/// to ACP. ACP receives `Arc<dyn OrchestrationProvider>` and never needs to
/// import orchestration concrete types.
///
#[allow(dead_code)] // activated, formerly BLUE56-GAP-A07 — public API surface
pub struct OrchestrationProviderImpl {
    skill_registry: Arc<Mutex<SkillRegistry>>,
    #[allow(dead_code)] // Reserved for future full-auto flow creation (E-GAP-12)
    tool_registry: Arc<ToolRegistry>,
}

impl OrchestrationProviderImpl {
    /// activated, formerly BLUE56-GAP-A07
    #[allow(dead_code)] // activated — public API surface
    pub fn new(
        skill_registry: Arc<Mutex<SkillRegistry>>,
        tool_registry: Arc<ToolRegistry>,
    ) -> Self {
        Self {
            skill_registry,
            tool_registry,
        }
    }
}

impl OrchestrationProvider for OrchestrationProviderImpl {
    fn register_skill(&self, name: &str, _skill: Arc<dyn std::any::Any + Send + Sync>) {
        if let Ok(mut registry) = self.skill_registry.lock() {
            let generic = GenericSkill::new(name.to_string());
            let _ = registry.register(Arc::new(generic) as Arc<dyn Skill>);
            tracing::debug!(name = %name, "registered skill via OrchestrationProvider");
        }
    }

    fn has_skill(&self, name: &str) -> bool {
        if let Ok(registry) = self.skill_registry.lock() {
            registry.get(name).is_some()
        } else {
            false
        }
    }

    fn skill_count(&self) -> usize {
        if let Ok(registry) = self.skill_registry.lock() {
            registry.list().len()
        } else {
            0
        }
    }

    fn record_capability_execution(&self, _capability_id: &str, _duration_ms: u64, _success: bool) {
        // BLUE56-GAP-B07: wired to SelfModelCore in later step
    }
}

/// A minimal generic Skill wrapper for the provider layer.
#[allow(dead_code)] // activated, formerly BLUE56-GAP-A07 — public API surface
struct GenericSkill {
    name: String,
}

#[allow(dead_code)] // activated, formerly BLUE56-GAP-A07 — public API surface
impl GenericSkill {
    fn new(name: String) -> Self {
        Self { name }
    }
}

#[async_trait::async_trait]
impl Skill for GenericSkill {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "Generic skill registered via OrchestrationProvider"
    }

    async fn execute(&self, _input: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::json!({"status": "ok", "skill": self.name}))
    }
}
