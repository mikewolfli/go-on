//! Chat handling implementation functions for ACP server
//!
//! This module contains standalone functions that implement chat handling
//! functionality previously in the `impl AcpServer` block in `impl/chat.rs`.
//! These functions take `AcpServer` as their first parameter to maintain
//! compatibility with the original implementation.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::Instant;
use std::{fs, path::Path};

use anyhow::Result;
use opentelemetry::{Context as OtelContext, KeyValue};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::Duration;
use tracing::{debug, info, warn};

use crate::acp::helpers::agent_selector::{collect_reputation_scores, AgentSelector};
use crate::acp::helpers::autonomy::{
    planner_guided_tool_preferences, run_followup_after_tool_observation,
};
use crate::acp::helpers::autonomy_metrics::{
    record_agent_switch, record_autonomy_loop_stop_reason, record_explicit_tool_route,
    record_planner_guided_route, record_reputation_routing_applied, record_tool_followup_attempt,
    record_tool_followup_fallback, record_tool_followup_success,
};
use crate::acp::helpers::cache_strategy::{
    should_bypass_for_execution, store_async, CacheDecision, CacheStrategy,
};
use crate::acp::helpers::context::{
    probe_agent_runtime_readiness, request_timeout, run_with_optional_timeout,
    AgentRuntimeReadiness,
};
use crate::acp::helpers::conversation::stream_would_exceed_limits;
use crate::acp::helpers::metrics::{stream_chunk_notification, stream_done_notification};
use crate::acp::helpers::model_router;
use crate::acp::helpers::response_assembler::{
    build_chat_response, build_role_routing, build_task_graph_checkpoint, ChatResponseContext,
};
use crate::acp::helpers::review_gate::{run_enhanced_verification, run_review_gate};
use crate::acp::r#impl::UserSession;
use crate::acp::server::AcpServer;
use crate::agent::Message;
use crate::config::PhaseOptions;
use crate::evaluation::TraceEvent;
use crate::flow::FlowManager;
use crate::i18n::runtime::{t, tf};
use crate::intelligence::token_cache::ContextLengthClass;
use crate::orchestration::autonomy_runtime::{
    build_tool_execution_followup_message, build_tool_result_block,
};

use crate::agents::sse_optimizer::SseBufferPool;
use crate::orchestration::prompt_layers::PromptAssembler;
use crate::orchestration::session_compressor::SessionCompressor;
use crate::orchestration::session_context::SessionContextManager;
use crate::orchestration::task_router::{TaskRouter, TaskType};
use crate::orchestration::tool::{execute_loop, LoopConfig, LoopDecision, ToolInput, ToolRegistry};

use crate::memory_module::{MemoryClass, MemoryEntry, MemoryPolicy, MemoryStore};
use crate::reinforcement::{
    build_task_plan, build_workflow_generated_artifact, persist_knowledge_insight_event,
    persist_workflow_generated, persist_workflow_learning_event, ArtifactLedger,
    ExecutionDecisionCandidate, KnowledgeBusArtifact, KnowledgeInsightArtifact,
    RequirementContractArtifact, TaskPlanArtifact, WorkflowLearningEvent,
};
use crate::rpc_protocol::{chat_trace_context, child_trace_context, RequestTraceContext};

/// Chat parameters structure
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatParams {
    /// Chat mode (e.g., "ask", "edit", "agent", "safeguard", "full_auto").
    /// When absent or empty (e.g., from external clients like Zed),
    /// defaults to "ask" (the safest general-purpose mode).
    #[serde(default)]
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

/// Context for a chat request, including authentication and tenant info.
#[derive(Debug, Clone)]
pub struct ChatRequestContext {
    /// Authenticated user session, if user auth is enabled.
    #[allow(dead_code)] // F-GAP-49 — Public API — reserved for audit logging and in-chat RBAC
    pub user_session: Option<UserSession>,
    /// Resolved tenant ID (from user session, or conversation_id, or default).
    pub tenant_id: String,
}

impl ChatRequestContext {
    /// Create a new context with optional user session.
    pub fn new(user_session: Option<UserSession>) -> Self {
        let tenant_id = user_session
            .as_ref()
            .and_then(|s| s.tenant_id.clone())
            .unwrap_or_else(|| "default-tenant".to_string());
        Self {
            user_session,
            tenant_id,
        }
    }
}

#[derive(Default)]
struct AgentSwitchState {
    forced_agent_by_phase: HashMap<String, String>,
    #[allow(dead_code)] // F-GAP-17 — reserved for agent switch state extensibility
    primary_agent_by_phase: HashMap<String, String>,
}

static AGENT_SWITCH_STATE: OnceLock<StdMutex<AgentSwitchState>> = OnceLock::new();

fn agent_switch_state() -> &'static StdMutex<AgentSwitchState> {
    AGENT_SWITCH_STATE.get_or_init(|| StdMutex::new(AgentSwitchState::default()))
}

fn round_metric(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

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

pub(crate) fn is_quota_or_token_limit_error(error_text: &str) -> bool {
    let text = error_text.to_ascii_lowercase();
    text.contains("429")
        || text.contains("rate limit")
        || text.contains("quota")
        || text.contains("insufficient_quota")
        || text.contains("token") && text.contains("limit")
        || text.contains("token") && text.contains("exhaust")
        || text.contains("billing")
        || text.contains("credit") && text.contains("insufficient")
}

#[derive(Debug, Clone)]
pub(crate) struct RiskVotePolicy {
    pub(crate) enabled: bool,
    threshold: usize,
    domain_keywords: Vec<String>,
    decision_keywords: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct RiskAssessment {
    pub(crate) score: usize,
    pub(crate) is_high_risk: bool,
    pub(crate) reasons: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct AgentStrongVoteOutcome {
    pub(crate) agent: String,
    pub(crate) model: Option<String>,
    pub(crate) response: String,
    pub(crate) reasoning: String,
}

pub(crate) type AgentVoteSource = (String, Arc<dyn crate::agent::Agent>, HashMap<String, Value>);

pub(crate) fn option_bool(options: &HashMap<String, Value>, key: &str, default: bool) -> bool {
    options.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn option_usize(options: &HashMap<String, Value>, key: &str, default: usize) -> usize {
    options
        .get(key)
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(default)
}

fn option_keywords(options: &HashMap<String, Value>, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(value) = options.get(key) {
        if let Some(items) = value.as_array() {
            for item in items {
                if let Some(text) = item.as_str() {
                    let trimmed = text.trim().to_ascii_lowercase();
                    if !trimmed.is_empty() {
                        out.push(trimmed);
                    }
                }
            }
        } else if let Some(text) = value.as_str() {
            for token in text.split(',') {
                let trimmed = token.trim().to_ascii_lowercase();
                if !trimmed.is_empty() {
                    out.push(trimmed);
                }
            }
        }
    }
    out
}

fn build_risk_vote_policy(options: &HashMap<String, Value>) -> RiskVotePolicy {
    const DEFAULT_DOMAIN_KEYWORDS: &[&str] = &[
        "medical",
        "diagnosis",
        "clinical",
        "prescription",
        "treatment",
        "surgery",
        "healthcare",
        "legal",
        "contract",
        "compliance",
        "regulation",
        "litigation",
        "finance",
        "financial",
        "investment",
        "portfolio",
        "credit",
        "loan",
        "underwriting",
        "fraud",
        "aml",
        "tax",
        "audit",
        "insurance",
        "privacy",
        "security incident",
        "safety-critical",
    ];
    const DEFAULT_DECISION_KEYWORDS: &[&str] = &[
        "approve",
        "reject",
        "deny",
        "diagnose",
        "prescribe",
        "recommendation",
        "risk control",
        "decision",
        "compliance decision",
        "legal advice",
        "medical advice",
        "financial advice",
    ];

    let mut domain_keywords = DEFAULT_DOMAIN_KEYWORDS
        .iter()
        .map(|item| (*item).to_string())
        .collect::<Vec<_>>();
    domain_keywords.extend(option_keywords(options, "high_risk_domain_keywords"));
    domain_keywords.sort();
    domain_keywords.dedup();

    let mut decision_keywords = DEFAULT_DECISION_KEYWORDS
        .iter()
        .map(|item| (*item).to_string())
        .collect::<Vec<_>>();
    decision_keywords.extend(option_keywords(options, "high_risk_decision_keywords"));
    decision_keywords.sort();
    decision_keywords.dedup();

    RiskVotePolicy {
        enabled: option_bool(options, "high_risk_vote_enabled", true),
        threshold: option_usize(options, "high_risk_vote_threshold", 2).clamp(1, 10),
        domain_keywords,
        decision_keywords,
    }
}

fn assess_high_risk(messages: &[Message], mode: &str, policy: &RiskVotePolicy) -> RiskAssessment {
    let corpus = messages
        .iter()
        .filter(|message| message.role.eq_ignore_ascii_case("user"))
        .map(|message| message.content.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("\n");

    let mut score = 0usize;
    let mut reasons = Vec::new();

    for keyword in &policy.domain_keywords {
        if corpus.contains(keyword) {
            score += 2;
            reasons.push(format!("domain:{keyword}"));
        }
    }
    for keyword in &policy.decision_keywords {
        if corpus.contains(keyword) {
            score += 1;
            reasons.push(format!("decision:{keyword}"));
        }
    }
    if matches!(mode, "safeguard" | "full_auto") {
        score += 1;
        reasons.push(format!("mode:{mode}"));
    }

    reasons.sort();
    reasons.dedup();

    RiskAssessment {
        score,
        is_high_risk: score >= policy.threshold,
        reasons,
    }
}

pub(crate) fn normalize_vote_key(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

pub(crate) struct HighRiskVoteAttemptResult {
    pub(crate) attempt_log: Value,
    pub(crate) candidate: Option<AgentStrongVoteOutcome>,
    pub(crate) source: Option<AgentVoteSource>,
    pub(crate) failure: Option<Value>,
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
            server
                .online_controller
                .lock()
                .unwrap_or_else(|poisoned| {
                    warn!("run_high_risk_vote_attempt: online_controller poisoned, recovering");
                    poisoned.into_inner()
                })
                .record_agent_outcome(phase_name, &agent_name, success, elapsed_ms);

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
            server
                .online_controller
                .lock()
                .unwrap_or_else(|poisoned| {
                    warn!("run_high_risk_vote_attempt: online_controller poisoned, recovering");
                    poisoned.into_inner()
                })
                .record_agent_outcome(phase_name, &agent_name, false, elapsed_ms);

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

pub(crate) fn select_strong_model_id(agent: &dyn crate::agent::Agent) -> Option<String> {
    let mut models = agent
        .available_models()
        .into_iter()
        .filter(|model| !model.id.trim().is_empty())
        .collect::<Vec<_>>();

    if models.is_empty() {
        return agent.default_model().map(|model| model.id);
    }

    models.sort_by(|left, right| {
        right
            .context_window
            .unwrap_or(0)
            .cmp(&left.context_window.unwrap_or(0))
            .then_with(|| right.capabilities.len().cmp(&left.capabilities.len()))
            .then_with(|| right.is_default.cmp(&left.is_default))
    });

    models.first().map(|model| model.id.clone())
}

pub(crate) fn select_top_models(agent: &dyn crate::agent::Agent, max_models: usize) -> Vec<String> {
    let mut models = agent
        .available_models()
        .into_iter()
        .filter(|model| !model.id.trim().is_empty())
        .collect::<Vec<_>>();

    if models.is_empty() {
        return agent
            .default_model()
            .map(|model| vec![model.id])
            .unwrap_or_default();
    }

    models.sort_by(|left, right| {
        right
            .context_window
            .unwrap_or(0)
            .cmp(&left.context_window.unwrap_or(0))
            .then_with(|| right.capabilities.len().cmp(&left.capabilities.len()))
            .then_with(|| right.is_default.cmp(&left.is_default))
    });

    let ordered = models.into_iter().map(|model| model.id).collect::<Vec<_>>();
    let mut selected = Vec::new();
    for model_id in ordered {
        if !selected.iter().any(|existing| existing == &model_id) {
            selected.push(model_id);
        }
    }
    selected.truncate(max_models.max(1));
    selected
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

/// BLUE48-AUTO: Intelligently select the optimal mode when none is specified.
///
/// Analyzes user messages to determine the best mode:
/// - "edit": code editing/refactoring/requests with code blocks or file paths
/// - "agent": complex multi-step tasks that need autonomous execution
/// - "safeguard": security-sensitive operations (delete/remove/shell commands)
/// - "full_auto": highly structured tasks with clear steps
/// - "ask": general Q&A / conversation (fallback)
///
/// This is analogous to VSCode Copilot Chat's auto mode, but with more
/// granular mode classification (5 modes instead of 2).
fn infer_optimal_mode(messages: &[Message], _server: &AcpServer) -> String {
    // Collect all user message text
    let corpus: String = messages
        .iter()
        .filter(|m| m.role.eq_ignore_ascii_case("user"))
        .map(|m| &m.content[..])
        .collect::<Vec<_>>()
        .join("\n");
    let lower = corpus.to_lowercase();
    let word_count = lower.split_whitespace().count().max(1);

    // ── Check for security-sensitive operations → "safeguard" ────
    let security_keywords = [
        "delete",
        "remove file",
        "rm ",
        "sudo",
        "chmod",
        "chown",
        "drop table",
        "drop database",
        "truncate",
        "format",
        "shutdown",
        "reboot",
        "kill ",
        "exec ",
        "eval ",
    ];
    if security_keywords.iter().any(|kw| lower.contains(kw)) {
        debug!("auto-mode: security keywords detected → safeguard");
        return "safeguard".to_string();
    }

    // ── Check for code editing requests → "edit" ────────────────
    let has_code_block = corpus.contains("```");
    let has_file_path = corpus.contains('/')
        && (corpus.contains(".rs")
            || corpus.contains(".py")
            || corpus.contains(".ts")
            || corpus.contains(".js")
            || corpus.contains(".go")
            || corpus.contains(".rs"));
    let edit_keywords = [
        "refactor",
        "rewrite",
        "change",
        "update",
        "modify",
        "fix bug",
        "add feature",
        "implement",
        "write code",
        "code review",
    ];
    let is_edit_request = edit_keywords.iter().any(|kw| lower.contains(kw));

    if (has_code_block || has_file_path) && is_edit_request {
        debug!("auto-mode: code editing detected → edit");
        return "edit".to_string();
    }

    // ── Check for complex multi-step tasks → "agent" ────────────
    let agent_keywords = [
        "investigate",
        "research",
        "analyze",
        "compare",
        "summarize",
        "explore",
        "monitor",
        "watch",
        "track",
        "debug",
        "troubleshoot",
        "diagnose",
        "find out",
        "figure out",
    ];
    let is_complex_task = word_count > 30 && agent_keywords.iter().any(|kw| lower.contains(kw));

    if is_complex_task {
        debug!("auto-mode: complex task detected → agent");
        return "agent".to_string();
    }

    // ── Check for highly structured tasks → "full_auto" ─────────
    let has_bullets = corpus.contains("* ") || corpus.contains("- ");
    let has_numbers = corpus.contains("1. ") && corpus.contains("2. ");
    if has_bullets && has_numbers && word_count > 50 {
        debug!("auto-mode: structured task detected → full_auto");
        return "full_auto".to_string();
    }

    // ── Default: general conversation → "ask" ───────────────────
    debug!("auto-mode: general conversation → ask");
    "ask".to_string()
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
            .observability
            .telemetry_runtime
            .lock()
            .unwrap_or_else(|poisoned| {
                warn!("handle_chat: telemetry_runtime poisoned, recovering");
                poisoned.into_inner()
            })
            .start_child_span(
                parent,
                "acp.chat",
                vec![KeyValue::new("phase.entry", "chat")],
            )
    });

    let result = async {
        let lifecycle_snapshot = {
            let lifecycle_guard = server.lifecycle_state.lock().unwrap_or_else(|poisoned| {
                warn!("handle_chat: lifecycle_state poisoned, recovering");
                poisoned.into_inner()
            });
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
                t("error.chat.server_shutting_down"),
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

        // BLUE48-AUTO: Intelligent mode selection when mode is absent, empty, or "auto".
        // - Client passes a specific mode ("ask"/"edit"/"agent"/"safeguard"/"full_auto"): use it as-is
        // - Client passes nothing or "auto": analyze input to select optimal mode
        if chat_params.mode.trim().is_empty()
            || chat_params.mode.trim().eq_ignore_ascii_case("auto")
        {
            let auto_mode = infer_optimal_mode(&chat_params.messages, server);
            chat_params.mode = auto_mode;
            info!(
                "mode not specified by client, auto-selected mode='{}'",
                chat_params.mode
            );
        }

        // GAP-46-12: Track session context across requests.
        // Use SessionContextManager to extract key concepts from the conversation
        // and maintain continuity markers for long-running sessions.
        let mut session_mgr = SessionContextManager::default();
        let conversation_id = chat_params
            .conversation_id
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let msg_count = chat_params.messages.len();
        debug!(
            "SessionContextManager: tracking conversation '{}' with {} messages",
            conversation_id, msg_count
        );
        // Record each message for context extraction.
        for msg in &chat_params.messages {
            session_mgr.record_message(&msg.content, &msg.role);
        }
        let concept_count = session_mgr.concept_count();
        let decision_count = session_mgr.decision_count();
        if concept_count > 0 || decision_count > 0 {
            debug!(
                "SessionContextManager: {} concepts, {} decisions extracted",
                concept_count, decision_count
            );
        }
        // If the conversation is long, compute trim budget and apply it.
        if msg_count > 50 {
            let effective = session_mgr.budget.effective_retain();
            debug!(
                "SessionContextManager: effective retain budget for {} messages = {}",
                msg_count, effective
            );

            // Convert messages to the tuple format expected by select_retained_messages.
            let msg_tuples: Vec<(String, String)> = chat_params
                .messages
                .iter()
                .map(|m| (m.role.clone(), m.content.clone()))
                .collect();

            let retained_indices = session_mgr.select_retained_messages(&msg_tuples, effective);

            // If messages heavily exceed budget (50%+ over), try semantic
            // compression as an alternative to simple trimming.
            let compression_applied = if msg_count > effective * 3 / 2 {
                let compressor = SessionCompressor::default();
                let compressed = session_mgr.compress_messages(&msg_tuples, &compressor);
                if !compressed.summary.is_empty() {
                    warn!(
                        "SessionContextManager: compression reduced {}→{} messages (ratio: {:.2})",
                        compressed.original_count,
                        compressed.compressed_count,
                        compressed.compression_ratio,
                    );
                    let kept_count = compressed.kept_messages.len();
                    let orig_count = compressed.original_count;
                    let summary_text = compressed.summary.clone();
                    // Convert compressor messages back to agent messages.
                    let mut compressed_msgs: Vec<Message> = compressed
                        .kept_messages
                        .into_iter()
                        .map(|m| Message {
                            role: m.role,
                            content: m.content,
                        })
                        .collect();
                    // Prepend the summary as a system message.
                    compressed_msgs.insert(
                        0,
                        Message {
                            role: "system".to_string(),
                            content: format!(
                                "[Session compressed: {} messages summarized]\n{}",
                                orig_count - kept_count,
                                summary_text,
                            ),
                        },
                    );
                    chat_params.messages = compressed_msgs;
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if !compression_applied && retained_indices.len() < msg_count {
                let trimmed_count = msg_count - retained_indices.len();
                warn!(
                    "SessionContextManager: trimming {} of {} messages (retaining {})",
                    trimmed_count,
                    msg_count,
                    retained_indices.len(),
                );

                // Generate continuity marker for the trimmed messages.
                let trimmed_indices: Vec<usize> = (0..msg_count)
                    .filter(|i| !retained_indices.contains(i))
                    .collect();
                let marker = session_mgr.generate_continuity_marker(&trimmed_indices);

                // Build a concise continuity marker text for the LLM.
                let marker_text = format!(
                    "[Continuity: {} messages trimmed to fit context window]\n\
                     Key concepts: {}\n\
                     Files referenced: {}\n\
                     Decisions made: {}",
                    marker.messages_trimmed,
                    if marker.key_concepts.is_empty() {
                        "(none)".to_string()
                    } else {
                        marker.key_concepts.join(", ")
                    },
                    if marker.files_referenced.is_empty() {
                        "(none)".to_string()
                    } else {
                        marker.files_referenced.join(", ")
                    },
                    if marker.decisions_made.is_empty() {
                        "(none)".to_string()
                    } else {
                        marker.decisions_made.join(", ")
                    },
                );

                // Rebuild the message list from retained indices only.
                chat_params.messages = retained_indices
                    .iter()
                    .map(|&i| chat_params.messages[i].clone())
                    .collect();

                // Prepend the continuity marker as a system message so the LLM
                // knows what context was trimmed and can reference it if needed.
                chat_params.messages.insert(
                    0,
                    Message {
                        role: "system".to_string(),
                        content: marker_text,
                    },
                );
            }
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

fn has_flow_phase(config: &crate::config::AppConfig, phase: &str) -> bool {
    config
        .flow
        .phases
        .iter()
        .any(|candidate| candidate == phase)
        || config.phases.contains_key(phase)
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

/// Process chat request
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
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
    // Clear per-request cache to ensure fresh task description computation
    clear_task_description_cache();
    let (flow, registry) = routing_handles(server)?;
    let tenant_id = &ctx.tenant_id;

    // ── Step 1: Pre-route policy evaluation ────────────────────────────
    evaluate_pre_route_policies(server, params, trace, tenant_id).await?;

    // ── Step 2: Phase resolution ──────────────────────────────────────
    let phase_res = resolve_request_phase(server, params, &flow, registry.as_ref()).await?;

    let phase = phase_res.phase;
    let phase_name = &phase_res.phase_name;
    let phase_origin = &phase_res.phase_origin;
    let _has_requested_phase = phase_res.has_requested_phase;
    let _has_controller_phase = phase_res.has_controller_phase;
    let _has_adaptive_phase = phase_res.has_adaptive_phase;
    let mut resolved = phase_res.resolved;
    let schema_warnings = phase_res.schema_warnings;
    let schema_error = phase_res.schema_error;
    let mut routing_provenance = phase_res.routing_provenance;
    let reputation_scores = phase_res.reputation_scores;

    // ── Step 3: Agent selection & scoring ─────────────────────────────
    let agent_sel = select_and_score_agents(
        server,
        params,
        &mut resolved,
        &phase,
        phase_name,
        tenant_id,
        trace,
        &mut routing_provenance,
        &reputation_scores,
    )
    .await?;

    let capability_info = crate::acp::helpers::response_assembler::CapabilityRoutingInfo {
        selected_agent: agent_sel.capability_selected_agent,
        recommended_mode: agent_sel.capability_recommended_mode,
        candidate_count: agent_sel.capability_candidate_count,
        decision_confidence: agent_sel.capability_decision_confidence,
        selection_reason: agent_sel.capability_selection_reason,
        optimization_hint: agent_sel.capability_optimization_hint,
    };
    let configured_primary_agent = agent_sel.configured_primary_agent;
    let _preferred_agent_from_request = agent_sel.preferred_agent_from_request;
    let conversation_id = agent_sel.conversation_id;
    let branch_id = agent_sel.branch_id;
    let agent_messages = agent_sel.agent_messages;
    let layered_prompt_segments = agent_sel.layered_prompt_segments;
    let base_agent_options = agent_sel.base_agent_options;
    let risk_policy = agent_sel.risk_policy;
    let risk_assessment = agent_sel.risk_assessment;
    let enable_high_risk_vote = agent_sel.enable_high_risk_vote;
    let enable_high_risk_multi_agent_vote = agent_sel.enable_high_risk_multi_agent_vote;
    let min_vote_agents = agent_sel.min_vote_agents;
    let max_vote_agents = agent_sel.max_vote_agents;
    let escalation_enabled = agent_sel.escalation_enabled;
    let escalation_models_per_agent = agent_sel.escalation_models_per_agent;
    let escalation_max_agents = agent_sel.escalation_max_agents;
    let _model_is_specific = agent_sel.model_is_specific;
    let unhealthy_fallback_agent = agent_sel.unhealthy_fallback_agent;
    let fallback_reason = agent_sel.fallback_reason;
    let council_decision = agent_sel.council_decision;
    let candidate_agents = agent_sel.candidate_agents;
    let vector_context = agent_sel.vector_context;

    // ── Step 4: Initialize execution state ────────────────────────────
    let mut selected_agent = String::new();
    let mut response_text = String::new();
    let mut reasoning_text: String;
    let selected_model_name: Option<String>;
    let mut last_err: Option<anyhow::Error>;
    let quota_failed_agents: Vec<String>;
    let mut agent_attempts: Vec<Value> = Vec::with_capacity(resolved.agents.len() + 2);
    let mut cache_hit = false;
    let cache_bypassed_for_execution = should_bypass_for_execution(&params.mode, &agent_messages);

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

    // ── Token cache lookup ──────────────────────────────────────────
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
        let cache_decision = CacheStrategy::decide_from_entry(
            &format!("{level}"),
            &entry,
            &input_text,
            cache_bypassed_for_execution,
        );

        match cache_decision {
            CacheDecision::Hit { response, level } => {
                tracing::info!(
                    target = "token_cache",
                    level = %level,
                    agent_count = resolved.agents.len(),
                    "process_chat_request: token cache HIT, skipping agent execution"
                );
                cache_hit = true;
                selected_agent = resolved
                    .agents
                    .first()
                    .map(|(name, _)| name.clone())
                    .unwrap_or_else(|| "cached".to_string());
                response_text = response;

                if let Some(ref observer) = stream_observer {
                    let meta = StreamEventMeta {
                        agent_name: &selected_agent,
                        phase_name,
                        trace_id: &trace.trace_id,
                    };
                    let total_chars = response_text.chars().count();
                    emit_stream_chunk(server, Some(observer), meta, &response_text, 1, total_chars)
                        .await?;
                    emit_stream_done(server, Some(observer), meta, 1, total_chars, 0u64, None)
                        .await?;
                }

                agent_attempts.push(CacheStrategy::attempt_entry(&CacheDecision::Hit {
                    response: response_text.clone(),
                    level: level.clone(),
                }));
            }
            CacheDecision::Refused { level, reason } => {
                tracing::info!(
                    target = "token_cache",
                    level = %level,
                    mode = %params.mode,
                    "process_chat_request: cache HIT but refused (execution-like request)"
                );
                crate::acp::helpers::autonomy_metrics::record_cache_shortcircuit_refused(&reason);
                crate::acp::helpers::autonomy_metrics::record_cache_bypass_for_execution();
                agent_attempts.push(CacheStrategy::attempt_entry(&CacheDecision::Refused {
                    level,
                    reason,
                }));
            }
            CacheDecision::Miss => {}
        }
    } else if cache_bypassed_for_execution {
        crate::acp::helpers::autonomy_metrics::record_cache_bypass_for_execution();
        agent_attempts.push(CacheStrategy::attempt_entry(&CacheDecision::Miss));
    }

    // ── Step 5: Autonomy round execution ───────────────────────────────
    let autonomy_outcome = execute_autonomy_round(
        server,
        params,
        &phase,
        phase_name,
        &resolved,
        &agent_messages,
        &base_agent_options,
        cache_hit,
    )
    .await;

    if autonomy_outcome.autonomy_loop_executed {
        selected_agent = autonomy_outcome.selected_agent;
        response_text = autonomy_outcome.response_text;
    }
    agent_attempts.extend(autonomy_outcome.agent_attempts);
    let autonomy_loop_executed = autonomy_outcome.autonomy_loop_executed;

    // ── Step 6: Fallback agent execution (parallel with concurrency limit 5) ──
    if !cache_hit && response_text.is_empty() {
        let fallback_result = execute_fallback_agents(
            server,
            resolved.agents.clone(),
            &phase,
            phase_name,
            agent_messages.clone(),
            base_agent_options.clone(),
            stream_observer.clone(),
            trace,
            unhealthy_fallback_agent,
            enable_high_risk_multi_agent_vote,
            max_vote_agents,
        )
        .await;

        selected_agent = fallback_result.selected_agent;
        response_text = fallback_result.response_text;
        reasoning_text = fallback_result.reasoning_text;
        selected_model_name = fallback_result.selected_model_name;
        last_err = fallback_result.last_err;
        quota_failed_agents = fallback_result.quota_failed_agents;
        agent_attempts.extend(fallback_result.agent_attempts);

        // ── High-Risk Vote Execution & Escalation ────────────────────
        let vote_timeout = request_timeout(phase.options.as_ref());
        let vote_result = crate::acp::helpers::vote_executor::execute_high_risk_vote(
            server,
            phase_name,
            &trace.trace_id,
            fallback_result.high_risk_vote_jobs,
            &agent_messages,
            phase.principles.clone(),
            vote_timeout,
            cache_hit,
            enable_high_risk_multi_agent_vote,
            min_vote_agents,
            max_vote_agents,
            escalation_enabled,
            escalation_models_per_agent,
            escalation_max_agents,
            &reputation_scores,
            &mut routing_provenance,
        )
        .await;

        agent_attempts.extend(vote_result.agent_attempts);

        let mut emit_final_vote_response = false;
        let mut vote_winner: Option<String> = None;
        let mut vote_report: Option<Value> = None;
        let mut used_multi_model_vote = false;
        let mut used_multi_agent_vote = false;
        let mut review_required = false;

        if vote_result.emit_final_vote_response {
            response_text = vote_result.response_text;
            reasoning_text = vote_result.reasoning_text;
            selected_agent = vote_result.selected_agent;
            last_err = vote_result.last_err;
            vote_winner = vote_result.vote_winner;
            vote_report = vote_result.vote_report;
            used_multi_agent_vote = vote_result.used_multi_agent_vote;
            used_multi_model_vote = vote_result.used_multi_model_vote;
            review_required = vote_result.review_required;
            emit_final_vote_response = true;
        }

        if emit_final_vote_response {
            if let Some(ref observer) = stream_observer {
                let meta = StreamEventMeta {
                    agent_name: &selected_agent,
                    phase_name,
                    trace_id: &trace.trace_id,
                };
                let total_chars = response_text.chars().count();
                emit_stream_chunk(server, Some(observer), meta, &response_text, 1, total_chars)
                    .await?;
                emit_stream_done(
                    server,
                    Some(observer),
                    meta,
                    1,
                    total_chars,
                    0u64,
                    selected_model_name.clone(),
                )
                .await?;
            }
        }

        // ── Error handling for empty / quota-limited responses ─────────
        // (inline — too tightly coupled to local state to extract)
        if response_text.is_empty() && last_err.is_none() {
            let all_empty_responses = !agent_attempts.is_empty()
                && agent_attempts.iter().all(|attempt| {
                    attempt
                        .get("ok")
                        .and_then(|value| value.as_bool())
                        .map(|ok| !ok)
                        .unwrap_or(false)
                        && attempt
                            .get("error")
                            .and_then(|value| value.as_str())
                            .map(|error| error == "empty_response")
                            .unwrap_or(false)
                });

            server
                .tenant_budget
                .lock()
                .unwrap_or_else(|poisoned| {
                    warn!("process_chat_request: tenant_budget poisoned, recovering");
                    poisoned.into_inner()
                })
                .record_usage(tenant_id, 0, 0);

            if all_empty_responses {
                anyhow::bail!(tf("error.chat.all_agents_empty", &[("phase", phase_name)]));
            }
            anyhow::bail!(tf("error.chat.no_healthy_agent", &[("phase", phase_name)]));
        }

        if let Some(err) = last_err {
            let all_attempts_quota_limited = !agent_attempts.is_empty()
                && agent_attempts.iter().all(|attempt| {
                    attempt
                        .get("ok")
                        .and_then(|value| value.as_bool())
                        .map(|ok| !ok)
                        .unwrap_or(false)
                        && attempt
                            .get("quota_limited")
                            .and_then(|value| value.as_bool())
                            .unwrap_or(false)
                });

            server
                .online_controller
                .lock()
                .unwrap_or_else(|poisoned| {
                    warn!("process_chat_request: online_controller poisoned, recovering");
                    poisoned.into_inner()
                })
                .record_phase_outcome(phase_name, false, started.elapsed().as_millis() as u64);

            if all_attempts_quota_limited {
                server
                    .tenant_budget
                    .lock()
                    .unwrap_or_else(|poisoned| {
                        warn!("process_chat_request: tenant_budget poisoned, recovering");
                        poisoned.into_inner()
                    })
                    .record_usage(tenant_id, 0, 0);
                let switch_prompt = tf(
                    "error.chat.all_agents_quota_limited",
                    &[("phase", phase_name)],
                );
                return Ok(json!({
                    "done": false,
                    "mode": params.mode,
                    "phase": phase_name,
                    "phase_origin": phase_origin,
                    "requires_user_action": true,
                    "action": "switch_agent",
                    "prompt": switch_prompt,
                    "available_agents": candidate_agents,
                    "quota_failed_agents": quota_failed_agents,
                    "agent_attempts": agent_attempts,
                    "risk_decision": json!({
                        "policy_enabled": risk_policy.enabled,
                        "score": risk_assessment.score,
                        "is_high_risk": risk_assessment.is_high_risk,
                        "reasons": risk_assessment.reasons,
                        "multi_model_vote_enabled": enable_high_risk_vote,
                        "multi_model_vote_used": used_multi_model_vote,
                        "multi_agent_vote_enabled": enable_high_risk_multi_agent_vote,
                        "multi_agent_vote_used": used_multi_agent_vote,
                        "escalation_enabled": escalation_enabled,
                        "escalation_models_per_agent": escalation_models_per_agent,
                        "escalation_max_agents": escalation_max_agents,
                        "review_required": review_required,
                        "vote_report": vote_report,
                    }),
                    "hint": {
                        "options_field": "options.extra.preferred_agent",
                        "example": {
                            "preferred_agent": candidate_agents.first().cloned().unwrap_or_else(|| "primary".to_string())
                        }
                    }
                }));
            }

            server
                .tenant_budget
                .lock()
                .unwrap_or_else(|poisoned| {
                    warn!("process_chat_request: tenant_budget poisoned, recovering");
                    poisoned.into_inner()
                })
                .record_usage(tenant_id, 0, 0);

            return Err(err);
        }

        // ── Post-success cleanup ──────────────────────────────────────
        if let Some(primary) = configured_primary_agent {
            if selected_agent == primary {
                agent_switch_state()
                    .lock()
                    .unwrap_or_else(|poisoned| {
                        warn!("process_chat_request: agent_switch_state poisoned, recovering");
                        poisoned.into_inner()
                    })
                    .forced_agent_by_phase
                    .remove(phase_name);
            }
        }

        server
            .online_controller
            .lock()
            .unwrap_or_else(|poisoned| {
                warn!("process_chat_request: online_controller poisoned, recovering");
                poisoned.into_inner()
            })
            .record_phase_outcome(phase_name, true, started.elapsed().as_millis() as u64);

        // ── Persist vector memory ─────────────────────────────────────
        persist_vector_memory(
            server,
            phase_name,
            phase.options.as_ref(),
            params,
            &response_text,
            &selected_agent,
        )
        .await;

        // ── Parallel persistence ──────────────────────────────────────
        let mut checkpoint_messages = params.messages.clone();
        checkpoint_messages.push(Message {
            role: "assistant".to_string(),
            content: response_text.clone(),
        });

        let (mut checkpoint, knowledge) = tokio::join!(
            crate::acp::r#impl::request::create_checkpoint_record(
                server,
                &conversation_id,
                &branch_id,
                checkpoint_messages,
                None,
                None,
            ),
            persist_chat_knowledge(
                server,
                &conversation_id,
                &branch_id,
                phase_name,
                &selected_agent,
                params,
                &response_text,
            ),
        );

        let (metacognitive_loop, distillation) = tokio::join!(
            crate::acp::r#impl::request::persist_checkpoint_metacognitive_loop(
                server,
                &conversation_id,
                &branch_id,
                &checkpoint.checkpoint_id,
                json!({
                    "active": true,
                    "schema_version": "blue25-metacognitive-loop-v1",
                    "cycle_count": 1,
                    "checkpoint_id": checkpoint.checkpoint_id,
                    "last_reflection": format!("{}:{}", phase_name, selected_agent),
                    "reflection_trigger": "response_completed",
                    "last_selected_agent": selected_agent,
                    "response_chars": response_text.chars().count(),
                }),
            ),
            persist_session_distillation(
                server,
                &conversation_id,
                &branch_id,
                phase_name,
                params,
                &selected_agent,
                &candidate_agents,
                &agent_attempts,
                &response_text,
            ),
        );
        checkpoint.metacognitive_loop = Some(metacognitive_loop.clone());

        if stream_observer.is_some() {
            emit_stream_token_economy(
                server,
                stream_observer.as_ref(),
                StreamEventMeta {
                    agent_name: &selected_agent,
                    phase_name,
                    trace_id: &trace.trace_id,
                },
                &estimate_token_economy(&params.messages, &response_text),
            )
            .await?;
        }

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
            phase: phase_name.clone(),
            agent: Some(selected_agent.clone()),
            tool: None,
            status: "ok".to_string(),
            inputs: json!({"attributes": {"agent": selected_agent.clone()}}),
            outputs: None,
            duration_ms: 0,
            error: None,
            pua_stage: None,
        });

        // ── Step 7: FullAuto execution (review gate + TAO loop) ────────
        // Cache task_description once for all full_auto sub-steps
        let full_auto_result = run_full_auto_execution(
            server,
            params,
            &phase,
            phase_name,
            span,
            trace,
            &selected_agent,
            &response_text,
            &conversation_id,
            autonomy_loop_executed,
        )
        .await;

        // ── Step 8: Response assembly ──────────────────────────────────
        let selected_agent_reputation = reputation_scores.get(&selected_agent).copied();

        let risk_decision = json!({
            "policy_enabled": risk_policy.enabled,
            "score": risk_assessment.score,
            "is_high_risk": risk_assessment.is_high_risk,
            "reasons": risk_assessment.reasons,
            "multi_model_vote_enabled": enable_high_risk_vote,
            "multi_model_vote_used": used_multi_model_vote,
            "multi_agent_vote_enabled": enable_high_risk_multi_agent_vote,
            "multi_agent_vote_used": used_multi_agent_vote,
            "escalation_enabled": escalation_enabled,
            "escalation_models_per_agent": escalation_models_per_agent,
            "escalation_max_agents": escalation_max_agents,
            "review_required": review_required,
            "vote_report": vote_report,
        });

        let result = apply_review_gate_assemble(
            server,
            params,
            trace,
            phase_name,
            phase_origin,
            &selected_agent,
            &selected_model_name,
            &response_text,
            &reasoning_text,
            tenant_id,
            started,
            &conversation_id,
            &branch_id,
            schema_warnings,
            schema_error,
            layered_prompt_segments,
            &full_auto_result.tool_execution_results,
            &sched_task_id,
            &candidate_agents,
            &routing_provenance,
            &reputation_scores,
            selected_agent_reputation,
            &council_decision,
            &vote_winner,
            &fallback_reason,
            cache_hit,
            cache_bypassed_for_execution,
            capability_info,
            full_auto_result.reviews,
            agent_attempts,
            risk_decision,
            quota_failed_agents,
            vector_context,
            knowledge,
            distillation,
            checkpoint,
            metacognitive_loop,
        )
        .await?;

        // P3: Auto-create skills from conversation patterns
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            auto_create_skills_from_conversation(server, params, &response_text),
        )
        .await;

        // P4: Auto-generate workflow from conversation patterns
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            auto_generate_workflow_from_conversation(server, params, &response_text),
        )
        .await;

        // BLUE48 Step 19: Run rationalization on the final decision.
        // This wires the intelligence hub's multi-factor risk analysis into
        // the request path. The result is logged for observability.
        let task_description = extract_task_description(&params.messages);
        let (justified, reason) = crate::intelligence::hub::rationalize_decision(
            &selected_agent,
            &task_description,
            if response_text.is_empty() { 0.3 } else { 0.8 },
        );
        if !justified {
            debug!(
                "rationalize_decision: decision blocked for agent={} reason={} confidence={}",
                selected_agent, reason, 0.8
            );
        }

        Ok(result)
    } else {
        Ok(json!({
            "done": true,
            "mode": params.mode,
            "phase": phase_name,
            "phase_origin": phase_origin,
            "cached": cache_hit,
            "agent": selected_agent,
            "response": response_text,
        }))
    }
}

// ═════════════════════════════════════════════════════════════════════
// Extracted sub-functions for process_chat_request (BLUE48 Step 1)
// ═════════════════════════════════════════════════════════════════════

/// Result of the phase resolution step.
struct PhaseResolution {
    phase: crate::orchestration::flow::ResolvedPhase,
    phase_name: String,
    phase_origin: String,
    resolved: crate::orchestration::flow::ResolvedRouting,
    has_requested_phase: bool,
    has_controller_phase: bool,
    has_adaptive_phase: bool,
    schema_warnings: Vec<String>,
    schema_error: Option<String>,
    routing_provenance: Vec<String>,
    reputation_scores: HashMap<String, f64>,
    _online_scores: Vec<(String, f64)>,
    _original_count: usize,
    _unavailable_agents: Vec<String>,
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

async fn resolve_request_phase(
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

    // ── SchemaRegistry task envelope validation (F-GAP-07) ─────────────
    let mut schema_warnings: Vec<String> = Vec::new();
    let mut schema_error: Option<String> = None;
    let sr_guard = server.schema_registry.lock().unwrap_or_else(|poisoned| {
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
        has_requested_phase,
        has_controller_phase,
        has_adaptive_phase,
        schema_warnings,
        schema_error,
        routing_provenance,
        reputation_scores,
        _online_scores: online_scores,
        _original_count: original_count,
        _unavailable_agents: unavailable_agents,
    })
}

/// Evaluate pre-route policies (HarnessBus, token gate, tenant budget).
/// Returns an error if any policy denies the request.
async fn evaluate_pre_route_policies(
    server: &AcpServer,
    params: &ChatParams,
    trace: &RequestTraceContext,
    tenant_id: &str,
) -> Result<()> {
    let _policy_result = crate::acp::helpers::pre_route_policy::evaluate_pre_route_policies(
        server, params, trace, tenant_id,
    )
    .await?;
    Ok(())
}

/// Outcome of the agent selection & scoring step.
#[allow(clippy::too_many_arguments)]
struct AgentSelectionOutcome {
    capability_selected_agent: Option<String>,
    capability_recommended_mode: Option<String>,
    capability_candidate_count: Option<u64>,
    capability_decision_confidence: Option<f64>,
    capability_selection_reason: Option<String>,
    capability_optimization_hint: Option<Value>,
    configured_primary_agent: Option<String>,
    preferred_agent_from_request: Option<String>,
    conversation_id: String,
    branch_id: String,
    agent_messages: Vec<Message>,
    layered_prompt_segments: usize,
    base_agent_options: HashMap<String, Value>,
    risk_policy: RiskVotePolicy,
    risk_assessment: RiskAssessment,
    enable_high_risk_vote: bool,
    enable_high_risk_multi_agent_vote: bool,
    min_vote_agents: usize,
    max_vote_agents: usize,
    escalation_enabled: bool,
    escalation_models_per_agent: usize,
    escalation_max_agents: usize,
    model_is_specific: bool,
    unhealthy_fallback_agent: Option<String>,
    fallback_reason: Option<String>,
    council_decision: Option<Value>,
    candidate_agents: Vec<String>,
    _routing_provenance: Vec<String>,
    vector_context: VectorContext,
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
async fn select_and_score_agents(
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
    if let Some(ref cb) = server.capability_bus {
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
        let reg_guard = server.skill_registry.lock().unwrap_or_else(|poisoned| {
            warn!("select_and_score_agents: skill_registry poisoned, recovering");
            poisoned.into_inner()
        });
        let skill_count = reg_guard.list().len();
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

    let candidate_agents = resolved
        .agents
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();

    // ── Council deliberation ───────────────────────────────────────────
    let (unhealthy_fallback_agent, fallback_reason, council_decision) =
        crate::acp::helpers::council_deliberation::run_council_deliberation_and_fallback(
            server.capability_bus.as_ref().map(|arc| arc.as_ref()),
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
        model_is_specific: filter_result.model_is_specific,
        unhealthy_fallback_agent,
        fallback_reason,
        council_decision,
        candidate_agents,
        _routing_provenance: routing_provenance.clone(),
        vector_context,
    })
}

/// Outcome of the autonomy loop execution.
struct AutonomyOutcome {
    autonomy_loop_executed: bool,
    selected_agent: String,
    response_text: String,
    _reasoning_text: String,
    _selected_model_name: Option<String>,
    agent_attempts: Vec<Value>,
}

// Execute the multi-round autonomy loop for full_auto / execution-like requests.
//
// Runs think → act → observe → replan cycles with agent rerouting support.
// Returns whether the loop was executed, and if successful, the response.
// ============================================================================
// Section: Autonomy Loop & Fallback
// ============================================================================

#[allow(clippy::too_many_arguments)]
async fn execute_autonomy_round(
    _server: &AcpServer,
    params: &ChatParams,
    phase: &crate::orchestration::flow::ResolvedPhase,
    _phase_name: &str,
    resolved: &crate::orchestration::flow::ResolvedRouting,
    agent_messages: &[Message],
    base_agent_options: &HashMap<String, Value>,
    cache_hit: bool,
) -> AutonomyOutcome {
    if cache_hit
        || !crate::acp::helpers::autonomy_loop_adapter::should_use_acp_autonomy_loop(
            &params.mode,
            agent_messages,
        )
    {
        return AutonomyOutcome {
            autonomy_loop_executed: false,
            selected_agent: String::new(),
            response_text: String::new(),
            _reasoning_text: String::new(),
            _selected_model_name: None,
            agent_attempts: Vec::new(),
        };
    }

    let reroute_enabled = option_bool(base_agent_options, "enable_agent_reroute", true);
    let autonomy_candidates = resolved.agents.clone();
    let mut agent_attempts: Vec<Value> = Vec::new();
    let mut response_text = String::new();
    let mut reasoning_text = String::new();
    let mut selected_agent = String::new();
    let mut selected_model_name: Option<String> = None;
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
        let autonomy_tool_registry = Some(std::sync::Arc::new(
            crate::orchestration::tool::ToolRegistry::new(),
        ));
        let result = crate::acp::helpers::autonomy_loop_adapter::run_acp_autonomy_loop(
            agent,
            autonomy_tool_registry,
            agent_messages.to_vec(),
            phase.principles.clone(),
            Some(base_agent_options.clone()),
            request_timeout(phase.options.as_ref()),
            None,
        )
        .await;

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
                    reasoning_text = loop_result.reasoning;
                    selected_model_name = loop_result.selected_model;
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
        _reasoning_text: reasoning_text,
        _selected_model_name: selected_model_name,
        agent_attempts,
    }
}

/// A job representing a high-risk multi-agent vote task.
type HighRiskVoteJob = (
    String,
    Arc<dyn crate::agent::Agent>,
    HashMap<String, Value>,
    Option<String>,
);

/// Result of executing fallback agents.
struct FallbackExecutionResult {
    selected_agent: String,
    response_text: String,
    reasoning_text: String,
    selected_model_name: Option<String>,
    last_err: Option<anyhow::Error>,
    agent_attempts: Vec<Value>,
    quota_failed_agents: Vec<String>,
    _cache_hit: bool,
    high_risk_vote_jobs: Vec<HighRiskVoteJob>,
}

/// Execute fallback agents using parallel execution with a concurrency limit.
///
/// Iterates over resolved agents and calls each concurrently using `tokio::spawn`
/// with a `Semaphore` limiting concurrency to 5. Returns the first successful
/// response or collects all errors for fallback handling.
#[allow(clippy::too_many_arguments)]
async fn execute_fallback_agents(
    server: &AcpServer,
    agent_list: Vec<(String, Arc<dyn crate::agent::Agent>)>,
    phase: &crate::orchestration::flow::ResolvedPhase,
    phase_name: &str,
    agent_messages: Vec<Message>,
    base_agent_options: HashMap<String, Value>,
    stream_observer: Option<StreamObserver>,
    trace: &RequestTraceContext,
    unhealthy_fallback_agent: Option<String>,
    enable_high_risk_multi_agent_vote: bool,
    max_vote_agents: usize,
) -> FallbackExecutionResult {
    let mut selected_agent = String::new();
    let mut response_text = String::new();
    let mut reasoning_text = String::new();
    let mut selected_model_name: Option<String> = None;
    let mut last_err: Option<anyhow::Error> = None;
    let mut agent_attempts: Vec<Value> = Vec::with_capacity(agent_list.len() + 2);
    let mut quota_failed_agents: Vec<String> = Vec::with_capacity(agent_list.len());
    let mut high_risk_vote_jobs: Vec<HighRiskVoteJob> = Vec::with_capacity(max_vote_agents);

    use futures_util::future::join_all;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(5));
    let mut futures = Vec::with_capacity(agent_list.len());

    for (agent_name, agent) in agent_list.into_iter() {
        // High-risk multi-agent vote collection: skip regular execution
        if enable_high_risk_multi_agent_vote {
            let strong_model = if agent.supports_model_override() {
                select_strong_model_id(agent.as_ref())
            } else {
                None
            };
            let mut vote_options = base_agent_options.clone();
            if let Some(model_id) = strong_model.clone() {
                vote_options.insert("model".to_string(), Value::String(model_id));
            }
            high_risk_vote_jobs.push((
                agent_name.clone(),
                Arc::clone(&agent),
                vote_options,
                strong_model,
            ));
            if high_risk_vote_jobs.len() >= max_vote_agents {
                continue;
            }
            continue;
        }

        let is_unhealthy = server.capability_bus.as_ref().is_some_and(|cb| {
            let unhealthy = !cb.is_agent_healthy(&agent_name);
            if unhealthy && unhealthy_fallback_agent.as_deref() != Some(agent_name.as_str()) {
                warn!(
                    phase = %phase_name,
                    agent = %agent_name,
                    "skipping unhealthy agent"
                );
            }
            unhealthy
        });

        if is_unhealthy && unhealthy_fallback_agent.as_deref() != Some(agent_name.as_str()) {
            agent_attempts.push(json!({
                "agent": agent_name,
                "ok": false,
                "skipped_unhealthy": true,
                "duration_ms": 0u64,
                "error": t("error.chat.agent_unhealthy")
            }));
            continue;
        }

        let sem_clone = Arc::clone(&semaphore);
        let agent_name_owned = agent_name.clone();
        let per_attempt_options = base_agent_options.clone();
        let stream_obs = stream_observer.clone();
        let msg_clone = agent_messages.clone();
        let principles = phase.principles.clone();
        let timeout = request_timeout(phase.options.as_ref());

        let fut = async move {
            let _permit = sem_clone.acquire().await.map_err(|_| {
                tracing::warn!("semaphore closed during agent execution — task skipped");
            });
            let _permit = match _permit {
                Ok(p) => p,
                Err(()) => {
                    return (
                        agent_name_owned,
                        std::time::Instant::now(),
                        Err(anyhow::anyhow!("semaphore closed")),
                    )
                }
            };
            let attempt_started = std::time::Instant::now();
            let stream_ctx = StreamNotificationContext {
                stream_observer: stream_obs,
                agent_name: &agent_name_owned,
                phase_name,
                trace_id: &trace.trace_id,
            };
            let result = run_agent_collecting(
                server,
                stream_ctx,
                agent,
                msg_clone,
                principles,
                Some(per_attempt_options),
                timeout,
            )
            .await;
            (agent_name_owned, attempt_started, result)
        };
        futures.push(fut);
    }

    // Collect results using JoinAll
    let results = join_all(futures).await;

    for (agent_name, attempt_started, agent_result) in results {
        match agent_result {
            Ok((output_text, reasoning_output, agent_selected_model)) => {
                if output_text.trim().is_empty() {
                    agent_attempts.push(json!({
                        "agent": agent_name,
                        "ok": false,
                        "duration_ms": attempt_started.elapsed().as_millis() as u64,
                        "error": "empty_response",
                    }));
                    server
                        .online_controller
                        .lock()
                        .unwrap_or_else(|poisoned| {
                            warn!(
                                "execute_fallback_agents: online_controller poisoned, recovering"
                            );
                            poisoned.into_inner()
                        })
                        .record_agent_outcome(
                            phase_name,
                            &agent_name,
                            false,
                            attempt_started.elapsed().as_millis() as u64,
                        );
                    continue;
                }

                server
                    .online_controller
                    .lock()
                    .unwrap_or_else(|poisoned| {
                        warn!("execute_fallback_agents: online_controller poisoned, recovering");
                        poisoned.into_inner()
                    })
                    .record_agent_outcome(
                        phase_name,
                        &agent_name,
                        true,
                        attempt_started.elapsed().as_millis() as u64,
                    );

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

                // Store in token cache
                let input_text =
                    crate::intelligence::token_cache::messages_to_text(&agent_messages);
                let token_count =
                    crate::intelligence::token_cache::estimate_token_count(&output_text);
                let cache = server.cache.token_cache.clone();
                let model_name = base_agent_options
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                store_async(
                    cache,
                    input_text,
                    output_text.clone(),
                    token_count,
                    Some(agent_name.clone()),
                    model_name,
                );

                last_err = None;
                break; // First success wins
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
                server
                    .online_controller
                    .lock()
                    .unwrap_or_else(|poisoned| {
                        warn!("execute_fallback_agents: online_controller poisoned, recovering");
                        poisoned.into_inner()
                    })
                    .record_agent_outcome(
                        phase_name,
                        &agent_name,
                        false,
                        attempt_started.elapsed().as_millis() as u64,
                    );
                let agent_label = agent_name.clone();
                let enriched_err = anyhow::anyhow!(tf(
                    "error.chat.agent_error_prefix",
                    &[("agent", &agent_label), ("error", &err.to_string())]
                ));
                last_err = Some(enriched_err);
            }
        }
    }

    FallbackExecutionResult {
        selected_agent,
        response_text,
        reasoning_text,
        selected_model_name,
        last_err,
        agent_attempts,
        quota_failed_agents,
        _cache_hit: false,
        high_risk_vote_jobs,
    }
}

/// Result of the full_auto execution block (review gate + TAO loop).
struct FullAutoExecutionResult {
    reviews: Vec<Value>,
    tool_execution_results: Vec<Value>,
}

// Execute the FullAuto review gate and TAO loop.
//
// Runs the review gate for full_auto mode, then conditionally executes
// the Think-Act-Observe (TAO) loop when the autonomy loop did not handle
// tool execution. Uses a cached task description to avoid redundant calls.
// ============================================================================
// Section: Full Auto Execution
// ============================================================================

#[allow(clippy::too_many_arguments)]
async fn run_full_auto_execution(
    server: &AcpServer,
    params: &ChatParams,
    phase: &crate::orchestration::flow::ResolvedPhase,
    phase_name: &str,
    span: Option<&OtelContext>,
    trace: &RequestTraceContext,
    selected_agent: &str,
    response_text: &str,
    conversation_id: &str,
    autonomy_loop_executed: bool,
) -> FullAutoExecutionResult {
    let mut reviews = Vec::new();
    let mut tool_execution_results = Vec::new();

    if !params.mode.eq_ignore_ascii_case("full_auto") {
        return FullAutoExecutionResult {
            reviews,
            tool_execution_results,
        };
    }

    // Cache task description once for all full_auto sub-steps
    let task_description = extract_task_description(&params.messages);

    // ── Review gate always runs for full_auto mode ────────────────
    let timeout_before = server
        .observability
        .metrics
        .snapshot()
        .review_gate_timeout_total;
    let review_outcome = run_review_gate(
        server,
        &params.messages,
        phase.options.as_ref(),
        span,
        trace,
    )
    .await;
    let timeout_after = server
        .observability
        .metrics
        .snapshot()
        .review_gate_timeout_total;

    let inferred_degrade_single_timeout = phase
        .options
        .as_ref()
        .map(|opts| {
            let review_timeout_policy = opts
                .extra
                .get("review_timeout_policy")
                .and_then(Value::as_str)
                .unwrap_or("reject");
            let dual_review_enabled = opts
                .full_auto_review_agents
                .as_ref()
                .map(|agents| agents.len() > 1)
                .unwrap_or(false);
            dual_review_enabled && review_timeout_policy.eq_ignore_ascii_case("degrade_single")
        })
        .unwrap_or(false);

    if inferred_degrade_single_timeout && review_outcome.passed && timeout_after == timeout_before {
        server.observability.metrics.inc_review_gate_timeout();
        server.observability.metrics.inc_review_gate_degraded();
    }

    if let Some(error) = &review_outcome.error {
        tracing::warn!("review gate failed: {}", error);
    }
    reviews = review_outcome.reviews.clone();

    // ── Only run TAO loop when the autonomy loop did NOT handle execution ──
    if !autonomy_loop_executed && review_outcome.passed {
        let tool_input = ToolInput {
            task_id: "chat".to_string(),
            phase: phase_name.to_string(),
            agent_role: selected_agent.to_string(),
            objective: task_description.clone(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "task": task_description,
                "phase": phase_name,
            }),
            allowed_base_dir: None,
        };

        let tool_registry = ToolRegistry::new();

        let preferred_tools: Vec<String> = {
            let calls = extract_tool_calls_from_response(response_text, 5);
            if calls.is_empty() {
                record_planner_guided_route();
                planner_guided_tool_preferences(
                    conversation_id,
                    phase_name,
                    selected_agent,
                    &task_description,
                    response_text,
                    5,
                )
            } else {
                record_explicit_tool_route();
                calls
            }
        };

        // ── FullAutoFlow execution (BLUE43) ─────────────────────────
        let full_auto_result = crate::acp::helpers::autonomy_loop_adapter::run_full_auto_flow(
            server.skill_registry.clone(),
            &task_description,
        )
        .await;
        match &full_auto_result {
            Ok(result) => {
                tool_execution_results.push(json!({
                    "tool_loop": "full_auto_flow",
                    "status": "completed",
                    "response": result.response,
                    "reasoning": result.reasoning,
                    "total_steps": result.report.total_tools,
                    "duration_ms": result.report.total_duration_ms,
                    "stop_reason": result.report.stop_reason,
                }));
            }
            Err(e) => {
                tracing::warn!("FullAutoFlow execution failed: {}", e);
                tool_execution_results.push(json!({
                    "tool_loop": "full_auto_flow",
                    "status": "failed",
                    "error": e.to_string(),
                }));
            }
        }

        let should_run_tao = !preferred_tools.is_empty()
            && full_auto_result
                .as_ref()
                .map(|result| result.report.total_tools > 0)
                .unwrap_or(true);

        if should_run_tao {
            let tao_config = LoopConfig::default();
            let (tao_decision, tao_trace) = execute_loop(
                &task_description,
                &tool_registry,
                &tool_input,
                &preferred_tools,
                &tao_config,
            );

            let tool_result = match &tao_decision {
                LoopDecision::Complete(output) => {
                    record_autonomy_loop_stop_reason("complete");
                    serde_json::json!({
                        "status": "complete",
                        "success": output.success,
                        "result": output.result,
                        "iterations": tao_trace.iterations.len(),
                        "duration_ms": tao_trace.total_duration_ms,
                    })
                }
                LoopDecision::Failed { reason, .. } => {
                    record_autonomy_loop_stop_reason("failed");
                    serde_json::json!({
                        "status": "failed",
                        "reason": reason,
                        "iterations": tao_trace.iterations.len(),
                        "duration_ms": tao_trace.total_duration_ms,
                    })
                }
                LoopDecision::Escalate { reason, .. } => {
                    record_autonomy_loop_stop_reason("escalated");
                    serde_json::json!({
                        "status": "escalated",
                        "reason": reason,
                        "iterations": tao_trace.iterations.len(),
                        "duration_ms": tao_trace.total_duration_ms,
                    })
                }
                _ => {
                    record_autonomy_loop_stop_reason("incomplete");
                    serde_json::json!({
                        "status": "incomplete",
                        "iterations": tao_trace.iterations.len(),
                        "duration_ms": tao_trace.total_duration_ms,
                    })
                }
            };

            tool_execution_results.push(json!({
                "tool_loop": "tao_executed",
                "decision": tool_result,
                "trace": serde_json::to_value(&tao_trace).unwrap_or_default(),
                "task": task_description
            }));
        } else {
            tool_execution_results.push(json!({
                "tool_loop": "tao_skipped",
                "status": "skipped",
                "reason": "no_actionable_tools",
                "task": task_description,
            }));
        }
    }

    FullAutoExecutionResult {
        reviews,
        tool_execution_results,
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
async fn apply_review_gate_assemble(
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
    let memory_promotion_result = if params.mode.eq_ignore_ascii_case("full_auto") {
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
        if params.mode.eq_ignore_ascii_case("full_auto") {
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
    let role_routing_result = if params.mode.eq_ignore_ascii_case("full_auto") {
        Some(build_role_routing(&task_description))
    } else {
        None
    };

    // Enhanced verification system integration
    let verification_result = if params.mode.eq_ignore_ascii_case("full_auto") {
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

/// Calls an agent and collects its streamed response.
/// Returns `(response_text, reasoning_text, selected_model)`.
/// The third element is `Some(model_id)` when the agent
/// explicitly reports which model it used (e.g. Copilot auto-select).
pub(crate) async fn run_agent_collecting(
    server: &AcpServer,
    stream_ctx: StreamNotificationContext<'_>,
    agent: Arc<dyn crate::agent::Agent>,
    messages: Vec<Message>,
    principles: Option<Vec<String>>,
    options: Option<std::collections::HashMap<String, Value>>,
    timeout_duration: Option<Duration>,
) -> Result<(String, String, Option<String>)> {
    use crate::acp::r#impl::request::tools_pack::execute_mcp_tool_call;
    let base_messages = messages.clone();
    let followup_agent = Arc::clone(&agent);
    let followup_principles = principles.clone();
    let followup_options = options.clone();

    let (sender, mut receiver) = mpsc::channel::<String>(2048);
    let sender = crate::agent::StreamingSender::from(sender);
    let task = tokio::spawn(async move { agent.chat(messages, principles, options, sender).await });

    let collect = async move {
        let stream_started = Instant::now();
        let mut response = String::new();
        let mut reasoning_buffer = String::new();
        let mut tool_calls: Vec<(String, String)> = Vec::new();
        let mut chunk_index = 0usize;
        let mut total_chars = 0usize;
        let mut selected_model: Option<String> = None;
        while let Some(token) = receiver.recv().await {
            // Check for model-used token (prefixed with __model_used__)
            // This is sent by CopilotAgent after a successful auto-select.
            if let Some(model_id) = token.strip_prefix("__model_used__:") {
                selected_model = Some(model_id.trim().to_string());
                continue;
            }

            // Check for tool call tokens (prefixed with __tool_call__)
            if let Some(tool_call_data) = token.strip_prefix("__tool_call__:") {
                // Format: __tool_call__:<tool_name>:<json_arguments>
                if let Some(colon_pos) = tool_call_data.find(':') {
                    let tool_name = &tool_call_data[..colon_pos];
                    let tool_args = &tool_call_data[colon_pos + 1..];
                    tool_calls.push((tool_name.to_string(), tool_args.to_string()));
                }
                continue;
            }

            let next_chars = token.chars().count();
            if stream_would_exceed_limits(chunk_index, total_chars, next_chars) {
                anyhow::bail!(t("error.chat.stream_output_limits"));
            }

            // Check for reasoning tokens (prefixed with __thinking__)
            if let Some(reasoning_token) = token.strip_prefix("__thinking__") {
                reasoning_buffer.push_str(reasoning_token);
            } else {
                response.push_str(&token);
            }

            chunk_index += 1;
            total_chars += next_chars;

            let display_token = if token.starts_with("__thinking__") {
                ""
            } else {
                &token
            };
            emit_stream_chunk(
                server,
                stream_ctx.stream_observer.as_ref(),
                StreamEventMeta {
                    agent_name: stream_ctx.agent_name,
                    phase_name: stream_ctx.phase_name,
                    trace_id: stream_ctx.trace_id,
                },
                display_token,
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
                    selected_model.clone(),
                )
                .await?;
                // ── Execute tool calls ────────────────────────────────
                // If the LLM responded with tool calls, execute each
                // registered skill and append the results to the response.
                const MAX_TOOL_CALLS_PER_AGENT: usize = 100;
                // ── Skill dedup: prevent AI from calling multiple skills at once ──
                // When the LLM tries to invoke several skills for the same request,
                // pick the single best one automatically. This stops indecisive AI
                // behavior where multiple nearly-identical skills are invoked together.
                let tool_calls = {
                    // Identify which tool calls are skills vs. built-in tools.
                    // Built-in tools (skill-finder, goon_*, etc.) are excluded
                    // from the multi-call dedup check.
                    let is_builtin = |name: &str| -> bool {
                        name == "skill-finder"
                            || name == "skill-creator"
                            || name == "acp_trace_get"
                            || name == "acp_debug_panel_get"
                            || name.starts_with("goon_")
                    };
                    let skill_names: Vec<&str> = tool_calls
                        .iter()
                        .filter(|(name, _)| !is_builtin(name))
                        .map(|(name, _)| name.as_str())
                        .collect();
                    if skill_names.len() > 1 {
                        // Multiple skills called at once — pick the best one by score.
                        let best = {
                            let reg = server.skill_registry.lock().unwrap_or_else(|poisoned| {
                                warn!("run_agent_collecting: skill_registry poisoned, recovering");
                                poisoned.into_inner()
                            });
                            skill_names
                                .iter()
                                .filter_map(|name| {
                                    let score = reg.score_of(name).unwrap_or(0.5);
                                    reg.get(name).map(|_| (name.to_string(), score))
                                })
                                .max_by(|a, b| {
                                    a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                                })
                        };
                        if let Some((best_name, _)) = best {
                            warn!(
                                "skill dedup: AI called {} skills ({}), auto-selecting '{}'",
                                skill_names.len(),
                                skill_names.join(", "),
                                best_name
                            );
                            tool_calls
                                .into_iter()
                                .filter(|(name, _)| *name == best_name)
                                .collect::<Vec<_>>()
                        } else {
                            tool_calls
                        }
                    } else {
                        tool_calls
                    }
                };

                if tool_calls.len() >= MAX_TOOL_CALLS_PER_AGENT {
                    warn!(
                        "run_agent_collecting: tool_calls limit reached ({}), truncating",
                        MAX_TOOL_CALLS_PER_AGENT
                    );
                }
                let mut tool_results: Vec<String> = Vec::new();
                for (tool_name, tool_args_str) in tool_calls.iter().take(MAX_TOOL_CALLS_PER_AGENT) {
                    let parsed_args: Value =
                        serde_json::from_str(tool_args_str).unwrap_or(json!({}));
                    match execute_mcp_tool_call(server, tool_name, &parsed_args).await {
                        Ok(result) => {
                            let result_text =
                                serde_json::to_string_pretty(&result).unwrap_or_default();
                            let tool_block =
                                build_tool_result_block(tool_name, &result_text, false);
                            tool_results.push(tool_block);
                        }
                        Err(err) => {
                            let err_block =
                                build_tool_result_block(tool_name, &err.to_string(), true);
                            tool_results.push(err_block);
                        }
                    }
                }
                if !tool_results.is_empty() {
                    let combined = tool_results.join("\n");
                    let mut followup_messages = base_messages.clone();
                    if !response.trim().is_empty() {
                        followup_messages.push(Message {
                            role: "assistant".to_string(),
                            content: response.clone(),
                        });
                    }
                    followup_messages.push(Message {
                        role: "user".to_string(),
                        content: build_tool_execution_followup_message(&tool_results, true),
                    });

                    let followup = run_followup_after_tool_observation(
                        Arc::clone(&followup_agent),
                        followup_messages,
                        followup_principles.clone(),
                        followup_options.clone(),
                        timeout_duration,
                    )
                    .await;
                    record_tool_followup_attempt();

                    match followup {
                        Ok((followup_response, followup_reasoning, followup_model))
                            if !followup_response.trim().is_empty() =>
                        {
                            record_tool_followup_success();
                            response = followup_response;
                            if !followup_reasoning.is_empty() {
                                reasoning_buffer.push_str(&followup_reasoning);
                            }
                            if selected_model.is_none() {
                                selected_model = followup_model;
                            }
                        }
                        _ => {
                            record_tool_followup_fallback();
                            response.push_str("\n\n");
                            response.push_str(&combined);
                        }
                    }

                    // Emit the tool result block via stream if an observer is attached.
                    if let Some(ref observer) = stream_ctx.stream_observer {
                        let meta = StreamEventMeta {
                            agent_name: stream_ctx.agent_name,
                            phase_name: stream_ctx.phase_name,
                            trace_id: stream_ctx.trace_id,
                        };
                        emit_stream_chunk(
                            server,
                            Some(observer),
                            meta,
                            &combined,
                            chunk_index,
                            total_chars,
                        )
                        .await?;
                    }
                }
                Ok::<(String, String, Option<String>), anyhow::Error>((
                    response,
                    reasoning_buffer,
                    selected_model,
                ))
            }
            Ok(Err(err)) => Err(err.into()),
            Err(join_err) => Err(anyhow::anyhow!(tf(
                "error.chat.agent_task_panicked",
                &[("error", &join_err.to_string())]
            ))),
        }
    };

    run_with_optional_timeout(timeout_duration, collect, |duration| {
        anyhow::anyhow!(tf(
            "error.chat.agent_request_timeout",
            &[("seconds", &duration.as_secs().max(1).to_string())]
        ))
    })
    .await
    .inspect_err(|err| {
        if err.to_string().to_ascii_lowercase().contains("timed out") {
            server.observability.metrics.inc_agent_timeout_failure();
        }
    })
}

// Stream event type constants to avoid repeated allocations
const STREAM_EVENT_CHUNK: &str = "chunk";
const STREAM_EVENT_DONE: &str = "done";
const STREAM_EVENT_TELEMETRY: &str = "telemetry";

// ── SseBufferPool (GAP-46-12 / BLUE48 Step 2) ────────────────────────
// Global pool of pre-allocated byte buffers for SSE event serialization.
// Avoids allocation churn during high-frequency streaming by reusing
// buffers across requests.  Pre-initialized at server startup to avoid
// first-request latency penalty.
static SSE_BUFFER_POOL: OnceLock<SseBufferPool> = OnceLock::new();

/// Pre-initialize the SSE buffer pool at server startup.
/// Call once during server initialization to avoid first-request latency.
pub fn pre_init_sse_buffer_pool() {
    SSE_BUFFER_POOL.get_or_init(|| SseBufferPool::new(4, 4096));
    tracing::info!("SSE buffer pool pre-initialized (4 buffers x 4096 bytes)");
}

/// Acquire a buffer from the global SSE buffer pool.
/// Returns a pre-allocated (empty) `Vec<u8>` suitable for building an SSE frame.
pub(crate) fn acquire_sse_buffer() -> Vec<u8> {
    SSE_BUFFER_POOL
        .get_or_init(|| SseBufferPool::new(4, 4096))
        .acquire()
}

/// Release a buffer back to the global SSE buffer pool for reuse.
pub(crate) fn release_sse_buffer(buf: Vec<u8>) {
    if let Some(pool) = SSE_BUFFER_POOL.get() {
        pool.release(buf);
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

    // Check if this is a reasoning token (prefixed with __thinking__)
    let (display_token, reasoning_token) = if let Some(rest) = token.strip_prefix("__thinking__") {
        ("", rest)
    } else {
        (token, "")
    };

    // Use as_ref() to avoid cloning response_id
    if let Some(response_id) = observer.jsonrpc_response_id.as_ref() {
        crate::acp::r#impl::io::send_notification(
            server,
            "chat.stream.chunk",
            stream_chunk_notification(
                Some(response_id),
                meta.agent_name,
                display_token,
                chunk_index,
                total_chars,
                None,
                Some(meta.phase_name),
                Some(meta.trace_id),
                if reasoning_token.is_empty() {
                    None
                } else {
                    Some(reasoning_token)
                },
            ),
        )
        .await?;
    }

    if let Some(sender) = &observer.sse_sender {
        let mut payload = json!({
            "agent": meta.agent_name,
            "chunk_index": chunk_index,
            "phase": meta.phase_name,
            "token": display_token,
            "total_chars": total_chars,
            "trace_id": meta.trace_id,
        });
        if !reasoning_token.is_empty() {
            payload["reasoning"] = json!(reasoning_token);
        }
        // Send failure is expected when client disconnects — non-critical.
        let _ = sender
            .send(StreamFrame {
                event: STREAM_EVENT_CHUNK,
                payload,
            })
            .await;
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
    // Actual model name reported by the agent (e.g. "gemini-2.5-pro" for copilot).
    // Passed through to SSE payload so the GUI can display it.
    selected_model: Option<String>,
) -> Result<()> {
    let Some(observer) = observer else {
        return Ok(());
    };

    // Use as_ref() to avoid cloning response_id
    if let Some(response_id) = observer.jsonrpc_response_id.as_ref() {
        crate::acp::r#impl::io::send_notification(
            server,
            "chat.stream.done",
            stream_done_notification(
                Some(response_id),
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
        // NOTE: This SSE frame structure should match helpers/metrics::stream_done_notification
        let mut payload = json!({
            "agent": meta.agent_name,
            "chunks": chunk_index,
            "done": true,
            "duration_ms": duration_ms,
            "phase": meta.phase_name,
            "total_chars": total_chars,
            "trace_id": meta.trace_id,
        });
        if let Some(ref m) = selected_model {
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("selected_model".to_string(), json!(m));
            }
        }
        // Send failure is expected when client disconnects — non-critical.
        let _ = sender
            .send(StreamFrame {
                event: STREAM_EVENT_DONE,
                payload,
            })
            .await;
    }

    Ok(())
}

async fn emit_stream_token_economy(
    server: &AcpServer,
    observer: Option<&StreamObserver>,
    meta: StreamEventMeta<'_>,
    token_economy: &Value,
) -> Result<()> {
    let Some(observer) = observer else {
        return Ok(());
    };

    // Use as_ref() to avoid cloning response_id
    if let Some(response_id) = observer.jsonrpc_response_id.as_ref() {
        crate::acp::r#impl::io::send_notification(
            server,
            "chat.stream.telemetry",
            json!({
                "id": response_id,
                "agent": meta.agent_name,
                "phase": meta.phase_name,
                "trace_id": meta.trace_id,
                "token_economy": token_economy,
            }),
        )
        .await?;
    }

    if let Some(sender) = &observer.sse_sender {
        // Send failure is expected when client disconnects — non-critical.
        let _ = sender
            .send(StreamFrame {
                event: STREAM_EVENT_TELEMETRY,
                payload: json!({
                    "agent": meta.agent_name,
                    "phase": meta.phase_name,
                    "trace_id": meta.trace_id,
                    "token_economy": token_economy,
                }),
            })
            .await;
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

/// Create default requirement contract
#[allow(dead_code)] // F-GAP-17 — reserved for default requirement contract wiring
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
pub(crate) struct StreamNotificationContext<'a> {
    pub(crate) stream_observer: Option<StreamObserver>,
    pub(crate) agent_name: &'a str,
    pub(crate) phase_name: &'a str,
    pub(crate) trace_id: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamEventMeta<'a> {
    pub(crate) agent_name: &'a str,
    pub(crate) phase_name: &'a str,
    pub(crate) trace_id: &'a str,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StreamFrame {
    pub event: &'static str,
    pub payload: Value,
}

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
pub(crate) struct EffectiveVectorSettings {
    pub(crate) min_query_chars: usize,
    pub(crate) top_k: usize,
    pub(crate) min_similarity: f32,
    pub(crate) max_snippet_chars: usize,
    pub(crate) summary_enabled: bool,
    pub(crate) summary_trigger_messages: usize,
    pub(crate) summary_max_chars: usize,
    pub(crate) auto_mode: bool,
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

    let Some(store) = server.cache.vector_store.clone() else {
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
            server
                .observability
                .metrics
                .record_vector_search(hits.len());
            apply_autotune_feedback(
                server,
                phase_options,
                settings.auto_mode,
                feedback.avg_similarity,
            )
            .await;
            let reranked_hits =
                rerank_hits_with_phase_summary(hits, summary.as_deref(), settings.top_k);
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

pub(crate) fn rerank_hits_with_phase_summary(
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

pub(crate) fn load_recent_knowledge_context(
    server: &AcpServer,
    phase_name: &str,
    limit: usize,
) -> Vec<String> {
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
    let Some(store) = server.cache.vector_store.clone() else {
        return;
    };
    let Some(query_text) = latest_user_message(&params.messages) else {
        return;
    };

    if let Err(err) = store.upsert(phase_name, query_text, response_text) {
        warn!(phase = phase_name, error = %err, "vector upsert failed");
    } else {
        server.observability.metrics.record_vector_store();
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
                server.observability.metrics.record_summary_store();
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

    let result = match run_agent_collecting(
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
        Some(Duration::from_secs(timeout_seconds)),
    )
    .await
    {
        Ok((text, _, _)) => text,
        Err(e) => {
            tracing::warn!("phase summary generation failed: {}", e);
            return None;
        }
    };

    let compact = result.trim();
    if compact.is_empty() {
        return None;
    }

    Some(normalize_phase_summary(
        compact,
        &fallback_summary,
        max_chars,
    ))
}

fn build_summary_dialogue(messages: &[Message], response_text: &str, max_chars: usize) -> String {
    let mut parts = messages.iter().rev().take(8).collect::<Vec<_>>();
    parts.reverse();

    let mut rendered = parts
        .into_iter()
        .map(|m| format!("{}: {}", m.role, m.content.trim()))
        .collect::<Vec<_>>();
    rendered.push(format!("assistant: {}", response_text.trim()));
    truncate_chars(&rendered.join("\n"), max_chars)
}

pub(crate) async fn load_phase_summary(
    server: &AcpServer,
    phase_name: &str,
    phase_options: Option<&PhaseOptions>,
) -> Option<String> {
    let settings = effective_vector_settings(server, phase_options).await?;
    if !settings.summary_enabled {
        return None;
    }

    let store = server.cache.vector_store.clone()?;

    match store.get_phase_summary(phase_name) {
        Ok(summary) => {
            server
                .observability
                .metrics
                .record_summary_read(summary.is_some());
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
    if matches!(
        phase_options.and_then(|opts| opts.vector_enabled),
        Some(false)
    ) {
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
    if !auto_mode
        || matches!(
            phase_options.and_then(|opts| opts.vector_enabled),
            Some(false)
        )
    {
        return;
    }

    let (Some(autotune), Some(config)) =
        (server.autotune.as_ref(), server.autotune_config.as_ref())
    else {
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
        .find(|message| {
            message.role.eq_ignore_ascii_case("user") && !message.content.trim().is_empty()
        })
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

pub(crate) fn build_phase_summary(
    messages: &[Message],
    response_text: &str,
    max_chars: usize,
) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let intent = latest_user_message(messages)
        .map(|text| truncate_chars(text, 180))
        .unwrap_or_else(|| "Continue and complete the active phase request".to_string());

    let constraints = collect_signal_lines(
        messages,
        &[
            "must", "need", "require", "不要", "不能", "必须", "完整", "一次",
        ],
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
        &[
            "implemented",
            "enabled",
            "fixed",
            "added",
            "updated",
            "完成",
            "已",
            "接入",
        ],
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
        &[
            "risk", "pending", "todo", "warning", "timeout", "fallback", "风险", "待", "告警",
        ],
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
        if keywords.iter().any(|keyword| {
            lower.contains(&keyword.to_ascii_lowercase()) || trimmed.contains(keyword)
        }) {
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

    let mut store = server.memory_store.lock().unwrap_or_else(|poisoned| {
        warn!("persist_chat_knowledge: memory_store poisoned, recovering");
        poisoned.into_inner()
    });
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
    let promoted_count = promotion.promoted_count;
    let retained_entries = store
        .retrieve(crate::memory_module::MemoryClass::Observation, 256)
        .len()
        + store
            .retrieve(crate::memory_module::MemoryClass::Episodic, 256)
            .len()
        + store
            .retrieve(crate::memory_module::MemoryClass::Semantic, 256)
            .len()
        + store
            .retrieve(crate::memory_module::MemoryClass::ProjectState, 256)
            .len();

    let mut vector_memory_written = false;
    if let Some(vector_store) = server.cache.vector_store.clone() {
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
            server.observability.metrics.record_vector_store();
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
        "retained_entries": &retained_entries,
        "promoted_count": &promoted_count,
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
    let base = if reusable_insights.is_empty() {
        0.72
    } else {
        0.82
    };
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

#[allow(clippy::too_many_arguments)]
async fn persist_session_distillation(
    server: &AcpServer,
    conversation_id: &str,
    branch_id: &str,
    phase_name: &str,
    params: &ChatParams,
    selected_agent: &str,
    candidate_agents: &[String],
    agent_attempts: &[Value],
    response_text: &str,
) -> Value {
    let task = extract_task_description(&params.messages);
    let success_count = agent_attempts
        .iter()
        .filter(|attempt| attempt.get("ok").and_then(Value::as_bool) == Some(true))
        .count();
    let failure_count = agent_attempts.len().saturating_sub(success_count);
    let success_rate = if agent_attempts.is_empty() {
        1.0
    } else {
        success_count as f64 / agent_attempts.len() as f64
    };
    let merged_agents = if candidate_agents.is_empty() {
        vec![selected_agent.to_string()]
    } else {
        candidate_agents.to_vec()
    };
    let distill_params = json!({
        "learning_mode": "adaptive",
        "memory_scope": "task_and_repo",
        "repair_iterations": failure_count,
        "distill_scope": "task_repo_runtime",
        "evolution_mode": "continuous",
    });
    let mut learning_profile = crate::acp::r#impl::request::build_learning_profile(
        "session.distill",
        &task,
        &distill_params,
    );
    if let Some(obj) = learning_profile.as_object_mut() {
        obj.insert(
            "session".to_string(),
            json!({
                "conversation_id": conversation_id,
                "branch_id": branch_id,
                "phase": phase_name,
                "selected_agent": selected_agent,
                "agents_considered": merged_agents,
                "agent_attempts_total": agent_attempts.len(),
                "success_rate": round_metric(success_rate),
            }),
        );
    }

    let mut knowledge_refinement = crate::acp::r#impl::request::build_knowledge_refinement_profile(
        "session.distill",
        &task,
        &distill_params,
        &learning_profile,
    );
    if let Some(obj) = knowledge_refinement.as_object_mut() {
        obj.insert(
            "merge".to_string(),
            json!({
                "selected_agent": selected_agent,
                "agents_considered": candidate_agents,
                "agents_succeeded": success_count,
                "agents_failed": failure_count,
                "shared_epistemic_base_updated": true,
            }),
        );
    }

    let artifact = json!({
        "generated_at": crate::acp::prelude::now_ts(),
        "conversation_id": conversation_id,
        "branch_id": branch_id,
        "phase": phase_name,
        "task": truncate_chars(&task, 200),
        "selected_agent": selected_agent,
        "merged_agents": merged_agents,
        "learning_profile": learning_profile,
        "knowledge_refinement": knowledge_refinement,
        "response_excerpt": truncate_chars(response_text, 240),
    });

    let ledger = crate::acp::r#impl::runtime::artifact_ledger(server);
    let artifact_path = ledger
        .write_json("spec", "latest-session-distillation.json", &artifact)
        .ok()
        .map(|path| path.display().to_string());

    let insight = KnowledgeInsightArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        conversation_id: conversation_id.to_string(),
        branch_id: branch_id.to_string(),
        phase: phase_name.to_string(),
        task: truncate_chars(&task, 200),
        agent: "multi-agent.merge".to_string(),
        source: "session_distillation".to_string(),
        request_excerpt: truncate_chars(&task, 200),
        response_excerpt: truncate_chars(response_text, 240),
        reusable_insights: vec![format!(
            "Selected agent '{}' after considering {} agent(s)",
            selected_agent,
            candidate_agents.len().max(1)
        )],
        verification_steps: agent_attempts
            .iter()
            .filter_map(|attempt| attempt.get("error").and_then(Value::as_str))
            .take(3)
            .map(|error| truncate_chars(error, 120))
            .collect(),
        confidence: round_metric((0.70 + success_rate * 0.25).clamp(0.0, 0.98)),
    };
    let knowledge_artifact_path = persist_knowledge_insight_event(&ledger, insight, 256)
        .ok()
        .map(|path| path.display().to_string());

    let analyzed = TaskRouter::analyze_task(&task);
    let learning_event = WorkflowLearningEvent {
        generated_at: crate::acp::prelude::now_ts(),
        task: truncate_chars(&task, 200),
        complexity: analyzed.complexity,
        predicted_success_rate: success_rate as f32,
        subtasks_total: agent_attempts.len().max(1),
        subtasks_completed: success_count,
        subtasks_failed: failure_count,
        subtasks_skipped: 0,
        serial_work_ms: 0,
        critical_path_ms: 0,
        parallel_speedup: if agent_attempts.len() > 1 { 1.0 } else { 0.0 },
        parallel_efficiency: if agent_attempts.len() > 1 {
            round_metric(success_rate)
        } else {
            1.0
        },
        executor: selected_agent.to_string(),
        source: "session_distillation".to_string(),
        runtime_healthy: server.get_status().lifecycle.is_healthy,
        gates_ok: true,
        work_grade: if failure_count == 0 {
            "A".to_string()
        } else if success_count > 0 {
            "B".to_string()
        } else {
            "C".to_string()
        },
        risk_score: round_metric((1.0 - success_rate).clamp(0.0, 1.0)),
        clarification_rounds: 0,
        clarification_quality_score: 1.0,
        requirement_change_count: 0,
        review_reject_root_cause: String::new(),
        primary_stability_score: round_metric(success_rate),
        secondary_utilization_rate: if agent_attempts.len() > 1 {
            round_metric(
                (agent_attempts.len().saturating_sub(1)) as f64 / agent_attempts.len() as f64,
            )
        } else {
            0.0
        },
        failover_count: failure_count as u32,
        failover_root_cause: agent_attempts
            .iter()
            .filter_map(|attempt| attempt.get("error").and_then(Value::as_str))
            .next()
            .unwrap_or_default()
            .to_string(),
    };
    let learning_artifact_path = persist_workflow_learning_event(&ledger, learning_event, 256)
        .ok()
        .map(|path| path.display().to_string());

    json!({
        "artifact_path": artifact_path,
        "knowledge_artifact_path": knowledge_artifact_path,
        "learning_artifact_path": learning_artifact_path,
        "shared_epistemic_base_updated": true,
        "merged_agents": candidate_agents,
        "success_rate": round_metric(success_rate),
        "learning_profile": artifact["learning_profile"].clone(),
        "knowledge_refinement": artifact["knowledge_refinement"].clone(),
    })
}

// ============================================================================
// Section: Supporting Utilities
// ============================================================================

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

/// Run tool execution loop for full_auto mode
/// This function integrates the tool execution loop from request.rs into the chat flow
#[cfg(test)]
#[allow(dead_code)]
// F-GAP-49 — reserved for future use
fn run_tool_execution_loop(task: &str, subtask: &str, record_index: usize) -> String {
    // Simplified tool execution loop
    format!(
        "Tool execution loop for task: {} (subtask: {}, index: {})",
        task, subtask, record_index
    )
}

/// Extract model tool calls from response
pub(crate) fn extract_tool_calls_from_response(response: &str, max_calls: usize) -> Vec<String> {
    // Parse only explicit tool-call markers; never synthesize placeholder calls.
    let mut calls: Vec<String> = Vec::with_capacity(max_calls);
    let mut json_block: Vec<String> = Vec::with_capacity(32);
    let mut in_json_block = false;

    let flush_json_block = |json_block: &mut Vec<String>, calls: &mut Vec<String>| {
        if json_block.is_empty() {
            return;
        }

        let block = json_block.join("\n");
        json_block.clear();

        let Ok(value) = serde_json::from_str::<Value>(&block) else {
            return;
        };

        let mut push_call = |call_name: &str| {
            let candidate = call_name.trim();
            if candidate.is_empty() {
                return;
            }

            let valid_name = candidate
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
            if !valid_name {
                return;
            }

            if !calls.iter().any(|name| name == candidate) {
                calls.push(candidate.to_string());
            }
        };

        match value {
            Value::Object(map) => {
                if let Some(tool_call) = map.get("tool_call").and_then(Value::as_str) {
                    push_call(tool_call);
                }

                if let Some(tool_calls) = map.get("tool_calls").and_then(Value::as_array) {
                    for item in tool_calls {
                        match item {
                            Value::String(name) => push_call(name),
                            Value::Object(object) => {
                                if let Some(name) = object.get("name").and_then(Value::as_str) {
                                    push_call(name);
                                } else if let Some(name) =
                                    object.get("tool").and_then(Value::as_str)
                                {
                                    push_call(name);
                                }
                            }
                            _ => {}
                        }
                    }
                }

                if let Some(actions) = map.get("actions").and_then(Value::as_array) {
                    for item in actions {
                        match item {
                            Value::String(name) => push_call(name),
                            Value::Object(object) => {
                                if let Some(name) = object.get("name").and_then(Value::as_str) {
                                    push_call(name);
                                } else if let Some(name) =
                                    object.get("tool").and_then(Value::as_str)
                                {
                                    push_call(name);
                                } else if let Some(name) =
                                    object.get("action").and_then(Value::as_str)
                                {
                                    push_call(name);
                                }
                            }
                            _ => {}
                        }
                    }
                }

                if let Some(action_plan) = map.get("action_plan") {
                    if let Some(action_plan_actions) =
                        action_plan.get("actions").and_then(Value::as_array)
                    {
                        for item in action_plan_actions {
                            match item {
                                Value::String(name) => push_call(name),
                                Value::Object(object) => {
                                    if let Some(name) = object.get("name").and_then(Value::as_str) {
                                        push_call(name);
                                    } else if let Some(name) =
                                        object.get("tool").and_then(Value::as_str)
                                    {
                                        push_call(name);
                                    } else if let Some(name) =
                                        object.get("action").and_then(Value::as_str)
                                    {
                                        push_call(name);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    if let Value::Object(object) = item {
                        if let Some(name) = object.get("name").and_then(Value::as_str) {
                            push_call(name);
                        } else if let Some(name) = object.get("tool").and_then(Value::as_str) {
                            push_call(name);
                        } else if let Some(name) = object.get("action").and_then(Value::as_str) {
                            push_call(name);
                        }
                    }
                }
            }
            _ => {}
        }
    };

    for line in response.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            if in_json_block {
                flush_json_block(&mut json_block, &mut calls);
                in_json_block = false;
                if calls.len() >= max_calls {
                    break;
                }
                continue;
            }

            let fence_lang = trimmed.trim_start_matches("```").trim();
            in_json_block = fence_lang.is_empty() || fence_lang.eq_ignore_ascii_case("json");
            continue;
        }

        if in_json_block {
            json_block.push(trimmed.to_string());
            continue;
        }

        let marker_value = trimmed
            .strip_prefix("__tool_call__")
            .map(|value| value.trim_start_matches(':').trim())
            .or_else(|| trimmed.strip_prefix("tool_call:").map(str::trim))
            .or_else(|| trimmed.strip_prefix("tool:").map(str::trim));

        let Some(raw_name) = marker_value else {
            continue;
        };

        let candidate = raw_name
            .split(|c: char| c == '(' || c == '{' || c == ':' || c.is_whitespace())
            .next()
            .unwrap_or("")
            .trim();

        if candidate.is_empty() {
            continue;
        }

        let valid_name = candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
        if !valid_name {
            continue;
        }

        if !calls.iter().any(|name| name == candidate) {
            calls.push(candidate.to_string());
        }

        if calls.len() >= max_calls {
            break;
        }
    }

    if in_json_block {
        flush_json_block(&mut json_block, &mut calls);
    }

    calls
}

/// Execute model tool calls
#[cfg(test)]
#[allow(dead_code)]
// F-GAP-49 — reserved for future use
fn execute_tool_calls(
    task: &str,
    subtask: &str,
    record_index: usize,
    calls: &[String],
) -> Vec<String> {
    // Simplified tool execution
    calls
        .iter()
        .map(|call| {
            format!(
                "Executed {} for task {} (subtask: {}, index: {})",
                call, task, subtask, record_index
            )
        })
        .collect()
}

/// A detected repeated task pattern in a conversation.
/// Used by P3 to proactively propose skill creation.
#[allow(dead_code)] // F-GAP-17
struct DetectedTaskPattern {
    /// Suggested skill name
    name: String,
    /// Suggested skill description
    description: String,
    /// How many times the pattern was observed
    occurrence_count: usize,
    /// The keyword cluster that identifies this pattern
    keywords: Vec<String>,
}

/// Detect repeated task patterns across user messages.
///
/// Analyzes all user messages for common keyword clusters that indicate
/// the same type of task is being requested multiple times.
/// Returns `Some(DetectedTaskPattern)` when a pattern appears 3+ times.
fn detect_repeated_task_pattern(messages: &[&str]) -> Option<DetectedTaskPattern> {
    if messages.len() < 3 {
        return None;
    }

    // Define keyword clusters for common task types as owned strings
    let task_clusters: Vec<(Vec<&str>, &str, &str)> =
        vec![
        (
            vec!["refactor", "restructure", "reorganize", "clean up", "cleanup", "technical debt"],
            "code-refactoring",
            "Refactors and restructures code to improve maintainability and reduce technical debt",
        ),
        (
            vec!["test", "unit test", "integration test", "e2e", "test coverage", "assert"],
            "testing",
            "Creates and runs tests including unit, integration, and end-to-end tests",
        ),
        (
            vec!["document", "readme", "docstring", "comment", "documentation", "docs"],
            "documentation",
            "Generates and updates documentation including README, docstrings, and technical docs",
        ),
        (
            vec!["debug", "fix", "bug", "issue", "error", "crash", "failing", "broken"],
            "bug-fixing",
            "Diagnoses and fixes bugs, errors, and crashes in the codebase",
        ),
        (
            vec!["optimize", "performance", "slow", "bottleneck", "speed up", "faster"],
            "performance-optimization",
            "Optimizes code performance by identifying and fixing bottlenecks",
        ),
        (
            vec!["api", "endpoint", "route", "rest", "graphql", "grpc"],
            "api-development",
            "Designs, implements, and documents API endpoints and integrations",
        ),
        (
            vec!["review", "code review", "audit", "inspect", "check quality"],
            "code-review",
            "Reviews code for quality, security, and adherence to best practices",
        ),
        (
            vec!["deploy", "ci/cd", "pipeline", "release", "rollout", "rollback"],
            "deployment",
            "Manages deployment, CI/CD pipelines, and release processes",
        ),
        (
            vec!["migrate", "migration", "upgrade", "port", "convert", "transpile"],
            "migration",
            "Migrates code between frameworks, languages, or versions",
        ),
        (
            vec!["config", "configure", "setup", "install", "initialize", "bootstrap"],
            "configuration",
            "Handles configuration, setup, and initialization of projects and tools",
        ),
    ];

    // Count how many messages match each cluster
    let mut cluster_hits: Vec<(usize, Vec<&str>, &str, &str)> = task_clusters
        .into_iter()
        .map(|(keywords, name, description)| {
            let count = messages
                .iter()
                .filter(|msg| {
                    let lower = msg.to_lowercase();
                    keywords.iter().any(|kw| lower.contains(kw))
                })
                .count();
            (count, keywords, name, description)
        })
        .collect();

    // Sort by hit count descending
    cluster_hits.sort_by_key(|b| std::cmp::Reverse(b.0));

    // Return the best match if it appears 3+ times
    if !cluster_hits.is_empty() {
        let (count, keywords, name, description) = cluster_hits.swap_remove(0);
        if count >= 3 {
            return Some(DetectedTaskPattern {
                name: name.to_string(),
                description: description.to_string(),
                occurrence_count: count,
                keywords: keywords.into_iter().map(|s| s.to_string()).collect(),
            });
        }
    }

    None
}

/// Analyze a completed chat conversation and auto-create skills for
/// repetitive task patterns that would benefit from being a reusable skill.
async fn auto_create_skills_from_conversation(
    server: &AcpServer,
    chat_params: &ChatParams,
    response_text: &str,
) -> Result<Vec<String>> {
    let mut created_skills = Vec::new();

    // Only attempt skill creation if the skill-creator skill is registered
    let has_skill_creator = server
        .skill_registry
        .lock()
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

    if has_creation_intent || has_response_hint || repeated_pattern.is_some() {
        // When a repeated pattern is detected, use its extracted info;
        // otherwise fall back to extracting from the last user message.
        let (skill_name, skill_description) = if let Some(ref pattern) = repeated_pattern {
            (pattern.name.clone(), pattern.description.clone())
        } else {
            let name = generate_skill_name_from_conversation(last_user_msg, response_text);
            let desc = generate_skill_description(last_user_msg, response_text);
            (name, desc)
        };

        if !skill_name.is_empty() && !skill_description.is_empty() {
            // Check if skill already exists
            let exists = server
                .skill_registry
                .lock()
                .ok()
                .map(|registry| registry.get(&skill_name).is_some())
                .unwrap_or(false);

            if !exists {
                let prompt = format!(
                    "You are an AI assistant specialized in: {}\n\nBased on the user's request, execute the following task:\n{}",
                    skill_description, last_user_msg
                );

                let result = {
                    let mut registry = server.skill_registry.lock().unwrap_or_else(|poisoned| {
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
async fn auto_generate_workflow_from_conversation(
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
