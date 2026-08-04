//! Legacy "e2e" suite — structural/unit-style validation (kept for historical
//! reasons; the real end-to-end suite that spawns the backend lives in
//! `tests/acp_runtime_rpc_integration.rs`, `tests/transport_parity_integration.rs`,
//! `tests/protocol_consistency_integration.rs`, `tests/e2e_integration.rs`, etc.).
//!
//! Each sub-module below validates invariants via in-memory type construction;
//! the names are historical and these files do NOT spawn a backend.

pub mod test_hitl_approval_e2e;
pub mod test_memory_persistence_e2e;
pub mod test_multimodal_e2e;
pub mod test_security_e2e;
pub mod test_self_evolution_e2e;
pub mod test_server_startup_health;
