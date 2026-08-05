//! Structural validation suite — unit-style invariant checks.
//!
//! The real end-to-end suite that spawns the backend lives in
//! `tests/acp_runtime_rpc_integration.rs`, `tests/transport_parity_integration.rs`,
//! `tests/protocol_consistency_integration.rs`, `tests/e2e_integration.rs`, etc.
//!
//! Each sub-module below validates invariants via in-memory type construction;
//! these files do NOT spawn a backend.

pub mod test_hitl_approval;
pub mod test_memory_persistence;
pub mod test_multimodal;
pub mod test_security;
pub mod test_self_evolution;
pub mod test_server_startup_health;
