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
/// Used in bootstrap.rs (skill_count), orchestration/provider_impl.rs (impl),
/// and re-exported from `lib.rs` for the public API.
/// Reserved for future expansion of the provider interface.
pub trait OrchestrationProvider: Send + Sync {
    /// Return the number of registered skills (for diagnostics / profiling).
    fn skill_count(&self) -> usize;
}

/// Default implementation of `OrchestrationProvider`.
///
/// Maintains an in-memory registry of skills.
pub struct DefaultOrchestrationProvider {
    skills: Mutex<HashMap<String, Arc<dyn std::any::Any + Send + Sync>>>,
}

impl Default for DefaultOrchestrationProvider {
    fn default() -> Self {
        Self {
            skills: Mutex::new(HashMap::new()),
        }
    }
}

impl OrchestrationProvider for DefaultOrchestrationProvider {
    fn skill_count(&self) -> usize {
        self.skills.lock().map(|skills| skills.len()).unwrap_or(0)
    }
}
