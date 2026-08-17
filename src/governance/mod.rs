//! Governance module — policy enforcement, runtime controls, and strategy engine.
//!
//! All governance components are wired through the `HarnessBus` strategy engine
//! which provides a single evaluate/validate/verify entry point for CapabilityBus.

/// Canonical schema version of the `governance.status` report payload.
///
/// Single source of truth: `governance_handlers::status` reports this value as
/// `governance.schema_version` / `governance.schema` /
/// `governance.artifact_contract.schema_version` (and the report `version`).
/// Bump this constant when the report shape changes — never inline a second
/// copy elsewhere.
pub const GOVERNANCE_SCHEMA_VERSION: &str = "blue26-governance-v1";

pub mod approval_chain;
pub mod audit;
pub mod drift;
pub mod guardian;
pub mod hardening;
pub mod harness_bus;
pub mod pua;
pub mod rationalization;
pub mod rbac;
pub mod review_controls;
pub mod runtime_controls;
pub mod security_governor;
pub mod status;
pub mod tool_capability;
