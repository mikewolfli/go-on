//! Resilience module — hyper-resilience, circuit breaking, failover, and self-healing.
//!
//! Implements F-GAP-27: Hyper-resilience for super-node failover,
//! multi-level circuit breaking, cascading degradation handling,
//! and automated self-healing capabilities.

pub mod chaos;
pub mod hyper_resilience;
