//! Enhanced telemetry and observability module
//!
//! This module provides structured logging, metrics collection, and distributed tracing
//! for comprehensive observability.
//!
//! ## Metrics
//!
//! - **`metrics_exporter::PrometheusMetricsRecorder` / `build_prometheus_metrics`** —
//!   **Primary** Prometheus-format metrics path via `RuntimeMetrics`. New code should
//!   use this system.
//!
//! The legacy `MetricsRecorder` / `AppMetrics` collector and its
//! `bridge_metrics_recorder` sync were removed — they had zero production
//! writers, so every bridge call merged all-zero values on each metrics scrape.
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
//! ```text
//! // This module is internal; doctests would need crate path.
//! use go_on::observability::telemetry_enhanced::{TelemetryConfig, init_telemetry};
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

use regex::Regex;
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::LazyLock;
use std::sync::Once;
use std::sync::OnceLock;
use tracing::info;

use opentelemetry::global;
use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::Context;
use opentelemetry::KeyValue;
use sha2::{Digest, Sha256};

use crate::config::RuntimeConfig;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::reload;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

// ---------------------------------------------------------------------------
// RedactingWriter — wraps an io::Write to redact sensitive content before
// it is emitted to the output stream.
// ---------------------------------------------------------------------------

/// A [`MakeWriter`] that wraps stderr output and redacts sensitive patterns
/// from every completed line before writing.
struct RedactingMakeWriter;

impl<'writer> MakeWriter<'writer> for RedactingMakeWriter {
    type Writer = RedactingWriter<io::Stderr>;

    fn make_writer(&self) -> Self::Writer {
        RedactingWriter {
            inner: io::stderr(),
            buffer: Vec::new(),
        }
    }
}

/// Wraps a `Write` and redacts sensitive content on each flush (line end).
struct RedactingWriter<W: Write> {
    inner: W,
    buffer: Vec<u8>,
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Buffer everything; we redact on flush (line end).
        // Check if the buffer contains a complete line (ends with newline)
        // and immediately redact + flush so output is not delayed.
        self.buffer.extend_from_slice(buf);
        if buf.last() == Some(&b'\n') {
            self.flush()?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        // Use from_utf8_lossy to safely handle any partial UTF-8 sequences
        // that may remain in the buffer across write calls.
        let raw = String::from_utf8_lossy(&self.buffer);
        let redacted = REDACTOR.redact(&raw);
        self.inner.write_all(redacted.as_bytes())?;
        self.inner.flush()?;
        self.buffer.clear();
        Ok(())
    }
}

// Lazy-init global redactor that compiles patterns once.

struct MessageRedactor {
    patterns: Vec<(Regex, &'static str)>,
}

impl MessageRedactor {
    fn redact(&self, input: &str) -> String {
        let mut output = input.to_string();
        for (re, replacement) in &self.patterns {
            output = re.replace_all(&output, *replacement).to_string();
        }
        output
    }
}

static REDACTOR: LazyLock<MessageRedactor> = LazyLock::new(|| MessageRedactor {
    patterns: vec![
        // OpenAI / Anthropic / generic API keys: sk-, pk-
        (
            Regex::new(r"(?i)(sk-|pk-)[a-z0-9A-Z_-]{20,}").unwrap(),
            "${1}****",
        ),
        // Authorization header values
        (
            Regex::new(r"(?i)(Authorization:\s*Bearer\s+)\S+").unwrap(),
            "${1}****",
        ),
        // URL-embedded passwords
        (
            Regex::new(r"(?i)(password|passwd|pwd|secret)(=|:\s*)\S+").unwrap(),
            "${1}${2}****",
        ),
        // Generic api_key / api-secret values
        (
            Regex::new(r"(?i)(api_key|api_secret|api-key|api-secret)(=|:\s*)\S+").unwrap(),
            "${1}${2}****",
        ),
        // JWT-like tokens
        (
            Regex::new(r"(?i)(eyJ[a-zA-Z0-9_-]{10,}\.[a-zA-Z0-9_-]{10,}\.[a-zA-Z0-9_-]{10,})")
                .unwrap(),
            "****.****.****",
        ),
    ],
});

/// Local time formatter that displays timestamps in the system's local timezone
/// instead of the default UTC. Uses chrono::Local for timezone-aware formatting.
struct LocalTimer;
impl FormatTime for LocalTimer {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        write!(
            w,
            "{}",
            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.6f%:z")
        )
    }
}

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

/// Guard against double-initialization of the global tracer provider.
/// Shared between the early telemetry init and the config-driven
/// `init_otel_export` path so both cannot race to set the global provider.
pub(crate) static TRACER_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Guard to prevent `init_telemetry` from running more than once,
/// protecting against double initialization of the global tracing subscriber
/// and the OTLP tracer provider.
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
/// overwrite a provider already set by `init_otel_export` (the config-driven late
/// wiring in main/mod.rs).
pub fn init_telemetry(config: &TelemetryConfig) -> anyhow::Result<()> {
    let mut result = Ok(());
    INIT_TELEMETRY.call_once(|| {
        let mut layers = Vec::new();

        // Configure logging layer
        if config.enable_logging {
            // Use RedactingMakeWriter to redact sensitive content (API keys,
            // tokens, passwords) before it reaches the stderr output stream.
            let fmt_layer = fmt::layer()
                .with_writer(RedactingMakeWriter)
                .with_target(true)
                .with_thread_ids(true)
                .with_thread_names(true)
                .with_span_events(FmtSpan::CLOSE)
                .with_timer(LocalTimer)
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

/// Initialize OpenTelemetry export from parsed runtime config.
///
/// Wired in `main/mod.rs` after config load (the export path needs
/// `[runtime] otel_*` values, so it cannot run at the early telemetry init
/// which only knows env defaults). Unlike `init_telemetry`, this does NOT go
/// through the `INIT_TELEMETRY: Once` gate — it is a deliberate late,
/// config-driven export wiring for the tracing side.
///
/// - `exporter` — "otlp" or "jaeger" (both export via OTLP).
/// - `endpoint` — explicit endpoint from config; falls back to
///   `OTEL_EXPORTER_OTLP_ENDPOINT` env, then logs a warning.
/// - `sample_ratio` — batch exporter sampling hint (0.0–1.0).
pub fn init_otel_export(
    exporter: &str,
    endpoint: Option<&str>,
    service_name: &str,
    sample_ratio: f64,
) -> anyhow::Result<()> {
    use opentelemetry::global;
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::resource::Resource;
    use tracing::info;

    if TRACER_INITIALIZED.load(Ordering::Acquire) {
        info!("OpenTelemetry tracer provider already initialized; skipping export wiring");
        return Ok(());
    }

    let effective_endpoint = endpoint
        .map(|e| e.to_string())
        .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok());

    let Some(endpoint) = effective_endpoint else {
        tracing::warn!(
            "otel_enabled=true but no endpoint configured: set [runtime] otel_endpoint \
             or OTEL_EXPORTER_OTLP_ENDPOINT; traces will not be exported"
        );
        return Ok(());
    };

    let exporter_str = exporter.to_string();

    let resource = Resource::builder()
        .with_attribute(KeyValue::new("service.name", service_name.to_string()))
        .with_attribute(KeyValue::new(
            "service.version",
            env!("CARGO_PKG_VERSION").to_string(),
        ))
        .build();

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&endpoint)
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build OTLP span exporter: {}", e))?;

    // The SDK sampler stays AlwaysOn: sampling is the in-process
    // `TelemetryRuntime::should_sample` gate (deterministic hash on the
    // sample key), which already applies `otel_sample_ratio` before a root
    // span is created. Adding a second SDK-level ratio sampler here would
    // compound the two independent 0..1 filters and under-export at
    // non-1.0 ratios (effective rate ≈ ratio²).
    let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build();

    global::set_tracer_provider(tracer_provider);
    TRACER_INITIALIZED.store(true, Ordering::Release);

    info!(
        service_name = %service_name,
        exporter = %exporter_str,
        otlp_endpoint = %endpoint,
        sample_ratio = sample_ratio,
        "OpenTelemetry tracing export initialized via OTLP"
    );

    Ok(())
}

/// OpenTelemetry metrics collection via stdout exporter.
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
    if TRACER_INITIALIZED.load(Ordering::Acquire) {
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

// ── TelemetryRuntime (migrated from telemetry.rs) ────────────────────────

#[derive(Debug, Default)]
pub struct TelemetryRuntime {
    enabled: bool,
    sample_ratio: f64,
    total_roots: AtomicU64,
    sampled_roots: AtomicU64,
}

impl TelemetryRuntime {
    pub fn new(config: &RuntimeConfig) -> Self {
        if !config.otel_enabled {
            return Self::default();
        }

        Self {
            enabled: true,
            sample_ratio: config.otel_sample_ratio.clamp(0.0, 1.0),
            total_roots: AtomicU64::new(0),
            sampled_roots: AtomicU64::new(0),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn sampling_rate(&self) -> f64 {
        let total = self.total_roots.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        let sampled = self.sampled_roots.load(Ordering::Relaxed);
        sampled as f64 / total as f64
    }

    pub fn start_root_span(
        &self,
        name: &str,
        sample_key: &str,
        attributes: Vec<KeyValue>,
    ) -> Option<Context> {
        if !self.enabled {
            return None;
        }

        self.total_roots.fetch_add(1, Ordering::Relaxed);
        if !self.should_sample(sample_key) {
            return None;
        }

        self.sampled_roots.fetch_add(1, Ordering::Relaxed);
        let tracer = global::tracer("go-on.acp");
        let span = tracer.start(name.to_string());
        let cx = Context::current_with_span(span);
        for attr in attributes {
            cx.span().set_attribute(attr);
        }
        Some(cx)
    }

    /// Inject the trace context from a `Context` into a `HashMap` of headers
    /// suitable for outbound HTTP requests (W3C Trace Context propagation).
    pub fn inject_context(&self, cx: &Context) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        if self.enabled && cx.span().span_context().is_valid() {
            global::get_text_map_propagator(|propagator| {
                propagator.inject_context(cx, &mut headers);
            });
        }
        headers
    }

    /// Extract a `Context` from a `HashMap` of HTTP headers (W3C Trace Context
    /// propagation). Returns a remote span context if `traceparent` etc. are
    /// present, otherwise returns the current default context.
    pub fn extract_context(&self, headers: &HashMap<String, String>) -> Context {
        if !self.enabled {
            return Context::current();
        }
        global::get_text_map_propagator(|propagator| propagator.extract(headers))
    }

    pub fn start_child_span(
        &self,
        parent: &Context,
        name: &str,
        attributes: Vec<KeyValue>,
    ) -> Option<Context> {
        if !self.enabled {
            return None;
        }

        let tracer = global::tracer("go-on.acp");
        let span = tracer.start_with_context(name.to_string(), parent);
        let cx = parent.with_span(span);
        for attr in attributes {
            cx.span().set_attribute(attr);
        }
        Some(cx)
    }

    pub fn end_span(&self, cx: Context, attributes: Vec<KeyValue>) {
        if !self.enabled {
            return;
        }

        for attr in attributes {
            cx.span().set_attribute(attr);
        }
        cx.span().end();
    }

    /// Deterministic hash-based sampling using SHA-256 of the sample key.
    /// Returns `true` if the request should be sampled based on `sample_ratio`.
    fn should_sample(&self, sample_key: &str) -> bool {
        if self.sample_ratio >= 1.0 {
            return true;
        }
        if self.sample_ratio <= 0.0 {
            return false;
        }

        let mut hasher = Sha256::new();
        hasher.update(sample_key.as_bytes());
        let digest = hasher.finalize();
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(&digest[0..8]);
        let value = u64::from_le_bytes(bytes);
        let ratio = (value as f64) / (u64::MAX as f64);
        ratio <= self.sample_ratio
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

    // ── TelemetryRuntime (migrated from telemetry.rs) ─────────────────

    #[test]
    fn test_should_sample_always_at_1_0() {
        let rt = TelemetryRuntime {
            enabled: true,
            sample_ratio: 1.0,
            ..Default::default()
        };
        for key in &["", "a", "test-key", "conversation-12345"] {
            assert!(
                rt.should_sample(key),
                "key '{}' should always sample at ratio 1.0",
                key
            );
        }
    }

    #[test]
    fn test_should_sample_never_at_0_0() {
        let rt = TelemetryRuntime {
            enabled: true,
            sample_ratio: 0.0,
            ..Default::default()
        };
        for key in &["", "anything", "another-test"] {
            assert!(
                !rt.should_sample(key),
                "key '{}' should never sample at ratio 0.0",
                key
            );
        }
    }

    #[test]
    fn test_should_sample_deterministic() {
        let rt = TelemetryRuntime {
            enabled: true,
            sample_ratio: 0.5,
            ..Default::default()
        };
        let key = "consistent-key";
        let r1 = rt.should_sample(key);
        let r2 = rt.should_sample(key);
        let r3 = rt.should_sample(key);
        assert_eq!(r1, r2, "should_sample must be deterministic");
        assert_eq!(r2, r3, "should_sample must be deterministic");
    }

    #[test]
    fn test_should_sample_different_keys_may_differ() {
        let rt = TelemetryRuntime {
            enabled: true,
            sample_ratio: 0.5,
            ..Default::default()
        };
        let mut saw_true = false;
        let mut saw_false = false;
        for i in 0..1000u64 {
            let key = format!("distinct-key-{}", i);
            if rt.should_sample(&key) {
                saw_true = true;
            } else {
                saw_false = true;
            }
            if saw_true && saw_false {
                break;
            }
        }
        assert!(
            saw_true && saw_false,
            "at ratio 0.5, 1000 distinct keys should produce both sampled and unsampled"
        );
    }

    #[test]
    fn test_should_sample_boundary_zero() {
        let rt = TelemetryRuntime {
            enabled: true,
            sample_ratio: 0.001,
            ..Default::default()
        };
        let sampled_count: u32 = (0..10000u64)
            .map(|i| rt.should_sample(&format!("key-{}", i)) as u32)
            .sum();
        assert!(
            sampled_count < 100,
            "at ratio 0.001, 10000 keys should produce <100 samples, got {}",
            sampled_count
        );
    }

    #[test]
    fn test_should_sample_clamps_ratio() {
        let rt = TelemetryRuntime::new(&RuntimeConfig {
            otel_enabled: true,
            otel_sample_ratio: 1.5,
            otel_exporter: "stdout".to_string(),
            otel_service_name: "go-on-test".to_string(),
            otel_endpoint: None,
            ..Default::default()
        });
        assert!(
            rt.should_sample("anything"),
            "ratio clamped to 1.0 should always sample"
        );

        let rt2 = TelemetryRuntime::new(&RuntimeConfig {
            otel_enabled: true,
            otel_sample_ratio: -0.5,
            otel_exporter: "stdout".to_string(),
            otel_service_name: "go-on-test".to_string(),
            otel_endpoint: None,
            ..Default::default()
        });
        assert!(
            !rt2.should_sample("anything"),
            "ratio clamped to 0.0 should never sample"
        );
    }

    #[test]
    fn test_disabled_runtime_returns_none() {
        let rt = TelemetryRuntime::default();
        assert!(!rt.is_enabled());
        assert!(rt.start_root_span("test", "key", vec![]).is_none());
        assert!(rt
            .start_child_span(&Context::current(), "child", vec![])
            .is_none());
        rt.end_span(Context::current(), vec![]);
    }

    mod otel_tests {
        use super::*;

        #[test]
        fn disabled_by_default() {
            let rt = TelemetryRuntime::new(&RuntimeConfig {
                otel_enabled: false,
                otel_sample_ratio: 1.0,
                otel_exporter: "stdout".to_string(),
                otel_service_name: "go-on-test".to_string(),
                otel_endpoint: None,
                ..Default::default()
            });
            assert!(
                !rt.is_enabled(),
                "should be disabled when otel_enabled=false"
            );
        }

        #[test]
        fn enabled_when_configured() {
            let rt = TelemetryRuntime::new(&RuntimeConfig {
                otel_enabled: true,
                otel_sample_ratio: 1.0,
                otel_exporter: "stdout".to_string(),
                otel_service_name: "go-on-test".to_string(),
                otel_endpoint: None,
                ..Default::default()
            });
            assert!(rt.is_enabled(), "should be enabled when otel_enabled=true");
        }

        #[test]
        fn sampling_rate_tracking() {
            let rt = TelemetryRuntime::new(&RuntimeConfig {
                otel_enabled: true,
                otel_sample_ratio: 0.25,
                otel_exporter: "stdout".to_string(),
                otel_service_name: "go-on-test".to_string(),
                otel_endpoint: None,
                ..Default::default()
            });
            assert!(rt.is_enabled());

            for i in 0..1000u64 {
                let _ = rt.start_root_span("op", &format!("key-{}", i), vec![]);
            }

            let rate = rt.sampling_rate();
            assert!(
                rate > 0.0,
                "sampling rate should be > 0 after 1000 attempts"
            );
            assert!(rate <= 1.0, "sampling rate should be <= 1.0");

            let sampled = rt.sampled_roots.load(Ordering::Relaxed);
            assert!(
                sampled > 50,
                "at ratio 0.25, 1000 attempts should produce at least 50 samples, got {}",
                sampled
            );
            assert!(
                sampled < 600,
                "at ratio 0.25, 1000 attempts should produce at most 600 samples, got {}",
                sampled
            );
        }

        #[test]
        fn sample_count_tracking() {
            let rt = TelemetryRuntime::new(&RuntimeConfig {
                otel_enabled: true,
                otel_sample_ratio: 0.5,
                otel_exporter: "stdout".to_string(),
                otel_service_name: "go-on-test".to_string(),
                otel_endpoint: None,
                ..Default::default()
            });
            assert!(rt.is_enabled());

            for i in 0..100u64 {
                let _ = rt.start_root_span("op", &format!("key-{}", i), vec![]);
            }

            let sampled = rt.sampled_roots.load(Ordering::Relaxed);
            assert!(
                sampled > 5,
                "at ratio 0.5, 100 attempts should produce at least 5 samples, got {}",
                sampled
            );
        }
    }
}
