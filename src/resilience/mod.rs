//! Resilience module — hyper-resilience, circuit breaking, failover, and self-healing.
//!
//! Implements F-GAP-27: Hyper-resilience for super-node failover,
//! multi-level circuit breaking, cascading degradation handling,
//! and automated self-healing capabilities.

pub mod hyper_resilience;

#[allow(unused_imports)]
pub use hyper_resilience::{
    CircuitBreaker, CircuitState, DegradationLevel, FailoverGroup, FailureMode, HealingReport,
    HyperResilienceEngine, ResilienceConfig, ResilienceLevel, ResilienceProfile, SelfHealingAction,
    SystemHealth,
};
