//! Provider traits for architectural dependency inversion.
//!
//! These traits define the boundary between `acp` (protocol layer) and
//! `orchestration` (business logic layer). ACP depends on these traits
//! rather than directly importing orchestration types, ensuring a clean
//! layered architecture: acp → core ← orchestration.
//!
//! BLUE56-GAP-A07: Each concrete orchestration type implements the
//! corresponding trait, and ACP accepts `Arc<dyn OrchestrationProvider>`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Provides orchestration capabilities to the ACP server.
///
/// This trait hides the concrete orchestration implementation behind
/// a stable interface, allowing ACP to remain agnostic of the
/// orchestration module internals.
/// Trait interface — designed for AcpServer wiring.
/// Used in bootstrap.rs (skill_count) and orchestration/provider_impl.rs (impl).
pub trait OrchestrationProvider: Send + Sync {
    /// Register a skill for later discovery.
    #[allow(dead_code)] // BLUE56-GAP-A07: reserved for ACP → orchestration wiring
    fn register_skill(&self, name: &str, skill: Arc<dyn std::any::Any + Send + Sync>);

    /// Check whether a named skill has been registered.
    #[allow(dead_code)] // BLUE56-GAP-A07: reserved for ACP → orchestration wiring
    fn has_skill(&self, name: &str) -> bool;

    /// Return the number of registered skills (for diagnostics / profiling).
    /// Used in bootstrap.rs for architecture boundary verification.
    fn skill_count(&self) -> usize;

    /// Record a capability execution for self-model feedback.
    #[allow(dead_code)] // BLUE56-GAP-A07: reserved for ACP → orchestration wiring
    fn record_capability_execution(&self, capability_id: &str, duration_ms: u64, success: bool);

    /// Retrieve execution statistics for a capability.
    #[allow(dead_code)] // BLUE56-GAP-A07: reserved for ACP → orchestration wiring
    fn capability_stats(&self, capability_id: &str) -> Option<CapabilityExecutionStats>;
}

/// Statistics for a single capability's execution history.
/// Used by DefaultOrchestrationProvider and OrchestrationProviderImpl
/// for tracking capability execution metrics.
#[derive(Debug, Clone)]
pub struct CapabilityExecutionStats {
    pub total_executions: u64,
    pub success_count: u64,
    pub total_duration_ms: u64,
    pub last_execution_ms: u64,
}

/// Default implementation of `OrchestrationProvider`.
///
/// Maintains an in-memory registry of skills and tracks capability
/// execution statistics for self-model feedback.
pub struct DefaultOrchestrationProvider {
    skills: Mutex<HashMap<String, Arc<dyn std::any::Any + Send + Sync>>>,
    stats: Mutex<HashMap<String, CapabilityExecutionStats>>,
}

impl Default for DefaultOrchestrationProvider {
    fn default() -> Self {
        Self {
            skills: Mutex::new(HashMap::new()),
            stats: Mutex::new(HashMap::new()),
        }
    }
}

impl OrchestrationProvider for DefaultOrchestrationProvider {
    fn register_skill(&self, name: &str, skill: Arc<dyn std::any::Any + Send + Sync>) {
        match self.skills.lock() {
            Ok(mut skills) => {
                skills.insert(name.to_string(), skill);
                tracing::debug!(
                    target: "go_on::core::provider",
                    skill = %name,
                    total = skills.len(),
                    "OrchestrationProvider: skill registered"
                );
            }
            Err(e) => {
                tracing::error!(
                    target: "go_on::core::provider",
                    skill = %name,
                    error = %e,
                    "OrchestrationProvider: failed to acquire skills lock"
                );
            }
        }
    }

    fn has_skill(&self, name: &str) -> bool {
        self.skills
            .lock()
            .map(|skills| skills.contains_key(name))
            .unwrap_or(false)
    }

    fn skill_count(&self) -> usize {
        self.skills.lock().map(|skills| skills.len()).unwrap_or(0)
    }

    fn record_capability_execution(&self, capability_id: &str, duration_ms: u64, success: bool) {
        match self.stats.lock() {
            Ok(mut stats) => {
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
                tracing::debug!(
                    target: "go_on::core::provider",
                    capability = %capability_id,
                    duration_ms,
                    success,
                    total_executions = entry.total_executions,
                    "OrchestrationProvider: capability execution recorded"
                );
            }
            Err(e) => {
                tracing::error!(
                    target: "go_on::core::provider",
                    capability = %capability_id,
                    error = %e,
                    "OrchestrationProvider: failed to acquire stats lock"
                );
            }
        }
    }

    fn capability_stats(&self, capability_id: &str) -> Option<CapabilityExecutionStats> {
        self.stats
            .lock()
            .ok()
            .and_then(|stats| stats.get(capability_id).cloned())
    }
}
