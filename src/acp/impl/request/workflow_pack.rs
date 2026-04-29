/// Step 2 Auto-Repair Loop Integration for Workflow Pack
/// Adds repair decision and governance control for workflow execution failures
#[allow(dead_code)]
fn should_enable_auto_repair_for_workflow(
    failure_count: usize,
    governance_config: Option<&Value>,
    execution_context: Option<&Value>,
) -> bool {
    if failure_count == 0 {
        return false;
    }

    let auto_repair_enabled = governance_config
        .and_then(|cfg| cfg.get("auto_repair_enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(true);

    if !auto_repair_enabled {
        return false;
    }

    let execution_mode = execution_context
        .and_then(|ctx| ctx.get("recommended_mode"))
        .and_then(Value::as_str)
        .unwrap_or("assisted");

    // Auto-repair is enabled in "assisted" and "autonomous" modes,
    // but disabled in "manual" or "safeguard" modes
    !matches!(execution_mode, "manual" | "safeguard")
}

#[allow(dead_code)]
fn enrich_response_with_repair_readiness(
    mut response: Value,
    failure_count: usize,
    governance_config: Option<&Value>,
) -> Value {
    if failure_count == 0 {
        return response;
    }

    let auto_repair_eligible =
        should_enable_auto_repair_for_workflow(failure_count, governance_config, None);

    if let Value::Object(ref mut obj) = response {
        obj.insert(
            "repair_readiness".to_string(),
            json!({
                "eligible": auto_repair_eligible,
                "reason": if auto_repair_eligible {
                    "failures detected and auto-repair is enabled"
                } else {
                    "repairs disabled or not eligible for this execution mode"
                },
                "next_step": if auto_repair_eligible {
                    "auto-repair loop will execute on next iteration"
                } else {
                    "manual review or re-execution required"
                },
            }),
        );
    }

    response
}

use super::*;

fn normalize_plan_control_mode(mode: Option<&str>) -> &'static str {
    match mode.unwrap_or("assisted").to_ascii_lowercase().as_str() {
        "full_auto" | "autonomous" => "autonomous",
        "agent" | "safeguard" | "assisted" => "assisted",
        _ => "manual",
    }
}

fn task_keywords(task: &str) -> Vec<String> {
    task.to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 4)
        .map(|token| token.to_string())
        .collect::<Vec<_>>()
}

fn is_related_task(candidate: &str, keywords: &[String]) -> bool {
    if keywords.is_empty() {
        return false;
    }
    let hay = candidate.to_ascii_lowercase();
    keywords.iter().any(|keyword| hay.contains(keyword))
}

fn build_task_memory_graph_and_recall(ledger: &ArtifactLedger, task: &str) -> (Value, Value) {
    let learning_path = ledger.latest_path("spec", "latest-learning.json");
    let keywords = task_keywords(task);

    let mut evidence = Vec::new();
    let mut nodes = vec![json!({
        "id": "task:current",
        "type": "task",
        "label": task,
    })];
    let mut edges = Vec::new();
    let mut related_failures = 0_usize;
    let mut sources = Vec::new();

    if let Ok(raw) = std::fs::read_to_string(&learning_path) {
        if let Ok(payload) = serde_json::from_str::<Value>(&raw) {
            if let Some(events) = payload.get("events").and_then(Value::as_array) {
                for event in events.iter().rev().take(24) {
                    let event_task = event
                        .get("task")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if !is_related_task(event_task, &keywords) {
                        continue;
                    }

                    let source = event
                        .get("source")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string();
                    let failed = event
                        .get("subtasks_failed")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let completed = event
                        .get("subtasks_completed")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let work_grade = event
                        .get("work_grade")
                        .and_then(Value::as_str)
                        .unwrap_or("agent");
                    let root_cause = event
                        .get("review_reject_root_cause")
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or("no_explicit_root_cause");

                    if failed > 0 {
                        related_failures += 1;
                    }
                    if !sources.iter().any(|existing| existing == &source) {
                        sources.push(source.clone());
                    }

                    let problem_node = format!("problem:{}:{}", source, failed);
                    let fix_node = format!("fix:{}:{}", source, work_grade);
                    let evidence_node = format!("evidence:{}:{}", source, completed + failed);

                    nodes.push(json!({
                        "id": problem_node,
                        "type": "problem",
                        "label": format!("{} failures (root: {})", failed, root_cause),
                    }));
                    nodes.push(json!({
                        "id": fix_node,
                        "type": "fix",
                        "label": format!("work_grade={}, source={}", work_grade, source),
                    }));
                    nodes.push(json!({
                        "id": evidence_node,
                        "type": "evidence",
                        "label": format!("completed={}, failed={}", completed, failed),
                    }));

                    edges.push(json!({"from": "task:current", "to": problem_node, "rel": "related_problem"}));
                    edges.push(json!({"from": problem_node, "to": fix_node, "rel": "fixed_by"}));
                    edges
                        .push(json!({"from": fix_node, "to": evidence_node, "rel": "verified_by"}));

                    evidence.push(json!({
                        "task": event_task,
                        "source": source,
                        "subtasks_failed": failed,
                        "subtasks_completed": completed,
                        "recommended_fix": format!("prefer work_grade={} with tighter gate checks", work_grade),
                        "root_cause": root_cause,
                    }));

                    if evidence.len() >= 6 {
                        break;
                    }
                }
            }
        }
    }

    let memory_graph = json!({
        "task": task,
        "nodes": nodes,
        "edges": edges,
        "summary": {
            "related_events": evidence.len(),
            "related_failures": related_failures,
            "sources": sources,
        }
    });

    let memory_recall = json!({
        "hit_count": evidence.len(),
        "sources": memory_graph["summary"]["sources"].clone(),
        "evidence": evidence,
        "recall_applied_before_planning": true,
    });

    (memory_graph, memory_recall)
}

pub(super) async fn handle_workflow_confirm(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    let task = params_task(&params).unwrap_or_default();
    let ready_to_confirm = params
        .get("ready_to_confirm")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !ready_to_confirm {
        return send_error(
            server,
            request_id,
            -32006,
            "clarification session not ready to confirm".to_string(),
            Some(json!({
                "kind": "clarification_session",
                "next_step": {"method": "workflow.clarify", "task": task}
            })),
        )
        .await;
    }

    let ledger = clone_artifact_ledger(server);
    let mut contract = parse_requirement_contract_from_params(&params, &task).unwrap_or(
        RequirementContractArtifact {
            generated_at: crate::acp::prelude::now_ts(),
            task: task.clone(),
            source: "workflow.confirm".to_string(),
            goal: String::new(),
            scope: String::new(),
            non_goals: Vec::new(),
            acceptance_criteria: Vec::new(),
            constraints: Vec::new(),
            open_questions: Vec::new(),
            ambiguity_score: 0,
            user_confirmed: false,
        },
    );
    contract.user_confirmed = params
        .get("user_confirmed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let requirement_contract_artifact_path = persist_requirement_contract(&ledger, &contract)?;
    let clarification_session = ClarificationSessionArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task.clone(),
        source: "workflow.confirm".to_string(),
        session_id: session_id_for_task(&task),
        round_index: params
            .get("round_index")
            .and_then(Value::as_u64)
            .unwrap_or(1) as usize,
        lead_clarifier: "local_echo".to_string(),
        assistant_clarifiers: Vec::new(),
        user_feedback: String::new(),
        resolved_points: vec!["requirement_confirmed".to_string()],
        open_points: Vec::new(),
        next_questions: Vec::new(),
        ready_to_confirm: true,
    };
    let clarification_session_artifact_path =
        persist_clarification_session_artifact(&ledger, &clarification_session)?;
    let learning_profile = build_learning_profile("workflow.confirm", &task, &params);
    let knowledge_refinement =
        build_knowledge_refinement_profile("workflow.confirm", &task, &params, &learning_profile);

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "requirement_contract": contract,
            "requirement_contract_artifact_path": requirement_contract_artifact_path.display().to_string(),
            "clarification_session": clarification_session,
            "clarification_session_artifact_path": clarification_session_artifact_path.display().to_string(),
            "learning_profile": learning_profile,
            "knowledge_refinement": knowledge_refinement,
        }),
    )
    .await
}

pub(super) async fn handle_workflow_clarify(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    let task = params_task(&params).unwrap_or_default();
    let ledger = clone_artifact_ledger(server);
    let clarification_session = ClarificationSessionArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task.clone(),
        source: "workflow.clarify".to_string(),
        session_id: session_id_for_task(&task),
        round_index: params
            .get("round_index")
            .and_then(Value::as_u64)
            .unwrap_or(1) as usize,
        lead_clarifier: "local_echo".to_string(),
        assistant_clarifiers: if params
            .get("clarify_collaboration_mode")
            .and_then(Value::as_str)
            == Some("multi_ai")
        {
            vec!["reviewer".to_string()]
        } else {
            Vec::new()
        },
        user_feedback: String::new(),
        resolved_points: Vec::new(),
        open_points: vec!["goal".to_string(), "scope".to_string()],
        next_questions: vec!["Please confirm goal and scope.".to_string()],
        ready_to_confirm: params
            .get("ready_to_confirm")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    };
    let clarification_session_artifact_path =
        persist_clarification_session_artifact(&ledger, &clarification_session)?;
    let learning_profile = build_learning_profile("workflow.clarify", &task, &params);
    let knowledge_refinement =
        build_knowledge_refinement_profile("workflow.clarify", &task, &params, &learning_profile);

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "clarification_session": clarification_session,
            "clarification_session_artifact_path": clarification_session_artifact_path.display().to_string(),
            "learning_profile": learning_profile,
            "knowledge_refinement": knowledge_refinement,
        }),
    )
    .await
}

pub(super) async fn handle_workflow_research(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    let task = params_task(&params).unwrap_or_default();
    if task.trim().is_empty() {
        return send_error(
            server,
            request_id,
            -32602,
            "task is required".to_string(),
            None,
        )
        .await;
    }

    let ledger = clone_artifact_ledger(server);
    let requirement_gate =
        evaluate_requirement_gate_facade(&ledger, &task, &params, "workflow.research")?;
    if requirement_gate.blocked {
        return send_error(
            server,
            request_id,
            -32006,
            requirement_gate
                .reason
                .clone()
                .unwrap_or_else(|| "requirement confirmation is required".to_string()),
            Some(requirement_gate.blocked_payload()),
        )
        .await;
    }

    let plan = build_task_plan(&task);
    let plan_artifact_path = persist_task_plan(&ledger, &plan)?;

    let planner_output = format!(
        "generated {} planned subtasks with predicted success {:.2}",
        plan.planned_subtasks.len(),
        plan.routing.predicted_success_rate
    );
    let researcher_output = params
        .get("research_focus")
        .or_else(|| params.get("context"))
        .and_then(Value::as_str)
        .unwrap_or("collected implementation evidence and risk notes")
        .to_string();
    let reviewer_output = if plan.characteristics.complexity >= 4 {
        "review suggests incremental rollout and rollback checkpoints".to_string()
    } else {
        "review suggests direct execution with standard verification".to_string()
    };
    let recommended_plan = plan
        .planned_subtasks
        .first()
        .map(|record| record.description.clone())
        .unwrap_or_else(|| format!("Execute task: {task}"));

    let artifact = WorkflowResearchArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task.clone(),
        planner_output,
        researcher_output,
        reviewer_output,
        recommended_plan,
    };
    let artifact_path = persist_workflow_research(&ledger, &artifact)?;
    let requirement_gate_payload = requirement_gate.success_payload();
    let execution_cycle = build_execution_cycle(
        "workflow.research",
        "review_research_artifact",
        "not_run",
        Vec::new(),
    );
    let gates = build_gate_matrix(
        requirement_gate_payload.clone(),
        "passed",
        "not_run",
        "not_run",
        Some(("research", "passed")),
    );
    let change_bundle = build_change_bundle(
        "analysis_only",
        format!(
            "workflow.research produced analysis artifacts for task '{}'",
            task
        ),
        "low",
        "not_run",
        format!("docs(research): capture analysis for {}", task),
        vec![
            artifact_path.display().to_string(),
            plan_artifact_path.display().to_string(),
        ],
    );
    let trace_ref = build_trace_ref(
        "workflow.research",
        request_id.as_ref(),
        Some(artifact_path.display().to_string().as_str()),
    );
    let capability_profile = build_capability_profile("workflow.research", &task, &params);
    let governance_profile =
        build_universal_governance_profile("workflow.research", &capability_profile, &params);
    let sandbox_profile = build_sandbox_profile("workflow.research", &params, &capability_profile);
    let approval_checkpoint =
        build_approval_checkpoint("workflow.research", &change_bundle, &params);
    let repo_context = build_repo_native_context("workflow.research", &params, &change_bundle);
    let learning_profile = build_learning_profile("workflow.research", &task, &params);
    let token_economy = build_token_economy(
        "workflow.research",
        &params,
        &governance_profile,
        &execution_cycle,
    );
    let knowledge_refinement =
        build_knowledge_refinement_profile("workflow.research", &task, &params, &learning_profile);

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "capability_profile": capability_profile,
            "governance_profile": governance_profile,
            "learning_profile": learning_profile,
            "token_economy": token_economy,
            "knowledge_refinement": knowledge_refinement,
            "artifact": artifact,
            "artifact_path": artifact_path.display().to_string(),
            "plan_artifact_path": plan_artifact_path.display().to_string(),
            "planned_subtasks": plan.planned_subtasks.len(),
            "execution_cycle": execution_cycle,
            "sandbox_profile": sandbox_profile,
            "requirement_gate": {
                "confirmed": true,
                "gate": requirement_gate_payload,
            },
            "approval_checkpoint": approval_checkpoint,
            "repo_context": repo_context,
            "gates": gates,
            "artifacts": {
                "research": artifact_path.display().to_string(),
                "plan": plan_artifact_path.display().to_string(),
            },
            "change_bundle": change_bundle,
            "trace_ref": trace_ref,
        }),
    )
    .await
}

pub(super) async fn handle_workflow_consult(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    let task = params_task(&params).unwrap_or_default();
    if task.trim().is_empty() {
        return send_error(
            server,
            request_id,
            -32602,
            "task is required".to_string(),
            None,
        )
        .await;
    }

    let ledger = clone_artifact_ledger(server);
    let requirement_gate =
        evaluate_requirement_gate_facade(&ledger, &task, &params, "workflow.consult")?;
    if requirement_gate.blocked {
        return send_error(
            server,
            request_id,
            -32006,
            requirement_gate
                .reason
                .clone()
                .unwrap_or_else(|| "requirement confirmation is required".to_string()),
            Some(requirement_gate.blocked_payload()),
        )
        .await;
    }

    let artifact = ConsultationArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task.clone(),
        source: "workflow.consult".to_string(),
        trigger_reason: params
            .get("trigger_reason")
            .and_then(Value::as_str)
            .unwrap_or("manual_consultation")
            .to_string(),
        participants: vec!["local_echo".to_string(), "reviewer".to_string()],
        candidate_plans: vec![format!("Analyze and execute: {}", task)],
        consensus_plan: format!("Proceed with governed workflow for {}", task),
        risk_matrix: json!({"risk": "moderate"}),
        decision_confidence: 0.75,
        handoff_primary_agent: "local_echo".to_string(),
    };
    let artifact_path = persist_consultation_artifact(&ledger, &artifact)?;
    let requirement_gate_payload = requirement_gate.success_payload();
    let execution_cycle = build_execution_cycle(
        "workflow.consult",
        "review_consensus_plan",
        "not_run",
        Vec::new(),
    );
    let gates = build_gate_matrix(
        requirement_gate_payload.clone(),
        "passed",
        "not_run",
        "not_run",
        Some(("consultation", "passed")),
    );
    let change_bundle = build_change_bundle(
        "consultation_only",
        format!(
            "workflow.consult produced a consensus plan for task '{}'",
            task
        ),
        "medium",
        "not_run",
        format!("docs(consult): capture consensus plan for {}", task),
        vec![artifact_path.display().to_string()],
    );
    let trace_ref = build_trace_ref(
        "workflow.consult",
        request_id.as_ref(),
        Some(artifact_path.display().to_string().as_str()),
    );
    let capability_profile = build_capability_profile("workflow.consult", &task, &params);
    let governance_profile =
        build_universal_governance_profile("workflow.consult", &capability_profile, &params);
    let sandbox_profile = build_sandbox_profile("workflow.consult", &params, &capability_profile);
    let approval_checkpoint =
        build_approval_checkpoint("workflow.consult", &change_bundle, &params);
    let repo_context = build_repo_native_context("workflow.consult", &params, &change_bundle);
    let learning_profile = build_learning_profile("workflow.consult", &task, &params);
    let token_economy = build_token_economy(
        "workflow.consult",
        &params,
        &governance_profile,
        &execution_cycle,
    );
    let knowledge_refinement =
        build_knowledge_refinement_profile("workflow.consult", &task, &params, &learning_profile);
    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "capability_profile": capability_profile,
            "governance_profile": governance_profile,
            "learning_profile": learning_profile,
            "token_economy": token_economy,
            "knowledge_refinement": knowledge_refinement,
            "artifact": artifact,
            "artifact_path": artifact_path.display().to_string(),
            "execution_cycle": execution_cycle,
            "sandbox_profile": sandbox_profile,
            "requirement_gate": {
                "confirmed": true,
                "gate": requirement_gate_payload,
            },
            "approval_checkpoint": approval_checkpoint,
            "repo_context": repo_context,
            "gates": gates,
            "artifacts": {
                "consultation": artifact_path.display().to_string(),
            },
            "change_bundle": change_bundle,
            "trace_ref": trace_ref,
        }),
    )
    .await
}

pub(super) async fn handle_workflow_generate(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    trace: &RequestTraceContext,
) -> Result<()> {
    let Some(task) = params.get("task").and_then(Value::as_str) else {
        return send_error(
            server,
            request_id,
            -32602,
            "task is required for workflow.generate".to_string(),
            None,
        )
        .await;
    };
    if task.trim().is_empty() {
        return send_error(
            server,
            request_id,
            -32602,
            "task is required for workflow.generate".to_string(),
            None,
        )
        .await;
    }

    let ledger = clone_artifact_ledger(server);
    let requirement_gate =
        evaluate_requirement_gate_facade(&ledger, task, &params, "workflow.generate")?;
    if requirement_gate.blocked {
        return send_error(
            server,
            request_id,
            -32006,
            requirement_gate
                .reason
                .clone()
                .unwrap_or_else(|| "requirement confirmation is required".to_string()),
            Some(requirement_gate.blocked_payload()),
        )
        .await;
    }

    let mut plan = build_task_plan(task);
    let plan_artifact_path = persist_task_plan(&ledger, &plan)?;
    let mut workflow = build_workflow_generated_artifact(&plan);
    let adaptive_planning = apply_learning_plan_feedback(&ledger, &mut plan, &mut workflow);
    let workflow_artifact_path = persist_workflow_generated(&ledger, &workflow)?;
    let requirement_gate_payload = requirement_gate.success_payload();
    let execution_cycle = build_execution_cycle(
        "workflow.generate",
        "review_generated_workflow",
        "not_run",
        Vec::new(),
    );
    let gates = build_gate_matrix(
        requirement_gate_payload.clone(),
        "passed",
        "not_run",
        "not_run",
        Some(("planning", "passed")),
    );
    let change_bundle = build_change_bundle(
        "planning_only",
        format!(
            "workflow.generate emitted a workflow graph for task '{}'",
            task
        ),
        "low",
        "not_run",
        format!("docs(workflow): capture generated workflow for {}", task),
        vec![
            plan_artifact_path.display().to_string(),
            workflow_artifact_path.display().to_string(),
        ],
    );
    let trace_ref = build_trace_ref(
        "workflow.generate",
        request_id.as_ref(),
        Some(workflow_artifact_path.display().to_string().as_str()),
    );
    let capability_profile = build_capability_profile("workflow.generate", task, &params);
    let governance_profile =
        build_universal_governance_profile("workflow.generate", &capability_profile, &params);
    let sandbox_profile = build_sandbox_profile("workflow.generate", &params, &capability_profile);
    let approval_checkpoint =
        build_approval_checkpoint("workflow.generate", &change_bundle, &params);
    let repo_context = build_repo_native_context("workflow.generate", &params, &change_bundle);
    let learning_profile = build_learning_profile("workflow.generate", task, &params);
    let token_economy = build_token_economy(
        "workflow.generate",
        &params,
        &governance_profile,
        &execution_cycle,
    );
    let knowledge_refinement =
        build_knowledge_refinement_profile("workflow.generate", task, &params, &learning_profile);

    record_trace_event(
        server,
        trace,
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
        0,
    );

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "capability_profile": capability_profile,
            "governance_profile": governance_profile,
            "learning_profile": learning_profile,
            "token_economy": token_economy,
            "knowledge_refinement": knowledge_refinement,
            "plan": plan,
            "workflow": workflow,
            "adaptive": {
                "planning": adaptive_planning,
            },
            "plan_artifact_path": plan_artifact_path.display().to_string(),
            "workflow_artifact_path": workflow_artifact_path.display().to_string(),
            "execution_cycle": execution_cycle,
            "sandbox_profile": sandbox_profile,
            "requirement_gate": {
                "confirmed": true,
                "gate": requirement_gate_payload,
            },
            "approval_checkpoint": approval_checkpoint,
            "repo_context": repo_context,
            "gates": gates,
            "artifacts": {
                "plan": plan_artifact_path.display().to_string(),
                "workflow": workflow_artifact_path.display().to_string(),
            },
            "change_bundle": change_bundle,
            "trace_ref": trace_ref,
        }),
    )
    .await
}

pub(super) async fn handle_task_plan(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    trace: &RequestTraceContext,
) -> Result<()> {
    let Some(task) = params.get("task").and_then(Value::as_str) else {
        return send_error(
            server,
            request_id,
            -32602,
            "task is required for task.plan".to_string(),
            None,
        )
        .await;
    };
    if task.trim().is_empty() {
        return send_error(
            server,
            request_id,
            -32602,
            "task is required for task.plan".to_string(),
            None,
        )
        .await;
    }

    let ledger = clone_artifact_ledger(server);
    let requirement_gate = evaluate_requirement_gate_facade(&ledger, task, &params, "task.plan")?;
    if requirement_gate.blocked {
        return send_error(
            server,
            request_id,
            -32006,
            requirement_gate
                .reason
                .clone()
                .unwrap_or_else(|| "requirement confirmation is required".to_string()),
            Some(requirement_gate.blocked_payload()),
        )
        .await;
    }

    let (memory_graph, memory_recall) = build_task_memory_graph_and_recall(&ledger, task);
    let plan = build_task_plan(task);
    let artifact_path = persist_task_plan(&ledger, &plan)?;
    let requirement_gate_payload = requirement_gate.success_payload();
    let execution_cycle =
        build_execution_cycle("task.plan", "review_task_plan", "not_run", Vec::new());
    let gates = build_gate_matrix(
        requirement_gate_payload.clone(),
        "passed",
        "not_run",
        "not_run",
        Some(("planning", "passed")),
    );
    let change_bundle = build_change_bundle(
        "planning_only",
        format!("task.plan produced a task plan for '{}'", task),
        "low",
        "not_run",
        format!("docs(plan): capture task plan for {}", task),
        vec![artifact_path.display().to_string()],
    );
    let trace_ref = build_trace_ref(
        "task.plan",
        request_id.as_ref(),
        Some(artifact_path.display().to_string().as_str()),
    );
    let capability_profile = build_capability_profile("task.plan", task, &params);
    let governance_profile =
        build_universal_governance_profile("task.plan", &capability_profile, &params);
    let sandbox_profile = build_sandbox_profile("task.plan", &params, &capability_profile);
    let approval_checkpoint = build_approval_checkpoint("task.plan", &change_bundle, &params);
    let repo_context = build_repo_native_context("task.plan", &params, &change_bundle);
    let learning_profile = build_learning_profile("task.plan", task, &params);
    let token_economy =
        build_token_economy("task.plan", &params, &governance_profile, &execution_cycle);
    let knowledge_refinement =
        build_knowledge_refinement_profile("task.plan", task, &params, &learning_profile);
    record_trace_event(
        server,
        trace,
        "phase.plan",
        "ok",
        "plan",
        json!({
            "task": task,
            "sub_agent_recommended": plan.sub_agent_recommended,
            "planned_subtasks": plan.planned_subtasks.len(),
        }),
        None,
        0,
    );

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "capability_profile": capability_profile,
            "governance_profile": governance_profile,
            "learning_profile": learning_profile,
            "token_economy": token_economy,
            "knowledge_refinement": knowledge_refinement,
            "plan": plan,
            "artifact_path": artifact_path.display().to_string(),
            "run_mode": normalize_plan_control_mode(params.get("mode").and_then(Value::as_str)),
            "memory_graph": memory_graph,
            "memory_recall": memory_recall,
            "execution_cycle": execution_cycle,
            "sandbox_profile": sandbox_profile,
            "requirement_gate": {
                "confirmed": true,
                "gate": requirement_gate_payload,
            },
            "approval_checkpoint": approval_checkpoint,
            "repo_context": repo_context,
            "gates": gates,
            "artifacts": {
                "plan": artifact_path.display().to_string(),
            },
            "change_bundle": change_bundle,
            "trace_ref": trace_ref,
        }),
    )
    .await
}
