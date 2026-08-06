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
    pub conversation_id: String,
    pub branch_id: String,
    pub agent_messages: Vec<Message>,
    pub layered_prompt_segments: usize,
    pub base_agent_options: HashMap<String, Value>,
    pub risk_policy: super::voting::RiskVotePolicy,
    pub risk_assessment: super::voting::RiskAssessment,
    pub enable_high_risk_multi_agent_vote: bool,
    pub min_vote_agents: usize,
    pub max_vote_agents: usize,
    pub escalation_enabled: bool,
    pub escalation_models_per_agent: usize,
    pub escalation_max_agents: usize,
    pub unhealthy_fallback_agent: Option<String>,
    pub fallback_reason: Option<String>,
    pub council_decision: Option<Value>,
    pub candidate_agents: Vec<String>,
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

    // ── Determine if user specified an explicit model ────────────────
    // When the user explicitly selects a model (not "auto"), skip the
    // capability bus agent selection and rely on filter_agents_by_model
    // to match the correct agent. This applies to GUI chat, Zed agent_servers,
    // and VS Code addon alike — only when BOTH agent and model are "auto"
    // does the capability bus scoring take effect.
    let user_model_specific = model_router::model_option_is_specific(
        params
            .options
            .as_ref()
            .and_then(|opts| opts.extra.get("model"))
            .and_then(|v| v.as_str()),
    );

    // ── CapabilityBus agent selection + vector context ───────────────
    // SKIP capability bus selection when a specific model was chosen by the
    // user. The selection (mutates `resolved.agents`) and the vector context
    // load (reads only `server` / `phase` / `params`) are independent, so run
    // them concurrently instead of serially.
    let capability_bus_future = async {
        if !user_model_specific {
            if let Some(ref cb) = server.governance_deps.capability_bus {
                let result =
                    crate::acp::helpers::capability_selector::apply_capability_bus_selection(
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
                (
                    result.capability_selected_agent,
                    result.recommended_mode,
                    Some(result.candidate_count as u64),
                    Some(result.confidence),
                    Some(result.capability_selection_reason),
                    result.optimization_hint,
                )
            } else {
                (None, None, None, None, None, None)
            }
        } else {
            routing_provenance.push("capability_bus_skipped_model_selected".to_string());
            (None, None, None, None, None, None)
        }
    };
    let vector_context_future =
        load_vector_context(server, phase_name, phase.options.as_ref(), params);
    let (capability_bus_result, vector_context) =
        tokio::join!(capability_bus_future, vector_context_future);
    let (
        capability_selected_agent,
        capability_recommended_mode,
        capability_candidate_count,
        capability_decision_confidence,
        capability_selection_reason,
        capability_optimization_hint,
    ) = capability_bus_result;

    // ── Agent Switch State & Preferred Agent Resolution ──────────────
    let agent_prefs = crate::acp::helpers::agent_preference::resolve_agent_preferences(
        server, params, phase, resolved, tenant_id,
    )?;

    let configured_primary_agent = agent_prefs.configured_primary_agent;
    let conversation_id = agent_prefs.conversation_id;
    let branch_id = agent_prefs.branch_id;

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
            .read()
            .unwrap_or_else(|poisoned| {
                warn!("select_and_score_agents: skill_registry poisoned, recovering");
                poisoned.into_inner()
            });
        let skill_count = reg_guard.list(false).len();
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

    // When the user explicitly selected a model but no configured agent
    // matches, report the error directly instead of silently falling back
    // to all phase agents (which would use the wrong provider/model).
    if filter_result.model_is_specific && resolved.agents.is_empty() {
        let model_val = base_agent_options
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        anyhow::bail!(
            "{}",
            tf(
                "error.chat.model_no_matching_agent",
                &[("model", model_val)]
            )
        );
    }

    let risk_policy = build_risk_vote_policy(&base_agent_options);
    let risk_assessment = assess_high_risk(&params.messages, &params.mode, &risk_policy);
    let vote_config = model_router::build_high_risk_vote_config(
        &base_agent_options,
        &risk_policy,
        &risk_assessment,
        filter_result.model_is_specific,
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
        conversation_id,
        branch_id,
        agent_messages,
        layered_prompt_segments: layered_prompt.segments.len(),
        base_agent_options,
        risk_policy,
        risk_assessment,
        enable_high_risk_multi_agent_vote: vote_config.enable_high_risk_multi_agent_vote,
        min_vote_agents: vote_config.min_vote_agents,
        max_vote_agents: vote_config.max_vote_agents,
        escalation_enabled: vote_config.escalation_enabled,
        escalation_models_per_agent: vote_config.escalation_models_per_agent,
        escalation_max_agents: vote_config.escalation_max_agents,
        unhealthy_fallback_agent,
        fallback_reason,
        council_decision,
        candidate_agents,
        vector_context,
    })
}

/// Outcome of the autonomy loop execution.
pub(crate) struct AutonomyOutcome {
    pub autonomy_loop_executed: bool,
    pub selected_agent: String,
    pub response_text: String,
    pub agent_attempts: Vec<Value>,
    /// True when tools were requested but ALL of them failed.
    /// The caller can use this to distinguish "task failed" from "task succeeded".
    pub all_tools_failed: bool,
}
