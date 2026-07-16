//! OpenTelemetry runtime bridge for ACP tracing (Phase 2).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use opentelemetry::global;
use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::Context;
use opentelemetry::KeyValue;
use sha2::{Digest, Sha256};

use crate::config::RuntimeConfig;

/// Guard against double-initialization of the global tracer provider.
/// Shared with `telemetry_enhanced` via `use crate::observability::telemetry::TRACER_INITIALIZED`.
pub(crate) static TRACER_INITIALIZED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Default)]
pub struct TelemetryRuntime {
    enabled: bool,
    sample_ratio: f64,
    total_roots: AtomicU64,
    sampled_roots: AtomicU64,
}

impl TelemetryRuntime {
    pub fn new(config: &RuntimeConfig) -> Self {
        // Telemetry initialization is now handled by
        // `telemetry_enhanced::init_telemetry` / `init_tracing` at the
        // bootstrap layer.  The legacy `init_otel_provider` has been removed.
        // When OTel is enabled in config, the bootstrap/CLI layer is
        // responsible for wiring the enhanced telemetry module.
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

    // ── Sampling logic (pure unit tests, NO global state) ────────────

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
        // Note: TelemetryRuntime::new() may access OTEL_INIT global if OTEL
        // is enabled, but clamping logic is independent of init state.
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

    // ── Span lifecycle (pure unit tests, NO global state) ─────────────

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
            let rt = TelemetryRuntime::new(&otel_config(false, 1.0));
            assert!(
                !rt.is_enabled(),
                "should be disabled when otel_enabled=false"
            );
        }

        #[test]
        fn enabled_when_configured() {
            let rt = TelemetryRuntime::new(&otel_config(true, 1.0));
            assert!(rt.is_enabled(), "should be enabled when otel_enabled=true");
        }

        #[test]
        fn sampling_rate_tracking() {
            let rt = TelemetryRuntime::new(&otel_config(true, 0.25));
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
            let rt = TelemetryRuntime::new(&otel_config(true, 0.5));
            assert!(rt.is_enabled());

            for i in 0..100u64 {
                let _ = rt.start_root_span("op", &format!("key-{}", i), vec![]);
            }

            let sampled = rt.sampled_roots.load(std::sync::atomic::Ordering::Relaxed);
            assert!(
                sampled > 5,
                "at ratio 0.5, 100 attempts should produce at least 5 samples, got {}",
                sampled
            );
        }
    }
}
