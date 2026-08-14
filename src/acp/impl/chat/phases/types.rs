//! Shared phase result types (Observe/Think/Act) — consumed by every phase
//! and by the pipeline that sequences them.

use std::collections::HashMap;

use serde_json::Value;

use crate::acp::r#impl::chat::{RiskAssessment, RiskVotePolicy, VectorContext};
use crate::agent::Message;
use crate::orchestration::flow::ResolvedPhase;

pub(crate) struct ObserveOutput {
    pub tenant_id: String,
    pub user_id: Option<String>,
    pub phase: ResolvedPhase,
    pub phase_name: String,
    pub phase_origin: String,
    pub resolved: crate::orchestration::flow::ResolvedRouting,
    pub schema_warnings: Vec<String>,
    pub schema_error: Option<String>,
    pub routing_provenance: Vec<String>,
    pub reputation_scores: HashMap<String, f64>,
    pub multimodal_context: Option<String>,
}

/// Collected output from the think phase.
pub(crate) struct ThinkOutput {
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
    pub risk_policy: RiskVotePolicy,
    pub risk_assessment: RiskAssessment,
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

/// Collected output from the act phase.
pub(crate) struct ActOutput {
    pub selected_agent: String,
    pub response_text: String,
    pub reasoning_text: String,
    pub selected_model_name: Option<String>,
    pub last_err: Option<anyhow::Error>,
    pub cache_hit: bool,
    pub cache_bypassed_for_execution: bool,
    pub agent_attempts: Vec<Value>,
    pub quota_failed_agents: Vec<String>,
    pub vote_winner: Option<String>,
    pub vote_report: Option<Value>,
    pub used_multi_model_vote: bool,
    pub used_multi_agent_vote: bool,
    pub review_required: bool,
    pub review_blocked: bool,
    pub checkpoint: crate::acp::ConversationCheckpoint,
    pub knowledge: Value,
    pub metacognitive_loop: Value,
    pub distillation: Value,
    /// True when tools were requested but ALL of them failed.
    pub all_tools_failed: bool,
}
