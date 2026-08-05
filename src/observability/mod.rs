//! Observability module — telemetry, metrics, performance monitoring, and alerting.
//!
//! Initially exported as a public module for out-of-tree consumers who need
//! observability without an ACP dependency. Modules live one-per-file under
//! `observability/`.

pub mod alert_manager;
pub mod live_performance;
pub mod memory_health;
pub mod metrics_exporter;
pub mod performance;
pub mod provenance;
pub mod telemetry_enhanced;

/// Lock a Mutex, recovering from poison with a log.
///
/// Standard pattern used across performance/live_performance modules.
/// Delegates to shared `crate::lock_or_recover!` macro.
pub fn lock_mutex<T>(mtx: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    crate::lock_or_recover!(mtx, "observability")
}
