//! Chat request processing pipeline
//!
//! This module contains the orchestration layer for processing chat requests:
//! routing, agent execution, streaming, vector context, knowledge persistence,
//! and the main entry points `handle_chat` and `process_chat_request`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use std::{fs, path::Path};

use anyhow::Result;
use opentelemetry::{Context as OtelContext, KeyValue};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::Duration;
use tracing::{debug, info, warn};

use crate::acp::helpers::context::{
    probe_agent_runtime_readiness, request_timeout, run_with_optional_timeout,
    AgentRuntimeReadiness,
};
use crate::acp::helpers::conversation::stream_would_exceed_limits;
use crate::acp::helpers::metrics::{stream_chunk_notification, stream_done_notification};
use crate::acp::server::AcpServer;
use crate::agent::Message;
use crate::config::PhaseOptions;
use crate::evaluation::TraceEvent;
use crate::flow::FlowManager;
use crate::i18n::runtime::tf;
use crate::intelligence::token_cache::ContextLengthClass;
use crate::orchestration::autonomy_runtime::{
    build_tool_execution_followup_message, build_tool_result_block, parse_model_used_token,
    parse_thinking_token, parse_tool_call_token, TOKEN_THINKING_PREFIX, TOKEN_TOOL_CALL_PREFIX,
};
use crate::orchestration::planner_executor::Planner;
use crate::orchestration::prompt_layers::PromptAssembler;
use crate::orchestration::skill::SkillDescriptor;
use crate::orchestration::task_router::{TaskRouter, TaskType};
use crate::orchestration::tool::{execute_loop, LoopConfig, LoopDecision, ToolInput, ToolRegistry};
use crate::orchestration::workflow_optimizer::OptimizationContext;
use crate::pua::PuaEnforcementPlan;

use crate::intelligence::verification::{
    DeterministicVerifier, StructuredReview, VerificationVerdict,
};
use crate::memory_module::{MemoryClass, MemoryEntry, MemoryPolicy, MemoryStore};
use crate::orchestration::roles::{AgentRole, RoleRegistry};
use crate::orchestration::task_graph::{TaskGraph, TaskNode};
use crate::reinforcement::{
    build_task_plan, build_workflow_generated_artifact, persist_knowledge_insight_event,
    persist_workflow_generated, persist_workflow_learning_event, ArtifactLedger,
    KnowledgeBusArtifact, KnowledgeInsightArtifact, RequirementContractArtifact,
    TaskPlanArtifact, WorkflowLearningEvent,
};
use crate::rpc_protocol::{chat_trace_context, child_trace_context, RequestTraceContext};

use super::helpers::{
    agent_switch_state, cache_short_circuit_allowed, has_flow_phase, is_quota_or_token_limit_error,
    option_bool, option_usize, reorder_agents_with_priority, round_metric,
    select_strong_model_id, select_top_models, AgentSwitchState,
};
use super::params::{estimate_token_economy, ChatParams, ChatRequestContext};
use super::risk::{
    assess_high_risk, build_risk_vote_policy, normalize_vote_key, AgentStrongVoteOutcome,
    AgentVoteSource, RiskAssessment, RiskVotePolicy,
};

// ── Entry points ──────────────────────────────────────────────────────

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
            .observability
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
        let mut chat_params: ChatParams = match serde_json::from_value(params_value) {
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

        // Fallback to "ask" mode when absent or empty (e.g., from external clients like Zed)
        if chat_params.mode.trim().is_empty() {
            chat_params.mode = "ask".to_string();
            info!("mode not specified by client, defaulting to 'ask'");
        }

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
        }

        // Process chat request
        let result = process_chat_request(
            server,
            &chat_params,
            Some(StreamObserver::jsonrpc(id.clone())),
            &pipeline_trace,
            chat_span.as_ref(),
            None,
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

    server
        .observability
        .metrics
        .record_chat_latency(duration_ms as f64);
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
pub async fn should_escalate_approval_strategy(
    server: &AcpServer,
    mode: &str,
    messages: &[Message],
    conversation_id: Option<&str>,
    phase: Option<&str>,
    options: Option<&PhaseOptions>,
) -> Result<bool> {
    let mode_requires_escalation = matches!(mode, "full_auto" | "safeguard");

    let has_sensitive_content = messages.iter().any(|msg| {
        let content = msg.content.to_lowercase();
        content.contains("delete")
            || content.contains("drop")
            || content.contains("remove")
            || content.contains("sensitive")
            || content.contains("confidential")
    });

    let history_requires_escalation = if let Some(conv_id) = conversation_id {
        check_conversation_history_escalation(server, conv_id).await?
    } else {
        false
    };

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

// ── Agent filtering and inference ─────────────────────────────────────

async fn filter_runtime_ready_agents(
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

fn infer_adaptive_phase(
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

fn controller_recommended_phase(
    server: &AcpServer,
    config: &crate::config::AppConfig,
    mode: &str,
) -> Option<String> {
    let candidates = config.flow.phases.clone();
    let recommended = server
        .online_controller
        .lock()
        .ok()
        .and_then(|ctrl| ctrl.recommend_phase(&candidates))?;

    if recommended == "review" && !mode.eq_ignore_ascii_case("review") {
        return None;
    }
    if has_flow_phase(config, &recommended) {
        Some(recommended)
    } else {
        None
    }
}

// ── Private types ─────────────────────────────────────────────────────

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

/// Stream frame for SSE-based streaming
#[derive(Debug, Clone, Serialize)]
pub(crate) struct StreamFrame {
    pub event: String,
    pub payload: Value,
}

/// Observer for streaming chat responses
#[derive(Debug, Clone)]
pub(crate) struct StreamObserver {
    jsonrpc_response_id: Option<Value>,
    sse_sender: Option<mpsc::Sender<StreamFrame>>,
}

impl StreamObserver {
    pub(crate) fn jsonrpc(response_id: Option<Value>) -> Self {
        Self {
            jsonrpc_response_id: response_id,
            sse_sender: None,
        }
    }

    pub(crate) fn sse(sender: mpsc::Sender<StreamFrame>) -> Self {
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

/// A detected repeated task pattern in a conversation.
/// Used by P3 to proactively propose skill creation.
#[allow(dead_code)] // F-GAP-17
struct DetectedTaskPattern {
    name: String,
    description: String,
    occurrence_count: usize,
    keywords: Vec<String>,
}

// ── Main chat processing ──────────────────────────────────────────────

/// Process chat request
pub(crate) async fn process_chat_request(
    server: &AcpServer,
    params: &ChatParams,
    stream_observer: Option<StreamObserver>,
    trace: &RequestTraceContext,
    span: Option<&OtelContext>,
    ctx: Option<ChatRequestContext>,
) -> Result<serde_json::Value> {
    let started = std::time::Instant::now();

    let ctx = ctx.unwrap_or_else(|| ChatRequestContext::new(None));

    let (flow, registry) = routing_handles(server)?;

    // ── HarnessBus pre-route policy evaluation ─────────────────────────
    if let Some(ref harness) = server.harness_bus {
        if let Ok(mut budget) = harness.evaluator.budget.lock() {
            budget.reset();
        }
        let task_ctx = crate::governance::pua::TaskContext {
            task_type: crate::governance::pua::TaskType::Other,
            file_count: params.messages.len(),
            risk_score: 0.3,
        };
        let verdict = harness.evaluate(&task_ctx);
        match &verdict {
            crate::governance::harness_bus::PolicyVerdict::Deny(v) => {
                anyhow::bail!("harness policy denied: {}", v.detail);
            }
            crate::governance::harness_bus::PolicyVerdict::Escalate(r) => {
                warn!("harness policy escalation: {}", r.reason);
            }
            crate::governance::harness_bus::PolicyVerdict::Review(r) => {
                info!("harness policy flagged for review: {}", r.reason);
            }
            _ => {
                warn!("unexpected PolicyVerdict variant in gate evaluation");
            }
        }
    }

    // ── HarnessBus token gate evaluation (ARCH-04) ─────────────────────
    if let Some(ref harness) = server.harness_bus {
        let input_chars: usize = params.messages.iter().map(|m| m.content.len()).sum();
        let estimated_input = (input_chars / 4).max(1) as u64;
        let gate_ctx = crate::orchestration::token_layers::GateContext {
            request_id: trace.request_id.clone(),
            estimated_input_tokens: estimated_input,
            estimated_output_tokens: estimated_input / 2,
            keywords: vec![],
            has_cache_hit: false,
            confidence_score: 0.8,
            request_text: String::new(),
            max_input_tokens: None,
            max_output_tokens: None,
        };
        let verdict = harness.evaluate_token_gate(&gate_ctx);
        if matches!(
            verdict,
            crate::orchestration::token_layers::TokenGateVerdict::Reject(_)
        ) {
            let reason = match verdict {
                crate::orchestration::token_layers::TokenGateVerdict::Reject(r) => r,
                _ => "token gate rejected".to_string(),
            };
            anyhow::bail!("token gate L0 rejected request: {}", reason);
        }
        debug!("token gate verdict: {:?}", verdict);
    }

    // ── TenantBudgetEnforcer pre-route check (F-GAP-08) ───────────────
    let tenant_id = &ctx.tenant_id;
    if let Ok(mut budget) = server.tenant_budget.lock() {
        if server.runtime_config.production_strict {
            if let Err(e) = budget.check_can_start(tenant_id) {
                warn!("tenant budget limit reached for {}: {}", tenant_id, e);
                return Err(anyhow::anyhow!(
                    "tenant '{}' at resource limit: {}",
                    tenant_id,
                    e
                ));
            }
        } else {
            if let Err(e) = budget.check_can_start(tenant_id) {
                warn!(
                    "tenant budget limit reached for {}: {} (non-strict, allowing)",
                    tenant_id, e
                );
            }
        }
        budget.start_task(tenant_id);
    }

    // ── SchemaRegistry task envelope validation (F-GAP-07) ─────────────
    let mut schema_warnings: Vec<String> = Vec::new();
    let mut schema_error: Option<String> = None;
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

    let mut resolved = match flow.resolve(chosen_phase.clone(), registry.as_ref()) {
        Ok(r) => r,
        Err(_) => {
            warn!(
                "chat: phase '{:?}' not found in flow config, falling back to default",
                chosen_phase
            );
            flow.resolve(None, registry.as_ref())?
        }
    };
    let original_count = resolved.agents.len();
    let unavailable_agents =
        filter_runtime_ready_agents(server, app_config.as_ref(), &mut resolved.agents).await;
    if resolved.agents.is_empty() {
        resolved = flow.resolve(chosen_phase.clone(), registry.as_ref())?;
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
    };
    let phase = resolved.phase.clone();
    let phase_name = &phase.phase_name;
    reorder_chat_agents_by_runtime_score(server, phase_name, &mut resolved.agents);

    // ── SchemaRegistry task envelope validation (F-GAP-07) ─────────────
    if let Ok(sr) = server.schema_registry.lock() {
        for (role_name, _agent) in &resolved.agents {
            if let Some(schema) = sr.get(role_name) {
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
    }

    // ── PromptLayers assembly (ARCH-03) ────────────────────────────────
    let prompt_segments = vec![
        crate::orchestration::prompt_layers::PromptSegment {
            layer: crate::orchestration::prompt_layers::PromptLayer::L1SystemPrompt,
            content: format!(
                "You are a helpful assistant operating in phase '{}' with mode '{}'.",
                phase_name, params.mode
            ),
            priority: 100,
        },
        crate::orchestration::prompt_layers::PromptSegment {
            layer: crate::orchestration::prompt_layers::PromptLayer::L2RoleIdentity,
            content: format!(
                "Your role: {}",
                resolved
                    .agents
                    .first()
                    .map(|(name, _)| name.as_str())
                    .unwrap_or("general")
            ),
            priority: 200,
        },
    ];
    let layered_prompt = PromptAssembler::assemble(prompt_segments);
    debug!(
        "assembled layered prompt with {} segments (~{} tokens)",
        layered_prompt.segments.len(),
        layered_prompt.token_estimate
    );

    let mut capability_selected_agent: Option<String> = None;
    let mut capability_recommended_mode: Option<String> = None;
    #[cfg(feature = "sub-bus-optimization")]
    let mut capability_optimization_hint: Option<Value> = None;
    #[cfg(not(feature = "sub-bus-optimization"))]
    let capability_optimization_hint: Option<Value> = None;

    let capability_risk_policy = build_risk_vote_policy(&HashMap::new());
    let capability_risk = assess_high_risk(&params.messages, &params.mode, &capability_risk_policy);

    // ── CapabilityBus agent selection ──────────────────────────────────
    if let Some(ref cb) = server.capability_bus {
        let task_ctx = crate::governance::pua::TaskContext {
            task_type: crate::governance::pua::TaskType::Other,
            file_count: params.messages.len(),
            risk_score: (capability_risk.score as f64 / 4.0).clamp(0.1, 1.0),
        };
        let sensing = cb.sense(&task_ctx);
        let decision = cb.decide(&task_ctx, &sensing);
        capability_selected_agent = decision.selected_agent.clone();
        capability_recommended_mode = Some(decision.recommended_mode.clone());

        #[cfg(feature = "sub-bus-optimization")]
        {
            let opt = cb.optimization_recommendation(
                phase_name,
                (params.messages.len() as u64).saturating_mul(512),
                if params.mode.eq_ignore_ascii_case("full_auto") {
                    "high"
                } else {
                    "balanced"
                },
            );
            capability_optimization_hint = Some(serde_json::json!({
                "suggested_agent": opt.suggested_agent,
                "estimated_cost": opt.estimated_cost,
                "estimated_duration_ms": opt.estimated_duration_ms,
                "reliability_score": opt.reliability_score,
                "confidence": opt.confidence,
            }));
        }

        if let Some(ref agent) = decision.selected_agent {
            if resolved.agents.iter().any(|(name, _)| name == agent) {
                resolved.agents.retain(|(name, _)| name == agent);
            }
            let _ = reorder_agents_with_priority(&mut resolved.agents, agent);
        }
        cb.record_event(
            "sense",
            decision.selected_agent.clone(),
            Some(trace.request_id.clone()),
            "success",
            serde_json::json!({
                "candidate_count": sensing.capability_agent_count,
                "confidence": decision.confidence,
                "duration_ms": decision.duration_ms,
                "recommended_mode": decision.recommended_mode,
                "high_risk": capability_risk.is_high_risk,
                "risk_reasons": capability_risk.reasons,
                "optimization": capability_optimization_hint,
            }),
        );
    }

    let configured_primary_agent = phase
        .agent_names
        .first()
        .cloned()
        .or_else(|| resolved.agents.first().map(|(name, _)| name.clone()));
    let preferred_agent_from_request = params
        .options
        .as_ref()
        .and_then(|opts| opts.extra.get("preferred_agent"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());

    if let Some(primary) = configured_primary_agent.as_ref() {
        if let Ok(mut state) = agent_switch_state().lock() {
            state
                .primary_agent_by_phase
                .insert(phase_name.clone(), primary.clone());
        }
    }

    if let Some(preferred) = preferred_agent_from_request.as_deref() {
        if reorder_agents_with_priority(&mut resolved.agents, preferred) {
            if let Ok(mut state) = agent_switch_state().lock() {
                state
                    .forced_agent_by_phase
                    .insert(phase_name.clone(), preferred.to_string());
            }
        }
    } else if let Ok(state) = agent_switch_state().lock() {
        if let Some(forced) = state.forced_agent_by_phase.get(phase_name) {
            let primary = state.primary_agent_by_phase.get(phase_name);
            if let Some(primary_name) = primary {
                let _ = reorder_agents_with_priority(&mut resolved.agents, forced);
                let _ = reorder_agents_with_priority(&mut resolved.agents, primary_name);
            }
        }
    }

    // Phase-level rate limiter support
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
                .map(|guard| guard.allow(phase_name, rpm_limit, burst))
                .unwrap_or_else(|e| {
                    warn!("rate limiter lock failed: {e}");
                    true
                });
            if !allowed {
                let burst_str = burst
                    .map(|b| b.to_string())
                    .unwrap_or_else(|| "none".to_string());
                anyhow::bail!(
                    "rate limited for phase '{}' (rpm={}, burst={})",
                    phase_name,
                    rpm_limit,
                    burst_str
                );
            }
        }
    }

    // Get or create conversation state
    let raw_conversation_id = params.conversation_id.clone().unwrap_or_else(|| {
        format!(
            "conv_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
    });
    let conversation_id = if server.runtime_config.user_auth_enabled {
        format!("{}:{}", tenant_id, raw_conversation_id)
    } else {
        raw_conversation_id
    };
    let branch_id = params
        .branch_id
        .clone()
        .unwrap_or_else(|| "main".to_string());

    // Requirement contract
    let _requirement_contract = if let Some(contract) = &params.requirement_contract {
        contract.clone()
    } else {
        let task_description = extract_task_description(&params.messages);
        default_requirement_contract(&task_description, "chat")
    };

    // Task plan
    let _plan = if let Some(existing_plan) = &params.plan {
        existing_plan.clone()
    } else {
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

    let vector_context =
        load_vector_context(server, phase_name, phase.options.as_ref(), params).await;
    let agent_messages = merge_context_into_messages(
        &params.messages,
        build_vector_context_message(
            vector_context.summary.as_deref(),
            &vector_context.hits,
            &vector_context.knowledge,
        ),
    );

    // ── StartupContext injection ───────────────────────────────────────
    let agent_messages = {
        if let Some(ctx) = crate::orchestration::startup_context::get() {
            let summary = crate::orchestration::startup_context::summary_text(&ctx);
            if !summary.is_empty() {
                let startup_msg = format!("[startup context]\n{}", summary);
                merge_context_into_messages(&agent_messages, Some(startup_msg))
            } else {
                agent_messages
            }
        } else {
            agent_messages
        }
    };

    // ── Skill system prompt enhancement ────────────────────────────────
    let agent_messages = {
        if let Ok(registry) = server.skill_registry.lock() {
            let skill_count = registry.list().len();
            let skill_instruction = format!(
                r#"

## Skill System

You have access to {} registered skill(s). Skills are reusable templates that automate common tasks.

### How to Use Skills
1. **Discover Local** — Call `skill-finder(query, top_k)` to search LOCAL skills matching the user's intent.
2. **Search GitHub** — Call `github_search_skills(query, max_results)` to search GitHub for community skills.
3. **Import** — Once you find a suitable GitHub repo, use `import_skill` with {{ "source": {{ "kind": "github", "repo": "owner/repo", "ref": "main" }} }} to install it.
4. **Create** — If no existing skill fits, create one via `skill-creator(name, description, prompt_template, input_schema)`.

### Important Rules
- When multiple skills seem relevant, pick the one with the HIGHEST score.
- When a user request could match several skills, call `skill-finder` first, then choose the single best match.
- If the best match has score < 0.4, do NOT use it. Instead, ask the user for clarification or create a new skill.
- NEVER call multiple skills at once for the same request. Pick one and execute it."#,
                skill_count
            );
            merge_context_into_messages(&agent_messages, Some(skill_instruction))
        } else {
            agent_messages
        }
    };

    let mut selected_agent = String::new();
    let mut response_text = String::new();
    let mut reasoning_text = String::new();
    let mut selected_model_name: Option<String> = None;
    let mut last_err: Option<anyhow::Error> = None;
    let candidate_agents = resolved
        .agents
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    let mut quota_failed_agents: Vec<String> = Vec::new();
    let mut agent_attempts: Vec<Value> = Vec::new();
    let mut cache_hit = false;

    // ── Scheduler task submission (ARCH-02) ────────────────────────────
    let sched_task_id = trace.request_id.clone();
    if let Some(ref sched) = server.scheduler {
        let submitted_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let primary_role = resolved
            .agents
            .first()
            .map(|(n, _)| n.clone())
            .unwrap_or_else(|| "general".to_string());
        let task = crate::orchestration::scheduler::ScheduledTask {
            task_id: sched_task_id.clone(),
            role: primary_role,
            priority: crate::orchestration::scheduler::Priority(100),
            base_score: 1.0,
            urgency: 0.5,
            cost_efficiency: 0.8,
            deadline_pressure: 0.0,
            aging_bonus: 0.0,
            submitted_at,
            retries: 0,
            max_retries: 3,
        };
        if let Err(e) = sched.level1.submit(task) {
            tracing::warn!("scheduler submit failed: {}", e);
        }
    }

    // ── Token cache lookup ──────────────────────────────────────────────
    if cache_short_circuit_allowed(params) {
        let input_text = crate::intelligence::token_cache::messages_to_text(&agent_messages);
        let estimated_tokens =
            crate::intelligence::token_cache::estimate_messages_token_count(&agent_messages);
        let context_class = ContextLengthClass::from_token_count(estimated_tokens);

        if let Some((level, entry)) = server
            .cache
            .token_cache
            .lookup(&input_text, context_class)
            .await
        {
            let confidence = match level {
                crate::intelligence::token_cache::CacheLevel::L1 => 1.0,
                crate::intelligence::token_cache::CacheLevel::L2 => {
                    let input_vec =
                        crate::intelligence::token_cache::simple_embedding(&input_text);
                    let cached_vec =
                        crate::intelligence::token_cache::simple_embedding(&entry.input);
                    crate::intelligence::token_cache::cosine_similarity(&input_vec, &cached_vec)
                }
                crate::intelligence::token_cache::CacheLevel::L3 => {
                    if entry.output.len() > 50 {
                        0.96
                    } else {
                        0.0
                    }
                }
            };

            if confidence > 0.95 {
                tracing::info!(
                    target = "token_cache",
                    level = %level,
                    confidence,
                    agent_count = resolved.agents.len(),
                    "process_chat_request: token cache HIT, skipping agent execution"
                );
                cache_hit = true;
                selected_agent = resolved
                    .agents
                    .first()
                    .map(|(name, _)| name.clone())
                    .unwrap_or_else(|| "cached".to_string());
                response_text = entry.output.clone();

                if let Some(ref observer) = stream_observer {
                    let meta = StreamEventMeta {
                        agent_name: &selected_agent,
                        phase_name,
                        trace_id: &trace.trace_id,
                    };
                    let total_chars = response_text.chars().count();
                    emit_stream_chunk(
                        server,
                        Some(observer),
                        meta,
                        &response_text,
                        1,
                        total_chars,
                    )
                    .await?;
                    emit_stream_done(
                        server,
                        Some(observer),
                        meta,
                        1,
                        total_chars,
                        0u64,
                        None,
                    )
                    .await?;
                }

                agent_attempts.push(json!({
                    "agent": selected_agent,
                    "ok": true,
                    "cached": true,
                    "cache_level": format!("{level}"),
                    "duration_ms": 0u64
                }));
            }
        }
    } else {
        tracing::debug!(
            target = "token_cache",
            mode = %params.mode,
            "process_chat_request: bypass token-cache short-circuit for execution-oriented request"
        );
    }

    let mut base_agent_options = phase
        .options
        .as_ref()
        .and_then(|opts| opts.agent_options())
        .unwrap_or_default();
    if let Some(request_options) = params.options.as_ref() {
        for (key, value) in &request_options.extra {
            if key == "extra" {
                if let Some(obj) = value.as_object() {
                    for (k, v) in obj {
                        base_agent_options.insert(k.clone(), v.clone());
                    }
                }
            } else {
                base_agent_options.insert(key.clone(), value.clone());
            }
        }
    }

    // ── Inject registered skills as LLM-callable tools ──────────────────
    {
        if let Ok(registry) = server.skill_registry.lock() {
            let sanitize_fn_name = |name: &str| -> String {
                name.chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect::<String>()
            };
            let skill_tools: Vec<Value> = registry
                .list()
                .iter()
                .map(|skill: &SkillDescriptor| {
                    let safe_name = sanitize_fn_name(&skill.name);
                    let fallback_name = if safe_name.is_empty() {
                        format!("skill-{}", skill.name.len())
                    } else {
                        safe_name
                    };
                    json!({
                        "type": "function",
                        "function": {
                            "name": fallback_name,
                            "description": skill.description,
                            "parameters": skill.input_schema,
                        }
                    })
                })
                .collect();
            if !skill_tools.is_empty() {
                base_agent_options.insert("tools".to_string(), json!(skill_tools));
                base_agent_options.insert("tool_choice".to_string(), json!("auto"));
            }
        }
    }

    // ── Model-based agent routing ────────────────────────────────────
    let model_is_specific = base_agent_options
        .get("model")
        .and_then(|v| v.as_str())
        .is_some_and(|m| !m.is_empty() && m != "auto");

    if let Some(model_str) = base_agent_options.get("model").and_then(|v| v.as_str()) {
        if model_str == "copilot/auto" || model_str == "copilot-auto" || model_str == "copilot" {
            resolved
                .agents
                .retain(|(name, _)| name.eq_ignore_ascii_case("copilot"));
        }
    }

    if model_is_specific {
        let agents_before_model_filter = resolved.agents.clone();
        let model = base_agent_options
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let model_lower = model.to_ascii_lowercase();
        resolved.agents.retain(|(name, _)| {
            let name_lower = name.to_ascii_lowercase();
            if model_lower.starts_with(&name_lower) && model_lower.contains('/') {
                name_lower.starts_with(&model_lower)
                    || model_lower.ends_with(&format!("/{}", name_lower))
            } else {
                model_lower.starts_with(&name_lower) || name_lower.starts_with(&model_lower)
            }
        });

        if resolved.agents.is_empty() {
            warn!(
                model = %model,
                "model filter did not match any agent, falling back to phase candidate agents"
            );
            resolved.agents = agents_before_model_filter;
        }
    }

    let risk_policy = build_risk_vote_policy(&base_agent_options);
    let risk_assessment = assess_high_risk(&params.messages, &params.mode, &risk_policy);
    let enable_high_risk_vote =
        risk_policy.enabled && risk_assessment.is_high_risk && !model_is_specific;
    let enable_high_risk_multi_agent_vote = enable_high_risk_vote
        && option_bool(
            &base_agent_options,
            "high_risk_multi_agent_vote_enabled",
            true,
        );
    let min_vote_agents =
        option_usize(&base_agent_options, "high_risk_vote_min_agents", 2).clamp(1, 6);
    let max_vote_agents = option_usize(&base_agent_options, "high_risk_vote_max_agents", 3)
        .max(min_vote_agents)
        .clamp(min_vote_agents, 8);
    let escalation_enabled = option_bool(
        &base_agent_options,
        "high_risk_escalate_multi_model_enabled",
        true,
    );
    let escalation_models_per_agent = option_usize(
        &base_agent_options,
        "high_risk_escalate_models_per_agent",
        2,
    )
    .clamp(2, 6);
    let escalation_max_agents = option_usize(
        &base_agent_options,
        "high_risk_escalate_max_agents",
        max_vote_agents,
    )
    .clamp(1, max_vote_agents);

    let mut used_multi_model_vote = false;
    let mut used_multi_agent_vote = false;
    let mut review_required = false;
    let mut vote_report: Option<Value> = None;
    let mut agent_vote_candidates: Vec<AgentStrongVoteOutcome> = Vec::new();
    let mut agent_vote_failures: Vec<Value> = Vec::new();
    let mut agent_vote_sources: Vec<AgentVoteSource> = Vec::new();
    let mut emit_final_vote_response = false;

    let unhealthy_fallback_agent = if let Some(ref cb) = server.capability_bus {
        let healthy_count = resolved
            .agents
            .iter()
            .filter(|(name, _)| cb.is_agent_healthy(name))
            .count();
        if healthy_count == 0 {
            let selected = resolved.agents.first().map(|(name, _)| name.clone());
            if let Some(ref name) = selected {
                warn!(
                    phase = %phase_name,
                    fallback_agent = %name,
                    "all candidate agents unhealthy; forcing degraded fallback attempt"
                );
            }
            selected
        } else {
            None
        }
    } else {
        None
    };

    if !cache_hit {
        for (agent_name, agent) in resolved.agents {
            let attempt_started = std::time::Instant::now();

            if let Some(ref cb) = server.capability_bus {
                if !cb.is_agent_healthy(&agent_name) {
                    if unhealthy_fallback_agent.as_deref() == Some(agent_name.as_str()) {
                        warn!(
                            phase = %phase_name,
                            agent = %agent_name,
                            "executing unhealthy agent due to degraded fallback"
                        );
                    } else {
                        agent_attempts.push(json!({
                            "agent": agent_name,
                            "ok": false,
                            "skipped_unhealthy": true,
                            "duration_ms": 0u64,
                            "error": "agent unhealthy by capability bus"
                        }));
                        continue;
                    }
                }
            }

            let per_attempt_options = base_agent_options.clone();
            let model_name = per_attempt_options
                .get("model")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            if enable_high_risk_multi_agent_vote {
                if agent_vote_candidates.len() >= max_vote_agents {
                    continue;
                }

                let strong_model = if agent.supports_model_override() {
                    select_strong_model_id(agent.as_ref())
                } else {
                    None
                };

                let mut vote_options = per_attempt_options.clone();
                if let Some(model_id) = strong_model.clone() {
                    vote_options.insert("model".to_string(), Value::String(model_id));
                }

                match run_agent_collecting(
                    server,
                    StreamNotificationContext {
                        stream_observer: None,
                        agent_name: &agent_name,
                        phase_name,
                        trace_id: &trace.trace_id,
                    },
                    Arc::clone(&agent),
                    agent_messages.clone(),
                    phase.principles.clone(),
                    Some(vote_options),
                    request_timeout(phase.options.as_ref()),
                )
                .await
                {
                    Ok((output_text, reasoning_output, _sel_m))
                        if !output_text.trim().is_empty() =>
                    {
                        if let Ok(mut ctrl) = server.online_controller.lock() {
                            ctrl.record_agent_outcome(
                                phase_name,
                                &agent_name,
                                true,
                                attempt_started.elapsed().as_millis() as u64,
                            );
                        }
                        agent_attempts.push(json!({
                            "agent": agent_name,
                            "ok": true,
                            "duration_ms": attempt_started.elapsed().as_millis() as u64,
                            "risk_vote_mode": "strong_model",
                            "model": strong_model,
                        }));
                        agent_vote_candidates.push(AgentStrongVoteOutcome {
                            agent: agent_name.clone(),
                            model: strong_model,
                            response: output_text,
                            reasoning: reasoning_output,
                        });
                        agent_vote_sources.push((
                            agent_name.clone(),
                            Arc::clone(&agent),
                            per_attempt_options.clone(),
                        ));
                        continue;
                    }
                    Ok((_, _, _)) => {
                        agent_vote_failures.push(json!({
                            "agent": agent_name,
                            "reason": "empty_response",
                        }));
                        agent_attempts.push(json!({
                            "agent": agent_name,
                            "ok": false,
                            "duration_ms": attempt_started.elapsed().as_millis() as u64,
                            "risk_vote_mode": "strong_model",
                            "error": "empty_response",
                        }));
                        continue;
                    }
                    Err(err) => {
                        let err_text = err.to_string();
                        agent_vote_failures.push(json!({
                            "agent": agent_name,
                            "reason": err_text,
                        }));
                        agent_attempts.push(json!({
                            "agent": agent_name,
                            "ok": false,
                            "duration_ms": attempt_started.elapsed().as_millis() as u64,
                            "risk_vote_mode": "strong_model",
                            "error": err.to_string(),
                        }));
                        if last_err.is_none() {
                            last_err = Some(anyhow::anyhow!("{}: {}", agent_name, err));
                        }
                        continue;
                    }
                }
            }

            match run_agent_collecting(
                server,
                StreamNotificationContext {
                    stream_observer: stream_observer.clone(),
                    agent_name: &agent_name,
                    phase_name,
                    trace_id: &trace.trace_id,
                },
                agent,
                agent_messages.clone(),
                phase.principles.clone(),
                Some(per_attempt_options),
                request_timeout(phase.options.as_ref()),
            )
            .await
            {
                Ok((output_text, reasoning_output, agent_selected_model)) => {
                    if output_text.trim().is_empty() {
                        agent_attempts.push(json!({
                            "agent": agent_name,
                            "ok": false,
                            "duration_ms": attempt_started.elapsed().as_millis() as u64,
                            "error": "empty_response",
                        }));
                        if let Ok(mut ctrl) = server.online_controller.lock() {
                            ctrl.record_agent_outcome(
                                phase_name,
                                &agent_name,
                                false,
                                attempt_started.elapsed().as_millis() as u64,
                            );
                        }
                        continue;
                    }

                    if let Ok(mut ctrl) = server.online_controller.lock() {
                        ctrl.record_agent_outcome(
                            phase_name,
                            &agent_name,
                            true,
                            attempt_started.elapsed().as_millis() as u64,
                        );
                    }
                    agent_attempts.push(json!({
                        "agent": agent_name,
                        "ok": true,
                        "duration_ms": attempt_started.elapsed().as_millis() as u64,
                        "model": agent_selected_model,
                    }));
                    selected_agent = agent_name.clone();
                    response_text = output_text.clone();
                    reasoning_text = reasoning_output.clone();
                    if let Some(ref m) = agent_selected_model {
                        selected_model_name = Some(m.clone());
                    }

                    // ── Store result in token cache ─────────────────────
                    {
                        let input_text =
                            crate::intelligence::token_cache::messages_to_text(&agent_messages);
                        let token_count =
                            crate::intelligence::token_cache::estimate_token_count(&output_text);
                        let cache = server.cache.token_cache.clone();
                        let agent_name_for_cache = Some(agent_name.clone());
                        let cached_output = output_text.clone();
                        let model_name_clone = model_name.clone();
                        tokio::spawn(async move {
                            cache
                                .store(
                                    &input_text,
                                    &cached_output,
                                    token_count,
                                    agent_name_for_cache,
                                    model_name_clone,
                                )
                                .await;
                        });
                    }

                    last_err = None;
                    break;
                }
                Err(err) => {
                    let err_text = err.to_string();
                    let quota_limited = is_quota_or_token_limit_error(&err_text);
                    if quota_limited {
                        quota_failed_agents.push(agent_name.clone());
                    }
                    agent_attempts.push(json!({
                        "agent": agent_name,
                        "ok": false,
                        "quota_limited": quota_limited,
                        "duration_ms": attempt_started.elapsed().as_millis() as u64,
                        "error": err_text
                    }));
                    if let Ok(mut ctrl) = server.online_controller.lock() {
                        ctrl.record_agent_outcome(
                            phase_name,
                            &agent_name,
                            false,
                            attempt_started.elapsed().as_millis() as u64,
                        );
                    }
                    let agent_label = agent_name.clone();
                    let enriched_err = anyhow::anyhow!("{}: {}", agent_label, err);
                    last_err = Some(enriched_err);
                }
            }
        }
    }

    Ok(json!({
        "done": true,
        "response": response_text,
    }))
}
