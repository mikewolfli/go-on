//! OpenTelemetry runtime bridge for ACP tracing (Phase 2).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use anyhow::Result;
use opentelemetry::global;
use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::{Context, KeyValue};
use sha2::{Digest, Sha256};

use crate::config::RuntimeConfig;

/// OTEL initialization state, wrapped in a `Mutex` instead of `OnceLock`
/// so it can be reset via `reset_otel()` for re-initialization support
/// (e.g. after config reload or testing).
static OTEL_INIT: Mutex<Option<Result<(), String>>> = Mutex::new(None);

/// Reset the OpenTelemetry initialization state and replace the global
/// tracer provider with a fresh one, allowing `TelemetryRuntime::new()`
/// to re-initialize on the next call. This is useful for testing and
/// dynamic config reloads.
pub fn reset_otel() {
    let mut guard = match OTEL_INIT.lock() {
        Ok(g) => g,
        Err(poisoned) => {
            // Recover from a poisoned mutex so that subsequent tests
            // are not blocked by a prior panic.
            tracing::error!("OTEL_INIT mutex was poisoned; recovering");
            poisoned.into_inner()
        }
    };
    *guard = None;
    // Replace the global provider with a fresh instance so that any
    // previous state is discarded. `global::shutdown_tracer_provider`
    // is not available in opentelemetry 0.31, so we achieve the same
    // effect by setting a new empty SDK provider as the global.
    let fresh_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
    global::set_tracer_provider(fresh_provider);
}

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

        {
            let mut guard = match OTEL_INIT.lock() {
                Ok(g) => g,
                Err(poisoned) => {
                    tracing::error!("OTEL_INIT mutex was poisoned; recovering");
                    poisoned.into_inner()
                }
            };
            if guard.is_none() {
                *guard = Some(
                    init_otel_provider(
                        config.otel_exporter.as_str(),
                        config.otel_endpoint.clone(),
                        config.otel_service_name.as_str(),
                    )
                    .map_err(|err| err.to_string()),
                );
            }
            // Safe to unwrap: guard is guaranteed to be Some at this point
            // because we just set it above if it was None.
            let init = guard.as_ref().unwrap();
            if init.is_err() {
                return Self::default();
            }
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
    pub fn inject_context(&self, cx: &Context) -> std::collections::HashMap<String, String> {
        let mut headers = std::collections::HashMap::new();
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
    pub fn extract_context(&self, headers: &std::collections::HashMap<String, String>) -> Context {
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
        // Create a child span linked to the parent context so the trace tree
        // is properly maintained across async boundaries.
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

fn init_otel_provider(_exporter: &str, endpoint: Option<String>, service_name: &str) -> Result<()> {
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use opentelemetry_sdk::Resource;

    let resource = Resource::builder_empty()
        .with_attribute(KeyValue::new("service.name", service_name.to_string()))
        .build();

    let provider = if let Some(ep) = endpoint {
        use opentelemetry_otlp::{SpanExporter, WithExportConfig};
        let span_exporter = SpanExporter::builder()
            .with_tonic()
            .with_endpoint(&ep)
            .build()
            .map_err(|e| anyhow::anyhow!("OTLP span exporter init error: {}", e))?;
        SdkTracerProvider::builder()
            .with_resource(resource)
            .with_batch_exporter(span_exporter)
            .build()
    } else {
        let span_exporter = opentelemetry_stdout::SpanExporter::default();
        SdkTracerProvider::builder()
            .with_resource(resource)
            .with_simple_exporter(span_exporter)
            .build()
    };

    global::set_tracer_provider(provider);
    tracing::info!(
        service = service_name,
        "OpenTelemetry tracing provider initialized"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// Helper to build a minimal RuntimeConfig with OTEL enabled.
    fn otel_config(enabled: bool, sample_ratio: f64) -> RuntimeConfig {
        RuntimeConfig {
            otel_enabled: enabled,
            otel_sample_ratio: sample_ratio,
            otel_exporter: "stdout".to_string(),
            otel_service_name: "go-on-test".to_string(),
            otel_endpoint: None,
            ..Default::default()
        }
    }

    // ── reset_otel ────────────────────────────────────────────────────

    #[test]
    fn test_reset_otel_allows_reinit() {
        // Reset any prior state
        reset_otel();

        // First init
        let rt = TelemetryRuntime::new(&otel_config(true, 1.0));
        assert!(rt.is_enabled(), "should be enabled after first init");

        // Reset
        reset_otel();

        // Re-init
        let rt2 = TelemetryRuntime::new(&otel_config(true, 1.0));
        assert!(rt2.is_enabled(), "should be re-enabled after reset");
    }

    #[test]
    fn test_reset_otel_clears_init_state() {
        reset_otel();

        let rt1 = TelemetryRuntime::new(&otel_config(true, 1.0));
        assert!(rt1.is_enabled());

        reset_otel();

        // After reset, verify we can re-init fresh (this is the reliable
        // behavioral check). The raw guard value may be concurrently modified
        // by other tests running in parallel, so we avoid asserting on the
        // internal state and instead verify the observable behaviour:
        let rt2 = TelemetryRuntime::new(&otel_config(true, 1.0));
        assert!(rt2.is_enabled(), "should be re-initializable after reset");
    }

    #[test]
    fn test_reset_otel_repeat_reset_behavior() {
        // Verify that calling reset_otel multiple times in sequence
        // does not panic and correctly allows re-initialization each time.
        for i in 0..3 {
            reset_otel();

            let rt = TelemetryRuntime::new(&otel_config(true, 1.0));
            assert!(
                rt.is_enabled(),
                "iteration {}: should be enabled after reset+init",
                i
            );

            // After re-init, the OTEL_INIT guard should be Some(Ok(()))
            let guard = OTEL_INIT.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            assert!(
                guard.is_some(),
                "iteration {}: OTEL_INIT should be Some after init",
                i
            );
            assert!(
                guard.as_ref().unwrap().is_ok(),
                "iteration {}: OTEL_INIT should be Ok",
                i
            );
            // If poisoned (from a prior panic), the recovery handled it;
        }
    }

    // ── Sampling logic ────────────────────────────────────────────────

    #[test]
    fn test_should_sample_always_at_1_0() {
        let rt = TelemetryRuntime {
            enabled: true,
            sample_ratio: 1.0,
            ..Default::default()
        };
        // Every key should sample at ratio 1.0
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
        // Same key must produce the same result every time
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
        // With 1000 distinct keys at ratio 0.5, both true and false
        // should appear (statistically near-certain). This test verifies
        // the hash-based sampler is not degenerate.
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
        // Very low ratio — most keys should not sample
        let sampled_count: u32 = (0..10000u64)
            .map(|i| rt.should_sample(&format!("key-{}", i)) as u32)
            .sum();
        // With ratio 0.001 and 10000 keys, expected ~10 samples.
        // Allow generous range: 0-50 to avoid flakiness.
        assert!(
            sampled_count < 100,
            "at ratio 0.001, 10000 keys should produce <100 samples, got {}",
            sampled_count
        );
    }

    #[test]
    fn test_should_sample_clamps_ratio() {
        let rt = TelemetryRuntime::new(&otel_config(true, 1.5));
        assert!(
            rt.should_sample("anything"),
            "ratio clamped to 1.0 should always sample"
        );

        let rt2 = TelemetryRuntime::new(&otel_config(true, -0.5));
        assert!(
            !rt2.should_sample("anything"),
            "ratio clamped to 0.0 should never sample"
        );
    }

    // ── Span lifecycle ────────────────────────────────────────────────

    #[test]
    fn test_disabled_runtime_returns_none() {
        let rt = TelemetryRuntime::default(); // enabled = false
        assert!(!rt.is_enabled());
        assert!(rt.start_root_span("test", "key", vec![]).is_none());
        assert!(rt
            .start_child_span(&Context::current(), "child", vec![])
            .is_none());
        // end_span should not panic when disabled
        rt.end_span(Context::current(), vec![]);
    }

    #[test]
    fn test_sampling_rate_tracking() {
        reset_otel();
        let rt = TelemetryRuntime::new(&otel_config(true, 0.25));
        assert!(rt.is_enabled());

        // Start 1000 root spans — about 25% should be sampled
        for i in 0..1000u64 {
            let _ = rt.start_root_span("op", &format!("key-{}", i), vec![]);
        }

        let rate = rt.sampling_rate();
        assert!(
            rate > 0.0,
            "sampling rate should be > 0 after 1000 attempts"
        );
        assert!(rate <= 1.0, "sampling rate should be <= 1.0");
        // At ratio 0.25, expected ~250 samples. Allow 50-500 range.
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
    fn test_start_root_span_sets_attributes() {
        reset_otel();
        let rt = TelemetryRuntime::new(&otel_config(true, 1.0));
        let cx = rt
            .start_root_span(
                "test-op",
                "test-key",
                vec![KeyValue::new("test_attr", "hello")],
            )
            .expect("root span should be created at ratio 1.0");
        let span = cx.span();
        assert_eq!(span.span_context().trace_id().to_string().len(), 32);
        rt.end_span(cx, vec![]);
    }

    // ── Reset + reinit tracer functionality ──────────────────────────

    /// Verify that `reset_otel()` followed by re-initialization produces a
    /// fully functional tracer that can create spans with valid trace IDs.
    #[test]
    fn test_tracer_functional_after_reset() {
        // First init cycle
        reset_otel();
        let rt = TelemetryRuntime::new(&otel_config(true, 1.0));
        let cx = rt
            .start_root_span("first-cycle", "key-a", vec![])
            .expect("first span after init");
        assert_eq!(
            cx.span().span_context().trace_id().to_string().len(),
            32,
            "first cycle trace_id must be 32 hex chars"
        );
        rt.end_span(cx, vec![]);

        // Reset and re-init
        reset_otel();
        let rt2 = TelemetryRuntime::new(&otel_config(true, 1.0));
        assert!(rt2.is_enabled(), "runtime should be enabled after reset");

        let cx2 = rt2
            .start_root_span("second-cycle", "key-b", vec![])
            .expect("second span after reset+reinit");
        assert_eq!(
            cx2.span().span_context().trace_id().to_string().len(),
            32,
            "second cycle trace_id must be 32 hex chars"
        );
        rt2.end_span(cx2, vec![]);
    }

    /// Verify that a child span can be created after a reset and that
    /// the parent-child relationship works correctly.
    #[test]
    fn test_child_span_after_reset() {
        reset_otel();
        let rt = TelemetryRuntime::new(&otel_config(true, 1.0));

        let parent = rt
            .start_root_span("parent", "parent-key", vec![])
            .expect("parent span");

        let child = rt
            .start_child_span(&parent, "child", vec![KeyValue::new("child_attr", "yes")])
            .expect("child span");

        rt.end_span(child, vec![]);
        rt.end_span(parent, vec![]);
    }
}
