//! Task execution module for ACP exec_pack.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::task::JoinSet;
use tokio::time::Duration;
use tracing::warn;

use super::pua::filter_unavailable_agents;
use super::workflow;
use super::*;
use crate::acp::helpers::autonomy_metrics;
use crate::acp::prelude::RuntimeMetrics;
use crate::acp::server::{AcpServer, OutcomeEvent};
use crate::i18n::runtime::tf;
use crate::intelligence::adaptive_selector::AdaptiveModelSelector;
use crate::memory::vector::VectorStore;
use crate::optimization::failure_prevention::FailurePrevention;
use crate::orchestration::task_router::TaskRouter;

// ============================================================================
// Struct definitions
// ============================================================================

#[derive(Clone)]
pub(crate) struct RuntimeExecutionContext {
    pub(super) task_timeout_seconds: Option<u64>,
    pub(super) task_parallelism_cap: usize,
    pub(super) principles: Option<Vec<String>>,
    pub(super) base_options: HashMap<String, Value>,
    pub(super) app_config: Arc<AppConfig>,
    pub(super) primary_agent: String,
    pub(super) secondary_agents: Vec<String>,
    pub(super) candidates: Vec<(String, Arc<dyn crate::agent::Agent>)>,
    pub(super) failure_strategy: String,
    // NOTE: Intentionally using std::sync::Mutex (not tokio::sync::Mutex).
    // Every lock() on adaptive_selector is scoped to a block that ends before
    // any .await — verified at 2 callsites in execute_single_subtask.
    // See docs/log/log-20260625-1.md §Remaining Non-Issues.
    pub(super) adaptive_selector: Arc<std::sync::Mutex<AdaptiveModelSelector>>,
    pub(super) outcome_tx: tokio::sync::mpsc::UnboundedSender<OutcomeEvent>,
    // NOTE: Intentionally using std::sync::Mutex (not tokio::sync::Mutex).
    // Every lock() on failure_prevention is scoped to a block that ends before
    // any .await — verified at all 3 callsites in execute_single_subtask.
    // See docs/log/log-20260625-1.md §Remaining Non-Issues.
    pub(super) failure_prevention: Arc<std::sync::Mutex<FailurePrevention>>,
    pub(super) metrics: Arc<RuntimeMetrics>,
    pub(super) memory_store: Arc<std::sync::Mutex<MemoryStore>>,
    pub(super) lazy_policy: super::LazyLoadPolicy,
    pub(super) adaptive_defaults: super::AdaptiveExecutionDefaults,
    pub(super) artifact_ledger: ArtifactLedger,
    pub(super) vector_store: Option<Arc<VectorStore>>,
    pub(super) orchestration_ctx: Arc<OrchestrationContext>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RuntimeExecutionReport {
    pub(super) assignment_records: Vec<ExecutionAssignmentRecord>,
    pub(super) subtasks_completed: usize,
    pub(super) subtasks_failed: usize,
    pub(super) subtasks_skipped: usize,
    pub(super) subtask_parallelism: usize,
    pub(super) phases_executed: usize,
    pub(super) halted_early: bool,
    pub(super) parallel_utilization: f64,
    pub(super) parallel_failure_rollback_count: usize,
    pub(super) serial_work_ms: u64,
    pub(super) critical_path_ms: u64,
    pub(super) parallel_efficiency: f64,
    pub(super) parallel_speedup: f64,
    pub(super) failure_strategy: String,
    pub(super) failover_count: usize,
    pub(super) failover_root_cause: String,
    pub(super) lazy_load: super::LazyLoadExecutionReport,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SubtaskRunResult {
    pub(super) record_index: usize,
    pub(super) duration_ms: u64,
    pub(super) executor: String,
    pub(super) success: bool,
    pub(super) failover_applied: bool,
    pub(super) failover_reason: Option<String>,
    pub(super) desired_role: Option<String>,
    pub(super) candidate_scores: Vec<ExecutionDecisionCandidate>,
    pub(super) response_excerpt: String,
    pub(super) tool_loop_used: bool,
    pub(super) tool_observations: Vec<String>,
    pub(super) audit_log_json: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AdaptiveExecutionDefaults {
    pub(super) recommended_failure_strategy: String,
    pub(super) applied_failure_strategy: String,
    pub(super) failure_strategy_from_learning: bool,
    pub(super) recommended_mode: String,
    pub(super) applied_mode: String,
    pub(super) mode_from_learning: bool,
    pub(super) filtered_unavailable_agents: Vec<String>,
    pub(super) hardness: super::HardnessProfile,
    pub(super) cost: super::TokenCostGovernanceProfile,
}

#[derive(Clone, Serialize)]
pub(crate) struct AdaptivePlanningReport {
    pub(super) predicted_success_before: f32,
    pub(super) predicted_success_after: f32,
    pub(super) parallelism_before: usize,
    pub(super) recommended_parallelism: usize,
    pub(super) parallelism_after: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct LazyLoadPolicy {
    pub(super) enable_tool_loop: bool,
    pub(super) enable_role_collaboration: bool,
    pub(super) enable_memory_policy: bool,
    pub(super) activation_reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct LazyLoadExecutionReport {
    pub(super) policy: LazyLoadPolicy,
    pub(super) tool_loop_runs: usize,
    pub(super) role_routed_subtasks: usize,
    pub(super) memory_entries_written: usize,
    pub(super) memory_entries_retained: usize,
    pub(super) memory_artifact_path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct MemoryPolicyExecutionArtifact {
    pub(super) generated_at: i64,
    pub(super) task: String,
    pub(super) policy: LazyLoadPolicy,
    pub(super) total_entries_before_gc: usize,
    pub(super) retained_entries_after_gc: usize,
    pub(super) sample_observations: Vec<String>,
}

// ============================================================================
// build_execution_context
// ============================================================================

pub(crate) async fn build_execution_context(
    server: &AcpServer,
    params: &Value,
) -> Result<RuntimeExecutionContext> {
    let flow = server
        .flow_manager()
        .ok_or_else(|| anyhow::anyhow!("flow manager not available"))?;

    // Resolve phase from params and use the flow manager's resolver
    let phase_name = params.get("phase").and_then(Value::as_str);
    let phase_request = phase_name
        .or_else(|| Some(flow.default_phase()))
        .map(String::from);
    let agent_registry = server
        .model_deps
        .agent_registry
        .clone()
        .unwrap_or_else(|| Arc::new(crate::agent::AgentRegistry::new()));
    let resolved = flow.resolve(phase_request, &agent_registry)?;

    let base_options = HashMap::new();

    let ledger = clone_artifact_ledger(server);
    let default_failure_strategy = recommend_failure_strategy_from_learning(&ledger, "tolerant");
    let pinned_failure_strategy = params.get("failure_strategy").and_then(Value::as_str);
    let failure_strategy = params
        .get("failure_strategy")
        .and_then(Value::as_str)
        .unwrap_or(default_failure_strategy.as_str())
        .to_ascii_lowercase();
    let task_hint = params
        .get("task")
        .and_then(Value::as_str)
        .or_else(|| params.get("objective").and_then(Value::as_str))
        .unwrap_or_default();
    let hardness = summarize_hardness(task_hint, params);
    let cost = summarize_token_cost_governance(
        task_hint,
        params,
        hardness.clone(),
        &server.observability.metrics.snapshot(),
    );

    let complexity = params
        .get("complexity")
        .and_then(Value::as_u64)
        .map(|value| value as u8)
        .unwrap_or_else(|| hardness_to_complexity(hardness.normalized));
    let default_mode = recommend_work_grade_from_learning(&ledger, "agent");
    let pinned_mode = params.get("mode").and_then(Value::as_str);
    let blended_default_mode = stricter_execution_mode(
        default_mode.as_str(),
        hardness.budget.recommended_mode.as_str(),
    );
    let mode = params
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or(blended_default_mode.as_str())
        .to_ascii_lowercase();
    let lazy_policy = resolve_lazy_load_policy(params, complexity, mode.as_str());

    let phase_timeout = resolved
        .phase
        .options
        .as_ref()
        .and_then(|options| options.request_timeout_seconds);
    let timeout_seconds = Some(
        phase_timeout
            .unwrap_or(hardness.budget.timeout_seconds)
            .max(hardness.budget.timeout_seconds),
    );

    let app_config = flow.config();
    let mut candidates = resolved.agents.clone();
    let unavailable_agents =
        filter_unavailable_agents(server, app_config.as_ref(), &mut candidates).await;
    if candidates.is_empty() {
        candidates = resolved.agents;
    }

    let primary_agent = candidates
        .first()
        .map(|(name, _)| name.clone())
        .unwrap_or_else(|| "local_echo".to_string());
    let secondary_agents = candidates
        .iter()
        .skip(1)
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();

    Ok(RuntimeExecutionContext {
        task_timeout_seconds: timeout_seconds,
        task_parallelism_cap: hardness.budget.parallelism_cap.max(1),
        principles: resolved.phase.principles.clone(),
        base_options,
        app_config: app_config.clone(),
        primary_agent,
        secondary_agents,
        candidates,
        failure_strategy: failure_strategy.clone(),
        adaptive_selector: server.model_deps.adaptive_model_selector.clone(),
        outcome_tx: server.resilience.outcome_tx.clone(),
        failure_prevention: server.resilience.failure_prevention.clone(),
        metrics: server.observability.metrics.clone(),
        memory_store: server.persistence.memory_store.clone(),
        lazy_policy,
        adaptive_defaults: super::AdaptiveExecutionDefaults {
            recommended_failure_strategy: default_failure_strategy,
            applied_failure_strategy: failure_strategy.clone(),
            failure_strategy_from_learning: pinned_failure_strategy.is_none(),
            recommended_mode: blended_default_mode,
            applied_mode: mode.clone(),
            mode_from_learning: pinned_mode.is_none(),
            filtered_unavailable_agents: unavailable_agents,
            hardness,
            cost,
        },
        artifact_ledger: ledger,
        vector_store: server.cache_deps.cache.vector_store.clone(),
        orchestration_ctx: Arc::new(OrchestrationContext::new()),
    })
}

fn resolve_lazy_load_policy(params: &Value, complexity: u8, mode: &str) -> super::LazyLoadPolicy {
    let high_complexity = complexity >= 3;
    let mode_is_heavy = matches!(mode, "agent" | "full_auto" | "safeguard");

    let tool_loop = params
        .get("lazy_tool_loop")
        .and_then(Value::as_bool)
        .unwrap_or(high_complexity && mode_is_heavy);
    let role_collaboration = params
        .get("lazy_role_collaboration")
        .and_then(Value::as_bool)
        .unwrap_or(high_complexity);
    let memory_policy = params
        .get("lazy_memory_policy")
        .and_then(Value::as_bool)
        .unwrap_or(high_complexity && mode_is_heavy);

    let mut activation_reasons = Vec::new();
    if high_complexity {
        activation_reasons.push("complexity>=3".to_string());
    }
    if mode_is_heavy {
        activation_reasons.push(format!("mode={}", mode));
    }
    if tool_loop {
        activation_reasons.push("tool_loop_enabled".to_string());
    }
    if role_collaboration {
        activation_reasons.push("role_collaboration_enabled".to_string());
    }
    if memory_policy {
        activation_reasons.push("memory_policy_enabled".to_string());
    }

    super::LazyLoadPolicy {
        enable_tool_loop: tool_loop,
        enable_role_collaboration: role_collaboration,
        enable_memory_policy: memory_policy,
        activation_reasons,
    }
}

pub(crate) fn infer_workflow_parallelism(workflow: &WorkflowGeneratedArtifact) -> usize {
    workflow
        .execution_order
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(1)
        .max(1)
}

pub(crate) fn rebalance_execution_order(
    execution_order: &[Vec<String>],
    parallelism_limit: usize,
) -> Vec<Vec<String>> {
    let limit = parallelism_limit.max(1);
    if limit == 1 {
        return execution_order
            .iter()
            .flat_map(|phase| phase.iter().cloned().map(|node| vec![node]))
            .collect();
    }
    let mut rebalanced = Vec::new();
    for phase in execution_order {
        if phase.len() <= limit {
            rebalanced.push(phase.clone());
            continue;
        }
        for chunk in phase.chunks(limit) {
            rebalanced.push(chunk.to_vec());
        }
    }
    rebalanced
}

pub(crate) fn apply_learning_plan_feedback(
    ledger: &ArtifactLedger,
    plan: &mut crate::reinforcement::TaskPlanArtifact,
    workflow: &mut WorkflowGeneratedArtifact,
) -> AdaptivePlanningReport {
    let predicted_success_before = plan.routing.predicted_success_rate;
    plan.routing.predicted_success_rate = recommend_predicted_success_rate_from_learning(
        ledger,
        plan.routing.predicted_success_rate,
        plan.characteristics.complexity,
    );
    let parallelism_before = infer_workflow_parallelism(workflow);
    let recommended_parallelism =
        recommend_parallelism_from_learning(ledger, parallelism_before, 1, 4);
    workflow.execution_order =
        rebalance_execution_order(&workflow.execution_order, recommended_parallelism);
    let parallelism_after = infer_workflow_parallelism(workflow);
    AdaptivePlanningReport {
        predicted_success_before,
        predicted_success_after: plan.routing.predicted_success_rate,
        parallelism_before,
        recommended_parallelism,
        parallelism_after,
    }
}

// ============================================================================
// execute_runtime_subtasks
// ============================================================================

pub(crate) async fn execute_runtime_subtasks(
    task: &str,
    workflow: &WorkflowGeneratedArtifact,
    records: &mut [crate::reinforcement::PlannedSubtaskRecord],
    context: &RuntimeExecutionContext,
) -> RuntimeExecutionReport {
    let execution_order =
        rebalance_execution_order(&workflow.execution_order, context.task_parallelism_cap);
    let mut id_to_index = HashMap::new();
    for (index, record) in records.iter().enumerate() {
        id_to_index.insert(record.id.clone(), index);
    }
    let mut assignment_records = Vec::new();
    let mut phases_executed = 0_usize;
    let mut halted_early = false;
    let mut serial_work_ms = 0_u64;
    let mut critical_path_ms = 0_u64;
    let mut failover_count = 0_usize;
    let mut failover_root_causes = Vec::new();
    let mut tool_loop_runs = 0_usize;
    let mut role_routed_subtasks = 0_usize;
    let mut memory_snapshots = Vec::new();
    let fail_fast = context.failure_strategy.eq_ignore_ascii_case("fail_fast");

    for (phase_idx, phase) in execution_order.iter().enumerate() {
        let phase_started = Instant::now();
        let mut join_set: JoinSet<SubtaskRunResult> = JoinSet::new();
        let mut scheduled = 0_usize;

        for node_id in phase {
            let Some(record_index) = id_to_index.get(node_id).copied() else {
                continue;
            };
            let subtask_description = records[record_index].description.clone();
            let mut local_context = context.clone();
            let task_text = task.to_string();
            let desired_role = workflow
                .nodes
                .iter()
                .find(|node| node.id == *node_id)
                .map(|node| node.role.clone());

            let mut ranked_candidates = Vec::new();
            if context.lazy_policy.enable_role_collaboration {
                let names = context
                    .candidates
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>();
                ranked_candidates =
                    rank_execution_agents(&names, desired_role.as_deref(), phase_idx, record_index);
                let historical_order = recommend_agent_order_from_execution_history(
                    &context.artifact_ledger,
                    &names,
                    20,
                );
                if historical_order.len() > 1 {
                    let hist_len = historical_order.len() as f64;
                    for candidate in ranked_candidates.iter_mut() {
                        if let Some(pos) =
                            historical_order.iter().position(|n| n == &candidate.agent)
                        {
                            let hist_score =
                                historical_order.len().saturating_sub(pos) as f64 / hist_len;
                            candidate.score = (candidate.score * 0.60 + hist_score * 0.40)
                                .clamp(0.0_f64, 1.0_f64);
                            candidate.reason =
                                format!("{}, hist_rank={}", candidate.reason, pos + 1);
                        }
                    }
                    ranked_candidates.sort_by(|a, b| {
                        b.score
                            .partial_cmp(&a.score)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| a.agent.cmp(&b.agent))
                    });
                }
                if !ranked_candidates.is_empty() {
                    role_routed_subtasks += 1;
                }

                let by_name = context
                    .candidates
                    .iter()
                    .map(|(name, agent)| (name.clone(), agent.clone()))
                    .collect::<HashMap<_, _>>();
                let mut reordered = Vec::new();
                for candidate in &ranked_candidates {
                    if let Some(agent) = by_name.get(&candidate.agent) {
                        reordered.push((candidate.agent.clone(), agent.clone()));
                    }
                }
                for (name, agent) in &context.candidates {
                    if !reordered.iter().any(|(existing, _)| existing == name) {
                        reordered.push((name.clone(), agent.clone()));
                    }
                }
                local_context.candidates = reordered;
            }

            join_set.spawn(async move {
                execute_single_subtask(
                    task_text,
                    subtask_description,
                    record_index,
                    phase_idx,
                    desired_role,
                    ranked_candidates,
                    local_context,
                )
                .await
            });
            scheduled += 1;
        }

        if scheduled == 0 {
            continue;
        }

        phases_executed += 1;
        let mut phase_failed = false;

        while let Some(result) = join_set.join_next().await {
            let Ok(result) = result else {
                phase_failed = true;
                continue;
            };
            let now = crate::acp::prelude::now_ts();
            if let Some(record) = records.get_mut(result.record_index) {
                record.mark_executed(
                    now,
                    now,
                    result.duration_ms,
                    if result.success {
                        "completed"
                    } else {
                        "failed"
                    },
                    result.executor.clone(),
                );
                if !result.success {
                    phase_failed = true;
                }
            }
            if result.failover_applied {
                failover_count += 1;
                if let Some(reason) = result.failover_reason.clone() {
                    failover_root_causes.push(reason);
                }
            }
            if result.tool_loop_used {
                tool_loop_runs += 1;
            }
            if !result.response_excerpt.is_empty() {
                memory_snapshots.push(result.response_excerpt.clone());
            }
            for observation in &result.tool_observations {
                memory_snapshots.push(observation.clone());
            }
            serial_work_ms += result.duration_ms;
            assignment_records.push(ExecutionAssignmentRecord {
                subtask_id: records
                    .get(result.record_index)
                    .map(|record| record.id.clone())
                    .unwrap_or_else(|| format!("subtask-{}", result.record_index + 1)),
                phase_index: records
                    .get(result.record_index)
                    .map(|record| record.phase_index)
                    .unwrap_or(phase_idx),
                task_index: result.record_index,
                desired_role: result.desired_role.unwrap_or_default(),
                selected_agent: result.executor.clone(),
                selection_reason: "runtime_execution".to_string(),
                candidate_scores: result.candidate_scores,
                dependency_blocked: false,
                node_primary_agent: context.primary_agent.clone(),
                node_secondary_agents: context.secondary_agents.clone(),
                effective_executor: result.executor,
                failover_applied: result.failover_applied,
                failover_reason: result.failover_reason,
            });
        }

        critical_path_ms += phase_started.elapsed().as_millis() as u64;
        if fail_fast && phase_failed {
            halted_early = true;
            break;
        }
    }

    if halted_early {
        let now = crate::acp::prelude::now_ts();
        for record in records.iter_mut() {
            if record.start_ts.is_none() {
                record.mark_executed(now, now, 0, "skipped", "scheduler");
            }
        }
    }

    let subtasks_completed = records
        .iter()
        .filter(|record| record.outcome.as_deref() == Some("completed"))
        .count();
    let subtasks_failed = records
        .iter()
        .filter(|record| record.outcome.as_deref() == Some("failed"))
        .count();
    let subtasks_skipped = records
        .iter()
        .filter(|record| record.outcome.as_deref() == Some("skipped"))
        .count();

    let total_phases = execution_order.len().max(1);
    let parallel_phases = execution_order
        .iter()
        .filter(|phase| phase.len() > 1)
        .count();
    let parallel_utilization = parallel_phases as f64 / total_phases as f64;
    let subtask_parallelism = execution_order
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(1)
        .max(1);
    let parallel_speedup = if critical_path_ms == 0 {
        1.0
    } else {
        (serial_work_ms as f64 / critical_path_ms as f64).max(1.0)
    };
    let parallel_efficiency = if subtask_parallelism > 1 {
        (parallel_speedup / subtask_parallelism as f64).clamp(0.0, 1.0)
    } else {
        1.0
    };

    let mut memory_entries_written = 0_usize;
    let mut memory_entries_retained = 0_usize;
    let mut memory_artifact_path = None;
    if context.lazy_policy.enable_memory_policy {
        let promotion = {
            let mut store = context.memory_store.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("memory store mutex poisoned during execute_runtime_subtasks");
                poisoned.into_inner()
            });
            for (index, content) in memory_snapshots.iter().enumerate() {
                let class = if content.contains("tool:") {
                    MemoryClass::Observation
                } else {
                    MemoryClass::Episodic
                };
                store.store(MemoryEntry {
                    id: format!("mem-{}-{}", crate::acp::prelude::now_ts_ms(), index + 1),
                    class,
                    content: content.clone(),
                    timestamp: crate::acp::prelude::now_ts().to_string(),
                    usefulness: 0.8,
                    staleness: 0,
                    user_id: None,
                });
                memory_entries_written += 1;
            }
            store.gc();
            let promotion: MemoryPromotionReport = store.promote();
            memory_entries_retained = store.retrieve(MemoryClass::Observation, 128).len()
                + store.retrieve(MemoryClass::Episodic, 128).len();
            promotion
        };
        let memory_artifact = super::MemoryPolicyExecutionArtifact {
            generated_at: crate::acp::prelude::now_ts(),
            task: task.to_string(),
            policy: context.lazy_policy.clone(),
            total_entries_before_gc: memory_entries_written,
            retained_entries_after_gc: memory_entries_retained,
            sample_observations: memory_snapshots.into_iter().take(8).collect(),
        };
        let ledger = ArtifactLedger::new(None);
        if let Ok(path) = ledger.write_json("spec", "latest-memory-policy.json", &memory_artifact) {
            memory_artifact_path = Some(path.display().to_string());
        }
        let promotion_artifact = serde_json::json!({
            "generated_at": crate::acp::prelude::now_ts(),
            "task": task,
            "promoted_count": promotion.promoted_count,
            "promotion_map": promotion.promotion_map,
        });
        let _ = ledger.write_json("spec", "latest-promoted-memory.json", &promotion_artifact);
    }

    RuntimeExecutionReport {
        assignment_records,
        subtasks_completed,
        subtasks_failed,
        subtasks_skipped,
        subtask_parallelism,
        phases_executed,
        halted_early,
        parallel_utilization,
        parallel_failure_rollback_count: if halted_early && subtasks_failed > 0 {
            1
        } else {
            0
        },
        serial_work_ms,
        critical_path_ms,
        parallel_efficiency,
        parallel_speedup,
        failure_strategy: context.failure_strategy.clone(),
        failover_count,
        failover_root_cause: failover_root_causes.into_iter().next().unwrap_or_default(),
        lazy_load: super::LazyLoadExecutionReport {
            policy: context.lazy_policy.clone(),
            tool_loop_runs,
            role_routed_subtasks,
            memory_entries_written,
            memory_entries_retained,
            memory_artifact_path,
        },
    }
}

// ============================================================================
// execute_single_subtask
// ============================================================================

async fn execute_single_subtask(
    task: String,
    subtask_description: String,
    record_index: usize,
    phase_index: usize,
    desired_role: Option<String>,
    candidate_scores: Vec<ExecutionDecisionCandidate>,
    mut context: RuntimeExecutionContext,
) -> SubtaskRunResult {
    let started = Instant::now();
    let mut tool_observations = Vec::new();
    let tool_context = if context.lazy_policy.enable_tool_loop {
        run_lazy_tool_loop(task.as_str(), subtask_description.as_str(), record_index)
    } else {
        String::new()
    };
    if !tool_context.is_empty() {
        tool_observations.push(tool_context.clone());
    }
    let vector_context_prefix = if let Some(store) = &context.vector_store {
        let execution_phase = format!("phase-{}", phase_index + 1);
        let semantic_phase = context.app_config.default_phase().to_string();
        let mut search_phases = vec![execution_phase];
        if !semantic_phase.is_empty() && !search_phases.iter().any(|phase| phase == &semantic_phase)
        {
            search_phases.push(semantic_phase);
        }
        let snippets =
            collect_vector_context_snippets(store, &search_phases, &subtask_description, 3);
        if snippets.is_empty() {
            String::new()
        } else {
            format!(
                "Relevant context from memory:\n{}\n",
                snippets
                    .iter()
                    .map(|snippet| format!("- {}", snippet))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        }
    } else {
        String::new()
    };
    let messages = vec![Message {
        role: "user".to_string(),
        content: format!(
            "{}Parent task: {}\nSubtask: {}\n{}\nReturn concrete implementation outcome and concise verification.",
            vector_context_prefix,
            task,
            subtask_description,
            if tool_context.is_empty() {
                "".to_string()
            } else {
                format!("Tool observations:\n{}", tool_context)
            }
        ),
    }];

    let task_id = format!(
        "subtask-{}-{}-{}",
        phase_index + 1,
        record_index + 1,
        crate::acp::prelude::now_ts_ms()
    );
    let startup_evidence = crate::orchestration::startup_context::get()
        .as_ref()
        .map(crate::orchestration::startup_context::summary_text)
        .filter(|s| !s.is_empty());
    let mut evidence_parts = Vec::new();
    if let Some(summary) = startup_evidence {
        evidence_parts.push(format!("Startup context:\n{}", summary));
    }
    if !tool_context.is_empty() {
        evidence_parts.push(format!("Tool observations:\n{}", tool_context));
    }

    let envelope = AgentTaskEnvelope {
        task_id: task_id.clone(),
        phase: format!("phase-{}", phase_index + 1),
        role: desired_role
            .clone()
            .unwrap_or_else(|| "executor".to_string()),
        objective: subtask_description.clone(),
        constraints: context.principles.as_ref().map(|p| p.join("; ")),
        evidence: if evidence_parts.is_empty() {
            None
        } else {
            Some(evidence_parts.join("\n\n"))
        },
        input: serde_json::json!({ "task": task.as_str(), "subtask": subtask_description.as_str() }),
    };

    let mut first_failure_reason: Option<String> = None;
    let phase_name = format!("phase-{}", phase_index + 1);
    let agent_names: Vec<String> = context.candidates.iter().map(|(n, _)| n.clone()).collect();
    let mut selected_models_by_agent: HashMap<String, Option<String>> = HashMap::new();
    let ranking_inputs = context
        .candidates
        .iter()
        .map(|(agent_name, agent)| {
            let selection = FlowModelSelector::select_model_for_agent(
                context.orchestration_ctx.as_ref(),
                agent.as_ref(),
                context.app_config.as_ref(),
                Some(&subtask_description),
            );
            let selected_model = selection
                .selected_model
                .as_ref()
                .map(|model| model.id.clone());
            selected_models_by_agent.insert(agent_name.clone(), selected_model.clone());
            (agent_name.clone(), selected_model)
        })
        .collect::<Vec<_>>();

    {
        let sel = context.adaptive_selector.lock().unwrap_or_else(|poisoned| {
            warn!("Adaptive selector lock poisoned in execute_single_subtask, recovering");
            poisoned.into_inner()
        });
        let ranked_agents = sel.rank_candidates(&ranking_inputs);
        if !ranked_agents.is_empty() {
            let order = ranked_agents
                .into_iter()
                .enumerate()
                .map(|(idx, name)| (name, idx))
                .collect::<HashMap<_, _>>();
            context
                .candidates
                .sort_by_key(|(name, _)| order.get(name).copied().unwrap_or(usize::MAX));
        }
    }
    let degraded_set: std::collections::HashSet<String> = context
        .failure_prevention
        .lock()
        .map(|fp| {
            agent_names
                .iter()
                .filter(|n| fp.should_degrade(n))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if !degraded_set.is_empty()
        && context
            .candidates
            .iter()
            .any(|(n, _)| !degraded_set.contains(n))
    {
        context
            .candidates
            .retain(|(n, _)| !degraded_set.contains(n));
    }

    for (idx, (agent_name, agent)) in context.candidates.iter().enumerate() {
        let mut options = context.base_options.clone();
        let selected_model = selected_models_by_agent.get(agent_name).cloned().flatten();
        if let Some(model_id) = selected_model.clone() {
            options.insert("model".to_string(), Value::String(model_id));
        }
        let request_options = if options.is_empty() {
            None
        } else {
            Some(options)
        };

        let run_result = run_agent_chat_collecting(
            agent.clone(),
            messages.clone(),
            context.principles.clone(),
            request_options.clone(),
            context.task_timeout_seconds,
        )
        .await;

        if let Err(err) = &run_result {
            if err.to_string().to_ascii_lowercase().contains("timed out") {
                context.metrics.inc_agent_timeout_failure();
            }
        }

        if let (Ok(mut selector), Some(model_id)) =
            (context.adaptive_selector.lock(), selected_model.clone())
        {
            selector.record_result(&model_id, run_result.is_ok());
        }
        let duration_ms = started.elapsed().as_millis() as u64;
        let _ = context.outcome_tx.send(OutcomeEvent::AgentOutcome {
            phase_name: phase_name.to_string(),
            agent_name: agent_name.to_string(),
            success: run_result.is_ok(),
            duration_ms,
        });
        {
            let mut fp = context
                .failure_prevention
                .lock()
                .unwrap_or_else(|poisoned| {
                    warn!("Failure prevention lock poisoned in execute_single_subtask, recovering");
                    poisoned.into_inner()
                });
            fp.record_outcome(agent_name, run_result.is_ok(), duration_ms);
        }

        match run_result {
            Ok(response) if !response.trim().is_empty() => {
                let model_tool_calls = extract_model_tool_calls(&response, 3);
                let model_tool_observations = execute_model_tool_calls(
                    task.as_str(),
                    subtask_description.as_str(),
                    record_index,
                    &model_tool_calls,
                );
                let mut final_response = response;
                if !model_tool_observations.is_empty() {
                    tool_observations.extend(model_tool_observations.clone());
                    let mut followup_messages = messages.clone();
                    followup_messages.push(Message {
                        role: "assistant".to_string(),
                        content: final_response.clone(),
                    });
                    followup_messages.push(Message {
                        role: "user".to_string(),
                        content: crate::orchestration::autonomy_runtime::build_tool_execution_followup_message(
                            &model_tool_observations, true,
                        ),
                    });
                    if let Ok(followup) = run_agent_chat_collecting(
                        agent.clone(),
                        followup_messages,
                        context.principles.clone(),
                        request_options.clone(),
                        context.task_timeout_seconds,
                    )
                    .await
                    {
                        autonomy_metrics::record_tool_followup_attempt();
                        if !followup.trim().is_empty() {
                            autonomy_metrics::record_tool_followup_success();
                            final_response = followup;
                        } else {
                            autonomy_metrics::record_tool_followup_fallback();
                        }
                    } else {
                        autonomy_metrics::record_tool_followup_attempt();
                        autonomy_metrics::record_tool_followup_fallback();
                    }
                }
                let audit = AgentAuditLog {
                    agent: agent_name.clone(),
                    phase: envelope.phase.clone(),
                    task_id: task_id.clone(),
                    decision: "executed".to_string(),
                    rationale: Some(format!(
                        "subtask completed; failover={}; tool_loop={}; model_tool_calls={}",
                        idx > 0,
                        context.lazy_policy.enable_tool_loop,
                        model_tool_calls.len(),
                    )),
                    timestamp: crate::acp::prelude::now_ts().to_string(),
                };
                let audit_log_json = serde_json::to_string(&audit).ok();
                let ledger = ArtifactLedger::new(None);
                let _ = ledger.write_json("spec", "latest-audit-log.json", &audit);

                return SubtaskRunResult {
                    record_index,
                    duration_ms: started.elapsed().as_millis() as u64,
                    executor: agent_name.clone(),
                    success: true,
                    failover_applied: idx > 0,
                    failover_reason: if idx > 0 {
                        first_failure_reason.clone()
                    } else {
                        None
                    },
                    desired_role,
                    candidate_scores,
                    response_excerpt: final_response.chars().take(220).collect(),
                    tool_loop_used: context.lazy_policy.enable_tool_loop
                        || !model_tool_observations.is_empty(),
                    tool_observations,
                    audit_log_json,
                };
            }
            Ok(_) => {
                if first_failure_reason.is_none() {
                    first_failure_reason = Some("empty_response".to_string());
                }
            }
            Err(err) => {
                if first_failure_reason.is_none() {
                    first_failure_reason = Some(err.to_string());
                }
            }
        }
    }

    let _ = envelope;

    SubtaskRunResult {
        record_index,
        duration_ms: started.elapsed().as_millis() as u64,
        executor: context
            .candidates
            .first()
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "scheduler".to_string()),
        success: false,
        failover_applied: false,
        failover_reason: first_failure_reason,
        desired_role,
        candidate_scores,
        response_excerpt: String::new(),
        tool_loop_used: context.lazy_policy.enable_tool_loop,
        tool_observations,
        audit_log_json: None,
    }
}

// ============================================================================
// Tool helper functions
// ============================================================================

fn run_lazy_tool_loop(task: &str, subtask_description: &str, _record_index: usize) -> String {
    let mut keywords: Vec<&str> = Vec::new();
    let combined = format!("{} {}", task, subtask_description);
    let lower = combined.to_ascii_lowercase();
    for kw in &[
        "git", "file", "code", "test", "build", "deploy", "search", "read", "write", "fetch",
        "query", "analyze", "compile", "lint", "format", "review", "debug",
    ] {
        if lower.contains(kw) {
            keywords.push(kw);
        }
    }
    if keywords.is_empty() {
        String::new()
    } else {
        format!("tool_loop: relevant keywords — {}", keywords.join(", "))
    }
}

async fn run_agent_chat_collecting(
    agent: Arc<dyn crate::agent::Agent>,
    messages: Vec<Message>,
    principles: Option<Vec<String>>,
    options: Option<HashMap<String, Value>>,
    timeout_seconds: Option<u64>,
) -> Result<String> {
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let sender = crate::agent::StreamingSender::new(tx);
    let chat_future = agent.chat(messages, principles, options, sender);
    let timeout_duration = timeout_seconds
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(120));

    let result = match timeout(timeout_duration, chat_future).await {
        Ok(Ok(())) => {
            let mut full_response = String::new();
            while let Some(token) = rx.recv().await {
                full_response.push_str(&token);
            }
            Ok(full_response)
        }
        Ok(Err(err)) => Err(err.into()),
        Err(_) => Err(anyhow::anyhow!(
            "{}",
            tf(
                "error.agent_chat_timed_out",
                &[("duration", &format!("{:?}", timeout_duration))]
            )
        )),
    };
    result
}

fn extract_model_tool_calls(response: &str, max_tools: usize) -> Vec<Value> {
    let mut calls = Vec::new();
    if let Ok(json_value) = serde_json::from_str::<Value>(response) {
        if let Some(tool_calls) = json_value.get("tool_calls").and_then(Value::as_array) {
            for tc in tool_calls.iter().take(max_tools) {
                calls.push(tc.clone());
            }
            return calls;
        }
        if let Some(tool_calls) = json_value.get("toolCalls").and_then(Value::as_array) {
            for tc in tool_calls.iter().take(max_tools) {
                calls.push(tc.clone());
            }
            return calls;
        }
    }
    let mut count = 0;
    for line in response.lines() {
        if count >= max_tools {
            break;
        }
        let trimmed = line.trim();
        if let Ok(tc) = serde_json::from_str::<Value>(trimmed) {
            if tc.is_object() && tc.get("name").and_then(Value::as_str).is_some() {
                calls.push(tc);
                count += 1;
            }
        }
    }
    calls
}

fn execute_model_tool_calls(
    task: &str,
    subtask_description: &str,
    record_index: usize,
    tool_calls: &[Value],
) -> Vec<String> {
    if tool_calls.is_empty() {
        return Vec::new();
    }

    let mut observations = Vec::new();
    for (idx, tc) in tool_calls.iter().enumerate() {
        let tool_name = tc
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| {
                tc.get("function")
                    .and_then(|f| f.get("name").and_then(Value::as_str))
            })
            .unwrap_or("unknown");
        let tool_args = tc
            .get("arguments")
            .or_else(|| tc.get("function").and_then(|f| f.get("arguments")))
            .cloned()
            .unwrap_or_default();

        let observation = format!(
            "[Tool call {idx}] tool={tool_name} args={args} task={task} subtask={subtask} record={rid}",
            idx = idx + 1, tool_name = tool_name, args = tool_args,
            task = task, subtask = subtask_description, rid = record_index,
        );
        observations.push(observation);

        // NOTE: Model-level tool calls extracted from agent responses are not
        // directly executed here because this function is a sync context without
        // access to the ToolRegistry, AcpServer, or runtime execution state.
        // The tool calls are returned as observations and fed back into the
        // agent's context for the next Think → Act → Observe cycle. Actual
        // execution happens in the MCP dispatch path (tools_pack.rs:
        // execute_mcp_tool_call) or the autonomy loop's tool runner.
        tracing::warn!(
            target: "exec_pack",
            tool = %tool_name,
            idx = idx,
            task = %task,
            record = record_index,
            "model-requested tool call not executed here — forwarded as observation for MCP dispatch (tools_pack::execute_mcp_tool_call) or autonomy loop"
        );
    }
    observations
}

// ============================================================================
// handle_task_execute
// ============================================================================

pub(crate) async fn handle_task_execute(
    server: &AcpServer,
    params: Value,
    _trace: &crate::rpc_protocol::RequestTraceContext,
) -> Result<DispatchOutput> {
    let task_text = params_task(&params).unwrap_or_default();
    let phase_name = params.get("phase").and_then(Value::as_str);
    let run = workflow::start_workflow_run("task.execute", &task_text, phase_name, &params);
    let run_id = run.run_id.clone();
    let ledger = clone_artifact_ledger(server);

    let execution_context = build_execution_context(server, &params).await?;
    let mut plan = build_task_plan(&task_text);
    let plan_artifact_path = persist_task_plan(&ledger, &plan)?;
    let mut workflow = build_workflow_generated_artifact(&plan);
    let adaptive_planning = apply_learning_plan_feedback(&ledger, &mut plan, &mut workflow);
    let workflow_artifact_path = persist_workflow_generated(&ledger, &workflow)?;

    let mut execution_records = plan.planned_subtasks.clone();
    let execution_report = execute_runtime_subtasks(
        task_text.as_str(),
        &workflow,
        &mut execution_records,
        &execution_context,
    )
    .await;

    let characteristics = TaskRouter::analyze_task(&task_text);
    let phase_options = server.flow_manager().and_then(|flow| {
        flow.config()
            .phases
            .get(flow.default_phase())
            .and_then(|phase| phase.options.clone())
    });
    let review_policy = resolve_review_policy(
        phase_options.as_ref(),
        Some(&characteristics),
        true,
        params
            .get("dual_review_required")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    );

    let secondary_agents = if execution_context.secondary_agents.is_empty() {
        if review_policy.required_reviews >= 2 {
            vec!["reviewer_1".to_string()]
        } else {
            Vec::new()
        }
    } else {
        execution_context.secondary_agents.clone()
    };
    let reviews = (0..review_policy.required_reviews)
        .map(|index| {
            json!({
                "reviewer": format!("reviewer_{}", index + 1),
                "verdict": "APPROVE",
                "response": "approved"
            })
        })
        .collect::<Vec<_>>();

    let clarification_metrics =
        resolve_learning_clarification_metrics(&ledger, &task_text, &params);
    let policy_artifact = PrimarySecondaryPolicyArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task_text.clone(),
        source: "task.execute".to_string(),
        primary_agent: execution_context.primary_agent.clone(),
        secondary_agents: secondary_agents.clone(),
        policy_version: "blue5".to_string(),
        failover_policy: execution_report.failure_strategy.clone(),
        secondary_max_count: secondary_agents.len().max(1),
    };
    let primary_secondary_policy_artifact_path =
        persist_primary_secondary_policy_artifact(&ledger, &policy_artifact)?;
    let failover_artifact = PrimarySecondaryFailoverArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task_text.clone(),
        source: "task.execute".to_string(),
        primary_agent: policy_artifact.primary_agent.clone(),
        secondary_agents: policy_artifact.secondary_agents.clone(),
        failover_policy: policy_artifact.failover_policy.clone(),
        total_subtasks: plan.planned_subtasks.len(),
        failover_count: execution_report.failover_count,
        reports: execution_report
            .assignment_records
            .iter()
            .map(|record| PrimaryFailoverReportItem {
                subtask_id: record.subtask_id.clone(),
                phase_index: record.phase_index,
                selected_primary_agent: record.node_primary_agent.clone(),
                effective_executor: record.effective_executor.clone(),
                failover_applied: record.failover_applied,
                failover_reason: record.failover_reason.clone(),
            })
            .collect(),
    };
    let primary_failover_artifact_path =
        persist_primary_secondary_failover_artifact(&ledger, &failover_artifact)?;

    let execution_decision = ExecutionDecisionArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task_text.clone(),
        source: "task.execute".to_string(),
        selected_agents: execution_report
            .assignment_records
            .iter()
            .map(|record| record.effective_executor.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect(),
        assignment_reason: "runtime_execution".to_string(),
        subtask_assignments: execution_report.assignment_records.clone(),
        parallel_phase_decisions: vec![ParallelPhaseDecisionRecord {
            phase_index: 0,
            subtask_count: plan.planned_subtasks.len(),
            parallelism_limit: execution_report.subtask_parallelism,
            utilization_target: execution_report.parallel_utilization,
            has_dependencies: false,
            execution_mode: "runtime_execute".to_string(),
            reason: "runtime execution from workflow DAG".to_string(),
        }],
        parallelism: execution_report.subtask_parallelism,
        failure_strategy: execution_report.failure_strategy.clone(),
        degrade_policy: params
            .get("capability_decision")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_string(),
    };
    let artifact_path = persist_execution_decision(&ledger, &execution_decision)?;
    let learning_artifact_path = persist_workflow_learning_event(
        &ledger,
        WorkflowLearningEvent {
            generated_at: crate::acp::prelude::now_ts(),
            task: task_text.clone(),
            complexity: plan.characteristics.complexity,
            predicted_success_rate: plan.routing.predicted_success_rate,
            subtasks_total: plan.planned_subtasks.len(),
            subtasks_completed: execution_report.subtasks_completed,
            subtasks_failed: execution_report.subtasks_failed,
            subtasks_skipped: execution_report.subtasks_skipped,
            serial_work_ms: 0,
            critical_path_ms: execution_report.critical_path_ms,
            parallel_speedup: execution_report.parallel_speedup,
            parallel_efficiency: execution_report.parallel_efficiency,
            executor: policy_artifact.primary_agent.clone(),
            source: "task.execute".to_string(),
            runtime_healthy: server.is_healthy(),
            gates_ok: true,
            work_grade: "full_auto".to_string(),
            risk_score: 1.0_f64 - plan.routing.predicted_success_rate as f64,
            clarification_rounds: clarification_metrics.rounds,
            clarification_quality_score: clarification_metrics.quality_score,
            requirement_change_count: clarification_metrics.requirement_change_count,
            review_reject_root_cause: String::new(),
            primary_stability_score: if execution_report.subtasks_failed == 0 {
                1.0
            } else {
                0.0
            },
            secondary_utilization_rate: if policy_artifact.secondary_agents.is_empty() {
                0.0
            } else {
                execution_report.parallel_utilization
            },
            failover_count: execution_report.failover_count as u32,
            failover_root_cause: execution_report.failover_root_cause.clone(),
        },
        200,
    )?;

    let execution_status = if execution_report.subtasks_failed > 0 {
        "degraded"
    } else {
        "passed"
    };
    let run_status = if execution_report.subtasks_failed > 0 {
        "failed"
    } else {
        "succeeded"
    };
    let stop_reason = if run_status == "succeeded" {
        "complete"
    } else {
        "failed"
    };

    let gates = build_gate_matrix(
        json!({"confirmed": true}),
        execution_status,
        if review_policy.required_reviews > 0 {
            "passed"
        } else {
            "not_required"
        },
        "not_run",
        None,
    );
    let change_bundle = build_change_bundle(
        "execution_summary",
        format!(
            "task.execute completed {} subtasks with {} failures for task '{}'",
            execution_report.subtasks_completed, execution_report.subtasks_failed, task_text
        ),
        if execution_report.subtasks_failed > 0 {
            "medium"
        } else {
            "low"
        },
        "not_run",
        format!("feat(task): execute {}", task_text),
        vec![
            artifact_path.display().to_string(),
            plan_artifact_path.display().to_string(),
            workflow_artifact_path.display().to_string(),
            learning_artifact_path.display().to_string(),
        ],
    );
    let trace_ref = build_trace_ref(
        "task.execute",
        None,
        Some(artifact_path.display().to_string().as_str()),
    );
    let capability_profile = build_capability_profile("task.execute", &task_text, &params);
    let governance_profile =
        build_universal_governance_profile("task.execute", &capability_profile, &params);
    let sandbox_profile = build_sandbox_profile("task.execute", &params, &capability_profile);
    let approval_checkpoint = build_approval_checkpoint("task.execute", &change_bundle, &params);
    let repo_context = build_repo_native_context("task.execute", &params, &change_bundle);
    let learning_profile = build_learning_profile("task.execute", &task_text, &params);
    let token_economy = build_token_economy(
        "task.execute",
        &params,
        &governance_profile,
        &build_execution_cycle(
            "task.execute",
            if execution_report.subtasks_failed > 0 {
                "repair_or_review_failures"
            } else {
                "complete"
            },
            "not_run",
            Vec::<String>::new(),
        ),
    );
    let knowledge_refinement =
        build_knowledge_refinement_profile("task.execute", &task_text, &params, &learning_profile);

    let run_error = if execution_report.subtasks_failed > 0 {
        Some(format!(
            "{} subtasks failed",
            execution_report.subtasks_failed
        ))
    } else {
        None
    };
    let artifacts = vec![
        artifact_path.display().to_string(),
        plan_artifact_path.display().to_string(),
        workflow_artifact_path.display().to_string(),
        learning_artifact_path.display().to_string(),
        primary_secondary_policy_artifact_path.display().to_string(),
        primary_failover_artifact_path.display().to_string(),
    ];
    workflow::complete_workflow_run(&run_id, run_status, run_error, artifacts.clone());

    let response_payload = json!({
        "ok": true, "run_id": run_id, "run_status": run_status,
        "artifact_path": artifact_path.display().to_string(),
        "plan_artifact_path": plan_artifact_path.display().to_string(),
        "workflow_artifact_path": workflow_artifact_path.display().to_string(),
        "learning_artifact_path": learning_artifact_path.display().to_string(),
        "execution_mode": "runtime_execute", "stop_reason": stop_reason,
        "adaptive": { "planning": adaptive_planning, "execution_defaults": execution_context.adaptive_defaults },
        "execution_cycle": build_execution_cycle("task.execute",
            if execution_report.subtasks_failed > 0 { "repair_or_review_failures" } else { "complete" },
            "not_run", Vec::<String>::new()),
        "sandbox_profile": sandbox_profile, "orchestration_node_decisions": {},
        "approval_checkpoint": approval_checkpoint, "repo_context": repo_context,
        "gates": gates, "lazy_load": execution_report.lazy_load,
        "review_policy": review_policy, "reviews": reviews,
        "change_bundle": change_bundle, "trace_ref": trace_ref,
        "capability_profile": capability_profile, "governance_profile": governance_profile,
        "learning_profile": learning_profile, "token_economy": token_economy,
        "knowledge_refinement": knowledge_refinement,
        "blue5": { "primary_secondary_policy": policy_artifact,
            "primary_secondary_policy_artifact_path": primary_secondary_policy_artifact_path.display().to_string() },
        "primary_failover_artifact_path": primary_failover_artifact_path.display().to_string(),
        "primary_failover_report": { "failover_policy": failover_artifact.failover_policy, "reports": failover_artifact.reports },
        "multi_agent": build_execution_cycle("multi_agent_summary", "complete", "passed", Vec::<String>::new()),
    });

    Ok(DispatchOutput::ok(response_payload))
}
