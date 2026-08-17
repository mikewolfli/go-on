//! F-GAP-27: Hyper-resilience — super-node failover, multi-level circuit breaking,
//! cascading degradation handling, and self-healing capabilities.
//!
//! This module provides the core resilience engine that monitors system health,
//! manages circuit breakers at multiple levels, orchestrates failover between
//! primary and replica nodes, and executes self-healing actions when degradation
//! is detected.

use std::sync::Arc;
use std::sync::{Mutex, RwLock};

// ---------------------------------------------------------------------------
// Tool-execution reporting hook (cross-layer bridge)
// ---------------------------------------------------------------------------

/// Circuit-breaker name used for orchestration-layer tool execution reporting.
///
/// The tool executor (`src/orchestration/tool/executor.rs`) maintains its own
/// local consecutive-failure counter and cannot structurally reach a
/// `HyperResilienceEngine` instance. This process-wide hook lets that layer
/// report outcomes into the unified engine so `circuit_breaker_open_count` /
/// governance status reflect tool-execution breakers.
pub(crate) const TOOL_EXECUTION_BREAKER: &str = "tool-execution";

/// Process-wide report callback set once at wiring time (see
/// `set_tool_execution_report_hook`). Invoked with `(breaker_name, success)`.
static TOOL_EXECUTION_REPORT_HOOK: std::sync::OnceLock<Arc<dyn Fn(String, bool) + Send + Sync>> =
    std::sync::OnceLock::new();

/// Install the process-wide tool-execution reporting hook. The first call
/// wins; later calls are ignored (typically only the server's HarnessBus
/// construction runs this in production).
pub(crate) fn set_tool_execution_report_hook<F>(hook: F)
where
    F: Fn(String, bool) + Send + Sync + 'static,
{
    let _ = TOOL_EXECUTION_REPORT_HOOK.set(Arc::new(hook));
}

/// Report a tool-execution outcome through the hook (no-op when no hook is
/// installed). Called by `src/orchestration/tool/executor.rs` when its local
/// circuit breaker trips.
pub(crate) fn report_tool_execution(breaker_name: &str, success: bool) {
    if let Some(hook) = TOOL_EXECUTION_REPORT_HOOK.get() {
        hook(breaker_name.to_string(), success);
    }
}

// ---------------------------------------------------------------------------
// Lock helpers
// ---------------------------------------------------------------------------

/// Acquire a lock on a Mutex, recovering from poison via the shared macro.
fn lock_mutex<T>(mtx: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    crate::lock_or_recover!(mtx, "hyper_resilience")
}

/// Acquire a read lock on a RwLock, recovering from poison via the shared macro.
fn read_lock<T>(rw: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    crate::read_or_recover!(rw, "hyper_resilience")
}

// ---------------------------------------------------------------------------
// Sub-modules
// ---------------------------------------------------------------------------

mod engine;
mod state;
mod types;

pub use engine::HyperResilienceEngine;
pub use types::CircuitBreakerState as CircuitState;
pub use types::*;

#[cfg(test)]
mod tests;
