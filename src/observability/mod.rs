//! Observability module — telemetry, metrics, performance monitoring, and alerting.
//!
//! Initially exported as a public module for out-of-tree consumers who need
//! observability without an ACP dependency. Modules live one-per-file under
//! `observability/`.

pub mod alert_manager;
pub mod live_performance;
pub mod memory_health;
pub mod metrics_exporter;
#[allow(clippy::module_inception)]
pub mod observability;
pub mod performance;
pub mod provenance;
pub mod telemetry;
pub mod telemetry_enhanced;

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

// ── Re-exports ──────────────────────────────────────────────────────────────

/// Configuration for initialising the observability stack independently of ACP.
#[derive(Debug, Clone)]
pub struct ObservabilityConfig {
    /// Service name for telemetry resource attributes.
    pub service_name: String,
    /// Enable OpenTelemetry tracing.
    pub otel_enabled: bool,
    /// OTLP endpoint for exporting spans (empty = stdout).
    pub otlp_endpoint: Option<String>,
    /// Sampling ratio for telemetry (0.0–1.0).
    pub sample_ratio: f64,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            service_name: "go-on".to_string(),
            otel_enabled: false,
            otlp_endpoint: None,
            sample_ratio: 1.0,
        }
    }
}

/// Global singleton for the independent observability stack.
static OBSERVABILITY_STACK: OnceLock<ObservabilityStackInner> = OnceLock::new();

/// Inner observability stack — fields are initialized once and never read directly.
/// They are stored as a singleton to keep the subsystems alive for the program lifetime.
#[allow(dead_code)]
struct ObservabilityStackInner {
    pub performance_monitor: Arc<Mutex<crate::observability::performance::PerformanceMonitor>>,
    pub telemetry_runtime: crate::observability::telemetry::TelemetryRuntime,
    pub alert_manager: crate::observability::alert_manager::AlertManager,
}

/// Initialize the independent observability stack (idempotent — only first call wins).
pub fn init_independent_stack(config: &ObservabilityConfig) -> bool {
    if OBSERVABILITY_STACK.get().is_some() {
        return false;
    }

    let performance_monitor = crate::observability::performance::init_performance_monitoring();

    let runtime_config = crate::config::RuntimeConfig {
        otel_enabled: config.otel_enabled,
        otel_sample_ratio: config.sample_ratio,
        otel_service_name: config.service_name.clone(),
        otel_endpoint: config.otlp_endpoint.clone(),
        ..Default::default()
    };
    let telemetry_runtime = crate::observability::telemetry::TelemetryRuntime::new(&runtime_config);

    let alert_manager = crate::observability::alert_manager::AlertManager::new(
        crate::observability::alert_manager::default_alert_rules(),
    );

    OBSERVABILITY_STACK
        .set(ObservabilityStackInner {
            performance_monitor,
            telemetry_runtime,
            alert_manager,
        })
        .is_ok()
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Lock a Mutex, recovering from poison with a log.
///
/// Standard pattern used across performance/live_performance modules.
pub fn lock_mutex<T>(mtx: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mtx.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!(target: "observability", "Mutex poisoned in lock_mutex — recovering");
            poisoned.into_inner()
        }
    }
}
