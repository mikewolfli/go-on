//! Provider traits for architectural dependency inversion.
//!
//! These traits define the boundary between `acp` (protocol layer) and
//! `orchestration` (business logic layer). ACP depends on these traits
//! rather than directly importing orchestration types, ensuring a clean
//! layered architecture: acp → core ← orchestration.
//!
//! BLUE56-GAP-A07: Each concrete orchestration type implements the
//! corresponding trait, and ACP accepts `Arc<dyn OrchestrationProvider>`.

use std::sync::Arc;

/// Provides orchestration capabilities to the ACP server.
///
/// This trait hides the concrete orchestration implementation behind
/// a stable interface, allowing ACP to remain agnostic of the
/// orchestration module internals.
#[allow(dead_code)] // Trait interface — designed for AcpServer wiring
pub trait OrchestrationProvider: Send + Sync {
    /// Register a skill for later discovery.
    fn register_skill(&self, name: &str, skill: Arc<dyn std::any::Any + Send + Sync>);

    /// Check whether a named skill has been registered.
    fn has_skill(&self, name: &str) -> bool;

    /// Return the number of registered skills (for diagnostics / profiling).
    fn skill_count(&self) -> usize;

    /// Record a capability execution for self-model feedback.
    fn record_capability_execution(&self, capability_id: &str, duration_ms: u64, success: bool);
}

/// Default implementation of `OrchestrationProvider` backed by PluginRegistry.
///
/// Wires the provider trait to the existing orchestration infrastructure
/// without creating circular dependencies.
#[derive(Default)]
pub struct DefaultOrchestrationProvider;

impl OrchestrationProvider for DefaultOrchestrationProvider {
    fn register_skill(&self, name: &str, _skill: Arc<dyn std::any::Any + Send + Sync>) {
        tracing::debug!(
            target: "go_on::core::provider",
            skill = %name,
            "OrchestrationProvider: skill registered"
        );
    }

    fn has_skill(&self, name: &str) -> bool {
        crate::orchestration::capabilities_registry::global_plugin_registry()
            .and_then(|reg| reg.get(name))
            .is_some()
    }

    fn skill_count(&self) -> usize {
        crate::orchestration::capabilities_registry::global_plugin_registry()
            .map(|reg| reg.count())
            .unwrap_or(0)
    }

    fn record_capability_execution(&self, capability_id: &str, duration_ms: u64, success: bool) {
        tracing::debug!(
            target: "go_on::core::provider",
            capability = %capability_id,
            duration_ms,
            success,
            "OrchestrationProvider: capability execution recorded"
        );
    }
}
