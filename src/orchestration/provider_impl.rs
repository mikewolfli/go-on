//! Concrete implementation of `OrchestrationProvider` for the orchestration module.
//!
//! This file bridges the `core::provider::OrchestrationProvider` trait with the
//! actual orchestration types (SkillRegistry, ToolRegistry).
//! ACP depends on the trait; orchestration implements the trait.
//!
//! BLUE56-GAP-A07: Architecture dependency inversion boundary.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::core::provider::{CapabilityExecutionStats, OrchestrationProvider};
use crate::orchestration::skill::{Skill, SkillRegistry};
use crate::orchestration::tool::ToolRegistry;

/// Wraps orchestration dependencies behind the `OrchestrationProvider` trait.
///
/// This struct is the concrete adapter that the orchestration module provides
/// to ACP. ACP receives `Arc<dyn OrchestrationProvider>` and never needs to
/// import orchestration concrete types.
///
/// Implements OrchestrationProvider — used via trait dispatch.
/// Truly dead until wired into ServerBuilder (E-GAP-12); keeps the architectural
/// boundary ready for injection. Struct field and impl code are #[allow(dead_code)].
#[allow(dead_code)]
pub struct OrchestrationProviderImpl {
    skill_registry: Arc<Mutex<SkillRegistry>>,
    #[allow(dead_code)]
    // Reserved for future full-auto flow creation (E-GAP-12)
    tool_registry: Arc<ToolRegistry>,
    capability_stats: Mutex<HashMap<String, CapabilityExecutionStats>>,
}

impl OrchestrationProviderImpl {
    /// Create a new provider impl (reserved for ACP wiring, E-GAP-12).
    /// Truly dead until wired into ServerBuilder; kept for the architectural boundary.
    #[allow(dead_code)]
    pub fn new(
        skill_registry: Arc<Mutex<SkillRegistry>>,
        tool_registry: Arc<ToolRegistry>,
    ) -> Self {
        Self {
            skill_registry,
            tool_registry,
            capability_stats: Mutex::new(HashMap::new()),
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

    fn record_capability_execution(&self, capability_id: &str, duration_ms: u64, success: bool) {
        if let Ok(mut stats) = self.capability_stats.lock() {
            let entry =
                stats
                    .entry(capability_id.to_string())
                    .or_insert(CapabilityExecutionStats {
                        total_executions: 0,
                        success_count: 0,
                        total_duration_ms: 0,
                        last_execution_ms: 0,
                    });
            entry.total_executions += 1;
            if success {
                entry.success_count += 1;
            }
            entry.total_duration_ms += duration_ms;
            entry.last_execution_ms = duration_ms;
        }
    }

    fn capability_stats(&self, capability_id: &str) -> Option<CapabilityExecutionStats> {
        self.capability_stats
            .lock()
            .ok()
            .and_then(|stats| stats.get(capability_id).cloned())
    }
}

/// A minimal generic Skill wrapper for the provider layer.
/// Used via trait dispatch in OrchestrationProviderImpl::register_skill.
/// Truly dead until OrchestrationProviderImpl is wired.
#[allow(dead_code)]
struct GenericSkill {
    name: String,
}

#[allow(dead_code)]
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
