//! e2e — End-to-end integration test suite for go-on.
//!
//! Each sub-module covers a complete end-to-end workflow across multiple
//! go-on subsystems. Tests use in-memory type construction and structural
//! validation to verify invariants without requiring external infrastructure.

pub mod test_hitl_approval_e2e;
pub mod test_memory_persistence_e2e;
pub mod test_multimodal_e2e;
pub mod test_security_e2e;
pub mod test_self_evolution_e2e;
pub mod test_server_startup_health;
