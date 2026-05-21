//! OPS (Operations) pack module
//!
//! Handles operational requests: breaker management, observability, security
//! baseline, release readiness, harness status, cache/vector maintenance, etc.

mod handlers;
mod health;
mod security;

// Re-export all pub(super) handler functions so that
// `use self::ops_pack::*;` in `request.rs` continues to work.
pub(super) use handlers::{
    handle_breaker_recovery, handle_breaker_reset, handle_breaker_status, handle_cache_clear,
    handle_harness_status, handle_lock_status, handle_maintenance_gc, handle_observability_alerts,
    handle_release_readiness, handle_security_baseline, handle_vector_clear,
};
pub(super) use health::{
    circuit_state_label, collect_degraded_services, degradation_level_label, health_status_label,
    recovery_action,
};
pub(super) use security::build_security_baseline_payload;
