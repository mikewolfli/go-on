//! Structural validation suite — unit-style invariant checks.
//!
//! The real end-to-end suite that spawns the backend lives in
//! `tests/acp_runtime_rpc_integration.rs`, `tests/transport_parity_integration.rs`,
//! `tests/protocol_consistency_integration.rs`, `tests/e2e_integration.rs`, etc.
//!
//! Each sub-module below validates subsystem behavior via the production API
//! with real temporary files (SQLite / archive dirs / sandboxes); they do NOT
//! spawn the full backend process. Pure default-value invariants live as
//! inline unit tests in `src/` (see `core/config/defaults.rs` and
//! `governance/status.rs`).

pub mod test_memory_persistence;
pub mod test_multimodal;
pub mod test_security;
pub mod test_self_evolution;
