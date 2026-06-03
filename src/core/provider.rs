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
///
/// BLUE56-GAP-A07: Will be wired to AcpServer in upcoming integration.
/// The trait is structurally complete but no concrete implementation has
/// been instantiated and no `Arc<dyn OrchestrationProvider>` has been
/// injected into `AcpServer`. Tracked under BLUE56-GAP-A07 — remove this
/// annotation once wiring is complete.
#[allow(dead_code)] // activated, formerly BLUE56-GAP-A07 — public API surface
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
