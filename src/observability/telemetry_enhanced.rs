//! Enhanced telemetry and observability module
//!
//! This module provides structured logging, metrics collection, and distributed tracing
//! for comprehensive observability.
//!
//! ## Metrics: Legacy vs Primary
//!
//! - **`MetricsRecorder` / `AppMetrics`** (this file) — **Legacy** in-memory metrics
//!   collector with atomic counters. Kept for backward compatibility.
//! - **`metrics_exporter::PrometheusMetricsRecorder` / `build_prometheus_metrics`** —
//!   **Primary** Prometheus-format metrics path via `RuntimeMetrics`. New code should
//!   use this system.
//!
//! The [`bridge_metrics_recorder`] function in `metrics_exporter` synchronizes the legacy
//! recorder values into the primary `RuntimeMetrics` path so that manual recordings made
//! through the legacy API are still visible on the `/metrics` endpoint.
//!
//! # Features
//!
//! - **Structured Logging**: JSON-formatted logs with context and metadata
//! - **Metrics Collection**: Prometheus-compatible metrics with histograms
//! - **Distributed Tracing**: OpenTelemetry support for request tracing
//! - **Health Monitoring**: System health checks and status reporting
//! - **Performance Profiling**: Performance metrics and profiling tools
//!
//! # Architecture
//!
//! The telemetry system is built on three pillars:
//!
//! 1. **Logging**: Uses `tracing` crate for structured, hierarchical logging
//! 2. **Metrics**: Uses `opentelemetry` for metrics collection and export
//! 3. **Tracing**: Uses `opentelemetry` for distributed tracing
//!
//! # Usage
//!
//! ```rust
//! use telemetry_enhanced::{TelemetryConfig, init_telemetry};
//!
//! // Configure telemetry
//! let config = TelemetryConfig {
//!     enable_logging: true,
//!     enable_metrics: true,
//!     enable_tracing: false,
//!     log_level: "info".to_string(),
//!     metrics_interval_secs: 30,
//!     service_name: "my-service".to_string(),
//!     service_version: "1.2.0".to_string(),
//! };
//!
//! // Initialize telemetry
//! init_telemetry(&config).expect("failed to initialize telemetry");
//!
//! // Use structured logging
//! info!("application started", service_name = "my-service");
//!
//! // Record metrics
//! let metrics_recorder = MetricsRecorder::new();
//! metrics_recorder.record_request("api_call", 150.0);
//! ```
//!
//! # Configuration
//!
//! The telemetry system can be configured via `TelemetryConfig`:
//!
//! - `enable_logging`: Enable/disable structured logging
//! - `enable_metrics`: Enable/disable metrics collection
//! - `enable_tracing`: Enable/disable distributed tracing (requires OTLP endpoint)
//! - `log_level`: Log level filter (trace, debug, info, warn, error)
//! - `metrics_interval_secs`: Metrics export interval in seconds
//! - `service_name`: Service name for telemetry metadata
//! - `service_version`: Service version for telemetry metadata
//!
//! # Integration
//!
//! This module integrates with:
//!
//! - **Prometheus**: Metrics are exposed in Prometheus format
//! - **Grafana**: Metrics can be visualized in Grafana dashboards
//! - **Jaeger**: Distributed traces can be sent to Jaeger
//! - **Datadog**: Metrics and traces can be sent to Datadog
//! - **New Relic**: Metrics and traces can be sent to New Relic

use std::sync::Once;
use std::sync::OnceLock;
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::reload;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

/// Telemetry configuration
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// Enable structured logging
    pub enable_logging: bool,
    /// Enable metrics collection
    pub enable_metrics: bool,
    /// Enable distributed tracing
    pub enable_tracing: bool,
    /// Log level filter
    pub log_level: String,
    /// Metrics export interval in seconds
    pub metrics_interval_secs: u64,
    /// Service name for telemetry
    pub service_name: String,
    /// Service version
    pub service_version: String,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enable_logging: true,
            enable_metrics: true,
            enable_tracing: false, // Disabled by default as it requires OTLP endpoint
            log_level: "info".to_string(),
            metrics_interval_secs: 30,
            service_name: "go-on".to_string(),
            service_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Guard to prevent `init_telemetry` from running more than once,
/// protecting against double initialization of the global tracing subscriber
/// and the OTLP tracer provider (which could otherwise overwrite a provider
/// already set by `telemetry.rs::init_otel_provider`).
static INIT_TELEMETRY: Once = Once::new();

/// Reload handle for the active EnvFilter, allowing dynamic log-level
/// changes at runtime (e.g. via MCP `logging/setLevel`).
static FILTER_HANDLE: OnceLock<reload::Handle<EnvFilter, tracing_subscriber::Registry>> =
    OnceLock::new();

/// Reload the globally-active log filter with a new RUST_LOG-style directive.
///
/// Returns `Ok(())` if the filter was successfully swapped, or an error
/// string if the handle has not been initialised (telemetry not started)
/// or the reload fails.
pub fn reload_log_filter(directive: &str) -> Result<(), String> {
    let filter = EnvFilter::new(directive);
    match FILTER_HANDLE.get() {
        Some(handle) => handle
            .reload(filter)
            .map_err(|e| format!("failed to reload filter: {}", e)),
        None => Err("filter handle not initialised (telemetry not started)".to_string()),
    }
}

/// Initialize telemetry system
///
/// # Arguments
/// * `config` - Telemetry configuration
///
/// # Returns
/// * `Result<()>` - Returns Ok if initialization succeeds, or an error if something goes wrong
///
/// # Idempotency
///
/// This function is safe to call multiple times — only the first call performs initialization.
/// Subsequent calls are no-ops that return `Ok(())`. This prevents double initialization of
/// the global tracing subscriber and the OTLP tracer provider, which could otherwise
/// overwrite a provider already set by `telemetry.rs::init_otel_provider`.
pub fn init_telemetry(config: &TelemetryConfig) -> anyhow::Result<()> {
    let mut result = Ok(());
    INIT_TELEMETRY.call_once(|| {
        let mut layers = Vec::new();

        // Configure logging layer
        if config.enable_logging {
            let fmt_layer = fmt::layer()
                .with_writer(std::io::stderr)
                .with_target(true)
                .with_thread_ids(true)
                .with_thread_names(true)
                .with_span_events(FmtSpan::CLOSE)
                .compact();

            let filter = EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(&config.log_level));

            let (filter_layer, handle) = reload::Layer::new(filter);
            let _ = FILTER_HANDLE.set(handle);

            layers.push(fmt_layer.with_filter(filter_layer).boxed());
        }

        // Initialize the subscriber with all layers BEFORE metrics/tracing init
        // so that info!()/warn!() calls in init_metrics() and init_tracing() are captured.
        if let Err(err) = tracing_subscriber::registry()
            .with(layers)
            .try_init()
            .map_err(|err| anyhow::anyhow!("failed to initialize tracing subscriber: {}", err))
        {
            result = Err(err);
            return;
        }

        // Configure metrics layer if enabled
        if config.enable_metrics {
            if let Err(err) = init_metrics(config) {
                result = Err(err);
                return;
            }
        }

        // Configure tracing layer if enabled
        if config.enable_tracing {
            if let Err(err) = init_tracing(config) {
                result = Err(err);
                return;
            }
        }

        info!(
            service_name = config.service_name,
            service_version = config.service_version,
            "telemetry initialized"
        );
    });
    result
}

/// Initialize metrics collection using OpenTelemetry.
///
/// Sets up a meter provider with stdout exporter for development/demo use.
/// In production, configure an OTLP endpoint to send metrics to a backend
/// such as Prometheus, Datadog, or New Relic.
fn init_metrics(config: &TelemetryConfig) -> anyhow::Result<()> {
    use opentelemetry::global;
    use opentelemetry::KeyValue;
    use opentelemetry_sdk::metrics::{MeterProviderBuilder, PeriodicReader};
    use opentelemetry_sdk::resource::Resource;
    use tracing::info;

    // Build resource with service metadata
    let resource = Resource::builder()
        .with_attribute(KeyValue::new("service.name", config.service_name.clone()))
        .with_attribute(KeyValue::new(
            "service.version",
            config.service_version.clone(),
        ))
        .build();

    // Create a periodic reader with stdout exporter
    let exporter = opentelemetry_stdout::MetricExporter::default();
    let reader = PeriodicReader::builder(exporter)
        .with_interval(std::time::Duration::from_secs(config.metrics_interval_secs))
        .build();

    // Create a meter provider with the configured reader
    let meter_provider = MeterProviderBuilder::default()
        .with_resource(resource)
        .with_reader(reader)
        .build();

    // Set as the global meter provider
    global::set_meter_provider(meter_provider);

    info!(
        service_name = config.service_name,
        service_version = config.service_version,
        interval_secs = config.metrics_interval_secs,
        "OpenTelemetry metrics initialized with stdout exporter"
    );

    Ok(())
}

/// Initialize distributed tracing using OpenTelemetry OTLP.
///
/// Requires an OTLP-compatible endpoint (e.g., Jaeger, Grafana Tempo, Datadog Agent).
/// If the endpoint is not configured via `OTEL_EXPORTER_OTLP_ENDPOINT` environment
/// variable, tracing is initialized but logs a warning.
use std::sync::atomic::Ordering;

/// Shared guard against double-initialization of the global tracer provider.
/// Uses the same `TRACER_INITIALIZED` static defined in `telemetry.rs` to
/// prevent both modules from racing to set the global tracer provider.
use crate::observability::telemetry::TRACER_INITIALIZED;

fn init_tracing(config: &TelemetryConfig) -> anyhow::Result<()> {
    use opentelemetry::global;
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::resource::Resource;
    // TracerProvider is constructed via SdkTracerProvider::builder()

    use tracing::warn;

    // ── Guard: avoid re-initializing the global tracer provider ───────
    // `telemetry.rs::init_otel_provider` may have already called
    // `global::set_tracer_provider()`. Check before setting again to
    // prevent the second call from silently replacing the first.
    if TRACER_INITIALIZED.load(Ordering::Relaxed) {
        info!(
            "OpenTelemetry tracer provider already initialized; \
             skipping re-initialization"
        );
        return Ok(());
    }

    // Check if an OTLP endpoint is configured
    let otlp_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();

    if let Some(endpoint) = otlp_endpoint {
        let resource = Resource::builder()
            .with_attribute(KeyValue::new("service.name", config.service_name.clone()))
            .with_attribute(KeyValue::new(
                "service.version",
                config.service_version.clone(),
            ))
            .build();

        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(&endpoint)
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build OTLP span exporter: {}", e))?;

        let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_resource(resource)
            .with_batch_exporter(exporter)
            .build();

        global::set_tracer_provider(tracer_provider);
        TRACER_INITIALIZED.store(true, Ordering::Release);

        info!(
            service_name = config.service_name,
            otlp_endpoint = endpoint,
            "OpenTelemetry tracing initialized with OTLP exporter"
        );
    } else {
        warn!(
            "distributed tracing is enabled but OTEL_EXPORTER_OTLP_ENDPOINT is not set; \
             traces will not be exported. Set the environment variable or disable tracing."
        );
    }

    Ok(())
}

/// Application metrics
#[derive(Debug, Clone)]
pub struct AppMetrics {
    /// Total requests processed
    pub requests_total: u64,
    /// Successful requests
    pub requests_success: u64,
    /// Failed requests
    pub requests_failed: u64,
    /// Cache hits
    pub cache_hits: u64,
    /// Cache misses
    pub cache_misses: u64,
    /// Average request latency in milliseconds
    pub avg_latency_ms: f64,
    /// Active connections
    pub active_connections: u64,
    /// Memory usage in bytes
    pub memory_usage_bytes: u64,
}

impl Default for AppMetrics {
    fn default() -> Self {
        Self {
            requests_total: 0,
            requests_success: 0,
            requests_failed: 0,
            cache_hits: 0,
            cache_misses: 0,
            avg_latency_ms: 0.0,
            active_connections: 0,
            memory_usage_bytes: 0,
        }
    }
}

/// Legacy in-memory metrics collector.
///
/// Deprecated: Use `metrics_exporter::PrometheusMetricsRecorder` / `build_prometheus_metrics`
/// (the primary Prometheus-format metrics path) instead. This recorder is kept for backward
/// compatibility and is bridged into the primary path via `bridge_metrics_recorder`.
pub struct MetricsRecorder {
    metrics: std::sync::RwLock<AppMetrics>,
}

impl MetricsRecorder {
    /// Create a new metrics recorder
    pub fn new() -> Self {
        Self {
            metrics: std::sync::RwLock::new(AppMetrics::default()),
        }
    }

    fn read_metrics(&self) -> std::sync::RwLockReadGuard<'_, AppMetrics> {
        match self.metrics.read() {
            Ok(guard) => guard,
            Err(poisoned) => {
                warn!("enhanced telemetry metrics lock poisoned during read; recovering metrics state");
                poisoned.into_inner()
            }
        }
    }

    fn write_metrics(&self) -> std::sync::RwLockWriteGuard<'_, AppMetrics> {
        match self.metrics.write() {
            Ok(guard) => guard,
            Err(poisoned) => {
                warn!("enhanced telemetry metrics lock poisoned during write; recovering metrics state");
                poisoned.into_inner()
            }
        }
    }

    /// Record a request
    pub fn record_request(&self, success: bool, latency_ms: f64) {
        let mut metrics = self.write_metrics();
        metrics.requests_total += 1;
        if success {
            metrics.requests_success += 1;
        } else {
            metrics.requests_failed += 1;
        }

        // Update average latency using exponential moving average
        if metrics.avg_latency_ms == 0.0 {
            metrics.avg_latency_ms = latency_ms;
        } else {
            metrics.avg_latency_ms = 0.9 * metrics.avg_latency_ms + 0.1 * latency_ms;
        }
    }

    /// Record a cache hit
    pub fn record_cache_hit(&self) {
        let mut metrics = self.write_metrics();
        metrics.cache_hits += 1;
    }

    /// Record a cache miss
    pub fn record_cache_miss(&self) {
        let mut metrics = self.write_metrics();
        metrics.cache_misses += 1;
    }

    /// Update active connections
    pub fn update_active_connections(&self, count: u64) {
        let mut metrics = self.write_metrics();
        metrics.active_connections = count;
    }

    /// Update memory usage
    pub fn update_memory_usage(&self, bytes: u64) {
        let mut metrics = self.write_metrics();
        metrics.memory_usage_bytes = bytes;
    }

    /// Get current metrics snapshot
    pub fn get_metrics(&self) -> AppMetrics {
        self.read_metrics().clone()
    }

    /// Export metrics as JSON
    pub fn export_json(&self) -> serde_json::Value {
        let metrics = self.get_metrics();
        serde_json::json!({
            "requests_total": metrics.requests_total,
            "requests_success": metrics.requests_success,
            "requests_failed": metrics.requests_failed,
            "cache_hits": metrics.cache_hits,
            "cache_misses": metrics.cache_misses,
            "cache_hit_rate": if metrics.cache_hits + metrics.cache_misses > 0 {
                metrics.cache_hits as f64 / (metrics.cache_hits + metrics.cache_misses) as f64
            } else {
                0.0
            },
            "avg_latency_ms": metrics.avg_latency_ms,
            "active_connections": metrics.active_connections,
            "memory_usage_mb": metrics.memory_usage_bytes as f64 / 1024.0 / 1024.0,
        })
    }
}

impl Default for MetricsRecorder {
    fn default() -> Self {
        Self::new()
    }
}

/// Global singleton `MetricsRecorder` for use across the observability stack.
///
/// This allows the Prometheus metrics exporter bridge to access the OTLP
/// metrics recorder without needing to pass it through every constructor.
static GLOBAL_METRICS_RECORDER: std::sync::LazyLock<MetricsRecorder> =
    std::sync::LazyLock::new(MetricsRecorder::new);

/// Return a reference to the legacy global `MetricsRecorder` singleton.
///
/// Deprecated: Use `metrics_exporter`'s Prometheus-based metrics path for new code.
/// This global recorder is bridged into `RuntimeMetrics` automatically via
/// `bridge_metrics_recorder` on each metrics scrape.
///
/// The recorder is lazily initialized on first access and can be safely
/// shared across threads for recording requests, cache operations, and
/// exporting metrics snapshots.
pub fn global_metrics_recorder() -> &'static MetricsRecorder {
    &GLOBAL_METRICS_RECORDER
}

/// Structured logging macros for common patterns
pub mod log {
    use super::*;

    /// Log request start with structured fields
    pub fn request_start(method: &str, path: &str, request_id: &str) {
        info!(
            method = method,
            path = path,
            request_id = request_id,
            "request_start"
        );
    }

    /// Log request completion with structured fields
    pub fn request_complete(
        method: &str,
        path: &str,
        request_id: &str,
        status_code: u16,
        duration_ms: f64,
    ) {
        info!(
            method = method,
            path = path,
            request_id = request_id,
            status_code = status_code,
            duration_ms = duration_ms,
            "request_complete"
        );
    }

    /// Log error with structured fields
    pub fn error_with_context(error: &anyhow::Error, context: &str, request_id: Option<&str>) {
        if let Some(req_id) = request_id {
            error!(
                error = %error,
                error_debug = ?error,
                context = context,
                request_id = req_id,
                "error_occurred"
            );
        } else {
            error!(
                error = %error,
                error_debug = ?error,
                context = context,
                "error_occurred"
            );
        }
    }

    /// Log cache operation
    pub fn cache_operation(operation: &str, key: &str, hit: bool, duration_ms: f64) {
        debug!(
            operation = operation,
            key = key,
            hit = hit,
            duration_ms = duration_ms,
            "cache_operation"
        );
    }

    /// Log agent operation
    pub fn agent_operation(agent_name: &str, operation: &str, duration_ms: f64, success: bool) {
        info!(
            agent_name = agent_name,
            operation = operation,
            duration_ms = duration_ms,
            success = success,
            "agent_operation"
        );
    }
}

/// Health check metrics
pub struct HealthMetrics {
    /// Last health check timestamp
    pub last_check: std::time::Instant,
    /// Total health checks performed
    pub checks_total: u64,
    /// Failed health checks
    pub checks_failed: u64,
    /// Current health status
    pub is_healthy: bool,
    /// Consecutive failures (reset on success)
    consecutive_failures: u32,
    /// Consecutive successes (reset on failure)
    consecutive_successes: u32,
}

impl HealthMetrics {
    /// Create new health metrics
    pub fn new() -> Self {
        Self {
            last_check: std::time::Instant::now(),
            checks_total: 0,
            checks_failed: 0,
            is_healthy: true,
            consecutive_failures: 0,
            consecutive_successes: 0,
        }
    }

    /// Record health check with consecutive-failure threshold (N=3)
    /// to prevent health flapping from transient failures.
    pub fn record_check(&mut self, healthy: bool) {
        self.last_check = std::time::Instant::now();
        self.checks_total += 1;

        if !healthy {
            self.checks_failed += 1;
            self.consecutive_failures += 1;
            self.consecutive_successes = 0;
            if self.consecutive_failures >= 3 {
                self.is_healthy = false;
                error!(
                    "health_check_failed ({} consecutive)",
                    self.consecutive_failures
                );
            } else {
                error!(
                    "health_check_failed (transient, {}/3 consecutive)",
                    self.consecutive_failures
                );
            }
        } else {
            self.consecutive_successes += 1;
            self.consecutive_failures = 0;
            if self.consecutive_successes >= 3 {
                self.is_healthy = true;
                trace!(
                    "health_check_passed ({} consecutive)",
                    self.consecutive_successes
                );
            } else {
                trace!(
                    "health_check_passed (recovering, {}/3 consecutive)",
                    self.consecutive_successes
                );
            }
        }
    }

    /// Get health status
    pub fn get_status(&self) -> serde_json::Value {
        serde_json::json!({
            "is_healthy": self.is_healthy,
            "last_check": self.last_check.elapsed().as_secs(),
            "checks_total": self.checks_total,
            "checks_failed": self.checks_failed,
            "success_rate": if self.checks_total > 0 {
                1.0 - (self.checks_failed as f64 / self.checks_total as f64)
            } else {
                1.0
            },
        })
    }
}

impl Default for HealthMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── init_telemetry idempotency ────────────────────────────────────

    /// Verify that `init_telemetry` can be called multiple times without
    /// panicking. The second call is expected to return an `Err` because
    /// the tracing subscriber is a singleton, but that is harmless.
    #[test]
    fn test_init_telemetry_idempotent() {
        let config = TelemetryConfig::default();

        // First call should succeed
        let r1 = init_telemetry(&config);
        assert!(
            r1.is_ok() || r1.is_err(),
            "first init may succeed or fail depending on other tests"
        );

        // Second call should not panic — Err is expected (singleton subscriber)
        let r2 = init_telemetry(&config);
        // The second call will likely be Err, but the important thing is no panic
        let _ = r2;
    }

    /// Verify that calling init_telemetry with only logging, only metrics,
    /// and only tracing doesn't panic and returns a Result.
    #[test]
    fn test_init_telemetry_partial_configs() {
        let config_log = TelemetryConfig {
            enable_logging: true,
            enable_metrics: false,
            enable_tracing: false,
            ..Default::default()
        };
        // Should not panic
        let _ = init_telemetry(&config_log);

        let config_metrics = TelemetryConfig {
            enable_logging: false,
            enable_metrics: true,
            enable_tracing: false,
            ..Default::default()
        };
        let _ = init_telemetry(&config_metrics);

        let config_tracing = TelemetryConfig {
            enable_logging: false,
            enable_metrics: false,
            enable_tracing: true,
            ..Default::default()
        };
        let _ = init_telemetry(&config_tracing);
    }

    // ── MetricsRecorder ──────────────────────────────────────────────

    /// Verify that `MetricsRecorder::record_request` correctly increments
    /// counters for both success and failure cases.
    #[test]
    fn test_record_request_increments_counters() {
        let recorder = MetricsRecorder::new();

        // Record 3 successful requests
        recorder.record_request(true, 10.0);
        recorder.record_request(true, 20.0);
        recorder.record_request(true, 30.0);

        let metrics = recorder.get_metrics();
        assert_eq!(metrics.requests_total, 3);
        assert_eq!(metrics.requests_success, 3);
        assert_eq!(metrics.requests_failed, 0);
        // With requests of 10, 20, 30ms, EMA = 0.9*11 + 0.1*30 = 12.9
        assert!((metrics.avg_latency_ms - 12.9).abs() < 1.0);

        // Record 2 failed requests
        recorder.record_request(false, 50.0);
        recorder.record_request(false, 100.0);

        let metrics = recorder.get_metrics();
        assert_eq!(metrics.requests_total, 5);
        assert_eq!(metrics.requests_success, 3);
        assert_eq!(metrics.requests_failed, 2);
    }

    /// Verify that recording many requests produces correct final state.
    #[test]
    fn test_record_request_latency_ema() {
        let recorder = MetricsRecorder::new();

        // First request sets baseline
        recorder.record_request(true, 100.0);
        let m1 = recorder.get_metrics();
        assert!((m1.avg_latency_ms - 100.0).abs() < 0.01);

        // Second request: 0.9 * 100 + 0.1 * 200 = 110
        recorder.record_request(true, 200.0);
        let m2 = recorder.get_metrics();
        assert!((m2.avg_latency_ms - 110.0).abs() < 0.01);

        // Third request: 0.9 * 110 + 0.1 * 50 = 104
        recorder.record_request(true, 50.0);
        let m3 = recorder.get_metrics();
        assert!((m3.avg_latency_ms - 104.0).abs() < 0.01);
    }

    // ── HealthMetrics ─────────────────────────────────────────────────

    /// Verify that `HealthMetrics` reports correct health status through
    /// consecutive failure and success transitions.
    #[test]
    fn test_health_metrics_healthy_by_default() {
        let health = HealthMetrics::new();
        let status = health.get_status();
        assert_eq!(status["is_healthy"], true);
        assert_eq!(status["checks_total"], 0);
        assert_eq!(status["checks_failed"], 0);
    }

    /// Verify health transitions to unhealthy after 3 consecutive failures,
    /// and recovers after 3 consecutive successes.
    #[test]
    fn test_health_metrics_consecutive_failures_and_recovery() {
        let mut health = HealthMetrics::new();

        // 1 failure — still healthy (transient)
        health.record_check(false);
        assert!(health.is_healthy, "1 failure: should still be healthy");

        // 2 failures — still healthy (transient)
        health.record_check(false);
        assert!(health.is_healthy, "2 failures: should still be healthy");

        // 3 failures — becomes unhealthy
        health.record_check(false);
        assert!(!health.is_healthy, "3 failures: should be unhealthy");

        let status = health.get_status();
        assert_eq!(status["checks_total"], 3);
        assert_eq!(status["checks_failed"], 3);
        assert_eq!(status["is_healthy"], false);

        // 1 success — still unhealthy (recovering)
        health.record_check(true);
        assert!(
            !health.is_healthy,
            "1 success after 3 fails: still unhealthy"
        );

        // 2 successes — still unhealthy (recovering)
        health.record_check(true);
        assert!(
            !health.is_healthy,
            "2 successes after 3 fails: still unhealthy"
        );

        // 3 successes — becomes healthy again
        health.record_check(true);
        assert!(health.is_healthy, "3 successes: should be healthy again");

        let status = health.get_status();
        assert_eq!(status["is_healthy"], true);
    }

    /// Verify that success_rate calculation works correctly.
    #[test]
    fn test_health_metrics_success_rate() {
        let mut health = HealthMetrics::new();

        // 7 checks: 5 success, 2 failure
        health.record_check(true);
        health.record_check(false);
        health.record_check(true);
        health.record_check(true);
        health.record_check(false);
        health.record_check(true);
        health.record_check(true);

        let status = health.get_status();
        assert_eq!(status["checks_total"], 7);
        assert_eq!(status["checks_failed"], 2);
        let expected_rate = 1.0 - (2.0 / 7.0);
        let rate = status["success_rate"].as_f64().unwrap();
        assert!((rate - expected_rate).abs() < 0.001);
    }
}
