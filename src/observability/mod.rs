//! Observability modules for monitoring, telemetry, and performance tracking.
//!
//! This module contains components responsible for system observability,
//! including:
//!
//! - **Observability**: Core observability infrastructure and utilities
//! - **Performance**: Performance monitoring and optimization tracking
//! - **Telemetry**: Basic telemetry collection and reporting
//! - **Telemetry Enhanced**: Advanced telemetry with additional metrics and insights
//! - **Alert Manager**: Threshold-based alerting with webhook dispatch
//! - **Provenance**: Data provenance tracking
//! - **Metrics Exporter**: System metrics export
//! - **Live Performance**: Real-time performance views
//! - **Memory Health**: Memory health monitoring
//!
//! # Decoupling from ACP server
//!
//! The `ObservabilityStack` and `ObservabilityConfig` types below allow
//! constructing the observability layer **without** requiring an `AcpServer`.
//! This enables independent testing, embedding in non-ACP runtimes, or
//! running observability as a standalone sidecar.
//!
//! ## Extracting as an independent concern
//!
//! ```rust,ignore
//! use go_on::observability::{ObservabilityConfig, ObservabilityStack};
//!
//! let config = ObservabilityConfig {
//!     service_name: "my-app".into(),
//!     otel_enabled: true,
//!     ..Default::default()
//! };
//! let stack = ObservabilityStack::init_independent(&config)?;
//! // Use stack.alert_manager, stack.telemetry, etc.
//! # Ok::<_, anyhow::Error>(())
//! ```
//!
//! The `ObservabilityLayer` struct in `crate::acp::server` still bundles
//! ACP-specific types (`RuntimeMetrics`, `AcpLockMonitor`). The stack here
//! is the pure-observability subset that can be migrated out of the ACP
//! crate entirely.

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

use std::sync::Arc;
use std::sync::Mutex;

// Re-exports are not needed; consumers use full crate paths.
// See main.rs for usage: crate::observability::memory_health::*

/// Configuration for initialising the observability stack independently
/// of the ACP server.
///
/// This is the minimal config needed to bootstrap all observability
/// subsystems. Additional ACP-specific fields (e.g. `RuntimeMetrics`)
/// are wired separately in `crate::acp::server::ObservabilityLayer`.
#[derive(Debug, Clone)]
#[allow(dead_code)]
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

/// An independently constructed observability stack, decoupled from
/// `AcpServer`.
///
/// Holds the subsystems that are purely about observability, without
/// any ACP-specific runtime metrics or lock monitoring. If you need
/// the full ACP observability layer, construct `ObservabilityLayer`
/// from `crate::acp::server` instead.
#[allow(dead_code)]
pub struct ObservabilityStack {
    /// Performance monitoring (latency, throughput, error rates).
    pub performance_monitor: Arc<Mutex<crate::observability::performance::PerformanceMonitor>>,
    /// Telemetry runtime for distributed tracing.
    pub telemetry_runtime: Arc<Mutex<crate::observability::telemetry::TelemetryRuntime>>,
    /// Alert manager for threshold-based alerting.
    pub alert_manager: Arc<Mutex<crate::observability::alert_manager::AlertManager>>,
}

impl ObservabilityStack {
    /// Initialise all observability subsystems from a standalone config,
    /// without requiring an `AcpServer`.
    ///
    /// This is the primary constructor for use cases that want observability
    /// but do not need the full ACP server (sidecars, headless agents,
    /// test harnesses, etc.).
    #[allow(dead_code)]
    pub fn init_independent(config: &ObservabilityConfig) -> Result<Self, anyhow::Error> {
        let performance_monitor = crate::observability::performance::init_performance_monitoring();

        // Build a minimal RuntimeConfig for telemetry init.
        // `otel_sample_ratio` is the field name used by RuntimeConfig;
        // our ObservabilityConfig uses `sample_ratio` for consistency.
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
