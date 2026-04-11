//! Chat handling implementation functions for ACP server
//!
//! This module contains standalone functions that implement chat handling
//! functionality previously in the `impl AcpServer` block in `impl/chat.rs`.
//! These functions take `AcpServer` as their first parameter to maintain
//! compatibility with the original implementation.

use std::sync::Arc;
use std::time::Instant;
use std::{fs, path::Path};

use anyhow::Result;
use opentelemetry::{Context as OtelContext, KeyValue};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::Duration;
use tracing::{info, warn};

use crate::acp::helpers::conversation::stream_would_exceed_limits;
use crate::acp::helpers::metrics::{stream_chunk_notification, stream_done_notification};
use crate::acp::server::AcpServer;
use crate::agent::Message;
use crate::config::PhaseOptions;
use crate::evaluation::TraceEvent;
use crate::flow::FlowManager;
use crate::i18n::runtime::tf;
use crate::pua::PuaEnforcementPlan;

use crate::reinforcement::{
    persist_knowledge_insight_event, ExecutionDecisionCandidate, KnowledgeBusArtifact,
    KnowledgeInsightArtifact, RequirementContractArtifact, TaskPlanArtifact,
};
use crate::rpc_protocol::{chat_trace_context, child_trace_context, RequestTraceContext};

/// Chat parameters structure
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatParams {
    /// Chat mode (e.g., "ask", "edit", "agent", "safeguard", "full_auto")
    pub mode: String,
    /// Messages to process
    pub messages: Vec<Message>,
    /// Optional conversation ID for continuation
    pub conversation_id: Option<String>,
    /// Optional branch ID for tree-based history
    pub branch_id: Option<String>,
    /// Optional phase to force
    pub phase: Option<String>,
    /// Optional options for phase configuration
    pub options: Option<PhaseOptions>,
    /// Optional requirement contract
    pub requirement_contract: Option<RequirementContractArtifact>,
    /// Optional task plan
    pub plan: Option<TaskPlanArtifact>,
    /// Optional vector search hits
    pub vector_hits: Option<Vec<serde_json::Value>>,
    /// Optional execution decision candidate
    pub execution_decision_candidate: Option<ExecutionDecisionCandidate>,
}

/// Handle chat request
///
/// This function replaces the `AcpServer::handle_chat` method.
pub async fn handle_chat(
    server: &AcpServer,
    id: Option<Value>,
    params: Option<Value>,
    request_span: Option<OtelContext>,
    parent_trace: Option<RequestTraceContext>,
) -> Result<()> {
    let started = Instant::now();
    let pipeline_trace = parent_trace
        .map(|trace| child_trace_context(&trace, "chat.pipeline"))
        .unwrap_or_else(|| chat_trace_context(&id, "chat.pipeline"));

    info!(
        trace_id = %pipeline_trace.trace_id,
        "pipeline entry: chat request received"
    );

    let chat_span = request_span.as_ref().and_then(|parent| {
        server
            .telemetry_runtime
            .lock()
            .ok()
            .and_then(|telemetry_guard| {
                telemetry_guard.start_child_span(
                    parent,
                    "acp.chat",
                    vec![KeyValue::new("phase.entry", "chat")],
                )
            })
    });

    let result = async {
        let lifecycle_snapshot = {
            let lifecycle_guard = server
                .lifecycle_state
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock lifecycle state"))?;
            if lifecycle_guard.is_shutting_down() {
                Some(serde_json::to_value(lifecycle_guard.snapshot())?)
            } else {
                None
            }
        };
        if let Some(snapshot) = lifecycle_snapshot {
            send_error(
                server,
                id,
                -32031,
                "server is shutting down".to_string(),
                Some(snapshot),
            )
            .await?;
            return Ok(());
        }

        let params_value = params.unwrap_or_else(|| json!({}));
        let chat_params: ChatParams = match serde_json::from_value(params_value) {
            Ok(value) => value,
            Err(err) => {
                send_error(
                    server,
                    id,
                    -32602,
                    tf("error.invalid_chat_params", &[("error", &format!("{err}"))]),
                    None,
                )
                .await?;
                return Ok(());
            }
        };

        // Check if should escalate approval strategy
        let should_escalate = should_escalate_approval_strategy(
            server,
            &chat_params.mode,
            &chat_params.messages,
            chat_params.conversation_id.as_deref(),
            chat_params.phase.as_deref(),
            chat_params.options.as_ref(),
        )
        .await?;

        if should_escalate {
            info!(
                trace_id = %pipeline_trace.trace_id,
                "approval strategy escalated due to policy"
            );
            // Handle escalation logic here
            // This will be implemented when we migrate the escalation logic
        }

        // Process chat request
        let result = process_chat_request(
            server,
            &chat_params,
            Some(StreamObserver::jsonrpc(id.clone())),
            &pipeline_trace,
            chat_span.as_ref(),
        )
        .await?;

        // Send success response
        send_result(server, id, json!(result)).await?;

        Ok(())
    }
    .await;

    // Record trace event
    let duration_ms = started.elapsed().as_millis() as u64;
    let status = if result.is_ok() { "success" } else { "error" };

    server.metrics.record_chat_latency(duration_ms as f64);
    record_trace_event(
        server,
        &pipeline_trace,
        "chat.complete",
        status,
        "pipeline",
        json!({}),
        None,
        duration_ms,
    );

    result
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
    // Check various conditions that might require escalation
    // This is a simplified implementation - the actual logic would be more complex

    // 1. Check if mode requires escalation
    let mode_requires_escalation = matches!(mode, "full_auto" | "safeguard");

    // 2. Check message content for sensitive keywords
    let has_sensitive_content = messages.iter().any(|msg| {
        let content = msg.content.to_lowercase();
        content.contains("delete")
            || content.contains("drop")
            || content.contains("remove")
            || content.contains("sensitive")
            || content.contains("confidential")
    });

    // 3. Check conversation history if available
    let history_requires_escalation = if let Some(conv_id) = conversation_id {
        check_conversation_history_escalation(server, conv_id).await?
    } else {
        false
    };

    // 4. Check phase-specific escalation rules
    let phase_requires_escalation = if let Some(phase_name) = phase {
        check_phase_escalation_rules(server, phase_name, options).await?
    } else {
        false
    };

    Ok(mode_requires_escalation
        || has_sensitive_content
        || history_requires_escalation
        || phase_requires_escalation)
}

/// Process chat request
pub(crate) async fn process_chat_request(
    server: &AcpServer,
    params: &ChatParams,
    stream_observer: Option<StreamObserver>,
    trace: &RequestTraceContext,
    span: Option<&OtelContext>,
) -> Result<serde_json::Value> {
    let started = std::time::Instant::now();

    // Get routing handles
    let (flow, registry) = routing_handles(server)?;

    let mut resolved = flow.resolve(params.phase.clone(), registry.as_ref())?;
    let phase = resolved.phase.clone();
    reorder_chat_agents_by_runtime_score(server, &phase.phase_name, &mut resolved.agents);

    // Phase-level rate limiter support migrated from legacy ACP behavior.
    if let Some(options) = phase.options.as_ref() {
        let rpm_limit = options
            .extra
            .get("rate_limit_rpm")
            .and_then(|v| v.as_u64())
            .unwrap_or(u64::MAX);
        let burst = options
            .extra
            .get("rate_limit_burst")
            .and_then(|v| v.as_u64());
        if rpm_limit != u64::MAX {
            let allowed = server
                .phase_rate_limiter
                .lock()
                .map(|guard| guard.allow(&phase.phase_name, rpm_limit, burst))
                .unwrap_or(true);
            if !allowed {
                anyhow::bail!("rate limited");
            }
        }
    }

    // Get or create conversation state
    let conversation_id = params.conversation_id.clone().unwrap_or_else(|| {
        format!(
            "conv_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
    });
    let branch_id = params
        .branch_id
        .clone()
        .unwrap_or_else(|| "main".to_string());

    // Get or create requirement contract
    let _requirement_contract = if let Some(contract) = &params.requirement_contract {
        contract.clone()
    } else {
        // Create default requirement contract
        let task_description = extract_task_description(&params.messages);
        default_requirement_contract(&task_description, "chat")
    };

    // Get or create task plan
    let _plan = if let Some(existing_plan) = &params.plan {
        existing_plan.clone()
    } else {
        // Create default task plan
        TaskPlanArtifact {
            generated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            task: String::new(),
            characteristics: crate::orchestration::task_router::TaskCharacteristics {
                description: String::new(),
                task_type: crate::orchestration::task_router::TaskType::BugFix,
                complexity: 1,
                required_capabilities: Vec::new(),
                involves_multiple_modules: false,
                is_time_critical: false,
                needs_verification: false,
                has_safety_concerns: false,
            },
            routing: crate::orchestration::task_router::RoutingDecision {
                roles: Vec::new(),
                requirements: Vec::new(),
                predicted_success_rate: 1.0,
                estimated_duration_seconds: 1000,
                can_parallelize: Vec::new(),
                risk_factors: Vec::new(),
                recommended_safeguards: Vec::new(),
                pua_enforcement: PuaEnforcementPlan {
                    escalation_level: String::new(),
                    mandatory_roles: Vec::new(),
                    red_lines: Vec::new(),
                    quality_compass: Vec::new(),
                    mandatory_safeguards: Vec::new(),
                    mandatory_evidence: Vec::new(),
                    stage_requirements: Vec::new(),
                },
            },
            decomposition: None,
            planned_subtasks: Vec::new(),
            sub_agent_recommended: false,
            activation_reasons: Vec::new(),
            action_checks_required: Vec::new(),
        }
    };

    let vector_context = load_vector_context(server, &phase.phase_name, phase.options.as_ref(), params)
        .await;
    let agent_messages = merge_context_into_messages(
        &params.messages,
        build_vector_context_message(
            vector_context.summary.as_deref(),
            &vector_context.hits,
            &vector_context.knowledge,
        ),
    );

    let mut selected_agent = String::new();
    let mut response_text = String::new();
    let mut last_err: Option<anyhow::Error> = None;

    for (agent_name, agent) in resolved.agents {
        match run_agent_collecting(
            server,
            StreamNotificationContext {
                stream_observer: stream_observer.clone(),
                agent_name: &agent_name,
                phase_name: &phase.phase_name,
                trace_id: &trace.trace_id,
            },
            agent,
            agent_messages.clone(),
            phase.principles.clone(),
            phase.options.as_ref().and_then(|opts| opts.agent_options()),
            phase
                .options
                .as_ref()
                .and_then(|opts| opts.request_timeout_seconds),
        )
        .await
        {
            Ok(output) => {
                selected_agent = agent_name;
                response_text = output;
                last_err = None;
                break;
            }
            Err(err) => {
                last_err = Some(err);
            }
        }
    }

    if let Some(err) = last_err {
        return Err(err);
    }

    persist_vector_memory(
        server,
        &phase.phase_name,
        phase.options.as_ref(),
        params,
        &response_text,
        &selected_agent,
    )
    .await;

    let mut checkpoint_messages = params.messages.clone();
    checkpoint_messages.push(Message {
        role: "assistant".to_string(),
        content: response_text.clone(),
    });
    let checkpoint = crate::acp::r#impl::request::create_checkpoint_record(
        server,
        &conversation_id,
        &branch_id,
        checkpoint_messages.clone(),
        None,
        None,
    )
    .await;

    let knowledge = persist_chat_knowledge(
        server,
        &conversation_id,
        &branch_id,
        &phase.phase_name,
        &selected_agent,
        params,
        &response_text,
    )
    .await;

    crate::acp::r#impl::request::append_trace_event(TraceEvent {
        timestamp: format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        ),
        event_type: "phase.agent".to_string(),
        task_id: "chat".to_string(),
        phase: phase.phase_name.clone(),
        agent: Some(selected_agent.clone()),
        tool: None,
        status: "ok".to_string(),
        inputs: json!({"attributes": {"agent": selected_agent.clone()}}),
        outputs: None,
        duration_ms: 0,
        error: None,
        pua_stage: None,
    });

    let mut reviews = Vec::new();
    if params.mode.eq_ignore_ascii_case("full_auto") {
        match crate::acp::r#impl::agent::run_dual_review_gate(
            server,
            None,
            &params.messages,
            phase.options.as_ref(),
            span,
            trace,
        )
        .await
        {
            Ok(outcome) => {
                reviews.push(json!({
                    "reviewer": outcome.reviewer,
                    "verdict": if outcome.passed { "APPROVE" } else { "REJECT" },
                    "response": outcome.comments.join("; "),
                    "duration_ms": outcome.duration_ms,
                }));
            }
            Err(err) => {
                reviews.push(json!({
                    "reviewer": "review_gate",
                    "verdict": "REJECT",
                    "response": format!("review gate failed: {err}"),
                }));
            }
        }
    }

    let result = json!({
        "done": true,
        "conversation_id": conversation_id,
        "branch_id": branch_id,
        "mode": params.mode,
        "phase": phase.phase_name,
        "agent": selected_agent,
        "duration_ms": started.elapsed().as_millis() as u64,
        "response": response_text,
        "checkpoint": checkpoint,
        "vector_hits": vector_context.hits,
        "summary_used": vector_context.summary.is_some(),
        "knowledge": knowledge,
        "reviews": reviews
    });

    Ok(result)
}

async fn run_agent_collecting(
    server: &AcpServer,
    stream_ctx: StreamNotificationContext<'_>,
    agent: Arc<dyn crate::agent::Agent>,
    messages: Vec<Message>,
    principles: Option<Vec<String>>,
    options: Option<std::collections::HashMap<String, Value>>,
    timeout_seconds: Option<u64>,
) -> Result<String> {
    let (sender, mut receiver) = mpsc::channel::<String>(2048);
    let sender = crate::agent::StreamingSender::from(sender);
    let task = tokio::spawn(async move { agent.chat(messages, principles, options, sender).await });

    let collect = async move {
        let stream_started = Instant::now();
        let mut response = String::new();
        let mut chunk_index = 0usize;
        let mut total_chars = 0usize;
        while let Some(token) = receiver.recv().await {
            let next_chars = token.chars().count();
            if stream_would_exceed_limits(chunk_index, total_chars, next_chars) {
                anyhow::bail!("stream output exceeded configured safety limits");
            }
            response.push_str(&token);
            chunk_index += 1;
            total_chars += next_chars;
            emit_stream_chunk(
                server,
                stream_ctx.stream_observer.as_ref(),
                StreamEventMeta {
                    agent_name: stream_ctx.agent_name,
                    phase_name: stream_ctx.phase_name,
                    trace_id: stream_ctx.trace_id,
                },
                &token,
                chunk_index,
                total_chars,
            )
            .await?;
        }

        match task.await {
            Ok(Ok(())) => {
                emit_stream_done(
                    server,
                    stream_ctx.stream_observer.as_ref(),
                    StreamEventMeta {
                        agent_name: stream_ctx.agent_name,
                        phase_name: stream_ctx.phase_name,
                        trace_id: stream_ctx.trace_id,
                    },
                    chunk_index,
                    total_chars,
                    stream_started.elapsed().as_millis() as u64,
                )
                .await?;
                Ok::<String, anyhow::Error>(response)
            }
            Ok(Err(err)) => Err(err),
            Err(join_err) => Err(anyhow::anyhow!("agent task panicked: {join_err}")),
        }
    };

    if let Some(seconds) = timeout_seconds {
        let timeout = Duration::from_secs(seconds.max(1));
        match tokio::time::timeout(timeout, collect).await {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!(
                "agent request timed out after {}s",
                seconds
            )),
        }
    } else {
        collect.await
    }
}

async fn emit_stream_chunk(
    server: &AcpServer,
    observer: Option<&StreamObserver>,
    meta: StreamEventMeta<'_>,
    token: &str,
    chunk_index: usize,
    total_chars: usize,
) -> Result<()> {
    let Some(observer) = observer else {
        return Ok(());
    };

    if let Some(response_id) = observer.jsonrpc_response_id.clone() {
        let response_id = Some(response_id);
        crate::acp::r#impl::io::send_notification(
            server,
            "chat.stream.chunk",
            stream_chunk_notification(
                &response_id,
                meta.agent_name,
                token,
                chunk_index,
                total_chars,
                None,
                Some(meta.phase_name),
                Some(meta.trace_id),
            ),
        )
        .await?;
    }

    if let Some(sender) = &observer.sse_sender {
        let _ = sender.send(StreamFrame {
            event: "chunk".to_string(),
            payload: json!({
                "agent": meta.agent_name,
                "chunk_index": chunk_index,
                "phase": meta.phase_name,
                "token": token,
                "total_chars": total_chars,
                "trace_id": meta.trace_id,
            }),
        });
    }

    Ok(())
}

async fn emit_stream_done(
    server: &AcpServer,
    observer: Option<&StreamObserver>,
    meta: StreamEventMeta<'_>,
    chunk_index: usize,
    total_chars: usize,
    duration_ms: u64,
) -> Result<()> {
    let Some(observer) = observer else {
        return Ok(());
    };

    if let Some(response_id) = observer.jsonrpc_response_id.clone() {
        let response_id = Some(response_id);
        crate::acp::r#impl::io::send_notification(
            server,
            "chat.stream.done",
            stream_done_notification(
                &response_id,
                meta.agent_name,
                chunk_index,
                total_chars,
                None,
                Some(meta.phase_name),
                Some(meta.trace_id),
                duration_ms,
            ),
        )
        .await?;
    }

    if let Some(sender) = &observer.sse_sender {
        let _ = sender.send(StreamFrame {
            event: "done".to_string(),
            payload: json!({
                "agent": meta.agent_name,
                "chunks": chunk_index,
                "done": true,
                "duration_ms": duration_ms,
                "phase": meta.phase_name,
                "total_chars": total_chars,
                "trace_id": meta.trace_id,
            }),
        });
    }

    Ok(())
}

/// Check conversation history for escalation requirements
async fn check_conversation_history_escalation(
    server: &AcpServer,
    conversation_id: &str,
) -> Result<bool> {
    // This is a simplified implementation
    // In reality, this would check the conversation store for history
    let conversation_state = server.conversation_state.lock().await;

    // Check if conversation exists in checkpoints
    let has_conversation = conversation_state
        .checkpoints
        .iter()
        .any(|checkpoint| checkpoint.conversation_id == conversation_id);

    if has_conversation {
        // Check if conversation has had previous escalations
        let has_previous_escalations = conversation_state
            .checkpoints
            .iter()
            .any(|checkpoint| checkpoint.note.as_deref().unwrap_or("") == "escalation");

        // Check conversation length (long conversations might need escalation)
        let conversation_checkpoints: Vec<_> = conversation_state
            .checkpoints
            .iter()
            .filter(|cp| cp.conversation_id == conversation_id)
            .collect();
        let is_long_conversation = conversation_checkpoints.len() > 10;

        Ok(has_previous_escalations || is_long_conversation)
    } else {
        Ok(false) // New conversation, no history to check
    }
}

/// Check phase-specific escalation rules
async fn check_phase_escalation_rules(
    _server: &AcpServer,
    phase: &str,
    options: Option<&PhaseOptions>,
) -> Result<bool> {
    // Check phase-specific rules
    // This is a simplified implementation

    match phase {
        "full_auto" => {
            // Full auto phase always requires careful consideration
            Ok(true)
        }
        "safeguard" => {
            // Safeguard phase might require escalation based on options
            if let Some(opts) = options {
                // Check if extra options indicate escalation needed
                let requires_escalation = opts
                    .extra
                    .get("require_escalation")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Ok(requires_escalation)
            } else {
                Ok(false)
            }
        }
        _ => {
            // Default phases don't require escalation
            Ok(false)
        }
    }
}

/// Extract task description from messages
fn extract_task_description(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| message.role.eq_ignore_ascii_case("user"))
        .map(|message| message.content.clone())
        .or_else(|| messages.last().map(|message| message.content.clone()))
        .unwrap_or_default()
}

/// Create default requirement contract
fn default_requirement_contract(task: &str, source: &str) -> RequirementContractArtifact {
    RequirementContractArtifact {
        generated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
        task: task.to_string(),
        source: source.to_string(),
        goal: String::new(),
        scope: String::new(),
        non_goals: Vec::new(),
        acceptance_criteria: Vec::new(),
        constraints: Vec::new(),
        open_questions: Vec::new(),
        ambiguity_score: 0,
        user_confirmed: false,
    }
}

// Helper functions that will be implemented in other modules

/// Send error response
async fn send_error(
    server: &AcpServer,
    id: Option<Value>,
    code: i64,
    message: String,
    data: Option<Value>,
) -> Result<()> {
    crate::acp::r#impl::io::send_error(server, id, code, message, data).await
}

/// Send result response
async fn send_result(server: &AcpServer, id: Option<Value>, result: Value) -> Result<()> {
    crate::acp::r#impl::io::send_result(server, id, result).await
}

#[derive(Debug, Clone, Default)]
struct VectorContext {
    hits: Vec<Value>,
    summary: Option<String>,
    knowledge: Vec<String>,
}

#[derive(Debug, Clone)]
struct StreamNotificationContext<'a> {
    stream_observer: Option<StreamObserver>,
    agent_name: &'a str,
    phase_name: &'a str,
    trace_id: &'a str,
}

#[derive(Debug, Clone, Copy)]
struct StreamEventMeta<'a> {
    agent_name: &'a str,
    phase_name: &'a str,
    trace_id: &'a str,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StreamFrame {
    pub event: String,
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub(crate) struct StreamObserver {
    jsonrpc_response_id: Option<Value>,
    sse_sender: Option<mpsc::UnboundedSender<StreamFrame>>,
}

impl StreamObserver {
    pub(crate) fn jsonrpc(response_id: Option<Value>) -> Self {
        Self {
            jsonrpc_response_id: response_id,
            sse_sender: None,
        }
    }

    pub(crate) fn sse(sender: mpsc::UnboundedSender<StreamFrame>) -> Self {
        Self {
            jsonrpc_response_id: None,
            sse_sender: Some(sender),
        }
    }
}

#[derive(Debug, Clone)]
struct EffectiveVectorSettings {
    min_query_chars: usize,
    top_k: usize,
    min_similarity: f32,
    max_snippet_chars: usize,
    summary_enabled: bool,
    summary_trigger_messages: usize,
    summary_max_chars: usize,
    auto_mode: bool,
}

async fn load_vector_context(
    server: &AcpServer,
    phase_name: &str,
    phase_options: Option<&PhaseOptions>,
    params: &ChatParams,
) -> VectorContext {
    if let Some(hits) = params.vector_hits.clone() {
        let knowledge = load_recent_knowledge_context(server, phase_name, 3);
        return VectorContext {
            hits,
            summary: load_phase_summary(server, phase_name, phase_options).await,
            knowledge,
        };
    }

    let knowledge = load_recent_knowledge_context(server, phase_name, 3);
    let Some(settings) = effective_vector_settings(server, phase_options).await else {
        return VectorContext {
            hits: Vec::new(),
            summary: None,
            knowledge,
        };
    };
    let Some(query_text) = latest_user_message(&params.messages) else {
        return VectorContext {
            hits: Vec::new(),
            summary: None,
            knowledge,
        };
    };

    let summary = load_phase_summary(server, phase_name, phase_options).await;
    if query_text.chars().count() < settings.min_query_chars {
        return VectorContext {
            hits: Vec::new(),
            summary,
            knowledge,
        };
    }

    let Some(store) = server.vector_store.clone() else {
        return VectorContext {
            hits: Vec::new(),
            summary,
            knowledge,
        };
    };

    match store.search(
        phase_name,
        query_text,
        settings.top_k,
        settings.min_similarity,
        settings.max_snippet_chars,
    ) {
        Ok((hits, feedback)) => {
            server.metrics.record_vector_search(hits.len());
            apply_autotune_feedback(server, phase_options, settings.auto_mode, feedback.avg_similarity)
                .await;
            let reranked_hits = rerank_hits_with_phase_summary(hits, summary.as_deref(), settings.top_k);
            VectorContext {
                hits: reranked_hits
                    .into_iter()
                    .map(|hit| {
                        json!({
                            "response_snippet": hit.response_snippet,
                            "similarity": hit.similarity,
                        })
                    })
                    .collect(),
                summary,
                knowledge,
            }
        }
        Err(err) => {
            warn!(phase = phase_name, error = %err, "vector search failed, continuing without retrieval context");
            VectorContext {
                hits: Vec::new(),
                summary,
                knowledge,
            }
        }
    }
}

fn rerank_hits_with_phase_summary(
    mut hits: Vec<crate::vector::VectorHit>,
    summary: Option<&str>,
    top_k: usize,
) -> Vec<crate::vector::VectorHit> {
    if hits.len() <= 1 {
        return hits;
    }

    let Some(summary_text) = summary else {
        return hits;
    };

    let intent_terms = parse_summary_field(summary_text, "Intent:")
        .map(|text| extract_terms(&text, 24))
        .unwrap_or_default();
    let constraints_terms = parse_summary_field(summary_text, "Constraints:")
        .map(|text| extract_terms(&text, 24))
        .unwrap_or_default();
    let decisions_terms = parse_summary_field(summary_text, "Decisions:")
        .map(|text| extract_terms(&text, 24))
        .unwrap_or_default();
    let risks_terms = parse_summary_field(summary_text, "Risks:")
        .map(|text| extract_terms(&text, 24))
        .unwrap_or_default();
    let next_terms = parse_summary_field(summary_text, "Next:")
        .map(|text| extract_terms(&text, 24))
        .unwrap_or_default();

    // Keep semantic similarity as the anchor while prioritizing risk/next continuity.
    hits.sort_by(|left, right| {
        let left_score = combined_hit_score(
            left.similarity,
            &left.response_snippet,
            &intent_terms,
            &constraints_terms,
            &decisions_terms,
            &risks_terms,
            &next_terms,
        );
        let right_score = combined_hit_score(
            right.similarity,
            &right.response_snippet,
            &intent_terms,
            &constraints_terms,
            &decisions_terms,
            &risks_terms,
            &next_terms,
        );

        right_score
            .partial_cmp(&left_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    hits.truncate(top_k.max(1));
    hits
}

fn combined_hit_score(
    similarity: f32,
    snippet: &str,
    intent_terms: &[String],
    constraints_terms: &[String],
    decisions_terms: &[String],
    risks_terms: &[String],
    next_terms: &[String],
) -> f32 {
    let section_score = weighted_section_overlap(
        snippet,
        intent_terms,
        constraints_terms,
        decisions_terms,
        risks_terms,
        next_terms,
    );
    (similarity * 0.75) + (section_score * 0.25)
}

fn weighted_section_overlap(
    snippet: &str,
    intent_terms: &[String],
    constraints_terms: &[String],
    decisions_terms: &[String],
    risks_terms: &[String],
    next_terms: &[String],
) -> f32 {
    let sections = [
        (0.10_f32, intent_terms),
        (0.15_f32, constraints_terms),
        (0.10_f32, decisions_terms),
        (0.40_f32, risks_terms),
        (0.25_f32, next_terms),
    ];

    let mut weighted_sum = 0.0_f32;
    let mut total_weight = 0.0_f32;

    for (weight, terms) in sections {
        if terms.is_empty() {
            continue;
        }
        weighted_sum += weight * term_overlap_score(snippet, terms);
        total_weight += weight;
    }

    if total_weight <= f32::EPSILON {
        0.0
    } else {
        weighted_sum / total_weight
    }
}

fn term_overlap_score(text: &str, terms: &[String]) -> f32 {
    if terms.is_empty() {
        return 0.0;
    }

    let lower = text.to_ascii_lowercase();
    let matched = terms
        .iter()
        .filter(|term| lower.contains(term.as_str()))
        .count();
    (matched as f32) / (terms.len() as f32)
}

fn extract_terms(text: &str, max_terms: usize) -> Vec<String> {
    let mut terms = Vec::new();

    for token in text
        .split(|c: char| !c.is_alphanumeric())
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        let normalized = token.to_ascii_lowercase();
        if normalized.chars().count() >= 3
            && !terms.contains(&normalized)
            && !matches!(normalized.as_str(), "the" | "and" | "for" | "with" | "that")
        {
            terms.push(normalized);
        }
        if terms.len() >= max_terms {
            break;
        }
    }

    terms
}

fn load_recent_knowledge_context(server: &AcpServer, phase_name: &str, limit: usize) -> Vec<String> {
    let ledger = crate::acp::r#impl::runtime::artifact_ledger(server);
    let path = ledger.latest_path("spec", "latest-knowledge.json");
    if !Path::new(&path).exists() {
        return Vec::new();
    }

    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => return Vec::new(),
    };
    let bus = match serde_json::from_str::<KnowledgeBusArtifact>(&raw) {
        Ok(bus) => bus,
        Err(_) => return Vec::new(),
    };

    bus.events
        .iter()
        .rev()
        .filter(|event| event.phase == phase_name)
        .take(limit.max(1))
        .map(|event| {
            format!(
                "{} | confidence {:.2} | {}",
                event.task,
                event.confidence,
                event.reusable_insights.join(" | ")
            )
        })
        .collect()
}

async fn persist_vector_memory(
    server: &AcpServer,
    phase_name: &str,
    phase_options: Option<&PhaseOptions>,
    params: &ChatParams,
    response_text: &str,
    selected_agent: &str,
) {
    let Some(settings) = effective_vector_settings(server, phase_options).await else {
        return;
    };
    let Some(store) = server.vector_store.clone() else {
        return;
    };
    let Some(query_text) = latest_user_message(&params.messages) else {
        return;
    };

    if let Err(err) = store.upsert(phase_name, query_text, response_text) {
        warn!(phase = phase_name, error = %err, "vector upsert failed");
    } else {
        server.metrics.record_vector_store();
    }

    if settings.summary_enabled && params.messages.len() >= settings.summary_trigger_messages {
        let summary_text = generate_phase_summary_text(
            server,
            phase_name,
            phase_options,
            selected_agent,
            &params.messages,
            response_text,
            settings.summary_max_chars,
        )
        .await
        .unwrap_or_else(|| {
            build_phase_summary(&params.messages, response_text, settings.summary_max_chars)
        });
        if !summary_text.is_empty() {
            if let Err(err) = store.upsert_phase_summary(phase_name, &summary_text) {
                warn!(phase = phase_name, error = %err, "phase summary upsert failed");
            } else {
                server.metrics.record_summary_store();
            }
        }
    }
}

async fn generate_phase_summary_text(
    server: &AcpServer,
    phase_name: &str,
    phase_options: Option<&PhaseOptions>,
    selected_agent: &str,
    messages: &[Message],
    response_text: &str,
    max_chars: usize,
) -> Option<String> {
    let llm_enabled = phase_options
        .and_then(|opts| opts.extra.get("llm_summary_enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !llm_enabled || selected_agent.trim().is_empty() {
        return None;
    }

    let registry = server.agent_registry.as_ref()?;
    let agent = registry.get(selected_agent)?;

    let timeout_seconds = phase_options
        .and_then(|opts| opts.extra.get("llm_summary_timeout_seconds"))
        .and_then(Value::as_u64)
        .unwrap_or(12)
        .max(1);
    let max_tokens = phase_options
        .and_then(|opts| opts.extra.get("llm_summary_max_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(((max_chars / 3).clamp(64, 512)) as u64);

    let mut summary_options = phase_options
        .and_then(PhaseOptions::agent_options)
        .unwrap_or_default();
    summary_options.insert("max_tokens".to_string(), json!(max_tokens));
    summary_options.insert("temperature".to_string(), json!(0.1));

    let dialogue = build_summary_dialogue(messages, response_text, max_chars.saturating_mul(2));
    let fallback_summary = build_phase_summary(messages, response_text, max_chars);
    let summary_prompt = vec![
        Message {
            role: "system".to_string(),
            content: format!(
                "You are generating a reusable abstractive phase summary for future turns. Output plain text only (no markdown, no code fences) and keep at most {} characters.",
                max_chars
            ),
        },
        Message {
            role: "user".to_string(),
            content: format!(
                "Phase: {}\n\nDialogue:\n{}\n\nReturn exactly five lines in this exact label format:\nIntent: ...\nConstraints: ...\nDecisions: ...\nRisks: ...\nNext: ...\n\nKeep each line concise and avoid placeholders like N/A unless truly unknown.",
                phase_name, dialogue,
            ),
        },
    ];

    let result = run_agent_collecting(
        server,
        StreamNotificationContext {
            stream_observer: None,
            agent_name: selected_agent,
            phase_name,
            trace_id: "summary",
        },
        agent,
        summary_prompt,
        None,
        Some(summary_options),
        Some(timeout_seconds),
    )
    .await
    .ok()?;

    let compact = result.trim();
    if compact.is_empty() {
        return None;
    }

    Some(normalize_phase_summary(compact, &fallback_summary, max_chars))
}

fn build_summary_dialogue(messages: &[Message], response_text: &str, max_chars: usize) -> String {
    let mut parts = messages
        .iter()
        .rev()
        .take(8)
        .collect::<Vec<_>>();
    parts.reverse();

    let mut rendered = parts
        .into_iter()
        .map(|m| format!("{}: {}", m.role, m.content.trim()))
        .collect::<Vec<_>>();
    rendered.push(format!("assistant: {}", response_text.trim()));
    truncate_chars(&rendered.join("\n"), max_chars)
}

async fn load_phase_summary(
    server: &AcpServer,
    phase_name: &str,
    phase_options: Option<&PhaseOptions>,
) -> Option<String> {
    let settings = effective_vector_settings(server, phase_options).await?;
    if !settings.summary_enabled {
        return None;
    }

    let store = server.vector_store.clone()?;

    match store.get_phase_summary(phase_name) {
        Ok(summary) => {
            server.metrics.record_summary_read(summary.is_some());
            summary
        }
        Err(err) => {
            warn!(phase = phase_name, error = %err, "phase summary lookup failed");
            None
        }
    }
}

async fn effective_vector_settings(
    server: &AcpServer,
    phase_options: Option<&PhaseOptions>,
) -> Option<EffectiveVectorSettings> {
    let vector_config = server.vector_config.clone()?;
    if !vector_config.enabled {
        return None;
    }
    if matches!(phase_options.and_then(|opts| opts.vector_enabled), Some(false)) {
        return None;
    }

    let mut min_query_chars = phase_options
        .and_then(|opts| opts.vector_min_query_chars)
        .unwrap_or(vector_config.min_query_chars);
    let mut top_k = phase_options
        .and_then(|opts| opts.vector_top_k)
        .unwrap_or(vector_config.top_k);
    let auto_mode = phase_options
        .and_then(|opts| opts.vector_auto)
        .unwrap_or(vector_config.auto_mode);

    if auto_mode {
        if let Some(autotune) = server.autotune.as_ref() {
            let state = autotune.lock().await;
            min_query_chars = state.current_min_query_chars;
            top_k = state.current_top_k;
        }
    }

    Some(EffectiveVectorSettings {
        min_query_chars,
        top_k,
        min_similarity: phase_options
            .and_then(|opts| opts.vector_min_similarity)
            .unwrap_or(vector_config.min_similarity),
        max_snippet_chars: phase_options
            .and_then(|opts| opts.vector_max_snippet_chars)
            .unwrap_or(vector_config.max_snippet_chars),
        summary_enabled: phase_options
            .and_then(|opts| opts.summary_enabled)
            .unwrap_or(vector_config.summary_enabled),
        summary_trigger_messages: phase_options
            .and_then(|opts| opts.summary_trigger_messages)
            .unwrap_or(vector_config.summary_trigger_messages),
        summary_max_chars: phase_options
            .and_then(|opts| opts.summary_max_chars)
            .unwrap_or(vector_config.summary_max_chars),
        auto_mode,
    })
}

async fn apply_autotune_feedback(
    server: &AcpServer,
    phase_options: Option<&PhaseOptions>,
    auto_mode: bool,
    precision: f32,
) {
    if !auto_mode || matches!(phase_options.and_then(|opts| opts.vector_enabled), Some(false)) {
        return;
    }

    let (Some(autotune), Some(config)) = (server.autotune.as_ref(), server.autotune_config.as_ref()) else {
        return;
    };

    let mut state = autotune.lock().await;
    state.record_vector_search(precision, config);
    let _ = state.advance_cooldown_window(config);
    let _ = state.evaluate_and_adjust(config);
    if let Some(path) = &server.autotune_state_path {
        if let Err(err) = state.save(path) {
            warn!(error = %err, "failed to persist autotune state after vector feedback");
        }
    }
}

fn latest_user_message(messages: &[Message]) -> Option<&str> {
    messages
        .iter()
        .rev()
        .find(|message| message.role.eq_ignore_ascii_case("user") && !message.content.trim().is_empty())
        .map(|message| message.content.trim())
}

fn build_vector_context_message(
    summary: Option<&str>,
    hits: &[Value],
    knowledge: &[String],
) -> Option<String> {
    let mut sections = Vec::new();

    if let Some(summary) = summary {
        let trimmed = summary.trim();
        if !trimmed.is_empty() {
            sections.push(format!("Phase summary:\n{}", trimmed));
        }
    }

    if !hits.is_empty() {
        let rendered = hits
            .iter()
            .enumerate()
            .filter_map(|(index, hit)| {
                let snippet = hit.get("response_snippet").and_then(Value::as_str)?;
                let similarity = hit.get("similarity").and_then(Value::as_f64).unwrap_or(0.0);
                Some(format!("{}. ({:.3}) {}", index + 1, similarity, snippet))
            })
            .collect::<Vec<_>>();
        if !rendered.is_empty() {
            sections.push(format!("Relevant prior context:\n{}", rendered.join("\n")));
        }
    }

    if !knowledge.is_empty() {
        sections.push(format!(
            "Distilled reusable knowledge:\n{}",
            knowledge
                .iter()
                .enumerate()
                .map(|(idx, item)| format!("{}. {}", idx + 1, item))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    if sections.is_empty() {
        None
    } else {
        Some(format!(
            "Use the following retrieved context when it is relevant, but do not repeat it verbatim unless needed.\n\n{}",
            sections.join("\n\n")
        ))
    }
}

fn merge_context_into_messages(messages: &[Message], context: Option<String>) -> Vec<Message> {
    let Some(context) = context else {
        return messages.to_vec();
    };

    let mut merged = messages.to_vec();
    if let Some(first) = merged.first_mut() {
        if first.role.eq_ignore_ascii_case("system") {
            first.content = format!("{}\n\n{}", first.content.trim(), context);
            return merged;
        }
    }

    merged.insert(
        0,
        Message {
            role: "system".to_string(),
            content: context,
        },
    );
    merged
}

fn build_phase_summary(messages: &[Message], response_text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let intent = latest_user_message(messages)
        .map(|text| truncate_chars(text, 180))
        .unwrap_or_else(|| "Continue and complete the active phase request".to_string());

    let constraints = collect_signal_lines(
        messages,
        &["must", "need", "require", "不要", "不能", "必须", "完整", "一次"],
        2,
        80,
    )
    .join("; ");
    let constraints = if constraints.is_empty() {
        "Maintain reliability with fallback and validation".to_string()
    } else {
        constraints
    };

    let decisions = collect_lines_from_text(
        response_text,
        &["implemented", "enabled", "fixed", "added", "updated", "完成", "已", "接入"],
        3,
        90,
    )
    .join("; ");
    let decisions = if decisions.is_empty() {
        truncate_chars(response_text, 180)
    } else {
        decisions
    };

    let risks = collect_lines_from_text(
        response_text,
        &["risk", "pending", "todo", "warning", "timeout", "fallback", "风险", "待", "告警"],
        2,
        80,
    )
    .join("; ");
    let risks = if risks.is_empty() {
        "No blocking risks identified; fallback remains active".to_string()
    } else {
        risks
    };

    let next = collect_lines_from_text(
        response_text,
        &["next", "follow", "suggest", "can", "可以", "下一步", "建议"],
        2,
        80,
    )
    .join("; ");
    let next = if next.is_empty() {
        "Proceed with full test and clippy validation".to_string()
    } else {
        next
    };

    let structured = format!(
        "Intent: {}\nConstraints: {}\nDecisions: {}\nRisks: {}\nNext: {}",
        intent, constraints, decisions, risks, next
    );
    truncate_chars(&structured, max_chars)
}

fn collect_signal_lines(
    messages: &[Message],
    keywords: &[&str],
    limit: usize,
    max_chars: usize,
) -> Vec<String> {
    let mut selected = Vec::new();

    for message in messages.iter().rev() {
        let text = message.content.trim();
        if text.is_empty() {
            continue;
        }
        let lower = text.to_ascii_lowercase();
        if keywords
            .iter()
            .any(|keyword| lower.contains(&keyword.to_ascii_lowercase()) || text.contains(keyword))
        {
            selected.push(truncate_chars(text, max_chars));
        }
        if selected.len() >= limit {
            break;
        }
    }

    selected.reverse();
    selected
}

fn collect_lines_from_text(
    text: &str,
    keywords: &[&str],
    limit: usize,
    max_chars: usize,
) -> Vec<String> {
    let mut selected = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let lower = trimmed.to_ascii_lowercase();
        if keywords
            .iter()
            .any(|keyword| lower.contains(&keyword.to_ascii_lowercase()) || trimmed.contains(keyword))
        {
            selected.push(truncate_chars(trimmed, max_chars));
        }

        if selected.len() >= limit {
            break;
        }
    }

    selected
}

fn parse_summary_field(text: &str, label: &str) -> Option<String> {
    text.lines().find_map(|line| {
        line.strip_prefix(label)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

fn normalize_phase_summary(raw: &str, fallback: &str, max_chars: usize) -> String {
    let intent = parse_summary_field(raw, "Intent:")
        .or_else(|| parse_summary_field(fallback, "Intent:"))
        .unwrap_or_else(|| "Continue and complete the active phase request".to_string());
    let constraints = parse_summary_field(raw, "Constraints:")
        .or_else(|| parse_summary_field(fallback, "Constraints:"))
        .unwrap_or_else(|| "Maintain reliability with fallback and validation".to_string());
    let decisions = parse_summary_field(raw, "Decisions:")
        .or_else(|| parse_summary_field(fallback, "Decisions:"))
        .unwrap_or_else(|| truncate_chars(raw, 140));
    let risks = parse_summary_field(raw, "Risks:")
        .or_else(|| parse_summary_field(fallback, "Risks:"))
        .unwrap_or_else(|| "No blocking risks identified; fallback remains active".to_string());
    let next = parse_summary_field(raw, "Next:")
        .or_else(|| parse_summary_field(fallback, "Next:"))
        .unwrap_or_else(|| "Proceed with full test and clippy validation".to_string());

    let normalized = format!(
        "Intent: {}\nConstraints: {}\nDecisions: {}\nRisks: {}\nNext: {}",
        intent, constraints, decisions, risks, next
    );
    truncate_chars(&normalized, max_chars)
}

async fn persist_chat_knowledge(
    server: &AcpServer,
    conversation_id: &str,
    branch_id: &str,
    phase_name: &str,
    agent_name: &str,
    params: &ChatParams,
    response_text: &str,
) -> Value {
    let task = extract_task_description(&params.messages);
    let request_excerpt = truncate_chars(&task, 240);
    let response_excerpt = truncate_chars(response_text, 320);
    let reusable_insights = derive_reusable_insights(response_text);
    let verification_steps = derive_verification_steps(response_text);
    let confidence = derive_knowledge_confidence(&reusable_insights, &verification_steps);

    let artifact = KnowledgeInsightArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        conversation_id: conversation_id.to_string(),
        branch_id: branch_id.to_string(),
        phase: phase_name.to_string(),
        task: truncate_chars(&task, 200),
        agent: agent_name.to_string(),
        source: "chat".to_string(),
        request_excerpt: request_excerpt.clone(),
        response_excerpt: response_excerpt.clone(),
        reusable_insights: reusable_insights.clone(),
        verification_steps: verification_steps.clone(),
        confidence,
    };

    let memory_class = if confidence >= 0.9 && reusable_insights.len() >= 2 {
        crate::memory_module::MemoryClass::Semantic
    } else {
        crate::memory_module::MemoryClass::Episodic
    };
    let memory_class_name = format!("{:?}", memory_class);

    let memory_content = json!({
        "phase": phase_name,
        "conversation_id": conversation_id,
        "branch_id": branch_id,
        "task": artifact.task,
        "reusable_insights": artifact.reusable_insights,
        "verification_steps": artifact.verification_steps,
        "response_excerpt": artifact.response_excerpt,
    })
    .to_string();

    let mut retained_entries = 0usize;
    let mut promoted_count = 0usize;
    if let Ok(mut store) = server.memory_store.lock() {
        store.store(crate::memory_module::MemoryEntry {
            id: format!(
                "knowledge-{}-{}",
                crate::acp::prelude::now_ts_ms(),
                branch_id
            ),
            class: memory_class,
            content: memory_content,
            timestamp: crate::acp::prelude::now_ts().to_string(),
            usefulness: confidence as f32,
            staleness: 0,
        });
        store.gc();
        let promotion = store.promote();
        promoted_count = promotion.promoted_count;
        retained_entries = store.retrieve(crate::memory_module::MemoryClass::Observation, 256).len()
            + store.retrieve(crate::memory_module::MemoryClass::Episodic, 256).len()
            + store.retrieve(crate::memory_module::MemoryClass::Semantic, 256).len()
            + store
                .retrieve(crate::memory_module::MemoryClass::ProjectState, 256)
                .len();
    }

    let mut vector_memory_written = false;
    if let Some(vector_store) = server.vector_store.clone() {
        let vector_payload = format!(
            "Task: {}\nInsights:\n{}\nVerification:\n{}\nAnswer:\n{}",
            request_excerpt,
            reusable_insights.join("\n"),
            verification_steps.join("\n"),
            response_excerpt,
        );
        if vector_store
            .upsert(
                phase_name,
                &format!("knowledge:{}:{}", phase_name, request_excerpt),
                &vector_payload,
            )
            .is_ok()
        {
            server.metrics.record_vector_store();
            vector_memory_written = true;
        }
    }

    let ledger = crate::acp::r#impl::runtime::artifact_ledger(server);
    let artifact_path = persist_knowledge_insight_event(&ledger, artifact, 256)
        .ok()
        .map(|path| path.display().to_string());

    json!({
        "memory_class": memory_class_name,
        "confidence": confidence,
        "reusable_insights": reusable_insights,
        "verification_steps": verification_steps,
        "artifact_path": artifact_path,
        "retained_entries": retained_entries,
        "promoted_count": promoted_count,
        "vector_memory_written": vector_memory_written,
    })
}

fn derive_reusable_insights(response_text: &str) -> Vec<String> {
    response_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| line.len() >= 24)
        .filter(|line| !line.starts_with("```") && !line.starts_with('#'))
        .take(4)
        .map(|line| truncate_chars(line, 180))
        .collect()
}

fn derive_verification_steps(response_text: &str) -> Vec<String> {
    response_text
        .lines()
        .map(str::trim)
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("test")
                || lower.contains("verify")
                || lower.contains("clippy")
                || lower.contains("check")
                || lower.contains("build")
        })
        .take(4)
        .map(|line| truncate_chars(line, 160))
        .collect()
}

fn derive_knowledge_confidence(reusable_insights: &[String], verification_steps: &[String]) -> f64 {
    let base = if reusable_insights.is_empty() { 0.72 } else { 0.82 };
    let verification_bonus = (verification_steps.len().min(3) as f64) * 0.05;
    let insight_bonus = (reusable_insights.len().min(3) as f64) * 0.03;
    (base + verification_bonus + insight_bonus).clamp(0.0, 0.98)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let trimmed = text.trim();
    let mut result = trimmed.chars().take(max_chars).collect::<String>();
    if trimmed.chars().count() > max_chars && max_chars > 1 {
        let keep = max_chars.saturating_sub(3);
        result = trimmed.chars().take(keep).collect::<String>();
        result.push_str("...");
    }
    result
}

/// Get routing handles
fn routing_handles(
    server: &AcpServer,
) -> Result<(Arc<FlowManager>, Arc<crate::agent::AgentRegistry>)> {
    crate::acp::r#impl::runtime::routing_handles(server)
}

fn reorder_chat_agents_by_runtime_score(
    server: &AcpServer,
    phase_name: &str,
    agents: &mut Vec<(String, Arc<dyn crate::agent::Agent>)>,
) {
    if agents.len() <= 1 {
        return;
    }

    let names = agents
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    let ranked = server
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

/// Record trace event
#[allow(clippy::too_many_arguments)]
fn record_trace_event(
    _server: &AcpServer,
    _trace: &RequestTraceContext,
    _event_type: &str,
    _status: &str,
    _stage: &str,
    _inputs: Value,
    _outputs: Option<Value>,
    _duration_ms: u64,
) {
    // Trace sink will be extended with persistent storage in a follow-up.
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use anyhow::Result;
    use async_trait::async_trait;
    use serde_json::Value;
    use serde_json::json;

    use crate::acp::server::ServerBuilder;
    use crate::agent::{Agent, AgentRegistry, Message, StreamingSender};
    use crate::config::{AppConfig, FlowConfig, PhaseConfig, PhaseOptions, VectorConfig};
    use crate::flow::FlowManager;
    use crate::rpc_protocol::chat_trace_context;
    use crate::vector::VectorStore;

    use super::{build_phase_summary, process_chat_request, ChatParams};

    struct RecordingAgent {
        seen_messages: Arc<Mutex<Vec<Message>>>,
        output: String,
    }

    #[async_trait]
    impl Agent for RecordingAgent {
        async fn chat(
            &self,
            messages: Vec<Message>,
            _principles: Option<Vec<String>>,
            _options: Option<HashMap<String, Value>>,
            sender: StreamingSender,
        ) -> Result<()> {
            *self.seen_messages.lock().expect("messages lock") = messages;
            sender
                .send(self.output.clone())
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            Ok(())
        }
    }

    fn test_config() -> AppConfig {
        let mut phases = HashMap::new();
        phases.insert(
            "coding".to_string(),
            PhaseConfig {
                description: "coding".to_string(),
                agents: vec!["test-agent".to_string()],
                fallback: Some(true),
                principles: None,
                options: Some(PhaseOptions {
                    vector_enabled: Some(true),
                    vector_min_query_chars: Some(4),
                    vector_top_k: Some(2),
                    vector_min_similarity: Some(0.0),
                    vector_max_snippet_chars: Some(120),
                    summary_enabled: Some(true),
                    summary_trigger_messages: Some(1),
                    summary_max_chars: Some(240),
                    extra: std::iter::once(("llm_summary_enabled".to_string(), json!(false)))
                        .collect(),
                    ..PhaseOptions::default()
                }),
            },
        );

        AppConfig {
            default_phase: "coding".to_string(),
            agents: HashMap::new(),
            flow: FlowConfig {
                name: "flow".to_string(),
                phases: vec!["coding".to_string()],
            },
            phases,
            runtime: None,
            cache: None,
            vector: Some(VectorConfig {
                enabled: true,
                auto_mode: false,
                path: "vector.sqlite3".to_string(),
                dimensions: 32,
                min_query_chars: 4,
                top_k: 2,
                min_similarity: 0.0,
                max_snippet_chars: 120,
                max_entries: 128,
                summary_enabled: true,
                summary_trigger_messages: 1,
                summary_max_chars: 240,
            }),
            autotune: None,
            model_selection_mode: "adaptive".to_string(),
        }
    }

    #[tokio::test]
    async fn process_chat_request_wires_vector_context_and_checkpoint_tree() {
        let temp = tempfile::tempdir().expect("tempdir should exist");
        let vector_path = temp.path().join("vector.sqlite3");
        let vector_store = Arc::new(
            VectorStore::new(&vector_path, 32, 128).expect("vector store should initialize"),
        );
        vector_store
            .upsert(
                "coding",
                "rust stream notifications",
                "Use structured stream notifications for chunked output.",
            )
            .expect("seed vector entry");
        vector_store
            .upsert_phase_summary("coding", "Existing coding summary")
            .expect("seed phase summary");

        let seen_messages = Arc::new(Mutex::new(Vec::new()));
        let mut registry = AgentRegistry::new();
        registry.register_arc(
            "test-agent",
            Arc::new(RecordingAgent {
                seen_messages: Arc::clone(&seen_messages),
                output: "streamed answer".to_string(),
            }),
        );

        let config = Arc::new(test_config());
        let flow = Arc::new(FlowManager::new(Arc::clone(&config), None));

        let mut server = ServerBuilder::new().build().expect("server should build");
        server.flow_manager = Some(flow);
        server.agent_registry = Some(Arc::new(registry));
        server.vector_store = Some(Arc::clone(&vector_store));
        server.vector_config = config.vector.clone();
        let config_path = temp.path().join("config.toml");
        std::fs::write(&config_path, "default_phase = \"coding\"\n").expect("config write");
        server.config_path = Some(config_path.display().to_string());
        if let Ok(mut ledger) = server.artifact_ledger.lock() {
            *ledger = crate::reinforcement::ArtifactLedger::new(Some(&config_path));
        }

        let params = ChatParams {
            mode: "ask".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "How should rust stream notifications be implemented?".to_string(),
            }],
            conversation_id: Some("conv-chat".to_string()),
            branch_id: Some("feature-a".to_string()),
            phase: Some("coding".to_string()),
            options: None,
            requirement_contract: None,
            plan: None,
            vector_hits: None,
            execution_decision_candidate: None,
        };

        let trace = chat_trace_context(&Some(json!(1)), "chat.test");
        let result = process_chat_request(&server, &params, None, &trace, None)
            .await
            .expect("chat request should succeed");

        assert_eq!(result["branch_id"], "feature-a");
        assert_eq!(result["response"], "streamed answer");
        assert_eq!(result["vector_hits"].as_array().map(|items| items.len()), Some(1));
        assert_eq!(result["checkpoint"]["branch_id"], "feature-a");
        assert_eq!(result["knowledge"]["vector_memory_written"], true);
        assert!(result["knowledge"]["artifact_path"].is_string());

        let captured = seen_messages.lock().expect("messages lock").clone();
        assert_eq!(captured.first().map(|msg| msg.role.as_str()), Some("system"));
        let system_text = &captured.first().expect("system message expected").content;
        assert!(system_text.contains("Existing coding summary"));
        assert!(system_text.contains("stream notifications"));

        let state = server.conversation_state.lock().await;
        assert_eq!(state.checkpoints.len(), 1);
        assert!(state.branch_heads.contains_key("conv-chat:feature-a"));

        assert_eq!(vector_store.memory_entry_count().expect("count"), 3);
        assert!(vector_store
            .get_phase_summary("coding")
            .expect("summary read")
            .expect("summary should exist")
            .contains("Intent:"));

        let artifact_path = result["knowledge"]["artifact_path"]
            .as_str()
            .expect("artifact path should be present");
        assert!(std::path::Path::new(artifact_path).exists());
    }

    #[test]
    fn build_phase_summary_trims_to_requested_size() {
        let summary = build_phase_summary(
            &[Message {
                role: "user".to_string(),
                content: "0123456789abcdef".to_string(),
            }],
            "response",
            12,
        );

        assert!(summary.chars().count() <= 12);
        assert!(!summary.is_empty());
    }

    #[test]
    fn weighted_section_overlap_prioritizes_risks_and_next() {
        let risk_next = super::weighted_section_overlap(
            "fallback timeout risk and next validation step",
            &["intent".to_string()],
            &["constraints".to_string()],
            &["decisions".to_string()],
            &["fallback".to_string(), "risk".to_string(), "timeout".to_string()],
            &["next".to_string(), "validation".to_string()],
        );
        let intent_only = super::weighted_section_overlap(
            "intent constraints only",
            &["intent".to_string()],
            &["constraints".to_string()],
            &["decisions".to_string()],
            &["fallback".to_string(), "risk".to_string(), "timeout".to_string()],
            &["next".to_string(), "validation".to_string()],
        );

        assert!(risk_next > intent_only);
    }
}
