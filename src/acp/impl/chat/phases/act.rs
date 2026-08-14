//! Phase module: act.
//!
//! Split out of the former `chat_phases.rs` (M0.4).

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use futures_util::future::join_all;
use opentelemetry::Context as OtelContext;
use serde_json::{json, Value};
use tracing::{debug, info};

use crate::acp::helpers::autonomy_metrics::{
    record_cache_bypass_for_execution, record_cache_shortcircuit_refused,
};
use crate::acp::helpers::cache_strategy::{
    should_bypass_for_execution, CacheDecision, CacheStrategy,
};
use crate::acp::helpers::context::request_timeout;
use crate::acp::helpers::response_assembler::CapabilityRoutingInfo;
use crate::acp::helpers::review_gate::run_review_gate;
use crate::acp::helpers::vote_executor::{execute_high_risk_vote, HighRiskVoteExecutionResult};
use crate::acp::r#impl::chat::{
    agent_switch_state, apply_review_gate_assemble, auto_create_skills_from_conversation,
    auto_generate_workflow_from_conversation, emit_status_event, emit_stream_chunk,
    emit_stream_done, emit_stream_token_economy, estimate_token_economy,
    evaluate_pre_route_policies, execute_autonomy_round, execute_fallback_agents,
    extract_task_description, persist_chat_knowledge, persist_session_distillation,
    persist_vector_memory, resolve_request_phase, routing_handles, select_and_score_agents,
    AutonomyOutcome, ChatParams, ChatRequestContext, FallbackExecutionResult, RiskAssessment,
    RiskVotePolicy, StreamEventMeta, StreamObserver, VectorContext,
};
use crate::orchestration::mode::{resolve_mode_runtime, ModeKind};
use crate::orchestration::multi_agent_pipeline::MultiAgentPipeline;
use crate::rpc_protocol::{child_trace_context, RequestTraceContext};
// ═════════════════════════════════════════════════════════════════════

/// Phase 3: Execute actions: LLM calls, tool execution, autonomy loop,
/// fallback, vote, cache operations, scheduler.
pub(crate) async fn act_phase(
    server: &AcpServer,
    params: &ChatParams,
    trace: &RequestTraceContext,
    stream_observer: Option<StreamObserver>,
    started: Instant,
    resolve_out: &mut ObserveOutput,
    routing_out: &ThinkOutput,
) -> Result<ActOutput> {
    let mut selected_agent = String::new();
    let mut response_text = String::new();
    let mut reasoning_text = String::new();
    let mut selected_model_name: Option<String> = None;
    let mut last_err: Option<anyhow::Error> = None;
    let mut quota_failed_agents: Vec<String> = Vec::new();
    let mut agent_attempts: Vec<Value> = Vec::with_capacity(resolve_out.resolved.agents.len() + 2);
    let mut cache_hit = false;
    // Whether a `done` stream event was already emitted by an inner execution
    // path: cache hits via `stream_cache_response`, fallback via
    // `run_agent_collecting`, and the high-risk vote in-block emission. Only
    // the autonomy loop emits no `done` of its own, so the completion block
    // below must not double-emit for the other paths (previously every
    // cache-hit / fallback request sent two `chat.stream.done` notifications).
    let mut stream_done_emitted = false;
    let cache_bypassed_for_execution =
        should_bypass_for_execution(&params.mode, &routing_out.agent_messages);

    // Phase-level cache switch: `cache_enabled` in [phases.<name>.options] was
    // declared (and shipped in every config template) but never read — the
    // token/semantic lookups and the populate block below ran unconditionally.
    // Wire it here: `Some(false)` disables both lookup and populate for the
    // phase (equivalent to the execution-bypass path), matching the documented
    // semantics of the option.
    let phase_cache_enabled = resolve_out
        .phase
        .options
        .as_ref()
        .and_then(|opts| opts.cache_enabled)
        .unwrap_or(true);
    // Phase-level semantic-cache TTL override: `cache_ttl_seconds` was only
    // validated (rejected 0) but never applied. Pass it through to the
    // populate block so a configured per-phase TTL actually governs how long
    // the cached answer is reusable.
    let phase_cache_ttl_seconds = resolve_out
        .phase
        .options
        .as_ref()
        .and_then(|opts| opts.cache_ttl_seconds);
    if !phase_cache_enabled {
        tracing::debug!(
            target = "token_cache",
            phase = %resolve_out.phase_name,
            "act_phase: phase cache_enabled=false — skipping token/semantic cache"
        );
    }

    // Token & semantic caches (run concurrently for lower latency)
    let input_text = messages_to_text(&routing_out.agent_messages);
    let estimated_tokens = estimate_messages_token_count(&routing_out.agent_messages);
    let context_class = ContextLengthClass::from_token_count(estimated_tokens);

    // Semantic-cache key: the LAST user message (the current intent), not the
    // full conversation history. History grows append-only, so two consecutive
    // turns share a long prefix; keying on history would make every turn after
    // the first hash-collide with the first (the bucket hash truncates to
    // max_request_hash_len) and both the exact and similarity branches would
    // return turn-1's answer for turn-N's question.
    let semantic_key = routing_out
        .agent_messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .unwrap_or(&input_text);

    // Duplicate-user detection must run BEFORE the canonical lookup: if the
    // last user message repeats an earlier one, serving the cached answer would
    // silently return a stale response (previously the CachedAgentWrapper's
    // duplicate check never ran when act_phase's lookup hit first).
    let is_duplicate_user = crate::intelligence::token_cache::last_user_message_is_duplicate(
        &routing_out.agent_messages,
    );

    #[derive(Default)]
    struct TokenOutcome {
        response_text: String,
        selected_agent: String,
        agent_entry: Option<Value>,
    }

    #[derive(Default)]
    struct SemanticOutcome {
        response_text: String,
        selected_agent: String,
    }

    let (token_outcome, semantic_outcome) = tokio::join!(
        // ── Token cache lookup ────────────────────────────────────────
        async {
            if !phase_cache_enabled {
                // Phase-level switch off: behave like a miss (no lookup, no
                // entry) so the agent runs fresh.
                TokenOutcome::default()
            } else if is_duplicate_user {
                // Repeated user message → bypass cache so the agent produces
                // a fresh response (same intent as CachedAgentWrapper).
                tracing::debug!(
                    target = "token_cache",
                    "act_phase: last user message is a duplicate — bypassing cache"
                );
                TokenOutcome {
                    agent_entry: Some(
                        json!({"agent": "cache", "ok": false, "duplicate_user": true}),
                    ),
                    ..Default::default()
                }
            } else if let Some((level, entry, confidence)) = server
                .cache_deps
                .cache
                .token_cache
                .lookup(&input_text, context_class)
                .await
            {
                let decision = CacheStrategy::decide_from_entry(
                    &format!("{level}"),
                    &entry,
                    confidence,
                    cache_bypassed_for_execution,
                );
                match decision {
                    CacheDecision::Hit { response } => {
                        let agent = resolve_out
                            .resolved
                            .agents
                            .first()
                            .map(|(n, _)| n.clone())
                            .unwrap_or_else(|| "cached".to_string());
                        TokenOutcome {
                            response_text: response.clone(),
                            selected_agent: agent,
                            agent_entry: Some(
                                json!({"agent": "cache", "ok": true, "level": format!("{level}")}),
                            ),
                        }
                    }
                    CacheDecision::Refused { level, reason } => {
                        record_cache_shortcircuit_refused(&reason);
                        record_cache_bypass_for_execution();
                        TokenOutcome {
                            agent_entry: Some(
                                json!({"agent": "cache", "ok": false, "refused": true, "level": format!("{level}")}),
                            ),
                            ..Default::default()
                        }
                    }
                    CacheDecision::Miss => TokenOutcome::default(),
                }
            } else if cache_bypassed_for_execution {
                record_cache_bypass_for_execution();
                TokenOutcome {
                    agent_entry: Some(json!({"agent": "cache", "ok": false})),
                    ..Default::default()
                }
            } else {
                TokenOutcome::default()
            }
        },
        // ── Semantic cache lookup ─────────────────────────────────────
        async {
            if phase_cache_enabled && !cache_bypassed_for_execution && !is_duplicate_user {
                if let Some(text) = try_semantic_cache(server, semantic_key) {
                    let agent = resolve_out
                        .resolved
                        .agents
                        .first()
                        .map(|(n, _)| n.clone())
                        .unwrap_or_else(|| "cached".to_string());
                    return SemanticOutcome {
                        response_text: text,
                        selected_agent: agent,
                    };
                }
            }
            SemanticOutcome::default()
        },
    );

    // Merge: token cache has priority
    let token_hit = !token_outcome.response_text.is_empty();
    let semantic_hit = !semantic_outcome.response_text.is_empty();

    // Apply non-hit agent-entries immediately (Refused / bypass -- no streaming needed)
    if !token_hit {
        if let Some(ref entry) = token_outcome.agent_entry {
            agent_attempts.push(entry.clone());
        }
    }

    if token_hit {
        cache_hit = true;
        response_text.clone_from(&token_outcome.response_text);
        selected_agent.clone_from(&token_outcome.selected_agent);
        stream_cache_response(
            server,
            stream_observer.as_ref(),
            &selected_agent,
            &resolve_out.phase_name,
            &trace.trace_id,
            &response_text,
            &None,
            None,
        )
        .await?;
        stream_done_emitted = true;
        // Push hit entry only after successful stream (preserves original ordering)
        if let Some(entry) = token_outcome.agent_entry {
            agent_attempts.push(entry);
        }
    } else if semantic_hit {
        cache_hit = true;
        response_text.clone_from(&semantic_outcome.response_text);
        selected_agent.clone_from(&semantic_outcome.selected_agent);
        stream_cache_response(
            server,
            stream_observer.as_ref(),
            &selected_agent,
            &resolve_out.phase_name,
            &trace.trace_id,
            &response_text,
            &None,
            None,
        )
        .await?;
        stream_done_emitted = true;
    }

    // ── Pre-execution review gate (SafeGuard mode) ────────────────────
    // In non-enforce governance policy modes ("audit"/"advisory"/"disabled")
    // governance is log-only: skip the LLM review round-trip (two model calls
    // with reasoning traces) instead of blocking execution for trivial prompts.
    // Tool-level safety is still enforced per call via the harness bus.
    let enforce_review_gate = {
        let cfg = &server.runtime_config;
        let mode = cfg.governance_policy_mode.trim().to_ascii_lowercase();
        cfg.governance_enabled && (mode.is_empty() || mode == "active")
    };
    let mut review_blocked = false;
    let review_passed = match (ModeKind::from(params.mode.as_str()), enforce_review_gate) {
        (ModeKind::SafeGuard, false) => {
            tracing::info!(
                "safeguard review gate skipped (governance_policy_mode is not 'active')"
            );
            true
        }
        (ModeKind::SafeGuard, true) => {
            let outcome = run_review_gate(
                server,
                &params.messages,
                resolve_out.phase.options.as_ref(),
                None,
                trace,
            )
            .await;
            if !outcome.passed {
                let reason = if outcome.comments.is_empty() {
                    "SafeGuard review blocked the requested operation due to risk detection."
                        .to_string()
                } else {
                    format!(
                        "SafeGuard review blocked the requested operation: {}",
                        outcome.comments.join("; ")
                    )
                };
                tracing::info!(
                    "safeguard review gate blocked execution (verdict={:?}): {reason}",
                    outcome.verdict
                );
                emit_status_event(
                    stream_observer.as_ref(),
                    &format!("{} (verdict: {:?})", reason, outcome.verdict),
                )
                .await?;
                response_text = reason;
                review_blocked = true;
            }
            outcome.passed
        }
        _ => true,
    };

    // Autonomy round
    let progress_sse_tx = stream_observer.as_ref().and_then(|o| o.sse_sender());
    let autonomy_outcome = if review_passed {
        execute_autonomy_round(
            server,
            params,
            &resolve_out.phase,
            &resolve_out.phase_name,
            &resolve_out.resolved,
            &routing_out.agent_messages,
            &routing_out.base_agent_options,
            cache_hit,
            progress_sse_tx,
        )
        .await
    } else {
        AutonomyOutcome {
            autonomy_loop_executed: false,
            selected_agent: String::new(),
            response_text: String::new(),
            agent_attempts: Vec::new(),
            all_tools_failed: false,
        }
    };
    if autonomy_outcome.autonomy_loop_executed {
        selected_agent = autonomy_outcome.selected_agent;
        response_text = autonomy_outcome.response_text;
    }
    let all_tools_failed = autonomy_outcome.all_tools_failed;
    agent_attempts.extend(autonomy_outcome.agent_attempts);
    let autonomy_loop_executed = autonomy_outcome.autonomy_loop_executed;

    // BLUE56-GAP-C04: Record autonomy round execution in hyper-resilience engine
    if autonomy_loop_executed {
        let success = !response_text.is_empty() && last_err.is_none();
        server
            .resilience
            .hyper_resilience
            .record_execution(&selected_agent, success)
            .await;
    }

    // Fallback + vote
    let mut checkpoint = cognitive_empty_checkpoint();
    let mut knowledge = Value::Null;
    let mut metacognitive_loop = Value::Null;
    let mut distillation = Value::Null;
    // Hoisted out of the fallback block so the post-block stream completion
    // logic can tell whether the high-risk vote path already emitted done.
    let mut emit_final_vote = false;
    // Hoisted vote metadata so ActOutput carries the real values to reflect_phase
    // (risk_decision / routing_diagnostics consumers), instead of None/false.
    let mut used_multi_model_vote = false;
    let mut used_multi_agent_vote = false;
    let mut review_required = false;
    let mut vote_winner: Option<String> = None;
    let mut vote_report: Option<Value> = None;
    if !(cache_hit || autonomy_loop_executed && !response_text.trim().is_empty()) {
        let (fallback_result, vote_result, vote_flag) = execute_fallback_with_vote(
            server,
            params,
            &mut *resolve_out,
            routing_out,
            trace,
            stream_observer.clone(),
            agent_attempts,
        )
        .await?;
        emit_final_vote = vote_flag;

        selected_agent = fallback_result.selected_agent;
        response_text = fallback_result.response_text;
        reasoning_text = fallback_result.reasoning_text;
        selected_model_name = fallback_result.selected_model_name;
        last_err = fallback_result.last_err;
        quota_failed_agents = fallback_result.quota_failed_agents;
        agent_attempts = fallback_result.agent_attempts;

        let (
            used_multi_model_vote_val,
            used_multi_agent_vote_val,
            review_required_val,
            vote_winner_val,
            vote_report_val,
        ) = if emit_final_vote {
            response_text = vote_result.response_text;
            reasoning_text = vote_result.reasoning_text;
            selected_agent = vote_result.selected_agent;
            last_err = vote_result.last_err;
            agent_attempts.extend(vote_result.agent_attempts);
            stream_cache_response(
                server,
                stream_observer.as_ref(),
                &selected_agent,
                &resolve_out.phase_name,
                &trace.trace_id,
                &response_text,
                &selected_model_name,
                Some(&params.mode),
            )
            .await?;
            (
                vote_result.used_multi_model_vote,
                vote_result.used_multi_agent_vote,
                vote_result.review_required,
                vote_result.vote_winner,
                vote_result.vote_report,
            )
        } else {
            (false, false, false, None, None)
        };
        used_multi_model_vote = used_multi_model_vote_val;
        used_multi_agent_vote = used_multi_agent_vote_val;
        review_required = review_required_val;
        vote_winner = vote_winner_val;
        vote_report = vote_report_val;

        // Error handling
        if let Some(early_value) = handle_execution_errors(
            server,
            params,
            &resolve_out.phase_name,
            &resolve_out.phase_origin,
            &resolve_out.tenant_id,
            &response_text,
            &last_err,
            &agent_attempts,
            &quota_failed_agents,
            &routing_out.candidate_agents,
            &routing_out.risk_policy,
            &routing_out.risk_assessment,
            used_multi_model_vote,
            used_multi_agent_vote,
            review_required,
            &vote_report,
            started,
        )
        .await?
        {
            // Use the error prompt as the response text so the user sees
            // actionable guidance (e.g., quota exhaustion, switch agent, retry).
            if let Some(prompt) = early_value.get("prompt").and_then(|v| v.as_str()) {
                if response_text.trim().is_empty() {
                    response_text = prompt.to_string();
                }
            }
        }
        if let Some(err) = last_err.take() {
            return Err(err);
        }

        // Post-success cleanup
        if let Some(ref primary) = routing_out.configured_primary_agent {
            if selected_agent == *primary {
                let _ = agent_switch_state().write().map(|mut s| {
                    s.forced_agent_by_phase
                        .shift_remove(&resolve_out.phase_name)
                });
            }
        }
        let _ = server
            .resilience
            .outcome_tx
            .send(OutcomeEvent::PhaseOutcome {
                phase_name: resolve_out.phase_name.to_string(),
                success: true,
                duration_ms: started.elapsed().as_millis() as u64,
            });
        // Fallback (non-vote) emits its own `done` inside `run_agent_collecting`;
        // the vote path emits chunk+done in-block above. Either way a `done`
        // was already sent, so the completion block below must not re-emit.
        stream_done_emitted = true;
    }

    // Semantic + token cache populate — runs for ALL successful execution
    // paths (autonomy loop and fallback). Previously this block lived inside
    // the fallback-only branch, so autonomy-produced responses never filled
    // the caches they read on the next request.
    if phase_cache_enabled
        && !cache_hit
        && !response_text.is_empty()
        && !cache_bypassed_for_execution
    {
        // Clone BEFORE acquiring write lock to minimize critical section.
        let cached_response = Value::String(response_text.clone());
        {
            let guard = server
                .cache_deps
                .cache
                .semantic_cache
                .write()
                .unwrap_or_else(|p| p.into_inner());
            match phase_cache_ttl_seconds {
                // Per-phase TTL override: use the explicit-TTL insert path so
                // `cache_ttl_seconds` actually governs entry expiry instead of
                // the global default.
                Some(ttl) if ttl > 0 => guard.put_with_ttl(semantic_key, cached_response, ttl),
                _ => guard.put(semantic_key, cached_response),
            }
        }

        // Token cache populate (multi-level: L1 exact + L2 semantic + L3
        // durable) so the primary execution path fills the cache it reads
        // on the next request.
        let token_cache = server.cache_deps.cache.token_cache.clone();
        let model_name = selected_model_name.clone();
        let agent_for_cache = if selected_agent.is_empty() {
            None
        } else {
            Some(selected_agent.clone())
        };
        crate::acp::helpers::cache_strategy::store_async(
            token_cache,
            input_text.clone(),
            response_text.clone(),
            crate::intelligence::token_cache::estimate_token_count(&response_text),
            agent_for_cache,
            model_name,
        );
    }

    // Persistence + post-execute verification — run for ALL successful execution
    // paths (autonomy loop, fallback, or cache hit). Previously this was only
    // reachable via the fallback branch, so autonomy-loop chats never persisted
    // knowledge / checkpoints / distillation / vector memory.
    if !selected_agent.is_empty() && !response_text.is_empty() && last_err.is_none() {
        let mut msgs = params.messages.clone();
        msgs.push(Message {
            role: "assistant".to_string(),
            content: response_text.clone(),
        });
        let (kn, (new_checkpoint_from_join, ml), dst, _vec) = tokio::join!(
            persist_chat_knowledge(
                server,
                &routing_out.conversation_id,
                &routing_out.branch_id,
                &resolve_out.phase_name,
                &selected_agent,
                params,
                &response_text
            ),
            async {
                let cp = request::create_checkpoint_record(
                    server,
                    &routing_out.conversation_id,
                    &routing_out.branch_id,
                    msgs,
                    None,
                    None,
                )
                .await;
                let ml = request::persist_checkpoint_metacognitive_loop(
                    server,
                    &routing_out.conversation_id,
                    &routing_out.branch_id,
                    &cp.checkpoint_id,
                    json!({
                        "active": true, "schema_version": "blue25-metacognitive-loop-v1", "cycle_count": 1,
                        "checkpoint_id": cp.checkpoint_id, "last_reflection": format!("{}:{}", resolve_out.phase_name, selected_agent),
                        "trigger": "response_completed", "last_selected_agent": selected_agent, "response_chars": response_text.chars().count(),
                    })
                ).await;
                (cp, ml)
            },
            persist_session_distillation(
                server,
                &routing_out.conversation_id,
                &routing_out.branch_id,
                &resolve_out.phase_name,
                params,
                &selected_agent,
                &routing_out.candidate_agents,
                &agent_attempts,
                &response_text
            ),
            persist_vector_memory(
                server,
                &resolve_out.phase_name,
                resolve_out.phase.options.as_ref(),
                params,
                &response_text,
                &selected_agent,
            ),
        );
        checkpoint = crate::acp::ConversationCheckpoint {
            metacognitive_loop: Some(ml.clone()),
            ..new_checkpoint_from_join
        };
        knowledge = kn;
        metacognitive_loop = ml;
        distillation = dst;

        // HarnessBus post-execute — this runs in the PUA verification stage,
        // so pass the real stage so the PUA evidence chain is evaluated
        // against the actual stage requirements.
        if let Some(ref harness) = server.governance_deps.harness_bus {
            let output_v = harness.verify_output(
                &json!({"agent": &selected_agent, "response": &response_text, "reasoning": &reasoning_text, "phase": &resolve_out.phase_name}),
                "verification",
            );
            if !output_v.quality {
                tracing::warn!(target: "harness_bus", risk_score = output_v.risk_score, "post-execute: verification flagged quality issue");
            }
        }
    }

    // Stream completion events (telemetry + done) — emitted for ALL successful
    // execution paths (autonomy loop, fallback, or cache hit). Previously these
    // only fired inside the fallback branch, so autonomy-loop chats never sent
    // telemetry/done. The high-risk vote path already emits chunk+done in-block.
    if !selected_agent.is_empty() && !response_text.is_empty() && last_err.is_none() {
        if let Some(ref observer) = stream_observer {
            let meta = StreamEventMeta {
                agent_name: &selected_agent,
                phase_name: &resolve_out.phase_name,
                trace_id: &trace.trace_id,
                mode: Some(&params.mode),
                risk_score: None,
                degrade_policy: None,
            };
            emit_stream_token_economy(
                server,
                Some(observer),
                meta,
                &estimate_token_economy(&params.messages, &response_text),
            )
            .await?;
            if !emit_final_vote && !stream_done_emitted {
                let total_chars = response_text.chars().count();
                emit_stream_done(
                    server,
                    Some(observer),
                    meta,
                    1,
                    total_chars,
                    started.elapsed().as_millis() as u64,
                    selected_model_name.clone(),
                    Some(&response_text),
                )
                .await?;
            }
        }
    }

    // Trace event — recorded for ALL successful execution paths (autonomy loop,
    // fallback, or cache hit). Previously this only fired inside the fallback
    // branch, so autonomy-loop chats had no phase.agent telemetry.
    if !selected_agent.is_empty() && !response_text.is_empty() && last_err.is_none() {
        request::append_trace_event(TraceEvent {
            timestamp: crate::shared::timestamps::now_ts().to_string(),
            event_type: "phase.agent".into(),
            task_id: "chat".into(),
            phase: resolve_out.phase_name.clone(),
            agent: Some(selected_agent.clone()),
            tool: None,
            status: "ok".into(),
            inputs: json!({"attributes": {"agent": selected_agent.clone()}}),
            outputs: None,
            duration_ms: 0,
            error: None,
            pua_stage: None,
        });
    }

    // Global performance accounting for chat happens at the transport
    // boundary: the HTTP route (`route_http_post`) and the stdio dispatch
    // loop (`runtime.rs`) each record one op per request. Recording here as
    // well would double-count HTTP chat requests (route + act_phase) in the
    // same global store. Cache-hit and fallback-early chats are covered by
    // the transport-level records.
    Ok(ActOutput {
        selected_agent,
        response_text,
        reasoning_text,
        selected_model_name,
        last_err,
        cache_hit,
        cache_bypassed_for_execution,
        agent_attempts,
        quota_failed_agents,
        vote_winner,
        vote_report,
        used_multi_model_vote,
        used_multi_agent_vote,
        review_required,
        review_blocked,
        all_tools_failed,
        checkpoint,
        knowledge,
        metacognitive_loop,
        distillation,
    })
}

fn cognitive_empty_checkpoint() -> crate::acp::ConversationCheckpoint {
    crate::acp::ConversationCheckpoint {
        checkpoint_id: String::new(),
        conversation_id: String::new(),
        branch_id: String::new(),
        parent_checkpoint_id: None,
        created_at: 0,
        note: None,
        metacognitive_loop: None,
        messages: Vec::new(),
    }
}

// ── Execution internal helpers ──────────────────────────────────────────

pub(crate) fn try_semantic_cache(server: &AcpServer, cache_key: &str) -> Option<String> {
    server
        .cache_deps
        .cache
        .semantic_cache
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .get(cache_key)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn stream_cache_response(
    server: &AcpServer,
    observer: Option<&StreamObserver>,
    agent: &str,
    phase: &str,
    tid: &str,
    text: &str,
    model: &Option<String>,
    mode: Option<&str>,
) -> Result<()> {
    if let Some(o) = observer {
        let meta = StreamEventMeta {
            agent_name: agent,
            phase_name: phase,
            trace_id: tid,
            mode,
            risk_score: None,
            degrade_policy: None,
        };
        let total = text.chars().count();
        emit_stream_chunk(server, Some(o), meta, text, 1, total).await?;
        emit_stream_done(
            server,
            Some(o),
            meta,
            1,
            total,
            0u64,
            model.clone(),
            Some(text),
        )
        .await?;
    }
    Ok(())
}

async fn execute_fallback_with_vote(
    server: &AcpServer,
    params: &ChatParams,
    resolve_out: &mut ObserveOutput,
    routing_out: &ThinkOutput,
    trace: &RequestTraceContext,
    stream_observer: Option<StreamObserver>,
    _agent_attempts: Vec<Value>,
) -> Result<(FallbackExecutionResult, HighRiskVoteExecutionResult, bool)> {
    // Derive the effective operation mode the same way execute_autonomy_round
    // does, so fallback tool execution carries the real mode (approval events
    // report safeguard/edit/full_auto instead of a hard-coded "edit").
    let effective_mode = if params.mode.is_empty() {
        "edit"
    } else {
        params.mode.as_str()
    };
    let is_safeguard = effective_mode == "safeguard";
    let fallback_result = execute_fallback_agents(
        server,
        resolve_out.resolved.agents.clone(),
        &resolve_out.phase,
        &resolve_out.phase_name,
        routing_out.agent_messages.clone(),
        routing_out.base_agent_options.clone(),
        stream_observer,
        trace,
        routing_out.unhealthy_fallback_agent.clone(),
        routing_out.enable_high_risk_multi_agent_vote,
        routing_out.max_vote_agents,
        &resolve_out.tenant_id,
        effective_mode,
        is_safeguard,
    )
    .await;

    let vote_result = execute_high_risk_vote(
        server,
        &resolve_out.phase_name,
        &trace.trace_id,
        fallback_result.high_risk_vote_jobs.clone(),
        &routing_out.agent_messages,
        resolve_out.phase.principles.clone(),
        request_timeout(resolve_out.phase.options.as_ref()),
        false,
        routing_out.enable_high_risk_multi_agent_vote,
        routing_out.min_vote_agents,
        routing_out.max_vote_agents,
        routing_out.escalation_enabled,
        routing_out.escalation_models_per_agent,
        routing_out.escalation_max_agents,
        &resolve_out.reputation_scores,
        &mut resolve_out.routing_provenance,
    )
    .await;

    let emit_final_vote = vote_result.emit_final_vote_response;
    Ok((fallback_result, vote_result, emit_final_vote))
}

/// Handle errors from execution. Returns `Some(value)` for early return or `None` to continue.
#[allow(clippy::too_many_arguments)]
async fn handle_execution_errors(
    server: &AcpServer,
    params: &ChatParams,
    phase_name: &str,
    phase_origin: &str,
    _tenant_id: &str,
    response_text: &str,
    last_err: &Option<anyhow::Error>,
    agent_attempts: &[Value],
    quota_failed_agents: &[String],
    candidate_agents: &[String],
    risk_policy: &RiskVotePolicy,
    risk_assessment: &RiskAssessment,
    used_multi_model_vote: bool,
    used_multi_agent_vote: bool,
    review_required: bool,
    vote_report: &Option<Value>,
    started: Instant,
) -> Result<Option<Value>> {
    if response_text.is_empty() && last_err.is_none() {
        let all_empty = !agent_attempts.is_empty()
            && agent_attempts.iter().all(|a| {
                a.get("ok")
                    .and_then(|v| v.as_bool())
                    .map(|ok| !ok)
                    .unwrap_or(false)
                    && a.get("error")
                        .and_then(|v| v.as_str())
                        .map(|e| e == "empty_response")
                        .unwrap_or(false)
            });
        if all_empty {
            return Ok(Some(json!({
                "done": false, "mode": params.mode, "phase": phase_name, "phase_origin": phase_origin,
                "requires_user_action": true, "action": "retry",
                "prompt": tf("error.chat.all_agents_empty", &[("phase", phase_name)]),
                "agent_attempts": agent_attempts,
            })));
        }
        return Ok(Some(json!({
            "done": false, "mode": params.mode, "phase": phase_name, "phase_origin": phase_origin,
            "requires_user_action": true, "action": "retry",
            "prompt": tf("error.chat.no_healthy_agent", &[("phase", phase_name)]),
            "agent_attempts": agent_attempts,
        })));
    }
    if let Some(_err) = last_err {
        let all_quota = !agent_attempts.is_empty()
            && agent_attempts.iter().all(|a| {
                a.get("ok")
                    .and_then(|v| v.as_bool())
                    .map(|ok| !ok)
                    .unwrap_or(false)
                    && a.get("quota_limited")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
            });
        let _ = server
            .resilience
            .outcome_tx
            .send(OutcomeEvent::PhaseOutcome {
                phase_name: phase_name.to_string(),
                success: false,
                duration_ms: started.elapsed().as_millis() as u64,
            });
        if all_quota {
            return Ok(Some(json!({
                "done": false, "mode": params.mode, "phase": phase_name, "phase_origin": phase_origin,
                "requires_user_action": true, "action": "switch_agent",
                "prompt": tf("error.chat.all_agents_quota_limited", &[("phase", phase_name)]),
                "available_agents": candidate_agents, "quota_failed_agents": quota_failed_agents,
                "agent_attempts": agent_attempts,
                "risk_decision": json!({"policy_enabled": risk_policy.enabled, "score": risk_assessment.score,
                    "is_high_risk": risk_assessment.is_high_risk, "reasons": risk_assessment.reasons,
                    "multi_model_vote_enabled": used_multi_model_vote, "multi_agent_vote_enabled": used_multi_agent_vote,
                    "review_required": review_required, "vote_report": vote_report}),
                "hint": {"options_field": "options.extra.preferred_agent",
                    "example": {"preferred_agent": candidate_agents.first().cloned().unwrap_or_else(|| "primary".into())}},
            })));
        }
    }
    Ok(None)
}
