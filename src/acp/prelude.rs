// ACP (Agent Coordination Protocol) server implementation
//
// This module implements the core server functionality for the go-on ACP proxy,
// including request handling, caching, vector storage, circuit breaking, and performance monitoring.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use futures_util::stream::{self, StreamExt};
use opentelemetry::{Context as OtelContext, KeyValue};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Mutex, Notify};
use tokio::task::{spawn_blocking, JoinHandle};
use tokio::time::{sleep, timeout, MissedTickBehavior};
use tracing::{debug, error, info, warn};

use crate::adaptive_selector::AdaptiveModelSelector;
use crate::advanced_modules::{DynamicParameterTuner, ResourceAllocator};
use crate::agent::{Agent, AgentRegistry, Message};
use crate::cache::ResponseCache;
use crate::config::{
    is_agent_env_ready, validate_runtime_readiness, AppConfig, AutoTuneConfig, AutoTuneState,
    PhaseOptions, RuntimeConfig, VectorConfig,
};
use crate::cost_optimizer::{CostOptimizer, TaskComplexity as CostTaskComplexity};
use crate::error::ProxyError;
use crate::evaluation::TraceEvent;
use crate::failure_prevention::FailurePrevention;
use crate::flow::{FlowManager, ResolvedPhase};
use crate::flow_with_models::FlowModelSelector;
use crate::memory_response_cache::MemoryResponseCache;
use crate::observability::{push_metric_header, push_scalar_metric};
use crate::performance;
use crate::pua::review_gate_prompt;
use crate::reinforcement::{
    aggregate_status, assistant_excerpt, build_runtime_healthcheck_report, build_task_plan,
    build_workflow_generated_artifact, persist_clarification_session_artifact,
    persist_consultation_artifact, persist_execution_decision, persist_governance_policy,
    persist_pipeline_unified_metrics, persist_primary_secondary_failover_artifact,
    persist_primary_secondary_policy_artifact, persist_requirement_contract,
    persist_runtime_healthcheck, persist_task_plan, persist_workflow_generated,
    persist_workflow_learning_event, persist_workflow_optimization_policy,
    persist_workflow_research, persist_workflow_work_grade,
    recommend_agent_order_from_execution_history, recommend_failure_strategy_from_learning,
    recommend_parallelism_from_learning, recommend_predicted_success_rate_from_learning,
    recommend_reattach_modules_from_policy_history, recommend_work_grade_from_learning,
    run_action_check, total_message_chars, ActionCheckKind, ArtifactLedger, CheckStatus,
    CheckpointSummaryArtifact, ClarificationSessionArtifact, ComponentReport, ConsultationArtifact,
    ExecutionAssignmentRecord, ExecutionDecisionArtifact, ExecutionDecisionCandidate,
    GovernancePolicyArtifact, ParallelPhaseDecisionRecord, PipelineUnifiedMetricsArtifact,
    PrimaryFailoverReportItem, PrimarySecondaryFailoverArtifact, PrimarySecondaryPolicyArtifact,
    RequirementContractArtifact, WorkflowLearningBusArtifact, WorkflowLearningEvent,
    WorkflowOptimizationPolicyArtifact, WorkflowResearchArtifact, WorkflowWorkGradeArtifact,
};
use crate::reinforcement::{
    persist_task_execution_summary, TaskExecutionMetrics, TaskExecutionSummary,
};
use crate::reliability_optimizer::{
    ComplexityLevel as ReliabilityComplexityLevel, ReliabilityOptimizer,
};
use crate::review_controls::{
    review_timeout, review_verdict, ReviewDecision, ReviewGateOutcome, ReviewTimeoutPolicy,
    ReviewVerdict,
};
use crate::roles::AgentRole;
use crate::rpc_protocol::{
    chat_trace_context, child_trace_context, value_to_id, JsonRpcError, JsonRpcRequest,
    JsonRpcResponse, RequestTraceContext,
};
use crate::runtime_controls::OnlineControllerState;
use crate::speed_optimizer::{SpeculationStrategy, SpeedOptimizer, StreamingMode};
use crate::task_router::{RoutingDecision, TaskCharacteristics, TaskRouter};
use crate::telemetry::TelemetryRuntime;
use crate::telemetry_enhanced;
use crate::vector::{VectorHit, VectorStore};
use crate::workflow_optimizer::PredictiveFailureHandler;

const TRACE_BUFFER_MAX: usize = 2048;
static TRACE_COUNTER: AtomicU64 = AtomicU64::new(1);
static CHECKPOINT_COUNTER: AtomicU64 = AtomicU64::new(1);

type Blue5DocCache = StdMutex<Option<(PathBuf, SystemTime, Blue5DocSnapshot)>>;
type AppConfigCache = StdMutex<Option<(PathBuf, SystemTime, Arc<AppConfig>)>>;
type ClarificationArtifactCache =
    StdMutex<Option<(PathBuf, SystemTime, RequirementContractArtifact)>>;

static BLUE5_DOC_CACHE: OnceLock<Blue5DocCache> = OnceLock::new();
static APP_CONFIG_CACHE: OnceLock<AppConfigCache> = OnceLock::new();
static CLARIFICATION_ARTIFACT_CACHE: OnceLock<ClarificationArtifactCache> = OnceLock::new();
static LAZY_BLUE5_DOC_LOOKUP_TOTAL: AtomicU64 = AtomicU64::new(0);
static LAZY_BLUE5_DOC_HIT_TOTAL: AtomicU64 = AtomicU64::new(0);
static LAZY_BLUE5_DOC_RELOAD_TOTAL: AtomicU64 = AtomicU64::new(0);
static LAZY_APP_CONFIG_LOOKUP_TOTAL: AtomicU64 = AtomicU64::new(0);
static LAZY_APP_CONFIG_HIT_TOTAL: AtomicU64 = AtomicU64::new(0);
static LAZY_APP_CONFIG_RELOAD_TOTAL: AtomicU64 = AtomicU64::new(0);
static LAZY_CLARIFICATION_LOOKUP_TOTAL: AtomicU64 = AtomicU64::new(0);
static LAZY_CLARIFICATION_HIT_TOTAL: AtomicU64 = AtomicU64::new(0);
static LAZY_CLARIFICATION_RELOAD_TOTAL: AtomicU64 = AtomicU64::new(0);
const DEFAULT_VECTOR_MIN_QUERY_CHARS: usize = 80;
const DEFAULT_VECTOR_TOP_K: usize = 2;
const DEFAULT_VECTOR_MIN_SIMILARITY: f32 = 0.82;
const DEFAULT_VECTOR_MAX_SNIPPET_CHARS: usize = 800;
const DEFAULT_SUMMARY_TRIGGER_MESSAGES: usize = 8;
const DEFAULT_SUMMARY_MAX_CHARS: usize = 1200;
const DEFAULT_BREAKER_FAILURE_THRESHOLD: u32 = 3;
const DEFAULT_BREAKER_OPEN_SECONDS: i64 = 60;
const MAX_CONVERSATION_ID_LEN: usize = 128;
const MAX_BRANCH_ID_LEN: usize = 64;
const MAX_CHECKPOINT_ID_LEN: usize = 128;
const MAX_CHECKPOINTS_PER_CONVERSATION: usize = 256;
const MAX_CHECKPOINT_MESSAGE_CHARS: usize = 64_000;
const MAX_CONVERSATIONS_TRACKED: usize = 512;
const MAX_STREAM_CHUNKS: usize = 4_096;
const MAX_STREAM_CHARS: usize = 256_000;
const HISTOGRAM_BUCKETS_SECONDS: [f64; 10] =
    [0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0];

#[derive(Debug, Clone, Serialize)]
struct Blue5DocSnapshot {
    lazy_loaded: bool,
    path: String,
    enabled: bool,
    supports_requirement_codiscussion: bool,
    supports_consultation: bool,
    digest: String,
}

#[derive(Debug, Clone, Serialize)]
struct Blue5AutoDecision {
    enabled: bool,
    lazy_loaded: bool,
    should_multi_ai_clarify: bool,
    should_consultation: bool,
    reasons: Vec<String>,
    primary_agent: Option<String>,
    secondary_agents: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct PrimarySecondaryPolicy {
    primary_agent: String,
    secondary_agents: Vec<String>,
    policy_version: String,
    failover_policy: String,
    secondary_max_count: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
struct LazyLoadCacheSnapshot {
    blue5_doc_lookup_total: u64,
    blue5_doc_hit_total: u64,
    blue5_doc_reload_total: u64,
    app_config_lookup_total: u64,
    app_config_hit_total: u64,
    app_config_reload_total: u64,
    clarification_lookup_total: u64,
    clarification_hit_total: u64,
    clarification_reload_total: u64,
}

fn lazy_load_cache_snapshot() -> LazyLoadCacheSnapshot {
    LazyLoadCacheSnapshot {
        blue5_doc_lookup_total: LAZY_BLUE5_DOC_LOOKUP_TOTAL.load(Ordering::Relaxed),
        blue5_doc_hit_total: LAZY_BLUE5_DOC_HIT_TOTAL.load(Ordering::Relaxed),
        blue5_doc_reload_total: LAZY_BLUE5_DOC_RELOAD_TOTAL.load(Ordering::Relaxed),
        app_config_lookup_total: LAZY_APP_CONFIG_LOOKUP_TOTAL.load(Ordering::Relaxed),
        app_config_hit_total: LAZY_APP_CONFIG_HIT_TOTAL.load(Ordering::Relaxed),
        app_config_reload_total: LAZY_APP_CONFIG_RELOAD_TOTAL.load(Ordering::Relaxed),
        clarification_lookup_total: LAZY_CLARIFICATION_LOOKUP_TOTAL.load(Ordering::Relaxed),
        clarification_hit_total: LAZY_CLARIFICATION_HIT_TOTAL.load(Ordering::Relaxed),
        clarification_reload_total: LAZY_CLARIFICATION_RELOAD_TOTAL.load(Ordering::Relaxed),
    }
}

fn reset_lazy_load_cache_snapshot() {
    LAZY_BLUE5_DOC_LOOKUP_TOTAL.store(0, Ordering::Relaxed);
    LAZY_BLUE5_DOC_HIT_TOTAL.store(0, Ordering::Relaxed);
    LAZY_BLUE5_DOC_RELOAD_TOTAL.store(0, Ordering::Relaxed);
    LAZY_APP_CONFIG_LOOKUP_TOTAL.store(0, Ordering::Relaxed);
    LAZY_APP_CONFIG_HIT_TOTAL.store(0, Ordering::Relaxed);
    LAZY_APP_CONFIG_RELOAD_TOTAL.store(0, Ordering::Relaxed);
    LAZY_CLARIFICATION_LOOKUP_TOTAL.store(0, Ordering::Relaxed);
    LAZY_CLARIFICATION_HIT_TOTAL.store(0, Ordering::Relaxed);
    LAZY_CLARIFICATION_RELOAD_TOTAL.store(0, Ordering::Relaxed);
}

fn resolve_blue5_doc_path(config_path: Option<&PathBuf>) -> PathBuf {
    if let Some(path) =
        config_path.and_then(|cfg| cfg.parent().map(|parent| parent.join("blue5.md")))
    {
        return path;
    }
    PathBuf::from("blue5.md")
}

fn load_blue5_doc_lazy(config_path: Option<&PathBuf>) -> Blue5DocSnapshot {
    LAZY_BLUE5_DOC_LOOKUP_TOTAL.fetch_add(1, Ordering::Relaxed);
    let path = resolve_blue5_doc_path(config_path);
    let modified = fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH);

    let cache = BLUE5_DOC_CACHE.get_or_init(|| StdMutex::new(None));
    let mut guard = match cache.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return Blue5DocSnapshot {
                lazy_loaded: true,
                path: path.display().to_string(),
                enabled: false,
                supports_requirement_codiscussion: false,
                supports_consultation: false,
                digest: String::new(),
            };
        }
    };

    if let Some((cached_path, cached_modified, snapshot)) = guard.as_ref() {
        if *cached_path == path && *cached_modified == modified {
            LAZY_BLUE5_DOC_HIT_TOTAL.fetch_add(1, Ordering::Relaxed);
            return snapshot.clone();
        }
    }

    let content = fs::read_to_string(&path).unwrap_or_default();
    let digest = if content.is_empty() {
        String::new()
    } else {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let bytes = hasher.finalize();
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    let content_lower = content.to_ascii_lowercase();
    let enabled = !content.is_empty() && content.contains("BLUE5");
    let supports_requirement_codiscussion = enabled
        && (content.contains("目标 F")
            || content_lower.contains("requirement co-discussion")
            || content.contains("多轮讨论"));
    let supports_consultation =
        enabled && (content_lower.contains("consultation") || content.contains("会诊"));

    let snapshot = Blue5DocSnapshot {
        lazy_loaded: true,
        path: path.display().to_string(),
        enabled,
        supports_requirement_codiscussion,
        supports_consultation,
        digest,
    };
    LAZY_BLUE5_DOC_RELOAD_TOTAL.fetch_add(1, Ordering::Relaxed);
    *guard = Some((path, modified, snapshot.clone()));
    snapshot
}

fn load_app_config_lazy(path: &PathBuf) -> Option<Arc<AppConfig>> {
    LAZY_APP_CONFIG_LOOKUP_TOTAL.fetch_add(1, Ordering::Relaxed);
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH);

    let cache = APP_CONFIG_CACHE.get_or_init(|| StdMutex::new(None));
    let mut guard = cache.lock().ok()?;
    if let Some((cached_path, cached_modified, cached_config)) = guard.as_ref() {
        if *cached_path == *path && *cached_modified == modified {
            LAZY_APP_CONFIG_HIT_TOTAL.fetch_add(1, Ordering::Relaxed);
            return Some(cached_config.clone());
        }
    }

    let loaded = Arc::new(AppConfig::load(path).ok()?);
    LAZY_APP_CONFIG_RELOAD_TOTAL.fetch_add(1, Ordering::Relaxed);
    *guard = Some((path.clone(), modified, loaded.clone()));
    Some(loaded)
}

fn load_latest_requirement_contract_lazy(
    ledger: &ArtifactLedger,
) -> Option<RequirementContractArtifact> {
    LAZY_CLARIFICATION_LOOKUP_TOTAL.fetch_add(1, Ordering::Relaxed);
    let latest = ledger.latest_path("spec", "latest-clarification.json");
    let modified = fs::metadata(&latest)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH);

    let cache = CLARIFICATION_ARTIFACT_CACHE.get_or_init(|| StdMutex::new(None));
    let mut guard = cache.lock().ok()?;
    if let Some((cached_path, cached_modified, artifact)) = guard.as_ref() {
        if *cached_path == latest && *cached_modified == modified {
            LAZY_CLARIFICATION_HIT_TOTAL.fetch_add(1, Ordering::Relaxed);
            return Some(artifact.clone());
        }
    }

    let raw = fs::read_to_string(&latest).ok()?;
    let artifact = serde_json::from_str::<RequirementContractArtifact>(&raw).ok()?;
    LAZY_CLARIFICATION_RELOAD_TOTAL.fetch_add(1, Ordering::Relaxed);
    *guard = Some((latest, modified, artifact.clone()));
    Some(artifact)
}

fn resolve_primary_secondary_policy(
    phase_agent_names: &[String],
    params: &Value,
    phase_options: Option<&PhaseOptions>,
) -> Result<PrimarySecondaryPolicy> {
    if phase_agent_names.is_empty() {
        return Err(anyhow::anyhow!(
            "{}",
            crate::i18n::t("error.primary_secondary_policy_requires_agent")
        ));
    }

    let failover_policy = params
        .get("primary_failover_policy")
        .and_then(|v| v.as_str())
        .map(|value| value.trim().to_ascii_lowercase())
        .or_else(|| {
            extra_string(phase_options, "primary_failover_policy")
                .map(|value| value.trim().to_ascii_lowercase())
        })
        .unwrap_or_else(|| "first_secondary".to_string());
    if !matches!(
        failover_policy.as_str(),
        "first_secondary" | "score_based_secondary" | "abort"
    ) {
        return Err(anyhow::anyhow!(
            "{}",
            crate::i18n::t("error.invalid_failover_policy")
        ));
    }

    let secondary_max_count = params
        .get("secondary_max_count")
        .and_then(|v| v.as_u64())
        .or_else(|| extra_u64(phase_options, "secondary_max_count"))
        .map(|value| value.max(1) as usize)
        .unwrap_or(2);

    let candidate_set = phase_agent_names
        .iter()
        .cloned()
        .collect::<HashSet<String>>();
    let primary_agent = params
        .get("primary_agent")
        .and_then(|v| v.as_str())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| phase_agent_names[0].clone());

    if !candidate_set.contains(&primary_agent) {
        return Err(anyhow::anyhow!(
            "{}",
            crate::i18n::t("error.primary_agent_not_found")
        ));
    }

    let requested_secondary = params
        .get("secondary_agents")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut secondary_agents = Vec::new();
    let mut seen = HashSet::new();
    for item in requested_secondary {
        let Some(agent) = item.as_str() else {
            continue;
        };
        let trimmed = agent.trim();
        if trimmed.is_empty() || trimmed == primary_agent {
            continue;
        }
        if candidate_set.contains(trimmed) && seen.insert(trimmed.to_string()) {
            secondary_agents.push(trimmed.to_string());
        }
    }
    if secondary_agents.is_empty() {
        for candidate in phase_agent_names {
            if candidate != &primary_agent && seen.insert(candidate.clone()) {
                secondary_agents.push(candidate.clone());
            }
        }
    }
    secondary_agents.truncate(secondary_max_count);

    if phase_agent_names.len() > 1 && secondary_agents.is_empty() {
        return Err(anyhow::anyhow!(
            "{}",
            crate::i18n::t("error.secondary_agents_empty")
        ));
    }

    Ok(PrimarySecondaryPolicy {
        primary_agent,
        secondary_agents,
        policy_version: "blue5.v1".to_string(),
        failover_policy,
        secondary_max_count,
    })
}

fn evaluate_blue5_for_clarify(
    doc: &Blue5DocSnapshot,
    contract: &RequirementContractArtifact,
    missing_fields: &[String],
    params: &Value,
) -> Blue5AutoDecision {
    let clarification_rounds = params
        .get("clarification_rounds")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let force_multi_ai = params
        .get("force_multi_ai_clarify")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let failure_threshold = params
        .get("consultation_failure_threshold")
        .and_then(|v| v.as_u64())
        .unwrap_or(2);

    let mut reasons = Vec::new();
    let should_multi_ai = doc.enabled
        && doc.supports_requirement_codiscussion
        && (force_multi_ai || missing_fields.len() >= 2 || contract.ambiguity_score >= 3);
    if should_multi_ai {
        reasons.push(crate::i18n::t("blue5.reason.requirement_clarification"));
    }

    let should_consultation = doc.enabled
        && doc.supports_consultation
        && ((clarification_rounds >= failure_threshold && !contract.user_confirmed)
            || params
                .get("consultation_required")
                .and_then(|v| v.as_bool())
                .unwrap_or(false));
    if should_consultation {
        reasons.push(crate::i18n::t("blue5.reason.clarification_rounds_exceeded"));
    }

    Blue5AutoDecision {
        enabled: doc.enabled,
        lazy_loaded: doc.lazy_loaded,
        should_multi_ai_clarify: should_multi_ai,
        should_consultation,
        reasons,
        primary_agent: None,
        secondary_agents: Vec::new(),
    }
}

fn evaluate_blue5_for_execute(
    doc: &Blue5DocSnapshot,
    plan: &crate::reinforcement::TaskPlanArtifact,
    phase_agent_names: &[String],
    params: &Value,
) -> Blue5AutoDecision {
    let failure_count = params
        .get("failure_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let failure_threshold = params
        .get("consultation_failure_threshold")
        .and_then(|v| v.as_u64())
        .unwrap_or(2);
    let clarification_quality_score = params
        .get("clarification_quality_score")
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0);

    let primary_agent = phase_agent_names.first().cloned();
    let secondary_agents = if phase_agent_names.len() > 1 {
        phase_agent_names[1..].to_vec()
    } else {
        Vec::new()
    };

    let mut reasons = Vec::new();
    let should_multi_ai = doc.enabled
        && phase_agent_names.len() >= 2
        && (plan.characteristics.complexity >= 4 || plan.characteristics.involves_multiple_modules);
    if should_multi_ai {
        reasons.push(crate::i18n::t("blue5.reason.task_complexity"));
    }

    let explicit_consultation_required = params
        .get("consultation_required")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let should_consultation = explicit_consultation_required
        || (doc.enabled
            && doc.supports_consultation
            && (failure_count >= failure_threshold
                || clarification_quality_score < 0.6
                || (plan.characteristics.complexity >= 4
                    && plan.routing.predicted_success_rate < 0.70)));
    if should_consultation {
        reasons.push(crate::i18n::t("blue5.reason.auto_consultation_gate"));
    }

    Blue5AutoDecision {
        enabled: doc.enabled,
        lazy_loaded: doc.lazy_loaded,
        should_multi_ai_clarify: should_multi_ai,
        should_consultation,
        reasons,
        primary_agent,
        secondary_agents,
    }
}

async fn run_consultation_workflow(
    server: &AcpServer,
    registry: &AgentRegistry,
    task: &str,
    source: &str,
    trigger_reason: &str,
    primary_secondary_policy: &PrimarySecondaryPolicy,
    consultation_confidence_threshold: f64,
) -> Result<(ConsultationArtifact, bool)> {
    let lead_name = primary_secondary_policy.primary_agent.clone();
    let specialist_names = primary_secondary_policy
        .secondary_agents
        .iter()
        .take(2)
        .cloned()
        .collect::<Vec<_>>();
    let reviewer_name = specialist_names
        .first()
        .cloned()
        .unwrap_or_else(|| lead_name.clone());

    let lead_agent = registry.get(&lead_name).ok_or_else(|| {
        anyhow::anyhow!(
            "{}",
            crate::i18n::tf("error.consult_lead_not_found", &[("name", &lead_name)])
        )
    })?;
    let reviewer_agent = registry.get(&reviewer_name).ok_or_else(|| {
        anyhow::anyhow!(
            "{}",
            crate::i18n::tf(
                "error.consult_reviewer_not_found",
                &[("name", &reviewer_name)]
            )
        )
    })?;

    let lead_prompt = crate::i18n::tf(
        "consultation.lead_prompt",
        &[("task", task), ("trigger", trigger_reason)],
    );
    let lead_output = server
        .run_agent_collecting(
            lead_name.clone(),
            lead_agent,
            vec![Message {
                role: "user".to_string(),
                content: lead_prompt,
            }],
            None,
            None,
            Some(Duration::from_secs(120)),
        )
        .await?;

    let mut candidate_plans = vec![lead_output.chars().take(600).collect::<String>()];
    let mut participants = vec![lead_name.clone()];

    for specialist_name in &specialist_names {
        if let Some(agent) = registry.get(specialist_name) {
            let specialist_prompt = crate::i18n::tf(
                "consultation.reviewer_alternative_prompt",
                &[("task", task), ("role", specialist_name)],
            );
            let specialist_output = server
                .run_agent_collecting(
                    specialist_name.clone(),
                    agent,
                    vec![Message {
                        role: "user".to_string(),
                        content: specialist_prompt,
                    }],
                    None,
                    None,
                    Some(Duration::from_secs(120)),
                )
                .await?;
            participants.push(specialist_name.clone());
            candidate_plans.push(specialist_output.chars().take(600).collect::<String>());
        }
    }

    if !participants.iter().any(|name| name == &reviewer_name) {
        participants.push(reviewer_name.clone());
    }

    let reviewer_prompt = crate::i18n::tf(
        "consultation.reviewer_consensus_prompt",
        &[
            ("task", task),
            (
                "plans",
                &candidate_plans
                    .iter()
                    .enumerate()
                    .map(|(idx, plan)| {
                        crate::i18n::tf(
                            "ui.plan_number",
                            &[("number", &(idx + 1).to_string()), ("plan", plan)],
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            ),
        ],
    );
    let reviewer_output = server
        .run_agent_collecting(
            reviewer_name,
            reviewer_agent,
            vec![Message {
                role: "user".to_string(),
                content: reviewer_prompt,
            }],
            None,
            None,
            Some(Duration::from_secs(120)),
        )
        .await?;

    let consensus_plan = reviewer_output.chars().take(800).collect::<String>();
    let reviewer_lower = reviewer_output.to_ascii_lowercase();
    let looks_uncertain = reviewer_lower.contains("无法")
        || reviewer_lower.contains("不确定")
        || reviewer_lower.contains("insufficient")
        || reviewer_lower.contains("uncertain");
    let decision_confidence = if looks_uncertain { 0.45 } else { 0.78 };
    let consensus_achieved = !consensus_plan.trim().is_empty()
        && decision_confidence >= consultation_confidence_threshold;

    let artifact = ConsultationArtifact {
        generated_at: now_ts(),
        task: task.to_string(),
        source: source.to_string(),
        trigger_reason: trigger_reason.to_string(),
        participants,
        candidate_plans,
        consensus_plan: consensus_plan.clone(),
        risk_matrix: json!({
            "top_risks": reviewer_output.chars().take(500).collect::<String>(),
        }),
        decision_confidence,
        handoff_primary_agent: primary_secondary_policy.primary_agent.clone(),
    };

    Ok((artifact, consensus_achieved))
}

/// Chat mode enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatMode {
    /// Ask mode - regular chat
    Ask,
    /// Edit mode - code editing
    Edit,
    /// Agent mode - agent execution
    Agent,
    /// Full auto mode - autonomous operation
    FullAuto,
}

impl ChatMode {
    /// Parse chat mode from string
    fn parse(raw: Option<&str>) -> Option<Self> {
        let value = raw?.trim().to_ascii_lowercase();
        match value.as_str() {
            "ask" => Some(Self::Ask),
            "edit" => Some(Self::Edit),
            "agent" => Some(Self::Agent),
            "full_auto" | "full-auto" | "auto" => Some(Self::FullAuto),
            _ => None,
        }
    }

    /// Convert chat mode to string
    fn as_str(&self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Edit => "edit",
            Self::Agent => "agent",
            Self::FullAuto => "full_auto",
        }
    }
}

/// Autopilot complexity level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutopilotComplexity {
    /// Simple autopilot mode
    Simple,
    /// Complex autopilot mode
    Complex,
}

impl AutopilotComplexity {
    /// Parse complexity from string
    fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "simple" => Some(Self::Simple),
            "complex" => Some(Self::Complex),
            _ => None,
        }
    }
}

/// Approval strategy enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalStrategy {
    /// Default approval process
    DefaultApprovals,
    /// Bypass approval process
    ByPassApproval,
    /// Simple autopilot approval
    AutoPilotSimple,
    /// Complex autopilot approval (requires dual review)
    AutoPilotComplex,
}

impl ApprovalStrategy {
    /// Convert approval strategy to string
    fn as_str(&self) -> &'static str {
        match self {
            Self::DefaultApprovals => "default_approvals",
            Self::ByPassApproval => "by_pass_approval",
            Self::AutoPilotSimple => "autopilot_simple",
            Self::AutoPilotComplex => "autopilot_complex",
        }
    }

    /// Check if dual review is needed
    fn needs_dual_review(&self) -> bool {
        matches!(self, Self::AutoPilotComplex)
    }
}

/// Convert chat mode and complexity to approval strategy
fn mode_to_approval_strategy(
    mode: Option<ChatMode>,
    complexity: Option<AutopilotComplexity>,
) -> ApprovalStrategy {
    match mode {
        Some(ChatMode::Ask) => ApprovalStrategy::DefaultApprovals,
        Some(ChatMode::Edit) | Some(ChatMode::Agent) => ApprovalStrategy::ByPassApproval,
        Some(ChatMode::FullAuto) => match complexity {
            Some(AutopilotComplexity::Simple) => ApprovalStrategy::AutoPilotSimple,
            Some(AutopilotComplexity::Complex) => ApprovalStrategy::AutoPilotComplex,
            None => ApprovalStrategy::AutoPilotSimple,
        },
        None => ApprovalStrategy::DefaultApprovals,
    }
}

/// Chat request parameters
#[derive(Debug, Deserialize)]
struct ChatParams {
    /// Chat messages
    messages: Vec<Message>,
    /// Phase name
    phase: Option<String>,
    /// Chat mode
    mode: Option<String>,
    /// Conversation identifier for checkpoint grouping across turns
    conversation_id: Option<String>,
    /// Additional context
    #[allow(dead_code)]
    context: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConversationCheckpoint {
    checkpoint_id: String,
    conversation_id: String,
    branch_id: String,
    parent_checkpoint_id: Option<String>,
    created_at: i64,
    note: Option<String>,
    messages: Vec<Message>,
}

#[derive(Debug, Clone, Default)]
struct ConversationState {
    checkpoints: Vec<ConversationCheckpoint>,
    branch_heads: HashMap<String, String>,
    last_touched_at: i64,
}

#[derive(Debug, Clone, Serialize, Default)]
struct ConversationPruneResult {
    removed: usize,
    repaired_heads: usize,
    dropped_heads: usize,
}

#[derive(Debug, Default, Clone, Serialize)]
struct MetricsSnapshot {
    chat_requests_total: u64,
    cache_lookup_total: u64,
    cache_hit_total: u64,
    cache_store_total: u64,
    vector_search_total: u64,
    vector_hit_total: u64,
    vector_store_total: u64,
    summary_read_total: u64,
    summary_hit_total: u64,
    summary_store_total: u64,
    agent_failures_total: u64,
    review_gate_total: u64,
    review_gate_approved_total: u64,
    review_gate_rejected_total: u64,
    review_gate_timeout_total: u64,
    review_gate_degraded_total: u64,
    review_gate_invalid_response_total: u64,
    lazy_blue5_doc_lookup_total: u64,
    lazy_blue5_doc_hit_total: u64,
    lazy_blue5_doc_reload_total: u64,
    lazy_app_config_lookup_total: u64,
    lazy_app_config_hit_total: u64,
    lazy_app_config_reload_total: u64,
    lazy_clarification_lookup_total: u64,
    lazy_clarification_hit_total: u64,
    lazy_clarification_reload_total: u64,
    agent_timeout_failures_total: u64,
    agent_panic_failures_total: u64,
    agent_other_failures_total: u64,
    chat_latency_count: u64,
    chat_latency_sum_seconds: f64,
    chat_latency_bucket_counts: [u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
    agent_latency_count: u64,
    agent_latency_sum_seconds: f64,
    agent_latency_bucket_counts: [u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
    review_latency_count: u64,
    review_latency_sum_seconds: f64,
    review_latency_bucket_counts: [u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
}

#[derive(Debug, Clone, Default)]
struct RuntimeGaugeSnapshot {
    memory_cache_entries: u64,
    sqlite_cache_entries: u64,
    vector_memory_entries: u64,
    vector_summary_entries: u64,
    circuit_open_agents: u64,
    circuit_half_open_agents: u64,
    circuit_tracked_agents: u64,
    rate_limiter_tracked_phases: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
struct MaintenanceSnapshot {
    running: bool,
    cycles_total: u64,
    last_started_at: Option<i64>,
    last_completed_at: Option<i64>,
    last_memory_expired_removed: u64,
    last_sqlite_expired_removed: u64,
    last_cache_vacuumed: bool,
    last_vector_vacuumed: bool,
    last_error: Option<String>,
}

#[derive(Default)]
struct MaintenanceTracker {
    inner: StdMutex<MaintenanceSnapshot>,
}

impl MaintenanceTracker {
    fn snapshot(&self) -> MaintenanceSnapshot {
        self.inner
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    fn note_started(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.running = true;
            guard.last_started_at = Some(now_ts());
            guard.cycles_total = guard.cycles_total.saturating_add(1);
            guard.last_error = None;
        }
    }

    fn note_completed(
        &self,
        memory_removed: usize,
        sqlite_removed: usize,
        cache_vacuumed: bool,
        vector_vacuumed: bool,
    ) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.running = false;
            guard.last_completed_at = Some(now_ts());
            guard.last_memory_expired_removed = memory_removed as u64;
            guard.last_sqlite_expired_removed = sqlite_removed as u64;
            guard.last_cache_vacuumed = cache_vacuumed;
            guard.last_vector_vacuumed = vector_vacuumed;
            guard.last_error = None;
        }
    }

    fn note_failed(&self, err: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.running = false;
            guard.last_completed_at = Some(now_ts());
            guard.last_error = Some(err.to_string());
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
struct LifecycleSnapshot {
    shutting_down: bool,
    shutdown_started_at: Option<i64>,
    shutdown_reason: Option<String>,
}

#[derive(Default)]
struct LifecycleState {
    inner: StdMutex<LifecycleSnapshot>,
}

impl LifecycleState {
    fn snapshot(&self) -> LifecycleSnapshot {
        self.inner
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    fn is_shutting_down(&self) -> bool {
        self.inner
            .lock()
            .map(|guard| guard.shutting_down)
            .unwrap_or(false)
    }

    fn start_shutdown(&self, reason: &str) -> bool {
        if let Ok(mut guard) = self.inner.lock() {
            if guard.shutting_down {
                return false;
            }
            guard.shutting_down = true;
            guard.shutdown_started_at = Some(now_ts());
            guard.shutdown_reason = Some(reason.to_string());
            return true;
        }
        false
    }
}

#[derive(Default)]
struct RuntimeMetrics {
    inner: StdMutex<MetricsSnapshot>,
}

impl RuntimeMetrics {
    fn snapshot(&self) -> MetricsSnapshot {
        let mut snapshot = self.inner.lock().map(|g| g.clone()).unwrap_or_default();
        let lazy = lazy_load_cache_snapshot();
        snapshot.lazy_blue5_doc_lookup_total = lazy.blue5_doc_lookup_total;
        snapshot.lazy_blue5_doc_hit_total = lazy.blue5_doc_hit_total;
        snapshot.lazy_blue5_doc_reload_total = lazy.blue5_doc_reload_total;
        snapshot.lazy_app_config_lookup_total = lazy.app_config_lookup_total;
        snapshot.lazy_app_config_hit_total = lazy.app_config_hit_total;
        snapshot.lazy_app_config_reload_total = lazy.app_config_reload_total;
        snapshot.lazy_clarification_lookup_total = lazy.clarification_lookup_total;
        snapshot.lazy_clarification_hit_total = lazy.clarification_hit_total;
        snapshot.lazy_clarification_reload_total = lazy.clarification_reload_total;
        snapshot
    }

    fn reset(&self) {
        if let Ok(mut metrics) = self.inner.lock() {
            *metrics = MetricsSnapshot::default();
        }
        reset_lazy_load_cache_snapshot();
    }

    fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut MetricsSnapshot),
    {
        if let Ok(mut metrics) = self.inner.lock() {
            f(&mut metrics);
        }
    }

    fn inc_chat_requests(&self) {
        self.update(|m| m.chat_requests_total += 1);
    }

    fn inc_cache_lookup(&self) {
        self.update(|m| m.cache_lookup_total += 1);
    }

    fn inc_cache_hit(&self) {
        self.update(|m| m.cache_hit_total += 1);
    }

    fn inc_cache_store(&self) {
        self.update(|m| m.cache_store_total += 1);
    }

    fn inc_vector_search(&self) {
        self.update(|m| m.vector_search_total += 1);
    }

    fn inc_vector_hit(&self) {
        self.update(|m| m.vector_hit_total += 1);
    }

    fn inc_vector_store(&self) {
        self.update(|m| m.vector_store_total += 1);
    }

    fn inc_summary_read(&self) {
        self.update(|m| m.summary_read_total += 1);
    }

    fn inc_summary_hit(&self) {
        self.update(|m| m.summary_hit_total += 1);
    }

    fn inc_summary_store(&self) {
        self.update(|m| m.summary_store_total += 1);
    }

    fn inc_agent_failures(&self) {
        self.update(|m| m.agent_failures_total += 1);
    }

    fn inc_agent_timeout_failures(&self) {
        self.update(|m| m.agent_timeout_failures_total += 1);
    }

    fn inc_agent_panic_failures(&self) {
        self.update(|m| m.agent_panic_failures_total += 1);
    }

    fn inc_agent_other_failures(&self) {
        self.update(|m| m.agent_other_failures_total += 1);
    }

    fn inc_review_gate(&self) {
        self.update(|m| m.review_gate_total += 1);
    }

    fn inc_review_gate_approved(&self) {
        self.update(|m| m.review_gate_approved_total += 1);
    }

    fn inc_review_gate_rejected(&self) {
        self.update(|m| m.review_gate_rejected_total += 1);
    }

    fn inc_review_gate_timeout(&self) {
        self.update(|m| m.review_gate_timeout_total += 1);
    }

    fn inc_review_gate_degraded(&self) {
        self.update(|m| m.review_gate_degraded_total += 1);
    }

    fn inc_review_gate_invalid_response(&self) {
        self.update(|m| m.review_gate_invalid_response_total += 1);
    }

    fn observe_chat_latency(&self, duration: Duration) {
        self.update(|m| {
            observe_latency_histogram(
                duration,
                &mut m.chat_latency_count,
                &mut m.chat_latency_sum_seconds,
                &mut m.chat_latency_bucket_counts,
            )
        });
    }

    fn observe_agent_latency(&self, duration: Duration) {
        self.update(|m| {
            observe_latency_histogram(
                duration,
                &mut m.agent_latency_count,
                &mut m.agent_latency_sum_seconds,
                &mut m.agent_latency_bucket_counts,
            )
        });
    }

    fn observe_review_latency(&self, duration: Duration) {
        self.update(|m| {
            observe_latency_histogram(
                duration,
                &mut m.review_latency_count,
                &mut m.review_latency_sum_seconds,
                &mut m.review_latency_bucket_counts,
            )
        });
    }
}

struct PreparedChatInput {
    messages: Vec<Message>,
    latest_user_query: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CircuitBreakerStage {
    Closed,
    Open,
    HalfOpen,
}

impl CircuitBreakerStage {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::HalfOpen => "half_open",
        }
    }
}

#[derive(Debug, Clone)]
struct CircuitBreakerState {
    consecutive_failures: u32,
    stage: CircuitBreakerStage,
    open_until: Option<i64>,
    probe_in_flight: bool,
}

impl Default for CircuitBreakerState {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            stage: CircuitBreakerStage::Closed,
            open_until: None,
            probe_in_flight: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct CircuitBreakerSnapshot {
    consecutive_failures: u32,
    state: String,
    open_until: Option<i64>,
    probe_in_flight: bool,
}

enum CircuitBreakerAdmission {
    Closed,
    HalfOpenProbe,
    Rejected {
        state: &'static str,
        retry_after_seconds: Option<i64>,
    },
}

#[derive(Default)]
struct CircuitBreakerRegistry {
    inner: StdMutex<HashMap<String, CircuitBreakerState>>,
}

impl CircuitBreakerRegistry {
    fn allow_request(&self, agent_name: &str) -> CircuitBreakerAdmission {
        let now = now_ts();
        if let Ok(mut guard) = self.inner.lock() {
            let state = guard.entry(agent_name.to_string()).or_default();
            match state.stage {
                CircuitBreakerStage::Closed => CircuitBreakerAdmission::Closed,
                CircuitBreakerStage::Open => {
                    if let Some(open_until) = state.open_until {
                        if open_until > now {
                            return CircuitBreakerAdmission::Rejected {
                                state: "open",
                                retry_after_seconds: Some((open_until - now).max(0)),
                            };
                        }
                    }

                    state.stage = CircuitBreakerStage::HalfOpen;
                    state.open_until = None;
                    state.probe_in_flight = true;
                    CircuitBreakerAdmission::HalfOpenProbe
                }
                CircuitBreakerStage::HalfOpen => {
                    if state.probe_in_flight {
                        CircuitBreakerAdmission::Rejected {
                            state: "half_open",
                            retry_after_seconds: None,
                        }
                    } else {
                        state.probe_in_flight = true;
                        CircuitBreakerAdmission::HalfOpenProbe
                    }
                }
            }
        } else {
            CircuitBreakerAdmission::Closed
        }
    }

    fn record_success(&self, agent_name: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            let state = guard.entry(agent_name.to_string()).or_default();
            state.consecutive_failures = 0;
            state.stage = CircuitBreakerStage::Closed;
            state.open_until = None;
            state.probe_in_flight = false;
        }
    }

    fn record_failure_with_config(
        &self,
        agent_name: &str,
        failure_threshold: u32,
        open_seconds: i64,
    ) {
        let now = now_ts();
        if let Ok(mut guard) = self.inner.lock() {
            let state = guard.entry(agent_name.to_string()).or_default();
            let effective_threshold = failure_threshold.max(1);
            if state.stage == CircuitBreakerStage::HalfOpen {
                state.consecutive_failures = effective_threshold;
                state.stage = CircuitBreakerStage::Open;
                state.probe_in_flight = false;
                state.open_until = Some(now + open_seconds.max(1));
                return;
            }

            state.consecutive_failures = state.consecutive_failures.saturating_add(1);
            if state.consecutive_failures >= effective_threshold {
                state.stage = CircuitBreakerStage::Open;
                state.probe_in_flight = false;
                state.open_until = Some(now + open_seconds.max(1));
            }
        }
    }

    fn snapshot(&self) -> HashMap<String, CircuitBreakerSnapshot> {
        let now = now_ts();
        self.inner
            .lock()
            .map(|guard| {
                guard
                    .iter()
                    .map(|(name, state)| {
                        let state_name = if state.stage == CircuitBreakerStage::Open
                            && state.open_until.map(|until| until <= now).unwrap_or(false)
                        {
                            "half_open_ready".to_string()
                        } else {
                            state.stage.as_str().to_string()
                        };
                        (
                            name.clone(),
                            CircuitBreakerSnapshot {
                                consecutive_failures: state.consecutive_failures,
                                state: state_name,
                                open_until: state.open_until,
                                probe_in_flight: state.probe_in_flight,
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn open_count(&self) -> usize {
        let now = now_ts();
        if let Ok(guard) = self.inner.lock() {
            return guard
                .values()
                .filter(|state| {
                    state.stage == CircuitBreakerStage::Open
                        && state.open_until.map(|until| until > now).unwrap_or(false)
                })
                .count();
        }
        0
    }

    fn half_open_count(&self) -> usize {
        if let Ok(guard) = self.inner.lock() {
            return guard
                .values()
                .filter(|state| state.stage == CircuitBreakerStage::HalfOpen)
                .count();
        }
        0
    }

    fn tracked_agents(&self) -> usize {
        self.inner.lock().map(|guard| guard.len()).unwrap_or(0)
    }
}

#[derive(Default)]
struct PhaseRateLimiter {
    inner: StdMutex<HashMap<String, TokenBucketState>>,
}

#[derive(Clone)]
struct TokenBucketState {
    tokens: f64,
    capacity: f64,
    refill_per_second: f64,
    last_refill_ms: i64,
}

impl TokenBucketState {
    fn new(capacity: f64, refill_per_second: f64, now_ms: i64) -> Self {
        Self {
            tokens: capacity,
            capacity,
            refill_per_second,
            last_refill_ms: now_ms,
        }
    }

    fn refill(&mut self, now_ms: i64) {
        let elapsed_ms = (now_ms - self.last_refill_ms).max(0) as f64;
        if elapsed_ms > 0.0 {
            let refill = elapsed_ms / 1000.0 * self.refill_per_second;
            self.tokens = (self.tokens + refill).min(self.capacity);
            self.last_refill_ms = now_ms;
        }
    }
}

impl PhaseRateLimiter {
    fn allow(&self, phase_name: &str, rpm_limit: u64, burst_capacity: Option<u64>) -> bool {
        if rpm_limit == 0 {
            return false;
        }

        let now = now_ms();
        let refill_per_second = rpm_limit as f64 / 60.0;
        let capacity = burst_capacity.unwrap_or(rpm_limit).max(1) as f64;

        if let Ok(mut guard) = self.inner.lock() {
            let state = guard
                .entry(phase_name.to_string())
                .or_insert_with(|| TokenBucketState::new(capacity, refill_per_second, now));

            if (state.capacity - capacity).abs() > f64::EPSILON
                || (state.refill_per_second - refill_per_second).abs() > f64::EPSILON
            {
                *state = TokenBucketState::new(capacity, refill_per_second, now);
            }

            state.refill(now);
            if state.tokens < 1.0 {
                return false;
            }
            state.tokens -= 1.0;
            return true;
        }
        true
    }

    fn tracked_phases(&self) -> usize {
        self.inner.lock().map(|guard| guard.len()).unwrap_or(0)
    }

    fn snapshot(&self) -> HashMap<String, (f64, f64)> {
        self.inner
            .lock()
            .map(|guard| {
                guard
                    .iter()
                    .map(|(phase, state)| (phase.clone(), (state.tokens, state.capacity)))
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Default)]
struct InflightLimiter {
    inner: StdMutex<InflightState>,
}

#[derive(Default)]
struct InflightState {
    global: usize,
    phase: HashMap<String, usize>,
}

struct InflightGuard {
    limiter: Arc<InflightLimiter>,
    phase_name: String,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.limiter.leave(&self.phase_name);
    }
}

impl InflightLimiter {
    fn try_enter(
        self: &Arc<Self>,
        phase_name: &str,
        phase_limit: Option<u64>,
        global_limit: Option<u64>,
    ) -> Option<InflightGuard> {
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(limit) = global_limit {
                if guard.global as u64 >= limit.max(1) {
                    return None;
                }
            }

            let phase_count = guard.phase.get(phase_name).copied().unwrap_or(0);
            if let Some(limit) = phase_limit {
                if phase_count as u64 >= limit.max(1) {
                    return None;
                }
            }

            guard.global += 1;
            *guard.phase.entry(phase_name.to_string()).or_insert(0) += 1;
            return Some(InflightGuard {
                limiter: Arc::clone(self),
                phase_name: phase_name.to_string(),
            });
        }
        None
    }

    fn leave(&self, phase_name: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.global = guard.global.saturating_sub(1);
            if let Some(value) = guard.phase.get_mut(phase_name) {
                *value = value.saturating_sub(1);
                if *value == 0 {
                    guard.phase.remove(phase_name);
                }
            }
        }
    }

    fn snapshot(&self) -> (usize, HashMap<String, usize>) {
        self.inner
            .lock()
            .map(|guard| (guard.global, guard.phase.clone()))
            .unwrap_or_default()
    }

    fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.global = 0;
            guard.phase.clear();
        }
    }
}

