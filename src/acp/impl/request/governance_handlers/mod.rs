//! governance_handlers -- Governance status, plan, audit, and remediation handlers.
//!
//! Split from the monolithic `governance_handlers.rs` into sub-modules:
//! - `audit`:   Audit event types and persistence (append / load)
//! - `status`:  `handle_governance_status` — comprehensive governance status
//! - `plan`:    Plan get/update handlers and norms helper
//! - `actions`: `handle_governance_audit_recent`, `handle_governance_remediate`,
//!   `handle_governance_config_save`

pub(super) mod actions;
pub(super) mod audit;
pub(super) mod plan;
pub(super) mod status;

// ── Import everything from parent `request.rs` so sub-modules
//     can access sibling-pack items via `super::<name>`.
use super::*;

// ── Re-exports for parent module (`request.rs` `use self::governance_handlers::*`) ──

#[allow(unused_imports)] // re-exports used by sibling packs via super::governance_handlers::*
pub(super) use audit::{
    append_governance_audit_event, load_governance_audit_events, GovernanceAuditEvent,
};

#[allow(unused_imports)]
pub(super) use status::governance_status_payload;

#[allow(unused_imports)]
pub(super) use plan::{
    governance_plan_get_payload, governance_plan_update_payload, norms_tracked_for,
};

#[allow(unused_imports)]
pub(super) use actions::{
    governance_audit_recent_payload, governance_config_save_payload, governance_remediate_payload,
};
