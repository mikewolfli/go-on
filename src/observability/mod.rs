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

/// An independently constructed observability stack, decoupled from `AcpServer`.
pub struct ObservabilityStack {
    /// Performance monitoring (latency, throughput, error rates).
    #[allow(dead_code)] // F-GAP-49 — reserved observability fields
    pub performance_monitor: Arc<Mutex<crate::observability::performance::PerformanceMonitor>>,
    /// Telemetry runtime for distributed tracing.
    #[allow(dead_code)] // F-GAP-49 — reserved observability fields
    pub telemetry_runtime: Arc<Mutex<crate::observability::telemetry::TelemetryRuntime>>,
    /// Alert manager for threshold-based alerting.
    #[allow(dead_code)] // F-GAP-49 — reserved observability fields
    pub alert_manager: Arc<Mutex<crate::observability::alert_manager::AlertManager>>,
}

impl ObservabilityStack {
    /// Initialise all observability subsystems from a standalone config,
    /// without requiring an `AcpServer`.
    pub fn init_independent(config: &ObservabilityConfig) -> Result<Self, anyhow::Error> {
        let performance_monitor = crate::observability::performance::init_performance_monitoring();

        let runtime_config = crate::config::RuntimeConfig {
            otel_enabled: config.otel_enabled,
            otel_sample_ratio: config.sample_ratio,
            otel_service_name: config.service_name.clone(),
            otel_endpoint: config.otlp_endpoint.clone(),
            ..Default::default()
        };
        let telemetry_runtime = Arc::new(Mutex::new(
            crate::observability::telemetry::TelemetryRuntime::new(&runtime_config),
        ));

        let alert_manager = Arc::new(Mutex::new(
            crate::observability::alert_manager::AlertManager::new(
                crate::observability::alert_manager::default_alert_rules(),
            ),
        ));

        Ok(Self {
            performance_monitor,
            telemetry_runtime,
            alert_manager,
        })
    }
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
