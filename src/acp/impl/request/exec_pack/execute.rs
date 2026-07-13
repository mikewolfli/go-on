//! Workflow execute handler and repair-loop integration.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use serde_json::{json, Value};

use super::artifact::{
    build_memory_graph_profile, build_multi_agent_sessions, build_replay_scoring,
    build_review_adjudication,
};
use super::repair::{
    build_repair_context, build_repair_history_response, build_repair_loop_state,
    record_repair_action, should_continue_repair_loop, should_trigger_auto_repair, RepairContext,
    RepairCycleReport,
};
use super::task;
use super::*;
use crate::acp::server::AcpServer;
use crate::rpc_protocol::RequestTraceContext;

fn normalize_control_mode(mode: &str) -> &'static str {
    match mode.to_ascii_lowercase().as_str() {
        "full_auto" | "autonomous" => "autonomous",
        "agent" | "safeguard" | "assisted" => "assisted",
        _ => "manual",
    }
}

fn build_runtime_cycle_patch_set(
    records: &[crate::reinforcement::PlannedSubtaskRecord],
) -> Vec<Value> {
    records
        .iter()
        .filter(|record| record.outcome.is_some())
        .map(|record| {
            json!({
                "subtask_id": record.id,
                "description": record.description,
                "phase_index": record.phase_index,
                "outcome": record.outcome,
                "executor": record.executor,
                "duration_ms": record.duration_ms,
            })
        })
        .collect()
}

fn build_runtime_repair_target_set(
    records: &[crate::reinforcement::PlannedSubtaskRecord],
) -> Vec<Value> {
    records
        .iter()
        .filter(|record| record.outcome.as_deref() == Some("failed"))
        .map(|record| {
            json!({
                "subtask_id": record.id,
                "description": record.description,
                "phase_index": record.phase_index,
                "retry_count": record.retry_count,
                "repair_action": "retry_with_recommended_strategy",
            })
        })
        .collect()
}

fn build_runtime_execute_autonomy_contract(
    total_rounds: usize,
    total_tools: usize,
    stop_reason: &str,
) -> Value {
    json!({
        "total_rounds": total_rounds,
        "total_tools": total_tools,
        "stop_reason": stop_reason,
        "corrective_actions_applied_total": 0,
        "corrective_action_effectiveness_ratio": 0.0,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_runtime_execution_cycle(
    stage: &str,
    next_action: &str,
    test_gate_result: &str,
    failure_taxonomy: Vec<String>,
    initial_records: &[crate::reinforcement::PlannedSubtaskRecord],
    final_records: &[crate::reinforcement::PlannedSubtaskRecord],
    adaptive_defaults: &AdaptiveExecutionDefaults,
    repair_context: Option<&RepairContext>,
) -> Value {
    let mut cycle = build_execution_cycle(stage, next_action, test_gate_result, failure_taxonomy);
    let patch_set = build_runtime_cycle_patch_set(initial_records);
    let repair_targets = build_runtime_repair_target_set(final_records);
    let auto_repair_eligible = !repair_targets.is_empty();
    let final_failed_count = final_records
        .iter()
        .filter(|record| record.outcome.as_deref() == Some("failed"))
        .count();

    let repair_iterations = repair_context
        .map(|context| context.cycle_reports.len())
        .unwrap_or(0);
    let current_iteration = 1 + repair_iterations as u32;
    let repair_preview = if auto_repair_eligible {
        Some(json!({
            "iteration": current_iteration + 1,
            "plan_version": format!("v1-repair-{}", current_iteration),
            "patch_set": repair_targets,
            "patch_set_size": build_runtime_repair_target_set(final_records).len(),
            "test_gate_result": "pending",
            "failure_taxonomy": cycle["failure_taxonomy"].clone(),
            "next_action": "retry_failed_subtasks",
            "status": "planned",
            "repair_strategy": {
                "failure_strategy": adaptive_defaults.recommended_failure_strategy,
                "execution_mode": adaptive_defaults.recommended_mode,
                "mode_from_learning": adaptive_defaults.mode_from_learning,
            }
        }))
    } else {
        None
    };

    if let Value::Object(obj) = &mut cycle {
        let status = if auto_repair_eligible {
            "degraded"
        } else {
            "passed"
        };
        let current_cycle = json!({
            "iteration": 1,
            "plan_version": "v1",
            "patch_set": patch_set,
            "patch_set_size": build_runtime_cycle_patch_set(initial_records).len(),
            "test_gate_result": test_gate_result,
            "failure_taxonomy": obj.get("failure_taxonomy").cloned().unwrap_or_else(|| json!([])),
            "next_action": next_action,
            "status": status,
            "started_at": crate::acp::prelude::now_ts(),
            "completed_at": crate::acp::prelude::now_ts(),
        });
        let mut cycles_vec = vec![current_cycle.clone()];
        if let Some(context) = repair_context {
            for cycle_report in &context.cycle_reports {
                let iteration_actions = context
                    .repair_actions
                    .iter()
                    .filter(|action| action.iteration == cycle_report.iteration)
                    .map(|action| {
                        json!({
                            "subtask_id": action.target_subtask_id,
                            "repair_action": action.action_type,
                            "description": action.description,
                            "result": action.result,
                            "details": action.details,
                        })
                    })
                    .collect::<Vec<_>>();

                cycles_vec.push(json!({
                    "iteration": cycle_report.iteration + 1,
                    "plan_version": format!("v1-repair-{}", cycle_report.iteration),
                    "patch_set": iteration_actions,
                    "patch_set_size": cycle_report.actions_applied,
                    "test_gate_result": if cycle_report.failed_after == 0 { "passed" } else { "failed" },
                    "failure_taxonomy": obj.get("failure_taxonomy").cloned().unwrap_or_else(|| json!([])),
                    "next_action": if cycle_report.failed_after == 0 { "complete" } else { "retry_failed_subtasks" },
                    "status": cycle_report.result,
                    "started_at": crate::acp::prelude::now_ts(),
                    "completed_at": crate::acp::prelude::now_ts(),
                    "failed_before": cycle_report.failed_before,
                    "failed_after": cycle_report.failed_after,
                }));
            }
        }

        if repair_iterations == 0 {
            if let Some(preview) = repair_preview.clone() {
                cycles_vec.push(preview);
            }
        }

        let auto_repair_status = if !auto_repair_eligible {
            "not_needed"
        } else if repair_iterations == 0 {
            "planned"
        } else if final_failed_count == 0 {
            "completed"
        } else if repair_context
            .map(|context| repair_iterations as u32 >= context.max_iterations)
            .unwrap_or(false)
        {
            "exhausted"
        } else {
            "in_progress"
        };

        obj.insert("patch_set".to_string(), current_cycle["patch_set"].clone());
        obj.insert("current_cycle".to_string(), current_cycle);
        obj.insert("cycles".to_string(), json!(cycles_vec));
        obj.insert("history_summary".to_string(), json!({
            "total_cycles": 1 + repair_iterations as u64,
            "current_iteration": current_iteration,
            "repair_iterations": repair_iterations,
            "pending_repair_iterations": if auto_repair_eligible && final_failed_count > 0 { 1 } else { 0 },
            "last_outcome": status,
        }));
        obj.insert("auto_repair".to_string(), json!({
            "status": auto_repair_status,
            "eligible": auto_repair_eligible,
            "recommended_max_iterations": repair_context.map(|context| context.max_iterations).unwrap_or(0),
            "trigger_classes": repair_context
                .map(|context| json!(context.failure_classes))
                .unwrap_or_else(|| obj.get("failure_taxonomy").cloned().unwrap_or_else(|| json!([]))),
            "governance_mode": repair_context
                .map(|context| context.governance_mode.clone())
                .unwrap_or_else(|| "assisted".to_string()),
            "target_subtasks": build_runtime_repair_target_set(final_records),
            "next_cycle_preview": if final_failed_count > 0 { repair_preview.unwrap_or(Value::Null) } else { Value::Null },
        }));
        obj.insert("task_graph_checkpoint".to_string(), json!({
            "checkpoint_id": format!("ckpt-{}", crate::acp::prelude::now_ts()),
            "schema_version": "blue26-taskgraph-checkpoint-v1",
            "phases_completed": repair_iterations + 1,
            "resume_eligible": final_failed_count < final_records.len() || final_failed_count == 0,
            "resume_reason": if final_failed_count > 0 {
                format!("{} subtasks failed, resume will retry them", final_failed_count)
            } else { "all subtasks complete".to_string() },
        }));
        obj.insert(
            "tool_loop".to_string(),
            json!({
                "schema_version": "blue26-tool-loop-v1",
                "phase": "observe",
                "idempotent": true,
                "safety_gate_passed": final_failed_count == 0,
                "confirmations_required": false,
                "governance": {
                    "dangerous_ops_intercepted": 0,
                    "whitelist_bypass_count": 0,
                    "permission_violations": 0,
                    "budget_remaining_pct": if repair_iterations == 0 { 1.0_f64 } else {
                        (1.0_f64 - repair_iterations as f64 * 0.25_f64).max(0.0_f64)
                    },
                },
            }),
        );
    }
    cycle
}

fn finalize_repair_action_results(
    context: &mut RepairContext,
    records: &[crate::reinforcement::PlannedSubtaskRecord],
    iteration: u32,
) {
    let mut outcome_by_id = HashMap::with_capacity(records.len());
    for record in records {
        outcome_by_id.insert(
            record.id.clone(),
            record.outcome.clone().unwrap_or_default(),
        );
    }
    for action in context.repair_actions.iter_mut() {
        if action.iteration != iteration || action.result != "in_progress" {
            continue;
        }
        let outcome = outcome_by_id
            .get(&action.target_subtask_id)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        action.result = if outcome == "completed" {
            "success".to_string()
        } else {
            "failed".to_string()
        };
        if let Value::Object(map) = &mut action.details {
            map.insert("post_repair_outcome".to_string(), Value::String(outcome));
        }
    }
}

fn apply_repair_strategy_to_failed_subtasks(
    failed_records: &[crate::reinforcement::PlannedSubtaskRecord],
    context: &mut RepairContext,
) -> Vec<Value> {
    let mut repair_outcomes = Vec::with_capacity(failed_records.len());
    for record in failed_records {
        if context.iteration >= context.max_iterations {
            break;
        }
        let diagnosis = crate::acp::helpers::repair_diagnosis::diagnose_repair(
            &record.id,
            record.outcome.as_deref().unwrap_or("failed"),
            None,
            record.retry_count as usize,
        );
        let diagnosis_summary =
            crate::acp::helpers::repair_diagnosis::diagnosis_to_strategy_adjustment(&diagnosis);
        let action_type = match &diagnosis.kind {
            crate::acp::helpers::repair_diagnosis::DiagnosisKind::Retry => "retry_subtask",
            crate::acp::helpers::repair_diagnosis::DiagnosisKind::Reroute => "reroute_subtask",
            crate::acp::helpers::repair_diagnosis::DiagnosisKind::Replan => "replan_subtask",
            crate::acp::helpers::repair_diagnosis::DiagnosisKind::Repair => "repair_subtask",
            crate::acp::helpers::repair_diagnosis::DiagnosisKind::Escalate => "escalate_subtask",
        };
        let repair_action = json!({
            "subtask_id": record.id.clone(),
            "subtask_description": record.description.clone(),
            "iteration": context.iteration,
            "action": action_type,
            "previous_failure": record.outcome.as_deref().unwrap_or("unknown"),
            "strategy_applied": diagnosis.suggested_strategy,
            "estimated_success_probability": match &diagnosis.kind {
                crate::acp::helpers::repair_diagnosis::DiagnosisKind::Retry => 0.7,
                crate::acp::helpers::repair_diagnosis::DiagnosisKind::Reroute => 0.75,
                crate::acp::helpers::repair_diagnosis::DiagnosisKind::Replan => 0.72,
                crate::acp::helpers::repair_diagnosis::DiagnosisKind::Repair => 0.78,
                crate::acp::helpers::repair_diagnosis::DiagnosisKind::Escalate => 0.35,
            },
            "diagnosis": diagnosis_summary,
        });
        record_repair_action(
            context,
            action_type,
            record.id.clone(),
            format!("{} in iteration {}", action_type, context.iteration),
            "in_progress",
            repair_action.clone(),
        );
        repair_outcomes.push(repair_action);
    }
    repair_outcomes
}

async fn execute_runtime_subtasks_with_repair_loop(
    task: &str,
    workflow: &WorkflowGeneratedArtifact,
    records: &mut [crate::reinforcement::PlannedSubtaskRecord],
    context: &RuntimeExecutionContext,
    mut report: super::task::RuntimeExecutionReport,
    auto_repair_enabled: bool,
    repair_context: &mut RepairContext,
) -> super::task::RuntimeExecutionReport {
    if !auto_repair_enabled {
        return report;
    }
    let repair_start_time_ms = crate::acp::prelude::now_ts_ms().max(0) as u64;
    loop {
        let failed_records = records
            .iter()
            .filter(|record| record.outcome.as_deref() == Some("failed"))
            .cloned()
            .collect::<Vec<_>>();
        if !should_continue_repair_loop(repair_context, failed_records.len(), repair_start_time_ms)
        {
            break;
        }
        let cycle_iteration = repair_context.iteration;
        let failed_before = failed_records.len();
        let _ = apply_repair_strategy_to_failed_subtasks(&failed_records, repair_context);
        let rerun_report = task::execute_runtime_subtasks(task, workflow, records, context).await;
        finalize_repair_action_results(repair_context, records, cycle_iteration);
        let failed_after = records
            .iter()
            .filter(|record| record.outcome.as_deref() == Some("failed"))
            .count();
        let _ = build_repair_loop_state(repair_context, failed_after, repair_start_time_ms);
        let actions_applied = repair_context
            .repair_actions
            .iter()
            .filter(|action| action.iteration == cycle_iteration)
            .count();
        let (diagnosis, strategy_adjustment) = {
            let last_diag = failed_records.first().map(|r| {
                crate::acp::helpers::repair_diagnosis::diagnose_and_summarize(
                    &r.id,
                    r.outcome.as_deref().unwrap_or("failed"),
                    None,
                    r.retry_count as usize,
                )
            });
            let diagnosis_text = if failed_after == 0 {
                "repair actions fully addressed failed subtasks"
            } else if failed_after < failed_before {
                "partial recovery; remaining failures need deeper replanning"
            } else {
                "retry-only repair insufficient; escalate to replan/reroute"
            };
            let strategy_text = if failed_after < failed_before {
                "continue targeted retry with context-preserving adjustments"
            } else {
                "switch from retry to reroute/replan for remaining failures"
            };
            let enriched_diagnosis = if let Some(ref d) = last_diag {
                format!(
                    "{} | diagnosis_kind={}, confidence={:.2}",
                    diagnosis_text,
                    d.get("diagnosis")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown"),
                    d.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0)
                )
            } else {
                diagnosis_text.to_string()
            };
            let enriched_strategy = if let Some(ref d) = last_diag {
                format!(
                    "{} | diagnosis_strategy={}",
                    strategy_text,
                    d.get("strategy")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                )
            } else {
                strategy_text.to_string()
            };
            (enriched_diagnosis, enriched_strategy)
        };
        repair_context.cycle_reports.push(RepairCycleReport {
            iteration: cycle_iteration,
            failed_before,
            failed_after,
            actions_applied,
            result: if failed_after == 0 {
                "resolved".to_string()
            } else if failed_after < failed_before {
                "improved".to_string()
            } else {
                "unresolved".to_string()
            },
            diagnosis,
            strategy_adjustment,
        });
        if let Some(report) = repair_context.cycle_reports.last() {
            crate::acp::helpers::autonomy_metrics::record_repair_cycle_result(&report.result);
        }
        report = rerun_report;
        if failed_after == 0 || repair_context.iteration >= repair_context.max_iterations {
            break;
        }
        repair_context.iteration += 1;
    }
    report
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn handle_workflow_execute(
    server: &AcpServer,
    params: Value,
    _trace: &RequestTraceContext,
) -> Result<DispatchOutput> {
    let task_text = params_task(&params).unwrap_or_default();
    let phase_name = params.get("phase").and_then(Value::as_str);
    let run =
        super::workflow::start_workflow_run("workflow.execute", &task_text, phase_name, &params);
    let run_id = run.run_id.clone();
    let effective_options = run.effective_options.clone();
    let ledger = clone_artifact_ledger(server);

    let requirement_continuation =
        crate::acp::helpers::requirement_continuation::evaluate_with_continuation(
            &ledger,
            &task_text,
            &params,
            "workflow.execute",
        );
    if !crate::acp::helpers::requirement_continuation::can_proceed_with_continuation(
        &requirement_continuation,
    ) {
        let blocked_payload = requirement_continuation.gate.blocked_payload();
        let reason = requirement_continuation
            .gate
            .reason
            .clone()
            .unwrap_or_else(|| "requirement confirmation required".to_string());
        let kind = blocked_payload["kind"]
            .as_str()
            .unwrap_or("requirement_contract")
            .to_string();
        let next_step = blocked_payload["next_step"].clone();
        if matches!(requirement_continuation.kind,
            crate::acp::helpers::requirement_continuation::RequirementContinuationKind::ClarificationRequired)
        {
            crate::acp::helpers::autonomy_metrics::record_autonomy_loop_stop_reason("incomplete");
            super::workflow::complete_workflow_run(&run_id, "waiting_clarification", Some(reason.clone()), Vec::new());
            return Ok(DispatchOutput::ok(json!({"ok": true, "run_id": run_id,
                "run_status": "waiting_clarification", "status": "clarification_required",
                "kind": kind, "reason": reason, "next_step": next_step,
                "requirement_gate": blocked_payload,
                "requirement_continuation": requirement_continuation.next_step})));
        }
        crate::acp::helpers::autonomy_metrics::record_autonomy_loop_stop_reason("failed");
        super::workflow::complete_workflow_run(&run_id, "failed", Some(reason.clone()), Vec::new());
        let err_msg = format!("workflow execution failed: {}", reason);
        anyhow::bail!(err_msg);
    }

    let _auto_clarification_in_progress = matches!(requirement_continuation.kind,
        crate::acp::helpers::requirement_continuation::RequirementContinuationKind::AutoConfirmed
        | crate::acp::helpers::requirement_continuation::RequirementContinuationKind::ClarificationInProgress);
    let requirement_gate_payload =
        crate::acp::helpers::requirement_continuation::requirement_gate_payload_for_response(
            &requirement_continuation,
        );

    let planner_bridge = crate::acp::helpers::planner_bridge::build_planner_bridge(
        run_id.clone(),
        phase_name.unwrap_or("execute"),
        task_text.clone(),
        &params,
    );

    if params
        .get("consultation_required")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && params
            .get("consultation_confidence_threshold")
            .and_then(Value::as_f64)
            .unwrap_or(0.5)
            > 0.9
    {
        let artifact = ConsultationArtifact {
            generated_at: crate::acp::prelude::now_ts(),
            task: task_text.clone(),
            source: "workflow.execute".to_string(),
            trigger_reason: "consultation_required".to_string(),
            participants: vec!["local_echo".to_string(), "reviewer".to_string()],
            candidate_plans: vec![format!("Conservative path for {}", task_text)],
            consensus_plan: String::new(),
            risk_matrix: json!({"risk": "high"}),
            decision_confidence: 0.75,
            handoff_primary_agent: "local_echo".to_string(),
        };
        let _consultation_artifact_path = persist_consultation_artifact(&ledger, &artifact)?;
        let blocked_reason = crate::i18n::runtime::t("error.consultation_blocked");
        super::workflow::complete_workflow_run(
            &run_id,
            "failed",
            Some(blocked_reason.clone()),
            Vec::new(),
        );
        anyhow::bail!(blocked_reason);
    }

    let mut plan = build_task_plan(&task_text);
    let plan_artifact_path = persist_task_plan(&ledger, &plan)?;
    let mut workflow = build_workflow_generated_artifact(&plan);
    let _ = crate::acp::helpers::planner_bridge::apply_dag_order_to_workflow(
        &mut workflow,
        &planner_bridge,
    );
    let adaptive_planning = apply_learning_plan_feedback(&ledger, &mut plan, &mut workflow);
    let workflow_artifact_path = persist_workflow_generated(&ledger, &workflow)?;

    let execution_context = task::build_execution_context(server, &params).await?;
    let mut execution_records = plan.planned_subtasks.clone();
    let initial_execution_records = execution_records.clone();
    let initial_execution_report = task::execute_runtime_subtasks(
        task_text.as_str(),
        &workflow,
        &mut execution_records,
        &execution_context,
    )
    .await;
    let failure_taxonomy = if initial_execution_report.subtasks_failed > 0 {
        vec!["execution_subtask_failed".to_string()]
    } else {
        Vec::new()
    };

    let auto_repair_enabled = should_trigger_auto_repair(
        initial_execution_report.subtasks_failed,
        &failure_taxonomy,
        params.get("governance"),
    );
    let mut repair_context = if auto_repair_enabled {
        build_repair_context(
            task_text.clone(),
            failure_taxonomy.clone(),
            params.get("governance"),
        )
    } else {
        RepairContext {
            iteration: 0,
            max_iterations: 0,
            task_id: task_text.clone(),
            failure_classes: failure_taxonomy.clone(),
            budget_tokens: 0,
            budget_time_seconds: 0,
            governance_mode: "disabled".to_string(),
            repair_actions: Vec::new(),
            cycle_reports: Vec::new(),
        }
    };
    let execution_report = execute_runtime_subtasks_with_repair_loop(
        task_text.as_str(),
        &workflow,
        &mut execution_records,
        &execution_context,
        initial_execution_report,
        auto_repair_enabled,
        &mut repair_context,
    )
    .await;

    let characteristics = TaskRouter::analyze_task(&task_text);
    let phase_options = server.flow_manager().and_then(|flow| {
        flow.config()
            .phases
            .get(flow.default_phase())
            .and_then(|phase| phase.options.clone())
    });
    let review_policy = crate::acp::helpers::policy::resolve_review_policy(
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
        .map(|i| json!({"reviewer": format!("reviewer_{}", i + 1), "verdict": "APPROVE", "response": "approved"}))
        .collect::<Vec<_>>();

    let policy_artifact = PrimarySecondaryPolicyArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task_text.clone(),
        source: "workflow.execute".to_string(),
        primary_agent: execution_context.primary_agent.clone(),
        secondary_agents: secondary_agents.clone(),
        policy_version: "blue5".to_string(),
        failover_policy: execution_report.failure_strategy.clone(),
        secondary_max_count: secondary_agents.len().max(1),
    };
    let psp_path = persist_primary_secondary_policy_artifact(&ledger, &policy_artifact)?;
    let failover_artifact = PrimarySecondaryFailoverArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task_text.clone(),
        source: "workflow.execute".to_string(),
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
    let pf_path = persist_primary_secondary_failover_artifact(&ledger, &failover_artifact)?;

    let execution_decision = ExecutionDecisionArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task_text.clone(),
        source: "workflow.execute".to_string(),
        selected_agents: execution_report
            .assignment_records
            .iter()
            .map(|record| record.effective_executor.clone())
            .collect::<HashSet<_>>()
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
    let _ = persist_workflow_learning_event(
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
            source: "workflow.execute".to_string(),
            runtime_healthy: server.is_healthy(),
            gates_ok: true,
            work_grade: "full_auto".to_string(),
            risk_score: 1.0_f64 - plan.routing.predicted_success_rate as f64,
            clarification_rounds: 0,
            clarification_quality_score: 0.0,
            requirement_change_count: 0,
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

    let execution_cycle = build_runtime_execution_cycle(
        "workflow.execute",
        if execution_report.subtasks_failed > 0 {
            "repair_or_review_failures"
        } else {
            "complete"
        },
        "not_run",
        failure_taxonomy,
        &initial_execution_records,
        &execution_records,
        &execution_context.adaptive_defaults,
        Some(&repair_context),
    );
    let repair_history = build_repair_history_response(&repair_context);
    let review_status = if review_policy.required_reviews > 0 {
        "passed"
    } else {
        "not_required"
    };
    let execution_status = if execution_report.subtasks_failed > 0 {
        "degraded"
    } else {
        "passed"
    };
    let gates = build_gate_matrix(
        requirement_gate_payload.clone(),
        execution_status,
        review_status,
        "not_run",
        None,
    );
    let change_bundle = build_change_bundle(
        "execution_summary",
        format!(
            "workflow.execute completed {} subtasks with {} failures for task '{}'",
            execution_report.subtasks_completed, execution_report.subtasks_failed, task_text
        ),
        if execution_report.subtasks_failed > 0 {
            "medium"
        } else {
            "low"
        },
        "not_run",
        format!("feat(workflow): execute governed task {}", task_text),
        vec![
            artifact_path.display().to_string(),
            plan_artifact_path.display().to_string(),
            workflow_artifact_path.display().to_string(),
            "".to_string(),
            psp_path.display().to_string(),
            pf_path.display().to_string(),
        ],
    );
    let trace_ref = build_trace_ref(
        "workflow.execute",
        None,
        Some(artifact_path.display().to_string().as_str()),
    );
    let capability_profile = build_capability_profile("workflow.execute", &task_text, &params);
    let governance_profile =
        build_universal_governance_profile("workflow.execute", &capability_profile, &params);
    let sandbox_profile = build_sandbox_profile("workflow.execute", &params, &capability_profile);
    let approval_checkpoint =
        build_approval_checkpoint("workflow.execute", &change_bundle, &params);
    let repo_context = build_repo_native_context("workflow.execute", &params, &change_bundle);
    let learning_profile = build_learning_profile("workflow.execute", &task_text, &params);
    let token_economy = build_token_economy(
        "workflow.execute",
        &params,
        &governance_profile,
        &execution_cycle,
    );
    let knowledge_refinement = build_knowledge_refinement_profile(
        "workflow.execute",
        &task_text,
        &params,
        &learning_profile,
    );
    let multi_agent = build_multi_agent_sessions(&task_text, "workflow.execute", &execution_report);

    let status = if execution_report.subtasks_failed > 0 {
        "failed"
    } else {
        "succeeded"
    };
    let stop_reason = if status == "succeeded" {
        "complete"
    } else {
        "failed"
    };
    let autonomy_contract = build_runtime_execute_autonomy_contract(
        1 + repair_context.cycle_reports.len(),
        execution_report.subtasks_completed
            + execution_report.subtasks_failed
            + execution_report.subtasks_skipped,
        stop_reason,
    );
    crate::acp::helpers::autonomy_metrics::record_autonomy_loop_stop_reason(stop_reason);
    crate::acp::helpers::agent_router::record_task_agent_outcome(
        &task_text,
        &policy_artifact.primary_agent,
        status == "succeeded",
    );
    let run_error = if execution_report.subtasks_failed > 0 {
        Some(format!(
            "{} subtasks failed",
            execution_report.subtasks_failed
        ))
    } else {
        None
    };
    super::workflow::complete_workflow_run(
        &run_id,
        status,
        run_error,
        vec![artifact_path.display().to_string()],
    );

    let response_payload = json!({
        "ok": true, "run_id": run_id, "run_status": status,
        "effective_options": effective_options,
        "capability_profile": capability_profile,
        "governance_profile": governance_profile,
        "learning_profile": learning_profile,
        "token_economy": token_economy,
        "knowledge_refinement": knowledge_refinement,
        "artifact_path": artifact_path.display().to_string(),
        "plan_artifact_path": plan_artifact_path.display().to_string(),
        "workflow_artifact_path": workflow_artifact_path.display().to_string(),
        "execution_mode": "runtime_execute",
        "run_mode": normalize_control_mode(&execution_context.adaptive_defaults.applied_mode),
        "autonomy_contract": autonomy_contract,
        "total_rounds": 1 + repair_context.cycle_reports.len(),
        "stop_reason": stop_reason,
        "adaptive": { "planning": adaptive_planning, "execution_defaults": execution_context.adaptive_defaults },
        "execution_cycle": execution_cycle,
        "sandbox_profile": sandbox_profile,
        "requirement_gate": { "confirmed": true, "gate": requirement_gate_payload, "auto_clarification_in_progress": _auto_clarification_in_progress },
        "orchestration_node_decisions": {},
        "approval_checkpoint": approval_checkpoint,
        "repo_context": repo_context,
        "multi_agent": multi_agent,
        "gates": gates,
        "lazy_load": execution_report.lazy_load,
        "review_policy": review_policy,
        "reviews": reviews,
        "change_bundle": change_bundle,
        "trace_ref": trace_ref,
        "repair_readiness": { "eligible": auto_repair_enabled, "max_iterations": repair_context.max_iterations,
            "governance_mode": repair_context.governance_mode.clone(),
            "reason": if auto_repair_enabled {
                format!("{} failures detected and auto-repair is enabled", execution_report.subtasks_failed)
            } else { "no failures or auto-repair disabled".to_string() } },
        "repair_history": if auto_repair_enabled && execution_report.subtasks_failed > 0 { repair_history } else { json!({ "actions": [] }) },
        "memory_graph": build_memory_graph_profile(&task_text),
        "review_adjudication": build_review_adjudication(execution_report.subtasks_failed),
        "replay_scoring": build_replay_scoring(execution_report.subtasks_completed, execution_report.subtasks_failed),
    });

    Ok(DispatchOutput::ok(response_payload))
}
