//! Chat handling implementation functions for ACP server
//!
//! This module contains standalone functions that implement chat handling
//! functionality, organized into 10 sub-modules:
//! `agent_runtime`, `agent_selection`, `fallback`, `knowledge`, `params`,
//! `session`, `streaming`, `tool_extraction`, `vector_context`, `voting`.
//!
//! These functions take `AcpServer` as their first parameter to maintain
//! compatibility with the original implementation.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use anyhow::Result;
use opentelemetry::Context as OtelContext;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::acp::helpers::agent_selector::{collect_reputation_scores, AgentSelector};
use crate::acp::helpers::autonomy_metrics::{
    record_agent_switch, record_autonomy_loop_stop_reason, record_reputation_routing_applied,
};
use crate::acp::helpers::context::{
    probe_agent_runtime_readiness, request_timeout, AgentRuntimeReadiness,
};

use crate::acp::helpers::response_assembler::{
    build_chat_response, build_role_routing, build_task_graph_checkpoint, ChatResponseContext,
};
use crate::acp::helpers::review_gate::run_enhanced_verification;
use crate::acp::server::{AcpServer, OutcomeEvent};
use crate::agent::Message;
use crate::config::PhaseOptions;
use crate::flow::FlowManager;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::mode::ModeKind;

use crate::orchestration::task_router::{TaskRouter, TaskType};

use crate::memory_module::{MemoryClass, MemoryEntry, MemoryPolicy, MemoryStore};
use crate::reinforcement::{
    build_task_plan, build_workflow_generated_artifact, persist_workflow_generated, ArtifactLedger,
};
use crate::rpc_protocol::RequestTraceContext;

pub mod agent_runtime;
pub mod agent_selection;
pub mod fallback;
pub mod knowledge;
pub mod params;
pub mod pipeline;
pub mod session;
pub mod streaming;
pub mod tool_extraction;
pub mod vector_context;
pub mod voting;

pub(crate) use agent_runtime::run_agent_collecting;
pub(crate) use session::handle_chat;
pub(crate) use tool_extraction::detect_repeated_task_pattern;

// Re-export streaming items that are part of the chat module's public API
pub use self::params::ChatParams;
pub use self::params::ChatRequestContext;
pub(crate) use self::voting::{
    normalize_vote_key, option_bool, select_strong_model_id, select_top_models,
    AgentStrongVoteOutcome, AgentVoteSource, HighRiskVoteAttemptResult, RiskAssessment,
    RiskVotePolicy,
};
pub(crate) use crate::acp::helpers::agent_preference::agent_switch_state;
pub(crate) use knowledge::{
    persist_chat_knowledge, persist_session_distillation, persist_vector_memory, round_metric,
    truncate_chars,
};
pub use streaming::pre_init_sse_buffer_pool;
pub(crate) use streaming::{
    acquire_sse_buffer, emit_status_event, emit_stream_chunk, emit_stream_done,
    emit_stream_token_economy, release_sse_buffer, StreamEventMeta, StreamFrame,
    StreamNotificationContext, StreamObserver,
};
pub(crate) use vector_context::{
    build_phase_summary, build_vector_context_message, effective_vector_settings,
    generate_phase_summary_text, latest_user_message, load_vector_context,
    merge_context_into_messages, VectorContext,
};

pub(crate) fn estimate_token_economy(messages: &[Message], response_text: &str) -> Value {
    let input_chars = messages
        .iter()
        .map(|message| message.content.chars().count())
        .sum::<usize>();
    let output_chars = response_text.chars().count();
    let input_tokens = if input_chars == 0 {
        0_u64
    } else {
        input_chars.div_ceil(4) as u64
    };
    let output_tokens = if output_chars == 0 {
        0_u64
    } else {
        output_chars.div_ceil(4) as u64
    };
    let compression_ratio = if input_tokens == 0 {
        1.0
    } else {
        round_metric(output_tokens as f64 / input_tokens as f64)
    };
    let saving_ratio = if input_tokens == 0 {
        0.0
    } else {
        round_metric((1.0 - compression_ratio).clamp(0.0, 1.0))
    };

    json!({
        "schema_version": "blue25-stream-token-economy-v1",
        "round": 1,
        "input_chars": input_chars,
        "output_chars": output_chars,
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": input_tokens + output_tokens,
        "compression_ratio": compression_ratio,
        "saving_ratio": saving_ratio,
        "efficiency_class": if compression_ratio <= 0.60 {
            "strong"
        } else if compression_ratio <= 0.85 {
            "efficient"
        } else {
            "expanded"
        },
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_high_risk_vote_attempt(
    server: &AcpServer,
    phase_name: &str,
    trace_id: &str,
    agent_name: String,
    agent: Arc<dyn crate::agent::Agent>,
    agent_messages: Vec<Message>,
    principles: Option<Vec<String>>,
    options: HashMap<String, Value>,
    timeout: Option<Duration>,
    strong_model: Option<String>,
    vote_mode: &'static str,
) -> HighRiskVoteAttemptResult {
    let attempt_started = Instant::now();
    let model = strong_model.clone();

    let outcome = run_agent_collecting(
        server,
        StreamNotificationContext {
            stream_observer: None,
            agent_name: &agent_name,
            phase_name,
            trace_id,
        },
        Arc::clone(&agent),
        agent_messages,
        principles,
        Some(options.clone()),
        timeout,
    )
    .await;

    let elapsed_ms = attempt_started.elapsed().as_millis() as u64;

    match outcome {
        Ok((output_text, reasoning_output, _sel_m)) => {
            let success = !output_text.trim().is_empty();
            let _ = server
                .resilience
                .outcome_tx
                .send(OutcomeEvent::AgentOutcome {
                    phase_name: phase_name.to_string(),
                    agent_name: agent_name.to_string(),
                    success,
                    duration_ms: elapsed_ms,
                });

            if success {
                HighRiskVoteAttemptResult {
                    attempt_log: json!({
                        "agent": agent_name,
                        "ok": true,
                        "duration_ms": elapsed_ms,
                        "risk_vote_mode": vote_mode,
                        "model": model,
                    }),
                    candidate: Some(AgentStrongVoteOutcome {
                        agent: agent_name.clone(),
                        model: strong_model,
                        response: output_text,
                        reasoning: reasoning_output,
                    }),
                    source: Some((agent_name, agent, options)),
                    failure: None,
                }
            } else {
                HighRiskVoteAttemptResult {
                    attempt_log: json!({
                        "agent": agent_name,
                        "ok": false,
                        "duration_ms": elapsed_ms,
                        "risk_vote_mode": vote_mode,
                        "error": "empty_response",
                    }),
                    candidate: None,
                    source: None,
                    failure: Some(json!({
                        "agent": agent_name,
                        "reason": "empty_response",
                    })),
                }
            }
        }
        Err(err) => {
            let err_text = err.to_string();
            let _ = server
                .resilience
                .outcome_tx
                .send(OutcomeEvent::AgentOutcome {
                    phase_name: phase_name.to_string(),
                    agent_name: agent_name.to_string(),
                    success: false,
                    duration_ms: elapsed_ms,
                });

            HighRiskVoteAttemptResult {
                attempt_log: json!({
                    "agent": agent_name,
                    "ok": false,
                    "duration_ms": elapsed_ms,
                    "risk_vote_mode": vote_mode,
                    "error": err_text,
                }),
                candidate: None,
                source: None,
                failure: Some(json!({
                    "agent": agent_name,
                    "reason": err_text,
                })),
            }
        }
    }
}

pub(crate) fn reorder_agents_with_priority(
    agents: &mut Vec<(String, Arc<dyn crate::agent::Agent>)>,
    preferred: &str,
) -> bool {
    if let Some(index) = agents.iter().position(|(name, _)| name == preferred) {
        if index > 0 {
            let selected = agents.remove(index);
            agents.insert(0, selected);
        }
        return true;
    }
    false
}

/// Determine if approval strategy should be escalated
///
/// This function replaces the `AcpServer::should_escalate_approval_strategy` method.
pub async fn should_escalate_approval_strategy(
    server: &AcpServer,
    mode: &str,
    messages: &[Message],
    conversation_id: Option<&str>,
    phase: Option<&str>,
    options: Option<&PhaseOptions>,
) -> Result<bool> {
    // ── 1. Governance policy evaluation (HarnessBus) ────────────────
    // The canonical escalation path: consult the HarnessBus policy engine.
    // If the policy engine denies the action, escalation is required.
    if let Some(ref harness) = server.governance_deps.harness_bus {
        let payload = serde_json::json!({
            "mode": mode,
            "phase": phase,
            "message_count": messages.len(),
            "total_content_length": messages.iter().map(|m| m.content.len()).sum::<usize>(),
        });
        let verdict = harness.validate_action("chat.escalate", &payload);
        if !verdict.is_allowed() {
            return Ok(true);
        }
    }

    // ── 2. Mode-based escalation ────────────────────────────────────
    // Plan mode never escalates (planning is always safe).
    // Ask mode never escalates (single-turn Q&A).
    // Edit mode escalates only if high-risk or sensitive.
    // FullAuto escalates only when complexity exceeds threshold.
    // SafeGuard inherently requires escalation for human review.
    let mode_requires_escalation = match mode {
        "safeguard" => true,
        "full_auto" => {
            // FullAuto only escalates when there are complex instructions
            // or sensitive content. Simple automation tasks don't need escalation.
            let total_content_len: usize = messages.iter().map(|m| m.content.len()).sum();
            let has_complex_indicators = messages.iter().any(|msg| {
                let c = msg.content.to_lowercase();
                c.contains("multiple steps")
                    || c.contains("complex")
                    || c.contains("critical")
                    || c.contains("recursive")
                    || (c.matches("```").count() > 2)
            });
            total_content_len > 2000 || has_complex_indicators
        }
        "edit" => {
            // Edit mode escalates only for high-risk operations
            messages.iter().any(|msg| {
                let c = msg.content.to_lowercase();
                c.contains("delete production")
                    || c.contains("drop database")
                    || c.contains("rm -rf")
                    || c.contains("shutdown")
            })
        }
        _ => false,
    };

    // ── 3. Injection/probe detection ───────────────────────────────
    let mut has_injection = false;
    if let Some(ref detector) = server.governance_deps.injection_detector {
        let joined: String = messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<&str>>()
            .join("\n");
        let warnings = detector.detect(&joined);
        if !warnings.violations.is_empty() {
            has_injection = true;
            tracing::warn!(
                target: "escalation",
                violations = ?warnings.violations,
                "prompt injection detected — escalating approval"
            );
        }
    }

    // ── 4. Sensitive content detection (safety checker) ────────────
    let has_sensitive_content = messages.iter().any(|msg| {
        let content = msg.content.to_lowercase();
        content.contains("password")
            || content.contains("api_key")
            || content.contains("secret_key")
            || content.contains("token=")
            || content.contains("authorization: bearer")
            || content.contains("credentials")
            || content.contains("confidential")
            || content.contains("ssn")
            || content.contains("social security")
    });

    // ── 5. Conversation history ────────────────────────────────────
    let history_requires_escalation = if let Some(conv_id) = conversation_id {
        check_conversation_history_escalation(server, conv_id).await?
    } else {
        false
    };

    // ── 6. Phase-specific escalation rules ─────────────────────────
    let phase_requires_escalation = if let Some(phase_name) = phase {
        check_phase_escalation_rules(server, phase_name, options).await?
    } else {
        false
    };

    Ok(mode_requires_escalation
        || has_injection
        || has_sensitive_content
        || history_requires_escalation
        || phase_requires_escalation)
}

pub(crate) async fn filter_runtime_ready_agents(
    server: &AcpServer,
    config: &crate::config::AppConfig,
    agents: &mut Vec<(String, Arc<dyn crate::agent::Agent>)>,
) -> Vec<String> {
    let mut unavailable = Vec::new();
    let mut retained = Vec::with_capacity(agents.len());

    for (name, agent) in std::mem::take(agents) {
        let readiness =
            probe_agent_runtime_readiness(config, &name, Duration::from_millis(250)).await;
        match readiness {
            AgentRuntimeReadiness::Ready => retained.push((name, agent)),
            AgentRuntimeReadiness::EndpointTimedOut => {
                server.observability.metrics.inc_runtime_probe_timeout();
                unavailable.push(name);
            }
            AgentRuntimeReadiness::MissingSecret | AgentRuntimeReadiness::EndpointUnavailable => {
                unavailable.push(name);
            }
        }
    }

    *agents = retained;
    unavailable
}

pub(crate) fn has_flow_phase(config: &crate::config::AppConfig, phase: &str) -> bool {
    config
        .flow
        .phases
        .iter()
        .any(|candidate| candidate == phase)
        || config.phases.contains_key(phase)
}

pub(crate) fn infer_adaptive_phase(
    config: &crate::config::AppConfig,
    mode: &str,
    messages: &[Message],
) -> Option<String> {
    let task = extract_task_description(messages);
    if task.trim().is_empty() {
        return None;
    }

    let characteristics = TaskRouter::analyze_task(&task);
    let mut candidate = match characteristics.task_type {
        TaskType::ArchitectureDesign => Some("planning"),
        TaskType::CodeReview => {
            if mode.eq_ignore_ascii_case("review") {
                Some("review")
            } else {
                Some("coding")
            }
        }
        TaskType::Documentation => Some("delivery"),
        TaskType::BugFix
        | TaskType::FeatureImplementation
        | TaskType::Refactoring
        | TaskType::TestImplementation
        | TaskType::PerformanceOptimization
        | TaskType::Unknown => Some("coding"),
    };

    if mode.eq_ignore_ascii_case("review") && has_flow_phase(config, "review") {
        candidate = Some("review");
    }

    if characteristics.complexity >= 4 && has_flow_phase(config, "planning") {
        candidate = Some("planning");
    }

    candidate
        .filter(|phase| has_flow_phase(config, phase))
        .map(str::to_string)
}

pub(crate) fn controller_recommended_phase(
    server: &AcpServer,
    config: &crate::config::AppConfig,
    mode: &str,
) -> Option<String> {
    let candidates = config.flow.phases.clone();
    let recommended = server
        .resilience
        .online_controller
        .lock()
        .unwrap_or_else(|poisoned| {
            warn!("controller_recommended_phase: online_controller poisoned, recovering");
            poisoned.into_inner()
        })
        .recommend_phase(&candidates)?;

    if recommended == "review" && !mode.eq_ignore_ascii_case("review") {
        return None;
    }
    if has_flow_phase(config, &recommended) {
        Some(recommended)
    } else {
        None
    }
}

// ============================================================================
// Section: Chat Request Lifecycle
// ============================================================================

/// Process chat request (orchestrator — delegates to extracted phases)
///
/// This function is now a thin orchestrator that splits the request lifecycle
/// into four phases, each in `chat_phases.rs`:
///   1. `observe_phase` — input validation, multimodal detection, prompt injection check,
///      context gathering, memory recall, capability sensing
///   2. `think_phase`   — model resolution, agent selection, routing, planning,
///      capability analysis, risk assessment, metacognitive evaluation
///   3. `act_phase`     — LLM calls, tool execution, autonomy loop, fallback, vote,
///      cache operations, scheduler
///   4. `reflect_phase` — response assembly, error handling, knowledge persistence,
///      metacognitive updates, threshold learning, capability bus feedback,
///      BrainLoop reflection
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) async fn process_chat_request(
    server: &AcpServer,
    params: &mut ChatParams,
    stream_observer: Option<StreamObserver>,
    trace: &RequestTraceContext,
    span: Option<&OtelContext>,
    ctx: Option<ChatRequestContext>,
) -> Result<serde_json::Value> {
    use crate::acp::r#impl::chat::pipeline::ChatPipeline;
    use crate::orchestration::plan_output::extract_plan_from_response;

    let outcome =
        ChatPipeline::run(server, params, stream_observer.clone(), trace, span, ctx).await?;
    let mut result = outcome.result;

    // ── Plan output extraction ──────────────────────────────────────
    // When in Plan mode, extract the structured plan from the chat response
    // so it can be used for execution handoff (Edit/SafeGuard/FullAuto).
    let is_plan_mode = params.mode.eq_ignore_ascii_case("plan");
    if is_plan_mode {
        let response_text = result
            .get("response")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !response_text.is_empty() {
            let plan_output = extract_plan_from_response(response_text);
            if let Some(obj) = result.as_object_mut() {
                obj.insert(
                    "plan_output".to_string(),
                    serde_json::to_value(&plan_output).unwrap_or_default(),
                );
            }
        }
    }

    // Send final result SSE event so the GUI receives the response.
    // This is necessary because the spawned task only handles errors;
    // the Ok(result) is discarded. The event is sent here, after all
    // phases succeed, so it won't be overwritten by error handlers.
    //
    // When both response and agent are empty (which can happen when the
    // Mode Runtime in reflect_phase is the primary execution engine and
    // the ACP act_phase autonomy round was skipped or returned empty),
    // send an error event instead of silently skipping — otherwise the
    // GUI sees a stream that ends without any "result" or "done" event
    // and displays the misleading "empty response" message.
    if let Some(ref observer) = stream_observer {
        let response_text = result
            .get("response")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let agent = result.get("agent").and_then(|v| v.as_str()).unwrap_or("");
        if response_text.is_empty() && agent.is_empty() {
            tracing::warn!(
                target: "chat_stream",
                "process_chat_request: result has empty response AND empty agent — sending error event"
            );
            observer.send_sse(crate::acp::r#impl::chat::streaming::StreamFrame {
                event: "error",
                payload: serde_json::json!({
                    "error": "The chat completed but produced no response. This can happen if no agents are available, API keys are misconfigured, or the backend is overloaded.",
                    "message": "error.chat.no_response_from_pipeline",
                }),
                status: None,
            });
        } else {
            let plan_output_val = result.get("plan_output");
            let mut payload = serde_json::json!({
                "response": response_text,
                "agent": agent,
                "done": true,
            });
            if let Some(po) = plan_output_val {
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert("plan_output".to_string(), po.clone());
                }
            }
            observer.send_sse(crate::acp::r#impl::chat::streaming::StreamFrame {
                event: "result",
                payload,
                status: None,
            });
        }
    }

    Ok(result)
}

// ═════════════════════════════════════════════════════════════════════
// Extracted sub-functions for process_chat_request (BLUE48 Step 1)
// ═════════════════════════════════════════════════════════════════════

/// Result of the phase resolution step.
pub(crate) struct PhaseResolution {
    pub phase: crate::orchestration::flow::ResolvedPhase,
    pub phase_name: String,
    pub phase_origin: String,
    pub resolved: crate::orchestration::flow::ResolvedRouting,
    pub schema_warnings: Vec<String>,
    pub schema_error: Option<String>,
    pub routing_provenance: Vec<String>,
    pub reputation_scores: HashMap<String, f64>,
}

// Resolve the request phase from parameters, adaptive inference, and controller recommendation.
//
// Determines the phase to use for this chat request by considering (in order):
// 1. The explicitly requested phase in `params.phase`
// 2. The controller-recommended phase (based on live outcome data)
// 3. The adaptively inferred phase (based on message content)
// 4. The flow default
//
// Also performs schema registry validation and initial agent reordering.
// ============================================================================
// Section: Request Phase Resolution
// ============================================================================

pub(crate) async fn resolve_request_phase(
    server: &AcpServer,
    params: &ChatParams,
    flow: &FlowManager,
    registry: &crate::agent::AgentRegistry,
) -> Result<PhaseResolution> {
    let app_config = flow.config();

    let requested_phase = params.phase.as_ref();
    let adaptive_phase = if requested_phase.is_none() {
        infer_adaptive_phase(app_config.as_ref(), &params.mode, &params.messages)
    } else {
        None
    };
    let controller_phase = if requested_phase.is_none() {
        controller_recommended_phase(server, app_config.as_ref(), &params.mode)
    } else {
        None
    };

    let has_requested_phase = requested_phase.is_some();
    let has_controller_phase = controller_phase.is_some();
    let has_adaptive_phase = adaptive_phase.is_some();

    let chosen_phase = requested_phase
        .cloned()
        .or(controller_phase)
        .or(adaptive_phase);

    let mut resolved = match flow.resolve(chosen_phase.clone(), registry) {
        Ok(r) => r,
        Err(_) => {
            warn!(
                "chat: phase '{:?}' not found in flow config, falling back to default",
                chosen_phase
            );
            flow.resolve(None, registry)?
        }
    };
    let original_count = resolved.agents.len();
    let unavailable_agents =
        filter_runtime_ready_agents(server, app_config.as_ref(), &mut resolved.agents).await;
    if resolved.agents.is_empty() {
        resolved = flow.resolve(chosen_phase.clone(), registry)?;
    } else if resolved.agents.len() < original_count {
        warn!(
            phase = %resolved.phase.phase_name,
            retained = resolved.agents.len(),
            original = original_count,
            unavailable = %unavailable_agents.join(","),
            "filtered runtime-unavailable agents before chat execution"
        );
    }

    let phase_origin = if has_requested_phase {
        "requested"
    } else if has_controller_phase {
        "controller"
    } else if has_adaptive_phase {
        "adaptive"
    } else {
        "default"
    }
    .to_string();

    let phase = resolved.phase.clone();
    let phase_name = phase.phase_name.clone();
    reorder_chat_agents_by_runtime_score(server, &phase_name, &mut resolved.agents);

    let mut routing_provenance: Vec<String> = vec!["runtime_score_rerank_applied".to_string()];
    let reputation_scores = collect_reputation_scores(server, &resolved.agents);

    let online_scores = resolved
        .agents
        .iter()
        .map(|(name, _)| {
            (
                name.clone(),
                crate::acp::helpers::agent_router::task_agent_success_rate(&phase_name, name),
            )
        })
        .collect::<Vec<_>>();

    let selector = AgentSelector::default();
    if let Some(selection) = selector.reorder_agents_by_selection(
        &mut resolved.agents,
        None,
        &reputation_scores,
        &online_scores,
        &phase_name,
    ) {
        if !reputation_scores.is_empty() {
            record_reputation_routing_applied();
        }
        routing_provenance.push(format!("agent_selector_winner:{}", selection.winner));
        routing_provenance.push(format!(
            "agent_selector_reason:{}",
            selection.selection_reason
        ));
    }

    // ── SchemaRegistry task envelope validation (activated, formerly F-GAP-07) ─
    let mut schema_warnings: Vec<String> = Vec::new();
    let mut schema_error: Option<String> = None;
    let sr_guard = server
        .registries
        .schema_registry
        .lock()
        .unwrap_or_else(|poisoned| {
            warn!("resolve_request_phase: schema_registry poisoned, recovering");
            poisoned.into_inner()
        });
    for (role_name, _agent) in &resolved.agents {
        if let Some(schema) = sr_guard.get(role_name) {
            let input_val = serde_json::json!({
                "mode": params.mode,
                "phase": phase_name,
                "message_count": params.messages.len(),
            });
            match schema.validate_input(&input_val) {
                Ok(warnings) => {
                    for w in warnings {
                        schema_warnings.push(format!("[{}] {}", role_name, w));
                    }
                }
                Err(e) => {
                    schema_error = Some(format!("[{}] {}", role_name, e));
                    warn!("schema validation error for {}: {}", role_name, e);
                }
            }
        }
    }
    drop(sr_guard);

    Ok(PhaseResolution {
        phase,
        phase_name,
        phase_origin,
        resolved,
        schema_warnings,
        schema_error,
        routing_provenance,
        reputation_scores,
    })
}

/// Evaluate pre-route policies (HarnessBus, token gate, tenant budget).
/// Returns an error if any policy denies the request.
pub(crate) async fn evaluate_pre_route_policies(
    server: &AcpServer,
    params: &ChatParams,
    tenant_id: &str,
) -> Result<()> {
    crate::acp::helpers::pre_route_policy::evaluate_pre_route_policies(server, params, tenant_id)
        .await
}

pub(crate) use agent_selection::{select_and_score_agents, AutonomyOutcome};
pub(crate) use fallback::{execute_fallback_agents, FallbackExecutionResult};

// Execute the multi-round autonomy loop for full_auto / execution-like requests.
//
// Runs think → act → observe → replan cycles with agent rerouting support.
// Returns whether the loop was executed, and if successful, the response.
// ============================================================================
// Section: Autonomy Loop & Fallback
// ============================================================================

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_autonomy_round(
    _server: &AcpServer,
    _params: &ChatParams,
    phase: &crate::orchestration::flow::ResolvedPhase,
    _phase_name: &str,
    resolved: &crate::orchestration::flow::ResolvedRouting,
    agent_messages: &[Message],
    base_agent_options: &HashMap<String, Value>,
    cache_hit: bool,
    progress_sse_tx: Option<mpsc::UnboundedSender<StreamFrame>>,
) -> AutonomyOutcome {
    if cache_hit {
        return AutonomyOutcome {
            autonomy_loop_executed: false,
            selected_agent: String::new(),
            response_text: String::new(),
            agent_attempts: Vec::new(),
        };
    }

    let reroute_enabled = option_bool(base_agent_options, "enable_agent_reroute", true);
    let autonomy_candidates = resolved.agents.clone();
    let mut agent_attempts: Vec<Value> = Vec::new();
    let mut response_text = String::new();
    let mut selected_agent = String::new();
    let mut autonomy_loop_executed = false;

    for (idx, (agent_name, agent)) in autonomy_candidates.into_iter().enumerate() {
        if idx > 0 && !reroute_enabled {
            break;
        }
        if idx > 0 {
            let switch_reason = if idx == 1 { "failure" } else { "reputation" };
            record_agent_switch(switch_reason);
        }

        let attempt_started = std::time::Instant::now();

        // Build per-agent options — strip model override for fallback agents
        // (idx > 0) so the user's model selection for the primary agent doesn't
        // get passed to a different provider that doesn't support it.
        let mut agent_opts = base_agent_options.clone();
        if idx > 0 {
            agent_opts.remove("model");
        }
        // Some agents (notably Copilot/GitHub) don't support custom tools.
        // For those, strip tools from options so the agent responds naturally
        // without being confused by unfamiliar function definitions.
        if agent_name.to_lowercase().contains("copilot") {
            agent_opts.remove("tools");
            agent_opts.remove("tool_choice");
        }
        let agent_opts = Some(agent_opts);
        let autonomy_tool_registry = if agent_name.to_lowercase().contains("copilot") {
            None // Copilot has its own native tools, no Go-On tool registry needed
        } else {
            Some(std::sync::Arc::new(
                crate::orchestration::tool::ToolRegistry::new(),
            ))
        };

        let acp_params = crate::acp::helpers::autonomy_loop_adapter::AcpAutonomyLoopParams {
            agent,
            tool_registry: autonomy_tool_registry,
            messages: agent_messages.to_vec(),
            principles: phase.principles.clone(),
            options: agent_opts,
            timeout_duration: request_timeout(phase.options.as_ref()),
            stream_tx: None,
            progress_sse_tx: progress_sse_tx.clone(),
        };
        let result =
            crate::acp::helpers::autonomy_loop_adapter::run_acp_autonomy_loop(acp_params).await;

        match result {
            Ok(loop_result) => {
                let stop_reason = loop_result.report.stop_reason.clone();
                let produced_response = !loop_result.response.trim().is_empty();
                let autonomy_contract =
                    crate::acp::helpers::autonomy_loop::contract_snapshot(&loop_result.report);
                agent_attempts.push(json!({
                    "agent": agent_name,
                    "ok": produced_response,
                    "autonomy_loop": true,
                    "autonomy_contract": autonomy_contract,
                    "total_rounds": loop_result.report.total_rounds,
                    "total_tools": loop_result.report.total_tools,
                    "corrective_actions_applied_total": loop_result.report.corrective_actions_applied_total,
                    "corrective_action_effectiveness_ratio": loop_result.report.corrective_action_effectiveness_ratio,
                    "stop_reason": stop_reason,
                    "candidate_index": idx,
                    "candidate_count": resolved.agents.len(),
                    "duration_ms": attempt_started.elapsed().as_millis() as u64,
                }));
                record_autonomy_loop_stop_reason(&loop_result.report.stop_reason);
                if produced_response {
                    autonomy_loop_executed = true;
                    response_text = loop_result.response;
                    selected_agent = agent_name;
                    break;
                }
                if !reroute_enabled {
                    break;
                }
            }
            Err(e) => {
                warn!("autonomy loop failed for '{}': {}", agent_name, e);
                agent_attempts.push(json!({
                    "agent": agent_name,
                    "ok": false,
                    "autonomy_loop": true,
                    "error": e.to_string(),
                    "candidate_index": idx,
                    "candidate_count": resolved.agents.len(),
                    "duration_ms": attempt_started.elapsed().as_millis() as u64,
                }));
                if !reroute_enabled {
                    break;
                }
            }
        }
    }

    AutonomyOutcome {
        autonomy_loop_executed,
        selected_agent,
        response_text,
        agent_attempts,
    }
}

// Apply the review gate logic and assemble the final chat response.
//
// Computes the final response value including memory policy execution,
// task graph checkpoint, role routing, verification, and the final
// response assembly via `response_finalizer::finalize_chat_response`.
// Also handles fire-and-forget background tasks (skill creation, workflow generation).
// ============================================================================
// Section: Review Gate & Response Assembly
// ============================================================================

#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply_review_gate_assemble(
    server: &AcpServer,
    params: &ChatParams,
    trace: &RequestTraceContext,
    phase_name: &str,
    phase_origin: &str,
    selected_agent: &str,
    selected_model_name: &Option<String>,
    response_text: &str,
    reasoning_text: &str,
    tenant_id: &str,
    started: std::time::Instant,
    conversation_id: &str,
    branch_id: &str,
    schema_warnings: Vec<String>,
    schema_error: Option<String>,
    layered_prompt_segments: usize,
    tool_execution_results: &[Value],
    sched_task_id: &str,
    candidate_agents: &[String],
    routing_provenance: &[String],
    reputation_scores: &HashMap<String, f64>,
    selected_agent_reputation: Option<f64>,
    council_decision: &Option<Value>,
    vote_winner: &Option<String>,
    fallback_reason: &Option<String>,
    cache_hit: bool,
    cache_bypassed_for_execution: bool,
    capability_info: crate::acp::helpers::response_assembler::CapabilityRoutingInfo,
    reviews: Vec<Value>,
    agent_attempts: Vec<Value>,
    risk_decision: Value,
    quota_failed_agents: Vec<String>,
    vector_context: VectorContext,
    knowledge: Value,
    distillation: Value,
    checkpoint: crate::acp::ConversationCheckpoint,
    metacognitive_loop: Value,
) -> Result<serde_json::Value> {
    let switched_from_quota_limit = !quota_failed_agents.is_empty() && !selected_agent.is_empty();
    if let Some(primary_candidate) = candidate_agents.first() {
        if !selected_agent.is_empty() && selected_agent != *primary_candidate {
            if switched_from_quota_limit {
                record_agent_switch("failure");
            } else {
                record_agent_switch("reputation");
            }
        }
    }
    let agent_switch_notice = if switched_from_quota_limit {
        Some(json!({
            "type": "quota_fallback",
            "message": tf("status.chat.quota_fallback_notice", &[
                ("agents", &quota_failed_agents.join(", ")),
                ("agent", selected_agent)
            ]),
            "quota_failed_agents": quota_failed_agents,
            "active_agent": selected_agent.to_string(),
            "available_agents": candidate_agents.to_vec(),
            "auto_recover": t("status.chat.auto_recover_notice")
        }))
    } else {
        None
    };

    let token_economy = estimate_token_economy(&params.messages, response_text);

    // Cache task description once for all full_auto sub-steps
    let task_description = extract_task_description(&params.messages);

    // Memory policy execution integration
    // F-GAP-64: Use structured ModeKind::FullAuto check instead of raw string comparison.
    // Same pattern repeated at L~2260, L~2277, L~2284.
    let memory_promotion_result = if ModeKind::from(params.mode.as_str()) == ModeKind::FullAuto {
        let memory_entry = MemoryEntry {
            id: format!("task-{}-{}", conversation_id, started.elapsed().as_millis()),
            class: MemoryClass::Observation,
            content: format!(
                "Task completed: {} with response: {}",
                task_description, response_text
            ),
            timestamp: format!(
                "{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            ),
            usefulness: 0.8,
            staleness: 0,
            user_id: None,
        };

        let mut memory_store = MemoryStore::new(MemoryPolicy::default());
        memory_store.store(memory_entry);
        let promotion_report = memory_store.promote();

        Some(json!({
            "memory_promotion": {
                "promoted_count": promotion_report.promoted_count,
                "promotion_map": promotion_report.promotion_map,
                "task_recorded": true
            }
        }))
    } else {
        None
    };

    // Task graph execution engine integration
    let (task_graph_result, _saved_graph_id, _saved_checkpoint_id) =
        if ModeKind::from(params.mode.as_str()) == ModeKind::FullAuto {
            build_task_graph_checkpoint(
                server,
                conversation_id,
                &task_description,
                &params.mode,
                phase_name,
                response_text,
                tool_execution_results,
                memory_promotion_result.as_ref(),
                started.elapsed().as_millis() as u64,
            )
        } else {
            (None, None, None)
        };

    // Role-based agent routing integration
    let role_routing_result = if ModeKind::from(params.mode.as_str()) == ModeKind::FullAuto {
        Some(build_role_routing(&task_description))
    } else {
        None
    };

    // Enhanced verification system integration
    let verification_result = if ModeKind::from(params.mode.as_str()) == ModeKind::FullAuto {
        Some(run_enhanced_verification(response_text))
    } else {
        None
    };

    let result = crate::acp::helpers::response_finalizer::finalize_chat_response(
        server,
        trace,
        &params.mode,
        phase_name,
        selected_agent,
        selected_model_name,
        response_text,
        reasoning_text,
        tenant_id,
        started,
        build_chat_response(ChatResponseContext {
            mode: params.mode.clone(),
            conversation_id: conversation_id.to_string(),
            branch_id: branch_id.to_string(),
            phase_name: phase_name.to_string(),
            phase_origin: phase_origin.to_string(),
            selected_agent: selected_agent.to_string(),
            selected_model_name: selected_model_name.clone(),
            response_text: response_text.to_string(),
            checkpoint: json!(checkpoint),
            metacognitive_loop,
            token_economy,
            vector_hits: vector_context.hits,
            summary_used: vector_context.summary.is_some(),
            knowledge,
            distillation,
            reviews,
            agent_attempts,
            risk_decision,
            agent_switch_notice,
            tool_execution_results: tool_execution_results.to_vec(),
            memory_promotion_result,
            task_graph_result,
            role_routing_result,
            verification_result,
            capability_info,
            routing_diagnostics: json!({
                "routing_provenance": routing_provenance,
                "candidate_reputation_scores": reputation_scores,
                "selected_agent_reputation": selected_agent_reputation,
                "council_decision": council_decision,
                "vote_winner": vote_winner,
                "fallback_reason": fallback_reason,
            }),
            cache_hit,
            cache_bypassed: cache_bypassed_for_execution,
            started,
        }),
        conversation_id,
        schema_warnings,
        schema_error,
        layered_prompt_segments,
        tool_execution_results,
        sched_task_id,
        candidate_agents,
        routing_provenance,
        reputation_scores,
        selected_agent_reputation,
        council_decision,
        vote_winner,
        fallback_reason,
        cache_hit,
        cache_bypassed_for_execution,
        params
            .messages
            .first()
            .map(|m| m.content.as_str())
            .unwrap_or(""),
    );

    Ok(result)
}

/// Maximum checkpoint count before a conversation is considered "long".
const MAX_CHECKPOINTS_BEFORE_ESCALATION: usize = 20;

/// Check conversation history for escalation requirements.
///
/// Evaluates whether a conversation's history warrants escalation:
/// - Has the conversation had previous escalations?
/// - Is the conversation unusually long (many checkpoints)?
/// - Has the conversation had repeated failures or errors?
async fn check_conversation_history_escalation(
    server: &AcpServer,
    conversation_id: &str,
) -> Result<bool> {
    let conversation_state = server.session.conversation_state.lock().await;

    // Filter checkpoints scoped to this conversation only
    let conversation_checkpoints: Vec<_> = conversation_state
        .checkpoints
        .iter()
        .filter(|cp| cp.conversation_id == conversation_id)
        .collect();

    if conversation_checkpoints.is_empty() {
        return Ok(false); // New conversation, no history to check
    }

    // ── Check if this conversation has had previous escalations ──
    // Now correctly scoped to this conversation_id (was a bug: was scanning ALL checkpoints)
    let has_previous_escalations = conversation_checkpoints
        .iter()
        .any(|cp| cp.note.as_deref().unwrap_or("") == "escalation");

    // ── Check if conversation is unusually long ──────────────────
    let is_long_conversation = conversation_checkpoints.len() > MAX_CHECKPOINTS_BEFORE_ESCALATION;

    // ── Check for repeated failures in recent checkpoints ────────
    let recent_failures = conversation_checkpoints
        .iter()
        .rev()
        .take(5)
        .filter(|cp| {
            cp.note
                .as_deref()
                .is_some_and(|n| n.contains("fail") || n.contains("error") || n.contains("reject"))
        })
        .count();
    let has_repeated_failures = recent_failures >= 3;

    Ok(has_previous_escalations || is_long_conversation || has_repeated_failures)
}

/// Check phase-specific escalation rules.
///
/// Evaluates whether the current phase and its options warrant escalation:
/// - "full_auto": Escalates only when review agents are configured or complexity is high.
///   Full auto is designed to REDUCE human oversight, so always escalating is wrong.
/// - "safeguard": Escalates based on explicit config or governance policy.
/// - Other phases: Escalate only in edge cases (explicit flag or harness policy).
async fn check_phase_escalation_rules(
    server: &AcpServer,
    phase: &str,
    options: Option<&PhaseOptions>,
) -> Result<bool> {
    // ── Governance policy evaluation ────────────────────────────────
    // Let the HarnessBus policy engine weigh in first
    if let Some(ref harness) = server.governance_deps.harness_bus {
        let verdict = harness.validate_action(
            &format!("phase.{}.execute", phase),
            &serde_json::json!({
                "phase": phase,
                "options": options.as_ref().map(|o| serde_json::json!({
                    "autopilot_complexity": o.autopilot_complexity,
                    "full_auto_review_agents": o.full_auto_review_agents,
                })),
            }),
        );
        if !verdict.is_allowed() {
            return Ok(true);
        }
    }

    match phase {
        "full_auto" => {
            // FullAuto only escalates when:
            // 1. Review agents are explicitly configured (someone wants oversight)
            // 2. Autopilot complexity is high
            // 3. Explicit require_escalation flag is set
            if let Some(opts) = options {
                let has_review_agents = opts
                    .full_auto_review_agents
                    .as_ref()
                    .map(|a| !a.is_empty())
                    .unwrap_or(false);
                let is_complex = opts
                    .autopilot_complexity
                    .as_deref()
                    .map(|c| matches!(c, "high" | "complex" | "critical"))
                    .unwrap_or(false);
                let explicit_flag = opts
                    .extra
                    .get("require_escalation")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Ok(has_review_agents || is_complex || explicit_flag)
            } else {
                Ok(false)
            }
        }
        "safeguard" => {
            // Safeguard escalates when:
            // 1. Complexity is critical
            // 2. Review agents are configured
            // 3. Explicit flag is set
            if let Some(opts) = options {
                let is_critical = opts
                    .autopilot_complexity
                    .as_deref()
                    .map(|c| c == "critical")
                    .unwrap_or(false);
                let has_review_agents = opts
                    .full_auto_review_agents
                    .as_ref()
                    .map(|a| !a.is_empty())
                    .unwrap_or(false);
                let explicit_flag = opts
                    .extra
                    .get("require_escalation")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Ok(is_critical || has_review_agents || explicit_flag)
            } else {
                Ok(false)
            }
        }
        _ => {
            // Default phases: only escalate if governance policy requires it
            // (already checked via HarnessBus above)
            Ok(false)
        }
    }
}

// Thread-local cache for task description to avoid recomputing it
// 5+ times per request (O(N²) → O(1) optimization).
thread_local! {
    static TASK_DESCRIPTION_CACHE: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// Clear the task description cache (call at the start of each request).
pub(crate) fn clear_task_description_cache() {
    TASK_DESCRIPTION_CACHE.with(|cache| {
        *cache.borrow_mut() = None;
    });
}

/// Extract task description from messages, caching the result per request.
///
/// Uses a thread-local cache so that calling this 5+ times in a single
/// request (e.g. from infer_adaptive_phase, persist_chat_knowledge,
/// persist_session_distillation) costs only one iteration over messages.
pub(crate) fn extract_task_description(messages: &[Message]) -> String {
    // Return cached value if available (computed earlier in this request)
    if let Some(cached) = TASK_DESCRIPTION_CACHE.with(|cache| cache.borrow().clone()) {
        return cached;
    }

    // Compute and cache
    let result = messages
        .iter()
        .rev()
        .find(|message| message.role.eq_ignore_ascii_case("user"))
        .map(|message| message.content.clone())
        .or_else(|| messages.last().map(|message| message.content.clone()))
        .unwrap_or_default();

    TASK_DESCRIPTION_CACHE.with(|cache| {
        *cache.borrow_mut() = Some(result.clone());
    });
    result
}

/// Get routing handles
pub(crate) fn routing_handles(
    server: &AcpServer,
) -> Result<(Arc<FlowManager>, Arc<crate::agent::AgentRegistry>)> {
    crate::acp::r#impl::runtime::routing_handles(server)
}

pub(crate) fn reorder_chat_agents_by_runtime_score(
    server: &AcpServer,
    phase_name: &str,
    agents: &mut Vec<(String, Arc<dyn crate::agent::Agent>)>,
) {
    if agents.len() <= 1 {
        return;
    }

    let names = agents
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    let ranked = server
        .resilience
        .online_controller
        .lock()
        .map(|state| state.rank_agent_names_for_phase(phase_name, &names))
        .unwrap_or_default();
    if ranked.is_empty() {
        return;
    }

    let mut score_map = std::collections::HashMap::new();
    for (name, score) in ranked {
        score_map.insert(name, score);
    }

    agents.sort_by(|a, b| {
        let score_a = score_map.get(&a.0).copied().unwrap_or(0.0);
        let score_b = score_map.get(&b.0).copied().unwrap_or(0.0);
        score_b
            .partial_cmp(&score_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Analyze a completed chat conversation and auto-create skills for
/// repetitive task patterns that would benefit from being a reusable skill.
pub(crate) async fn auto_create_skills_from_conversation(
    server: &AcpServer,
    chat_params: &ChatParams,
    response_text: &str,
) -> Result<Vec<String>> {
    let mut created_skills = Vec::new();

    // Only attempt skill creation if the skill-creator skill is registered
    let has_skill_creator = server
        .orchestration_deps
        .skill_registry
        .read()
        .ok()
        .map(|registry| registry.get("skill-creator").is_some())
        .unwrap_or(false);

    if !has_skill_creator {
        return Ok(created_skills);
    }

    // Analyze the last user message for skill creation patterns
    let last_user_msg = chat_params
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .unwrap_or("");

    // Analyze all user messages for repeated task patterns
    let all_user_msgs: Vec<&str> = chat_params
        .messages
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .collect();

    // Check if the user's message contains a skill creation request
    // or if the AI response suggests creating a skill
    let user_lower = last_user_msg.to_lowercase();
    let response_lower = response_text.to_lowercase();

    let has_creation_intent = user_lower.contains("create a skill")
        || user_lower.contains("make a skill")
        || user_lower.contains("new skill")
        || user_lower.contains("save as skill")
        || user_lower.contains("create skill")
        || user_lower.contains("automate this")
        || user_lower.contains("turn this into")
        || user_lower.contains("skill for");

    let has_response_hint = response_lower.contains("i'll create a skill")
        || response_lower.contains("i have created a skill")
        || response_lower.contains("skill has been created")
        || response_lower.contains("created the skill");

    // P3: Detect repeated task patterns across conversation history.
    // If the same type of task (based on keyword overlap) appears 3+ times,
    // proactively propose creating a skill for it.
    let repeated_pattern = detect_repeated_task_pattern(&all_user_msgs);

    if has_creation_intent || has_response_hint || repeated_pattern {
        // Generate skill name and description from the current conversation.
        let skill_name = generate_skill_name_from_conversation(last_user_msg, response_text);
        let skill_description = generate_skill_description(last_user_msg, response_text);

        if !skill_name.is_empty() && !skill_description.is_empty() {
            // Check if skill already exists
            let exists = server
                .orchestration_deps
                .skill_registry
                .read()
                .ok()
                .map(|registry| registry.get(&skill_name).is_some())
                .unwrap_or(false);

            if !exists {
                let prompt = format!(
                    "You are an AI assistant specialized in: {}\n\nBased on the user's request, execute the following task:\n{}",
                    skill_description, last_user_msg
                );

                let result = {
                    let mut registry = server
                        .orchestration_deps
                        .skill_registry
                        .write()
                        .unwrap_or_else(|poisoned| {
                            tracing::warn!("lock poisoned, recovering");
                            poisoned.into_inner()
                        });
                    registry
                        .create_skill_from_prompt(
                            &skill_name,
                            &skill_description,
                            &prompt,
                            std::collections::HashMap::new(),
                        )
                        .ok()
                };

                if result.is_some() {
                    info!("Auto-created skill '{}' from conversation", skill_name);
                    created_skills.push(skill_name);
                }
            }
        }
    }

    Ok(created_skills)
}

/// Analyze a conversation and auto-generate a reusable workflow definition
/// when the system detects a multi-step task pattern that could be
/// standardized as a workflow.
pub(crate) async fn auto_generate_workflow_from_conversation(
    server: &AcpServer,
    chat_params: &ChatParams,
    response_text: &str,
) -> Result<Option<Value>> {
    // Only proceed if there are enough messages to detect a pattern
    let user_messages: Vec<&str> = chat_params
        .messages
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .collect();

    if user_messages.len() < 2 {
        return Ok(None);
    }

    let last_msg = user_messages.last().unwrap_or(&"").to_lowercase();
    let response_lower = response_text.to_lowercase();

    // Check for workflow generation intent
    let has_workflow_intent = last_msg.contains("create a workflow")
        || last_msg.contains("make a workflow")
        || last_msg.contains("new workflow")
        || last_msg.contains("save as workflow")
        || last_msg.contains("workflow for")
        || last_msg.contains("automate this process")
        || last_msg.contains("multi-step")
        || last_msg.contains("create workflow")
        || response_lower.contains("i'll create a workflow")
        || response_lower.contains("workflow has been created");

    if !has_workflow_intent {
        return Ok(None);
    }

    // Build a workflow from the conversation
    let task = user_messages.join(" | ");
    let workflow_name = generate_workflow_name(&last_msg, &task);

    // Create a simple workflow plan using the existing task plan builder
    let plan = build_task_plan(&task);
    let workflow = build_workflow_generated_artifact(&plan);

    // Persist the workflow artifact for traceability
    let ledger = server
        .persistence
        .artifact_ledger
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| ArtifactLedger::new(server.config_path.as_deref().map(Path::new)));
    let _ = persist_workflow_generated(&ledger, &workflow);

    info!(
        "Auto-generated workflow '{}' from conversation ({} nodes, {} edges)",
        workflow_name,
        workflow.nodes.len(),
        workflow.edges.len()
    );

    Ok(Some(json!({
        "name": workflow_name,
        "workflow": workflow,
        "plan": plan,
    })))
}

/// Generate a workflow name from conversation content
fn generate_workflow_name(last_msg: &str, _full_task: &str) -> String {
    // Try to extract a name from the message
    let lower = last_msg.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();

    // Look for patterns like "workflow for X" or "X workflow"
    for (i, w) in words.iter().enumerate() {
        if *w == "for" && i + 1 < words.len() {
            let name_candidate = words[i + 1];
            let sanitized: String = name_candidate
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if !sanitized.is_empty() {
                return format!("{}-workflow", sanitized);
            }
        }
    }

    // Fall back to timestamp-based name
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("auto-workflow-{}", ts)
}

/// Generate a skill name from conversation content
fn generate_skill_name_from_conversation(user_msg: &str, ai_response: &str) -> String {
    // Try to extract a name from the AI response (it might mention the skill name)
    for line in ai_response.lines() {
        let lower = line.to_lowercase();
        if lower.contains("skill")
            && (lower.contains("called") || lower.contains("named") || lower.contains("`"))
        {
            if let Some(name_start) = lower.find('`') {
                if let Some(name_end) = lower[name_start + 1..].find('`') {
                    let name = &lower[name_start + 1..name_start + 1 + name_end];
                    if !name.is_empty() && name.len() <= 64 {
                        return name.to_string();
                    }
                }
            }
        }
    }

    // Fall back to using the first few words of the user message
    let words: Vec<&str> = user_msg.split_whitespace().collect();
    let base = if words.len() >= 3 {
        words[..3].join("-")
    } else {
        words.join("-")
    };

    let sanitized: String = base
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .collect();

    if sanitized.len() > 50 {
        format!("{}-skill", &sanitized[..50])
    } else if sanitized.is_empty() {
        format!(
            "skill-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        )
    } else {
        format!("{}-skill", sanitized)
    }
}

/// Generate a skill description from conversation content
fn generate_skill_description(user_msg: &str, _ai_response: &str) -> String {
    let truncated: String = user_msg.chars().take(120).collect();
    if truncated.len() < user_msg.len() {
        format!("{}...", truncated)
    } else {
        truncated.to_string()
    }
}

// Test suite extracted to separate file for code organization.
#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
