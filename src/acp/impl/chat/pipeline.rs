//! ChatPipeline — Unified OODA loop state machine for chat processing.
//!
//! Extracts the OODA (Observe-Think-Act-Reflect) phase orchestration from
//! `process_chat_request` into a proper state machine, DRYing up telemetry
//! span management and adding pipeline-level metrics.
//!
//! BLUE62 ARCH-1 migration: this replaces inline phase coordination.

use std::time::Instant;

use opentelemetry::Context as OtelContext;
use serde_json::{json, Value};
use tracing::info;

use crate::acp::r#impl::chat::params::{ChatParams, ChatRequestContext};
use crate::acp::r#impl::chat::streaming::{emit_phase_event, StreamObserver};
use crate::acp::r#impl::chat::phases::{act_phase, observe_phase, reflect_phase, think_phase};
use crate::acp::server::AcpServer;
use crate::rpc_protocol::RequestTraceContext;
use std::future::Future;

use anyhow::Result;

// ── Pipeline phase enum ──────────────────────────────────────────────────

/// The four phases of the OODA (Observe-Orient-Decide-Act) loop used
/// by the go-on chat pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelinePhase {
    Observe,
    Think,
    Act,
    Reflect,
}

impl PipelinePhase {
    /// OTel span name for this phase.
    pub fn name(&self) -> &'static str {
        match self {
            PipelinePhase::Observe => "chat.observe",
            PipelinePhase::Think => "chat.think",
            PipelinePhase::Act => "chat.act",
            PipelinePhase::Reflect => "chat.reflect",
        }
    }
}

// ── Pipeline phase timing ────────────────────────────────────────────────

/// Cumulative timing for each pipeline phase (in milliseconds).
#[derive(Debug, Clone, Default)]
pub struct PhaseTiming {
    pub observe_ms: u64,
    pub think_ms: u64,
    pub act_ms: u64,
    pub reflect_ms: u64,
    pub total_ms: u64,
}

impl PhaseTiming {
    fn record(&mut self, phase: PipelinePhase, elapsed_ms: u64) {
        match phase {
            PipelinePhase::Observe => self.observe_ms = elapsed_ms,
            PipelinePhase::Think => self.think_ms = elapsed_ms,
            PipelinePhase::Act => self.act_ms = elapsed_ms,
            PipelinePhase::Reflect => self.reflect_ms = elapsed_ms,
        }
    }
}

// ── Pipeline outcome ─────────────────────────────────────────────────────

/// Rich outcome of a pipeline run.
/// `timing`/`phase_name`/`mode` were previously duplicated onto the struct
/// solely to re-log the same per-phase timings in `process_chat_request`;
/// that duplicate log was removed and the fields were dropped (the timing
/// record lives in [`ChatPipeline::run`]).
pub struct PipelineOutcome {
    pub result: Value,
}

// ── ChatPipeline ─────────────────────────────────────────────────────────

/// Unified chat pipeline that drives the OODA loop with:
/// - State machine phase transitions
/// - Automatic OTel span management (removes boilerplate from callers)
/// - Per-phase timing metrics
/// - Early-exit for cache hits (skips reflection)
pub(crate) struct ChatPipeline;

impl ChatPipeline {
    /// Run the full OODA pipeline.
    ///
    /// Each phase automatically wraps its execution in an OTel child span.
    /// Timing is accumulated and returned in the [`PipelineOutcome`].
    pub(crate) async fn run(
        server: &AcpServer,
        params: &mut ChatParams,
        stream_observer: Option<StreamObserver>,
        trace: &RequestTraceContext,
        span: Option<&OtelContext>,
        ctx: Option<ChatRequestContext>,
    ) -> Result<PipelineOutcome> {
        let started = Instant::now();
        let ctx = ctx.unwrap_or_else(|| ChatRequestContext::new(None));
        let mut timing = PhaseTiming::default();

        // ── Phase 1: Observe ────────────────────────────────────────
        let observe_start = Instant::now();
        emit_phase_event(
            server,
            stream_observer.as_ref(),
            "phase_start",
            "observe",
            "Scanning project structure, validating inputs...",
            Some((1, 4)),
        )
        .await?;
        let mut resolve_out =
            Self::with_otel_span(server, PipelinePhase::Observe, span, || async {
                observe_phase(server, params, ctx.clone(), stream_observer.as_ref()).await
            })
            .await?;
        emit_phase_event(
            server,
            stream_observer.as_ref(),
            "phase_end",
            "observe",
            "Input validation complete",
            Some((1, 4)),
        )
        .await?;
        timing.record(
            PipelinePhase::Observe,
            observe_start.elapsed().as_millis() as u64,
        );

        // ── Phase 2: Think ──────────────────────────────────────────
        let think_start = Instant::now();
        emit_phase_event(
            server,
            stream_observer.as_ref(),
            "phase_start",
            "think",
            "Analyzing request, selecting agents, assessing risks...",
            Some((2, 4)),
        )
        .await?;
        let routing_out = Self::with_otel_span(server, PipelinePhase::Think, span, || async {
            think_phase(server, params, &mut resolve_out, trace).await
        })
        .await?;
        emit_phase_event(
            server,
            stream_observer.as_ref(),
            "phase_end",
            "think",
            "Analysis complete, routing resolved",
            Some((2, 4)),
        )
        .await?;
        timing.record(
            PipelinePhase::Think,
            think_start.elapsed().as_millis() as u64,
        );

        // ── Phase 3: Act ────────────────────────────────────────────
        let act_start = Instant::now();
        emit_phase_event(
            server,
            stream_observer.as_ref(),
            "phase_start",
            "act",
            "Executing plan, running tools...",
            Some((3, 4)),
        )
        .await?;
        let mut exec_out = Self::with_otel_span(server, PipelinePhase::Act, span, || async {
            act_phase(
                server,
                params,
                trace,
                stream_observer.clone(),
                started,
                &mut resolve_out,
                &routing_out,
            )
            .await
        })
        .await?;
        emit_phase_event(
            server,
            stream_observer.as_ref(),
            "phase_end",
            "act",
            "Execution complete, preparing reflection",
            Some((3, 4)),
        )
        .await?;
        timing.record(PipelinePhase::Act, act_start.elapsed().as_millis() as u64);

        // ── Cache-hit early exit ────────────────────────────────────
        if exec_out.cache_hit && !exec_out.response_text.is_empty() {
            let result = json!({
                "done": true,
                "mode": params.mode,
                "phase": resolve_out.phase_name,
                "phase_origin": resolve_out.phase_origin,
                "cached": true,
                "agent": exec_out.selected_agent,
                "response": exec_out.response_text,
            });
            timing.total_ms = started.elapsed().as_millis() as u64;
            info!(
                target: "chat_pipeline",
                mode = params.mode,
                phase = resolve_out.phase_name,
                timing.observe_ms,
                timing.think_ms,
                timing.act_ms,
                timing.reflect_ms,
                timing.total_ms,
                "skipped reflect phase — cache hit",
            );
            return Ok(PipelineOutcome { result });
        }

        // ── Phase 4: Reflect ───────────────────────────────────────
        let reflect_start = Instant::now();
        emit_phase_event(
            server,
            stream_observer.as_ref(),
            "phase_start",
            "reflect",
            "Generating final response, persisting knowledge...",
            Some((4, 4)),
        )
        .await?;
        let result = Self::with_otel_span(server, PipelinePhase::Reflect, span, || async {
            reflect_phase(
                server,
                params,
                trace,
                span,
                started,
                stream_observer.clone(),
                &resolve_out,
                &routing_out,
                &mut exec_out,
            )
            .await
        })
        .await?;
        emit_phase_event(
            server,
            stream_observer.as_ref(),
            "phase_end",
            "reflect",
            "Response complete",
            Some((4, 4)),
        )
        .await?;
        timing.record(
            PipelinePhase::Reflect,
            reflect_start.elapsed().as_millis() as u64,
        );
        timing.total_ms = started.elapsed().as_millis() as u64;

        // Log pipeline timing
        info!(
            target: "chat_pipeline",
            mode = params.mode,
            phase = resolve_out.phase_name,
            timing.observe_ms,
            timing.think_ms,
            timing.act_ms,
            timing.reflect_ms,
            timing.total_ms,
            "chat pipeline completed",
        );

        Ok(PipelineOutcome { result })
    }

    // ── OTel span helper ──────────────────────────────────────────────

    /// Execute `f` wrapped in an OTel child span for the given phase.
    ///
    /// If `parent_span` is `None`, no span is created (idempotent).
    /// If the telemetry mutex is poisoned, a warning is logged and
    /// execution continues without a span.
    async fn with_otel_span<T, F, Fut>(
        server: &AcpServer,
        phase: PipelinePhase,
        parent_span: Option<&OtelContext>,
        f: F,
    ) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let span_cx = parent_span.and_then(|parent| {
            server
                .observability
                .telemetry_runtime
                .lock()
                .unwrap_or_else(|poisoned| {
                    tracing::warn!("telemetry_runtime mutex poisoned during span creation");
                    poisoned.into_inner()
                })
                .start_child_span(parent, phase.name(), vec![])
        });

        let result = f().await;

        if let Some(cx) = span_cx {
            server
                .observability
                .telemetry_runtime
                .lock()
                .unwrap_or_else(|poisoned| {
                    tracing::warn!("telemetry_runtime mutex poisoned during span end");
                    poisoned.into_inner()
                })
                .end_span(cx, vec![]);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_phase_names() {
        assert_eq!(PipelinePhase::Observe.name(), "chat.observe");
        assert_eq!(PipelinePhase::Think.name(), "chat.think");
        assert_eq!(PipelinePhase::Act.name(), "chat.act");
        assert_eq!(PipelinePhase::Reflect.name(), "chat.reflect");
    }

    #[test]
    fn test_phase_timing_default() {
        let timing = PhaseTiming::default();
        assert_eq!(timing.observe_ms, 0);
        assert_eq!(timing.think_ms, 0);
        assert_eq!(timing.act_ms, 0);
        assert_eq!(timing.reflect_ms, 0);
        assert_eq!(timing.total_ms, 0);
    }

    #[test]
    fn test_phase_timing_record() {
        let mut timing = PhaseTiming::default();
        timing.record(PipelinePhase::Observe, 42);
        assert_eq!(timing.observe_ms, 42);
        timing.record(PipelinePhase::Reflect, 100);
        assert_eq!(timing.reflect_ms, 100);
    }

    #[test]
    fn test_pipeline_phase_equality() {
        assert_eq!(PipelinePhase::Observe, PipelinePhase::Observe);
        assert_ne!(PipelinePhase::Observe, PipelinePhase::Act);
    }
}
