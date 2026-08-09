//! Service-level Resilience module — F-GAP-27
//!
//! **Scope — Service-level resilience:** circuit breaking, failover,
//! self-healing patterns, and cascading degradation handling.
//!
//! This module owns the service-level strategies that keep requests flowing
//! when individual nodes falter: tripping circuits to prevent cascading
//! failures, routing requests to healthy replicas, and applying self-healing
//! retry/backoff policies.  It does **not** manage node-level health tracking,
//! heartbeat detection, isolation groups, or recovery plans — those belong
//! to the `fault_tolerance` module.
//!
//! Both modules are complementary: `resilience` answers *"how do we keep
//! serving through transient failures?"* while `fault_tolerance` answers
//! *"is the node alive?"* and *"how do we bring it back?"*

/// Chaos engineering fault injection — only compiled when `chaos-testing` feature is enabled.
#[cfg(feature = "chaos-testing")]
pub mod chaos;
pub mod hyper_resilience;
