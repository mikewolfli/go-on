//! Governance module — policy enforcement, runtime controls, and strategy engine.
//!
//! All governance components are wired through the `HarnessBus` strategy engine
//! which provides a single evaluate/validate/verify entry point for CapabilityBus.

pub mod audit;
pub mod drift;
pub mod hardening;
pub mod harness_bus;
pub mod pua;
pub mod rationalization;
pub mod rbac;
pub mod review_controls;
pub mod runtime_controls;
pub mod security_governor;
