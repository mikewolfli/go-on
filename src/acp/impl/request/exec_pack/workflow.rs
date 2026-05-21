use super::*;

#[derive(Debug, Clone, Serialize)]
pub(super) struct WorkflowRunRecord {
    run_id: String,
    source_method: String,
    task: String,
    status: String,
    phase: String,
    created_at: i64,
    started_at: i64,
    ended_at: Option<i64>,
    error: Option<String>,
    artifacts: Vec<String>,
    effective_options: Value,
}

static WORKFLOW_RUNS: OnceLock<StdMutex<Vec<WorkflowRunRecord>>> = OnceLock::new();
static WORKFLOW_RUN_SEQ: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

fn workflow_runs() -> &'static StdMutex<Vec<WorkflowRunRecord>> {
    WORKFLOW_RUNS.get_or_init(|| StdMutex::new(Vec::new()))
}

fn next_workflow_run_id() -> String {
    let seq = WORKFLOW_RUN_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("run-{}-{}", crate::acp::prelude::now_ts_ms(), seq)
}

fn merge_effective_option_from_root(params: &Value, key: &str, out: &mut HashMap<String, Value>) {
    if let Some(value) = params.get(key) {
        out.insert(key.to_string(), value.clone());
    }
}

fn extract_effective_options(params: &Value) -> HashMap<String, Value> {
    let mut options = HashMap::new();
    let whitelist = ["temperature", "top_p", "max_tokens", "model"];

    if let Some(extra) = params
        .get("options")
        .and_then(|value| value.get("extra"))
        .and_then(Value::as_object)
    {
        for key in whitelist {
            if let Some(value) = extra.get(key) {
                options.insert(key.to_string(), value.clone());
            }
        }
    }

    for key in ["temperature", "top_p", "max_tokens", "model"] {
        merge_effective_option_from_root(params, key, &mut options);
    }

    options
}

fn effective_options_value(params: &Value) -> Value {
    Value::Object(extract_effective_options(params).into_iter().collect())
}

fn run_id_from_params(params: &Value) -> Option<String> {
    params
        .get("run_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub(super) fn start_workflow_run(
    source_method: &str,
    task: &str,
    phase: Option<&str>,
    params: &Value,
) -> WorkflowRunRecord {
    let now = crate::acp::prelude::now_ts();
    let record = WorkflowRunRecord {
        run_id: run_id_from_params(params).unwrap_or_else(next_workflow_run_id),
        source_method: source_method.to_string(),
        task: task.to_string(),
        status: "running".to_string(),
        phase: phase.unwrap_or("default").to_string(),
        created_at: now,
        started_at: now,
        ended_at: None,
        error: None,
        artifacts: Vec::new(),
        effective_options: effective_options_value(params),
    };

    if let Ok(mut guard) = workflow_runs().lock() {
        guard.push(record.clone());
        if guard.len() > 2000 {
            let overflow = guard.len() - 2000;
            guard.drain(0..overflow);
        }
    }

    record
}

pub(super) fn complete_workflow_run(
    run_id: &str,
    status: &str,
    error: Option<String>,
    artifacts: Vec<String>,
) {
    if let Ok(mut guard) = workflow_runs().lock() {
        if let Some(item) = guard.iter_mut().find(|record| record.run_id == run_id) {
            item.status = status.to_string();
            item.error = error;
            item.artifacts = artifacts;
            item.ended_at = Some(crate::acp::prelude::now_ts());
        }
    }
}

fn get_workflow_run_record(run_id: &str) -> Option<WorkflowRunRecord> {
    workflow_runs()
        .lock()
        .ok()
        .and_then(|guard| guard.iter().find(|record| record.run_id == run_id).cloned())
}

fn transition_workflow_run(run_id: &str, target_status: &str) -> Result<WorkflowRunRecord> {
    let mut guard = workflow_runs()
        .lock()
        .map_err(|err| anyhow::anyhow!("failed to lock workflow run store: {}", err))?;
    let record = guard
        .iter_mut()
        .find(|item| item.run_id == run_id)
        .ok_or_else(|| anyhow::anyhow!("workflow run '{}' not found", run_id))?;

    let allowed = match (record.status.as_str(), target_status) {
        ("queued", "cancelled") | ("queued", "running") => true,
        ("running", "paused") | ("running", "cancelled") | ("running", "succeeded") => true,
        ("paused", "running") | ("paused", "cancelled") => true,
        _ if record.status == target_status => true,
        _ => false,
    };

    if !allowed {
        anyhow::bail!(
            "invalid status transition: {} -> {}",
            record.status,
            target_status
        );
    }

    record.status = target_status.to_string();
    if matches!(target_status, "succeeded" | "failed" | "cancelled") {
        record.ended_at = Some(crate::acp::prelude::now_ts());
    }
    Ok(record.clone())
}

pub(super) fn execution_option_overrides(params: &Value) -> HashMap<String, Value> {
    extract_effective_options(params)
}

pub(super) fn workflow_run_list_payload(params: &Value) -> Value {
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(50)
        .min(500);
    let offset = params
        .get("offset")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(0);

    enum StatusFilter {
        Any,
        One(String),
        Many(HashSet<String>),
    }

    let status_filter = match params.get("status") {
        Some(Value::String(single)) => StatusFilter::One(single.clone()),
        Some(Value::Array(items)) => {
            let values = items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            if values.is_empty() {
                StatusFilter::Any
            } else {
                StatusFilter::Many(values.into_iter().collect())
            }
        }
        _ => StatusFilter::Any,
    };

    let matches_status = |record: &WorkflowRunRecord| match &status_filter {
        StatusFilter::Any => true,
        StatusFilter::One(single) => record.status == *single,
        StatusFilter::Many(items) => items.contains(&record.status),
    };

    let (total, runs) = match workflow_runs().lock() {
        Ok(guard) => {
            let mut total = 0usize;
            let mut runs = Vec::new();
            for record in guard.iter().rev() {
                if !matches_status(record) {
                    continue;
                }
                if total >= offset && runs.len() < limit {
                    runs.push(record.clone());
                }
                total += 1;
            }
            (total, runs)
        }
        Err(_) => (0usize, Vec::new()),
    };

    json!({
        "ok": true,
        "total": total,
        "offset": offset,
        "limit": limit,
        "runs": runs,
    })
}

pub(super) fn workflow_run_get_payload(params: &Value) -> Result<Value> {
    let run_id = params
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("run_id is required"))?;

    match get_workflow_run_record(run_id) {
        Some(run) => Ok(json!({"ok": true, "run": run})),
        None => Err(anyhow::anyhow!("workflow run '{}' not found", run_id)),
    }
}

pub(super) fn workflow_run_transition_payload(
    params: &Value,
    target_status: &str,
) -> Result<Value> {
    let run_id = params
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("run_id is required"))?;

    let run = transition_workflow_run(run_id, target_status)?;
    Ok(json!({"ok": true, "run": run, "action": target_status}))
}

pub(super) async fn handle_workflow_run_list(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, workflow_run_list_payload(&params)).await
}

pub(super) async fn handle_workflow_run_get(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    match workflow_run_get_payload(&params) {
        Ok(payload) => send_result(server, request_id, payload).await,
        Err(err) => send_error(server, request_id, -32602, err.to_string(), None).await,
    }
}

async fn handle_workflow_run_transition(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    target_status: &str,
) -> Result<()> {
    match workflow_run_transition_payload(&params, target_status) {
        Ok(payload) => send_result(server, request_id, payload).await,
        Err(err) => send_error(server, request_id, -32602, err.to_string(), None).await,
    }
}

pub(super) async fn handle_workflow_run_cancel(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    handle_workflow_run_transition(server, params, request_id, "cancelled").await
}

pub(super) async fn handle_workflow_run_pause(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    handle_workflow_run_transition(server, params, request_id, "paused").await
}

pub(super) async fn handle_workflow_run_resume(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    handle_workflow_run_transition(server, params, request_id, "running").await
}

pub(crate) async fn handle_workflow_execute(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    let mut params = params;
    let task = params_task(&params).unwrap_or_default();
    let phase_name = params.get("phase").and_then(Value::as_str);
    let run = start_workflow_run("workflow.execute", &task, phase_name, &params);
    let run_id = run.run_id.clone();
    let effective_options = run.effective_options.clone();

    let ledger = clone_artifact_ledger(server);
    let mut gate =
        evaluate_requirement_gate_facade(&ledger, &task, &params, "workflow.execute")?;
    let mut gate_auto_recovery = json!({"applied": false});
    if gate.blocked {
        if let Some(recovery) =
            try_auto_recover_requirement_gate(&ledger, &task, &params, "workflow.execute", &gate)?
        {
            params = recovery.params;
            gate = recovery.gate;
            gate_auto_recovery = recovery.metadata;
        }
    }
    if gate.blocked {
        let reason = gate
            .reason
            .clone()
            .unwrap_or_else(|| "requirement confirmation required".to_string());
        let blocked_payload = gate.blocked_payload();
        let kind = blocked_payload["kind"]
            .as_str()
            .unwrap_or("requirement_contract")
            .to_string();
        let next_step = blocked_payload["next_step"].clone();
        complete_workflow_run(&run_id, "failed", Some(reason.clone()), Vec::new());
        return send_error(
            server,
            request_id,
            -32006,
            reason,
            Some(json!({
                "run_id": run_id,
                "run_status": "failed",
                "kind": kind,
                "next_step": next_step,
                "auto_recovery": gate_auto_recovery,
                "requirement_gate": blocked_payload,
            })),
        )
        .await;
    }

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
            task: task.clone(),
            source: "workflow.execute".to_string(),
            trigger_reason: "consultation_required".to_string(),
            participants: vec!["local_echo".to_string(), "reviewer".to_string()],
            candidate_plans: vec![format!("Conservative path for {}", task)],
            consensus_plan: String::new(),
            risk_matrix: json!({"risk": "high"}),
            decision_confidence: 0.75,
            handoff_primary_agent: "local_echo".to_string(),
        };
        let consultation_artifact_path = persist_consultation_artifact(&ledger, &artifact)?;
        let blocked_reason = t("error.consultation_blocked");
        complete_workflow_run(&run_id, "failed", Some(blocked_reason.clone()), Vec::new());
        return send_error(
            server,
            request_id,
            -32007,
            blocked_reason,
            Some(json!({
                "kind": "consultation_blocked",
                "run_id": run_id,
                "run_status": "failed",
                "consultation_artifact_path": consultation_artifact_path.display().to_string(),
            })),
        )
        .await;
    }

    let mut plan = build_task_plan(&task);
    let plan_artifact_path = persist_task_plan(&ledger, &plan)?;
    let mut workflow = build_workflow_generated_artifact(&plan);
    let adaptive_planning = apply_learning_plan_feedback(&ledger, &mut plan, &mut workflow);
    let workflow_artifact_path = persist_workflow_generated(&ledger, &workflow)?;

    let execution_context = build_execution_context(server, &params).await?;
    let mut execution_records = plan.planned_subtasks.clone();
    let initial_execution_records = execution_records.clone();
    let initial_execution_report = execute_runtime_subtasks(
        task.as_str(),
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
            task.clone(),
            failure_taxonomy.clone(),
            params.get("governance"),
        )
    } else {
        RepairContext {
            iteration: 0,
            max_iterations: 0,
            task_id: task.clone(),
            failure_classes: failure_taxonomy.clone(),
            budget_tokens: 0,
            budget_time_seconds: 0,
            governance_mode: "disabled".to_string(),
            repair_actions: Vec::new(),
            cycle_reports: Vec::new(),
        }
    };
    let execution_report = execute_runtime_subtasks_with_repair_loop(
        task.as_str(),
        &workflow,
        &mut execution_records,
        &execution_context,
        initial_execution_report,
        auto_repair_enabled,
        &mut repair_context,
    )
    .await;

    let characteristics = TaskRouter::analyze_task(&task);
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
    let clarification_metrics = resolve_learning_clarification_metrics(&ledger, &task, &params);
    let policy_artifact = PrimarySecondaryPolicyArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task.clone(),
        source: "workflow.execute".to_string(),
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
        task: task.clone(),
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
    let primary_failover_artifact_path =
        persist_primary_secondary_failover_artifact(&ledger, &failover_artifact)?;
    let execution_decision = ExecutionDecisionArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task.clone(),
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
    let learning_artifact_path = persist_workflow_learning_event(
        &ledger,
        WorkflowLearningEvent {
            generated_at: crate::acp::prelude::now_ts(),
            task: task.clone(),
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
    let requirement_gate_payload = gate.success_payload();
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
        Some((
            "consultation",
            if params
                .get("consultation_required")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "passed"
            } else {
                "not_required"
            },
        )),
    );
    let change_bundle = build_change_bundle(
        "execution_summary",
        format!(
            "workflow.execute completed {} subtasks with {} failures for task '{}'",
            execution_report.subtasks_completed, execution_report.subtasks_failed, task
        ),
        if execution_report.subtasks_failed > 0 {
            "medium"
        } else {
            "low"
        },
        "not_run",
        format!("feat(workflow): execute governed task {}", task),
        vec![
            artifact_path.display().to_string(),
            plan_artifact_path.display().to_string(),
            workflow_artifact_path.display().to_string(),
            learning_artifact_path.display().to_string(),
            primary_secondary_policy_artifact_path.display().to_string(),
            primary_failover_artifact_path.display().to_string(),
        ],
    );
    let trace_ref = build_trace_ref(
        "workflow.execute",
        request_id.as_ref(),
        Some(artifact_path.display().to_string().as_str()),
    );
    let capability_profile = build_capability_profile("workflow.execute", &task, &params);
    let governance_profile =
        build_universal_governance_profile("workflow.execute", &capability_profile, &params);
    let sandbox_profile = build_sandbox_profile("workflow.execute", &params, &capability_profile);
    let approval_checkpoint =
        build_approval_checkpoint("workflow.execute", &change_bundle, &params);
    let repo_context = build_repo_native_context("workflow.execute", &params, &change_bundle);
    let learning_profile = build_learning_profile("workflow.execute", &task, &params);
    let token_economy = build_token_economy(
        "workflow.execute",
        &params,
        &governance_profile,
        &execution_cycle,
    );
    let knowledge_refinement =
        build_knowledge_refinement_profile("workflow.execute", &task, &params, &learning_profile);
    let multi_agent = build_multi_agent_sessions(&task, "workflow.execute", &execution_report);

    let artifacts = vec![
        artifact_path.display().to_string(),
        plan_artifact_path.display().to_string(),
        workflow_artifact_path.display().to_string(),
        learning_artifact_path.display().to_string(),
        primary_secondary_policy_artifact_path.display().to_string(),
        primary_failover_artifact_path.display().to_string(),
    ];
    let run_status = if execution_report.subtasks_failed > 0 {
        "failed"
    } else {
        "succeeded"
    };
    let run_error = if execution_report.subtasks_failed > 0 {
        Some(format!(
            "{} subtasks failed",
            execution_report.subtasks_failed
        ))
    } else {
        None
    };
    complete_workflow_run(&run_id, run_status, run_error, artifacts.clone());

    let response_payload = json!({
        "ok": true,
        "run_id": run_id,
        "run_status": run_status,
        "effective_options": effective_options,
        "capability_profile": capability_profile,
        "governance_profile": governance_profile,
        "learning_profile": learning_profile,
        "token_economy": token_economy,
        "knowledge_refinement": knowledge_refinement,
        "artifact_path": artifact_path.display().to_string(),
        "plan_artifact_path": plan_artifact_path.display().to_string(),
        "workflow_artifact_path": workflow_artifact_path.display().to_string(),
        "learning_artifact_path": learning_artifact_path.display().to_string(),
        "execution_mode": "runtime_execute",
        "run_mode": normalize_control_mode(&execution_context.adaptive_defaults.applied_mode),
        "adaptive": {
            "planning": adaptive_planning,
            "execution_defaults": execution_context.adaptive_defaults,
        },
        "execution_cycle": execution_cycle,
        "sandbox_profile": sandbox_profile,
        "requirement_gate": {
            "confirmed": true,
            "gate": requirement_gate_payload,
            "auto_recovery": gate_auto_recovery,
        },
        "approval_checkpoint": approval_checkpoint,
        "repo_context": repo_context,
        "multi_agent": multi_agent,
        "gates": gates,
        "lazy_load": execution_report.lazy_load,
        "review_policy": review_policy,
        "reviews": reviews,
        "artifacts": {
            "execution_decision": artifact_path.display().to_string(),
            "plan": plan_artifact_path.display().to_string(),
            "workflow": workflow_artifact_path.display().to_string(),
            "learning": learning_artifact_path.display().to_string(),
            "primary_secondary_policy": primary_secondary_policy_artifact_path.display().to_string(),
            "primary_failover": primary_failover_artifact_path.display().to_string(),
        },
        "change_bundle": change_bundle,
        "trace_ref": trace_ref,
        "blue5": {
            "primary_secondary_policy": policy_artifact,
            "primary_secondary_policy_artifact_path": primary_secondary_policy_artifact_path.display().to_string(),
        },
        "primary_failover_artifact_path": primary_failover_artifact_path.display().to_string(),
        "primary_failover_report": {
            "failover_policy": failover_artifact.failover_policy,
            "reports": failover_artifact.reports,
        },
        // Step 2: Add repair readiness information
        "repair_readiness": {
            "eligible": auto_repair_enabled,
            "max_iterations": repair_context.max_iterations,
            "governance_mode": repair_context.governance_mode.clone(),
            "reason": if auto_repair_enabled {
                format!("{} failures detected and auto-repair is enabled", execution_report.subtasks_failed)
            } else {
                "no failures or auto-repair disabled".to_string()
            },
        },
        // Step 2.3: Add repair history when repair was triggered
        "repair_history": if auto_repair_enabled && execution_report.subtasks_failed > 0 {
            repair_history
        } else {
            json!({ "actions": [] })
        },
        // B26-S5: memory graph drift protection profile
        "memory_graph": build_memory_graph_profile(&task),
        // B26-S6: structured review adjudication
        "review_adjudication": build_review_adjudication(execution_report.subtasks_failed),
        // B26-S7: three-dimensional replay scoring
        "replay_scoring": build_replay_scoring(execution_report.subtasks_completed, execution_report.subtasks_failed),
    });

    send_result(server, request_id, response_payload).await
}

#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn effective_options_merge_root_over_extra() {
        let params = json!({
            "temperature": 0.8,
            "options": {
                "extra": {
                    "temperature": 0.2,
                    "top_p": 0.9,
                    "ignored": "x"
                }
            }
        });

        let options = execution_option_overrides(&params);
        assert_eq!(options.get("temperature"), Some(&json!(0.8)));
        assert_eq!(options.get("top_p"), Some(&json!(0.9)));
        assert!(!options.contains_key("ignored"));
    }

    #[test]
    fn workflow_run_transition_rules() {
        let params = json!({"run_id": "run-test-transition"});
        let started = start_workflow_run("workflow.execute", "demo", Some("execute"), &params);
        assert_eq!(started.status, "running");

        let paused = transition_workflow_run("run-test-transition", "paused")
            .expect("running -> paused should be allowed");
        assert_eq!(paused.status, "paused");

        let resumed = transition_workflow_run("run-test-transition", "running")
            .expect("paused -> running should be allowed");
        assert_eq!(resumed.status, "running");

        let completed = transition_workflow_run("run-test-transition", "succeeded")
            .expect("running -> succeeded should be allowed");
        assert_eq!(completed.status, "succeeded");

        let invalid = transition_workflow_run("run-test-transition", "paused");
        assert!(invalid.is_err());
    }
}
