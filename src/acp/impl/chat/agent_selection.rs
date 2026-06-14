//! Agent selection and scoring for ACP chat
//!
//! Contains the agent selection and scoring logic used during chat request
//! processing. Extracted from the parent `chat.rs` to reduce the monolithic file size.

use std::collections::HashMap;

use crate::acp::helpers::model_router;
use crate::acp::server::AcpServer;
use crate::agent::Message;
use crate::i18n::runtime::tf;
use anyhow::Result;
use serde_json::Value;
use tracing::{debug, warn};

use super::voting::{assess_high_risk, build_risk_vote_policy};
use super::{
    build_vector_context_message, load_vector_context, merge_context_into_messages, ChatParams,
    RequestTraceContext, VectorContext,
};

use crate::orchestration::prompt_layers::PromptAssembler;

/// Outcome of the agent selection & scoring step.
#[allow(clippy::too_many_arguments)]
pub(crate) struct AgentSelectionOutcome {
    pub capability_selected_agent: Option<String>,
    pub capability_recommended_mode: Option<String>,
    pub capability_candidate_count: Option<u64>,
    pub capability_decision_confidence: Option<f64>,
    pub capability_selection_reason: Option<String>,
    pub capability_optimization_hint: Option<Value>,
    pub configured_primary_agent: Option<String>,
    pub preferred_agent_from_request: Option<String>,
    pub conversation_id: String,
    pub branch_id: String,
    pub agent_messages: Vec<Message>,
    pub layered_prompt_segments: usize,
    pub base_agent_options: HashMap<String, Value>,
    pub risk_policy: super::voting::RiskVotePolicy,
    pub risk_assessment: super::voting::RiskAssessment,
    pub enable_high_risk_vote: bool,
    pub enable_high_risk_multi_agent_vote: bool,
    pub min_vote_agents: usize,
    pub max_vote_agents: usize,
    pub escalation_enabled: bool,
    pub escalation_models_per_agent: usize,
    pub escalation_max_agents: usize,
    pub _model_is_specific: bool,
    pub unhealthy_fallback_agent: Option<String>,
    pub fallback_reason: Option<String>,
    pub council_decision: Option<Value>,
    pub candidate_agents: Vec<String>,
    pub _routing_provenance: Vec<String>,
    pub vector_context: VectorContext,
}

// Select and score agents using CapabilityBus, agent preferences, and model routing.
//
// This function performs all agent selection logic including:
// - CapabilityBus sense/decide pipeline
// - Agent Switch State & Preferred Agent Resolution
// - PromptLayers assembly
// - Vector context loading
// - Model-based filtering and risk assessment
// - Council deliberation for unhealthy fallback
// ============================================================================
// Section: Agent Selection & Scoring
// ============================================================================

#[allow(clippy::too_many_arguments)]
pub(crate) async fn select_and_score_agents(
    server: &AcpServer,
    params: &ChatParams,
    resolved: &mut crate::orchestration::flow::ResolvedRouting,
    phase: &crate::orchestration::flow::ResolvedPhase,
    phase_name: &str,
    tenant_id: &str,
    trace: &RequestTraceContext,
    routing_provenance: &mut Vec<String>,
    reputation_scores: &HashMap<String, f64>,
) -> Result<AgentSelectionOutcome> {
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

    let capability_risk_policy = build_risk_vote_policy(&HashMap::new());
    let capability_risk = assess_high_risk(&params.messages, &params.mode, &capability_risk_policy);

    // ── CapabilityBus agent selection ──────────────────────────────────
    let mut capability_selected_agent: Option<String> = None;
    let mut capability_recommended_mode: Option<String> = None;
    let mut capability_candidate_count: Option<u64> = None;
    let mut capability_decision_confidence: Option<f64> = None;
    let mut capability_selection_reason: Option<String> = None;
    let mut capability_optimization_hint: Option<Value> = None;
    if let Some(ref cb) = server.governance_deps.capability_bus {
        let result = crate::acp::helpers::capability_selector::apply_capability_bus_selection(
            cb,
            phase_name,
            &params.messages,
            &params.mode,
            &mut resolved.agents,
            &capability_risk,
            &trace.request_id,
            routing_provenance,
        )
        .await;
        capability_selected_agent = result.capability_selected_agent;
        capability_recommended_mode = result.recommended_mode;
        capability_candidate_count = Some(result.candidate_count as u64);
        capability_decision_confidence = Some(result.confidence);
        capability_selection_reason = Some(result.capability_selection_reason);
        capability_optimization_hint = result.optimization_hint;
    }

    // ── Agent Switch State & Preferred Agent Resolution ──────────────
    let agent_prefs = crate::acp::helpers::agent_preference::resolve_agent_preferences(
        server, params, phase, resolved, tenant_id,
    )?;

    let configured_primary_agent = agent_prefs.configured_primary_agent;
    let conversation_id = agent_prefs.conversation_id;
    let branch_id = agent_prefs.branch_id;
    let preferred_agent_from_request = agent_prefs.preferred_agent_from_request;

    // ── Vector context & message assembly ─────────────────────────────
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
        let reg_guard = server
            .orchestration_deps
            .skill_registry
            .lock()
            .unwrap_or_else(|poisoned| {
                warn!("select_and_score_agents: skill_registry poisoned, recovering");
                poisoned.into_inner()
            });
        let skill_count = reg_guard.list().len();
        let skill_instruction = tf(
            "prompts.skill_system",
            &[("count", &skill_count.to_string())],
        );
        merge_context_into_messages(&agent_messages, Some(skill_instruction))
    };

    // ── Model-based agent routing / Filtering ──────────────────────
    let base_agent_options =
        crate::acp::helpers::agent_options::assemble_agent_options(server, phase, params);

    let filter_result =
        model_router::filter_agents_by_model(&mut resolved.agents, &base_agent_options);

    let risk_policy = build_risk_vote_policy(&base_agent_options);
    let risk_assessment = assess_high_risk(&params.messages, &params.mode, &risk_policy);
    let vote_config = model_router::build_high_risk_vote_config(
        &base_agent_options,
        &risk_policy,
        &risk_assessment,
        filter_result.model_is_specific,
    );

    eprintln!(
        "DEBUG agent_selection: resolved.agents={:?}, capability_selected={:?}",
        resolved
            .agents
            .iter()
            .map(|(n, _)| n.clone())
            .collect::<Vec<_>>(),
        capability_selected_agent
    );
    let candidate_agents = resolved
        .agents
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();

    // ── Council deliberation ───────────────────────────────────────────
    let (unhealthy_fallback_agent, fallback_reason, council_decision) =
        crate::acp::helpers::council_deliberation::run_council_deliberation_and_fallback(
            server
                .governance_deps
                .capability_bus
                .as_ref()
                .map(|arc| arc.as_ref()),
            risk_assessment.is_high_risk,
            filter_result.model_is_specific,
            &mut resolved.agents,
            &base_agent_options,
            phase_name,
            reputation_scores,
            routing_provenance,
        );

    Ok(AgentSelectionOutcome {
        capability_selected_agent,
        capability_recommended_mode,
        capability_candidate_count,
        capability_decision_confidence,
        capability_selection_reason,
        capability_optimization_hint,
        configured_primary_agent,
        preferred_agent_from_request,
        conversation_id,
        branch_id,
        agent_messages,
        layered_prompt_segments: layered_prompt.segments.len(),
        base_agent_options,
        risk_policy,
        risk_assessment,
        enable_high_risk_vote: vote_config.enable_high_risk_vote,
        enable_high_risk_multi_agent_vote: vote_config.enable_high_risk_multi_agent_vote,
        min_vote_agents: vote_config.min_vote_agents,
        max_vote_agents: vote_config.max_vote_agents,
        escalation_enabled: vote_config.escalation_enabled,
        escalation_models_per_agent: vote_config.escalation_models_per_agent,
        escalation_max_agents: vote_config.escalation_max_agents,
        _model_is_specific: filter_result.model_is_specific,
        unhealthy_fallback_agent,
        fallback_reason,
        council_decision,
        candidate_agents,
        _routing_provenance: routing_provenance.clone(),
        vector_context,
    })
}

/// Outcome of the autonomy loop execution.
pub(crate) struct AutonomyOutcome {
    pub autonomy_loop_executed: bool,
    pub selected_agent: String,
    pub response_text: String,
    pub _reasoning_text: String,
    pub _selected_model_name: Option<String>,
    pub agent_attempts: Vec<Value>,
}
