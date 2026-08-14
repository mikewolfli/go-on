//! Phase module: think.
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

use super::observe::is_simple_chat;
// Phase 2: Think
// ═════════════════════════════════════════════════════════════════════

/// Phase 2: Think about the situation: model resolution, agent selection,
/// routing, planning, capability analysis, risk assessment, metacognitive evaluation.
pub(crate) async fn think_phase(
    server: &AcpServer,
    params: &ChatParams,
    resolve_out: &mut ObserveOutput,
    trace: &RequestTraceContext,
) -> Result<ThinkOutput> {
    let agent_sel = select_and_score_agents(
        server,
        params,
        &mut resolve_out.resolved,
        &resolve_out.phase,
        &resolve_out.phase_name,
        &resolve_out.tenant_id,
        trace,
        &mut resolve_out.routing_provenance,
        &resolve_out.reputation_scores,
    )
    .await?;

    let mut agent_messages = agent_sel.agent_messages;

    // Inject multimodal context
    if let Some(ctx_text) = &resolve_out.multimodal_context {
        agent_messages.insert(
            0,
            Message {
                role: "system".to_string(),
                content: ctx_text.clone(),
            },
        );
    }

    // AgentMemoryBus — inject relevant memories into context
    // Codex-style: skip for simple chat — no task context to recall
    let is_simple = is_simple_chat(params);
    if !is_simple {
        inject_agent_memory_bus(
            server,
            resolve_out.user_id.as_deref(),
            &resolve_out.phase_name,
            agent_sel.capability_selected_agent.as_deref(),
            &params.messages,
            &mut agent_messages,
        )
        .await;
    }

    Ok(ThinkOutput {
        capability_selected_agent: agent_sel.capability_selected_agent,
        capability_recommended_mode: agent_sel.capability_recommended_mode,
        capability_candidate_count: agent_sel.capability_candidate_count,
        capability_decision_confidence: agent_sel.capability_decision_confidence,
        capability_selection_reason: agent_sel.capability_selection_reason,
        capability_optimization_hint: agent_sel.capability_optimization_hint,
        configured_primary_agent: agent_sel.configured_primary_agent,
        conversation_id: agent_sel.conversation_id,
        branch_id: agent_sel.branch_id,
        agent_messages,
        layered_prompt_segments: agent_sel.layered_prompt_segments,
        base_agent_options: agent_sel.base_agent_options,
        risk_policy: agent_sel.risk_policy,
        risk_assessment: agent_sel.risk_assessment,
        enable_high_risk_multi_agent_vote: agent_sel.enable_high_risk_multi_agent_vote,
        min_vote_agents: agent_sel.min_vote_agents,
        max_vote_agents: agent_sel.max_vote_agents,
        escalation_enabled: agent_sel.escalation_enabled,
        escalation_models_per_agent: agent_sel.escalation_models_per_agent,
        escalation_max_agents: agent_sel.escalation_max_agents,
        unhealthy_fallback_agent: agent_sel.unhealthy_fallback_agent,
        fallback_reason: agent_sel.fallback_reason,
        council_decision: agent_sel.council_decision,
        candidate_agents: agent_sel.candidate_agents,
        vector_context: agent_sel.vector_context,
    })
}

async fn inject_agent_memory_bus(
    _server: &AcpServer,
    user_id: Option<&str>,
    phase_name: &str,
    agent_name: Option<&str>,
    messages: &[Message],
    agent_messages: &mut Vec<Message>,
) {
    use crate::memory::agent_memory_bus::{AgentMemoryBus, AGENT_MEMORY_BUS};
    if let Some(memory_ctx) = AGENT_MEMORY_BUS
        .get_or_init(AgentMemoryBus::new_default)
        .retrieve_context_for_agent(
            agent_name.unwrap_or("unknown"),
            phase_name,
            &extract_task_description(messages),
            5,
            user_id,
        )
        .await
    {
        agent_messages.insert(
            0,
            Message {
                role: "system".to_string(),
                content: format!("[AgentMemoryBus context]\n{}", memory_ctx),
            },
        );
    }
}

// ═════════════════════════════════════════════════════════════════════
// Phase 3: Act
// ═════════════════════════════════════════════════════════════════════

/// Phase 3: Execute actions: LLM calls, tool execution, autonomy loop,
/// fallback, vote, cache operations, scheduler.
