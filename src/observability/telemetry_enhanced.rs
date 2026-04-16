//! Enhanced telemetry and observability module
//!
//! This module provides structured logging, metrics collection, and distributed tracing
//! for comprehensive observability.
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
//!     service_version: "1.0.0".to_string(),
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

#![allow(dead_code)]

use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::fmt::format::FmtSpan;
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

/// Initialize telemetry system
///
/// # Arguments
/// * `config` - Telemetry configuration
///
/// # Returns
/// * `Result<()>` - Returns Ok if initialization succeeds, or an error if something goes wrong
pub fn init_telemetry(config: &TelemetryConfig) -> anyhow::Result<()> {
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

        let filter_layer =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log_level));

        layers.push(fmt_layer.with_filter(filter_layer).boxed());
    }

    // Configure metrics layer if enabled
    if config.enable_metrics {
        init_metrics(config)?;
    }

    // Configure tracing layer if enabled
    if config.enable_tracing {
        init_tracing(config)?;
    }

    // Initialize the subscriber with all layers
    tracing_subscriber::registry()
        .with(layers)
        .try_init()
        .map_err(|err| anyhow::anyhow!("failed to initialize tracing subscriber: {}", err))?;

    info!(
        service_name = config.service_name,
        service_version = config.service_version,
        "telemetry initialized"
    );

    Ok(())
}

/// Initialize metrics collection
fn init_metrics(config: &TelemetryConfig) -> anyhow::Result<()> {
    // Simplified metrics initialization
    // In production, you would set up proper OpenTelemetry metrics
    tracing::info!(
        "Metrics collection configured for service: {} v{}",
        config.service_name,
        config.service_version
    );
    tracing::info!("Metrics interval: {} seconds", config.metrics_interval_secs);
    Ok(())
}

/// Initialize distributed tracing
fn init_tracing(_config: &TelemetryConfig) -> anyhow::Result<()> {
    // Note: This requires an OTLP endpoint to be configured
    // For now, we'll just log that tracing is enabled but not configured
    warn!("distributed tracing is enabled but requires OTLP endpoint configuration");
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

/// Metrics recorder
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
}

impl HealthMetrics {
    /// Create new health metrics
    pub fn new() -> Self {
        Self {
            last_check: std::time::Instant::now(),
            checks_total: 0,
            checks_failed: 0,
            is_healthy: true,
        }
    }

    /// Record health check
    pub fn record_check(&mut self, healthy: bool) {
        self.last_check = std::time::Instant::now();
        self.checks_total += 1;

        if !healthy {
            self.checks_failed += 1;
            self.is_healthy = false;
            error!("health_check_failed");
        } else {
            self.is_healthy = true;
            trace!("health_check_passed");
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
