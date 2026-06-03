//! e2e — End-to-end integration test suite for go-on.
//!
//! Each sub-module covers a complete end-to-end workflow across multiple
//! go-on subsystems. Tests use in-memory type construction and structural
//! validation to verify invariants without requiring external infrastructure.

// Intentional: Tests in this module use only a subset of each module's API.
// Individual test modules may declare helper types (e.g. FlNodeIdentity,
// FederatedRound) that are internal to this crate and not re-exported.
// `#[allow(dead_code)]` is on the entire module rather than per-item to keep
// the test code clean and avoid cluttering helpers with individual annotations.
#![allow(dead_code)]

pub mod test_distributed_dag_e2e;
pub mod test_federated_learning_e2e;
pub mod test_hitl_approval_e2e;
pub mod test_memory_persistence_e2e;
pub mod test_multimodal_e2e;
pub mod test_security_e2e;
pub mod test_self_evolution_e2e;
