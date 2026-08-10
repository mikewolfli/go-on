//! Application phase — error types for the evolution pipeline.
//!
//! Contains [`EvolutionLoopError`] used across the evolution lifecycle
//! (apply, verify, approval stages).

use thiserror::Error;

// ---------------------------------------------------------------------------
// EvolutionLoopError
// ---------------------------------------------------------------------------

/// Errors that can occur during evolution loop operations.
#[derive(Debug, Error)]
pub enum EvolutionLoopError {
    /// No trigger sources are configured.
    #[error("no trigger sources configured")]
    NoTriggerSources,

    /// No sandbox executor is configured.
    #[error("no sandbox executor configured")]
    NoSandbox,

    /// Patch application failed.
    #[error("patch application failed: {0}")]
    PatchApplyFailed(String),

    /// No usable code patch could be proposed for this cycle (no
    /// SelfEvolutionAgent, patch generation failed, or the trigger has no
    /// actionable file target). The cycle is skipped with this reason recorded
    /// instead of fabricating a placeholder patch that would fail whitelist
    /// validation.
    #[error("no code patch proposed: {0}")]
    ProposalUnavailable(String),

    /// Approval was rejected.
    #[error("evolution rejected: {0}")]
    Rejected(String),
}
