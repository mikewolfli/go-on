//! Observability modules for monitoring, telemetry, and performance tracking.
//!
//! This module contains components responsible for system observability
//! in the ACP proxy system, including:
//!
//! - **Observability**: Core observability infrastructure and utilities
//! - **Performance**: Performance monitoring and optimization tracking
//! - **Telemetry**: Basic telemetry collection and reporting
//! - **Telemetry Enhanced**: Advanced telemetry with additional metrics and insights
//!
//! These modules work together to provide comprehensive visibility into
//! system behavior, performance characteristics, and operational health.

#![allow(clippy::module_inception)]

/// Acquire a lock on a `Mutex`, recovering from a poisoned state with a warning.
///
/// This is the canonical lock-acquisition helper for observability-layer mutexes.
/// All observability subsystems (alert_manager, provenance, metrics_exporter) should
/// use this helper instead of calling `mtx.lock()` directly to ensure consistent
/// poison recovery and logging.
///
/// # Future wiring
/// If a profiling/deadlock-detection wrapper is added later (e.g., timed lock
/// acquisition), this is the single point where the instrumentation is injected.
pub fn lock_mutex<T>(mtx: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mtx.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("observability mutex poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

pub mod live_performance;

pub mod alert_manager;
pub mod memory_health;
pub mod metrics_exporter;
pub mod observability;
pub mod performance;
pub mod provenance;
pub mod telemetry;
pub mod telemetry_enhanced;

// Re-exports are not needed; consumers use full crate paths.
// See main.rs for usage: crate::observability::memory_health::*
