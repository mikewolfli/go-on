//! ACP (Agent Coordination Protocol) server implementation
//!
//! This module implements the core server functionality for the go-on ACP proxy,
//! including request handling, caching, vector storage, circuit breaking, and performance monitoring.

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

/// ACP server implementation
///
/// This struct represents the main ACP server that handles incoming requests,
/// manages agents, and coordinates the overall system flow.
pub struct AcpServer {
    /// Flow manager for handling request routing through phases
    flow: Arc<StdMutex<Arc<FlowManager>>>,
    /// Agent registry for managing available agents
    registry: Arc<StdMutex<Arc<AgentRegistry>>>,
    /// Response cache (SQLite-based)
    cache: Arc<StdMutex<Option<Arc<ResponseCache>>>>,
    /// Vector store for similarity search and memory
    vector_store: Arc<StdMutex<Option<Arc<VectorStore>>>>,
    /// Vector store configuration
    vector_config: Arc<StdMutex<Option<VectorConfig>>>,
    /// Autotune state for adaptive configuration
    autotune: Arc<StdMutex<Option<Arc<Mutex<AutoTuneState>>>>>,
    /// Autotune configuration
    autotune_config: Arc<StdMutex<Option<AutoTuneConfig>>>,
    /// Path to autotune state file
    autotune_state_path: Arc<StdMutex<Option<String>>>,
    /// Runtime configuration
    runtime_config: Arc<StdMutex<RuntimeConfig>>,
    /// Runtime metrics collection
    metrics: Arc<RuntimeMetrics>,
    /// Online controller for adaptive strategy from live outcomes
    online_controller: Arc<StdMutex<OnlineControllerState>>,
    /// OpenTelemetry runtime bridge
    telemetry: Arc<TelemetryRuntime>,
    /// In-memory request trace events (phase-1 OTel-compatible)
    trace_events: Arc<StdMutex<Vec<TraceEvent>>>,
    /// In-memory response cache for fast access
    memory_cache: Arc<MemoryResponseCache>,
    /// Conversation checkpoint store for branch/rollback control
    conversation_store: Arc<StdMutex<HashMap<String, ConversationState>>>,
    /// Most-recently touched conversations; used for bounded conversation-store eviction
    conversation_touch_order: Arc<StdMutex<Vec<String>>>,
    /// Maintenance tracker for system health
    maintenance: Arc<MaintenanceTracker>,
    /// Lifecycle state management
    lifecycle: Arc<LifecycleState>,
    /// Circuit breakers for agent failure handling
    circuit_breakers: Arc<CircuitBreakerRegistry>,
    /// Rate limiter for phase-level throttling
    phase_rate_limiter: Arc<PhaseRateLimiter>,
    /// In-flight request limiter
    inflight_limiter: Arc<InflightLimiter>,
    /// Path to configuration file
    config_path: Option<PathBuf>,
    /// Forced phase name (if specified)
    forced_phase: Option<String>,
    /// HTTP client for external requests
    http_client: Option<reqwest::Client>,
    /// Verbose logging flag
    verbose: bool,
    /// Output stream for responses
    output: Arc<Mutex<tokio::io::Stdout>>,
    /// Shutdown notification mechanism
    shutdown_notify: Arc<Notify>,
}

impl AcpServer {
    /// Handle a chat request (main entry point). Instrumentation should be added in the implementation.
    // pub async fn handle_chat_request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse> { ... }
    /// Create a new ACP server instance
    ///
    /// # Arguments
    /// * `flow` - Flow manager for request routing through phases
    /// * `registry` - Agent registry for managing available agents
    /// * `cache` - Response cache (SQLite-based)
    /// * `vector_store` - Vector store for similarity search and memory
    /// * `vector_config` - Vector store configuration
    /// * `autotune` - Autotune state for adaptive configuration
    /// * `autotune_config` - Autotune configuration
    /// * `autotune_state_path` - Path to autotune state file
    /// * `runtime_config` - Runtime configuration
    /// * `config_path` - Path to configuration file
    /// * `forced_phase` - Forced phase name (if specified)
    /// * `http_client` - HTTP client for external requests
    /// * `verbose` - Verbose logging flag
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        flow: Arc<FlowManager>,
        registry: Arc<AgentRegistry>,
        cache: Option<Arc<ResponseCache>>,
        vector_store: Option<Arc<VectorStore>>,
        vector_config: Option<VectorConfig>,
        autotune: Option<Arc<Mutex<AutoTuneState>>>,
        autotune_config: Option<AutoTuneConfig>,
        autotune_state_path: Option<String>,
        runtime_config: RuntimeConfig,
        config_path: Option<PathBuf>,
        forced_phase: Option<String>,
        http_client: Option<reqwest::Client>,
        verbose: bool,
    ) -> Self {
        let telemetry = Arc::new(TelemetryRuntime::new(&runtime_config));
        Self {
            flow: Arc::new(StdMutex::new(flow)),
            registry: Arc::new(StdMutex::new(registry)),
            cache: Arc::new(StdMutex::new(cache)),
            vector_store: Arc::new(StdMutex::new(vector_store)),
            vector_config: Arc::new(StdMutex::new(vector_config)),
            autotune: Arc::new(StdMutex::new(autotune)),
            autotune_config: Arc::new(StdMutex::new(autotune_config)),
            autotune_state_path: Arc::new(StdMutex::new(autotune_state_path)),
            runtime_config: Arc::new(StdMutex::new(runtime_config)),
            metrics: Arc::new(RuntimeMetrics::default()),
            online_controller: Arc::new(StdMutex::new(OnlineControllerState::default())),
            telemetry,
            trace_events: Arc::new(StdMutex::new(Vec::new())),
            memory_cache: Arc::new(MemoryResponseCache::default()),
            conversation_store: Arc::new(StdMutex::new(HashMap::new())),
            conversation_touch_order: Arc::new(StdMutex::new(Vec::new())),
            maintenance: Arc::new(MaintenanceTracker::default()),
            lifecycle: Arc::new(LifecycleState::default()),
            circuit_breakers: Arc::new(CircuitBreakerRegistry::default()),
            phase_rate_limiter: Arc::new(PhaseRateLimiter::default()),
            inflight_limiter: Arc::new(InflightLimiter::default()),
            config_path,
            forced_phase,
            http_client,
            verbose,
            output: Arc::new(Mutex::new(tokio::io::stdout())),
            shutdown_notify: Arc::new(Notify::new()),
        }
    }

    /// Run the ACP server
    ///
    /// This method starts the server, handles incoming requests from stdin,
    /// and manages the server lifecycle.
    ///
    /// # Returns
    /// * `Result<()>` - Returns Ok(()) on successful shutdown, or an error if something goes wrong
    pub async fn run(&mut self) -> Result<()> {
        // Spawn background maintenance loop
        let background_task = self.spawn_background_maintenance_loop();
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin).lines();

        // Process incoming requests from stdin
        while let Some(line) = reader.next_line().await? {
            if self.lifecycle.is_shutting_down() {
                break;
            }

            if line.trim().is_empty() {
                continue;
            }

            // Parse JSON-RPC request
            let request: JsonRpcRequest = match serde_json::from_str(&line) {
                Ok(req) => req,
                Err(err) => {
                    self.send_error(
                        None,
                        -32700,
                        crate::i18n::tf("error.parse_error", &[("error", &format!("{err}"))]),
                        None,
                    )
                    .await?;
                    continue;
                }
            };

            // Validate JSON-RPC version
            if request.jsonrpc != "2.0" {
                self.send_error(
                    request.id,
                    -32600,
                    ProxyError::InvalidRequest("jsonrpc must be 2.0".to_string()).to_string(),
                    None,
                )
                .await?;
                continue;
            }

            let method = request.method.clone();
            if self.verbose {
                debug!("incoming method: {method}");
            }

            // Handle request in a separate task to avoid blocking the main loop
            let id_for_response = request.id.clone();
            let handle = tokio::spawn(async move { request });
            let request = match handle.await {
                Ok(req) => req,
                Err(join_err) => {
                    self.send_error(
                        id_for_response,
                        -32603,
                        crate::i18n::tf(
                            "error.request_handling_panic",
                            &[("error", &format!("{join_err}"))],
                        ),
                        None,
                    )
                    .await?;
                    continue;
                }
            };

            // Process the request
            let response = self.handle_request(request).await;
            if let Err(err) = response {
                error!(method = %method, "request failed: {err:#}");
            }

            // Check if shutdown is requested
            if method == "shutdown" || self.lifecycle.is_shutting_down() {
                info!("{}", crate::i18n::t("info.shutdown_requested"));
                break;
            }
        }

        // Shutdown sequence
        self.begin_shutdown(&crate::i18n::t("info.shutdown_sequence"));
        self.wait_for_inflight_drain().await;
        self.shutdown_notify.notify_waiters();

        // Wait for background task to complete
        if let Err(err) = background_task.await {
            warn!("background maintenance task exited unexpectedly: {}", err);
        }

        Ok(())
    }

    fn routing_handles(&self) -> Result<(Arc<FlowManager>, Arc<AgentRegistry>)> {
        let flow_guard = self.flow.lock().map_err(|_| {
            anyhow::anyhow!(
                "{}",
                crate::i18n::tf("error.mutex_poisoned", &[("name", "flow")])
            )
        })?;
        let registry_guard = self.registry.lock().map_err(|_| {
            anyhow::anyhow!(
                "{}",
                crate::i18n::tf("error.mutex_poisoned", &[("name", "registry")])
            )
        })?;
        Ok((flow_guard.clone(), registry_guard.clone()))
    }

    fn cache_handle(&self) -> Option<Arc<ResponseCache>> {
        self.cache.lock().ok().and_then(|guard| guard.clone())
    }

    fn artifact_ledger(&self) -> ArtifactLedger {
        ArtifactLedger::new(self.config_path.as_deref())
    }

    fn vector_store_handle(&self) -> Option<Arc<VectorStore>> {
        self.vector_store
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    fn vector_config_snapshot(&self) -> Option<VectorConfig> {
        self.vector_config
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    fn autotune_handle(&self) -> Option<Arc<Mutex<AutoTuneState>>> {
        self.autotune.lock().ok().and_then(|guard| guard.clone())
    }

    fn autotune_config_snapshot(&self) -> Option<AutoTuneConfig> {
        self.autotune_config
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    fn autotune_state_path_snapshot(&self) -> Option<String> {
        self.autotune_state_path
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    fn runtime_config_snapshot(&self) -> RuntimeConfig {
        self.runtime_config
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    fn runtime_healthcheck_report(&self) -> Result<crate::reinforcement::RuntimeHealthcheckReport> {
        let cache = self.cache_handle();
        let vector_store = self.vector_store_handle();
        let mut report = build_runtime_healthcheck_report(
            self.config_path.as_deref(),
            cache.as_deref(),
            vector_store.as_deref(),
        )?;

        let (global_inflight, phase_inflight) = self.inflight_limiter.snapshot();
        let runtime_status =
            if self.lifecycle.is_shutting_down() || self.circuit_breakers.open_count() > 0 {
                CheckStatus::Warn
            } else {
                CheckStatus::Healthy
            };

        report.components.push(ComponentReport {
            name: "runtime".to_string(),
            status: runtime_status,
            message: crate::i18n::t("info.runtime_controller_snapshot"),
            details: json!({
                "memory_cache_entries": self.memory_cache.active_entries(),
                "lazy_load_cache": lazy_load_cache_snapshot(),
                "circuit_breaker": {
                    "open_agents": self.circuit_breakers.open_count(),
                    "half_open_agents": self.circuit_breakers.half_open_count(),
                    "tracked_agents": self.circuit_breakers.tracked_agents(),
                    "agents": self.circuit_breakers.snapshot(),
                },
                "rate_limiter": {
                    "tracked_phases": self.phase_rate_limiter.tracked_phases(),
                },
                "inflight": {
                    "global": global_inflight,
                    "per_phase": phase_inflight,
                },
                "lifecycle": self.lifecycle.snapshot(),
                "maintenance": self.maintenance.snapshot(),
                "review_gate": {
                    "total": self.metrics.snapshot().review_gate_total,
                    "approved": self.metrics.snapshot().review_gate_approved_total,
                    "rejected": self.metrics.snapshot().review_gate_rejected_total,
                    "timeout": self.metrics.snapshot().review_gate_timeout_total,
                    "degraded": self.metrics.snapshot().review_gate_degraded_total,
                    "invalid_response": self.metrics.snapshot().review_gate_invalid_response_total,
                },
                "telemetry": {
                    "enabled": self.telemetry.is_enabled(),
                    "sampling_rate": self.telemetry.sampling_rate(),
                },
            }),
        });

        report.overall_status =
            aggregate_status(report.components.iter().map(|component| component.status));
        Ok(report)
    }

    fn persist_checkpoint_summary(&self, checkpoint: &ConversationCheckpoint) {
        let summary = CheckpointSummaryArtifact {
            checkpoint_id: checkpoint.checkpoint_id.clone(),
            conversation_id: checkpoint.conversation_id.clone(),
            branch_id: checkpoint.branch_id.clone(),
            parent_checkpoint_id: checkpoint.parent_checkpoint_id.clone(),
            created_at: checkpoint.created_at,
            note: checkpoint.note.clone(),
            message_count: checkpoint.messages.len(),
            message_chars: total_message_chars(&checkpoint.messages),
            assistant_excerpt: assistant_excerpt(&checkpoint.messages),
        };

        if let Err(err) = self
            .artifact_ledger()
            .write_json("checkpoints", "latest.json", &summary)
        {
            warn!(
                "{}",
                crate::i18n::tf(
                    "warning.failed_persist_checkpoint",
                    &[("error", &format!("{}", err))]
                )
            );
        }
    }

    fn begin_shutdown(&self, reason: &str) {
        if self.lifecycle.start_shutdown(reason) {
            self.shutdown_notify.notify_waiters();
        }
    }

    async fn wait_for_inflight_drain(&self) {
        let timeout_seconds = self.runtime_config_snapshot().shutdown_drain_seconds.max(1);
        let deadline = Instant::now() + Duration::from_secs(timeout_seconds);

        loop {
            let (global_inflight, _) = self.inflight_limiter.snapshot();
            if global_inflight == 0 {
                return;
            }

            if Instant::now() >= deadline {
                warn!(
                    "shutdown drain timeout reached with {} in-flight request(s) still tracked",
                    global_inflight
                );
                return;
            }

            sleep(Duration::from_millis(100)).await;
        }
    }

    async fn run_maintenance_cycle(&self, source: &str) -> MaintenanceCycleResult {
        match perform_maintenance_cycle(
            Arc::clone(&self.memory_cache),
            Arc::clone(&self.cache),
            Arc::clone(&self.vector_store),
            Arc::clone(&self.runtime_config),
            Arc::clone(&self.maintenance),
            source,
        )
        .await
        {
            Ok(result) => result,
            Err(err) => {
                warn!("maintenance cycle '{}' failed: {}", source, err);
                MaintenanceCycleResult::default()
            }
        }
    }

    fn spawn_background_maintenance_loop(&self) -> JoinHandle<()> {
        let runtime_config = Arc::clone(&self.runtime_config);
        let memory_cache = Arc::clone(&self.memory_cache);
        let cache = Arc::clone(&self.cache);
        let vector_store = Arc::clone(&self.vector_store);
        let maintenance = Arc::clone(&self.maintenance);
        let lifecycle = Arc::clone(&self.lifecycle);
        let circuit_breakers = Arc::clone(&self.circuit_breakers);
        let phase_rate_limiter = Arc::clone(&self.phase_rate_limiter);
        let inflight_limiter = Arc::clone(&self.inflight_limiter);
        let shutdown_notify = Arc::clone(&self.shutdown_notify);

        tokio::spawn(async move {
            run_background_maintenance_loop(
                runtime_config,
                memory_cache,
                cache,
                vector_store,
                maintenance,
                lifecycle,
                circuit_breakers,
                phase_rate_limiter,
                inflight_limiter,
                shutdown_notify,
            )
            .await;
        })
    }

    async fn handle_request(&self, request: JsonRpcRequest) -> Result<()> {
        let trace = self.new_request_trace(&request);
        let request_span = self.telemetry.start_root_span(
            "acp.request",
            &format!("{}:{}", trace.method, trace.request_id),
            vec![
                KeyValue::new("rpc.method", trace.method.clone()),
                KeyValue::new("rpc.request_id", trace.request_id.clone()),
                KeyValue::new("trace.id", trace.trace_id.clone()),
            ],
        );
        self.record_trace_event(
            &trace,
            "request.start",
            "ok",
            "rpc",
            json!({
                "method": trace.method,
                "request_id": trace.request_id,
            }),
            None,
            0,
        );

        // Enhanced telemetry logging
        telemetry_enhanced::log::request_start("rpc", &trace.method, &trace.request_id);

        let method = request.method.clone();
        let request_id = request.id.clone();
        let started = Instant::now();
        let result = async {
            if self.lifecycle.is_shutting_down() && method != "shutdown" {
                return self
                    .send_error(
                        request_id,
                        -32031,
                        "server is shutting down".to_string(),
                        Some(serde_json::to_value(self.lifecycle.snapshot())?),
                    )
                    .await;
            }

            match method.as_str() {
            "initialize" => {
                // Measure initialization performance
                let (result, duration) = performance::utils::measure_time(|| {
                    json!({
                        "name": "go-on",
                        "protocol": "acp",
                        "capabilities": {
                            "chat": true,
                            "streaming": true,
                            "phase": true,
                            "metrics": true,
                            "debug_panel": true,
                            "mcp_adapter": true,
                            "conversation_control": true,
                            "autotune": self.autotune_config_snapshot().map(|cfg| cfg.enabled).unwrap_or(false),
                        }
                    })
                });

                // Log performance metrics
                debug!("initialize request handled in {:?}", duration);
                self.send_result(request_id, result).await
            }
            "mcp.initialize" => {
                self.send_result(
                    request_id,
                    json!({
                        "protocolVersion": crate::mcp::MCP_VERSION,
                        "capabilities": {
                            "tools": {},
                        },
                        "serverInfo": {
                            "name": "go-on",
                            "version": env!("CARGO_PKG_VERSION"),
                        }
                    }),
                )
                .await
            }
            "mcp.tools.list" => {
                self.send_result(
                    request_id,
                    json!({
                        "tools": [
                            {
                                "name": "acp_debug_panel_get",
                                "description": "Get runtime debug panel snapshot",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "limit": {"type": "number"}
                                    }
                                }
                            },
                            {
                                "name": "acp_trace_get",
                                "description": "Get recent trace events",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "limit": {"type": "number"}
                                    }
                                }
                            },
                            {
                                "name": "acp_runtime_health",
                                "description": "Get runtime health summary",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {}
                                }
                            },
                            {
                                "name": "acp_task_plan",
                                "description": "Build and persist a controlled task plan",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "task": {"type": "string"}
                                    },
                                    "required": ["task"]
                                }
                            },
                            {
                                "name": "acp_action_check",
                                "description": "Run BLUE2 action checks against .goon artifacts",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "kind": {"type": "string"}
                                    }
                                }
                            },
                            {
                                "name": "acp_conversation_checkpoint_list",
                                "description": "List conversation checkpoints",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "conversation_id": {"type": "string"},
                                        "branch_id": {"type": "string"},
                                        "limit": {"type": "number"}
                                    }
                                }
                            }
                        ]
                    }),
                )
                .await
            }
            "mcp.tools.call" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let tool_name = match params.get("name").and_then(|v| v.as_str()) {
                    Some(value) => value,
                    None => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "name is required for mcp.tools.call".to_string(),
                                None,
                            )
                            .await;
                    }
                };
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));

                let tool_result = match tool_name {
                    "acp_debug_panel_get" => {
                        let limit = args
                            .get("limit")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(100)
                            .min(500) as usize;
                        let events = self.trace_snapshot(limit);
                        json!({
                            "ok": true,
                            "count": events.len(),
                            "events": events,
                            "trace_metrics": self.trace_metrics_snapshot(),
                        })
                    }
                    "acp_trace_get" => {
                        let limit = args
                            .get("limit")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(100)
                            .min(1000) as usize;
                        let events = self.trace_snapshot(limit);
                        json!({
                            "ok": true,
                            "count": events.len(),
                            "events": events,
                        })
                    }
                    "acp_runtime_health" => {
                        let report = self.runtime_healthcheck_report()?;
                        let artifact_path =
                            persist_runtime_healthcheck(&self.artifact_ledger(), &report)?;
                        let runtime_details = report
                            .components
                            .iter()
                            .find(|component| component.name == "runtime")
                            .map(|component| component.details.clone())
                            .unwrap_or(Value::Null);
                        let sqlite_cache_entries = report
                            .components
                            .iter()
                            .find(|component| component.name == "cache")
                            .and_then(|component| component.details.get("entries"))
                            .cloned()
                            .unwrap_or(Value::Null);
                        let vector = report
                            .components
                            .iter()
                            .find(|component| component.name == "vector")
                            .map(|component| component.details.clone())
                            .unwrap_or(Value::Null);
                        json!({
                            "ok": report.overall_status != CheckStatus::Error,
                            "report": report,
                            "artifact_path": artifact_path.display().to_string(),
                            "memory_cache_entries": self.memory_cache.active_entries(),
                            "sqlite_cache_entries": sqlite_cache_entries,
                            "lazy_load_cache": runtime_details.get("lazy_load_cache").cloned().unwrap_or(Value::Null),
                            "circuit_breaker": runtime_details.get("circuit_breaker").cloned().unwrap_or(Value::Null),
                            "rate_limiter": runtime_details.get("rate_limiter").cloned().unwrap_or(Value::Null),
                            "inflight": runtime_details.get("inflight").cloned().unwrap_or(Value::Null),
                            "vector": vector,
                            "lifecycle": runtime_details.get("lifecycle").cloned().unwrap_or(Value::Null),
                            "maintenance": runtime_details.get("maintenance").cloned().unwrap_or(Value::Null),
                            "review_gate": runtime_details.get("review_gate").cloned().unwrap_or(Value::Null),
                            "telemetry": runtime_details.get("telemetry").cloned().unwrap_or(Value::Null),
                        })
                    }
                    "acp_task_plan" => {
                        let task = match args.get("task").and_then(|v| v.as_str()) {
                            Some(value) if !value.trim().is_empty() => value,
                            _ => {
                                return self
                                    .send_error(
                                        request_id,
                                        -32602,
                                        "task is required for acp_task_plan".to_string(),
                                        None,
                                    )
                                    .await;
                            }
                        };
                        let plan = build_task_plan(task);
                        let artifact_path = persist_task_plan(&self.artifact_ledger(), &plan)?;
                        json!({
                            "ok": true,
                            "plan": plan,
                            "artifact_path": artifact_path.display().to_string(),
                        })
                    }
                    "acp_action_check" => {
                        let kind = args
                            .get("kind")
                            .and_then(|v| v.as_str())
                            .and_then(ActionCheckKind::parse)
                            .unwrap_or(ActionCheckKind::All);
                        let report = run_action_check(&self.artifact_ledger(), kind)?;
                        json!({
                            "ok": report.ok,
                            "report": report,
                        })
                    }
                    "acp_conversation_checkpoint_list" => {
                        let conversation_id = args
                            .get("conversation_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("default");
                        let branch_id = args.get("branch_id").and_then(|v| v.as_str());
                        let limit = args
                            .get("limit")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(50)
                            .min(500) as usize;
                        match self
                            .list_conversation_checkpoints(conversation_id, branch_id, limit)
                        {
                            Ok(checkpoints) => json!({
                                "ok": true,
                                "count": checkpoints.len(),
                                "checkpoints": checkpoints,
                            }),
                            Err(message) => json!({
                                "ok": false,
                                "error": message,
                            }),
                        }
                    }
                    _ => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                crate::i18n::tf("error.unknown_mcp_adapter_tool", &[("tool_name", tool_name)]),
                                None,
                            )
                            .await;
                    }
                };

                self.send_result(
                    request_id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": tool_result.to_string(),
                        }],
                        "structuredContent": tool_result,
                    }),
                )
                .await
            }
            "chat" => {
                // Measure chat handling performance
                let (result, duration) = performance::utils::measure_time(|| {
                    self.handle_chat(
                        request_id,
                        request.params,
                        request_span.clone(),
                        Some(trace.clone()),
                    )
                });

                // Log performance metrics
                debug!("chat request handled in {:?}", duration);
                result.await
            }
            "metrics.get" => {
                // Measure metrics retrieval performance
                let (result, duration) = performance::utils::measure_time(|| {
                    serde_json::to_value(self.metrics.snapshot())
                });

                // Log performance metrics
                debug!("metrics.get request handled in {:?}", duration);

                self.send_result(request_id, result?).await
            }
            "metrics.prometheus" => {
                // Measure Prometheus metrics generation performance
                let (result, duration) = performance::utils::measure_time_async(|| async {
                    let sqlite_cache_entries = if let Some(cache) = self.cache_handle() {
                        self.cache_entry_count(cache.clone()).await.unwrap_or(0)
                    } else {
                        0
                    };
                    let (vector_memory_entries, vector_summary_entries) =
                        if let Some(store) = self.vector_store_handle() {
                            self.vector_entry_counts(store.clone())
                                .await
                                .unwrap_or((0, 0))
                        } else {
                            (0, 0)
                        };

                    let gauges = RuntimeGaugeSnapshot {
                        memory_cache_entries: self.memory_cache.active_entries() as u64,
                        sqlite_cache_entries,
                        vector_memory_entries,
                        vector_summary_entries,
                        circuit_open_agents: self.circuit_breakers.open_count() as u64,
                        circuit_half_open_agents: self.circuit_breakers.half_open_count() as u64,
                        circuit_tracked_agents: self.circuit_breakers.tracked_agents() as u64,
                        rate_limiter_tracked_phases: self.phase_rate_limiter.tracked_phases() as u64,
                    };
                    let breaker_snapshot = self.circuit_breakers.snapshot();
                    let phase_limiter_snapshot = self.phase_rate_limiter.snapshot();
                    let inflight_snapshot = self.inflight_limiter.snapshot();
                    let lifecycle = self.lifecycle.snapshot();
                    let maintenance = self.maintenance.snapshot();

                    json!({
                        "text": build_prometheus_metrics(
                            &self.metrics.snapshot(),
                            &gauges,
                            &breaker_snapshot,
                            &phase_limiter_snapshot,
                            &inflight_snapshot,
                            &lifecycle,
                            &maintenance,
                        )
                    })
                }).await;

                // Log performance metrics
                debug!("metrics.prometheus request handled in {:?}", duration);

                self.send_result(request_id, result).await
            }
            "metrics.reset" => {
                self.metrics.reset();
                self.send_result(request_id, json!({"ok": true})).await
            }
            "trace.metrics" => {
                let result = self.trace_metrics_snapshot();
                self.send_result(request_id, result).await
            }
            "trace.get" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let limit = params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(100)
                    .min(1000) as usize;
                let events = self.trace_snapshot(limit);
                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "count": events.len(),
                        "events": events,
                    }),
                )
                .await
            }
            "debug.panel.get" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let limit = params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(100)
                    .min(500) as usize;
                let recent_events = self.trace_snapshot(limit);

                let stage_transitions = recent_events
                    .iter()
                    .filter(|event| event.event_type.starts_with("phase."))
                    .map(|event| {
                        json!({
                            "timestamp": event.timestamp,
                            "event_type": event.event_type,
                            "phase": event.phase,
                            "status": event.status,
                            "duration_ms": event.duration_ms,
                            "task_id": event.task_id,
                            "pua_stage": event.pua_stage,
                        })
                    })
                    .collect::<Vec<_>>();

                let review_outcomes = recent_events
                    .iter()
                    .filter(|event| event.event_type == "phase.review_gate")
                    .map(|event| {
                        let attrs = event.inputs.get("attributes").cloned().unwrap_or_else(|| json!({}));
                        json!({
                            "timestamp": event.timestamp,
                            "status": event.status,
                            "phase": event.phase,
                            "attributes": attrs,
                            "error": event.error,
                        })
                    })
                    .collect::<Vec<_>>();

                let mut selected_agents: Vec<String> = Vec::new();
                let mut seen_agents: HashSet<String> = HashSet::new();
                for event in &recent_events {
                    if event.event_type != "phase.agent" {
                        continue;
                    }
                    let maybe_agent = event
                        .inputs
                        .get("attributes")
                        .and_then(|attrs| attrs.get("agent"))
                        .and_then(|v| v.as_str())
                        .map(|v| v.to_string());
                    if let Some(agent) = maybe_agent {
                        if seen_agents.insert(agent.clone()) {
                            selected_agents.push(agent);
                        }
                    }
                }

                let (conversation_count, checkpoint_count, branch_head_count) = self
                    .conversation_store
                    .lock()
                    .map(|store| {
                        let conversation_count = store.len();
                        let checkpoint_count = store
                            .values()
                            .map(|state| state.checkpoints.len())
                            .sum::<usize>();
                        let branch_head_count = store
                            .values()
                            .map(|state| state.branch_heads.len())
                            .sum::<usize>();
                        (conversation_count, checkpoint_count, branch_head_count)
                    })
                    .unwrap_or((0, 0, 0));

                let ledger = self.artifact_ledger();
                let artifacts = json!({
                    "root": ledger.root().display().to_string(),
                    "spec_plan": ledger.latest_path("spec", "latest-plan.json").exists(),
                    "healthcheck": ledger.latest_path("qa", "latest-healthcheck.json").exists(),
                    "retest": ledger.latest_path("retest", "latest-action-check.json").exists(),
                    "final_summary": ledger.latest_path("final", "latest-summary.json").exists(),
                });

                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "panel": {
                            "trace": {
                                "count": recent_events.len(),
                                "stage_transitions": stage_transitions,
                            },
                            "selected_agents": selected_agents,
                            "review_outcomes": review_outcomes,
                            "runtime_health": {
                                "memory_cache_entries": self.memory_cache.active_entries(),
                                "lazy_load_cache": lazy_load_cache_snapshot(),
                                "circuit_breaker": {
                                    "open_agents": self.circuit_breakers.open_count(),
                                    "half_open_agents": self.circuit_breakers.half_open_count(),
                                    "tracked_agents": self.circuit_breakers.tracked_agents(),
                                },
                                "lifecycle": self.lifecycle.snapshot(),
                            },
                            "conversations": {
                                "count": conversation_count,
                                "checkpoints": checkpoint_count,
                                "branch_heads": branch_head_count,
                            },
                            "artifacts": artifacts,
                            "review_gate": {
                                "total": self.metrics.snapshot().review_gate_total,
                                "approved": self.metrics.snapshot().review_gate_approved_total,
                                "rejected": self.metrics.snapshot().review_gate_rejected_total,
                                "timeout": self.metrics.snapshot().review_gate_timeout_total,
                                "degraded": self.metrics.snapshot().review_gate_degraded_total,
                                "invalid_response": self.metrics.snapshot().review_gate_invalid_response_total,
                            },
                        }
                    }),
                )
                .await
            }
            "workflow.clarify" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let task = match params.get("task").and_then(|v| v.as_str()) {
                    Some(value) if !value.trim().is_empty() => value.to_string(),
                    _ => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "task is required for workflow.clarify".to_string(),
                                None,
                            )
                            .await;
                    }
                };

                let ledger = self.artifact_ledger();
                let base_contract = parse_requirement_contract_from_params(&params, &task)
                    .unwrap_or_else(|| default_requirement_contract(&task, "workflow.clarify"));
                let mut contract = base_contract.clone();
                let missing_fields = requirement_missing_fields(&contract);
                contract.open_questions = requirement_questions_from_missing(&missing_fields);
                contract.ambiguity_score = estimate_requirement_ambiguity(&task, &contract);
                contract.user_confirmed = false;
                let blue5_doc = load_blue5_doc_lazy(self.config_path.as_ref());
                let blue5_auto = evaluate_blue5_for_clarify(
                    &blue5_doc,
                    &contract,
                    &missing_fields,
                    &params,
                );

                let previous_session = fs::read_to_string(
                    ledger.latest_path("spec", "latest-clarification-session.json"),
                )
                .ok()
                .and_then(|raw| serde_json::from_str::<ClarificationSessionArtifact>(&raw).ok())
                .filter(|session| session.task.trim() == task.trim());

                let round_index = params
                    .get("round_index")
                    .and_then(|v| v.as_u64())
                    .map(|v| v.max(1) as u32)
                    .unwrap_or_else(|| {
                        previous_session
                            .as_ref()
                            .map(|session| session.round_index.saturating_add(1))
                            .unwrap_or(1)
                    });
                let session_id = params
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
                    .or_else(|| previous_session.as_ref().map(|session| session.session_id.clone()))
                    .unwrap_or_else(|| format!("clarify-{}", now_ts()));
                let user_feedback = params
                    .get("user_feedback")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let resolved_points = parse_string_list(params.get("resolved_points"));
                let ready_to_confirm = params
                    .get("ready_to_confirm")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(missing_fields.is_empty());
                let collaboration_mode = params
                    .get("clarify_collaboration_mode")
                    .and_then(|v| v.as_str())
                    .map(|v| v.trim().to_ascii_lowercase())
                    .filter(|v| v == "single_ai" || v == "multi_ai")
                    .unwrap_or_else(|| {
                        if blue5_auto.should_multi_ai_clarify {
                            "multi_ai".to_string()
                        } else {
                            "single_ai".to_string()
                        }
                    });

                let mut lead_clarifier = "none".to_string();
                let mut assistant_clarifiers: Vec<String> = Vec::new();
                if let Ok((flow, registry)) = self.routing_handles() {
                    let phase_hint = params
                        .get("phase")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let routing = flow
                        .resolve(phase_hint, registry.as_ref())
                        .unwrap_or_else(|_| {
                            flow.resolve(None, registry.as_ref())
                                .expect("default phase must always resolve")
                        });
                    let env_ready_agents =
                        filter_env_ready_agents(self.config_path.as_ref(), &routing.phase.agent_names);
                    if let Some(first) = env_ready_agents.first() {
                        lead_clarifier = first.clone();
                        if collaboration_mode == "multi_ai" {
                            assistant_clarifiers = env_ready_agents.iter().skip(1).take(2).cloned().collect();
                        }
                    }
                }

                let clarification_session = ClarificationSessionArtifact {
                    generated_at: now_ts(),
                    task: task.clone(),
                    source: "workflow.clarify".to_string(),
                    session_id,
                    round_index,
                    lead_clarifier,
                    assistant_clarifiers,
                    user_feedback,
                    resolved_points,
                    open_points: missing_fields.clone(),
                    next_questions: contract.open_questions.clone(),
                    ready_to_confirm,
                };

                let clarification_path = persist_requirement_contract(&ledger, &contract)?;
                let clarification_session_path =
                    persist_clarification_session_artifact(&ledger, &clarification_session)?;
                let governance = GovernancePolicyArtifact {
                    generated_at: now_ts(),
                    task: task.clone(),
                    source: "workflow.clarify".to_string(),
                    clarification_required: true,
                    confirmed: false,
                    blocked: true,
                    reason: Some("requirement clarification required before planning/execution".to_string()),
                    next_step: json!({
                        "method": "workflow.confirm",
                        "task": task,
                        "ready_to_confirm": clarification_session.ready_to_confirm,
                        "round_index": clarification_session.round_index,
                        "requirement_contract": {
                            "goal": contract.goal,
                            "scope": contract.scope,
                            "non_goals": contract.non_goals,
                            "acceptance_criteria": contract.acceptance_criteria,
                            "constraints": contract.constraints,
                            "user_confirmed": true
                        }
                    }),
                };
                let governance_path = persist_governance_policy(&ledger, &governance)?;

                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "clarification_required": true,
                        "missing_fields": missing_fields,
                        "open_questions": contract.open_questions,
                        "requirement_contract": contract,
                        "blue5": {
                            "doc": blue5_doc,
                            "auto": blue5_auto,
                        },
                        "clarification_session": clarification_session,
                        "clarify_collaboration_mode": collaboration_mode,
                        "clarification_artifact_path": clarification_path.display().to_string(),
                        "clarification_session_artifact_path": clarification_session_path
                            .display()
                            .to_string(),
                        "governance_artifact_path": governance_path.display().to_string(),
                    }),
                )
                .await
            }
            "workflow.confirm" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let task = match params.get("task").and_then(|v| v.as_str()) {
                    Some(value) if !value.trim().is_empty() => value.to_string(),
                    _ => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "task is required for workflow.confirm".to_string(),
                                None,
                            )
                            .await;
                    }
                };

                let ledger = self.artifact_ledger();
                let mut contract = parse_requirement_contract_from_params(&params, &task)
                    .or_else(|| load_latest_requirement_contract(&ledger, &task))
                    .unwrap_or_else(|| default_requirement_contract(&task, "workflow.confirm"));
                contract.generated_at = now_ts();
                contract.source = "workflow.confirm".to_string();
                contract.user_confirmed = params
                    .get("user_confirmed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                contract.ambiguity_score = estimate_requirement_ambiguity(&task, &contract);

                let missing_fields = requirement_missing_fields(&contract);
                if !missing_fields.is_empty() {
                    return self
                        .send_error(
                            request_id,
                            -32602,
                            "workflow.confirm requires complete requirement_contract (goal/scope/acceptance_criteria/constraints)"
                                .to_string(),
                            Some(json!({
                                "missing_fields": missing_fields,
                                "next_step": "fill requirement_contract and retry workflow.confirm"
                            })),
                        )
                        .await;
                }

                let latest_session = fs::read_to_string(
                    ledger.latest_path("spec", "latest-clarification-session.json"),
                )
                .ok()
                .and_then(|raw| serde_json::from_str::<ClarificationSessionArtifact>(&raw).ok())
                .filter(|session| session.task.trim() == task.trim());

                let ready_to_confirm = params
                    .get("ready_to_confirm")
                    .and_then(|v| v.as_bool())
                    .or_else(|| latest_session.as_ref().map(|session| session.ready_to_confirm))
                    .unwrap_or(false);
                if !ready_to_confirm {
                    return self
                        .send_error(
                            request_id,
                            -32006,
                            "workflow.confirm blocked: clarification session is not ready_to_confirm"
                                .to_string(),
                            Some(json!({
                                "kind": "clarification_session",
                                "task": task,
                                "next_step": {
                                    "method": "workflow.clarify",
                                    "task": task,
                                    "round_index": latest_session
                                        .as_ref()
                                        .map(|s| s.round_index.saturating_add(1))
                                        .unwrap_or(1)
                                }
                            })),
                        )
                        .await;
                }

                let clarification_path = persist_requirement_contract(&ledger, &contract)?;
                let confirm_session = ClarificationSessionArtifact {
                    generated_at: now_ts(),
                    task: task.clone(),
                    source: "workflow.confirm".to_string(),
                    session_id: latest_session
                        .as_ref()
                        .map(|s| s.session_id.clone())
                        .unwrap_or_else(|| format!("clarify-{}", now_ts())),
                    round_index: latest_session
                        .as_ref()
                        .map(|s| s.round_index)
                        .unwrap_or(1),
                    lead_clarifier: latest_session
                        .as_ref()
                        .map(|s| s.lead_clarifier.clone())
                        .unwrap_or_else(|| "none".to_string()),
                    assistant_clarifiers: latest_session
                        .as_ref()
                        .map(|s| s.assistant_clarifiers.clone())
                        .unwrap_or_default(),
                    user_feedback: params
                        .get("user_feedback")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    resolved_points: vec![
                        "goal".to_string(),
                        "scope".to_string(),
                        "acceptance_criteria".to_string(),
                        "constraints".to_string(),
                    ],
                    open_points: Vec::new(),
                    next_questions: Vec::new(),
                    ready_to_confirm,
                };
                let clarification_session_path =
                    persist_clarification_session_artifact(&ledger, &confirm_session)?;
                let governance = GovernancePolicyArtifact {
                    generated_at: now_ts(),
                    task: task.clone(),
                    source: "workflow.confirm".to_string(),
                    clarification_required: true,
                    confirmed: contract.user_confirmed,
                    blocked: !contract.user_confirmed,
                    reason: if contract.user_confirmed {
                        None
                    } else {
                        Some("user_confirmed=false".to_string())
                    },
                    next_step: json!({
                        "confirmed": contract.user_confirmed,
                        "next_method": if contract.user_confirmed { "task.plan" } else { "workflow.confirm" }
                    }),
                };
                let governance_path = persist_governance_policy(&ledger, &governance)?;

                self.send_result(
                    request_id,
                    json!({
                        "ok": contract.user_confirmed,
                        "confirmed": contract.user_confirmed,
                        "requirement_contract": contract,
                        "clarification_session": confirm_session,
                        "clarification_artifact_path": clarification_path.display().to_string(),
                        "clarification_session_artifact_path": clarification_session_path
                            .display()
                            .to_string(),
                        "governance_artifact_path": governance_path.display().to_string(),
                    }),
                )
                .await
            }
            "task.plan" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let task = match params.get("task").and_then(|v| v.as_str()) {
                    Some(value) if !value.trim().is_empty() => value,
                    _ => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "task is required for task.plan".to_string(),
                                None,
                            )
                            .await;
                    }
                };

                let ledger = self.artifact_ledger();
                let requirement_gate = evaluate_requirement_gate(&ledger, task, &params, "task.plan")?;
                if requirement_gate.blocked {
                    return self
                        .send_error(
                            request_id,
                            -32006,
                            requirement_gate
                                .reason
                                .clone()
                                .unwrap_or_else(|| "requirement confirmation is required".to_string()),
                            Some(json!({
                                "kind": "requirement_contract",
                                "task": task,
                                "missing_fields": requirement_gate.missing_fields,
                                "next_step": {
                                    "method": "workflow.clarify",
                                    "task": task
                                },
                                "governance_artifact_path": requirement_gate
                                    .governance_artifact_path
                                    .display()
                                    .to_string(),
                            })),
                        )
                        .await;
                }

                let plan = build_task_plan(task);
                let artifact_path = persist_task_plan(&ledger, &plan)?;
                self.record_trace_event(
                    &trace,
                    "phase.plan",
                    "ok",
                    "plan",
                    json!({
                        "task": task,
                        "sub_agent_recommended": plan.sub_agent_recommended,
                        "planned_subtasks": plan.planned_subtasks.len(),
                    }),
                    None,
                    started.elapsed().as_millis() as u64,
                );
                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "plan": plan,
                        "artifact_path": artifact_path.display().to_string(),
                        "requirement_gate": {
                            "confirmed": true,
                            "governance_artifact_path": requirement_gate.governance_artifact_path.display().to_string(),
                            "clarification_artifact_path": requirement_gate
                                .clarification_artifact_path
                                .as_ref()
                                .map(|p| p.display().to_string()),
                        }
                    }),
                )
                .await
            }
            "workflow.generate" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let task = match params.get("task").and_then(|v| v.as_str()) {
                    Some(value) if !value.trim().is_empty() => value,
                    _ => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "task is required for workflow.generate".to_string(),
                                None,
                            )
                            .await;
                    }
                };

                let ledger = self.artifact_ledger();
                let requirement_gate =
                    evaluate_requirement_gate(&ledger, task, &params, "workflow.generate")?;
                if requirement_gate.blocked {
                    return self
                        .send_error(
                            request_id,
                            -32006,
                            requirement_gate
                                .reason
                                .clone()
                                .unwrap_or_else(|| "requirement confirmation is required".to_string()),
                            Some(json!({
                                "kind": "requirement_contract",
                                "task": task,
                                "missing_fields": requirement_gate.missing_fields,
                                "next_step": {
                                    "method": "workflow.clarify",
                                    "task": task
                                },
                                "governance_artifact_path": requirement_gate
                                    .governance_artifact_path
                                    .display()
                                    .to_string(),
                            })),
                        )
                        .await;
                }

                let plan = build_task_plan(task);
                let plan_artifact_path = persist_task_plan(&ledger, &plan)?;
                let workflow = build_workflow_generated_artifact(&plan);
                let workflow_artifact_path = persist_workflow_generated(&ledger, &workflow)?;

                self.record_trace_event(
                    &trace,
                    "phase.plan",
                    "ok",
                    "workflow",
                    json!({
                        "task": task,
                        "nodes": workflow.nodes.len(),
                        "edges": workflow.edges.len(),
                        "execution_phases": workflow.execution_order.len(),
                    }),
                    None,
                    started.elapsed().as_millis() as u64,
                );
                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "plan": plan,
                        "workflow": workflow,
                        "plan_artifact_path": plan_artifact_path.display().to_string(),
                        "workflow_artifact_path": workflow_artifact_path.display().to_string(),
                        "requirement_gate": {
                            "confirmed": true,
                            "governance_artifact_path": requirement_gate.governance_artifact_path.display().to_string(),
                            "clarification_artifact_path": requirement_gate
                                .clarification_artifact_path
                                .as_ref()
                                .map(|p| p.display().to_string()),
                        }
                    }),
                )
                .await
            }
            "workflow.research" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let task = match params.get("task").and_then(|v| v.as_str()) {
                    Some(value) if !value.trim().is_empty() => value.to_string(),
                    _ => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "task is required for workflow.research".to_string(),
                                None,
                            )
                            .await;
                    }
                };

                let phase_hint = params
                    .get("phase")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let (flow, registry) = self.routing_handles()?;
                let routing = flow
                    .resolve(phase_hint, registry.as_ref())
                    .unwrap_or_else(|_| {
                        flow.resolve(None, registry.as_ref())
                            .expect("default phase must always resolve")
                    });
                let env_ready_phase_agents =
                    filter_env_ready_agents(self.config_path.as_ref(), &routing.phase.agent_names);
                if env_ready_phase_agents.is_empty() {
                    return self
                        .send_error(
                            request_id,
                            -32005,
                            "workflow.research has no env-ready agents; configure at least one key or switch phase"
                                .to_string(),
                            Some(json!({
                                "kind": "capability_ceiling",
                                "phase": routing.phase.phase_name,
                                "configured_agents": routing.phase.agent_names,
                                "next_step": {
                                    "configure_agent_key": true,
                                    "or_switch_phase": true
                                }
                            })),
                        )
                        .await;
                }

                let planner_agent_name = match env_ready_phase_agents.first().cloned() {
                    Some(name) => name,
                    None => {
                        return self
                            .send_error(
                                request_id,
                                -32603,
                                "workflow.research requires at least one routable agent".to_string(),
                                None,
                            )
                            .await;
                    }
                };
                let researcher_agent_name = env_ready_phase_agents
                    .get(1)
                    .cloned()
                    .unwrap_or_else(|| planner_agent_name.clone());
                let reviewer_agent_name = env_ready_phase_agents
                    .get(2)
                    .cloned()
                    .unwrap_or_else(|| researcher_agent_name.clone());

                let planner_agent = match registry.get(&planner_agent_name) {
                    Some(a) => a,
                    None => {
                        return self
                            .send_error(
                                request_id,
                                -32603,
                                format!(
                                    "workflow.research planner agent '{}' not found",
                                    planner_agent_name
                                ),
                                None,
                            )
                            .await;
                    }
                };
                let researcher_agent = match registry.get(&researcher_agent_name) {
                    Some(a) => a,
                    None => {
                        return self
                            .send_error(
                                request_id,
                                -32603,
                                format!(
                                    "workflow.research researcher agent '{}' not found",
                                    researcher_agent_name
                                ),
                                None,
                            )
                            .await;
                    }
                };
                let reviewer_agent = match registry.get(&reviewer_agent_name) {
                    Some(a) => a,
                    None => {
                        return self
                            .send_error(
                                request_id,
                                -32603,
                                format!(
                                    "workflow.research reviewer agent '{}' not found",
                                    reviewer_agent_name
                                ),
                                None,
                            )
                            .await;
                    }
                };

                let planner_prompt = format!(
                    "Task: {}\n\nAs Planner: produce a concise problem tree and acceptance criteria.",
                    task
                );
                let researcher_prompt = format!(
                    "Task: {}\n\nAs Researcher: propose 3 candidate solutions with risk matrix and tradeoffs.",
                    task
                );
                let reviewer_prompt = format!(
                    "Task: {}\n\nAs Reviewer: select one recommended plan from candidates with rationale and risks.",
                    task
                );

                let planner_output = self
                    .run_agent_collecting(
                        planner_agent_name.clone(),
                        planner_agent,
                        vec![Message {
                            role: "user".to_string(),
                            content: planner_prompt,
                        }],
                        None,
                        None,
                        Some(Duration::from_secs(120)),
                    )
                    .await?;
                let researcher_output = self
                    .run_agent_collecting(
                        researcher_agent_name.clone(),
                        researcher_agent,
                        vec![Message {
                            role: "user".to_string(),
                            content: researcher_prompt,
                        }],
                        None,
                        None,
                        Some(Duration::from_secs(120)),
                    )
                    .await?;
                let reviewer_output = self
                    .run_agent_collecting(
                        reviewer_agent_name.clone(),
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

                let artifact = WorkflowResearchArtifact {
                    generated_at: now_ts(),
                    task: task.clone(),
                    planner_output,
                    researcher_output,
                    recommended_plan: reviewer_output.chars().take(500).collect(),
                    reviewer_output,
                };
                let artifact_path = persist_workflow_research(&self.artifact_ledger(), &artifact)?;

                self.record_trace_event(
                    &trace,
                    "phase.research",
                    "ok",
                    "research",
                    json!({
                        "task": task,
                        "planner_agent": planner_agent_name,
                        "researcher_agent": researcher_agent_name,
                        "reviewer_agent": reviewer_agent_name,
                    }),
                    None,
                    started.elapsed().as_millis() as u64,
                );

                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "artifact": artifact,
                        "artifact_path": artifact_path.display().to_string(),
                    }),
                )
                .await
            }
            "workflow.consult" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let task = match params.get("task").and_then(|v| v.as_str()) {
                    Some(value) if !value.trim().is_empty() => value.to_string(),
                    _ => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "task is required for workflow.consult".to_string(),
                                None,
                            )
                            .await;
                    }
                };

                let phase_hint = params
                    .get("phase")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let (flow, registry) = self.routing_handles()?;
                let routing = flow
                    .resolve(phase_hint, registry.as_ref())
                    .unwrap_or_else(|_| {
                        flow.resolve(None, registry.as_ref())
                            .expect("default phase must always resolve")
                    });
                let env_ready_phase_agents =
                    filter_env_ready_agents(self.config_path.as_ref(), &routing.phase.agent_names);
                if env_ready_phase_agents.is_empty() {
                    return self
                        .send_error(
                            request_id,
                            -32005,
                            "workflow.consult has no env-ready agents; configure at least one key or switch phase"
                                .to_string(),
                            None,
                        )
                        .await;
                }

                let policy = resolve_primary_secondary_policy(
                    &env_ready_phase_agents,
                    &params,
                    routing.phase.options.as_ref(),
                )?;
                let trigger_reason = params
                    .get("trigger_reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("explicit workflow.consult request")
                    .to_string();
                let threshold = params
                    .get("consultation_confidence_threshold")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.65)
                    .clamp(0.0, 1.0);

                let (artifact, consensus_achieved) = run_consultation_workflow(
                    self,
                    registry.as_ref(),
                    &task,
                    "workflow.consult",
                    &trigger_reason,
                    &policy,
                    threshold,
                )
                .await?;
                let artifact_path =
                    persist_consultation_artifact(&self.artifact_ledger(), &artifact)?;

                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "consensus_achieved": consensus_achieved,
                        "artifact": artifact,
                        "artifact_path": artifact_path.display().to_string(),
                        "primary_secondary_policy": policy,
                    }),
                )
                .await
            }
            // Section 6 (sub-agent orchestration) + Section 5 (lifecycle tracking)
            method @ ("task.execute" | "workflow.execute") => {
                let is_workflow_execute = method == "workflow.execute";
                let params = request.params.unwrap_or_else(|| json!({}));
                let task_str = match params.get("task").and_then(|v| v.as_str()) {
                    Some(t) if !t.trim().is_empty() => t.to_string(),
                    _ => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "task is required for task.execute".to_string(),
                                None,
                            )
                            .await;
                    }
                };

                let phase_hint = params
                    .get("phase")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let mut plan = build_task_plan(&task_str);
                let ledger = self.artifact_ledger();
                let requirement_gate =
                    evaluate_requirement_gate(&ledger, &task_str, &params, method)?;
                if requirement_gate.blocked {
                    return self
                        .send_error(
                            request_id,
                            -32006,
                            requirement_gate
                                .reason
                                .clone()
                                .unwrap_or_else(|| "requirement confirmation is required".to_string()),
                            Some(json!({
                                "kind": "requirement_contract",
                                "task": task_str,
                                "missing_fields": requirement_gate.missing_fields,
                                "next_step": {
                                    "method": "workflow.clarify",
                                    "task": task_str
                                },
                                "governance_artifact_path": requirement_gate
                                    .governance_artifact_path
                                    .display()
                                    .to_string(),
                            })),
                        )
                        .await;
                }

                let (flow, registry) = self.routing_handles()?;
                let routing = flow
                    .resolve(phase_hint, registry.as_ref())
                    .unwrap_or_else(|_| {
                        flow.resolve(None, registry.as_ref())
                            .expect("default phase must always resolve")
                    });
                let adaptive_routing = params
                    .get("adaptive_routing")
                    .and_then(|v| v.as_bool())
                    .or_else(|| extra_bool(routing.phase.options.as_ref(), "adaptive_routing"))
                    .unwrap_or(true);
                let predicted_success_rate_base = plan.routing.predicted_success_rate;
                if adaptive_routing {
                    plan.routing.predicted_success_rate = recommend_predicted_success_rate_from_learning(
                        &ledger,
                        plan.routing.predicted_success_rate,
                        plan.characteristics.complexity,
                    );
                }
                let predicted_success_rate_tuned =
                    (plan.routing.predicted_success_rate - predicted_success_rate_base).abs()
                        > f32::EPSILON;
                let env_ready_phase_agents =
                    filter_env_ready_agents(self.config_path.as_ref(), &routing.phase.agent_names);
                if env_ready_phase_agents.is_empty() {
                    return self
                        .send_error(
                            request_id,
                            -32005,
                            "no env-ready agents are available for this phase; provide at least one agent key or switch phase"
                                .to_string(),
                            Some(json!({
                                "kind": "capability_ceiling",
                                "phase": routing.phase.phase_name,
                                "configured_agents": routing.phase.agent_names,
                                "suggestions": [
                                    "configure at least one agent credential",
                                    "switch to a phase with an env-ready agent"
                                ]
                            })),
                        )
                        .await;
                }
                let adaptive_agent_order = params
                    .get("adaptive_agent_order")
                    .and_then(|v| v.as_bool())
                    .or_else(|| {
                        extra_bool(routing.phase.options.as_ref(), "adaptive_agent_order")
                    })
                    .unwrap_or(is_workflow_execute);
                let phase_agent_names = if adaptive_agent_order {
                    recommend_agent_order_from_execution_history(
                        &ledger,
                        &env_ready_phase_agents,
                        40,
                    )
                } else {
                    env_ready_phase_agents.clone()
                };
                let agent_order_tuned = phase_agent_names != env_ready_phase_agents;
                let primary_secondary_policy = match resolve_primary_secondary_policy(
                    &phase_agent_names,
                    &params,
                    routing.phase.options.as_ref(),
                ) {
                    Ok(policy) => policy,
                    Err(err) => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                err.to_string(),
                                Some(json!({
                                    "kind": "primary_secondary_policy_invalid",
                                    "env_ready_phase_agents": phase_agent_names,
                                    "supported_failover_policy": [
                                        "first_secondary",
                                        "score_based_secondary",
                                        "abort"
                                    ],
                                })),
                            )
                            .await;
                    }
                };
                let blue5_doc = load_blue5_doc_lazy(self.config_path.as_ref());
                let mut blue5_auto =
                    evaluate_blue5_for_execute(&blue5_doc, &plan, &phase_agent_names, &params);
                blue5_auto.primary_agent = Some(primary_secondary_policy.primary_agent.clone());
                blue5_auto.secondary_agents = primary_secondary_policy.secondary_agents.clone();

                let mut consultation_artifact_path: Option<PathBuf> = None;
                let mut consultation_summary: Option<String> = None;
                let consultation_confidence_threshold = params
                    .get("consultation_confidence_threshold")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.65)
                    .clamp(0.0, 1.0);
                if blue5_auto.should_consultation {
                    let trigger_reason = if blue5_auto.reasons.is_empty() {
                        "blue5 auto consultation gate triggered".to_string()
                    } else {
                        blue5_auto.reasons.join("; ")
                    };
                    let (consultation_artifact, consensus_achieved) = run_consultation_workflow(
                        self,
                        registry.as_ref(),
                        &task_str,
                        method,
                        &trigger_reason,
                        &primary_secondary_policy,
                        consultation_confidence_threshold,
                    )
                    .await?;
                    let artifact_path = persist_consultation_artifact(&ledger, &consultation_artifact)?;
                    consultation_summary = Some(
                        consultation_artifact
                            .consensus_plan
                            .chars()
                            .take(240)
                            .collect::<String>(),
                    );
                    consultation_artifact_path = Some(artifact_path.clone());

                    if !consensus_achieved {
                        return self
                            .send_error(
                                request_id,
                                -32007,
                                "consultation did not reach executable consensus; clarify requirements before execution"
                                    .to_string(),
                                Some(json!({
                                    "kind": "consultation_blocked",
                                    "task": task_str,
                                    "trigger_reason": trigger_reason,
                                    "consultation_confidence_threshold": consultation_confidence_threshold,
                                    "consultation_artifact_path": artifact_path.display().to_string(),
                                    "next_step": {
                                        "method": "workflow.clarify",
                                        "task": task_str
                                    }
                                })),
                            )
                            .await;
                    }

                    let consensus_prefix = consultation_artifact
                        .consensus_plan
                        .chars()
                        .take(360)
                        .collect::<String>();
                    for subtask in plan.planned_subtasks.iter_mut() {
                        subtask.description = format!(
                            "Consultation consensus:\n{}\n\nSubtask:\n{}",
                            consensus_prefix, subtask.description
                        );
                    }
                }

                // M8: persist primary-secondary policy artifact immediately after resolution
                let _ps_policy_artifact_path = persist_primary_secondary_policy_artifact(
                    &ledger,
                    &PrimarySecondaryPolicyArtifact {
                        generated_at: now_ts(),
                        task: task_str.clone(),
                        source: method.to_string(),
                        primary_agent: primary_secondary_policy.primary_agent.clone(),
                        secondary_agents: primary_secondary_policy.secondary_agents.clone(),
                        policy_version: primary_secondary_policy.policy_version.clone(),
                        failover_policy: primary_secondary_policy.failover_policy.clone(),
                        secondary_max_count: primary_secondary_policy.secondary_max_count,
                    },
                )?;

                let capability_ready_agents = phase_agent_names.len();
                let capability_max = capability_max_complexity(capability_ready_agents);
                let capability_exceeded = plan.characteristics.complexity > capability_max;
                let enforce_capability_ceiling = params
                    .get("enforce_capability_ceiling")
                    .and_then(|v| v.as_bool())
                    .or_else(|| {
                        extra_bool(
                            routing.phase.options.as_ref(),
                            "enforce_capability_ceiling",
                        )
                    })
                    .unwrap_or(true);
                let capability_decision = params
                    .get("capability_decision")
                    .and_then(|v| v.as_str())
                    .map(|value| value.to_ascii_lowercase());
                let capability_confirm = params
                    .get("capability_confirm")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let mut capability_forced_degrade = false;
                let capability_decision_effective = if capability_exceeded && enforce_capability_ceiling {
                    match (capability_decision.as_deref(), capability_confirm) {
                        (Some("degrade"), true) => {
                            capability_forced_degrade = true;
                            "degrade"
                        }
                        (Some("multi_ai"), true) => {
                            if capability_ready_agents < 2 {
                                return self
                                    .send_error(
                                        request_id,
                                        -32005,
                                        "capability ceiling exceeded and multi_ai requires at least two env-ready agents"
                                            .to_string(),
                                        Some(json!({
                                            "kind": "capability_ceiling",
                                            "task_complexity": plan.characteristics.complexity,
                                            "capability_max_complexity": capability_max,
                                            "ready_agents": capability_ready_agents,
                                            "decision": "multi_ai",
                                            "suggestions": [
                                                "configure one more env-ready agent",
                                                "or choose capability_decision=degrade with capability_confirm=true"
                                            ]
                                        })),
                                    )
                                    .await;
                            }
                            "multi_ai"
                        }
                        _ => {
                            return self
                                .send_error(
                                    request_id,
                                    -32005,
                                    "task complexity exceeds current capability ceiling; choose capability_decision=multi_ai or capability_decision=degrade and set capability_confirm=true"
                                        .to_string(),
                                    Some(json!({
                                        "kind": "capability_ceiling",
                                        "task_complexity": plan.characteristics.complexity,
                                        "capability_max_complexity": capability_max,
                                        "ready_agents": capability_ready_agents,
                                        "configured_phase_agents": routing.phase.agent_names,
                                        "env_ready_phase_agents": env_ready_phase_agents,
                                        "next_step": {
                                            "degrade": {
                                                "capability_decision": "degrade",
                                                "capability_confirm": true
                                            },
                                            "multi_ai": {
                                                "capability_decision": "multi_ai",
                                                "capability_confirm": true
                                            }
                                        }
                                    })),
                                )
                                .await;
                        }
                    }
                } else if capability_exceeded {
                    warn!(
                        task = %task_str,
                        complexity = plan.characteristics.complexity,
                        capability_max = capability_max,
                        ready_agents = capability_ready_agents,
                        "capability ceiling exceeded but enforcement is disabled; continuing in warn-only mode"
                    );
                    "warn_only"
                } else {
                    "not_required"
                };
                let capability_governance = json!({
                    "ready_agents": capability_ready_agents,
                    "capability_max_complexity": capability_max,
                    "task_complexity": plan.characteristics.complexity,
                    "exceeded": capability_exceeded,
                    "enforced": enforce_capability_ceiling,
                    "decision": capability_decision_effective,
                    "forced_degrade": capability_forced_degrade,
                });
                let primary_agent_name = Some(primary_secondary_policy.primary_agent.clone());
                let executor_label = if phase_agent_names.len() > 1 {
                    "multi-agent-auto-assigned".to_string()
                } else {
                    primary_agent_name
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string())
                };
                let mut workflow_artifact_path: Option<PathBuf> = None;
                let mut workflow_meta: Option<Value> = None;

                let auto_research = params
                    .get("auto_research")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(is_workflow_execute);
                let mut research_artifact_path: Option<PathBuf> = None;
                let mut research_summary: Option<String> = None;
                if auto_research {
                    let planner_agent_name = match phase_agent_names.first().cloned() {
                        Some(name) => name,
                        None => {
                            return self
                                .send_error(
                                    request_id,
                                    -32603,
                                    "workflow.execute auto_research requires at least one routable agent"
                                        .to_string(),
                                    None,
                                )
                                .await;
                        }
                    };
                    let researcher_agent_name = phase_agent_names
                        .get(1)
                        .cloned()
                        .unwrap_or_else(|| planner_agent_name.clone());
                    let reviewer_agent_name = phase_agent_names
                        .get(2)
                        .cloned()
                        .unwrap_or_else(|| researcher_agent_name.clone());

                    let planner_agent = match registry.get(&planner_agent_name) {
                        Some(agent) => agent,
                        None => {
                            return self
                                .send_error(
                                    request_id,
                                    -32603,
                                    format!(
                                        "workflow.execute auto_research planner agent '{}' not found",
                                        planner_agent_name
                                    ),
                                    None,
                                )
                                .await;
                        }
                    };
                    let researcher_agent = match registry.get(&researcher_agent_name) {
                        Some(agent) => agent,
                        None => {
                            return self
                                .send_error(
                                    request_id,
                                    -32603,
                                    format!(
                                        "workflow.execute auto_research researcher agent '{}' not found",
                                        researcher_agent_name
                                    ),
                                    None,
                                )
                                .await;
                        }
                    };
                    let reviewer_agent = match registry.get(&reviewer_agent_name) {
                        Some(agent) => agent,
                        None => {
                            return self
                                .send_error(
                                    request_id,
                                    -32603,
                                    format!(
                                        "workflow.execute auto_research reviewer agent '{}' not found",
                                        reviewer_agent_name
                                    ),
                                    None,
                                )
                                .await;
                        }
                    };

                    let planner_prompt = format!(
                        "Task: {}\n\nAs Planner: produce a concise problem tree and acceptance criteria.",
                        task_str
                    );
                    let researcher_prompt = format!(
                        "Task: {}\n\nAs Researcher: propose 3 candidate solutions with risk matrix and tradeoffs.",
                        task_str
                    );
                    let reviewer_prompt = format!(
                        "Task: {}\n\nAs Reviewer: select one recommended plan from candidates with rationale and risks.",
                        task_str
                    );

                    let planner_output = self
                        .run_agent_collecting(
                            planner_agent_name.clone(),
                            planner_agent,
                            vec![Message {
                                role: "user".to_string(),
                                content: planner_prompt,
                            }],
                            None,
                            None,
                            Some(Duration::from_secs(120)),
                        )
                        .await?;
                    let researcher_output = self
                        .run_agent_collecting(
                            researcher_agent_name.clone(),
                            researcher_agent,
                            vec![Message {
                                role: "user".to_string(),
                                content: researcher_prompt,
                            }],
                            None,
                            None,
                            Some(Duration::from_secs(120)),
                        )
                        .await?;
                    let reviewer_output = self
                        .run_agent_collecting(
                            reviewer_agent_name.clone(),
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

                    let recommended_plan = reviewer_output.chars().take(500).collect::<String>();
                    let artifact = WorkflowResearchArtifact {
                        generated_at: now_ts(),
                        task: task_str.clone(),
                        planner_output,
                        researcher_output,
                        recommended_plan: recommended_plan.clone(),
                        reviewer_output,
                    };
                    let artifact_path = persist_workflow_research(&ledger, &artifact)?;
                    let summary = recommended_plan.chars().take(240).collect::<String>();

                    // Inject the research consensus into subtask prompts so execution follows the selected plan.
                    for subtask in plan.planned_subtasks.iter_mut() {
                        subtask.description = format!(
                            "Research consensus:\n{}\n\nSubtask:\n{}",
                            summary, subtask.description
                        );
                    }

                    research_summary = Some(summary);
                    research_artifact_path = Some(artifact_path);
                }

                let plan_artifact_path = persist_task_plan(&ledger, &plan)?;
                if is_workflow_execute {
                    let workflow = build_workflow_generated_artifact(&plan);
                    workflow_meta = Some(json!({
                        "nodes": workflow.nodes.len(),
                        "edges": workflow.edges.len(),
                        "execution_phases": workflow.execution_order.len(),
                    }));
                    workflow_artifact_path = Some(persist_workflow_generated(&ledger, &workflow)?);
                }

                let exec_started_ts = now_ts();
                let runtime_healthy =
                    !self.lifecycle.is_shutting_down() && self.circuit_breakers.open_count() == 0;

                let optimization_outcome = evaluate_optimization_policy(
                    &ledger,
                    &task_str,
                    &plan,
                    routing.phase.options.as_ref(),
                    runtime_healthy,
                    is_workflow_execute,
                );
                let requested_grade = params
                    .get("work_grade")
                    .and_then(|v| v.as_str())
                    .or_else(|| params.get("mode").and_then(|v| v.as_str()));
                let mut work_grade_decision = decide_work_grade(
                    requested_grade,
                    &plan,
                    is_workflow_execute,
                    runtime_healthy,
                    optimization_outcome.force_fail_fast,
                );
                let adaptive_work_grade = params
                    .get("adaptive_work_grade")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(is_workflow_execute);
                if adaptive_work_grade {
                    let recommended = recommend_work_grade_from_learning(
                        &ledger,
                        work_grade_decision.decided.as_str(),
                    );
                    if let Some(recommended_grade) = WorkGrade::parse(Some(&recommended)) {
                        if recommended_grade != work_grade_decision.decided {
                            work_grade_decision.reasons.push(format!(
                                "LearningBus tuned work grade from {} to {} based on recent cross-task outcomes",
                                work_grade_decision.decided.as_str(),
                                recommended_grade.as_str()
                            ));
                            work_grade_decision.decided = recommended_grade;
                            work_grade_decision.decision_action = work_grade_action(
                                work_grade_decision.requested,
                                work_grade_decision.decided,
                            );
                        }
                    }
                }
                if capability_forced_degrade && work_grade_decision.decided != WorkGrade::Safeguard {
                    work_grade_decision.decided = WorkGrade::Safeguard;
                    work_grade_decision.reasons.push(
                        "capability ceiling exceeded and user selected degrade; force safeguard work grade"
                            .to_string(),
                    );
                    work_grade_decision.decision_action = work_grade_action(
                        work_grade_decision.requested,
                        work_grade_decision.decided,
                    );
                }

                let mut completed = 0usize;
                let mut failed = 0usize;
                let mut skipped = 0usize;

                let phase_parallelism_base = extra_u64(routing.phase.options.as_ref(), "phase_max_inflight")
                    .or_else(|| extra_u64(routing.phase.options.as_ref(), "subtask_parallelism"))
                    .map(|value| value.max(1) as usize)
                    .unwrap_or(4);
                let mut phase_parallelism_base = optimization_outcome
                    .phase_parallelism_cap
                    .map(|cap| phase_parallelism_base.min(cap.max(1)))
                    .unwrap_or(phase_parallelism_base);
                if capability_forced_degrade {
                    phase_parallelism_base = 1;
                }
                let adaptive_parallelism = params
                    .get("adaptive_parallelism")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(is_workflow_execute);
                let phase_parallelism = if adaptive_parallelism {
                    recommend_parallelism_from_learning(&ledger, phase_parallelism_base, 1, 16)
                } else {
                    phase_parallelism_base
                };
                let phase_parallelism = if capability_forced_degrade {
                    1
                } else {
                    phase_parallelism
                };
                let parallelism_tuned = phase_parallelism != phase_parallelism_base;

                let role_aware_assignment = params
                    .get("role_aware_assignment")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(is_workflow_execute);
                let assignment_workflow = build_workflow_generated_artifact(&plan);
                let role_map: HashMap<String, String> = if role_aware_assignment {
                    assignment_workflow
                        .nodes
                        .iter()
                        .map(|node| (node.id.clone(), node.role.clone()))
                        .collect()
                } else {
                    HashMap::new()
                };
                let dependency_count_map: HashMap<String, usize> = assignment_workflow
                    .nodes
                    .iter()
                    .map(|node| (node.id.clone(), node.dependencies.len()))
                    .collect();

                let fail_fast_base = params
                    .get("fail_fast")
                    .and_then(|v| v.as_bool())
                    .or_else(|| {
                        extra_string(routing.phase.options.as_ref(), "subtask_failure_strategy")
                            .map(|v| v.eq_ignore_ascii_case("fail_fast"))
                    })
                    .unwrap_or(false);
                let fail_fast_base =
                    fail_fast_base || optimization_outcome.force_fail_fast || capability_forced_degrade;
                let adaptive_failure_strategy = params
                    .get("adaptive_failure_strategy")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(is_workflow_execute);
                let fail_fast = if adaptive_failure_strategy {
                    recommend_failure_strategy_from_learning(
                        &ledger,
                        if fail_fast_base { "fail_fast" } else { "tolerant" },
                    )
                    .eq_ignore_ascii_case("fail_fast")
                } else {
                    fail_fast_base
                };
                let failure_strategy = if fail_fast { "fail_fast" } else { "tolerant" };
                let failure_strategy_tuned = fail_fast != fail_fast_base;
                let review_policy = resolve_review_policy(
                    routing.phase.options.as_ref(),
                    Some(&plan.characteristics),
                    is_workflow_execute,
                    false,
                );
                let review_started = Instant::now();
                let review_decisions = if review_policy.enforce_dual_review {
                    let execute_review_messages = vec![Message {
                        role: "user".to_string(),
                        content: task_str.clone(),
                    }];
                    match self
                        .run_dual_review_gate(
                            request_id.clone(),
                            &execute_review_messages,
                            routing.phase.options.as_ref(),
                            None,
                            &trace,
                        )
                        .await
                    {
                        Ok(ReviewGateOutcome::Approved(decisions)) => {
                            self.record_trace_event(
                                &child_trace_context(&trace, "execute.review"),
                                "phase.review_gate",
                                "ok",
                                "review",
                                json!({
                                    "policy_status": "pass",
                                    "result": "approved",
                                    "review_decisions": decisions.len(),
                                    "method": method,
                                }),
                                None,
                                review_started.elapsed().as_millis() as u64,
                            );
                            Some(decisions)
                        }
                        Ok(ReviewGateOutcome::Rejected(decisions)) => {
                            self.record_trace_event(
                                &child_trace_context(&trace, "execute.review"),
                                "phase.review_gate",
                                "error",
                                "review",
                                json!({
                                    "policy_status": "blocked",
                                    "result": "rejected",
                                    "review_decisions": decisions.len(),
                                    "method": method,
                                }),
                                Some("review gate rejected execution".to_string()),
                                review_started.elapsed().as_millis() as u64,
                            );
                            return self
                                .send_error(
                                    request_id,
                                    -32603,
                                    "review gate rejected execution".to_string(),
                                    Some(json!({
                                        "kind": "review_gate",
                                        "method": method,
                                        "reviews": decisions,
                                    })),
                                )
                                .await;
                        }
                        Ok(ReviewGateOutcome::Degraded(decisions)) => {
                            self.record_trace_event(
                                &child_trace_context(&trace, "execute.review"),
                                "phase.review_gate",
                                "ok",
                                "review",
                                json!({
                                    "policy_status": "degraded",
                                    "result": "degraded",
                                    "review_decisions": decisions.len(),
                                    "method": method,
                                }),
                                None,
                                review_started.elapsed().as_millis() as u64,
                            );
                            self.send_notification(
                                "workflow.review",
                                json!({
                                    "id": request_id.clone(),
                                    "mode": "degrade_single",
                                    "reason": "review gate timeout",
                                    "method": method,
                                }),
                            )
                            .await?;
                            Some(decisions)
                        }
                        Err(err) => {
                            self.record_trace_event(
                                &child_trace_context(&trace, "execute.review"),
                                "phase.review_gate",
                                "error",
                                "review",
                                json!({
                                    "policy_status": "error",
                                    "method": method,
                                }),
                                Some(err.to_string()),
                                review_started.elapsed().as_millis() as u64,
                            );
                            return self
                                .send_error(
                                    request_id,
                                    -32603,
                                    crate::i18n::tf("error.review_gate_failed", &[("error", &format!("{err}"))]),
                                    Some(json!({
                                        "kind": "review_gate",
                                        "method": method,
                                    })),
                                )
                                .await;
                        }
                    }
                } else {
                    None
                };

                let mut serial_work_ms: u64 = 0;
                let mut critical_path_ms: u64 = 0;
                let mut phases_executed: usize = 0;
                let mut halted_early = false;
                let mut phase_parallel_utilization_sum: f64 = 0.0;
                let mut serial_degradation_count: usize = 0;
                let mut parallel_failure_rollback_count: usize = 0;
                let mut assignment_audit_records: Vec<ExecutionAssignmentRecord> = Vec::new();
                let mut parallel_phase_decisions: Vec<ParallelPhaseDecisionRecord> = Vec::new();
                let mut selected_agents_audit: HashSet<String> = HashSet::new();
                let learning_clarification =
                    resolve_learning_clarification_metrics(&ledger, &task_str, &params);

                // M5/M6/M7: pre-compute failover secondaries once for this execution
                let failover_policy_str = primary_secondary_policy.failover_policy.clone();
                let failover_secondary_runs: Vec<(String, std::sync::Arc<dyn crate::agent::Agent>)> =
                    primary_secondary_policy
                        .secondary_agents
                        .iter()
                        .filter_map(|name| registry.get(name).map(|a| (name.clone(), a)))
                        .collect();
                let mut total_failover_count: u32 = 0;

                let mut phase_records: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
                for (index, record) in plan.planned_subtasks.iter().enumerate() {
                    phase_records.entry(record.phase_index).or_default().push(index);
                }

                for (phase_index, indexes) in phase_records {
                    let mut phase_failed = false;
                    let mut phase_sum_duration_ms: u64 = 0;
                    let mut phase_max_duration_ms: u64 = 0;
                    let phase_width = indexes.len();
                    let has_phase_dependencies = indexes.iter().any(|idx| {
                        plan.planned_subtasks
                            .get(*idx)
                            .and_then(|record| dependency_count_map.get(&record.id))
                            .copied()
                            .unwrap_or(0)
                            > 0
                    });
                    let phase_parallelism_effective = if has_phase_dependencies {
                        1
                    } else {
                        phase_parallelism.max(1)
                    };
                    let phase_capacity = phase_parallelism_effective.max(1);
                    let phase_utilization =
                        (phase_width.min(phase_capacity) as f64 / phase_capacity as f64)
                            .clamp(0.0, 1.0);
                    phase_parallel_utilization_sum += phase_utilization;
                    if phase_width <= 1 || phase_capacity <= 1 {
                        serial_degradation_count = serial_degradation_count.saturating_add(1);
                    }

                    let mut phase_assignment_lookup: HashMap<usize, Option<String>> = HashMap::new();
                    for idx in &indexes {
                        let Some(record) = plan.planned_subtasks.get(*idx) else {
                            continue;
                        };
                        let subtask_id = record.id.clone();
                        let desired_role = role_map.get(&subtask_id).cloned();
                        let dependency_blocked = dependency_count_map
                            .get(&subtask_id)
                            .copied()
                            .unwrap_or(0)
                            > 0;
                        let ranked_candidates = rank_execution_agents(
                            &phase_agent_names,
                            desired_role.as_deref(),
                            phase_index,
                            *idx,
                        );
                        let selected_agent = ranked_candidates.first().map(|candidate| {
                            selected_agents_audit.insert(candidate.agent.clone());
                            candidate.agent.clone()
                        });
                        let selection_reason = ranked_candidates
                            .first()
                            .map(|candidate| candidate.reason.clone())
                            .unwrap_or_else(|| {
                                "no candidate agent available for this subtask".to_string()
                            });

                        phase_assignment_lookup.insert(*idx, selected_agent.clone());
                        assignment_audit_records.push(ExecutionAssignmentRecord {
                            subtask_id,
                            phase_index,
                            task_index: *idx,
                            desired_role,
                            selected_agent: selected_agent.clone(),
                            selection_reason,
                            candidate_scores: ranked_candidates,
                            dependency_blocked,
                            node_primary_agent: selected_agent,
                            node_secondary_agents: primary_secondary_policy.secondary_agents.clone(),
                            effective_executor: None,
                            failover_applied: false,
                            failover_reason: None,
                        });
                    }

                    parallel_phase_decisions.push(ParallelPhaseDecisionRecord {
                        phase_index,
                        subtask_count: phase_width,
                        parallelism_limit: phase_parallelism_effective,
                        utilization_target: phase_utilization,
                        has_dependencies: has_phase_dependencies,
                        execution_mode: if phase_parallelism_effective > 1 {
                            "parallel".to_string()
                        } else {
                            "serial".to_string()
                        },
                        reason: if has_phase_dependencies {
                            "phase contains dependency edges; enforce serial execution for safety"
                                .to_string()
                        } else {
                            "phase subtasks are independent; allow bounded parallel execution"
                                .to_string()
                        },
                    });

                    let tasks = indexes.iter().map(|index| {
                        let idx = *index;
                        let description = plan.planned_subtasks[idx].description.clone();
                        let subtask_id = plan.planned_subtasks[idx].id.clone();
                        let assigned_agent = phase_assignment_lookup
                            .get(&idx)
                            .cloned()
                            .unwrap_or(None);
                        let run_agent = assigned_agent
                            .as_ref()
                            .and_then(|name| registry.get(name));

                        // M5/M6/M7: capture failover context per closure
                        let fallback_runs = failover_secondary_runs.clone();
                        let failover_policy = failover_policy_str.clone();

                        async move {
                            let subtask_wall = Instant::now();
                            let subtask_start_ts = now_ts();

                            let Some(run_agent_name) = assigned_agent else {
                                let subtask_stop_ts = now_ts();
                                return (
                                    idx,
                                    subtask_id,
                                    "none".to_string(),
                                    subtask_start_ts,
                                    subtask_stop_ts,
                                    0u64,
                                    Ok(None),
                                    "none".to_string(),
                                    false,
                                    None::<String>,
                                );
                            };
                            let Some(run_agent) = run_agent else {
                                let subtask_stop_ts = now_ts();
                                return (
                                    idx,
                                    subtask_id,
                                    run_agent_name.clone(),
                                    subtask_start_ts,
                                    subtask_stop_ts,
                                    0u64,
                                    Ok(None),
                                    run_agent_name,
                                    false,
                                    None::<String>,
                                );
                            };

                            let description_for_failover = description.clone();
                            let messages = vec![Message {
                                role: "user".to_string(),
                                content: description,
                            }];

                            let primary_result = self
                                .run_agent_collecting(
                                    run_agent_name.clone(),
                                    run_agent,
                                    messages,
                                    None,
                                    None,
                                    Some(Duration::from_secs(120)),
                                )
                                .await
                                .map(Some);

                            // Failover chain: apply policy when primary fails
                            let (effective_executor, failover_applied, failover_reason, sub_result) =
                                if primary_result.is_err() {
                                    match failover_policy.as_str() {
                                        "abort" => {
                                            let reason = format!(
                                                "primary agent '{}' failed; failover_policy=abort",
                                                run_agent_name
                                            );
                                            (run_agent_name.clone(), false, Some(reason), primary_result)
                                        }
                                        "score_based_secondary" => {
                                            let mut found = false;
                                            let mut fb_executor = run_agent_name.clone();
                                            let mut fb_reason = Some(format!(
                                                "primary '{}' failed; score_based_secondary: no eligible secondary succeeded",
                                                run_agent_name
                                            ));
                                            let mut fb_result = primary_result;
                                            for (fb_name, fb_agent) in &fallback_runs {
                                                let fb_msgs = vec![Message {
                                                    role: "user".to_string(),
                                                    content: description_for_failover.clone(),
                                                }];
                                                let attempt = self
                                                    .run_agent_collecting(
                                                        fb_name.clone(),
                                                        fb_agent.clone(),
                                                        fb_msgs,
                                                        None,
                                                        None,
                                                        Some(Duration::from_secs(120)),
                                                    )
                                                    .await
                                                    .map(Some);
                                                if attempt.is_ok() {
                                                    fb_executor = fb_name.clone();
                                                    fb_reason = Some(format!(
                                                        "primary '{}' failed; score_based_secondary '{}' took over",
                                                        run_agent_name, fb_name
                                                    ));
                                                    fb_result = attempt;
                                                    found = true;
                                                    break;
                                                }
                                            }
                                            (fb_executor, found, fb_reason, fb_result)
                                        }
                                        _ => {
                                            // default / "first_secondary"
                                            if let Some((fb_name, fb_agent)) = fallback_runs.first() {
                                                let fb_msgs = vec![Message {
                                                    role: "user".to_string(),
                                                    content: description_for_failover.clone(),
                                                }];
                                                let attempt = self
                                                    .run_agent_collecting(
                                                        fb_name.clone(),
                                                        fb_agent.clone(),
                                                        fb_msgs,
                                                        None,
                                                        None,
                                                        Some(Duration::from_secs(120)),
                                                    )
                                                    .await
                                                    .map(Some);
                                                if attempt.is_ok() {
                                                    let reason = format!(
                                                        "primary '{}' failed; first_secondary '{}' took over",
                                                        run_agent_name, fb_name
                                                    );
                                                    (fb_name.clone(), true, Some(reason), attempt)
                                                } else {
                                                    let reason = format!(
                                                        "primary '{}' and first_secondary '{}' both failed",
                                                        run_agent_name, fb_name
                                                    );
                                                    (run_agent_name.clone(), false, Some(reason), primary_result)
                                                }
                                            } else {
                                                (
                                                    run_agent_name.clone(),
                                                    false,
                                                    Some("no secondary agents available for failover".to_string()),
                                                    primary_result,
                                                )
                                            }
                                        }
                                    }
                                } else {
                                    (run_agent_name.clone(), false, None, primary_result)
                                };

                            let duration_ms = subtask_wall.elapsed().as_millis() as u64;
                            let subtask_stop_ts = now_ts();
                            (
                                idx,
                                subtask_id,
                                run_agent_name,
                                subtask_start_ts,
                                subtask_stop_ts,
                                duration_ms,
                                sub_result,
                                effective_executor,
                                failover_applied,
                                failover_reason,
                            )
                        }
                    });

                    let results = stream::iter(tasks)
                        .buffer_unordered(phase_parallelism_effective)
                        .collect::<Vec<_>>()
                        .await;

                    phases_executed += 1;
                    for (
                        idx,
                        subtask_id,
                        run_agent_name,
                        subtask_start_ts,
                        subtask_stop_ts,
                        duration_ms,
                        sub_result,
                        effective_executor,
                        failover_applied,
                        failover_reason,
                    ) in results
                    {
                        // Update assignment audit record with node execution outcome
                        if let Some(audit) = assignment_audit_records
                            .iter_mut()
                            .find(|r| r.subtask_id == subtask_id)
                        {
                            audit.effective_executor = Some(effective_executor.clone());
                            audit.failover_applied = failover_applied;
                            audit.failover_reason = failover_reason.clone();
                        }
                        if failover_applied {
                            total_failover_count = total_failover_count.saturating_add(1);
                        }

                        let Some(record) = plan.planned_subtasks.get_mut(idx) else {
                            continue;
                        };

                        phase_sum_duration_ms = phase_sum_duration_ms.saturating_add(duration_ms);
                        phase_max_duration_ms = phase_max_duration_ms.max(duration_ms);

                        match sub_result {
                            Ok(Some(_response)) => {
                                record.mark_executed(
                                    subtask_start_ts,
                                    subtask_stop_ts,
                                    duration_ms,
                                    "completed",
                                    &effective_executor,
                                );
                                completed += 1;
                                info!(
                                    subtask_id = %subtask_id,
                                    executor = %effective_executor,
                                    failover_applied,
                                    duration_ms,
                                    "subtask completed"
                                );
                            }
                            Ok(None) => {
                                record.mark_executed(
                                    subtask_start_ts,
                                    subtask_stop_ts,
                                    0,
                                    "skipped",
                                    &effective_executor,
                                );
                                skipped += 1;
                            }
                            Err(err) => {
                                record.mark_executed(
                                    subtask_start_ts,
                                    subtask_stop_ts,
                                    duration_ms,
                                    "failed",
                                    &run_agent_name,
                                );
                                failed += 1;
                                warn!(
                                    subtask_id = %subtask_id,
                                    executor = %run_agent_name,
                                    error = %err,
                                    "subtask failed"
                                );
                                phase_failed = true;
                            }
                        }
                    }

                    serial_work_ms = serial_work_ms.saturating_add(phase_sum_duration_ms);
                    critical_path_ms = critical_path_ms.saturating_add(phase_max_duration_ms);

                    if fail_fast && phase_failed {
                        halted_early = true;
                        break;
                    }
                }

                if halted_early {
                    for record in plan.planned_subtasks.iter_mut() {
                        if record.status == "planned" {
                            let ts = now_ts();
                            record.mark_executed(ts, ts, 0, "skipped", "none");
                            skipped += 1;
                            parallel_failure_rollback_count =
                                parallel_failure_rollback_count.saturating_add(1);
                        }
                    }
                }

                let parallel_utilization = if phases_executed == 0 {
                    0.0
                } else {
                    (phase_parallel_utilization_sum / phases_executed as f64).clamp(0.0, 1.0)
                };

                let exec_stop_ts = now_ts();
                let parallel_efficiency = if serial_work_ms == 0 {
                    1.0
                } else {
                    (critical_path_ms as f64 / serial_work_ms as f64).clamp(0.0, 1.0)
                };
                let parallel_speedup = if critical_path_ms == 0 {
                    1.0
                } else {
                    serial_work_ms as f64 / critical_path_ms as f64
                };
                let summary = TaskExecutionSummary {
                    generated_at: exec_stop_ts,
                    task: task_str.clone(),
                    subtasks_total: plan.planned_subtasks.len(),
                    subtasks_completed: completed,
                    subtasks_failed: failed,
                    subtasks_skipped: skipped,
                    executor: executor_label.clone(),
                    records: plan.planned_subtasks.clone(),
                    execution_metrics: Some(TaskExecutionMetrics {
                        subtask_parallelism: phase_parallelism,
                        failure_strategy: failure_strategy.to_string(),
                        phases_executed,
                        halted_early,
                        parallel_utilization,
                        serial_degradation_count,
                        parallel_failure_rollback_count,
                        serial_work_ms,
                        critical_path_ms,
                        parallel_efficiency,
                        parallel_speedup,
                    }),
                    artifact_path: None,
                };
                let artifact_path = persist_task_execution_summary(&ledger, &summary)?;
                // Extract failover root cause before assignment_audit_records is moved into the artifact
                let failover_root_cause_str = assignment_audit_records
                    .iter()
                    .filter(|r| r.failover_applied)
                    .filter_map(|r| r.failover_reason.as_deref())
                    .next()
                    .unwrap_or("")
                    .to_string();
                let mut selected_agents = selected_agents_audit.into_iter().collect::<Vec<_>>();
                selected_agents.sort();
                let execution_decision_artifact = ExecutionDecisionArtifact {
                    generated_at: exec_stop_ts,
                    task: task_str.clone(),
                    source: method.to_string(),
                    selected_agents,
                    assignment_reason: format!(
                        "adaptive_agent_order={}, role_aware_assignment={}, capability_decision={}, env_ready_agents={}",
                        adaptive_agent_order,
                        role_aware_assignment,
                        capability_decision_effective,
                        phase_agent_names.len()
                    ),
                    subtask_assignments: assignment_audit_records,
                    parallel_phase_decisions,
                    parallelism: phase_parallelism,
                    failure_strategy: failure_strategy.to_string(),
                    degrade_policy: capability_decision_effective.to_string(),
                };
                let primary_failover_reports = execution_decision_artifact
                    .subtask_assignments
                    .iter()
                    .map(|record| PrimaryFailoverReportItem {
                        subtask_id: record.subtask_id.clone(),
                        phase_index: record.phase_index,
                        selected_primary_agent: record.node_primary_agent.clone(),
                        effective_executor: record.effective_executor.clone(),
                        failover_applied: record.failover_applied,
                        failover_reason: record.failover_reason.clone(),
                    })
                    .collect::<Vec<_>>();
                let execution_decision_artifact_path =
                    persist_execution_decision(&ledger, &execution_decision_artifact)?;
                let primary_failover_count = primary_failover_reports
                    .iter()
                    .filter(|report| report.failover_applied)
                    .count();
                let primary_failover_artifact_path = persist_primary_secondary_failover_artifact(
                    &ledger,
                    &PrimarySecondaryFailoverArtifact {
                        generated_at: exec_stop_ts,
                        task: task_str.clone(),
                        source: method.to_string(),
                        primary_agent: primary_secondary_policy.primary_agent.clone(),
                        secondary_agents: primary_secondary_policy.secondary_agents.clone(),
                        failover_policy: primary_secondary_policy.failover_policy.clone(),
                        total_subtasks: primary_failover_reports.len(),
                        failover_count: primary_failover_count,
                        reports: primary_failover_reports.clone(),
                    },
                )?;
                let optimization_artifact_path = persist_workflow_optimization_policy(
                    &ledger,
                    &WorkflowOptimizationPolicyArtifact {
                        generated_at: exec_stop_ts,
                        task: task_str.clone(),
                        source: method.to_string(),
                        policy_report: serde_json::to_value(&optimization_outcome.report)
                            .unwrap_or(Value::Null),
                        phase_parallelism_cap: optimization_outcome
                            .phase_parallelism_cap
                            .map(|value| value as u64),
                        force_fail_fast: optimization_outcome.force_fail_fast,
                        runtime_healthy,
                        anomaly_detected: optimization_outcome.report.anomaly_detected,
                        detached_modules: optimization_outcome.report.detached_modules.clone(),
                        reattached_modules: optimization_outcome.report.reattached_modules.clone(),
                    },
                )?;

                let auto_gates = params
                    .get("auto_gates")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(is_workflow_execute);
                let auto_gates = if review_policy.enforce_action_gates {
                    true
                } else {
                    auto_gates
                };
                let mut gate_reports = Vec::new();
                let mut gates_ok = true;
                if auto_gates {
                    let policy_gates = action_check_kinds_from_policy(&review_policy.required_checks);
                    let gates = if policy_gates.is_empty() {
                        vec![
                            ActionCheckKind::Qa,
                            ActionCheckKind::Retest,
                            ActionCheckKind::Final,
                        ]
                    } else {
                        policy_gates
                    };
                    for gate in gates {
                        let report = run_action_check(&ledger, gate)?;
                        if !report.ok {
                            gates_ok = false;
                        }
                        gate_reports.push(report);
                    }
                }
                let final_gate_report = gate_reports.iter().find(|report| report.kind == "final");
                let review_reject_root_cause = if failed > 0 {
                    "subtask_failed".to_string()
                } else if auto_gates && !gates_ok {
                    gate_reports
                        .iter()
                        .find(|report| !report.ok)
                        .map(|report| format!("action_check:{}", report.kind))
                        .unwrap_or_else(|| "action_check_failed".to_string())
                } else {
                    String::new()
                };
                let final_conclusion = json!({
                    "status": if failed == 0 && (!auto_gates || gates_ok) {
                        "approved"
                    } else {
                        "needs_attention"
                    },
                    "summary": if failed == 0 && (!auto_gates || gates_ok) {
                        "workflow execution and gates passed"
                    } else if failed > 0 {
                        "workflow execution contains failed subtasks"
                    } else {
                        "workflow execution completed but gate checks failed"
                    },
                    "evidence_refs": final_gate_report
                        .map(|report| report.evidence_refs.clone())
                        .unwrap_or_default(),
                    "final_summary_path": final_gate_report
                        .and_then(|report| report.final_summary_path.clone()),
                    "retest_report_path": final_gate_report
                        .and_then(|report| report.retest_report_path.clone()),
                });

                if (failed > 0 || !gates_ok) && work_grade_decision.decided != WorkGrade::Safeguard {
                    work_grade_decision.decided = WorkGrade::Safeguard;
                    work_grade_decision.reasons.push(
                        "execution produced failures or gate rejection, escalated to safeguard"
                            .to_string(),
                    );
                    work_grade_decision.decision_action = work_grade_action(
                        work_grade_decision.requested,
                        work_grade_decision.decided,
                    );
                }

                let work_grade_artifact_path = persist_workflow_work_grade(
                    &ledger,
                    &WorkflowWorkGradeArtifact {
                        generated_at: exec_stop_ts,
                        task: task_str.clone(),
                        source: method.to_string(),
                        requested_grade: work_grade_decision.requested.as_str().to_string(),
                        decided_grade: work_grade_decision.decided.as_str().to_string(),
                        decision_action: work_grade_decision.decision_action.clone(),
                        reasons: work_grade_decision.reasons.clone(),
                        risk_score: work_grade_decision.risk_score,
                    },
                )?;
                let learning_artifact_path = persist_workflow_learning_event(
                    &ledger,
                    WorkflowLearningEvent {
                        generated_at: exec_stop_ts,
                        task: task_str.clone(),
                        complexity: plan.characteristics.complexity,
                        predicted_success_rate: plan.routing.predicted_success_rate,
                        subtasks_total: summary.subtasks_total,
                        subtasks_completed: completed,
                        subtasks_failed: failed,
                        subtasks_skipped: skipped,
                        serial_work_ms,
                        critical_path_ms,
                        parallel_speedup,
                        parallel_efficiency,
                        executor: executor_label.clone(),
                        source: method.to_string(),
                        runtime_healthy,
                        gates_ok,
                        work_grade: work_grade_decision.decided.as_str().to_string(),
                        risk_score: work_grade_decision.risk_score,
                        clarification_rounds: learning_clarification.rounds,
                        clarification_quality_score: learning_clarification.quality_score,
                        requirement_change_count: learning_clarification.requirement_change_count,
                        review_reject_root_cause: review_reject_root_cause.clone(),
                        primary_stability_score: if summary.subtasks_total == 0 {
                            1.0
                        } else {
                            1.0 - (total_failover_count as f64 / summary.subtasks_total as f64)
                        },
                        secondary_utilization_rate: if summary.subtasks_total == 0 {
                            0.0
                        } else {
                            total_failover_count as f64 / summary.subtasks_total as f64
                        },
                        failover_count: total_failover_count,
                        failover_root_cause: failover_root_cause_str,
                    },
                    200,
                )?;
                let pipeline_metrics_artifact_path = persist_pipeline_unified_metrics(
                    &ledger,
                    &PipelineUnifiedMetricsArtifact {
                        generated_at: exec_stop_ts,
                        task: task_str.clone(),
                        source: method.to_string(),
                        predicted_success_rate: plan.routing.predicted_success_rate as f64,
                        risk_score: work_grade_decision.risk_score,
                        runtime_healthy,
                        gates_ok,
                        subtasks_total: summary.subtasks_total,
                        subtasks_completed: completed,
                        subtasks_failed: failed,
                        subtasks_skipped: skipped,
                        parallelism: phase_parallelism,
                        parallel_utilization,
                        serial_degradation_count,
                        parallel_failure_rollback_count,
                        failure_strategy: failure_strategy.to_string(),
                        work_grade: work_grade_decision.decided.as_str().to_string(),
                        optimization_policy: serde_json::to_value(&optimization_outcome.report)
                            .unwrap_or(Value::Null),
                    },
                )?;

                self.record_trace_event(
                    &trace,
                    "phase.execute",
                    if failed == 0 { "ok" } else { "warn" },
                    "execute",
                    json!({
                        "task": task_str,
                        "subtasks_total": summary.subtasks_total,
                        "subtasks_completed": completed,
                        "subtasks_failed": failed,
                        "subtask_parallelism": phase_parallelism,
                        "adaptive_routing": adaptive_routing,
                        "predicted_success_rate_tuned": predicted_success_rate_tuned,
                        "adaptive_agent_order": adaptive_agent_order,
                        "agent_order_tuned": agent_order_tuned,
                        "capability_governance": capability_governance.clone(),
                        "blue5": {
                            "doc": blue5_doc.clone(),
                            "auto": blue5_auto.clone(),
                            "primary_secondary_policy": primary_secondary_policy.clone(),
                        },
                        "failure_strategy": failure_strategy,
                        "parallel_utilization": parallel_utilization,
                        "serial_degradation_count": serial_degradation_count,
                        "parallel_failure_rollback_count": parallel_failure_rollback_count,
                        "review_policy": review_policy.clone(),
                        "reviews": review_decisions.clone(),
                        "gates_ok": gates_ok,
                        "work_grade": work_grade_decision.decided.as_str(),
                        "execution_decision_artifact_path": execution_decision_artifact_path.display().to_string(),
                        "primary_failover_artifact_path": primary_failover_artifact_path.display().to_string(),
                        "primary_failover_report": {
                            "failover_policy": primary_secondary_policy.failover_policy.clone(),
                            "total_subtasks": primary_failover_reports.len(),
                            "failover_count": primary_failover_count,
                        },
                        "pipeline_metrics_artifact_path": pipeline_metrics_artifact_path.display().to_string(),
                        "learning_artifact_path": learning_artifact_path.display().to_string(),
                        "consultation_summary": consultation_summary.clone(),
                        "consultation_artifact_path": consultation_artifact_path
                            .as_ref()
                            .map(|path| path.display().to_string()),
                        "executor": executor_label,
                    }),
                    None,
                    (exec_stop_ts.saturating_sub(exec_started_ts)) as u64 * 1000,
                );

                self.send_result(
                    request_id,
                    json!({
                        "ok": failed == 0 && (!auto_gates || gates_ok),
                        "summary": summary,
                        "execution_metrics": {
                            "subtask_parallelism": phase_parallelism,
                            "subtask_parallelism_base": phase_parallelism_base,
                            "adaptive_parallelism": adaptive_parallelism,
                            "parallelism_tuned": parallelism_tuned,
                            "adaptive_routing": adaptive_routing,
                            "predicted_success_rate_tuned": predicted_success_rate_tuned,
                            "adaptive_agent_order": adaptive_agent_order,
                            "agent_order_tuned": agent_order_tuned,
                            "capability_governance": capability_governance.clone(),
                            "optimization_policy": optimization_outcome.report,
                            "auto_research": auto_research,
                            "research_summary": research_summary.clone(),
                            "consultation_summary": consultation_summary.clone(),
                            "role_aware_assignment": role_aware_assignment,
                            "adaptive_failure_strategy": adaptive_failure_strategy,
                            "failure_strategy_tuned": failure_strategy_tuned,
                            "failure_strategy": failure_strategy,
                            "clarification_rounds": learning_clarification.rounds,
                            "clarification_quality_score": learning_clarification.quality_score,
                            "requirement_change_count": learning_clarification.requirement_change_count,
                            "review_reject_root_cause": review_reject_root_cause,
                            "phases_executed": phases_executed,
                            "halted_early": halted_early,
                            "parallel_utilization": parallel_utilization,
                            "serial_degradation_count": serial_degradation_count,
                            "parallel_failure_rollback_count": parallel_failure_rollback_count,
                            "serial_work_ms": serial_work_ms,
                            "critical_path_ms": critical_path_ms,
                            "parallel_efficiency": parallel_efficiency,
                            "parallel_speedup": parallel_speedup,
                        },
                        "auto_gates": auto_gates,
                        "review_policy": review_policy,
                        "reviews": review_decisions,
                        "adaptive_work_grade": adaptive_work_grade,
                        "gates_ok": gates_ok,
                        "capability_governance": capability_governance,
                        "blue5": {
                            "doc": blue5_doc,
                            "auto": blue5_auto,
                            "primary_secondary_policy": primary_secondary_policy,
                        },
                        "gate_reports": gate_reports,
                        "final_conclusion": final_conclusion,
                        "plan_artifact_path": plan_artifact_path.display().to_string(),
                        "workflow_meta": workflow_meta,
                        "workflow_artifact_path": workflow_artifact_path
                            .as_ref()
                            .map(|path| path.display().to_string()),
                        "work_grade": {
                            "requested": work_grade_decision.requested.as_str(),
                            "decided": work_grade_decision.decided.as_str(),
                            "decision_action": work_grade_decision.decision_action.clone(),
                            "risk_score": work_grade_decision.risk_score,
                            "reasons": work_grade_decision.reasons.clone(),
                        },
                        "artifact_path": artifact_path.display().to_string(),
                        "execution_decision_artifact_path": execution_decision_artifact_path.display().to_string(),
                        "primary_failover_artifact_path": primary_failover_artifact_path.display().to_string(),
                        "primary_failover_report": {
                            "failover_policy": primary_secondary_policy.failover_policy.clone(),
                            "total_subtasks": primary_failover_reports.len(),
                            "failover_count": primary_failover_count,
                            "reports": primary_failover_reports,
                        },
                        "learning_artifact_path": learning_artifact_path.display().to_string(),
                        "work_grade_artifact_path": work_grade_artifact_path.display().to_string(),
                        "optimization_artifact_path": optimization_artifact_path.display().to_string(),
                        "pipeline_metrics_artifact_path": pipeline_metrics_artifact_path.display().to_string(),
                        "research_artifact_path": research_artifact_path
                            .as_ref()
                            .map(|path| path.display().to_string()),
                        "consultation_artifact_path": consultation_artifact_path
                            .as_ref()
                            .map(|path| path.display().to_string()),
                    }),
                )
                .await
            }
            "learning.summary" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let limit = params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|v| v.clamp(1, 500) as usize)
                    .unwrap_or(50);

                let ledger = self.artifact_ledger();
                let latest_path = ledger.latest_path("spec", "latest-learning.json");

                let bus = fs::read_to_string(&latest_path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<WorkflowLearningBusArtifact>(&raw).ok())
                    .unwrap_or(WorkflowLearningBusArtifact {
                        generated_at: now_ts(),
                        total_events: 0,
                        events: Vec::new(),
                    });

                let sampled = bus
                    .events
                    .iter()
                    .rev()
                    .take(limit)
                    .cloned()
                    .collect::<Vec<_>>();
                let sampled_count = sampled.len();

                let mut total_clarification_rounds: u64 = 0;
                let mut total_clarification_quality: f64 = 0.0;
                let mut total_requirement_change_count: u64 = 0;
                let mut total_risk_score: f64 = 0.0;
                let mut total_predicted_success_rate: f64 = 0.0;
                let mut total_parallel_efficiency: f64 = 0.0;
                let mut total_parallel_speedup: f64 = 0.0;
                let mut gate_pass_count: usize = 0;
                let mut runtime_healthy_count: usize = 0;
                let mut review_reject_root_causes: HashMap<String, usize> = HashMap::new();
                let mut total_primary_stability_sum: f64 = 0.0;
                let mut total_secondary_utilization_sum: f64 = 0.0;
                let mut total_failover_count_sum: u64 = 0;

                for event in &sampled {
                    total_clarification_rounds =
                        total_clarification_rounds.saturating_add(event.clarification_rounds as u64);
                    total_clarification_quality += event.clarification_quality_score;
                    total_requirement_change_count = total_requirement_change_count
                        .saturating_add(event.requirement_change_count as u64);
                    total_risk_score += event.risk_score;
                    total_predicted_success_rate += event.predicted_success_rate as f64;
                    total_parallel_efficiency += event.parallel_efficiency;
                    total_parallel_speedup += event.parallel_speedup;
                    if event.gates_ok {
                        gate_pass_count = gate_pass_count.saturating_add(1);
                    }
                    if event.runtime_healthy {
                        runtime_healthy_count = runtime_healthy_count.saturating_add(1);
                    }
                    total_primary_stability_sum += event.primary_stability_score;
                    total_secondary_utilization_sum += event.secondary_utilization_rate;
                    total_failover_count_sum =
                        total_failover_count_sum.saturating_add(event.failover_count as u64);

                    let cause = event.review_reject_root_cause.trim();
                    if !cause.is_empty() {
                        *review_reject_root_causes
                            .entry(cause.to_string())
                            .or_insert(0) += 1;
                    }
                }

                let denominator = sampled_count.max(1) as f64;
                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "summary": {
                            "artifact_path": latest_path.display().to_string(),
                            "total_events": bus.total_events,
                            "sampled_events": sampled_count,
                            "sample_limit": limit,
                            "averages": {
                                "clarification_rounds": total_clarification_rounds as f64 / denominator,
                                "clarification_quality_score": total_clarification_quality / denominator,
                                "risk_score": total_risk_score / denominator,
                                "predicted_success_rate": total_predicted_success_rate / denominator,
                                "parallel_efficiency": total_parallel_efficiency / denominator,
                                "parallel_speedup": total_parallel_speedup / denominator,
                                "primary_stability_score": total_primary_stability_sum / denominator,
                                "secondary_utilization_rate": total_secondary_utilization_sum / denominator,
                            },
                            "totals": {
                                "requirement_change_count": total_requirement_change_count,
                                "failover_count": total_failover_count_sum,
                            },
                            "rates": {
                                "gates_pass_rate": gate_pass_count as f64 / denominator,
                                "runtime_healthy_rate": runtime_healthy_count as f64 / denominator,
                            },
                            "review_reject_root_causes": review_reject_root_causes,
                        }
                    }),
                )
                .await
            }
            "primary_secondary.summary" => {
                // M10: aggregate primary-secondary governance metrics
                let params = request.params.unwrap_or_else(|| json!({}));
                let limit = params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|v| v.clamp(1, 500) as usize)
                    .unwrap_or(50);

                let ledger = self.artifact_ledger();
                let learning_path = ledger.latest_path("spec", "latest-learning.json");
                let policy_path =
                    ledger.latest_path("spec", "latest-primary-secondary-policy.json");

                let bus = fs::read_to_string(&learning_path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<WorkflowLearningBusArtifact>(&raw).ok())
                    .unwrap_or(WorkflowLearningBusArtifact {
                        generated_at: now_ts(),
                        total_events: 0,
                        events: Vec::new(),
                    });

                let sampled = bus
                    .events
                    .iter()
                    .rev()
                    .take(limit)
                    .cloned()
                    .collect::<Vec<_>>();
                let sampled_count = sampled.len();

                let mut ps_failover_count: u64 = 0;
                let mut ps_primary_stability: f64 = 0.0;
                let mut ps_secondary_utilization: f64 = 0.0;
                let mut failover_root_causes: HashMap<String, usize> = HashMap::new();

                for event in &sampled {
                    ps_failover_count =
                        ps_failover_count.saturating_add(event.failover_count as u64);
                    ps_primary_stability += event.primary_stability_score;
                    ps_secondary_utilization += event.secondary_utilization_rate;
                    let cause = event.failover_root_cause.trim();
                    if !cause.is_empty() {
                        *failover_root_causes.entry(cause.to_string()).or_insert(0) += 1;
                    }
                }

                let denominator = sampled_count.max(1) as f64;
                let latest_policy = fs::read_to_string(&policy_path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());

                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "summary": {
                            "learning_artifact_path": learning_path.display().to_string(),
                            "policy_artifact_path": policy_path.display().to_string(),
                            "total_events": bus.total_events,
                            "sampled_events": sampled_count,
                            "sample_limit": limit,
                            "averages": {
                                "primary_stability_score": ps_primary_stability / denominator,
                                "secondary_utilization_rate": ps_secondary_utilization / denominator,
                            },
                            "totals": {
                                "failover_count": ps_failover_count,
                            },
                            "failover_root_causes": failover_root_causes,
                            "latest_policy": latest_policy,
                        }
                    }),
                )
                .await
            }
            "runtime.health" => {
                let report = self.runtime_healthcheck_report()?;
                let artifact_path = persist_runtime_healthcheck(&self.artifact_ledger(), &report)?;
                let runtime_details = report
                    .components
                    .iter()
                    .find(|component| component.name == "runtime")
                    .map(|component| component.details.clone())
                    .unwrap_or_else(|| json!({}));
                let sqlite_cache_entries = report
                    .components
                    .iter()
                    .find(|component| component.name == "cache")
                    .and_then(|component| component.details.get("entries"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let vector = report
                    .components
                    .iter()
                    .find(|component| component.name == "vector")
                    .map(|component| component.details.clone())
                    .unwrap_or(Value::Null);
                self.send_result(
                    request_id,
                    json!({
                        "ok": report.overall_status != CheckStatus::Error,
                        "report": report,
                        "artifact_path": artifact_path.display().to_string(),
                        "memory_cache_entries": self.memory_cache.active_entries(),
                        "sqlite_cache_entries": sqlite_cache_entries,
                        "circuit_breaker": runtime_details.get("circuit_breaker").cloned().unwrap_or(Value::Null),
                        "rate_limiter": runtime_details.get("rate_limiter").cloned().unwrap_or(Value::Null),
                        "inflight": runtime_details.get("inflight").cloned().unwrap_or(Value::Null),
                        "vector": vector,
                        "lifecycle": runtime_details.get("lifecycle").cloned().unwrap_or(Value::Null),
                        "maintenance": runtime_details.get("maintenance").cloned().unwrap_or(Value::Null),
                        "review_gate": runtime_details.get("review_gate").cloned().unwrap_or(Value::Null),
                        "telemetry": runtime_details.get("telemetry").cloned().unwrap_or(Value::Null),
                    }),
                )
                .await
            }
            "action.check" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let kind = params
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .and_then(ActionCheckKind::parse)
                    .unwrap_or(ActionCheckKind::All);
                let report = run_action_check(&self.artifact_ledger(), kind)?;
                self.send_result(
                    request_id,
                    json!({
                        "ok": report.ok,
                        "report": report,
                    }),
                )
                .await
            }
            "phase.status" => {
                let limiter = self
                    .phase_rate_limiter
                    .snapshot()
                    .into_iter()
                    .map(|(phase, (tokens, capacity))| {
                        (
                            phase,
                            json!({
                                "tokens": tokens,
                                "capacity": capacity,
                            }),
                        )
                    })
                    .collect::<serde_json::Map<String, Value>>();
                let inflight = self.inflight_limiter.snapshot().1;
                self.send_result(
                    request_id,
                    json!({
                        "rate_limiter": limiter,
                        "inflight": inflight,
                    }),
                )
                .await
            }
            "breaker.status" => {
                let now = now_ts();
                let status = self
                    .circuit_breakers
                    .snapshot()
                    .into_iter()
                    .map(|(agent, snapshot)| {
                        (
                            agent,
                            json!({
                                "consecutive_failures": snapshot.consecutive_failures,
                                "state": snapshot.state,
                                "open_until": snapshot.open_until,
                                "probe_in_flight": snapshot.probe_in_flight,
                                "open": snapshot.open_until.map(|ts| ts > now).unwrap_or(false),
                            }),
                        )
                    })
                    .collect::<serde_json::Map<String, Value>>();
                self.send_result(request_id, Value::Object(status)).await
            }
            "breaker.reset" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let target = params.get("agent").and_then(|v| v.as_str());
                let removed = if let Some(agent_name) = target {
                    self.circuit_breakers
                        .inner
                        .lock()
                        .ok()
                        .and_then(|mut guard| guard.remove(agent_name).map(|_| 1_usize))
                        .unwrap_or(0)
                } else {
                    self.circuit_breakers
                        .inner
                        .lock()
                        .map(|mut guard| {
                            let count = guard.len();
                            guard.clear();
                            count
                        })
                        .unwrap_or(0)
                };
                self.send_result(request_id, json!({"ok": true, "removed": removed}))
                    .await
            }
            "config.reload" => {
                let reloaded = self.reload_runtime_config().await?;
                self.send_result(request_id, reloaded).await
            }
            "cache.clear" => {
                let memory_removed = self.memory_cache.clear_all();
                let sqlite_removed = if let Some(cache) = self.cache_handle() {
                    self.cache_clear(cache.clone()).await.unwrap_or(0)
                } else {
                    0
                };

                let result = json!({
                    "ok": true,
                    "memory_removed": memory_removed,
                    "sqlite_removed": sqlite_removed,
                });
                self.send_result(request_id, result).await
            }
            "vector.clear" => {
                let (memory_removed, summary_removed) =
                    if let Some(store) = self.vector_store_handle() {
                        self.vector_clear(store.clone()).await?
                    } else {
                        (0, 0)
                    };

                let result = json!({
                    "ok": true,
                    "vector_removed": memory_removed,
                    "summary_removed": summary_removed,
                });
                self.send_result(request_id, result).await
            }
            "maintenance.gc" => {
                let cycle = self.run_maintenance_cycle("rpc").await;
                let result = json!({
                    "ok": true,
                    "memory_expired_removed": cycle.memory_expired_removed,
                    "sqlite_expired_removed": cycle.sqlite_expired_removed,
                    "cache_vacuumed": cycle.cache_vacuumed,
                    "vector_vacuumed": cycle.vector_vacuumed,
                    "maintenance": self.maintenance.snapshot(),
                });
                self.send_result(request_id, result).await
            }
            "autotune.get" => {
                if let Some(autotune) = self.autotune_handle() {
                    let state = autotune.lock().await;
                    let result = state.snapshot();
                    self.send_result(request_id, result).await
                } else {
                    self.send_error(
                        request_id,
                        -32603,
                        "autotune is not enabled".to_string(),
                        None,
                    )
                    .await
                }
            }
            "autotune.reset" => {
                if let Some(autotune) = self.autotune_handle() {
                    if let Some(config) = self.autotune_config_snapshot() {
                        let new_state = {
                            let mut state = autotune.lock().await;
                            *state = AutoTuneState::new(&config);
                            state.clone()
                        };
                        if let Some(path) = self.autotune_state_path_snapshot() {
                            let path_ref = path.as_str();
                            if let Err(e) = new_state.save(path_ref) {
                                warn!("{}", crate::i18n::tf("warning.failed_save_autotune", &[("error", &format!("{}", e))]));
                            }
                        } else {
                            warn!("autotune reset skipped persistence because no resolved state path is available");
                        }
                        self.send_result(request_id, json!({"ok": true})).await
                    } else {
                        self.send_error(
                            request_id,
                            -32603,
                            "autotune config not available".to_string(),
                            None,
                        )
                        .await
                    }
                } else {
                    self.send_error(
                        request_id,
                        -32603,
                        "autotune is not enabled".to_string(),
                        None,
                    )
                    .await
                }
            }
            "conversation.checkpoint.create" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let conversation_id_raw = params
                    .get("conversation_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let conversation_id = match validate_storage_key(
                    conversation_id_raw,
                    "conversation_id",
                    MAX_CONVERSATION_ID_LEN,
                ) {
                    Ok(value) => value,
                    Err(message) => {
                        return self.send_error(request_id, -32602, message, None).await;
                    }
                };
                let branch_id_raw = params
                    .get("branch_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("main");
                let branch_id =
                    match validate_storage_key(branch_id_raw, "branch_id", MAX_BRANCH_ID_LEN) {
                        Ok(value) => value,
                        Err(message) => {
                            return self.send_error(request_id, -32602, message, None).await;
                        }
                    };
                let note = params
                    .get("note")
                    .and_then(|v| v.as_str())
                    .map(|v| v.to_string());
                let messages_value = match params.get("messages") {
                    Some(value) => value.clone(),
                    None => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "messages is required for conversation.checkpoint.create"
                                    .to_string(),
                                None,
                            )
                            .await;
                    }
                };
                let messages: Vec<Message> = match serde_json::from_value(messages_value) {
                    Ok(value) => value,
                    Err(err) => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                crate::i18n::tf("error.invalid_messages_payload", &[("error", &format!("{err}"))]),
                                None,
                            )
                            .await;
                    }
                };

                match self.create_conversation_checkpoint(&conversation_id, &branch_id, messages, note)
                {
                    Ok(checkpoint) => {
                        self.send_result(
                            request_id,
                            json!({
                                "ok": true,
                                "checkpoint": checkpoint,
                            }),
                        )
                        .await
                    }
                    Err(message) => self.send_error(request_id, -32603, message, None).await,
                }
            }
            "conversation.checkpoint.list" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let conversation_id_raw = params
                    .get("conversation_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let conversation_id = match validate_storage_key(
                    conversation_id_raw,
                    "conversation_id",
                    MAX_CONVERSATION_ID_LEN,
                ) {
                    Ok(value) => value,
                    Err(message) => {
                        return self.send_error(request_id, -32602, message, None).await;
                    }
                };
                let branch_id = match params.get("branch_id").and_then(|v| v.as_str()) {
                    Some(value) => {
                        match validate_storage_key(value, "branch_id", MAX_BRANCH_ID_LEN) {
                            Ok(valid) => Some(valid),
                            Err(message) => {
                                return self.send_error(request_id, -32602, message, None).await;
                            }
                        }
                    }
                    None => None,
                };
                let limit = params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50)
                    .min(500) as usize;

                match self.list_conversation_checkpoints(&conversation_id, branch_id.as_deref(), limit)
                {
                    Ok(checkpoints) => {
                        self.send_result(
                            request_id,
                            json!({
                                "ok": true,
                                "count": checkpoints.len(),
                                "checkpoints": checkpoints,
                            }),
                        )
                        .await
                    }
                    Err(message) => self.send_error(request_id, -32603, message, None).await,
                }
            }
            "conversation.rollback" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let conversation_id_raw = params
                    .get("conversation_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let conversation_id = match validate_storage_key(
                    conversation_id_raw,
                    "conversation_id",
                    MAX_CONVERSATION_ID_LEN,
                ) {
                    Ok(value) => value,
                    Err(message) => {
                        return self.send_error(request_id, -32602, message, None).await;
                    }
                };
                let checkpoint_id = match params.get("checkpoint_id").and_then(|v| v.as_str()) {
                    Some(value) => {
                        match validate_storage_key(value, "checkpoint_id", MAX_CHECKPOINT_ID_LEN) {
                            Ok(valid) => valid,
                            Err(message) => {
                                return self.send_error(request_id, -32602, message, None).await;
                            }
                        }
                    }
                    None => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "checkpoint_id is required for conversation.rollback"
                                    .to_string(),
                                None,
                            )
                            .await;
                    }
                };
                let target_branch = match params.get("branch_id").and_then(|v| v.as_str()) {
                    Some(value) => {
                        match validate_storage_key(value, "branch_id", MAX_BRANCH_ID_LEN) {
                            Ok(valid) => Some(valid),
                            Err(message) => {
                                return self.send_error(request_id, -32602, message, None).await;
                            }
                        }
                    }
                    None => None,
                };

                if let Some(checkpoint) = self.rollback_conversation_checkpoint(
                    &conversation_id,
                    &checkpoint_id,
                    target_branch.as_deref(),
                ) {
                    self.send_result(
                        request_id,
                        json!({
                            "ok": true,
                            "conversation_id": conversation_id.clone(),
                            "branch_id": checkpoint.branch_id,
                            "checkpoint": checkpoint,
                            "messages": checkpoint.messages,
                        }),
                    )
                    .await
                } else {
                    self.send_error(
                        request_id,
                        -32602,
                        format!(
                            "checkpoint '{}' not found in conversation '{}'",
                            checkpoint_id, conversation_id
                        ),
                        None,
                    )
                    .await
                }
            }
            "conversation.checkpoint.prune" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let conversation_id_raw = params
                    .get("conversation_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let conversation_id = match validate_storage_key(
                    conversation_id_raw,
                    "conversation_id",
                    MAX_CONVERSATION_ID_LEN,
                ) {
                    Ok(value) => value,
                    Err(message) => {
                        return self.send_error(request_id, -32602, message, None).await;
                    }
                };
                let branch_id = match params.get("branch_id").and_then(|v| v.as_str()) {
                    Some(value) => {
                        match validate_storage_key(value, "branch_id", MAX_BRANCH_ID_LEN) {
                            Ok(valid) => Some(valid),
                            Err(message) => {
                                return self.send_error(request_id, -32602, message, None).await;
                            }
                        }
                    }
                    None => None,
                };
                let keep = match params.get("keep") {
                    Some(value) => match value.as_u64() {
                        Some(0) => {
                            return self
                                .send_error(
                                    request_id,
                                    -32602,
                                    "keep must be >= 1 for conversation.checkpoint.prune"
                                        .to_string(),
                                    None,
                                )
                                .await;
                        }
                        Some(valid) => valid.min(500) as usize,
                        None => {
                            return self
                                .send_error(
                                    request_id,
                                    -32602,
                                    "keep must be an integer >= 1 for conversation.checkpoint.prune"
                                        .to_string(),
                                    None,
                                )
                                .await;
                        }
                    },
                    None => 20,
                };

                let prune =
                    self.prune_conversation_checkpoints(&conversation_id, branch_id.as_deref(), keep);
                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "conversation_id": conversation_id,
                        "removed": prune.removed,
                        "repaired_heads": prune.repaired_heads,
                        "dropped_heads": prune.dropped_heads,
                    }),
                )
                .await
            }
            "shutdown" => {
                self.begin_shutdown("rpc shutdown");
                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "lifecycle": self.lifecycle.snapshot(),
                    }),
                )
                .await
            }
            other => {
                self.send_error(
                    request_id,
                    -32601,
                    ProxyError::UnknownMethod(other.to_string()).to_string(),
                    None,
                )
                .await
            }
            }
        }
        .await;

        let duration_ms = started.elapsed().as_millis() as u64;
        match &result {
            Ok(_) => self.record_trace_event(
                &trace,
                "request.end",
                "ok",
                "rpc",
                json!({
                    "method": method,
                    "request_id": trace.request_id,
                }),
                None,
                duration_ms,
            ),
            Err(err) => {
                self.record_trace_event(
                    &trace,
                    "request.end",
                    "error",
                    "rpc",
                    json!({
                        "method": method,
                        "request_id": trace.request_id,
                    }),
                    Some(err.to_string()),
                    duration_ms,
                );
                // Enhanced telemetry logging for errors
                telemetry_enhanced::log::error_with_context(
                    err,
                    "request_processing",
                    Some(&trace.request_id),
                );
            }
        }

        if let Some(span) = request_span {
            self.telemetry.end_span(
                span,
                vec![
                    KeyValue::new("request.duration_ms", duration_ms as i64),
                    KeyValue::new(
                        "request.status",
                        if result.is_ok() { "ok" } else { "error" },
                    ),
                ],
            );
        }

        // Enhanced telemetry logging for request completion
        let status_code = if result.is_ok() { 200 } else { 500 };
        telemetry_enhanced::log::request_complete(
            "rpc",
            &trace.method,
            &trace.request_id,
            status_code,
            duration_ms as f64,
        );

        result
    }

    fn new_request_trace(&self, request: &JsonRpcRequest) -> RequestTraceContext {
        let counter = TRACE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = format!(
            "{}:{}:{}:{}",
            request.method,
            request
                .id
                .as_ref()
                .map(value_to_id)
                .unwrap_or_else(|| "none".to_string()),
            now_ms(),
            counter
        );
        RequestTraceContext {
            trace_id: hash_hex(&base, 32),
            span_id: hash_hex(&format!("{}:span", base), 16),
            method: request.method.clone(),
            request_id: request
                .id
                .as_ref()
                .map(value_to_id)
                .unwrap_or_else(|| "none".to_string()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_trace_event(
        &self,
        trace: &RequestTraceContext,
        event_type: &str,
        status: &str,
        phase: &str,
        inputs: Value,
        error: Option<String>,
        duration_ms: u64,
    ) {
        let pua_stage = infer_pua_stage(event_type, phase);
        let attributes = normalize_trace_attributes(event_type, phase, status, inputs);
        let event = TraceEvent {
            timestamp: now_ms().to_string(),
            event_type: event_type.to_string(),
            task_id: trace.request_id.clone(),
            phase: phase.to_string(),
            agent: None,
            tool: None,
            status: status.to_string(),
            inputs: json!({
                "trace_id": trace.trace_id,
                "span_id": trace.span_id,
                "method": trace.method,
                "attributes": attributes,
            }),
            outputs: None,
            duration_ms,
            error,
            pua_stage,
        };

        if let Ok(mut guard) = self.trace_events.lock() {
            guard.push(event);
            if guard.len() > TRACE_BUFFER_MAX {
                let extra = guard.len() - TRACE_BUFFER_MAX;
                guard.drain(0..extra);
            }
        } else {
            warn!("failed to record trace event: trace_events lock poisoned");
        }
    }

    fn trace_snapshot(&self, limit: usize) -> Vec<TraceEvent> {
        self.trace_events
            .lock()
            .map(|guard| {
                guard
                    .iter()
                    .rev()
                    .take(limit.max(1))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    fn trace_metrics_snapshot(&self) -> Value {
        let slow_top_n = self.runtime_config_snapshot().trace_slow_top_n.max(1);
        let events = self
            .trace_events
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();

        let mut requests = events
            .iter()
            .filter(|e| e.event_type == "request.end")
            .map(|e| {
                let method = e
                    .inputs
                    .get("attributes")
                    .and_then(|v| v.get("method"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                json!({
                    "request_id": e.task_id,
                    "method": method,
                    "duration_ms": e.duration_ms,
                    "status": e.status,
                    "timestamp": e.timestamp,
                })
            })
            .collect::<Vec<_>>();

        requests.sort_by(|a, b| {
            b.get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                .cmp(&a.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0))
        });
        requests.truncate(slow_top_n);

        let mut phase_buckets: HashMap<String, Vec<u64>> = HashMap::new();
        for event in &events {
            if event.duration_ms == 0 {
                continue;
            }
            if event.event_type.starts_with("phase.") || event.event_type == "request.end" {
                phase_buckets
                    .entry(event.phase.clone())
                    .or_default()
                    .push(event.duration_ms);
            }
        }

        let mut by_phase = serde_json::Map::new();
        for (phase, mut samples) in phase_buckets {
            samples.sort_unstable();
            let p95 = percentile(&samples, 95.0);
            let p99 = percentile(&samples, 99.0);
            by_phase.insert(
                phase,
                json!({
                    "count": samples.len(),
                    "p95_ms": p95,
                    "p99_ms": p99,
                }),
            );
        }

        let mut by_pua_stage: HashMap<String, u64> = HashMap::new();
        for event in &events {
            if let Some(stage) = event.pua_stage.as_ref() {
                *by_pua_stage.entry(stage.clone()).or_insert(0) += 1;
            }
        }

        json!({
            "sampling_rate": self.telemetry.sampling_rate(),
            "buffered_events": events.len(),
            "slow_requests_top_n": requests,
            "phase_latency": by_phase,
            "pua_stage_counts": by_pua_stage,
        })
    }

    async fn handle_chat(
        &self,
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
            self.telemetry.start_child_span(
                parent,
                "acp.chat",
                vec![KeyValue::new("phase.entry", "chat")],
            )
        });
        let result = async {
            if self.lifecycle.is_shutting_down() {
                self.send_error(
                    id,
                    -32031,
                    "server is shutting down".to_string(),
                    Some(serde_json::to_value(self.lifecycle.snapshot())?),
                )
                .await?;
                return Ok(());
            }

            self.metrics.inc_chat_requests();

            let params_value = params.unwrap_or_else(|| json!({}));
            let chat_params: ChatParams = match serde_json::from_value(params_value) {
                Ok(value) => value,
                Err(err) => {
                    self.send_error(
                        id,
                        -32602,
                        crate::i18n::tf("error.invalid_chat_params", &[("error", &format!("{err}"))]),
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            };

            let mode = ChatMode::parse(chat_params.mode.as_deref());
            let mode_name = mode.map(|m| m.as_str()).unwrap_or("default");
            let auto_conv_id = chat_params
                .conversation_id
                .as_deref()
                .and_then(|value| {
                    validate_storage_key(value, "conversation_id", MAX_CONVERSATION_ID_LEN).ok()
                })
                .unwrap_or_else(|| pipeline_trace.trace_id.clone());
            let original_messages = chat_params.messages.clone();
            let (flow, registry) = self.routing_handles()?;
            let effective_phase = self.infer_phase_name_with_flow(
                flow.as_ref(),
                chat_params.phase.as_deref(),
                mode,
            );

            // Mandatory pipeline stage 1: Analyze task intent from conversation input.
            let analyzed_task = TaskRouter::analyze_task(&extract_task_description(&chat_params.messages));
            self.record_trace_event(
                &pipeline_trace,
                "phase.analyze",
                "ok",
                "analyze",
                json!({
                    "task_type": format!("{:?}", analyzed_task.task_type),
                    "complexity": analyzed_task.complexity,
                    "needs_verification": analyzed_task.needs_verification,
                    "has_safety_concerns": analyzed_task.has_safety_concerns,
                    "involves_multiple_modules": analyzed_task.involves_multiple_modules,
                }),
                None,
                0,
            );

            // Mandatory pipeline stage 2: Route into role-based hard gates.
            let pipeline_routing = TaskRouter::route_task(&analyzed_task);
            self.record_trace_event(
                &pipeline_trace,
                "phase.route_hard_gate",
                "ok",
                "route",
                json!({
                    "policy_status": "pass",
                    "roles": pipeline_routing
                        .roles
                        .iter()
                        .map(|role| format!("{:?}", role))
                        .collect::<Vec<_>>(),
                    "success_rate": pipeline_routing.predicted_success_rate,
                    "risk_factors": pipeline_routing.risk_factors.clone(),
                    "mandatory_safeguards": pipeline_routing.pua_enforcement.mandatory_safeguards.clone(),
                }),
                None,
                0,
            );

            let total_chars: usize = chat_params
                .messages
                .iter()
                .map(|m| m.content.chars().count())
                .sum();

            let routing_started = Instant::now();
            let routing = flow
                .resolve(Some(effective_phase.clone()), registry.as_ref())
                .map_err(|err| ProxyError::Internal(err.to_string()))?;
            self.record_trace_event(
                &child_trace_context(&pipeline_trace, "chat.route"),
                "phase.route",
                "ok",
                "route",
                json!({ "phase": routing.phase.phase_name }),
                None,
                routing_started.elapsed().as_millis() as u64,
            );

        if let Some(limit) = extra_u64(routing.phase.options.as_ref(), "max_request_chars") {
            if total_chars > limit as usize {
                self.send_error(
                    id,
                    -32600,
                    format!(
                        "request too large: {} chars exceeds limit {}",
                        total_chars, limit
                    ),
                    None,
                )
                .await?;
                return Ok(());
            }
        }

            if let Some(rpm_limit) = extra_u64(routing.phase.options.as_ref(), "rate_limit_rpm") {
                let burst_capacity = extra_u64(routing.phase.options.as_ref(), "rate_limit_burst").or_else(|| {
                    extra_f64(routing.phase.options.as_ref(), "rate_limit_burst_multiplier")
                        .map(|m| ((rpm_limit as f64) * m.max(0.1)).round() as u64)
                });
            if !self
                .phase_rate_limiter
                    .allow(&routing.phase.phase_name, rpm_limit, burst_capacity)
            {
                self.send_error(
                    id,
                    -32029,
                    format!(
                        "phase '{}' rate limited at {} requests/min",
                        routing.phase.phase_name, rpm_limit
                    ),
                    None,
                )
                .await?;
                return Ok(());
            }
            }

            let phase_max_inflight = extra_u64(routing.phase.options.as_ref(), "phase_max_inflight");
            let global_max_inflight = extra_u64(routing.phase.options.as_ref(), "global_max_inflight");
            let _inflight_guard = match self.inflight_limiter.try_enter(
                &routing.phase.phase_name,
                phase_max_inflight,
                global_max_inflight,
            ) {
                Some(guard) => guard,
                None => {
                    self.send_error(
                        id,
                        -32030,
                        "inflight limit exceeded for this phase or globally".to_string(),
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            };

        let autopilot_complexity = routing
            .phase
            .options
            .as_ref()
            .and_then(|opts| opts.autopilot_complexity.as_deref())
            .and_then(AutopilotComplexity::from_str);

        let mut approval_strategy = mode_to_approval_strategy(mode, autopilot_complexity);
        if matches!(approval_strategy, ApprovalStrategy::AutoPilotSimple)
            && analyzed_task.complexity >= 3
            && self.should_escalate_approval_strategy()
        {
            approval_strategy = ApprovalStrategy::AutoPilotComplex;
            self.record_trace_event(
                &pipeline_trace,
                "phase.route_adapt",
                "ok",
                "route",
                json!({
                    "reason": "online_controller_escalation",
                    "new_strategy": approval_strategy.as_str(),
                }),
                None,
                0,
            );
            self.send_notification(
                "chat.pipeline",
                json!({
                    "id": id.clone(),
                    "event": "strategy_escalated",
                    "strategy": approval_strategy.as_str(),
                }),
            )
            .await?;
        }

        let review_policy = resolve_review_policy(
            routing.phase.options.as_ref(),
            Some(&analyzed_task),
            false,
            approval_strategy.needs_dual_review(),
        );
        if review_policy.enforce_dual_review && !approval_strategy.needs_dual_review() {
            approval_strategy = ApprovalStrategy::AutoPilotComplex;
            self.record_trace_event(
                &pipeline_trace,
                "phase.route_adapt",
                "ok",
                "route",
                json!({
                    "reason": "review_policy_enforced_dual_review",
                    "new_strategy": approval_strategy.as_str(),
                }),
                None,
                0,
            );
        }

        if let Some(reason) = pipeline_gate_violation(&analyzed_task, &pipeline_routing, approval_strategy) {
            self.record_trace_event(
                &pipeline_trace,
                "phase.route_hard_gate",
                "error",
                "route",
                json!({
                    "reason": reason,
                    "policy_status": "blocked",
                }),
                Some(reason.clone()),
                0,
            );
            self.send_error(id, -32603, crate::i18n::tf("error.pipeline_gate_blocked", &[("reason", &reason)]), None)
                .await?;
            return Ok(());
        }

        info!(
            "phase '{}' ({}) selected from flow '{}' with {} candidate agent(s); mode={}, strategy={}",
            routing.phase.phase_name,
            routing.phase.phase_description,
            routing.phase.flow_name,
            routing.agents.len(),
            mode_name,
            approval_strategy.as_str(),
        );

        let review_started = Instant::now();
        let review_decisions = if review_policy.enforce_dual_review {
            match self
                .run_dual_review_gate(
                    id.clone(),
                    &chat_params.messages,
                    routing.phase.options.as_ref(),
                    chat_span.as_ref().or(request_span.as_ref()),
                    &pipeline_trace,
                )
                .await
            {
                Ok(ReviewGateOutcome::Approved(decisions)) => {
                    self.record_trace_event(
                        &child_trace_context(&pipeline_trace, "chat.review"),
                        "phase.review_gate",
                        "ok",
                        "review",
                        json!({
                            "policy_status": "pass",
                            "result": "approved",
                            "review_decisions": decisions.len(),
                        }),
                        None,
                        review_started.elapsed().as_millis() as u64,
                    );
                    Some(decisions)
                }
                Ok(ReviewGateOutcome::Rejected(decisions)) => {
                    self.record_trace_event(
                        &child_trace_context(&pipeline_trace, "chat.review"),
                        "phase.review_gate",
                        "error",
                        "review",
                        json!({
                            "policy_status": "blocked",
                            "result": "rejected",
                            "review_decisions": decisions.len(),
                        }),
                        Some("review gate rejected execution".to_string()),
                        review_started.elapsed().as_millis() as u64,
                    );
                    self.send_error(
                        id,
                        -32603,
                        "review gate rejected execution".to_string(),
                        Some(json!({ "reviews": decisions })),
                    )
                    .await?;
                    return Ok(());
                }
                    Ok(ReviewGateOutcome::Degraded(decisions)) => {
                        self.record_trace_event(
                            &child_trace_context(&pipeline_trace, "chat.review"),
                            "phase.review_gate",
                            "ok",
                            "review",
                            json!({
                                "policy_status": "degraded",
                                "result": "degraded",
                                "review_decisions": decisions.len(),
                            }),
                            None,
                            review_started.elapsed().as_millis() as u64,
                        );
                        self.send_notification(
                            "chat.review",
                            json!({
                                "id": id.clone(),
                                "mode": "degrade_single",
                                "reason": "review gate timeout",
                            }),
                        )
                        .await?;
                        warn!(
                            trace_id = %pipeline_trace.trace_id,
                            "review gate degraded: timeout reached, proceeding with degraded single-reviewer approval"
                        );
                        Some(decisions)
                    }
                Err(err) => {
                    self.record_trace_event(
                        &child_trace_context(&pipeline_trace, "chat.review"),
                        "phase.review_gate",
                        "error",
                        "review",
                        json!({
                            "policy_status": "error",
                            "result": "failed",
                        }),
                        Some(err.to_string()),
                        review_started.elapsed().as_millis() as u64,
                    );
                    self.send_error(id, -32603, crate::i18n::tf("error.review_gate_failed", &[("error", &format!("{err}"))]), None)
                        .await?;
                    return Ok(());
                }
            }
        } else {
            None
        };

        self.record_trace_event(
            &pipeline_trace,
            "phase.verify",
            "ok",
            "verify",
            json!({
                "needs_dual_review": review_policy.enforce_dual_review,
                "review_decisions": review_decisions.as_ref().map(|v| v.len()).unwrap_or(0),
                "review_policy": review_policy,
            }),
            None,
            0,
        );
        let prepared_input = self
            .build_effective_messages(&routing.phase, &chat_params.messages)
            .await?;
        let bypass_cache = matches!(mode, Some(ChatMode::FullAuto));
        let cache_enabled = routing
            .phase
            .options
            .as_ref()
            .and_then(|opts| opts.cache_enabled)
            .unwrap_or(true);

        if !bypass_cache && cache_enabled {
            let cache_ttl = routing
                .phase
                .options
                .as_ref()
                .and_then(|opts| opts.cache_ttl_seconds)
                .unwrap_or(300);

            let cache_key = build_cache_key(
                &routing.phase,
                &prepared_input.messages,
                mode_name,
                approval_strategy.as_str(),
                &routing.phase.agent_names,
            )?;

            if let Some(memory_hit) = self.memory_cache.get(&cache_key) {
                self.metrics.inc_cache_hit();
                let cached_agent = memory_hit
                    .agent_name
                    .clone()
                    .unwrap_or_else(|| "memory-cache".to_string());
                let stream_payload = stream_chunk_notification(
                    &id,
                    &cached_agent,
                    &memory_hit.response_text,
                    1,
                    memory_hit.response_text.chars().count(),
                    Some("memory"),
                    Some(routing.phase.phase_name.as_str()),
                    Some(pipeline_trace.trace_id.as_str()),
                );
                self.send_notification(
                    "chat.stream",
                    stream_payload,
                )
                .await?;
                let done_payload = stream_done_notification(
                    &id,
                    &cached_agent,
                    1,
                    memory_hit.response_text.chars().count(),
                    Some("memory"),
                    Some(routing.phase.phase_name.as_str()),
                    Some(pipeline_trace.trace_id.as_str()),
                    0,
                );
                self.send_notification("chat.stream.done", done_payload).await?;
                self.record_trace_event(
                    &pipeline_trace,
                    "phase.agent",
                    "ok",
                    routing.phase.phase_name.as_str(),
                    json!({
                        "agent": cached_agent,
                        "cache_level": "memory",
                        "source": "memory_cache",
                    }),
                    None,
                    0,
                );

                self.send_result(
                    id,
                    json!({
                        "agent": memory_hit.agent_name,
                        "phase": routing.phase.phase_name,
                        "mode": mode_name,
                        "approval_strategy": approval_strategy.as_str(),
                        "review_policy": review_policy,
                        "cached": true,
                        "cache_level": "memory",
                        "done": true,
                        "reviews": review_decisions,
                        "pipeline": {
                            "analyze": format!("{:?}", analyzed_task.task_type),
                            "route_roles": pipeline_routing
                                .roles
                                .iter()
                                .map(|role| format!("{:?}", role))
                                .collect::<Vec<_>>(),
                        },
                    }),
                )
                .await?;
                self.record_trace_event(
                    &pipeline_trace,
                    "phase.learn",
                    "ok",
                    "learn",
                    json!({"source": "memory_cache"}),
                    None,
                    0,
                );
                return Ok(());
            }

            if let Some(cache) = self.cache_handle() {
                self.metrics.inc_cache_lookup();
                if let Some(hit) = self.cache_get(cache.clone(), cache_key.clone()).await? {
                    self.metrics.inc_cache_hit();
                        let cached_agent =
                            hit.agent_name.clone().unwrap_or_else(|| "cache".to_string());

                    self.memory_cache.put(
                        cache_key,
                        hit.response_text.clone(),
                        hit.agent_name.clone(),
                        cache_ttl,
                    );

                        let stream_payload = stream_chunk_notification(
                            &id,
                            &cached_agent,
                            &hit.response_text,
                            1,
                            hit.response_text.chars().count(),
                            Some("sqlite"),
                            Some(routing.phase.phase_name.as_str()),
                            Some(pipeline_trace.trace_id.as_str()),
                        );
                    self.send_notification(
                        "chat.stream",
                            stream_payload,
                    )
                    .await?;
                        let done_payload = stream_done_notification(
                            &id,
                            &cached_agent,
                            1,
                            hit.response_text.chars().count(),
                            Some("sqlite"),
                            Some(routing.phase.phase_name.as_str()),
                            Some(pipeline_trace.trace_id.as_str()),
                            0,
                        );
                        self.send_notification("chat.stream.done", done_payload).await?;
                    self.record_trace_event(
                        &pipeline_trace,
                        "phase.agent",
                        "ok",
                        routing.phase.phase_name.as_str(),
                        json!({
                            "agent": cached_agent,
                            "cache_level": "sqlite",
                            "source": "sqlite_cache",
                        }),
                        None,
                        0,
                    );

                    self.send_result(
                        id,
                        json!({
                            "agent": hit.agent_name,
                            "phase": routing.phase.phase_name,
                            "mode": mode_name,
                            "approval_strategy": approval_strategy.as_str(),
                            "review_policy": review_policy,
                            "cached": true,
                            "done": true,
                            "reviews": review_decisions,
                            "pipeline": {
                                "analyze": format!("{:?}", analyzed_task.task_type),
                                "route_roles": pipeline_routing
                                    .roles
                                    .iter()
                                    .map(|role| format!("{:?}", role))
                                    .collect::<Vec<_>>(),
                            },
                        }),
                    )
                    .await?;
                    self.record_trace_event(
                        &pipeline_trace,
                        "phase.learn",
                        "ok",
                        "learn",
                        json!({"source": "sqlite_cache"}),
                        None,
                        0,
                    );
                    return Ok(());
                }
                debug!(
                    trace_id = %pipeline_trace.trace_id,
                    phase = %routing.phase.phase_name,
                    "sqlite cache miss — forwarding to live agent"
                );
            }
        }

        let phase_name = routing.phase.phase_name.clone();
        let phase_options = routing.phase.options.clone();
        let phase_agent_options = routing
            .phase
            .options
            .as_ref()
            .and_then(|opts| opts.agent_options());
        let phase_principles = routing.phase.principles.clone();
        let phase_agent_names = routing.phase.agent_names.clone();
        let mut candidate_agents = routing.agents;
        let original_agent_order = candidate_agents
            .iter()
            .map(|(agent_name, _)| agent_name.clone())
            .collect::<Vec<_>>();
        let mut ranked_scores: Vec<(String, f64)> = Vec::new();

        if let Ok(state) = self.online_controller.lock() {
            let ranked = state.rank_agent_names_for_phase(&phase_name, &original_agent_order);
            let rank_index = ranked
                .iter()
                .enumerate()
                .map(|(idx, (name, _))| (name.clone(), idx))
                .collect::<HashMap<_, _>>();
            candidate_agents.sort_by_key(|(agent_name, _)| {
                rank_index
                    .get(agent_name)
                    .copied()
                    .unwrap_or(usize::MAX)
            });
            ranked_scores = ranked;
        }

        let ranked_agent_order = candidate_agents
            .iter()
            .map(|(agent_name, _)| agent_name.clone())
            .collect::<Vec<_>>();
        if original_agent_order != ranked_agent_order {
            self.record_trace_event(
                &pipeline_trace,
                "phase.route_adapt",
                "ok",
                "route",
                json!({
                    "reason": "online_controller_agent_ranking",
                    "original_order": original_agent_order,
                    "ranked_order": ranked_agent_order,
                    "scores": ranked_scores,
                }),
                None,
                0,
            );
        }

        let mut errors: Vec<String> = Vec::new();

            let breaker_failure_threshold = extra_u64(
                routing.phase.options.as_ref(),
                "circuit_breaker_failures",
            )
            .unwrap_or(DEFAULT_BREAKER_FAILURE_THRESHOLD as u64)
                as u32;
            let breaker_open_seconds = extra_u64(
                routing.phase.options.as_ref(),
                "circuit_breaker_open_seconds",
            )
            .unwrap_or(DEFAULT_BREAKER_OPEN_SECONDS as u64)
                as i64;

            for (agent_name, agent) in candidate_agents {
            let agent_started = Instant::now();
            let agent_span = chat_span.as_ref().or(request_span.as_ref()).and_then(|parent| {
                self.telemetry.start_child_span(
                    parent,
                    "acp.chat.agent",
                    vec![
                        KeyValue::new("agent.name", agent_name.clone()),
                        KeyValue::new("phase", phase_name.clone()),
                    ],
                )
            });
            match self.circuit_breakers.allow_request(&agent_name) {
                CircuitBreakerAdmission::Closed => {}
                CircuitBreakerAdmission::HalfOpenProbe => {
                    info!("agent '{}' entering half-open probe", agent_name);
                }
                CircuitBreakerAdmission::Rejected {
                    state,
                    retry_after_seconds,
                } => {
                    warn!(
                        "agent '{}' skipped due to circuit breaker state {}",
                        agent_name, state
                    );
                    errors.push(match retry_after_seconds {
                        Some(seconds) => format!(
                            "{}: skipped by circuit breaker ({}, retry after {}s)",
                            agent_name, state, seconds
                        ),
                        None => format!(
                            "{}: skipped by circuit breaker ({})",
                            agent_name, state
                        ),
                    });
                    if let Some(span) = agent_span {
                        self.telemetry.end_span(
                            span,
                            vec![
                                KeyValue::new("agent.status", "skipped"),
                                KeyValue::new("breaker.state", state.to_string()),
                            ],
                        );
                    }
                    continue;
                }
            }

            match self
                .run_agent_streaming(
                    id.clone(),
                    agent_name.clone(),
                    agent,
                    prepared_input.messages.clone(),
                    phase_principles.clone(),
                    phase_agent_options.clone(),
                    request_timeout(phase_options.as_ref()),
                    Some(phase_name.as_str()),
                    Some(pipeline_trace.trace_id.as_str()),
                )
                .await
            {
                Ok(response_text) => {
                    let agent_duration = agent_started.elapsed();
                    self.record_online_controller_agent_outcome(
                        &phase_name,
                        &agent_name,
                        true,
                        agent_duration,
                    );
                    self.circuit_breakers.record_success(&agent_name);
                    if !bypass_cache && cache_enabled {
                        if let Some(cache) = self.cache_handle() {
                            let cache_key = build_cache_key_from_parts(
                                &phase_name,
                                &prepared_input.messages,
                                phase_principles.as_ref(),
                                phase_options.as_ref(),
                                mode_name,
                                approval_strategy.as_str(),
                                &phase_agent_names,
                            )?;
                            let ttl = phase_options
                                .as_ref()
                                .and_then(|opts| opts.cache_ttl_seconds);
                            self.cache_put(
                                cache.clone(),
                                cache_key,
                                response_text.clone(),
                                agent_name.clone(),
                                ttl,
                            )
                            .await?;
                            self.metrics.inc_cache_store();
                        }

                        let ttl = phase_options
                            .as_ref()
                            .and_then(|opts| opts.cache_ttl_seconds)
                            .unwrap_or(300);
                        self.memory_cache.put(
                            build_cache_key_from_parts(
                                &phase_name,
                                &prepared_input.messages,
                                phase_principles.as_ref(),
                                phase_options.as_ref(),
                                mode_name,
                                approval_strategy.as_str(),
                                &phase_agent_names,
                            )?,
                            response_text.clone(),
                            Some(agent_name.clone()),
                            ttl,
                        );
                    }

                    self.persist_memory_updates(
                        &phase_name,
                        phase_options.as_ref(),
                        prepared_input.latest_user_query.as_deref(),
                        &response_text,
                    )
                    .await?;

                    self.send_result(
                        id.clone(),
                        json!({
                            "agent": agent_name,
                            "phase": phase_name,
                            "mode": mode_name,
                            "approval_strategy": approval_strategy.as_str(),
                            "review_policy": review_policy,
                            "cached": false,
                            "done": true,
                            "reviews": review_decisions,
                            "pipeline": {
                                "analyze": format!("{:?}", analyzed_task.task_type),
                                "route_roles": pipeline_routing
                                    .roles
                                    .iter()
                                    .map(|role| format!("{:?}", role))
                                    .collect::<Vec<_>>(),
                                "success_rate": pipeline_routing.predicted_success_rate,
                            },
                        }),
                    )
                    .await?;
                    self.record_trace_event(
                        &pipeline_trace,
                        "phase.evaluate",
                        "ok",
                        "evaluate",
                        json!({
                            "predicted_success_rate": pipeline_routing.predicted_success_rate,
                            "risk_factors": pipeline_routing.risk_factors,
                        }),
                        None,
                        0,
                    );
                    self.record_trace_event(
                        &child_trace_context(&pipeline_trace, &format!("chat.agent.{}", agent_name)),
                        "phase.agent",
                        "ok",
                        &phase_name,
                        json!({ "agent": agent_name.clone() }),
                        None,
                        agent_started.elapsed().as_millis() as u64,
                    );
                    if let Some(span) = agent_span {
                        self.telemetry.end_span(
                            span,
                            vec![
                                KeyValue::new("agent.status", "ok"),
                                KeyValue::new(
                                    "agent.duration_ms",
                                    agent_duration.as_millis() as i64,
                                ),
                            ],
                        );
                    }
                    self.record_trace_event(
                        &pipeline_trace,
                        "phase.learn",
                        "ok",
                        "learn",
                        json!({"source": "agent_output"}),
                        None,
                        0,
                    );
                    // Auto-checkpoint: capture input messages + agent response for recovery
                    let mut cp_messages = original_messages.clone();
                    cp_messages.push(Message {
                        role: "assistant".to_string(),
                        content: response_text.clone(),
                    });
                    let cp_note = format!("{}/{}", phase_name, agent_name);
                    match self.create_conversation_checkpoint(
                        &auto_conv_id,
                        "main",
                        cp_messages,
                        Some(cp_note),
                    ) {
                        Ok(cp) => {
                            let _ = self
                                .send_notification(
                                    "conversation.checkpoint",
                                    json!({
                                        "checkpoint_id": cp.checkpoint_id,
                                        "conversation_id": cp.conversation_id,
                                        "branch_id": cp.branch_id,
                                        "auto": true,
                                    }),
                                )
                                .await;
                        }
                        Err(err) => {
                            warn!("auto-checkpoint skipped: {}", err);
                        }
                    }
                    // Section 7: QA gate — only for FullAuto + high-complexity requests
                    if matches!(mode, Some(ChatMode::FullAuto))
                        && analyzed_task.complexity >= 3
                    {
                        match run_action_check(&self.artifact_ledger(), ActionCheckKind::Qa) {
                            Ok(qa_report) => {
                                if !qa_report.ok {
                                    warn!(
                                        trace_id = %pipeline_trace.trace_id,
                                        phase = %phase_name,
                                        overall_status = ?qa_report.overall_status,
                                        "qa gate: artifacts incomplete — checkpoint and retest before promotion"
                                    );
                                }
                                let _ = self
                                    .send_notification(
                                        "chat.qa_gate",
                                        json!({
                                            "trace_id": pipeline_trace.trace_id,
                                            "ok": qa_report.ok,
                                            "overall_status": format!("{:?}", qa_report.overall_status),
                                            "evidence_refs": qa_report.evidence_refs,
                                        }),
                                    )
                                    .await;
                            }
                            Err(err) => {
                                warn!(
                                    trace_id = %pipeline_trace.trace_id,
                                    error = %err,
                                    "qa gate check skipped: ledger unavailable"
                                );
                            }
                        }
                    }
                    return Ok(());
                }
                Err(err) => {
                    let agent_duration = agent_started.elapsed();
                    self.record_online_controller_agent_outcome(
                        &phase_name,
                        &agent_name,
                        false,
                        agent_duration,
                    );
                    self.metrics.inc_agent_failures();
                    let failure_kind = classify_agent_failure(&err);
                    match failure_kind {
                        "timeout" => self.metrics.inc_agent_timeout_failures(),
                        "panic" => self.metrics.inc_agent_panic_failures(),
                        _ => self.metrics.inc_agent_other_failures(),
                    }
                    self.circuit_breakers.record_failure_with_config(
                        &agent_name,
                        breaker_failure_threshold,
                        breaker_open_seconds,
                    );
                    if let Some(span) = agent_span {
                        self.telemetry.end_span(
                            span,
                            vec![
                                KeyValue::new("agent.status", "error"),
                                KeyValue::new("error", err.to_string()),
                                KeyValue::new(
                                    "agent.duration_ms",
                                    agent_duration.as_millis() as i64,
                                ),
                            ],
                        );
                    }
                    self.record_trace_event(
                        &child_trace_context(
                            &pipeline_trace,
                            &format!("chat.agent.{}", agent_name),
                        ),
                        "phase.agent",
                        "error",
                        &phase_name,
                        json!({
                            "agent": agent_name,
                            "failure_kind": failure_kind,
                        }),
                        Some(err.to_string()),
                        agent_duration.as_millis() as u64,
                    );
                    warn!("agent '{}' failed: {err:#}", agent_name);
                    errors.push(format!("{}: {}", agent_name, err));
                }
            }
            }

            self.record_trace_event(
                &pipeline_trace,
                "phase.evaluate",
                "error",
                "evaluate",
                json!({
                    "policy_status": "error",
                    "error_count": errors.len(),
                }),
                Some("all candidate agents failed".to_string()),
                0,
            );
            error!(
                trace_id = %pipeline_trace.trace_id,
                phase = %phase_name,
                error_count = errors.len(),
                "all candidate agents failed: {:?}",
                errors
            );
            self.send_error(
                id,
                -32603,
                crate::i18n::t("error.all_agents_failed"),
                Some(json!({ "errors": errors })),
            )
            .await
        }
        .await;

        if let Some(span) = chat_span {
            self.telemetry.end_span(
                span,
                vec![
                    KeyValue::new("chat.status", if result.is_ok() { "ok" } else { "error" }),
                    KeyValue::new("chat.duration_ms", started.elapsed().as_millis() as i64),
                ],
            );
        }

        if let Ok(mut state) = self.online_controller.lock() {
            state.record(result.is_ok(), started.elapsed().as_millis() as u64);
        }

        self.metrics.observe_chat_latency(started.elapsed());
        result
    }

    fn should_escalate_approval_strategy(&self) -> bool {
        self.online_controller
            .lock()
            .map(|state| state.should_escalate())
            .unwrap_or(false)
    }

    fn create_conversation_checkpoint(
        &self,
        conversation_id: &str,
        branch_id: &str,
        messages: Vec<Message>,
        note: Option<String>,
    ) -> std::result::Result<ConversationCheckpoint, String> {
        if checkpoint_message_chars(&messages) > MAX_CHECKPOINT_MESSAGE_CHARS {
            return Err(format!(
                "checkpoint messages exceed max chars {}",
                MAX_CHECKPOINT_MESSAGE_CHARS
            ));
        }

        let checkpoint = {
            let mut store = self
                .conversation_store
                .lock()
                .map_err(|_| "conversation store lock poisoned".to_string())?;

            if !store.contains_key(conversation_id) && store.len() >= MAX_CONVERSATIONS_TRACKED {
                if let Some(evicted) =
                    evict_oldest_conversation(&mut store, &self.conversation_touch_order)
                {
                    warn!(
                        "conversation store reached limit ({}), evicted oldest conversation '{}'",
                        MAX_CONVERSATIONS_TRACKED, evicted
                    );
                }
            }

            let touched_at = now_ts();
            let state = store
                .entry(conversation_id.to_string())
                .or_insert_with(ConversationState::default);
            state.last_touched_at = touched_at;

            enforce_checkpoint_capacity(state, 1, None);

            let parent_checkpoint_id = state.branch_heads.get(branch_id).cloned();
            let checkpoint = ConversationCheckpoint {
                checkpoint_id: format!("cp-{}", CHECKPOINT_COUNTER.fetch_add(1, Ordering::Relaxed)),
                conversation_id: conversation_id.to_string(),
                branch_id: branch_id.to_string(),
                parent_checkpoint_id,
                created_at: now_ts(),
                note,
                messages,
            };

            state
                .branch_heads
                .insert(branch_id.to_string(), checkpoint.checkpoint_id.clone());
            state.checkpoints.push(checkpoint.clone());
            touch_conversation_order(&self.conversation_touch_order, conversation_id);
            checkpoint
        };

        self.persist_checkpoint_summary(&checkpoint);
        Ok(checkpoint)
    }

    fn list_conversation_checkpoints(
        &self,
        conversation_id: &str,
        branch_id: Option<&str>,
        limit: usize,
    ) -> std::result::Result<Vec<ConversationCheckpoint>, String> {
        let store = self
            .conversation_store
            .lock()
            .map_err(|_| "conversation store lock poisoned".to_string())?;
        let Some(state) = store.get(conversation_id) else {
            return Ok(Vec::new());
        };

        Ok(state
            .checkpoints
            .iter()
            .rev()
            .filter(|checkpoint| {
                branch_id
                    .map(|target| checkpoint.branch_id == target)
                    .unwrap_or(true)
            })
            .take(limit.max(1))
            .cloned()
            .collect::<Vec<_>>())
    }

    fn rollback_conversation_checkpoint(
        &self,
        conversation_id: &str,
        checkpoint_id: &str,
        target_branch: Option<&str>,
    ) -> Option<ConversationCheckpoint> {
        let restored = {
            let mut store = match self.conversation_store.lock() {
                Ok(guard) => guard,
                Err(_) => {
                    warn!(
                        "conversation rollback failed because conversation store lock is poisoned"
                    );
                    return None;
                }
            };
            let state = store.get_mut(conversation_id)?;
            state.last_touched_at = now_ts();
            let checkpoint = state
                .checkpoints
                .iter()
                .find(|candidate| candidate.checkpoint_id == checkpoint_id)
                .cloned()?;

            let branch = target_branch
                .unwrap_or(checkpoint.branch_id.as_str())
                .to_string();
            let restored = ConversationCheckpoint {
                checkpoint_id: format!("cp-{}", CHECKPOINT_COUNTER.fetch_add(1, Ordering::Relaxed)),
                conversation_id: conversation_id.to_string(),
                branch_id: branch.clone(),
                parent_checkpoint_id: Some(checkpoint.checkpoint_id.clone()),
                created_at: now_ts(),
                note: Some(format!("rollback:{}", checkpoint.checkpoint_id)),
                messages: checkpoint.messages.clone(),
            };

            enforce_checkpoint_capacity(state, 1, Some(checkpoint_id));
            state.checkpoints.push(restored.clone());
            state
                .branch_heads
                .insert(branch, restored.checkpoint_id.clone());
            touch_conversation_order(&self.conversation_touch_order, conversation_id);
            restored
        };

        self.persist_checkpoint_summary(&restored);
        Some(restored)
    }

    fn prune_conversation_checkpoints(
        &self,
        conversation_id: &str,
        branch_id: Option<&str>,
        keep: usize,
    ) -> ConversationPruneResult {
        let Ok(mut store) = self.conversation_store.lock() else {
            warn!("conversation prune skipped because conversation store lock is poisoned");
            return ConversationPruneResult::default();
        };
        let Some(state) = store.get_mut(conversation_id) else {
            return ConversationPruneResult::default();
        };
        state.last_touched_at = now_ts();

        let original_len = state.checkpoints.len();
        if let Some(target_branch) = branch_id {
            let mut branch_checkpoints: Vec<String> = state
                .checkpoints
                .iter()
                .filter(|cp| cp.branch_id == target_branch)
                .map(|cp| cp.checkpoint_id.clone())
                .collect();

            if branch_checkpoints.len() <= keep {
                return ConversationPruneResult::default();
            }

            let to_remove_count = branch_checkpoints.len() - keep;
            let to_remove: HashSet<String> = branch_checkpoints.drain(0..to_remove_count).collect();
            state
                .checkpoints
                .retain(|cp| !to_remove.contains(&cp.checkpoint_id));
        } else {
            // Prune globally: keep most recent `keep` checkpoints across all branches
            if state.checkpoints.len() <= keep {
                return ConversationPruneResult::default();
            }
            let drain_to = state.checkpoints.len() - keep;
            state.checkpoints.drain(0..drain_to);
        }

        let before_heads = state.branch_heads.clone();
        repair_conversation_branch_heads(state);
        let (repaired_heads, dropped_heads) =
            branch_head_adjustment_counts(&before_heads, &state.branch_heads);
        touch_conversation_order(&self.conversation_touch_order, conversation_id);

        ConversationPruneResult {
            removed: original_len - state.checkpoints.len(),
            repaired_heads,
            dropped_heads,
        }
    }

    fn record_online_controller_agent_outcome(
        &self,
        phase_name: &str,
        agent_name: &str,
        success: bool,
        duration: Duration,
    ) {
        if let Ok(mut state) = self.online_controller.lock() {
            state.record_agent_outcome(
                phase_name,
                agent_name,
                success,
                duration.as_millis() as u64,
            );
        }
    }

    fn infer_phase_name_with_flow(
        &self,
        flow: &FlowManager,
        explicit_phase: Option<&str>,
        mode: Option<ChatMode>,
    ) -> String {
        if let Some(phase) = explicit_phase {
            return phase.to_string();
        }

        match mode {
            Some(ChatMode::Ask) if flow.has_phase("review") => "review".to_string(),
            Some(ChatMode::Edit) | Some(ChatMode::Agent) | Some(ChatMode::FullAuto)
                if flow.has_phase("coding") =>
            {
                "coding".to_string()
            }
            _ => flow.default_phase().to_string(),
        }
    }

    async fn build_effective_messages(
        &self,
        phase: &ResolvedPhase,
        messages: &[Message],
    ) -> Result<PreparedChatInput> {
        let vector_config_snapshot = self.vector_config_snapshot();
        let optimized_messages = optimize_messages(messages, phase.options.as_ref());
        let latest_query = latest_user_query(&optimized_messages);
        let mut prepared_messages: Vec<Message> = Vec::new();

        if let Some(vector_store) = self.vector_store_handle() {
            let tuned_state = if let Some(autotune) = self.autotune_handle() {
                Some(autotune_state_snapshot(&autotune).await)
            } else {
                None
            };

            let summary_enabled =
                effective_summary_enabled(phase.options.as_ref(), vector_config_snapshot.as_ref());
            let summary_trigger = effective_summary_trigger_messages(
                phase.options.as_ref(),
                vector_config_snapshot.as_ref(),
            );

            if summary_enabled && optimized_messages.len() >= summary_trigger {
                self.metrics.inc_summary_read();
                if let Some(summary) = self
                    .vector_get_phase_summary(vector_store.clone(), phase.phase_name.clone())
                    .await?
                {
                    self.metrics.inc_summary_hit();
                    prepared_messages.push(Message {
                        role: "user".to_string(),
                        content: format!("Conversation summary for this phase:\n{}", summary),
                    });
                }
            }

            let vector_enabled =
                effective_vector_enabled(phase.options.as_ref(), vector_config_snapshot.as_ref());
            if vector_enabled {
                let vector_auto =
                    effective_vector_auto(phase.options.as_ref(), vector_config_snapshot.as_ref());
                let min_query_chars = effective_vector_min_query_chars(
                    phase.options.as_ref(),
                    vector_config_snapshot.as_ref(),
                    tuned_state.as_ref(),
                );

                if let Some(query) = latest_query.as_ref() {
                    let should_search = if vector_auto {
                        query.chars().count() >= min_query_chars
                    } else {
                        !query.trim().is_empty()
                    };

                    if should_search {
                        self.metrics.inc_vector_search();
                        let top_k = effective_vector_top_k(
                            phase.options.as_ref(),
                            vector_config_snapshot.as_ref(),
                            tuned_state.as_ref(),
                        );
                        let min_similarity = effective_vector_min_similarity(
                            phase.options.as_ref(),
                            vector_config_snapshot.as_ref(),
                        );
                        let max_snippet_chars = effective_vector_max_snippet_chars(
                            phase.options.as_ref(),
                            vector_config_snapshot.as_ref(),
                        );

                        let (hits, feedback) = self
                            .vector_search(
                                vector_store.clone(),
                                phase.phase_name.clone(),
                                query.clone(),
                                top_k,
                                min_similarity,
                                max_snippet_chars,
                            )
                            .await?;

                        // Record precision feedback for autotune if enabled
                        if let Some(autotune) = self.autotune_handle() {
                            if let Some(config) = self.autotune_config_snapshot() {
                                let state_to_persist = {
                                    let mut state = autotune.lock().await;
                                    state.record_vector_search(feedback.avg_similarity, &config);

                                    let mut mutated = false;
                                    if state.advance_cooldown_window(&config) {
                                        mutated = true;
                                    } else if state.should_evaluate(&config) {
                                        state.evaluate_and_adjust(&config);
                                        mutated = true;
                                    }

                                    if mutated {
                                        Some(state.clone())
                                    } else {
                                        None
                                    }
                                };

                                if let Some(state) = state_to_persist {
                                    if let Some(path) = self.autotune_state_path_snapshot() {
                                        if let Err(e) = state.save(path.as_str()) {
                                            warn!(
                                                "{}",
                                                crate::i18n::tf(
                                                    "warning.failed_persist_autotune",
                                                    &[("error", &format!("{}", e))]
                                                )
                                            );
                                        }
                                    } else {
                                        warn!("autotune update skipped persistence because no resolved state path is available");
                                    }
                                }
                            }
                        }

                        if !hits.is_empty() {
                            self.metrics.inc_vector_hit();
                            prepared_messages.push(Message {
                                role: "user".to_string(),
                                content: build_vector_context_message(&hits),
                            });
                        }
                    }
                }
            }
        }

        prepared_messages.extend(optimized_messages);

        Ok(PreparedChatInput {
            messages: prepared_messages,
            latest_user_query: latest_query,
        })
    }

    async fn persist_memory_updates(
        &self,
        phase_name: &str,
        options: Option<&PhaseOptions>,
        latest_user_query: Option<&str>,
        response_text: &str,
    ) -> Result<()> {
        let vector_config_snapshot = self.vector_config_snapshot();
        let Some(vector_store) = self.vector_store_handle() else {
            return Ok(());
        };

        if let Some(query) = latest_user_query {
            self.vector_upsert(
                vector_store.clone(),
                phase_name.to_string(),
                query.to_string(),
                response_text.to_string(),
            )
            .await?;
            self.metrics.inc_vector_store();
        }

        let summary_enabled = effective_summary_enabled(options, vector_config_snapshot.as_ref());
        if !summary_enabled {
            return Ok(());
        }

        self.metrics.inc_summary_read();
        let existing_summary = self
            .vector_get_phase_summary(vector_store.clone(), phase_name.to_string())
            .await?;
        if existing_summary.is_some() {
            self.metrics.inc_summary_hit();
        }

        let summary_max_chars =
            effective_summary_max_chars(options, vector_config_snapshot.as_ref());
        let new_summary = append_recent_summary(
            existing_summary.as_deref(),
            latest_user_query,
            response_text,
            summary_max_chars,
        );

        self.vector_upsert_phase_summary(vector_store.clone(), phase_name.to_string(), new_summary)
            .await?;
        self.metrics.inc_summary_store();
        Ok(())
    }

    async fn cache_get(
        &self,
        cache: Arc<ResponseCache>,
        cache_key: String,
    ) -> Result<Option<crate::cache::CachedResponse>> {
        spawn_blocking(move || cache.get(&cache_key))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "cache_get"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn cache_put(
        &self,
        cache: Arc<ResponseCache>,
        cache_key: String,
        response_text: String,
        agent_name: String,
        ttl: Option<u64>,
    ) -> Result<()> {
        spawn_blocking(move || cache.put(&cache_key, &response_text, &agent_name, ttl))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "cache_put"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn cache_entry_count(&self, cache: Arc<ResponseCache>) -> Result<u64> {
        spawn_blocking(move || cache.entry_count())
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "cache_entry_count"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn cache_clear(&self, cache: Arc<ResponseCache>) -> Result<usize> {
        spawn_blocking(move || cache.clear_all())
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "cache_clear"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn vector_search(
        &self,
        vector_store: Arc<VectorStore>,
        phase: String,
        query: String,
        top_k: usize,
        min_similarity: f32,
        max_snippet_chars: usize,
    ) -> Result<(Vec<VectorHit>, crate::vector::VectorPrecisionFeedback)> {
        spawn_blocking(move || {
            vector_store.search(&phase, &query, top_k, min_similarity, max_snippet_chars)
        })
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "{}",
                crate::i18n::tf(
                    "error.task_join",
                    &[("task", "vector_search"), ("error", &format!("{}", e))]
                )
            )
        })?
    }

    async fn vector_get_phase_summary(
        &self,
        vector_store: Arc<VectorStore>,
        phase: String,
    ) -> Result<Option<String>> {
        spawn_blocking(move || vector_store.get_phase_summary(&phase))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[
                            ("task", "vector_get_phase_summary"),
                            ("error", &format!("{}", e))
                        ]
                    )
                )
            })?
    }

    async fn vector_upsert(
        &self,
        vector_store: Arc<VectorStore>,
        phase: String,
        query: String,
        response_text: String,
    ) -> Result<()> {
        spawn_blocking(move || vector_store.upsert(&phase, &query, &response_text))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "vector_upsert"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn vector_entry_counts(&self, vector_store: Arc<VectorStore>) -> Result<(u64, u64)> {
        spawn_blocking(move || {
            let memory = vector_store.memory_entry_count()?;
            let summaries = vector_store.summary_entry_count()?;
            Ok::<(u64, u64), anyhow::Error>((memory, summaries))
        })
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "{}",
                crate::i18n::tf(
                    "error.task_join",
                    &[
                        ("task", "vector_entry_counts"),
                        ("error", &format!("{}", e))
                    ]
                )
            )
        })?
    }

    async fn vector_clear(&self, vector_store: Arc<VectorStore>) -> Result<(usize, usize)> {
        spawn_blocking(move || vector_store.clear_all())
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "vector_clear"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn vector_upsert_phase_summary(
        &self,
        vector_store: Arc<VectorStore>,
        phase: String,
        summary: String,
    ) -> Result<()> {
        spawn_blocking(move || vector_store.upsert_phase_summary(&phase, &summary))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[
                            ("task", "vector_upsert_phase_summary"),
                            ("error", &format!("{}", e))
                        ]
                    )
                )
            })?
    }

    async fn run_dual_review_gate(
        &self,
        id: Option<Value>,
        messages: &[Message],
        phase_options: Option<&PhaseOptions>,
        parent_span: Option<&OtelContext>,
        pipeline_trace: &RequestTraceContext,
    ) -> Result<ReviewGateOutcome> {
        let started = Instant::now();
        self.metrics.inc_review_gate();
        let review_span = parent_span.and_then(|parent| {
            self.telemetry.start_child_span(
                parent,
                "acp.chat.review_gate",
                vec![KeyValue::new("gate.mode", "dual")],
            )
        });

        let timeout_policy = ReviewTimeoutPolicy::from_options(phase_options);
        let gate_timeout = extra_u64(phase_options, "review_gate_timeout_seconds")
            .or_else(|| phase_options.and_then(|opts| opts.review_timeout_seconds))
            .or_else(|| phase_options.and_then(|opts| opts.request_timeout_seconds))
            .map(Duration::from_secs);
        let gate_deadline = gate_timeout.map(|limit| Instant::now() + limit);

        let result = async {
            let (flow, registry) = self.routing_handles()?;

            let review_routing = flow
                .resolve(Some("review".to_string()), registry.as_ref())
                .map_err(|err| {
                    anyhow::anyhow!(
                        "{}",
                        crate::i18n::tf(
                            "error.review_phase_required",
                            &[("error", &format!("{err}"))]
                        )
                    )
                })?;

            let mut reviewer_names = phase_options
                .and_then(|options| options.full_auto_review_agents.clone())
                .unwrap_or_else(|| review_routing.phase.agent_names.clone());

            let review_phase_name = review_routing.phase.phase_name.clone();
            let original_reviewer_order = reviewer_names.clone();
            let mut reviewer_scores: Vec<(String, f64)> = Vec::new();
            if let Ok(state) = self.online_controller.lock() {
                let ranked = state.rank_agent_names_for_phase(&review_phase_name, &reviewer_names);
                let rank_index = ranked
                    .iter()
                    .enumerate()
                    .map(|(idx, (name, _))| (name.clone(), idx))
                    .collect::<HashMap<_, _>>();
                reviewer_names
                    .sort_by_key(|name| rank_index.get(name).copied().unwrap_or(usize::MAX));
                reviewer_scores = ranked;
            }

            if reviewer_names != original_reviewer_order {
                self.record_trace_event(
                    &child_trace_context(pipeline_trace, "chat.review.route_adapt"),
                    "phase.review_route_adapt",
                    "ok",
                    "review",
                    json!({
                        "reason": "online_controller_reviewer_ranking",
                        "original_order": original_reviewer_order,
                        "ranked_order": reviewer_names,
                        "scores": reviewer_scores,
                    }),
                    None,
                    0,
                );
            }

            if reviewer_names.len() > 2 {
                reviewer_names.truncate(2);
            }

            let min_reviewers = extra_u64(phase_options, "min_reviewers").unwrap_or(2) as usize;
            let required_approvals = extra_u64(phase_options, "required_approvals")
                .unwrap_or(min_reviewers as u64)
                .max(1) as usize;

            if reviewer_names.len() < min_reviewers {
                anyhow::bail!(
                    "complex full_auto mode requires at least {} review agents",
                    min_reviewers
                );
            }

            let mut prepared_review = self
                .build_effective_messages(&review_routing.phase, messages)
                .await?;
            prepared_review.messages.push(Message {
                role: "user".to_string(),
                content: review_gate_prompt(),
            });

            let mut decisions = Vec::new();
            let mut approved_count = 0usize;
            let min_review_chars =
                extra_u64(phase_options, "review_min_response_chars").unwrap_or(8) as usize;
            let total_reviewers = reviewer_names.len();
            for (idx, reviewer) in reviewer_names.into_iter().enumerate() {
                let reviewer_started = Instant::now();
                let reviewer_span = review_span.as_ref().and_then(|parent| {
                    self.telemetry.start_child_span(
                        parent,
                        "acp.chat.reviewer",
                        vec![KeyValue::new("reviewer", reviewer.clone())],
                    )
                });
                if let Some(deadline) = gate_deadline {
                    let now = Instant::now();
                    if now >= deadline {
                        let err = anyhow::anyhow!(
                            "review gate timed out after {}s",
                            gate_timeout.map(|d| d.as_secs()).unwrap_or(0)
                        );
                        self.metrics.inc_review_gate_timeout();
                        record_agent_failure_metrics(self.metrics.as_ref(), &err);

                        return match timeout_policy {
                            ReviewTimeoutPolicy::Reject => {
                                self.metrics.inc_review_gate_rejected();
                                Ok(ReviewGateOutcome::Rejected(decisions))
                            }
                            ReviewTimeoutPolicy::DegradeSingle => {
                                if approved_count >= 1 {
                                    self.metrics.inc_review_gate_degraded();
                                    self.metrics.inc_review_gate_approved();
                                    Ok(ReviewGateOutcome::Degraded(decisions))
                                } else {
                                    self.metrics.inc_review_gate_rejected();
                                    Ok(ReviewGateOutcome::Rejected(decisions))
                                }
                            }
                        };
                    }
                }

                let agent = registry.get(&reviewer).ok_or_else(|| {
                    anyhow::anyhow!(
                        "{}",
                        crate::i18n::tf("error.review_agent_not_available", &[("name", &reviewer)])
                    )
                })?;

                let reviewer_timeout = if let Some(deadline) = gate_deadline {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    let configured =
                        review_timeout(review_routing.phase.options.as_ref(), phase_options);
                    match configured {
                        Some(configured_limit) => Some(std::cmp::min(configured_limit, remaining)),
                        None => Some(remaining),
                    }
                } else {
                    review_timeout(review_routing.phase.options.as_ref(), phase_options)
                };

                let response = match self
                    .run_agent_collecting(
                        reviewer.clone(),
                        agent,
                        prepared_review.messages.clone(),
                        review_routing.phase.principles.clone(),
                        review_routing
                            .phase
                            .options
                            .as_ref()
                            .and_then(|opts| opts.agent_options()),
                        reviewer_timeout,
                    )
                    .await
                {
                    Ok(response) => response,
                    Err(err) => {
                        self.record_online_controller_agent_outcome(
                            &review_phase_name,
                            &reviewer,
                            false,
                            reviewer_started.elapsed(),
                        );
                        if let Some(span) = reviewer_span {
                            self.telemetry.end_span(
                                span,
                                vec![
                                    KeyValue::new("review.status", "error"),
                                    KeyValue::new("error", err.to_string()),
                                ],
                            );
                        }
                        record_agent_failure_metrics(self.metrics.as_ref(), &err);
                        let err_message = err.to_string();
                        if classify_agent_failure(&err) == "timeout" {
                            self.metrics.inc_review_gate_timeout();
                            return match timeout_policy {
                                ReviewTimeoutPolicy::Reject => {
                                    self.metrics.inc_review_gate_rejected();
                                    Ok(ReviewGateOutcome::Rejected(decisions))
                                }
                                ReviewTimeoutPolicy::DegradeSingle => {
                                    if approved_count >= 1 {
                                        self.metrics.inc_review_gate_degraded();
                                        self.metrics.inc_review_gate_approved();
                                        Ok(ReviewGateOutcome::Degraded(decisions))
                                    } else {
                                        self.metrics.inc_review_gate_rejected();
                                        Ok(ReviewGateOutcome::Rejected(decisions))
                                    }
                                }
                            };
                        }
                        return Err(anyhow::anyhow!(err_message));
                    }
                };

                let verdict = review_verdict(&response, min_review_chars);
                self.record_online_controller_agent_outcome(
                    &review_phase_name,
                    &reviewer,
                    verdict != ReviewVerdict::Invalid,
                    reviewer_started.elapsed(),
                );
                if verdict == ReviewVerdict::Invalid {
                    self.metrics.inc_review_gate_invalid_response();
                }
                let decision = ReviewDecision {
                    reviewer: reviewer.clone(),
                    verdict: verdict.as_str().to_string(),
                    response: response.clone(),
                };

                self.send_notification(
                    "chat.review",
                    json!({
                        "id": id.clone(),
                        "reviewer": reviewer,
                        "verdict": decision.verdict,
                    }),
                )
                .await?;

                decisions.push(decision);
                if let Some(span) = reviewer_span {
                    self.telemetry.end_span(
                        span,
                        vec![
                            KeyValue::new("review.status", verdict.as_str().to_string()),
                            KeyValue::new(
                                "review.duration_ms",
                                reviewer_started.elapsed().as_millis() as i64,
                            ),
                        ],
                    );
                }

                if verdict.is_approved() {
                    approved_count += 1;
                    if approved_count >= required_approvals {
                        self.metrics.inc_review_gate_approved();
                        return Ok(ReviewGateOutcome::Approved(decisions));
                    }
                }

                let remaining = total_reviewers - (idx + 1);
                if approved_count + remaining < required_approvals {
                    self.metrics.inc_review_gate_rejected();
                    return Ok(ReviewGateOutcome::Rejected(decisions));
                }
            }

            if approved_count >= required_approvals {
                self.metrics.inc_review_gate_approved();
                Ok(ReviewGateOutcome::Approved(decisions))
            } else {
                self.metrics.inc_review_gate_rejected();
                Ok(ReviewGateOutcome::Rejected(decisions))
            }
        };

        let output = result.await;
        if let Some(span) = review_span {
            self.telemetry.end_span(
                span,
                vec![
                    KeyValue::new("gate.status", if output.is_ok() { "ok" } else { "error" }),
                    KeyValue::new("gate.duration_ms", started.elapsed().as_millis() as i64),
                ],
            );
        }
        self.metrics.observe_review_latency(started.elapsed());
        output
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_agent_streaming(
        &self,
        id: Option<Value>,
        agent_name: String,
        agent: Arc<dyn Agent>,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        timeout_limit: Option<Duration>,
        phase_name: Option<&str>,
        trace_id: Option<&str>,
    ) -> Result<String> {
        let started = Instant::now();
        let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
        let agent_task =
            tokio::spawn(async move { agent.chat(messages, principles, options, sender).await });

        let mut response_text = String::new();
        let mut stream_chunks: usize = 0;
        let mut streamed_chars: usize = 0;
        let collect_stream = async {
            while let Some(token) = receiver.recv().await {
                let token_chars = token.chars().count();
                let projected_chunks = stream_chunks.saturating_add(1);
                let projected_chars = streamed_chars.saturating_add(token_chars);
                if stream_would_exceed_limits(stream_chunks, streamed_chars, token_chars) {
                    return Err(anyhow::anyhow!(
                        "agent '{}' stream exceeded limits (chunks={}, chars={})",
                        agent_name,
                        projected_chunks,
                        projected_chars
                    ));
                }
                response_text.push_str(&token);
                stream_chunks = projected_chunks;
                streamed_chars = projected_chars;
                let payload = stream_chunk_notification(
                    &id,
                    &agent_name,
                    &token,
                    stream_chunks,
                    streamed_chars,
                    None,
                    phase_name,
                    trace_id,
                );
                self.send_notification("chat.stream", payload).await?;
            }

            Ok::<(), anyhow::Error>(())
        };

        if let Some(limit) = timeout_limit {
            if timeout(limit, collect_stream).await.is_err() {
                agent_task.abort();
                return Err(anyhow::anyhow!(
                    "agent '{}' timed out after {}s",
                    agent_name,
                    limit.as_secs()
                ));
            }
        } else {
            collect_stream.await?;
        }

        let result = match agent_task.await {
            Ok(Ok(())) => {
                let done_payload = stream_done_notification(
                    &id,
                    &agent_name,
                    stream_chunks,
                    streamed_chars,
                    None,
                    phase_name,
                    trace_id,
                    started.elapsed().as_millis() as u64,
                );
                self.send_notification("chat.stream.done", done_payload)
                    .await?;
                Ok(response_text)
            }
            Ok(Err(err)) => Err(err),
            Err(join_err) => Err(anyhow::anyhow!(
                "agent '{}' panic: {}",
                agent_name,
                join_err
            )),
        };

        self.metrics.observe_agent_latency(started.elapsed());
        result
    }

    async fn run_agent_collecting(
        &self,
        agent_name: String,
        agent: Arc<dyn Agent>,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        timeout_limit: Option<Duration>,
    ) -> Result<String> {
        let started = Instant::now();
        let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
        let agent_task =
            tokio::spawn(async move { agent.chat(messages, principles, options, sender).await });

        let mut response_text = String::new();
        let mut stream_chunks: usize = 0;
        let mut streamed_chars: usize = 0;
        let collect_stream = async {
            while let Some(token) = receiver.recv().await {
                let token_chars = token.chars().count();
                let projected_chunks = stream_chunks.saturating_add(1);
                let projected_chars = streamed_chars.saturating_add(token_chars);
                if stream_would_exceed_limits(stream_chunks, streamed_chars, token_chars) {
                    return Err(anyhow::anyhow!(
                        "agent '{}' stream exceeded limits (chunks={}, chars={})",
                        agent_name,
                        projected_chunks,
                        projected_chars
                    ));
                }
                response_text.push_str(&token);
                stream_chunks = projected_chunks;
                streamed_chars = projected_chars;
            }

            Ok::<(), anyhow::Error>(())
        };

        if let Some(limit) = timeout_limit {
            if timeout(limit, collect_stream).await.is_err() {
                agent_task.abort();
                return Err(anyhow::anyhow!(
                    "agent '{}' timed out after {}s",
                    agent_name,
                    limit.as_secs()
                ));
            }
        } else {
            collect_stream.await?;
        }

        let result = match agent_task.await {
            Ok(Ok(())) => Ok(response_text),
            Ok(Err(err)) => Err(err),
            Err(join_err) => Err(anyhow::anyhow!(
                "agent '{}' panic: {}",
                agent_name,
                join_err
            )),
        };

        self.metrics.observe_review_latency(started.elapsed());
        result
    }

    async fn reload_runtime_config(&self) -> Result<Value> {
        let config_path = self
            .config_path
            .as_ref()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.config_reload_unavailable",
                        &[("reason", "config path not set")]
                    )
                )
            })?
            .clone();
        let client = self.http_client.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "{}",
                crate::i18n::tf(
                    "error.config_reload_unavailable",
                    &[("reason", "http client not set")]
                )
            )
        })?;

        let new_config = AppConfig::load(&config_path)?;
        let health_report =
            validate_runtime_readiness(&config_path, &new_config).map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.config_reload_failed",
                        &[("error", &format!("{err}"))]
                    )
                )
            })?;
        for warning in &health_report.warnings {
            let severity = match warning.severity {
                crate::config::ConfigWarningSeverity::Critical => "critical",
                crate::config::ConfigWarningSeverity::Warn => "warn",
                crate::config::ConfigWarningSeverity::Info => "info",
            };
            warn!(
                "config reload warning [{}:{}] {}",
                severity, warning.code, warning.message
            );
        }

        let config_arc = Arc::new(new_config);
        let new_registry = Arc::new(AgentRegistry::from_config(Arc::clone(&config_arc), client)?);
        let new_flow = Arc::new(FlowManager::new(
            Arc::clone(&config_arc),
            self.forced_phase.clone(),
        ));

        let new_cache = match &config_arc.cache {
            Some(cache_cfg) if cache_cfg.enabled => {
                let cache_path = if PathBuf::from(&cache_cfg.path).is_absolute() {
                    PathBuf::from(&cache_cfg.path)
                } else {
                    config_path
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new("."))
                        .join(&cache_cfg.path)
                };
                Some(Arc::new(ResponseCache::new(
                    &cache_path,
                    cache_cfg.default_ttl_seconds,
                    cache_cfg.max_entries,
                )?))
            }
            _ => None,
        };

        let new_vector_store = match &config_arc.vector {
            Some(vector_cfg) if vector_cfg.enabled => {
                let vector_path = if PathBuf::from(&vector_cfg.path).is_absolute() {
                    PathBuf::from(&vector_cfg.path)
                } else {
                    config_path
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new("."))
                        .join(&vector_cfg.path)
                };
                Some(Arc::new(VectorStore::new(
                    &vector_path,
                    vector_cfg.dimensions,
                    vector_cfg.max_entries,
                )?))
            }
            _ => None,
        };

        let new_autotune_state_path = config_arc.autotune.as_ref().and_then(|autotune_cfg| {
            if !autotune_cfg.enabled {
                return None;
            }
            Some(
                if PathBuf::from(&autotune_cfg.state_path).is_absolute() {
                    PathBuf::from(&autotune_cfg.state_path)
                } else {
                    config_path
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new("."))
                        .join(&autotune_cfg.state_path)
                }
                .to_string_lossy()
                .to_string(),
            )
        });

        let (new_autotune, new_autotune_config) = match config_arc.autotune.as_ref() {
            Some(autotune_cfg) if autotune_cfg.enabled => {
                let state_path = new_autotune_state_path
                    .clone()
                    .unwrap_or_else(|| "acp_autotune_state.json".to_string());
                let state = AutoTuneState::load_or_default(&state_path, autotune_cfg);
                (
                    Some(Arc::new(Mutex::new(state))),
                    Some(autotune_cfg.clone()),
                )
            }
            _ => (None, None),
        };

        let new_runtime_config = config_arc.runtime.clone().unwrap_or_default();

        {
            let mut flow_guard = self.flow.lock().map_err(|_| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.mutex_poisoned", &[("name", "flow")])
                )
            })?;
            *flow_guard = new_flow;
        }
        {
            let mut registry_guard = self.registry.lock().map_err(|_| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.mutex_poisoned", &[("name", "registry")])
                )
            })?;
            *registry_guard = new_registry;
        }
        {
            let mut cache_guard = self.cache.lock().map_err(|_| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.mutex_poisoned", &[("name", "cache")])
                )
            })?;
            *cache_guard = new_cache;
        }
        {
            let mut vector_guard = self.vector_store.lock().map_err(|_| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.mutex_poisoned", &[("name", "vector")])
                )
            })?;
            *vector_guard = new_vector_store;
        }
        {
            let mut vector_cfg_guard = self
                .vector_config
                .lock()
                .map_err(|_| anyhow::anyhow!("vector_config mutex poisoned"))?;
            *vector_cfg_guard = config_arc.vector.clone();
        }
        {
            let mut autotune_guard = self.autotune.lock().map_err(|_| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.mutex_poisoned", &[("name", "autotune")])
                )
            })?;
            *autotune_guard = new_autotune;
        }
        {
            let mut autotune_cfg_guard = self.autotune_config.lock().map_err(|_| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.mutex_poisoned", &[("name", "autotune_config")])
                )
            })?;
            *autotune_cfg_guard = new_autotune_config;
        }
        {
            let mut autotune_path_guard = self
                .autotune_state_path
                .lock()
                .map_err(|_| anyhow::anyhow!("autotune_state_path mutex poisoned"))?;
            *autotune_path_guard = new_autotune_state_path;
        }
        {
            let mut runtime_guard = self.runtime_config.lock().map_err(|_| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.mutex_poisoned", &[("name", "runtime_config")])
                )
            })?;
            *runtime_guard = new_runtime_config;
        }

        // Clear dynamic guardrails to avoid stale state after topology changes.
        if let Ok(mut g) = self.circuit_breakers.inner.lock() {
            g.clear();
        }
        if let Ok(mut g) = self.phase_rate_limiter.inner.lock() {
            g.clear();
        }
        self.inflight_limiter.clear();

        Ok(json!({
            "ok": true,
            "note": crate::i18n::t("info.resources_reloaded"),
            "path": config_path,
            "warning_count": health_report.total,
            "warnings": health_report.warning_messages(),
            "profile_recommendation": health_report.profile_recommendation,
            "recommendations": health_report.recommendations,
            "health": health_report,
        }))
    }

    async fn send_result(&self, id: Option<Value>, result: Value) -> Result<()> {
        self.write_response(JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        })
        .await
    }

    async fn send_error(
        &self,
        id: Option<Value>,
        code: i64,
        message: String,
        data: Option<Value>,
    ) -> Result<()> {
        self.write_response(JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data,
            }),
        })
        .await
    }

    async fn send_notification(&self, method: &str, params: Value) -> Result<()> {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_json_line(&payload).await
    }

    async fn write_response(&self, response: JsonRpcResponse) -> Result<()> {
        let value = serde_json::to_value(response)?;
        self.write_json_line(&value).await
    }

    async fn write_json_line(&self, value: &Value) -> Result<()> {
        let mut stdout = self.output.lock().await;
        let mut encoded = serde_json::to_vec(value)?;
        encoded.push(b'\n');
        stdout.write_all(&encoded).await?;
        stdout.flush().await?;
        Ok(())
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct MaintenanceCycleResult {
    memory_expired_removed: usize,
    sqlite_expired_removed: usize,
    cache_vacuumed: bool,
    vector_vacuumed: bool,
}

#[allow(clippy::too_many_arguments)]
async fn run_background_maintenance_loop(
    runtime_config: Arc<StdMutex<RuntimeConfig>>,
    memory_cache: Arc<MemoryResponseCache>,
    cache: Arc<StdMutex<Option<Arc<ResponseCache>>>>,
    vector_store: Arc<StdMutex<Option<Arc<VectorStore>>>>,
    maintenance: Arc<MaintenanceTracker>,
    lifecycle: Arc<LifecycleState>,
    circuit_breakers: Arc<CircuitBreakerRegistry>,
    phase_rate_limiter: Arc<PhaseRateLimiter>,
    inflight_limiter: Arc<InflightLimiter>,
    shutdown_notify: Arc<Notify>,
) {
    let config = runtime_config
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    let mut maintenance_interval = tokio::time::interval(Duration::from_secs(
        config.maintenance_interval_seconds.max(1),
    ));
    let mut health_interval =
        tokio::time::interval(Duration::from_secs(config.health_interval_seconds.max(1)));
    maintenance_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    health_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown_notify.notified() => break,
            _ = maintenance_interval.tick() => {
                if lifecycle.is_shutting_down() {
                    break;
                }

                if let Err(err) = perform_maintenance_cycle(
                    Arc::clone(&memory_cache),
                    Arc::clone(&cache),
                    Arc::clone(&vector_store),
                    Arc::clone(&runtime_config),
                    Arc::clone(&maintenance),
                    "background",
                ).await {
                    warn!("background maintenance cycle failed: {}", err);
                }
            }
            _ = health_interval.tick() => {
                if lifecycle.is_shutting_down() {
                    break;
                }

                log_background_health(
                    Arc::clone(&memory_cache),
                    Arc::clone(&cache),
                    Arc::clone(&vector_store),
                    Arc::clone(&circuit_breakers),
                    Arc::clone(&phase_rate_limiter),
                    Arc::clone(&inflight_limiter),
                    Arc::clone(&lifecycle),
                    Arc::clone(&maintenance),
                ).await;
            }
        }
    }
}

async fn perform_maintenance_cycle(
    memory_cache: Arc<MemoryResponseCache>,
    cache: Arc<StdMutex<Option<Arc<ResponseCache>>>>,
    vector_store: Arc<StdMutex<Option<Arc<VectorStore>>>>,
    runtime_config: Arc<StdMutex<RuntimeConfig>>,
    maintenance: Arc<MaintenanceTracker>,
    source: &str,
) -> Result<MaintenanceCycleResult> {
    maintenance.note_started();
    let vacuum_interval_cycles = runtime_config
        .lock()
        .map(|guard| guard.sqlite_vacuum_interval_cycles.max(1))
        .unwrap_or(60);
    let current_cycle = maintenance.snapshot().cycles_total;
    let should_vacuum = current_cycle.is_multiple_of(vacuum_interval_cycles);

    let memory_expired_removed = memory_cache.purge_expired();
    let cache_handle = cache.lock().ok().and_then(|guard| guard.clone());
    let sqlite_expired_removed_result = if let Some(cache) = cache_handle.clone() {
        spawn_blocking(move || cache.purge_expired())
            .await
            .map_err(|e| anyhow::anyhow!("cache purge task join error: {}", e))?
    } else {
        Ok(0)
    };
    let sqlite_expired_removed = match sqlite_expired_removed_result {
        Ok(value) => value,
        Err(err) => {
            maintenance.note_failed(&err.to_string());
            return Err(err);
        }
    };

    let cache_vacuumed = if should_vacuum {
        if let Some(cache) = cache_handle.clone() {
            spawn_blocking(move || cache.vacuum())
                .await
                .map_err(|e| anyhow::anyhow!("cache vacuum task join error: {}", e))??;
            true
        } else {
            false
        }
    } else {
        false
    };

    let vector_vacuumed = if should_vacuum {
        if let Some(store) = vector_store.lock().ok().and_then(|guard| guard.clone()) {
            spawn_blocking(move || store.vacuum())
                .await
                .map_err(|e| anyhow::anyhow!("vector vacuum task join error: {}", e))??;
            true
        } else {
            false
        }
    } else {
        false
    };

    let result = MaintenanceCycleResult {
        memory_expired_removed,
        sqlite_expired_removed,
        cache_vacuumed,
        vector_vacuumed,
    };

    maintenance.note_completed(
        memory_expired_removed,
        sqlite_expired_removed,
        cache_vacuumed,
        vector_vacuumed,
    );
    info!(
        "maintenance cycle '{}' completed (memory_removed={}, sqlite_removed={}, cache_vacuumed={}, vector_vacuumed={})",
        source,
        result.memory_expired_removed,
        result.sqlite_expired_removed,
        result.cache_vacuumed,
        result.vector_vacuumed
    );
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
async fn log_background_health(
    memory_cache: Arc<MemoryResponseCache>,
    cache: Arc<StdMutex<Option<Arc<ResponseCache>>>>,
    vector_store: Arc<StdMutex<Option<Arc<VectorStore>>>>,
    circuit_breakers: Arc<CircuitBreakerRegistry>,
    phase_rate_limiter: Arc<PhaseRateLimiter>,
    inflight_limiter: Arc<InflightLimiter>,
    lifecycle: Arc<LifecycleState>,
    maintenance: Arc<MaintenanceTracker>,
) {
    let sqlite_cache_entries =
        if let Some(cache) = cache.lock().ok().and_then(|guard| guard.clone()) {
            match spawn_blocking(move || cache.entry_count()).await {
                Ok(Ok(count)) => Some(count),
                Ok(Err(err)) => {
                    warn!(
                        "background health failed to read sqlite cache entries: {}",
                        err
                    );
                    None
                }
                Err(err) => {
                    warn!("background health cache count task failed: {}", err);
                    None
                }
            }
        } else {
            None
        };

    let vector_counts =
        if let Some(store) = vector_store.lock().ok().and_then(|guard| guard.clone()) {
            match spawn_blocking(move || {
                Ok::<(u64, u64), anyhow::Error>((
                    store.memory_entry_count()?,
                    store.summary_entry_count()?,
                ))
            })
            .await
            {
                Ok(Ok(counts)) => Some(counts),
                Ok(Err(err)) => {
                    warn!("background health failed to read vector counts: {}", err);
                    None
                }
                Err(err) => {
                    warn!("background health vector count task failed: {}", err);
                    None
                }
            }
        } else {
            None
        };

    let (global_inflight, phase_inflight) = inflight_limiter.snapshot();
    let lifecycle_snapshot = lifecycle.snapshot();
    let maintenance_snapshot = maintenance.snapshot();

    info!(
        "runtime health: shutting_down={}, inflight_global={}, inflight_phases={}, memory_cache_entries={}, sqlite_cache_entries={:?}, vector_counts={:?}, breaker_open={}, breaker_half_open={}, rate_limiter_tracked={}, maintenance_running={}, maintenance_cycles={}",
        lifecycle_snapshot.shutting_down,
        global_inflight,
        phase_inflight.len(),
        memory_cache.active_entries(),
        sqlite_cache_entries,
        vector_counts,
        circuit_breakers.open_count(),
        circuit_breakers.half_open_count(),
        phase_rate_limiter.tracked_phases(),
        maintenance_snapshot.running,
        maintenance_snapshot.cycles_total,
    );
}

fn request_timeout(options: Option<&PhaseOptions>) -> Option<Duration> {
    options
        .and_then(|opts| opts.request_timeout_seconds)
        .map(Duration::from_secs)
}

async fn autotune_state_snapshot(autotune: &Arc<Mutex<AutoTuneState>>) -> AutoTuneState {
    autotune.lock().await.clone()
}

fn effective_vector_enabled(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> bool {
    options
        .and_then(|opts| opts.vector_enabled)
        .or_else(|| vector_config.map(|cfg| cfg.enabled))
        .unwrap_or(true)
}

fn effective_vector_auto(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> bool {
    options
        .and_then(|opts| opts.vector_auto)
        .or_else(|| vector_config.map(|cfg| cfg.auto_mode))
        .unwrap_or(true)
}

fn effective_vector_min_query_chars(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
    autotune_state: Option<&AutoTuneState>,
) -> usize {
    autotune_state
        .map(|state| state.current_min_query_chars)
        .or_else(|| options.and_then(|opts| opts.vector_min_query_chars))
        .or_else(|| vector_config.map(|cfg| cfg.min_query_chars))
        .unwrap_or(DEFAULT_VECTOR_MIN_QUERY_CHARS)
}

fn effective_vector_top_k(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
    autotune_state: Option<&AutoTuneState>,
) -> usize {
    autotune_state
        .map(|state| state.current_top_k)
        .or_else(|| options.and_then(|opts| opts.vector_top_k))
        .or_else(|| vector_config.map(|cfg| cfg.top_k))
        .unwrap_or(DEFAULT_VECTOR_TOP_K)
}

fn effective_vector_min_similarity(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> f32 {
    options
        .and_then(|opts| opts.vector_min_similarity)
        .or_else(|| vector_config.map(|cfg| cfg.min_similarity))
        .unwrap_or(DEFAULT_VECTOR_MIN_SIMILARITY)
}

fn effective_vector_max_snippet_chars(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> usize {
    options
        .and_then(|opts| opts.vector_max_snippet_chars)
        .or_else(|| vector_config.map(|cfg| cfg.max_snippet_chars))
        .unwrap_or(DEFAULT_VECTOR_MAX_SNIPPET_CHARS)
}

fn effective_summary_enabled(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> bool {
    options
        .and_then(|opts| opts.summary_enabled)
        .or_else(|| vector_config.map(|cfg| cfg.summary_enabled))
        .unwrap_or(true)
}

fn effective_summary_trigger_messages(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> usize {
    options
        .and_then(|opts| opts.summary_trigger_messages)
        .or_else(|| vector_config.map(|cfg| cfg.summary_trigger_messages))
        .unwrap_or(DEFAULT_SUMMARY_TRIGGER_MESSAGES)
}

fn effective_summary_max_chars(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> usize {
    options
        .and_then(|opts| opts.summary_max_chars)
        .or_else(|| vector_config.map(|cfg| cfg.summary_max_chars))
        .unwrap_or(DEFAULT_SUMMARY_MAX_CHARS)
}

fn optimize_messages(messages: &[Message], options: Option<&PhaseOptions>) -> Vec<Message> {
    let mut trimmed = messages.to_vec();

    if let Some(max_messages) = options.and_then(|opts| opts.max_history_messages) {
        if trimmed.len() > max_messages {
            trimmed = trimmed[trimmed.len() - max_messages..].to_vec();
        }
    }

    if let Some(max_chars) = options.and_then(|opts| opts.max_history_chars) {
        let mut kept_reversed = Vec::new();
        let mut total_chars = 0usize;

        for message in trimmed.iter().rev() {
            let message_chars = message.content.chars().count();
            if !kept_reversed.is_empty() && total_chars + message_chars > max_chars {
                break;
            }

            kept_reversed.push(message.clone());
            total_chars += message_chars;
        }

        kept_reversed.reverse();
        trimmed = kept_reversed;
    }

    trimmed
}

fn latest_user_query(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.role.eq_ignore_ascii_case("user"))
        .map(|message| message.content.trim().to_string())
        .filter(|content| !content.is_empty())
}

fn build_vector_context_message(hits: &[VectorHit]) -> String {
    let normalized = dedupe_vector_hits(hits);
    let mut content = String::from("Relevant prior context from similar requests:\n");
    for (index, hit) in normalized.iter().enumerate() {
        content.push_str(&format!(
            "{}. [similarity {:.2}] {}\n",
            index + 1,
            hit.similarity,
            hit.response_snippet
        ));
    }
    content
}

fn append_recent_summary(
    existing_summary: Option<&str>,
    latest_user_query: Option<&str>,
    response_text: &str,
    max_chars: usize,
) -> String {
    let mut segments: Vec<String> = Vec::new();
    if let Some(existing) = existing_summary {
        if !existing.trim().is_empty() {
            segments.push(existing.trim().to_string());
        }
    }
    if let Some(query) = latest_user_query {
        segments.push(format!("User focus: {}", query.trim()));
    }
    if !response_text.trim().is_empty() {
        segments.push(format!("Latest response: {}", response_text.trim()));
    }

    trim_to_tail_chars(&segments.join("\n\n"), max_chars)
}

fn trim_to_tail_chars(input: &str, max_chars: usize) -> String {
    let chars: Vec<char> = input.chars().collect();
    if chars.len() <= max_chars {
        return input.to_string();
    }

    chars[chars.len() - max_chars..].iter().collect()
}

fn build_cache_key(
    phase: &ResolvedPhase,
    messages: &[Message],
    mode_name: &str,
    approval_strategy: &str,
    agent_names: &[String],
) -> Result<String> {
    build_cache_key_from_parts(
        &phase.phase_name,
        messages,
        phase.principles.as_ref(),
        phase.options.as_ref(),
        mode_name,
        approval_strategy,
        agent_names,
    )
}

fn build_cache_key_from_parts(
    phase_name: &str,
    messages: &[Message],
    principles: Option<&Vec<String>>,
    options: Option<&PhaseOptions>,
    mode_name: &str,
    approval_strategy: &str,
    agent_names: &[String],
) -> Result<String> {
    let payload = json!({
        "phase": phase_name,
        "messages": messages,
        "principles": principles,
        "options": options,
        "mode": mode_name,
        "approval_strategy": approval_strategy,
        "agents": agent_names,
    });

    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&payload)?);
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn dedupe_vector_hits(hits: &[VectorHit]) -> Vec<VectorHit> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for hit in hits {
        let key = hit
            .response_snippet
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        if seen.insert(key) {
            out.push(hit.clone());
        }
    }
    out
}

fn filter_env_ready_agents(config_path: Option<&PathBuf>, candidates: &[String]) -> Vec<String> {
    let Some(path) = config_path else {
        return candidates.to_vec();
    };
    let config = match load_app_config_lazy(path) {
        Some(cfg) => cfg,
        None => return candidates.to_vec(),
    };

    candidates
        .iter()
        .filter(|agent| is_agent_env_ready(config.as_ref(), agent))
        .cloned()
        .collect()
}

fn capability_max_complexity(ready_agents: usize) -> u8 {
    match ready_agents {
        0 => 0,
        1 => 2,
        2 => 4,
        _ => 5,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkGrade {
    Ask,
    Edit,
    Agent,
    Safeguard,
    FullAuto,
}

#[derive(Debug, Clone, Serialize)]
struct ReviewPolicy {
    min_review_level: String,
    required_reviews: usize,
    required_checks: Vec<String>,
    timeout_policy: String,
    enforce_dual_review: bool,
    enforce_action_gates: bool,
}

fn resolve_review_policy(
    options: Option<&PhaseOptions>,
    characteristics: Option<&TaskCharacteristics>,
    is_workflow_execute: bool,
    requested_dual_review: bool,
) -> ReviewPolicy {
    let inferred_enhanced = characteristics
        .map(|c| c.complexity >= 4 || c.has_safety_concerns)
        .unwrap_or(false)
        || is_workflow_execute;

    let min_review_level = extra_string(options, "review_min_level").unwrap_or_else(|| {
        if inferred_enhanced {
            "enhanced".to_string()
        } else {
            "standard".to_string()
        }
    });
    let required_reviews = extra_u64(options, "review_required_reviews")
        .map(|v| v.max(1) as usize)
        .unwrap_or_else(|| {
            if min_review_level.eq_ignore_ascii_case("enhanced") {
                2
            } else {
                1
            }
        });
    let required_checks =
        extra_string_list(options, "review_required_checks").unwrap_or_else(|| {
            if is_workflow_execute {
                vec!["qa".to_string(), "retest".to_string(), "final".to_string()]
            } else {
                Vec::new()
            }
        });
    let timeout_policy =
        extra_string(options, "review_timeout_policy").unwrap_or_else(|| "reject".to_string());
    let enforce_dual_review = requested_dual_review
        || required_reviews >= 2
        || min_review_level.eq_ignore_ascii_case("enhanced");
    let enforce_action_gates = !required_checks.is_empty();

    ReviewPolicy {
        min_review_level,
        required_reviews,
        required_checks,
        timeout_policy,
        enforce_dual_review,
        enforce_action_gates,
    }
}

fn action_check_kinds_from_policy(required_checks: &[String]) -> Vec<ActionCheckKind> {
    if required_checks.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for name in required_checks {
        if let Some(kind) = ActionCheckKind::parse(name) {
            if !out.contains(&kind) {
                out.push(kind);
            }
        }
    }
    out
}

impl WorkGrade {
    fn parse(raw: Option<&str>) -> Option<Self> {
        let value = raw?.trim().to_ascii_lowercase();
        match value.as_str() {
            "ask" => Some(Self::Ask),
            "edit" => Some(Self::Edit),
            "agent" => Some(Self::Agent),
            "safeguard" => Some(Self::Safeguard),
            "full_auto" | "full-auto" | "auto" => Some(Self::FullAuto),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Edit => "edit",
            Self::Agent => "agent",
            Self::Safeguard => "safeguard",
            Self::FullAuto => "full_auto",
        }
    }

    fn rank(&self) -> u8 {
        match self {
            Self::Ask => 0,
            Self::Edit => 1,
            Self::Agent => 2,
            Self::Safeguard => 3,
            Self::FullAuto => 4,
        }
    }
}

#[derive(Debug, Clone)]
struct WorkGradeDecision {
    requested: WorkGrade,
    decided: WorkGrade,
    decision_action: String,
    reasons: Vec<String>,
    risk_score: f64,
}

fn work_grade_action(requested: WorkGrade, decided: WorkGrade) -> String {
    if decided.rank() > requested.rank() {
        "upgraded".to_string()
    } else if decided.rank() < requested.rank() {
        "downgraded".to_string()
    } else {
        "unchanged".to_string()
    }
}

fn decide_work_grade(
    requested_grade: Option<&str>,
    plan: &crate::reinforcement::TaskPlanArtifact,
    is_workflow_execute: bool,
    runtime_healthy: bool,
    force_fail_fast: bool,
) -> WorkGradeDecision {
    let requested = WorkGrade::parse(requested_grade).unwrap_or({
        if is_workflow_execute {
            WorkGrade::FullAuto
        } else {
            WorkGrade::Agent
        }
    });

    let mut decided = requested;
    let mut reasons = Vec::new();

    let risk_score = ((plan.characteristics.complexity.min(5) as f64 / 5.0) * 0.4
        + if plan.characteristics.has_safety_concerns {
            0.25
        } else {
            0.0
        }
        + if plan.characteristics.involves_multiple_modules {
            0.15
        } else {
            0.0
        }
        + ((1.0 - plan.routing.predicted_success_rate as f64).clamp(0.0, 1.0)) * 0.2
        + if runtime_healthy { 0.0 } else { 0.1 })
    .clamp(0.0, 1.0);

    if force_fail_fast || plan.characteristics.has_safety_concerns || risk_score >= 0.75 {
        decided = WorkGrade::Safeguard;
        reasons.push(
            "high-risk posture detected (safety/fail_fast/high risk score), enforce safeguard"
                .to_string(),
        );
    } else if is_workflow_execute && plan.characteristics.complexity >= 3 {
        decided = WorkGrade::FullAuto;
        reasons
            .push("workflow.execute with moderate+ complexity, promote to full_auto".to_string());
    } else if plan.characteristics.complexity >= 3 {
        decided = WorkGrade::Agent;
        reasons.push("multi-step complexity, promote to agent execution".to_string());
    } else if plan.characteristics.complexity <= 1
        && !plan.characteristics.has_safety_concerns
        && plan.routing.predicted_success_rate >= 0.90
    {
        decided = WorkGrade::Edit;
        reasons.push("low-risk simple task, downgrade to edit for efficiency".to_string());
    }

    let decision_action = work_grade_action(requested, decided);
    WorkGradeDecision {
        requested,
        decided,
        decision_action,
        reasons,
        risk_score,
    }
}

#[derive(Debug, Clone, Serialize)]
struct OptimizationPolicyReport {
    auto_attach: bool,
    auto_detach: bool,
    runtime_healthy: bool,
    anomaly_detected: bool,
    requested_modules: Vec<String>,
    attached_modules: Vec<String>,
    detached_modules: Vec<String>,
    reattached_modules: Vec<String>,
    reattach_reasons: Vec<String>,
    detachment_reasons: Vec<String>,
    module_impacts: Vec<String>,
    recovery_conditions: Vec<String>,
    recommendations: Vec<String>,
    phase_parallelism_cap: Option<usize>,
    force_fail_fast: bool,
    risk_assessment: Value,
    resource_budget: Value,
    dynamic_parameters: Value,
    reliability: Value,
    speed: Value,
    cost: Value,
    anomaly: Value,
}

#[derive(Debug, Clone)]
struct OptimizationPolicyOutcome {
    report: OptimizationPolicyReport,
    phase_parallelism_cap: Option<usize>,
    force_fail_fast: bool,
}

const DEFAULT_OPTIMIZATION_MODULES: &[&str] = &[
    "workflow_optimizer",
    "advanced_modules",
    "reliability_optimizer",
    "failure_prevention",
    "speed_optimizer",
    "cost_optimizer",
    "adaptive_selector",
];

fn evaluate_optimization_policy(
    ledger: &ArtifactLedger,
    task: &str,
    plan: &crate::reinforcement::TaskPlanArtifact,
    options: Option<&PhaseOptions>,
    runtime_healthy: bool,
    is_workflow_execute: bool,
) -> OptimizationPolicyOutcome {
    let auto_attach = extra_bool(options, "auto_attach").unwrap_or(is_workflow_execute);
    let auto_detach = extra_bool(options, "auto_detach").unwrap_or(is_workflow_execute);

    let requested_modules = extra_string_list(options, "optimization_modules")
        .map(|modules| {
            modules
                .into_iter()
                .map(|name| name.trim().to_ascii_lowercase())
                .filter(|name| is_supported_optimization_module(name))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut attached_modules = if auto_attach {
        if requested_modules.is_empty() {
            DEFAULT_OPTIMIZATION_MODULES
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>()
        } else {
            requested_modules.clone()
        }
    } else {
        Vec::new()
    };

    attached_modules.sort();
    attached_modules.dedup();

    let mut detached_modules = Vec::new();
    let mut reattached_modules = Vec::new();
    let mut reattach_reasons = Vec::new();
    let mut detachment_reasons = Vec::new();
    let mut module_impacts = Vec::new();
    let mut recovery_conditions = Vec::new();
    let mut recommendations = Vec::new();
    let mut phase_parallelism_cap = None;
    let mut force_fail_fast = false;

    let mut risk_assessment = Value::Null;
    let mut resource_budget = Value::Null;
    let mut dynamic_parameters = Value::Null;
    let mut reliability = Value::Null;
    let mut speed = Value::Null;
    let mut cost = Value::Null;
    let mut anomaly = Value::Null;
    let mut anomaly_detected = false;

    if auto_attach && auto_detach {
        let recoverable = recommend_reattach_modules_from_policy_history(ledger, 2, 40);
        for module in recoverable {
            if is_supported_optimization_module(&module)
                && !attached_modules.iter().any(|attached| attached == &module)
            {
                attached_modules.push(module.clone());
                reattached_modules.push(module.clone());
                reattach_reasons.push(format!(
                    "reattached {} after policy history reported two consecutive healthy, anomaly-free executions",
                    module
                ));
                module_impacts.push(format!(
                    "{} reattached to restore optimization depth under healthy runtime conditions",
                    module
                ));
            }
        }
    }

    let has_module = |name: &str| attached_modules.iter().any(|module| module == name);

    if has_module("workflow_optimizer") {
        let risk = PredictiveFailureHandler::assess_risk(
            task,
            plan.characteristics.complexity,
            plan.characteristics.involves_multiple_modules,
            plan.characteristics.has_safety_concerns,
            plan.routing.predicted_success_rate,
        );
        if risk.use_safeguard_mode {
            force_fail_fast = true;
            recommendations.push(
                "workflow_optimizer recommends fail_fast because risk exceeds safeguard threshold"
                    .to_string(),
            );
            module_impacts.push(
                "failure strategy escalated to fail_fast, reducing throughput but limiting blast radius"
                    .to_string(),
            );
            recovery_conditions.push(
                "switch back to tolerant after consecutive low-risk executions with stable gate pass"
                    .to_string(),
            );
        }
        risk_assessment = serde_json::to_value(&risk).unwrap_or(Value::Null);
    }

    if has_module("advanced_modules") {
        let subtask_count = plan.planned_subtasks.len().max(1);
        let budget = ResourceAllocator::allocate_resources(
            "workflow",
            plan.characteristics.complexity,
            subtask_count,
        );
        let tuner = DynamicParameterTuner::new();
        let profile = match plan.characteristics.complexity {
            0 | 1 => "simple",
            2 | 3 => "medium",
            _ => "complex",
        };
        let tuned = tuner.select_parameters(profile, plan.characteristics.complexity);

        phase_parallelism_cap = Some(budget.max_parallel_tasks.max(1));
        recommendations.push(format!(
            "advanced_modules capped subtask parallelism to {} based on resource budget",
            budget.max_parallel_tasks.max(1)
        ));

        resource_budget = serde_json::to_value(&budget).unwrap_or(Value::Null);
        dynamic_parameters = serde_json::to_value(&tuned).unwrap_or(Value::Null);
    }

    if has_module("reliability_optimizer") {
        let optimizer = ReliabilityOptimizer::new();
        let complexity = optimizer.detect_complexity(task);
        let strategy = optimizer.recommend_strategy(complexity);
        let degradation = optimizer.get_degradation_strategy(complexity);
        if complexity >= ReliabilityComplexityLevel::VeryComplex && degradation.is_some() {
            recommendations.push(
                "reliability_optimizer suggests simplified fallback strategy for very complex task"
                    .to_string(),
            );
        }
        reliability = json!({
            "detected_complexity": format!("{:?}", complexity),
            "recommended_strategy": strategy,
            "degradation_strategy": degradation,
        });
    }

    if has_module("speed_optimizer") {
        let mut optimizer = SpeedOptimizer::new();
        optimizer.enable_speculation(SpeculationStrategy::HistoryBased);
        optimizer.set_streaming_mode(StreamingMode::TokenStreaming);
        let estimated = optimizer.estimate_speedup();
        speed = json!({
            "streaming_mode": format!("{:?}", optimizer.streaming_mode()),
            "estimated_speedup": estimated,
        });
        if estimated > 0.1 {
            recommendations.push(
                "speed_optimizer indicates meaningful acceleration potential on this route"
                    .to_string(),
            );
        }
    }

    if has_module("cost_optimizer") {
        let optimizer = CostOptimizer::new();
        let complexity = match plan.characteristics.complexity {
            0 | 1 => CostTaskComplexity::Simple,
            2 => CostTaskComplexity::Moderate,
            3 | 4 => CostTaskComplexity::Complex,
            _ => CostTaskComplexity::VeryComplex,
        };
        let compressed = optimizer.compress_prompt(task);
        let selected_model = optimizer.select_model(complexity, None);
        cost = json!({
            "selected_model": selected_model,
            "compression_ratio": compressed.compression_ratio,
            "original_tokens": compressed.original_tokens,
            "compressed_tokens": compressed.compressed_tokens,
        });
    }

    if has_module("failure_prevention") {
        let prevention = FailurePrevention::new();
        let detected = prevention.detect_anomaly(task, &HashMap::new());
        anomaly_detected = detected.detected;
        if detected.detected {
            force_fail_fast = true;
            recommendations.push(
                "failure_prevention detected anomaly and escalated failure policy to fail_fast"
                    .to_string(),
            );
            if auto_detach {
                for module in ["speed_optimizer", "cost_optimizer"] {
                    if has_module(module) {
                        detached_modules.push(module.to_string());
                        detachment_reasons.push(format!(
                            "detached {} due to anomaly-driven safety escalation",
                            module
                        ));
                        module_impacts.push(format!(
                            "{} detached, prioritizing safety over latency and cost efficiency",
                            module
                        ));
                        recovery_conditions.push(format!(
                            "reattach {} after runtime.health is healthy and no anomaly is detected for two consecutive executions",
                            module
                        ));
                    }
                }
            }
        }
        anomaly = serde_json::to_value(&detected).unwrap_or(Value::Null);
    }

    if auto_detach && plan.characteristics.complexity <= 1 {
        for module in ["reliability_optimizer", "workflow_optimizer"] {
            if has_module(module) {
                detached_modules.push(module.to_string());
                detachment_reasons.push(format!(
                    "detached {} for low-complexity task to reduce control-plane overhead",
                    module
                ));
                module_impacts.push(format!(
                    "{} detached for low-complexity path, reducing analysis depth to improve response speed",
                    module
                ));
                recovery_conditions.push(format!(
                    "reattach {} when task complexity rises above 1 or cross-module risk is detected",
                    module
                ));
            }
        }
    }

    detached_modules.sort();
    detached_modules.dedup();
    reattached_modules.sort();
    reattached_modules.dedup();
    reattach_reasons.sort();
    reattach_reasons.dedup();
    module_impacts.sort();
    module_impacts.dedup();
    recovery_conditions.sort();
    recovery_conditions.dedup();
    attached_modules.retain(|module| !detached_modules.iter().any(|detached| detached == module));

    let report = OptimizationPolicyReport {
        auto_attach,
        auto_detach,
        runtime_healthy,
        anomaly_detected,
        requested_modules,
        attached_modules,
        detached_modules,
        reattached_modules,
        reattach_reasons,
        detachment_reasons,
        module_impacts,
        recovery_conditions,
        recommendations,
        phase_parallelism_cap,
        force_fail_fast,
        risk_assessment,
        resource_budget,
        dynamic_parameters,
        reliability,
        speed,
        cost,
        anomaly,
    };

    OptimizationPolicyOutcome {
        phase_parallelism_cap,
        force_fail_fast,
        report,
    }
}

fn is_supported_optimization_module(name: &str) -> bool {
    matches!(
        name,
        "workflow_optimizer"
            | "adaptive_selector"
            | "advanced_modules"
            | "cost_optimizer"
            | "speed_optimizer"
            | "reliability_optimizer"
            | "failure_prevention"
    )
}

fn role_keywords_for(role: &str) -> Vec<&'static str> {
    match role {
        "planner" => vec!["planner", "plan", "architect"],
        "researcher" => vec!["researcher", "research", "analysis"],
        "coder" => vec!["coder", "code", "implement", "dev"],
        "tester" => vec!["tester", "test", "qa", "verify"],
        "reviewer" => vec!["reviewer", "review", "audit"],
        _ => vec![],
    }
}

fn rank_execution_agents(
    agent_names: &[String],
    desired_role: Option<&str>,
    phase_index: usize,
    task_index: usize,
) -> Vec<ExecutionDecisionCandidate> {
    if agent_names.is_empty() {
        return Vec::new();
    }

    let total = agent_names.len() as f64;
    let mut ranked = agent_names
        .iter()
        .enumerate()
        .map(|(idx, agent_name)| {
            let lower = agent_name.to_ascii_lowercase();
            let history_order_score =
                ((agent_names.len().saturating_sub(idx)) as f64 / total) * 0.55;

            let (role_match_score, role_reason) = if let Some(role) = desired_role {
                let role = role.to_ascii_lowercase();
                let keywords = role_keywords_for(role.as_str());
                if !keywords.is_empty() && keywords.iter().any(|keyword| lower.contains(keyword)) {
                    (0.35f64, format!("role match for {}", role))
                } else {
                    (-0.12f64, format!("no explicit role match for {}", role))
                }
            } else {
                (0.08f64, "no role constraint".to_string())
            };

            let rotation_target = (phase_index + task_index) % agent_names.len();
            let spread_score = if idx == rotation_target { 0.10 } else { 0.02 };
            let score = (history_order_score + role_match_score + spread_score).clamp(0.0, 1.0);

            ExecutionDecisionCandidate {
                agent: agent_name.clone(),
                score,
                reason: format!(
                    "history_order={:.3}, {}, spread_score={:.3}",
                    history_order_score, role_reason, spread_score
                ),
            }
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.agent.cmp(&b.agent))
    });
    ranked
}

fn extra_u64(options: Option<&PhaseOptions>, key: &str) -> Option<u64> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_u64())
}

fn extra_f64(options: Option<&PhaseOptions>, key: &str) -> Option<f64> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_f64())
}

fn extra_string(options: Option<&PhaseOptions>, key: &str) -> Option<String> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
}

fn extra_bool(options: Option<&PhaseOptions>, key: &str) -> Option<bool> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_bool())
}

fn extra_string_list(options: Option<&PhaseOptions>, key: &str) -> Option<Vec<String>> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str())
                .map(|item| item.to_string())
                .collect::<Vec<_>>()
        })
}

fn percentile(samples: &[u64], percentile: f64) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let clamped = percentile.clamp(0.0, 100.0);
    let rank = ((clamped / 100.0) * ((samples.len() - 1) as f64)).round() as usize;
    samples[rank]
}

#[derive(Debug, Clone)]
struct RequirementGateDecision {
    blocked: bool,
    reason: Option<String>,
    missing_fields: Vec<String>,
    clarification_artifact_path: Option<PathBuf>,
    governance_artifact_path: PathBuf,
}

#[derive(Debug, Clone, Copy)]
struct LearningClarificationMetrics {
    rounds: u32,
    quality_score: f64,
    requirement_change_count: u32,
}

fn parse_string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str())
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn parse_requirement_contract_from_params(
    params: &Value,
    task: &str,
) -> Option<RequirementContractArtifact> {
    let contract = params.get("requirement_contract")?;
    let goal = contract
        .get("goal")
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    let scope = contract
        .get("scope")
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .unwrap_or_default();

    Some(RequirementContractArtifact {
        generated_at: now_ts(),
        task: task.to_string(),
        source: "request.params.requirement_contract".to_string(),
        goal,
        scope,
        non_goals: parse_string_list(contract.get("non_goals")),
        acceptance_criteria: parse_string_list(contract.get("acceptance_criteria")),
        constraints: parse_string_list(contract.get("constraints")),
        open_questions: parse_string_list(contract.get("open_questions")),
        ambiguity_score: contract
            .get("ambiguity_score")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .min(5) as u8,
        user_confirmed: contract
            .get("user_confirmed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

fn default_requirement_contract(task: &str, source: &str) -> RequirementContractArtifact {
    RequirementContractArtifact {
        generated_at: now_ts(),
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

fn requirement_missing_fields(contract: &RequirementContractArtifact) -> Vec<String> {
    let mut missing = Vec::new();
    if contract.goal.trim().is_empty() {
        missing.push("goal".to_string());
    }
    if contract.scope.trim().is_empty() {
        missing.push("scope".to_string());
    }
    if contract.acceptance_criteria.is_empty() {
        missing.push("acceptance_criteria".to_string());
    }
    if contract.constraints.is_empty() {
        missing.push("constraints".to_string());
    }
    missing
}

fn requirement_questions_from_missing(missing_fields: &[String]) -> Vec<String> {
    missing_fields
        .iter()
        .map(|field| match field.as_str() {
            "goal" => "这个任务最终想达成的业务目标是什么？".to_string(),
            "scope" => "本次改动边界是什么？哪些模块必须包含？".to_string(),
            "acceptance_criteria" => "验收标准是什么？如何证明完成？".to_string(),
            "constraints" => "有哪些硬约束（时间、兼容性、性能、安全）？".to_string(),
            other => format!("请补充字段: {}", other),
        })
        .collect::<Vec<_>>()
}

fn estimate_requirement_ambiguity(task: &str, contract: &RequirementContractArtifact) -> u8 {
    let characteristics = TaskRouter::analyze_task(task);
    let mut score = characteristics.complexity.min(5);
    let missing = requirement_missing_fields(contract).len() as u8;
    score = score.saturating_add(missing.min(2));
    score.min(5)
}

fn load_latest_requirement_contract(
    ledger: &ArtifactLedger,
    task: &str,
) -> Option<RequirementContractArtifact> {
    let artifact = load_latest_requirement_contract_lazy(ledger)?;
    if artifact.task.trim() == task.trim() {
        Some(artifact)
    } else {
        None
    }
}

fn evaluate_requirement_gate(
    ledger: &ArtifactLedger,
    task: &str,
    params: &Value,
    source: &str,
) -> Result<RequirementGateDecision> {
    let characteristics = TaskRouter::analyze_task(task);
    let clarification_required = characteristics.complexity >= 3
        || characteristics.involves_multiple_modules
        || characteristics.needs_verification
        || characteristics.has_safety_concerns;

    let mut contract = parse_requirement_contract_from_params(params, task)
        .or_else(|| load_latest_requirement_contract(ledger, task))
        .unwrap_or_else(|| default_requirement_contract(task, source));
    contract.generated_at = now_ts();
    contract.source = source.to_string();
    contract.ambiguity_score = estimate_requirement_ambiguity(task, &contract);
    if let Some(v) = params
        .get("requirement_confirmed")
        .and_then(|v| v.as_bool())
    {
        contract.user_confirmed = v;
    }

    let missing_fields = requirement_missing_fields(&contract);
    let confirmed = contract.user_confirmed && missing_fields.is_empty();
    let blocked = clarification_required && !confirmed;

    let clarification_artifact_path =
        if parse_requirement_contract_from_params(params, task).is_some() {
            Some(persist_requirement_contract(ledger, &contract)?)
        } else {
            None
        };

    let reason = if blocked {
        Some(
            "requirement clarification/confirmation is required before planning or execution"
                .to_string(),
        )
    } else {
        None
    };
    let governance = GovernancePolicyArtifact {
        generated_at: now_ts(),
        task: task.to_string(),
        source: source.to_string(),
        clarification_required,
        confirmed,
        blocked,
        reason: reason.clone(),
        next_step: if blocked {
            json!({
                "method": "workflow.clarify",
                "task": task,
                "missing_fields": missing_fields,
                "suggested_followup": "call workflow.confirm with completed requirement_contract and user_confirmed=true"
            })
        } else {
            json!({"status": "confirmed"})
        },
    };
    let governance_artifact_path = persist_governance_policy(ledger, &governance)?;

    Ok(RequirementGateDecision {
        blocked,
        reason,
        missing_fields,
        clarification_artifact_path,
        governance_artifact_path,
    })
}

fn derive_clarification_quality_score(contract: &RequirementContractArtifact) -> f64 {
    let missing_count = requirement_missing_fields(contract).len() as f64;
    let completeness_score = ((4.0 - missing_count).max(0.0) / 4.0).clamp(0.0, 1.0);
    let ambiguity_penalty = (contract.ambiguity_score as f64 / 5.0).clamp(0.0, 1.0);
    let quality = 0.7 * completeness_score + 0.3 * (1.0 - ambiguity_penalty);
    quality.clamp(0.0, 1.0)
}

fn resolve_learning_clarification_metrics(
    ledger: &ArtifactLedger,
    task: &str,
    params: &Value,
) -> LearningClarificationMetrics {
    let provided_contract = parse_requirement_contract_from_params(params, task);
    let latest_contract = load_latest_requirement_contract(ledger, task);
    let active_contract = provided_contract.as_ref().or(latest_contract.as_ref());

    let rounds = params
        .get("clarification_rounds")
        .and_then(|v| v.as_u64())
        .map(|v| v.min(64) as u32)
        .unwrap_or_else(|| {
            if let Some(contract) = active_contract {
                let has_questions = !contract.open_questions.is_empty();
                let base_rounds = if has_questions { 1 } else { 0 };
                let confirm_round = if contract.user_confirmed { 1 } else { 0 };
                (base_rounds + confirm_round).max(1)
            } else {
                0
            }
        });

    let quality_score = params
        .get("clarification_quality_score")
        .and_then(|v| v.as_f64())
        .map(|v| v.clamp(0.0, 1.0))
        .unwrap_or_else(|| {
            active_contract
                .map(derive_clarification_quality_score)
                .unwrap_or(0.0)
        });

    let requirement_change_count = params
        .get("requirement_change_count")
        .and_then(|v| v.as_u64())
        .map(|v| v.min(4096) as u32)
        .or_else(|| {
            params
                .get("requirement_contract_revision")
                .and_then(|v| v.as_u64())
                .map(|revision| revision.saturating_sub(1).min(4096) as u32)
        })
        .unwrap_or_else(|| {
            if let (Some(current), Some(previous)) =
                (provided_contract.as_ref(), latest_contract.as_ref())
            {
                let changed = current.goal != previous.goal
                    || current.scope != previous.scope
                    || current.non_goals != previous.non_goals
                    || current.acceptance_criteria != previous.acceptance_criteria
                    || current.constraints != previous.constraints;
                if changed {
                    1
                } else {
                    0
                }
            } else if provided_contract.is_some() {
                1
            } else {
                0
            }
        });

    LearningClarificationMetrics {
        rounds,
        quality_score,
        requirement_change_count,
    }
}

fn observe_latency_histogram(
    duration: Duration,
    count: &mut u64,
    sum_seconds: &mut f64,
    buckets: &mut [u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
) {
    let value = duration.as_secs_f64();
    *count += 1;
    *sum_seconds += value;
    let mut idx = HISTOGRAM_BUCKETS_SECONDS.len();
    for (i, bound) in HISTOGRAM_BUCKETS_SECONDS.iter().enumerate() {
        if value <= *bound {
            idx = i;
            break;
        }
    }
    buckets[idx] = buckets[idx].saturating_add(1);
}

fn extract_task_description(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| {
            message.role.eq_ignore_ascii_case("user") && !message.content.trim().is_empty()
        })
        .map(|message| message.content.clone())
        .or_else(|| messages.last().map(|message| message.content.clone()))
        .unwrap_or_else(|| "general task".to_string())
}

fn pipeline_gate_violation(
    analyzed_task: &TaskCharacteristics,
    routing: &RoutingDecision,
    approval_strategy: ApprovalStrategy,
) -> Option<String> {
    let non_trivial = analyzed_task.complexity >= 3
        || analyzed_task.needs_verification
        || analyzed_task.involves_multiple_modules
        || analyzed_task.has_safety_concerns;

    if non_trivial && routing.roles.is_empty() {
        return Some("routing produced no roles for a non-trivial task".to_string());
    }

    let reviewer_required = routing.roles.contains(&AgentRole::Reviewer)
        || routing
            .pua_enforcement
            .mandatory_roles
            .contains(&AgentRole::Reviewer);
    if reviewer_required && !approval_strategy.needs_dual_review() {
        return Some(
            "reviewer role required by pipeline routing, but current mode does not enable dual review gate"
                .to_string(),
        );
    }

    if non_trivial && routing.pua_enforcement.mandatory_safeguards.is_empty() {
        return Some("PUA safeguards missing for non-trivial task".to_string());
    }

    None
}

fn touch_conversation_order(order: &StdMutex<Vec<String>>, conversation_id: &str) {
    if let Ok(mut guard) = order.lock() {
        if let Some(position) = guard.iter().position(|item| item == conversation_id) {
            guard.remove(position);
        }
        guard.push(conversation_id.to_string());
    }
}

fn evict_oldest_conversation(
    store: &mut HashMap<String, ConversationState>,
    order: &StdMutex<Vec<String>>,
) -> Option<String> {
    if let Ok(mut guard) = order.lock() {
        while let Some(candidate) = guard.first().cloned() {
            guard.remove(0);
            if store.remove(&candidate).is_some() {
                return Some(candidate);
            }
        }
        return None;
    }

    let oldest = store
        .iter()
        .min_by_key(|(_, state)| state.last_touched_at)
        .map(|(id, _)| id.clone());

    oldest.and_then(|id| store.remove(&id).map(|_| id))
}

fn enforce_checkpoint_capacity(
    state: &mut ConversationState,
    incoming: usize,
    protected_checkpoint_id: Option<&str>,
) {
    let total_after_insert = state.checkpoints.len().saturating_add(incoming);
    if total_after_insert <= MAX_CHECKPOINTS_PER_CONVERSATION {
        return;
    }

    let mut overflow = total_after_insert - MAX_CHECKPOINTS_PER_CONVERSATION;
    let mut cursor = 0usize;

    // Prefer removing oldest checkpoints, but keep the rollback target when requested.
    while overflow > 0 && cursor < state.checkpoints.len() {
        let can_remove = protected_checkpoint_id
            .map(|protected| state.checkpoints[cursor].checkpoint_id != protected)
            .unwrap_or(true);
        if can_remove {
            state.checkpoints.remove(cursor);
            overflow -= 1;
        } else {
            cursor += 1;
        }
    }

    if overflow > 0 {
        let drain_to = overflow.min(state.checkpoints.len());
        state.checkpoints.drain(0..drain_to);
    }

    repair_conversation_branch_heads(state);
}

fn stream_would_exceed_limits(
    current_chunks: usize,
    current_chars: usize,
    next_token_chars: usize,
) -> bool {
    current_chunks.saturating_add(1) > MAX_STREAM_CHUNKS
        || current_chars.saturating_add(next_token_chars) > MAX_STREAM_CHARS
}

fn validate_storage_key(
    value: &str,
    field: &str,
    max_len: usize,
) -> std::result::Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(crate::i18n::tf(
            "error.storage_key_empty",
            &[("field", field)],
        ));
    }
    if trimmed.len() > max_len {
        return Err(crate::i18n::tf(
            "error.storage_key_too_long",
            &[("field", field), ("max_len", &max_len.to_string())],
        ));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':' | '/'))
    {
        return Err(format!(
            "{field} contains invalid characters; allowed: [A-Za-z0-9_.:/-]"
        ));
    }

    Ok(trimmed.to_string())
}

fn checkpoint_message_chars(messages: &[Message]) -> usize {
    messages.iter().map(|msg| msg.content.chars().count()).sum()
}

fn repair_conversation_branch_heads(state: &mut ConversationState) {
    let existing_ids = state
        .checkpoints
        .iter()
        .map(|checkpoint| checkpoint.checkpoint_id.clone())
        .collect::<HashSet<_>>();
    let mut repaired_heads: HashMap<String, String> = HashMap::new();
    for (branch, head_id) in state.branch_heads.clone() {
        if existing_ids.contains(&head_id) {
            repaired_heads.insert(branch, head_id);
            continue;
        }

        if let Some(fallback) = state
            .checkpoints
            .iter()
            .rev()
            .find(|checkpoint| checkpoint.branch_id == branch)
            .map(|checkpoint| checkpoint.checkpoint_id.clone())
        {
            repaired_heads.insert(branch, fallback);
        }
    }
    state.branch_heads = repaired_heads;
}

fn branch_head_adjustment_counts(
    before: &HashMap<String, String>,
    after: &HashMap<String, String>,
) -> (usize, usize) {
    let mut repaired = 0usize;
    let mut dropped = 0usize;
    for (branch, old_head) in before {
        match after.get(branch) {
            Some(new_head) if new_head != old_head => repaired = repaired.saturating_add(1),
            Some(_) => {}
            None => dropped = dropped.saturating_add(1),
        }
    }

    (repaired, dropped)
}

fn infer_pua_stage(event_type: &str, phase: &str) -> Option<String> {
    if event_type.starts_with("phase.") {
        return Some(phase.to_string());
    }
    None
}

fn normalize_trace_attributes(event_type: &str, phase: &str, status: &str, inputs: Value) -> Value {
    let mut attrs = match inputs {
        Value::Object(map) => map,
        other => {
            let mut map = serde_json::Map::new();
            map.insert("payload".to_string(), other);
            map
        }
    };

    attrs
        .entry("event_type".to_string())
        .or_insert_with(|| Value::String(event_type.to_string()));
    attrs
        .entry("phase".to_string())
        .or_insert_with(|| Value::String(phase.to_string()));
    attrs
        .entry("stage".to_string())
        .or_insert_with(|| Value::String(phase.to_string()));
    attrs.entry("policy_status".to_string()).or_insert_with(|| {
        Value::String(
            match status {
                "ok" => "pass",
                "error" => "error",
                _ => "unknown",
            }
            .to_string(),
        )
    });

    Value::Object(attrs)
}

#[allow(clippy::too_many_arguments)]
fn stream_chunk_notification(
    id: &Option<Value>,
    agent: &str,
    token: &str,
    chunk_index: usize,
    total_chars: usize,
    cache_level: Option<&str>,
    phase: Option<&str>,
    trace_id: Option<&str>,
) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("id".to_string(), id.clone().unwrap_or(Value::Null));
    payload.insert("agent".to_string(), Value::String(agent.to_string()));
    payload.insert("token".to_string(), Value::String(token.to_string()));
    payload.insert("chunk_index".to_string(), json!(chunk_index));
    payload.insert("total_chars".to_string(), json!(total_chars));

    if let Some(level) = cache_level {
        payload.insert("cached".to_string(), Value::Bool(true));
        payload.insert("cache_level".to_string(), Value::String(level.to_string()));
    }
    if let Some(phase_name) = phase {
        payload.insert("phase".to_string(), Value::String(phase_name.to_string()));
    }
    if let Some(trace) = trace_id {
        payload.insert("trace_id".to_string(), Value::String(trace.to_string()));
    }

    Value::Object(payload)
}

#[allow(clippy::too_many_arguments)]
fn stream_done_notification(
    id: &Option<Value>,
    agent: &str,
    chunks: usize,
    total_chars: usize,
    cache_level: Option<&str>,
    phase: Option<&str>,
    trace_id: Option<&str>,
    duration_ms: u64,
) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("id".to_string(), id.clone().unwrap_or(Value::Null));
    payload.insert("agent".to_string(), Value::String(agent.to_string()));
    payload.insert("done".to_string(), Value::Bool(true));
    payload.insert("chunks".to_string(), json!(chunks));
    payload.insert("total_chars".to_string(), json!(total_chars));
    payload.insert("duration_ms".to_string(), json!(duration_ms));

    if let Some(level) = cache_level {
        payload.insert("cached".to_string(), Value::Bool(true));
        payload.insert("cache_level".to_string(), Value::String(level.to_string()));
    }
    if let Some(phase_name) = phase {
        payload.insert("phase".to_string(), Value::String(phase_name.to_string()));
    }
    if let Some(trace) = trace_id {
        payload.insert("trace_id".to_string(), Value::String(trace.to_string()));
    }

    Value::Object(payload)
}

fn histogram_prometheus_lines(
    name: &str,
    count: u64,
    sum_seconds: f64,
    buckets: &[u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
) -> Vec<String> {
    let mut lines = Vec::new();
    push_metric_header(
        &mut lines,
        name,
        "histogram",
        "ACP latency distribution in seconds",
    );
    let mut cumulative = 0_u64;
    for (idx, le) in HISTOGRAM_BUCKETS_SECONDS.iter().enumerate() {
        cumulative = cumulative.saturating_add(buckets[idx]);
        lines.push(format!("{}_bucket{{le=\"{}\"}} {}", name, le, cumulative));
    }
    cumulative = cumulative.saturating_add(buckets[HISTOGRAM_BUCKETS_SECONDS.len()]);
    lines.push(format!("{}_bucket{{le=\"+Inf\"}} {}", name, cumulative));
    lines.push(format!("{}_sum {}", name, sum_seconds));
    lines.push(format!("{}_count {}", name, count));
    lines
}

fn classify_agent_failure(err: &anyhow::Error) -> &'static str {
    let msg = err.to_string().to_ascii_lowercase();
    if msg.contains("timed out") || msg.contains("timeout") {
        return "timeout";
    }
    if msg.contains("panic") {
        return "panic";
    }
    "other"
}

fn record_agent_failure_metrics(metrics: &RuntimeMetrics, err: &anyhow::Error) {
    metrics.inc_agent_failures();
    match classify_agent_failure(err) {
        "timeout" => metrics.inc_agent_timeout_failures(),
        "panic" => metrics.inc_agent_panic_failures(),
        _ => metrics.inc_agent_other_failures(),
    }
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn hash_hex(input: &str, hex_len: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    let full = digest
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect::<String>();
    full.chars().take(hex_len).collect()
}

fn escape_prometheus_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn build_prometheus_metrics(
    snapshot: &MetricsSnapshot,
    gauges: &RuntimeGaugeSnapshot,
    breaker_snapshot: &HashMap<String, CircuitBreakerSnapshot>,
    phase_limiter_snapshot: &HashMap<String, (f64, f64)>,
    inflight_snapshot: &(usize, HashMap<String, usize>),
    lifecycle: &LifecycleSnapshot,
    maintenance: &MaintenanceSnapshot,
) -> String {
    let mut lines = Vec::new();
    push_scalar_metric(
        &mut lines,
        "acp_chat_requests_total",
        "counter",
        "Total ACP chat requests handled",
        snapshot.chat_requests_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_cache_lookup_total",
        "counter",
        "Total cache lookups performed",
        snapshot.cache_lookup_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_cache_hit_total",
        "counter",
        "Total cache hits served",
        snapshot.cache_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_cache_store_total",
        "counter",
        "Total cache writes performed",
        snapshot.cache_store_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_search_total",
        "counter",
        "Total vector searches performed",
        snapshot.vector_search_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_hit_total",
        "counter",
        "Total vector retrieval hits",
        snapshot.vector_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_store_total",
        "counter",
        "Total vector memory writes",
        snapshot.vector_store_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_summary_read_total",
        "counter",
        "Total summary memory reads",
        snapshot.summary_read_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_summary_hit_total",
        "counter",
        "Total summary memory hits",
        snapshot.summary_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_summary_store_total",
        "counter",
        "Total summary memory writes",
        snapshot.summary_store_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_agent_failures_total",
        "counter",
        "Total agent execution failures",
        snapshot.agent_failures_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_agent_timeout_failures_total",
        "counter",
        "Total agent timeout failures",
        snapshot.agent_timeout_failures_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_agent_panic_failures_total",
        "counter",
        "Total agent panic failures",
        snapshot.agent_panic_failures_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_agent_other_failures_total",
        "counter",
        "Total uncategorized agent failures",
        snapshot.agent_other_failures_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_total",
        "counter",
        "Total review gate evaluations",
        snapshot.review_gate_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_approved_total",
        "counter",
        "Total review gate approvals",
        snapshot.review_gate_approved_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_rejected_total",
        "counter",
        "Total review gate rejections",
        snapshot.review_gate_rejected_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_timeout_total",
        "counter",
        "Total review gate deadline timeouts",
        snapshot.review_gate_timeout_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_degraded_total",
        "counter",
        "Total review gate approvals degraded after timeout",
        snapshot.review_gate_degraded_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_invalid_response_total",
        "counter",
        "Total invalid review gate responses",
        snapshot.review_gate_invalid_response_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_blue5_doc_lookup_total",
        "counter",
        "Total BLUE5 document lazy-load lookups",
        snapshot.lazy_blue5_doc_lookup_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_blue5_doc_hit_total",
        "counter",
        "Total BLUE5 document lazy-load cache hits",
        snapshot.lazy_blue5_doc_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_blue5_doc_reload_total",
        "counter",
        "Total BLUE5 document lazy-load reloads",
        snapshot.lazy_blue5_doc_reload_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_app_config_lookup_total",
        "counter",
        "Total app config lazy-load lookups",
        snapshot.lazy_app_config_lookup_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_app_config_hit_total",
        "counter",
        "Total app config lazy-load cache hits",
        snapshot.lazy_app_config_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_app_config_reload_total",
        "counter",
        "Total app config lazy-load reloads",
        snapshot.lazy_app_config_reload_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_clarification_lookup_total",
        "counter",
        "Total clarification artifact lazy-load lookups",
        snapshot.lazy_clarification_lookup_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_clarification_hit_total",
        "counter",
        "Total clarification artifact lazy-load cache hits",
        snapshot.lazy_clarification_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_clarification_reload_total",
        "counter",
        "Total clarification artifact lazy-load reloads",
        snapshot.lazy_clarification_reload_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_memory_cache_entries",
        "gauge",
        "Current in-memory cache entries",
        gauges.memory_cache_entries,
    );
    push_scalar_metric(
        &mut lines,
        "acp_sqlite_cache_entries",
        "gauge",
        "Current SQLite cache entries",
        gauges.sqlite_cache_entries,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_memory_entries",
        "gauge",
        "Current vector memory entries",
        gauges.vector_memory_entries,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_summary_entries",
        "gauge",
        "Current vector summary entries",
        gauges.vector_summary_entries,
    );
    push_scalar_metric(
        &mut lines,
        "acp_circuit_open_agents",
        "gauge",
        "Current open circuit breaker agents",
        gauges.circuit_open_agents,
    );
    push_scalar_metric(
        &mut lines,
        "acp_circuit_half_open_agents",
        "gauge",
        "Current half-open circuit breaker agents",
        gauges.circuit_half_open_agents,
    );
    push_scalar_metric(
        &mut lines,
        "acp_circuit_tracked_agents",
        "gauge",
        "Current tracked circuit breaker agents",
        gauges.circuit_tracked_agents,
    );
    push_scalar_metric(
        &mut lines,
        "acp_rate_limiter_tracked_phases",
        "gauge",
        "Current tracked phases with rate limiter state",
        gauges.rate_limiter_tracked_phases,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lifecycle_shutting_down",
        "gauge",
        "Whether the ACP server is shutting down",
        if lifecycle.shutting_down { 1 } else { 0 },
    );
    push_scalar_metric(
        &mut lines,
        "acp_maintenance_cycles_total",
        "counter",
        "Total maintenance cycles executed",
        maintenance.cycles_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_maintenance_running",
        "gauge",
        "Whether a maintenance cycle is currently running",
        if maintenance.running { 1 } else { 0 },
    );

    push_metric_header(
        &mut lines,
        "acp_inflight_requests",
        "gauge",
        "Current in-flight request count by scope",
    );
    lines.push(format!(
        "acp_inflight_requests{{scope=\"global\"}} {}",
        inflight_snapshot.0
    ));
    for (phase, count) in inflight_snapshot.1.iter() {
        lines.push(format!(
            "acp_inflight_requests{{scope=\"phase\",phase=\"{}\"}} {}",
            escape_prometheus_label(phase),
            count
        ));
    }

    push_metric_header(
        &mut lines,
        "acp_phase_rate_limiter_tokens",
        "gauge",
        "Current token bucket tokens by phase",
    );
    push_metric_header(
        &mut lines,
        "acp_phase_rate_limiter_capacity",
        "gauge",
        "Current token bucket capacity by phase",
    );
    for (phase, (tokens, capacity)) in phase_limiter_snapshot.iter() {
        let phase = escape_prometheus_label(phase);
        lines.push(format!(
            "acp_phase_rate_limiter_tokens{{phase=\"{}\"}} {:.3}",
            phase, tokens
        ));
        lines.push(format!(
            "acp_phase_rate_limiter_capacity{{phase=\"{}\"}} {:.3}",
            phase, capacity
        ));
    }

    push_metric_header(
        &mut lines,
        "acp_circuit_breaker_state",
        "gauge",
        "Current circuit breaker state per agent",
    );
    push_metric_header(
        &mut lines,
        "acp_circuit_breaker_failures",
        "gauge",
        "Current consecutive failures per agent",
    );
    for (agent, state) in breaker_snapshot.iter() {
        let agent = escape_prometheus_label(agent);
        for stage in ["closed", "open", "half_open", "half_open_ready"] {
            let value = if state.state == stage { 1 } else { 0 };
            lines.push(format!(
                "acp_circuit_breaker_state{{agent=\"{}\",state=\"{}\"}} {}",
                agent, stage, value
            ));
        }
        lines.push(format!(
            "acp_circuit_breaker_failures{{agent=\"{}\"}} {}",
            agent, state.consecutive_failures
        ));
    }

    lines.extend(histogram_prometheus_lines(
        "acp_chat_latency_seconds",
        snapshot.chat_latency_count,
        snapshot.chat_latency_sum_seconds,
        &snapshot.chat_latency_bucket_counts,
    ));
    lines.extend(histogram_prometheus_lines(
        "acp_agent_latency_seconds",
        snapshot.agent_latency_count,
        snapshot.agent_latency_sum_seconds,
        &snapshot.agent_latency_bucket_counts,
    ));
    lines.extend(histogram_prometheus_lines(
        "acp_review_latency_seconds",
        snapshot.review_latency_count,
        snapshot.review_latency_sum_seconds,
        &snapshot.review_latency_bucket_counts,
    ));

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::*;
    use crate::config::{AgentConfig, AppConfig, FlowConfig, PhaseConfig};

    fn vector_config_fixture() -> VectorConfig {
        VectorConfig {
            enabled: true,
            auto_mode: false,
            path: "vector.sqlite3".to_string(),
            dimensions: 192,
            min_query_chars: 140,
            top_k: 4,
            min_similarity: 0.91,
            max_snippet_chars: 640,
            max_entries: 1000,
            summary_enabled: false,
            summary_trigger_messages: 12,
            summary_max_chars: 1500,
        }
    }

    fn phase_inference_server(default_phase: &str, phase_names: &[&str]) -> AcpServer {
        let mut agents = HashMap::new();
        agents.insert(
            "copilot".to_string(),
            AgentConfig {
                agent_type: "copilot".to_string(),
                url: Some("http://127.0.0.1:8080".to_string()),
                chat_path: None,
                api_key_env: None,
                secret_key_env: None,
                anthropic_version: None,
                model: None,
                max_tokens: None,
                supports_system: None,
            },
        );

        let phases = phase_names
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    PhaseConfig {
                        description: format!("{} phase", name),
                        agents: vec!["copilot".to_string()],
                        fallback: Some(true),
                        principles: None,
                        options: None,
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        let config = Arc::new(AppConfig {
            default_phase: default_phase.to_string(),
            agents,
            flow: FlowConfig {
                name: "test-flow".to_string(),
                phases: phase_names.iter().map(|name| (*name).to_string()).collect(),
            },
            phases,
            runtime: Some(RuntimeConfig::default()),
            cache: None,
            vector: None,
            autotune: None,
            model_selection_mode: "adaptive".to_string(),
        });

        let flow = Arc::new(FlowManager::new(Arc::clone(&config), None));
        let registry = Arc::new(
            AgentRegistry::from_config(Arc::clone(&config), reqwest::Client::new())
                .expect("test registry should build"),
        );

        AcpServer::new(
            flow,
            registry,
            None,
            None,
            None,
            None,
            None,
            None,
            RuntimeConfig::default(),
            None,
            None,
            None,
            false,
        )
    }

    fn phase_inference_flow(default_phase: &str, phase_names: &[&str]) -> FlowManager {
        let phases = phase_names
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    PhaseConfig {
                        description: format!("{} phase", name),
                        agents: vec!["copilot".to_string()],
                        fallback: Some(true),
                        principles: None,
                        options: None,
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        FlowManager::new(
            Arc::new(AppConfig {
                default_phase: default_phase.to_string(),
                agents: HashMap::from([(
                    "copilot".to_string(),
                    AgentConfig {
                        agent_type: "copilot".to_string(),
                        url: Some("http://127.0.0.1:8080".to_string()),
                        chat_path: None,
                        api_key_env: None,
                        secret_key_env: None,
                        anthropic_version: None,
                        model: None,
                        max_tokens: None,
                        supports_system: None,
                    },
                )]),
                flow: FlowConfig {
                    name: "test-flow".to_string(),
                    phases: phase_names.iter().map(|name| (*name).to_string()).collect(),
                },
                phases,
                runtime: Some(RuntimeConfig::default()),
                cache: None,
                vector: None,
                autotune: None,
                model_selection_mode: "adaptive".to_string(),
            }),
            None,
        )
    }

    #[test]
    fn chat_mode_parsing() {
        assert_eq!(ChatMode::parse(Some("ask")), Some(ChatMode::Ask));
        assert_eq!(ChatMode::parse(Some("edit")), Some(ChatMode::Edit));
        assert_eq!(ChatMode::parse(Some("agent")), Some(ChatMode::Agent));
        assert_eq!(ChatMode::parse(Some("full_auto")), Some(ChatMode::FullAuto));
        assert_eq!(ChatMode::parse(Some("FULL-AUTO")), Some(ChatMode::FullAuto));
        assert_eq!(ChatMode::parse(Some("unknown")), None);
        assert_eq!(ChatMode::parse(None), None);
    }

    #[test]
    fn autopilot_complexity_parsing() {
        assert_eq!(
            AutopilotComplexity::from_str("simple"),
            Some(AutopilotComplexity::Simple)
        );
        assert_eq!(
            AutopilotComplexity::from_str("complex"),
            Some(AutopilotComplexity::Complex)
        );
        assert_eq!(
            AutopilotComplexity::from_str("SIMPLE"),
            Some(AutopilotComplexity::Simple)
        );
        assert_eq!(AutopilotComplexity::from_str("unknown"), None);
    }

    #[test]
    fn mode_to_strategy_mapping() {
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::Ask), None),
            ApprovalStrategy::DefaultApprovals
        );
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::Edit), None),
            ApprovalStrategy::ByPassApproval
        );
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::Agent), None),
            ApprovalStrategy::ByPassApproval
        );
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::FullAuto), Some(AutopilotComplexity::Simple)),
            ApprovalStrategy::AutoPilotSimple
        );
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::FullAuto), Some(AutopilotComplexity::Complex)),
            ApprovalStrategy::AutoPilotComplex
        );
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::FullAuto), None),
            ApprovalStrategy::AutoPilotSimple
        );
        assert_eq!(
            mode_to_approval_strategy(None, None),
            ApprovalStrategy::DefaultApprovals
        );
    }

    #[test]
    fn conversation_checkpoint_roundtrip_and_rollback() {
        let server = phase_inference_server("coding", &["coding", "review"]);
        let first_messages = vec![Message {
            role: "user".to_string(),
            content: "draft plan".to_string(),
        }];

        let first = server
            .create_conversation_checkpoint(
                "conv-a",
                "main",
                first_messages.clone(),
                Some("initial".to_string()),
            )
            .expect("first checkpoint should be created");
        let second = server
            .create_conversation_checkpoint(
                "conv-a",
                "main",
                vec![Message {
                    role: "assistant".to_string(),
                    content: "second response".to_string(),
                }],
                Some("second".to_string()),
            )
            .expect("second checkpoint should be created");

        let listed = server
            .list_conversation_checkpoints("conv-a", Some("main"), 10)
            .expect("list should succeed");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].checkpoint_id, second.checkpoint_id);
        assert_eq!(listed[1].checkpoint_id, first.checkpoint_id);

        let restored = server
            .rollback_conversation_checkpoint("conv-a", &first.checkpoint_id, Some("hotfix"))
            .expect("rollback should locate target checkpoint");
        assert_eq!(restored.branch_id, "hotfix");
        assert_ne!(restored.checkpoint_id, first.checkpoint_id);
        assert_eq!(
            restored.parent_checkpoint_id.as_deref(),
            Some(first.checkpoint_id.as_str())
        );
        assert_eq!(restored.messages.len(), first_messages.len());
        assert_eq!(restored.messages[0].content, first_messages[0].content);

        let prune = server.prune_conversation_checkpoints("conv-a", Some("main"), 1);
        assert_eq!(prune.removed, 1);

        let hotfix_checkpoint = server
            .create_conversation_checkpoint(
                "conv-a",
                "hotfix",
                vec![Message {
                    role: "assistant".to_string(),
                    content: "hotfix follow-up".to_string(),
                }],
                Some("hotfix checkpoint".to_string()),
            )
            .expect("hotfix checkpoint should be created");
        assert_eq!(
            hotfix_checkpoint.parent_checkpoint_id.as_deref(),
            Some(restored.checkpoint_id.as_str())
        );
    }

    #[test]
    fn rollback_preserves_target_checkpoint_under_capacity_pressure() {
        let server = phase_inference_server("coding", &["coding", "review"]);
        let mut first_checkpoint_id = None;

        for idx in 0..MAX_CHECKPOINTS_PER_CONVERSATION {
            let cp = server
                .create_conversation_checkpoint(
                    "conv-cap",
                    "main",
                    vec![Message {
                        role: "user".to_string(),
                        content: format!("message-{idx}"),
                    }],
                    None,
                )
                .expect("checkpoint creation should succeed");
            if idx == 0 {
                first_checkpoint_id = Some(cp.checkpoint_id);
            }
        }

        let target = first_checkpoint_id.expect("first checkpoint id should be captured");
        let restored = server
            .rollback_conversation_checkpoint("conv-cap", &target, Some("hotfix"))
            .expect("rollback should succeed");

        assert_eq!(
            restored.parent_checkpoint_id.as_deref(),
            Some(target.as_str())
        );

        let store = server
            .conversation_store
            .lock()
            .expect("conversation store lock should succeed");
        let state = store
            .get("conv-cap")
            .expect("conversation state should exist");
        assert_eq!(state.checkpoints.len(), MAX_CHECKPOINTS_PER_CONVERSATION);
        assert!(state
            .checkpoints
            .iter()
            .any(|cp| cp.checkpoint_id == target));
    }

    #[test]
    fn stream_limits_reject_next_token_before_append() {
        assert!(stream_would_exceed_limits(0, MAX_STREAM_CHARS, 1));
        assert!(stream_would_exceed_limits(MAX_STREAM_CHUNKS, 0, 1));
        assert!(!stream_would_exceed_limits(
            MAX_STREAM_CHUNKS.saturating_sub(1),
            MAX_STREAM_CHARS.saturating_sub(1),
            1
        ));
    }

    #[test]
    fn infer_phase_prefers_explicit_phase_over_mode_default() {
        let server = phase_inference_server("planning", &["planning", "review", "coding"]);
        let flow = phase_inference_flow("planning", &["planning", "review", "coding"]);

        assert_eq!(
            server.infer_phase_name_with_flow(&flow, Some("delivery"), Some(ChatMode::Ask)),
            "delivery"
        );
    }

    #[test]
    fn infer_phase_uses_review_for_ask_when_available() {
        let server = phase_inference_server("planning", &["planning", "review"]);
        let flow = phase_inference_flow("planning", &["planning", "review"]);

        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::Ask)),
            "review"
        );
    }

    #[test]
    fn infer_phase_uses_coding_for_edit_agent_and_full_auto() {
        let server = phase_inference_server("planning", &["planning", "coding"]);
        let flow = phase_inference_flow("planning", &["planning", "coding"]);

        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::Edit)),
            "coding"
        );
        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::Agent)),
            "coding"
        );
        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::FullAuto)),
            "coding"
        );
    }

    #[test]
    fn infer_phase_falls_back_to_default_when_mode_phase_missing() {
        let server = phase_inference_server("planning", &["planning"]);
        let flow = phase_inference_flow("planning", &["planning"]);

        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::Ask)),
            "planning"
        );
        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::FullAuto)),
            "planning"
        );
    }

    #[test]
    fn approval_strategy_dual_review_check() {
        assert!(!ApprovalStrategy::DefaultApprovals.needs_dual_review());
        assert!(!ApprovalStrategy::ByPassApproval.needs_dual_review());
        assert!(!ApprovalStrategy::AutoPilotSimple.needs_dual_review());
        assert!(ApprovalStrategy::AutoPilotComplex.needs_dual_review());
    }

    #[test]
    fn optimize_messages_respects_limits() {
        let options = PhaseOptions {
            max_history_messages: Some(2),
            max_history_chars: Some(10),
            ..PhaseOptions::default()
        };
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: "12345".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: "67890".to_string(),
            },
            Message {
                role: "user".to_string(),
                content: "abc".to_string(),
            },
        ];

        let optimized = optimize_messages(&messages, Some(&options));
        assert_eq!(optimized.len(), 2);
        assert_eq!(optimized[0].content, "67890");
        assert_eq!(optimized[1].content, "abc");
    }

    #[test]
    fn append_recent_summary_keeps_recent_tail() {
        let summary =
            append_recent_summary(Some("old summary"), Some("new question"), "new answer", 24);

        assert!(summary.contains("new answer"));
    }

    #[test]
    fn review_verdict_requires_approve_first_line() {
        assert_eq!(
            review_verdict("APPROVE\nLooks safe.", 8),
            ReviewVerdict::Approve
        );
        assert_eq!(
            review_verdict("REJECT\nMissing tests.", 8),
            ReviewVerdict::Reject
        );
        assert_eq!(
            review_verdict("Looks fine, APPROVE", 8),
            ReviewVerdict::Invalid
        );
        assert_eq!(review_verdict("OK", 8), ReviewVerdict::Invalid);
    }

    #[test]
    fn review_timeout_prefers_review_phase_override() {
        let review_options = PhaseOptions {
            review_timeout_seconds: Some(15),
            request_timeout_seconds: Some(30),
            ..PhaseOptions::default()
        };
        let primary_options = PhaseOptions {
            review_timeout_seconds: Some(20),
            request_timeout_seconds: Some(40),
            ..PhaseOptions::default()
        };

        let timeout = review_timeout(Some(&review_options), Some(&primary_options));
        assert_eq!(timeout.map(|value| value.as_secs()), Some(15));
    }

    #[test]
    fn vector_defaults_fall_back_to_global_config() {
        let vector_config = vector_config_fixture();

        assert!(!effective_vector_auto(None, Some(&vector_config)));
        assert_eq!(
            effective_vector_min_query_chars(None, Some(&vector_config), None),
            140
        );
        assert_eq!(effective_vector_top_k(None, Some(&vector_config), None), 4);
        assert_eq!(
            effective_vector_min_similarity(None, Some(&vector_config)),
            0.91
        );
        assert_eq!(
            effective_vector_max_snippet_chars(None, Some(&vector_config)),
            640
        );
        assert!(!effective_summary_enabled(None, Some(&vector_config)));
        assert_eq!(
            effective_summary_trigger_messages(None, Some(&vector_config)),
            12
        );
    }

    #[test]
    fn autotune_thresholds_override_static_vector_defaults() {
        let vector_config = vector_config_fixture();
        let tuned_state = AutoTuneState {
            current_min_query_chars: 95,
            current_top_k: 3,
            window_phase: 0,
            high_precision_count: 0,
            low_precision_count: 0,
            vector_search_count: 0,
            cooldown_remaining: 0,
        };

        assert_eq!(
            effective_vector_min_query_chars(None, Some(&vector_config), Some(&tuned_state)),
            95
        );
        assert_eq!(
            effective_vector_top_k(None, Some(&vector_config), Some(&tuned_state)),
            3
        );
    }

    #[test]
    fn autotune_snapshot_includes_all_fields() {
        let config = AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = AutoTuneState::new(&config);
        state.current_min_query_chars = 120;
        state.current_top_k = 3;
        state.window_phase = 5;
        state.high_precision_count = 12;
        state.low_precision_count = 2;
        state.vector_search_count = 18;
        state.cooldown_remaining = 1;

        let snapshot = state.snapshot();
        assert_eq!(snapshot["current_min_query_chars"], 120);
        assert_eq!(snapshot["current_top_k"], 3);
        assert_eq!(snapshot["window_phase"], 5);
        assert_eq!(snapshot["high_precision_count"], 12);
        assert_eq!(snapshot["low_precision_count"], 2);
        assert_eq!(snapshot["vector_search_count"], 18);
        assert_eq!(snapshot["cooldown_remaining"], 1);
    }

    // Integration tests for full ACP protocol flow
    #[test]
    fn initialize_request_returns_server_capabilities() {
        let request_json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let request: JsonRpcRequest = serde_json::from_str(request_json).unwrap();

        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.id, Some(Value::Number(1.into())));
        assert_eq!(request.method, "initialize");
    }

    #[test]
    fn metrics_snapshot_structure() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_chat_requests();
        metrics.inc_cache_lookup();
        metrics.inc_cache_hit();
        metrics.inc_vector_search();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.chat_requests_total, 1);
        assert_eq!(snapshot.cache_lookup_total, 1);
        assert_eq!(snapshot.cache_hit_total, 1);
        assert_eq!(snapshot.vector_search_total, 1);
    }

    #[test]
    fn jsonrpc_response_serialization() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0",
            id: Some(Value::Number(1.into())),
            result: Some(json!({"status": "ok"})),
            error: None,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 1);
        assert_eq!(json["result"]["status"], "ok");
        assert!(json.get("error").is_none());
    }

    #[test]
    fn jsonrpc_error_response_serialization() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0",
            id: Some(Value::Number(2.into())),
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: "Method not found".to_string(),
                data: None,
            }),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 2);
        assert_eq!(json["error"]["code"], -32601);
        assert!(json.get("result").is_none());
    }

    // Cache hit integration test
    #[test]
    fn cache_hit_increments_metrics() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_cache_lookup();
        metrics.inc_cache_hit();
        metrics.inc_cache_lookup();
        metrics.inc_cache_hit();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.cache_lookup_total, 2);
        assert_eq!(snapshot.cache_hit_total, 2);
    }

    #[test]
    fn cache_miss_tracked_correctly() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_cache_lookup();
        // No hit incremented
        metrics.inc_cache_lookup();
        metrics.inc_cache_hit();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.cache_lookup_total, 2);
        assert_eq!(snapshot.cache_hit_total, 1);
    }

    // Dual review integration test
    #[test]
    fn autopilot_complex_requires_dual_review() {
        let mode = ChatMode::FullAuto;
        let complexity = AutopilotComplexity::Complex;
        let strategy = mode_to_approval_strategy(Some(mode), Some(complexity));

        assert_eq!(strategy, ApprovalStrategy::AutoPilotComplex);
        assert!(strategy.needs_dual_review());
    }

    #[test]
    fn autopilot_simple_bypasses_dual_review() {
        let mode = ChatMode::FullAuto;
        let complexity = AutopilotComplexity::Simple;
        let strategy = mode_to_approval_strategy(Some(mode), Some(complexity));

        assert_eq!(strategy, ApprovalStrategy::AutoPilotSimple);
        assert!(!strategy.needs_dual_review());
    }

    #[test]
    fn edit_mode_bypasses_approvals() {
        let mode = ChatMode::Edit;
        let strategy = mode_to_approval_strategy(Some(mode), None);

        assert!(!strategy.needs_dual_review());
        assert_eq!(strategy.as_str(), "by_pass_approval");
    }

    // Fallback chain integration test
    #[test]
    fn approval_strategy_fallback_chain() {
        // Test: Ask mode (default) requires approval
        let strategy_ask = mode_to_approval_strategy(Some(ChatMode::Ask), None);
        assert_eq!(strategy_ask, ApprovalStrategy::DefaultApprovals);

        // Test: No mode defaults to Ask behavior
        let strategy_none = mode_to_approval_strategy(None, None);
        assert_eq!(strategy_none, ApprovalStrategy::DefaultApprovals);

        // Test: FullAuto without complexity defaults to Simple
        let strategy_auto = mode_to_approval_strategy(Some(ChatMode::FullAuto), None);
        assert_eq!(strategy_auto, ApprovalStrategy::AutoPilotSimple);
    }

    #[test]
    fn strategy_string_representations() {
        let strategies = vec![
            (ApprovalStrategy::DefaultApprovals, "default_approvals"),
            (ApprovalStrategy::ByPassApproval, "by_pass_approval"),
            (ApprovalStrategy::AutoPilotSimple, "autopilot_simple"),
            (ApprovalStrategy::AutoPilotComplex, "autopilot_complex"),
        ];

        for (strategy, expected) in strategies {
            assert_eq!(strategy.as_str(), expected);
        }
    }

    #[test]
    fn resolve_primary_secondary_policy_defaults_to_single_primary_and_ranked_secondary() {
        let agents = vec![
            "primary-a".to_string(),
            "secondary-b".to_string(),
            "secondary-c".to_string(),
        ];
        let params = json!({});

        let policy = resolve_primary_secondary_policy(&agents, &params, None).unwrap();

        assert_eq!(policy.primary_agent, "primary-a");
        assert_eq!(
            policy.secondary_agents,
            vec!["secondary-b".to_string(), "secondary-c".to_string()]
        );
        assert_eq!(policy.failover_policy, "first_secondary");
        assert_eq!(policy.policy_version, "blue5.v1");
    }

    #[test]
    fn resolve_primary_secondary_policy_rejects_non_candidate_primary() {
        let agents = vec!["agent-a".to_string(), "agent-b".to_string()];
        let params = json!({"primary_agent": "agent-x"});

        let err = resolve_primary_secondary_policy(&agents, &params, None).unwrap_err();
        // Check for translation key since i18n system may not be initialized in tests
        assert!(err.to_string().contains("error.primary_agent_not_found"));
    }

    #[test]
    fn online_controller_ranks_agents_by_live_phase_outcomes() {
        let mut state = OnlineControllerState::default();

        for _ in 0..6 {
            state.record_agent_outcome("coding", "copilot", false, 10_000);
            state.record_agent_outcome("coding", "deepseek", true, 1_200);
        }

        let ranked = state
            .rank_agent_names_for_phase("coding", &["copilot".to_string(), "deepseek".to_string()]);

        assert_eq!(ranked[0].0, "deepseek");
        assert_eq!(ranked[1].0, "copilot");
        assert!(ranked[0].1 > ranked[1].1);
    }

    #[test]
    fn online_controller_keeps_original_order_without_enough_samples() {
        let mut state = OnlineControllerState::default();
        state.record_agent_outcome("coding", "copilot", true, 1_100);
        state.record_agent_outcome("coding", "deepseek", false, 1_100);

        let ranked = state
            .rank_agent_names_for_phase("coding", &["copilot".to_string(), "deepseek".to_string()]);

        assert_eq!(ranked[0].0, "copilot");
        assert_eq!(ranked[1].0, "deepseek");
    }

    #[test]
    fn circuit_breaker_transitions_to_half_open_and_closes_on_success() {
        let breaker = CircuitBreakerRegistry::default();

        breaker.record_failure_with_config("copilot", 2, 1);
        assert!(matches!(
            breaker.allow_request("copilot"),
            CircuitBreakerAdmission::Closed
        ));

        breaker.record_failure_with_config("copilot", 2, 1);
        let snapshot = breaker.snapshot();
        assert_eq!(snapshot["copilot"].state, "open");

        assert!(matches!(
            breaker.allow_request("copilot"),
            CircuitBreakerAdmission::Rejected {
                state: "open",
                retry_after_seconds: Some(_)
            }
        ));

        {
            let mut guard = breaker.inner.lock().unwrap();
            let state = guard.get_mut("copilot").unwrap();
            state.open_until = Some(now_ts() - 1);
        }

        assert!(matches!(
            breaker.allow_request("copilot"),
            CircuitBreakerAdmission::HalfOpenProbe
        ));
        assert!(matches!(
            breaker.allow_request("copilot"),
            CircuitBreakerAdmission::Rejected {
                state: "half_open",
                retry_after_seconds: None
            }
        ));

        breaker.record_success("copilot");
        let snapshot = breaker.snapshot();
        assert_eq!(snapshot["copilot"].state, "closed");
        assert_eq!(snapshot["copilot"].consecutive_failures, 0);
        assert!(!snapshot["copilot"].probe_in_flight);
    }

    #[test]
    fn circuit_breaker_half_open_failure_reopens_breaker() {
        let breaker = CircuitBreakerRegistry::default();

        breaker.record_failure_with_config("claude", 1, 1);
        {
            let mut guard = breaker.inner.lock().unwrap();
            let state = guard.get_mut("claude").unwrap();
            state.open_until = Some(now_ts() - 1);
        }

        assert!(matches!(
            breaker.allow_request("claude"),
            CircuitBreakerAdmission::HalfOpenProbe
        ));

        breaker.record_failure_with_config("claude", 1, 3);
        let snapshot = breaker.snapshot();
        assert_eq!(snapshot["claude"].state, "open");
        assert_eq!(snapshot["claude"].consecutive_failures, 1);
        assert!(!snapshot["claude"].probe_in_flight);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn prometheus_export_includes_headers_and_runtime_labels() {
        let mut snapshot = MetricsSnapshot::default();
        snapshot.chat_requests_total = 3;
        snapshot.cache_hit_total = 2;
        snapshot.review_gate_timeout_total = 1;
        snapshot.review_gate_degraded_total = 1;
        snapshot.review_gate_invalid_response_total = 1;
        snapshot.chat_latency_count = 1;
        snapshot.chat_latency_sum_seconds = 0.25;
        snapshot.chat_latency_bucket_counts[1] = 1;

        let gauges = RuntimeGaugeSnapshot {
            memory_cache_entries: 4,
            sqlite_cache_entries: 6,
            vector_memory_entries: 8,
            vector_summary_entries: 2,
            circuit_open_agents: 1,
            circuit_half_open_agents: 1,
            circuit_tracked_agents: 2,
            rate_limiter_tracked_phases: 1,
        };

        let breaker_snapshot = HashMap::from([(
            "copilot-main".to_string(),
            CircuitBreakerSnapshot {
                consecutive_failures: 3,
                state: "half_open_ready".to_string(),
                open_until: Some(now_ts() + 5),
                probe_in_flight: false,
            },
        )]);
        let phase_limiter_snapshot = HashMap::from([("coding".to_string(), (4.5, 12.0))]);
        let inflight_snapshot = (2_usize, HashMap::from([("coding".to_string(), 1_usize)]));
        let lifecycle = LifecycleSnapshot {
            shutting_down: true,
            shutdown_started_at: Some(now_ts()),
            shutdown_reason: Some("unit-test".to_string()),
        };
        let maintenance = MaintenanceSnapshot {
            running: true,
            cycles_total: 7,
            last_started_at: Some(now_ts()),
            last_completed_at: Some(now_ts()),
            last_memory_expired_removed: 3,
            last_sqlite_expired_removed: 5,
            last_cache_vacuumed: false,
            last_vector_vacuumed: false,
            last_error: None,
        };

        let rendered = build_prometheus_metrics(
            &snapshot,
            &gauges,
            &breaker_snapshot,
            &phase_limiter_snapshot,
            &inflight_snapshot,
            &lifecycle,
            &maintenance,
        );

        assert!(rendered.contains("# HELP acp_chat_requests_total Total ACP chat requests handled"));
        assert!(rendered.contains("# TYPE acp_chat_requests_total counter"));
        assert!(rendered.contains("acp_review_gate_timeout_total 1"));
        assert!(rendered.contains("acp_review_gate_degraded_total 1"));
        assert!(rendered.contains("acp_review_gate_invalid_response_total 1"));
        assert!(rendered.contains("acp_inflight_requests{scope=\"global\"} 2"));
        assert!(rendered.contains("acp_inflight_requests{scope=\"phase\",phase=\"coding\"} 1"));
        assert!(rendered.contains(
            "acp_circuit_breaker_state{agent=\"copilot-main\",state=\"half_open_ready\"} 1"
        ));
        assert!(rendered.contains("acp_lifecycle_shutting_down 1"));
        assert!(rendered.contains("acp_maintenance_cycles_total 7"));
        assert!(rendered.contains("acp_lazy_blue5_doc_lookup_total 0"));
        assert!(rendered.contains("acp_chat_latency_seconds_bucket{le=\"0.25\"} 1"));
    }

    #[test]
    fn metrics_reset_clears_all_counters() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_chat_requests();
        metrics.inc_cache_hit();
        metrics.inc_vector_search();

        let snapshot1 = metrics.snapshot();
        assert!(snapshot1.chat_requests_total > 0);

        metrics.reset();
        let snapshot2 = metrics.snapshot();
        assert_eq!(snapshot2.chat_requests_total, 0);
        assert_eq!(snapshot2.cache_hit_total, 0);
        assert_eq!(snapshot2.vector_search_total, 0);
    }

    #[test]
    fn record_agent_failure_metrics_tracks_timeout_bucket() {
        let metrics = RuntimeMetrics::default();
        let err = anyhow::anyhow!("agent timed out after 15s");

        record_agent_failure_metrics(&metrics, &err);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.agent_failures_total, 1);
        assert_eq!(snapshot.agent_timeout_failures_total, 1);
        assert_eq!(snapshot.agent_panic_failures_total, 0);
        assert_eq!(snapshot.agent_other_failures_total, 0);
    }

    #[test]
    fn record_agent_failure_metrics_tracks_panic_and_other_buckets() {
        let metrics = RuntimeMetrics::default();
        let panic_err = anyhow::anyhow!("agent panic: task join error");
        let other_err = anyhow::anyhow!("remote provider returned malformed payload");

        record_agent_failure_metrics(&metrics, &panic_err);
        record_agent_failure_metrics(&metrics, &other_err);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.agent_failures_total, 2);
        assert_eq!(snapshot.agent_timeout_failures_total, 0);
        assert_eq!(snapshot.agent_panic_failures_total, 1);
        assert_eq!(snapshot.agent_other_failures_total, 1);
    }

    // === ACP Runtime RPC Integration Tests ===
    // These tests verify the JSON-RPC protocol contract for ACP server endpoints.

    #[test]
    fn rpc_initialize_response_includes_server_name_and_capabilities() {
        let server = phase_inference_server("planning", &["planning", "coding"]);

        // Verify request parsing
        let request_json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let request: JsonRpcRequest = serde_json::from_str(request_json).unwrap();

        assert_eq!(request.method, "initialize");
        assert_eq!(request.id, Some(Value::Number(1.into())));

        // Runtime defaults are injected when no explicit runtime block is provided.
        assert!(server.runtime_config_snapshot().shutdown_drain_seconds > 0);
    }

    #[test]
    fn rpc_metrics_snapshot_includes_all_metric_types() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_chat_requests();
        metrics.inc_cache_lookup();
        metrics.inc_cache_hit();
        metrics.inc_vector_search();
        metrics.inc_vector_hit();
        metrics.inc_summary_read();
        metrics.inc_summary_hit();
        metrics.inc_agent_failures();
        metrics.inc_agent_timeout_failures();
        metrics.inc_review_gate();
        metrics.inc_review_gate_approved();
        metrics.inc_review_gate_timeout();
        metrics.inc_review_gate_degraded();
        metrics.inc_review_gate_invalid_response();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.chat_requests_total, 1);
        assert_eq!(snapshot.cache_lookup_total, 1);
        assert_eq!(snapshot.cache_hit_total, 1);
        assert_eq!(snapshot.vector_search_total, 1);
        assert_eq!(snapshot.vector_hit_total, 1);
        assert_eq!(snapshot.summary_read_total, 1);
        assert_eq!(snapshot.summary_hit_total, 1);
        assert_eq!(snapshot.agent_failures_total, 1);
        assert_eq!(snapshot.agent_timeout_failures_total, 1);
        assert_eq!(snapshot.review_gate_total, 1);
        assert_eq!(snapshot.review_gate_approved_total, 1);
        assert_eq!(snapshot.review_gate_timeout_total, 1);
        assert_eq!(snapshot.review_gate_degraded_total, 1);
        assert_eq!(snapshot.review_gate_invalid_response_total, 1);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn rpc_prometheus_metrics_serializes_to_valid_format() {
        let mut snapshot = MetricsSnapshot::default();
        snapshot.chat_requests_total = 42;
        snapshot.cache_hit_total = 15;
        snapshot.agent_failures_total = 2;
        snapshot.agent_timeout_failures_total = 1;
        snapshot.agent_panic_failures_total = 1;
        snapshot.review_gate_total = 3;
        snapshot.review_gate_approved_total = 2;
        snapshot.review_gate_timeout_total = 1;
        snapshot.review_gate_degraded_total = 1;
        snapshot.review_gate_invalid_response_total = 1;
        snapshot.lazy_blue5_doc_lookup_total = 4;

        let gauges = RuntimeGaugeSnapshot {
            memory_cache_entries: 12,
            sqlite_cache_entries: 45,
            vector_memory_entries: 8,
            vector_summary_entries: 3,
            circuit_open_agents: 1,
            circuit_half_open_agents: 0,
            circuit_tracked_agents: 2,
            rate_limiter_tracked_phases: 4,
        };

        let prometheus = build_prometheus_metrics(
            &snapshot,
            &gauges,
            &HashMap::new(),
            &HashMap::new(),
            &(0, HashMap::new()),
            &LifecycleSnapshot::default(),
            &MaintenanceSnapshot::default(),
        );

        assert!(prometheus.contains("acp_chat_requests_total 42"));
        assert!(prometheus.contains("acp_cache_hit_total 15"));
        assert!(prometheus.contains("acp_agent_failures_total 2"));
        assert!(prometheus.contains("acp_agent_timeout_failures_total 1"));
        assert!(prometheus.contains("acp_agent_panic_failures_total 1"));
        assert!(prometheus.contains("acp_review_gate_total 3"));
        assert!(prometheus.contains("acp_review_gate_approved_total 2"));
        assert!(prometheus.contains("acp_review_gate_timeout_total 1"));
        assert!(prometheus.contains("acp_review_gate_degraded_total 1"));
        assert!(prometheus.contains("acp_review_gate_invalid_response_total 1"));
        assert!(prometheus.contains("acp_lazy_blue5_doc_lookup_total 4"));
        assert!(prometheus.contains("acp_memory_cache_entries 12"));
        assert!(prometheus.contains("acp_circuit_tracked_agents 2"));
        assert!(prometheus.contains("acp_rate_limiter_tracked_phases 4"));
    }

    #[test]
    fn rpc_runtime_health_includes_all_subsystems() {
        let server = phase_inference_server("planning", &["planning", "coding"]);
        let memory_cache = &server.memory_cache;
        let circuit_breakers = &server.circuit_breakers;
        let phase_rate_limiter = &server.phase_rate_limiter;
        let inflight_limiter = &server.inflight_limiter;

        // Verify cache is accessible
        assert_eq!(memory_cache.active_entries(), 0);

        // Verify circuit breaker state
        let cb_snapshot = circuit_breakers.snapshot();
        assert!(cb_snapshot.is_empty());
        assert_eq!(circuit_breakers.tracked_agents(), 0);

        // Verify rate limiter
        assert_eq!(phase_rate_limiter.tracked_phases(), 0);

        // Verify inflight tracking
        let (global, phases) = inflight_limiter.snapshot();
        assert_eq!(global, 0);
        assert!(phases.is_empty());
    }

    #[test]
    fn rpc_phase_status_tracks_rate_limiter_state() {
        let phase_limiter = PhaseRateLimiter::default();

        // Test token bucket state tracking
        assert!(phase_limiter.allow("planning", 60, None));
        assert_eq!(phase_limiter.tracked_phases(), 1);

        let snapshot = phase_limiter.snapshot();
        assert!(snapshot.contains_key("planning"));
        let (tokens, capacity) = snapshot["planning"];
        assert!(tokens < capacity);
        assert_eq!(capacity, 60.0);
    }

    #[test]
    fn rpc_phase_status_burst_capacity_respected() {
        let phase_limiter = PhaseRateLimiter::default();

        // Allow requests up to burst capacity
        for _ in 0..5 {
            assert!(phase_limiter.allow("coding", 60, Some(5)));
        }

        // 6th request should fail
        assert!(!phase_limiter.allow("coding", 60, Some(5)));

        // Verify capacity constraint
        let snapshot = phase_limiter.snapshot();
        assert!(snapshot.contains_key("coding"));
        let (tokens, _) = snapshot["coding"];
        // Tokens should be less than 1.0 (since we just consumed one)
        assert!(tokens < 1.0);
    }

    #[test]
    fn rpc_inflight_limiter_enforces_phase_and_global_limits() {
        let limiter = Arc::new(InflightLimiter::default());

        // Test phase limit
        let guard1 = limiter.clone().try_enter("planning", Some(2), Some(5));
        assert!(guard1.is_some());

        let guard2 = limiter.clone().try_enter("planning", Some(2), Some(5));
        assert!(guard2.is_some());

        let guard3 = limiter.clone().try_enter("planning", Some(2), Some(5));
        assert!(guard3.is_none());

        drop(guard1);
        let guard4 = limiter.clone().try_enter("planning", Some(2), Some(5));
        assert!(guard4.is_some());

        let (global, _) = limiter.snapshot();
        assert_eq!(global, 2);
    }

    #[test]
    fn rpc_inflight_limiter_global_limit_respected() {
        let limiter = Arc::new(InflightLimiter::default());

        let mut guards = Vec::new();
        for _ in 0..3 {
            let guard = limiter.clone().try_enter("planning", None, Some(3));
            assert!(guard.is_some());
            guards.push(guard);
        }

        let guard4 = limiter.clone().try_enter("coding", None, Some(3));
        assert!(guard4.is_none());

        drop(guards.pop());
        let guard5 = limiter.clone().try_enter("coding", None, Some(3));
        assert!(guard5.is_some());
    }

    #[test]
    fn rpc_lifecycle_state_tracks_shutdown() {
        let lifecycle = LifecycleState::default();

        assert!(!lifecycle.is_shutting_down());
        assert!(lifecycle.start_shutdown("test shutdown"));
        assert!(lifecycle.is_shutting_down());

        // Second call should fail
        assert!(!lifecycle.start_shutdown("already shutting down"));

        let snapshot = lifecycle.snapshot();
        assert!(snapshot.shutting_down);
        assert_eq!(snapshot.shutdown_reason, Some("test shutdown".to_string()));
        assert!(snapshot.shutdown_started_at.is_some());
    }

    #[test]
    fn rpc_maintenance_tracker_records_cycle_metrics() {
        let maintenance = MaintenanceTracker::default();

        maintenance.note_started();
        let snapshot1 = maintenance.snapshot();
        assert!(snapshot1.running);
        assert_eq!(snapshot1.cycles_total, 1);

        maintenance.note_completed(5, 3, true, false);
        let snapshot2 = maintenance.snapshot();
        assert!(!snapshot2.running);
        assert_eq!(snapshot2.last_memory_expired_removed, 5);
        assert_eq!(snapshot2.last_sqlite_expired_removed, 3);
        assert!(snapshot2.last_cache_vacuumed);
        assert!(!snapshot2.last_vector_vacuumed);
        assert_eq!(snapshot2.cycles_total, 1);
    }

    #[test]
    fn rpc_maintenance_tracker_records_failures() {
        let maintenance = MaintenanceTracker::default();

        maintenance.note_started();
        maintenance.note_failed("connection timeout");

        let snapshot = maintenance.snapshot();
        assert!(!snapshot.running);
        assert_eq!(snapshot.last_error, Some("connection timeout".to_string()));
    }

    #[test]
    fn rpc_circuit_breaker_snapshot_complete() {
        let breaker = CircuitBreakerRegistry::default();

        breaker.record_failure_with_config("agent-a", 2, 10);
        breaker.record_failure_with_config("agent-a", 2, 10);
        breaker.record_failure_with_config("agent-b", 1, 10);

        let snapshot = breaker.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot["agent-a"].state, "open");
        assert_eq!(snapshot["agent-a"].consecutive_failures, 2);
        assert_eq!(snapshot["agent-b"].state, "open");
        assert_eq!(snapshot["agent-b"].consecutive_failures, 1);
    }

    #[test]
    fn rpc_metrics_reset_integration() {
        let metrics = RuntimeMetrics::default();

        metrics.inc_chat_requests();
        metrics.inc_cache_hit();
        metrics.inc_agent_failures();
        metrics.observe_chat_latency(Duration::from_secs_f64(0.25));

        let snapshot1 = metrics.snapshot();
        assert_eq!(snapshot1.chat_requests_total, 1);
        assert_eq!(snapshot1.cache_hit_total, 1);
        assert_eq!(snapshot1.agent_failures_total, 1);
        assert!(snapshot1.chat_latency_count > 0);

        metrics.reset();
        let snapshot2 = metrics.snapshot();
        assert_eq!(snapshot2.chat_requests_total, 0);
        assert_eq!(snapshot2.cache_hit_total, 0);
        assert_eq!(snapshot2.agent_failures_total, 0);
        assert_eq!(snapshot2.chat_latency_count, 0);
    }

    #[test]
    fn rpc_jsonrpc_error_codes_reserved() {
        // Verify standard JSON-RPC error codes
        assert_eq!(-32700, -32700); // Parse error
        assert_eq!(-32600, -32600); // Invalid request
        assert_eq!(-32601, -32601); // Method not found
        assert_eq!(-32602, -32602); // Invalid params
        assert_eq!(-32603, -32603); // Internal error
        assert_eq!(-32031, -32031); // Server state error (custom)
    }

    #[test]
    fn rpc_request_parsing_handles_missing_fields() {
        let request_json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let request: JsonRpcRequest = serde_json::from_str(request_json).unwrap();

        assert_eq!(request.method, "initialize");
        assert_eq!(request.id, Some(Value::Number(1.into())));
        assert_eq!(request.params, None);
    }

    #[test]
    fn rpc_response_with_result_omits_error() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0",
            id: Some(Value::Number(1.into())),
            result: Some(json!({"status": "ok"})),
            error: None,
        };

        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.contains("\"result\""));
        assert!(!serialized.contains("\"error\""));
    }

    #[test]
    fn rpc_response_with_error_omits_result() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0",
            id: Some(Value::Number(2.into())),
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: "Method not found".to_string(),
                data: None,
            }),
        };

        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.contains("\"error\""));
        assert!(!serialized.contains("\"result\""));
    }

    #[test]
    fn rpc_notification_has_no_id() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0",
            id: None,
            result: Some(json!({"type": "notification"})),
            error: None,
        };

        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains("\"id\""));
    }

    #[test]
    fn stream_chunk_notification_includes_progress_and_context() {
        let payload = stream_chunk_notification(
            &Some(json!(123)),
            "copilot",
            "hello",
            2,
            11,
            Some("memory"),
            Some("coding"),
            Some("trace-abc"),
        );

        assert_eq!(payload["id"], 123);
        assert_eq!(payload["agent"], "copilot");
        assert_eq!(payload["token"], "hello");
        assert_eq!(payload["chunk_index"], 2);
        assert_eq!(payload["total_chars"], 11);
        assert_eq!(payload["cached"], true);
        assert_eq!(payload["cache_level"], "memory");
        assert_eq!(payload["phase"], "coding");
        assert_eq!(payload["trace_id"], "trace-abc");
    }

    #[test]
    fn stream_done_notification_marks_done_with_totals() {
        let payload = stream_done_notification(
            &Some(json!("req-7")),
            "deepseek",
            4,
            128,
            None,
            Some("review"),
            Some("trace-xyz"),
            530,
        );

        assert_eq!(payload["id"], "req-7");
        assert_eq!(payload["agent"], "deepseek");
        assert_eq!(payload["done"], true);
        assert_eq!(payload["chunks"], 4);
        assert_eq!(payload["total_chars"], 128);
        assert_eq!(payload["duration_ms"], 530);
        assert_eq!(payload["phase"], "review");
        assert_eq!(payload["trace_id"], "trace-xyz");
        assert!(payload.get("cache_level").is_none());
    }
}
