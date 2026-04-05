//! OpenTelemetry runtime bridge for ACP tracing (Phase 2).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use opentelemetry::global;
use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::{Context, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::runtime::Tokio;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::TracerProvider;
use sha2::{Digest, Sha256};

use crate::config::RuntimeConfig;

static OTEL_INIT: OnceLock<Result<(), String>> = OnceLock::new();

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

        let init = OTEL_INIT.get_or_init(|| {
            init_otel_provider(
                config.otel_exporter.as_str(),
                config.otel_endpoint.clone(),
                config.otel_service_name.as_str(),
            )
            .map_err(|err| err.to_string())
        });

        if init.is_err() {
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

fn init_otel_provider(exporter: &str, endpoint: Option<String>, service_name: &str) -> Result<()> {
    let exporter_name = exporter.to_ascii_lowercase();
    let target_endpoint = endpoint.unwrap_or_else(|| "http://127.0.0.1:4317".to_string());

    // Jaeger support uses OTLP endpoint (Jaeger collector supports OTLP ingest).
    if exporter_name != "otlp" && exporter_name != "jaeger" {
        anyhow::bail!("unsupported otel exporter: {}", exporter);
    }

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(target_endpoint)
        .build()?;

    let provider = TracerProvider::builder()
        .with_resource(Resource::new(vec![KeyValue::new(
            "service.name",
            service_name.to_string(),
        )]))
        .with_batch_exporter(exporter, Tokio)
        .build();

    global::set_tracer_provider(provider);
    Ok(())
}