//! Phase module: reflect.
//!
//! Split out of the former `chat_phases.rs` (M0.4).

use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use opentelemetry::Context as OtelContext;
use serde_json::{json, Value};
use tracing::debug;

use crate::acp::helpers::response_assembler::CapabilityRoutingInfo;
use crate::acp::r#impl::chat::{
    apply_review_gate_assemble, auto_create_skills_from_conversation,
    auto_generate_workflow_from_conversation, estimate_token_economy, extract_task_description,
    ChatParams, StreamObserver,
};
use crate::acp::server::AcpServer;
use crate::orchestration::flow::ResolvedPhase;
use crate::orchestration::mode::{resolve_mode_runtime, ModeKind};
use crate::orchestration::multi_agent_pipeline::MultiAgentPipeline;
use crate::rpc_protocol::{child_trace_context, RequestTraceContext};

use super::observe::is_simple_chat;
use super::types::{ActOutput, ObserveOutput, ThinkOutput};
// Phase 4: Reflect
// ═════════════════════════════════════════════════════════════════════

/// Phase 4: Reflect on outcomes: response assembly, error handling,
/// knowledge persistence, metacognitive updates, threshold learning,
/// capability bus feedback, BrainLoop reflection.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn reflect_phase(
    server: &AcpServer,
    params: &ChatParams,
    trace: &RequestTraceContext,
    _span: Option<&OtelContext>,
    started: Instant,
    _stream_observer: Option<StreamObserver>,
    resolve_out: &ObserveOutput,
    routing_out: &ThinkOutput,
    exec_out: &mut ActOutput,
) -> Result<serde_json::Value> {
    // NOTE: the cache-hit early return is handled by `ChatPipeline::run`
    // before this phase — the identical branch inside `reflect_phase` was
    // unreachable (this function's only caller guards it).

    // ModeRuntime + MultiAgent — skip for ask mode since response is already handled
    let mode_kind = ModeKind::from(params.mode.as_str());
    if mode_kind != ModeKind::Ask {
        run_mode_runtime_and_multi_agent(
            server,
            params,
            trace,
            &resolve_out.phase_name,
            &resolve_out.phase,
            &resolve_out.resolved,
            exec_out,
        )
        .await;
    }

    // Extract inline tool calls from the model's natural-language response
    // for observability and audit. This catches tool calls that appear as
    // JSON blocks or inline markers (e.g. ```json {"tool_call": "read_file"} ```)
    // rather than through the structured streaming protocol.
    let inline_tool_calls =
        crate::acp::r#impl::chat::tool_extraction::extract_tool_calls_from_response(
            &exec_out.response_text,
            16,
        );
    if !inline_tool_calls.is_empty() {
        tracing::info!(
            target: "chat_phases",
            tool_calls = ?inline_tool_calls,
            "reflect_phase: detected {} inline tool call(s) in response",
            inline_tool_calls.len(),
        );
    }

    let risk_decision = json!({
        "policy_enabled": routing_out.risk_policy.enabled,
        "score": routing_out.risk_assessment.score,
        "is_high_risk": routing_out.risk_assessment.is_high_risk,
        "reasons": routing_out.risk_assessment.reasons,
        "multi_model_vote_enabled": routing_out.enable_high_risk_multi_agent_vote,
        "multi_model_vote_used": exec_out.used_multi_model_vote,
        "multi_agent_vote_enabled": routing_out.enable_high_risk_multi_agent_vote,
        "multi_agent_vote_used": exec_out.used_multi_agent_vote,
        "escalation_enabled": routing_out.escalation_enabled,
        "review_required": exec_out.review_required,
        "vote_report": exec_out.vote_report,
    });

    // Background skill/workflow generation runs concurrently with the
    // review-gate assembly — both are independent side-effects on the same
    // response text (the generators cap at one 2s timeout each).
    let response_text_for_skills = exec_out.response_text.clone();
    let empty_tool_results = Vec::<Value>::new();
    let (assemble_result, ()) = tokio::join!(
        apply_review_gate_assemble(
            server,
            params,
            trace,
            &resolve_out.phase_name,
            &resolve_out.phase_origin,
            &exec_out.selected_agent,
            &exec_out.selected_model_name,
            &exec_out.response_text,
            &exec_out.reasoning_text,
            &resolve_out.tenant_id,
            started,
            &routing_out.conversation_id,
            &routing_out.branch_id,
            resolve_out.schema_warnings.clone(),
            resolve_out.schema_error.clone(),
            routing_out.layered_prompt_segments,
            &empty_tool_results,
            &routing_out.candidate_agents,
            &resolve_out.routing_provenance,
            &resolve_out.reputation_scores,
            resolve_out
                .reputation_scores
                .get(&exec_out.selected_agent)
                .copied(),
            &routing_out.council_decision,
            &exec_out.vote_winner,
            &routing_out.fallback_reason,
            exec_out.cache_hit,
            exec_out.cache_bypassed_for_execution,
            CapabilityRoutingInfo {
                selected_agent: routing_out.capability_selected_agent.clone(),
                recommended_mode: routing_out.capability_recommended_mode.clone(),
                candidate_count: routing_out.capability_candidate_count,
                decision_confidence: routing_out.capability_decision_confidence,
                selection_reason: routing_out.capability_selection_reason.clone(),
                optimization_hint: routing_out.capability_optimization_hint.clone(),
            },
            Vec::new(),
            std::mem::take(&mut exec_out.agent_attempts),
            risk_decision,
            exec_out.quota_failed_agents.clone(),
            routing_out.vector_context.clone(),
            std::mem::take(&mut exec_out.knowledge),
            std::mem::take(&mut exec_out.distillation),
            std::mem::take(&mut exec_out.checkpoint),
            std::mem::take(&mut exec_out.metacognitive_loop),
        ),
        async {
            // Codex-style: skip for simple chat — no meaningful patterns to extract.
            if !is_simple_chat(params) {
                let (skills_res, workflow_res) = tokio::join!(
                    tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        auto_create_skills_from_conversation(
                            server,
                            params,
                            &response_text_for_skills
                        ),
                    ),
                    tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        auto_generate_workflow_from_conversation(
                            server,
                            params,
                            &response_text_for_skills
                        ),
                    ),
                );
                let _ = (skills_res, workflow_res);
            }
        },
    );
    let result = assemble_result?;

    // ── Independent post-execution side-effects ─────────────────────────
    // Rationalization is spawned fire-and-forget (see below); capability
    // feedback, memory-bus completion, provenance recording, and memory-bridge
    // store are mutually independent and hold only shared references —
    // serializing them added their latencies to the response. Run them
    // concurrently instead.
    let task_desc = extract_task_description(&params.messages);
    let confidence = if exec_out.response_text.is_empty() {
        0.3
    } else {
        0.8
    };
    // Rationalization (Delphi debate) runs off the response path: its verdict
    // only feeds a debug log, but awaiting it would block the response up to
    // the voter network timeouts (10s × voters). The audit records and
    // counters inside `rationalize_decision` still run — just asynchronously.
    {
        let agent = exec_out.selected_agent.clone();
        // `task_desc` is also borrowed by the provenance join-block below, so
        // move a clone into the fire-and-forget spawn instead of the original.
        let task_desc = task_desc.clone();
        tokio::spawn(async move {
            let (justified, reason) =
                crate::intelligence::hub::rationalize_decision(&agent, &task_desc, confidence)
                    .await;
            if !justified {
                debug!("rationalize: blocked agent={} reason={}", agent, reason);
            }
        });
    }
    let (_, _, _, _) = tokio::join!(
        capability_bus_feedback(
            server,
            trace,
            &resolve_out.phase_name,
            &exec_out.selected_agent,
            &exec_out.response_text,
            &exec_out.last_err,
            params,
        ),
        store_agent_memory_bus_completion(
            &exec_out.selected_agent,
            resolve_out.user_id.as_deref(),
            &resolve_out.phase_name,
            params,
            &exec_out.response_text,
            &exec_out.last_err,
        ),
        async {
            if let Some(ref ledger) = server.governance_deps.provenance_ledger {
                let _ = ledger
                    .record_provenance(
                        &trace.trace_id,
                        &task_desc,
                        &exec_out.selected_agent,
                        exec_out.last_err.is_none(),
                        started.elapsed().as_millis() as u64,
                    )
                    .await;
            }
        },
        async {
            // Memory bridge: persist reflection outcome (GAP-B54-011).
            // The entry carries the conversation id so session/load and
            // session/resume can restore it (D2: previously session_id was
            // always None, so the warm-tier session lookup found nothing).
            if let Some(mp) = server.get_or_init_memory_persistence() {
                use crate::memory::memory::{MemoryClass, MemoryEntry};
                let entry = MemoryEntry {
                    id: format!("reflect-{}", trace.request_id),
                    class: MemoryClass::Episodic,
                    content: if exec_out.response_text.is_empty() {
                        "empty_response".to_string()
                    } else {
                        exec_out.response_text.clone()
                    },
                    timestamp: crate::shared::timestamps::now_ts_ms().to_string(),
                    // Neutral usefulness for auto-recorded episodic memory
                    // (not user-rated).
                    usefulness: 0.5,
                    staleness: 0,
                    user_id: None,
                    session_id: Some(routing_out.conversation_id.clone()),
                };
                let _ = crate::memory::memory_bridge::bridge_store(
                    &server.persistence.memory_store,
                    mp.as_ref(),
                    entry,
                )
                .await;
            }
        },
    );

    // Metacognitive observation and fusion cycle — gated by
    // `enable_metacognitive_feedback` (the flag was previously written into
    // agent options but never read anywhere, so toggling it had no effect).
    //
    // NOTE: the per-request fire-and-forget persistence save was removed — the
    // periodic background save (every `maintenance_interval_seconds`, plus on
    // graceful shutdown) already persists the same snapshot, so the extra
    // per-request `create_dir_all` + rebuild + write (and its fixed tmp-file
    // races with the background writer) were redundant.
    if server.runtime_config.enable_metacognitive_feedback {
        if let Some(ref cb) = server.governance_deps.capability_bus {
            let success = !exec_out.response_text.is_empty() && exec_out.last_err.is_none();
            let task_desc = task_desc.clone();
            let now_ms = crate::shared::timestamps::now_ts_ms_u64();
            if let Ok(obs_id) = cb.metacognitive.record_observation(
                &format!("chat-{}", now_ms),
                &exec_out.selected_agent,
                if success {
                    "execution_success"
                } else {
                    "execution_failure"
                },
                if success { "info" } else { "error" },
                &format!(
                    "Chat execution for '{}': {}",
                    task_desc,
                    if success { "success" } else { "failed" }
                ),
            ) {
                if !success {
                    cb.metacognitive.autoreflect();
                    let _ = cb.metacognitive.resolve_observation(&obs_id);
                }
            }
        }

        // ── TripleFusion fusion cycle (fire-and-forget, non-blocking) ────────
        if let Some(ref cb) = server.governance_deps.capability_bus {
            let meta = cb.metacognitive.clone();
            let cs = cb.consciousness.clone();
            tokio::spawn(async move {
                let fusion_bridge =
                    crate::intelligence::triple_fusion::global_triple_fusion_bridge();
                let triggers = fusion_bridge.lock().await.run_fusion_cycle(&meta, &cs);
                crate::intelligence::fusion_evolution_bridge::send_triggers_to_evolution(triggers);
            });
        }
    }

    // Include all_tools_failed flag when all tools failed
    if exec_out.all_tools_failed {
        if let Some(obj) = result.as_object() {
            let mut enriched = obj.clone();
            enriched.insert(
                "all_tools_failed".to_string(),
                serde_json::Value::Bool(true),
            );
            enriched.insert(
                "error".to_string(),
                serde_json::Value::String(
                    "All tools failed to execute. The task could not be completed.".to_string(),
                ),
            );
            return Ok(serde_json::Value::Object(enriched));
        }
    }

    // Include review_blocked flag in the response when SafeGuard blocked execution
    if exec_out.review_blocked {
        if let Some(obj) = result.as_object() {
            let mut enriched = obj.clone();
            enriched.insert("review_blocked".to_string(), serde_json::Value::Bool(true));
            return Ok(serde_json::Value::Object(enriched));
        }
    }

    Ok(result)
}

// ── Response assembly helpers ───────────────────────────────────────

async fn run_mode_runtime_and_multi_agent(
    server: &AcpServer,
    params: &ChatParams,
    trace: &RequestTraceContext,
    phase_name: &str,
    _phase: &ResolvedPhase,
    resolved: &crate::orchestration::flow::ResolvedRouting,
    exec_out: &mut ActOutput,
) {
    let mode_runtime = match resolve_mode_runtime(
        &params.mode,
        server.agent_registry(),
        Some(exec_out.selected_agent.clone()),
    ) {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::warn!("failed to resolve mode runtime: {}", e);
            return;
        }
    };

    // Determine whether to run the mode runtime:
    //
    // ┌────────────┬─────────────────────────┬──────────────────────────────┐
    // │ All modes  │ Only as safety net      │ Captured (emergency         │
    // │            │ when act_phase          │ fallback) when act_phase    │
    // │            │ output is empty         │ output is empty             │
    // └────────────┴─────────────────────────┴──────────────────────────────┘
    //
    // The act_phase autonomy loop is the primary execution engine for ALL
    // modes (including FullAuto and SafeGuard). It runs the multi-round
    // think→act→observe cycle with tool execution, properly passing the
    // user's selected model via base_agent_options.
    //
    // Now all modes skip the mode runtime when act_phase produced output,
    // making FullAuto / SafeGuard fully autonomous via the autonomy loop.
    // The mode runtime is only kept as an emergency safety net: when act_phase
    // left both response AND agent empty, it can attempt recovery.
    let act_phase_produced_output =
        !exec_out.response_text.trim().is_empty() || !exec_out.selected_agent.trim().is_empty();
    let should_run = !exec_out.cache_hit && !act_phase_produced_output;

    if should_run {
        let envelope = crate::agent::AgentTaskEnvelope {
            task_id: format!("chat-{}-{}", phase_name, trace.request_id),
            phase: phase_name.to_string(),
            role: exec_out.selected_agent.clone(),
            objective: extract_task_description(&params.messages),
            constraints: None,
            evidence: Some(
                params
                    .messages
                    .iter()
                    .map(|m| format!("{}: {}", m.role, m.content))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            input: json!({"response_text": exec_out.response_text, "reasoning_text": exec_out.reasoning_text}),
        };
        if let Ok(result) = mode_runtime.run(envelope).await {
            // Capture the mode runtime's output back into exec_out so the
            // final "result" SSE event carries the actual response.
            // (should_run implies !act_phase_produced_output, so capture is
            // unconditional here — the old `should_capture` flag was always true.)
            if let Some(ref output) = result.output {
                let answer = output.get("answer").and_then(|v| v.as_str());
                let agent_from_output = output.get("agent").and_then(|v| v.as_str());
                if let Some(text) = answer {
                    if !text.trim().is_empty() {
                        exec_out.response_text = text.to_string();
                    }
                }
                if let Some(name) = agent_from_output {
                    if !name.trim().is_empty() {
                        exec_out.selected_agent = name.to_string();
                    }
                }
            }
        }
    }

    // ── Multi-agent pipeline (Edit + multiple agents) ──────────────────
    // Runs only as a safety net when the act-phase autonomy loop AND the mode
    // runtime above produced no output. The guard is re-evaluated AFTER the
    // mode runtime so a successful emergency recovery is not silently
    // overwritten by a second full agentic execution (previously the guard
    // used the pre-mode-runtime value, so the mode runtime's answer was always
    // discarded and the pipeline ran twice serially on the same request).
    let act_or_mode_produced_output =
        !exec_out.response_text.trim().is_empty() || !exec_out.selected_agent.trim().is_empty();
    if matches!(mode_runtime.kind(), ModeKind::Edit)
        && resolved.agents.len() > 1
        && !exec_out.cache_hit
        && !act_or_mode_produced_output
    {
        run_multi_agent_pipeline(server, params, resolved, exec_out, phase_name).await;
    }
}

async fn run_multi_agent_pipeline(
    server: &AcpServer,
    params: &ChatParams,
    resolved: &crate::orchestration::flow::ResolvedRouting,
    exec_out: &mut ActOutput,
    _phase_name: &str,
) {
    let desc = extract_task_description(&params.messages);

    // Single authoritative task analysis — reuse TaskRouter::analyze_task
    // instead of maintaining a second keyword/classification implementation
    // here (previously the two drifted on thresholds and type mapping).
    let task_chars = crate::orchestration::task_router::TaskRouter::analyze_task(&desc);
    let registry = server
        .agent_registry()
        .unwrap_or_else(|| Arc::new(crate::agent::AgentRegistry::new()));
    let pipeline_result = MultiAgentPipeline::new(registry)
        .execute(
            &desc,
            &task_chars,
            resolved.agents.first().map(|(_, a)| a.clone()),
        )
        .await;

    if pipeline_result.succeeded_count > 0 {
        exec_out.response_text = pipeline_result
            .merged_output
            .get("subtask_outputs")
            .and_then(|o| serde_json::to_string(o).ok())
            .unwrap_or_default();
        exec_out.selected_agent = format!(
            "multi-agent ({} succeeded)",
            pipeline_result.succeeded_count
        );
    }
}

#[allow(clippy::too_many_arguments)]
async fn capability_bus_feedback(
    server: &AcpServer,
    trace: &RequestTraceContext,
    phase_name: &str,
    selected_agent: &str,
    response_text: &str,
    last_err: &Option<anyhow::Error>,
    params: &ChatParams,
) {
    if let Some(ref cb) = server.governance_deps.capability_bus {
        let success = !response_text.is_empty() && last_err.is_none();
        let economy = estimate_token_economy(&params.messages, response_text);
        let token_cost_est = economy["total_tokens"].as_u64().unwrap_or(0);
        // NOTE: the per-request cb.feedback call was removed — the single
        // feedback point is finalize_chat_response (weight 1.0, stable
        // conversation_id). This function now only drives the THROTTLED
        // evolve() (every evolve_interval requests) so learning happens
        // without a per-request full pipeline spawn.

        // The evolve cycle is gated by `enable_capability_bus` (previously the
        // flag was never read — the 12-subsystem evolve ran regardless).
        if !cb.config.enable_capability_bus {
            return;
        }

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if count > 0 && count.is_multiple_of(cb.config.evolve_interval) {
            let cb = cb.clone();
            let agent_owned = selected_agent.to_string();
            let phase_owned = phase_name.to_string();
            // Pre-allocate the second copy to avoid a clone inside the tuple constructor.
            let agent_owned2 = agent_owned.clone();
            let phase_complete = format!("{}_complete", &phase_owned);
            let _child = child_trace_context(trace, "evolve");
            tokio::spawn(async move {
                cb.evolve(
                    &(agent_owned, phase_owned),
                    "chat_complete",
                    &(agent_owned2, phase_complete),
                    token_cost_est,
                    success,
                    if success { 0.8 } else { 0.2 },
                )
                .await;
            });
        }
    }
}

async fn store_agent_memory_bus_completion(
    selected_agent: &str,
    user_id: Option<&str>,
    phase_name: &str,
    params: &ChatParams,
    response_text: &str,
    last_err: &Option<anyhow::Error>,
) {
    use crate::memory::agent_memory_bus::AGENT_MEMORY_BUS;
    if let Some(bus) = AGENT_MEMORY_BUS.get() {
        let success = !response_text.is_empty() && last_err.is_none();
        bus.store_agent_completion(
            selected_agent,
            phase_name,
            &extract_task_description(&params.messages),
            response_text,
            success,
            user_id,
        )
        .await;
    }
}
