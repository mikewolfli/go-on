//! Cross-node fault tolerance module — F-GAP-28 (BLUE38 §6.6)
//!
//! Provides node-level failure isolation, heartbeat-based failure detection,
//! automatic failover coordination, and quorum-based recovery.

#[cfg(feature = "profile-multi-users-server")]
pub mod node_fault_tolerance;
