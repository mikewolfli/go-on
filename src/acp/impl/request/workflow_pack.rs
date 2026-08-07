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

fn requires_human_confirmation(task: &str, params: &Value) -> bool {
    if let Some(explicit) = params
        .get("requires_human_confirmation")
        .and_then(Value::as_bool)
    {
        return explicit;
    }

    let characteristics = TaskRouter::analyze_task(task);
    characteristics.has_safety_concerns || characteristics.complexity >= 4
}

/// Maximum allowed recursion depth for auto-recovery in workflow confirm.
const MAX_WORKFLOW_CONFIRM_DEPTH: usize = 5;

pub(super) async fn workflow_confirm_payload(server: &AcpServer, params: Value) -> Result<Value> {
    workflow_confirm_with_depth_payload(server, params, 0).await
}

async fn workflow_confirm_with_depth_payload(
    server: &AcpServer,
    params: Value,
    depth: usize,
) -> Result<Value> {
    if depth > MAX_WORKFLOW_CONFIRM_DEPTH {
        return Err(anyhow::anyhow!(
            "internal error: workflow.confirm auto-recovery exceeded maximum recursion depth"
        ));
    }

    let task = params_task(&params).unwrap_or_default();
    let user_confirmed = params
        .get("user_confirmed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let ready_to_confirm = params
        .get("ready_to_confirm")
        .and_then(Value::as_bool)
        .unwrap_or(user_confirmed);
    if !ready_to_confirm {
        let needs_human_confirmation = requires_human_confirmation(&task, &params);
        let auto_confirmable = !needs_human_confirmation;

        // AUTON-02: Try auto-recovery for auto-confirmable tasks
        // instead of immediately returning clarification_required.
        if auto_confirmable {
            let ledger = clone_artifact_ledger(server);
            let continuation =
                crate::acp::helpers::requirement_continuation::evaluate_with_continuation(
                    &ledger,
                    &task,
                    &params,
                    "workflow.confirm",
                );
            if crate::acp::helpers::requirement_continuation::can_proceed_with_continuation(
                &continuation,
            ) {
                let mut auto_recovered_params = continuation
                    .auto_recovery
                    .as_ref()
                    .map(|r| r.params.clone())
                    .unwrap_or_else(|| params.clone());
                // Mark as ready so the recursive call enters the confirmed path.
                if let Some(obj) = auto_recovered_params.as_object_mut() {
                    obj.insert("ready_to_confirm".to_string(), Value::Bool(true));
                }
                // (Auto-recovery metric is recorded exactly once, inside
                // `try_auto_recover_requirement_gate` — no duplicate call here.)
                // Fall through to the confirmed path with auto-recovered params
                return Box::pin(workflow_confirm_with_depth_payload(
                    server,
                    auto_recovered_params,
                    depth + 1,
                ))
                .await;
            }
        }

        return Ok(json!({
            "ok": true,
            "status": "clarification_required",
            "auto_confirmable": auto_confirmable,
            "requires_human_confirmation": needs_human_confirmation,
            "next_step": {"method": "workflow.clarify", "task": task},
        }));
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
    contract.user_confirmed = user_confirmed || ready_to_confirm;
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

    Ok(json!({
        "ok": true,
        "requirement_contract": contract,
        "requirement_contract_artifact_path": requirement_contract_artifact_path.display().to_string(),
        "clarification_session": clarification_session,
        "clarification_session_artifact_path": clarification_session_artifact_path.display().to_string(),
        "learning_profile": learning_profile,
        "knowledge_refinement": knowledge_refinement,
    }))
}

pub(super) async fn workflow_clarify_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let task = params_task(&params).unwrap_or_default();
    let needs_human_confirmation = requires_human_confirmation(&task, &params);

    // AUTON-02: For auto-confirmable tasks, try to auto-resolve the requirement
    // gate directly instead of creating a new clarification session. This reduces
    // the number of round-trips needed for low-risk tasks.
    if !needs_human_confirmation {
        let ledger = clone_artifact_ledger(server);
        let continuation =
            crate::acp::helpers::requirement_continuation::evaluate_with_continuation(
                &ledger,
                &task,
                &params,
                "workflow.clarify",
            );
        if crate::acp::helpers::requirement_continuation::can_proceed_with_continuation(
            &continuation,
        ) {
            // (Auto-recovery metric is recorded exactly once, inside
            // `try_auto_recover_requirement_gate` — no duplicate call here.)
            return Ok(json!({
                "ok": true,
                "status": "auto_confirmed",
                "auto_confirmable": true,
                "requires_human_confirmation": false,
                "requirement_gate": continuation.gate.success_payload(),
                "next_step": {"status": "confirmed"},
            }));
        }
    }

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

    Ok(json!({
        "ok": true,
        "status": "clarification_in_progress",
        "auto_confirmable": !needs_human_confirmation,
        "requires_human_confirmation": needs_human_confirmation,
        "clarification_session": clarification_session,
        "clarification_session_artifact_path": clarification_session_artifact_path.display().to_string(),
        "learning_profile": learning_profile,
        "knowledge_refinement": knowledge_refinement,
        "next_step": {"method": "workflow.confirm", "task": task, "ready_to_confirm": clarification_session.ready_to_confirm},
    }))
}

pub(super) async fn workflow_research_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let task = params_task(&params).unwrap_or_default();
    if task.trim().is_empty() {
        return Err(anyhow::anyhow!("task is required"));
    }

    let ledger = clone_artifact_ledger(server);
    let requirement_continuation =
        crate::acp::helpers::requirement_continuation::evaluate_with_continuation(
            &ledger,
            &task,
            &params,
            "workflow.research",
        );
    if !crate::acp::helpers::requirement_continuation::can_proceed_with_continuation(
        &requirement_continuation,
    ) {
        let blocked_payload = requirement_continuation.gate.blocked_payload();
        let reason = requirement_continuation
            .gate
            .reason
            .clone()
            .unwrap_or_else(|| "requirement confirmation is required".to_string());
        let kind = blocked_payload["kind"]
            .as_str()
            .unwrap_or("requirement_contract")
            .to_string();
        if matches!(
            requirement_continuation.kind,
            crate::acp::helpers::requirement_continuation::RequirementContinuationKind::ClarificationRequired
        ) {
            return Ok(json!({
                "ok": true,
                "status": "clarification_required",
                "run_status": "waiting_clarification",
                "kind": kind,
                "reason": reason,
                "next_step": requirement_continuation.next_step.clone(),
                "requirement_gate": blocked_payload,
                "requirement_continuation": requirement_continuation.next_step,
            }));
        }

        return Err(anyhow::anyhow!("{}", reason));
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
    let requirement_gate_payload =
        crate::acp::helpers::requirement_continuation::requirement_gate_payload_for_response(
            &requirement_continuation,
        );
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

    Ok(json!({
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
            "auto_clarification_in_progress": matches!(
                requirement_continuation.kind,
                crate::acp::helpers::requirement_continuation::RequirementContinuationKind::AutoConfirmed
            ),
        },
        "approval_checkpoint": approval_checkpoint,
        "repo_context": repo_context,
        "gates": gates,
        "artifacts": {
            "research": artifact_path.display().to_string(),
            "plan": plan_artifact_path.display().to_string(),
        },
        "change_bundle": change_bundle,
        "trace_ref": build_trace_ref(
            "workflow.research",
            None,
            Some(&plan_artifact_path.display().to_string()),
        ),
    }))
}

pub(super) async fn workflow_consult_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let task = params_task(&params).unwrap_or_default();
    if task.trim().is_empty() {
        return Err(anyhow::anyhow!("task is required"));
    }

    let ledger = clone_artifact_ledger(server);
    let requirement_continuation =
        crate::acp::helpers::requirement_continuation::evaluate_with_continuation(
            &ledger,
            &task,
            &params,
            "workflow.consult",
        );
    if !crate::acp::helpers::requirement_continuation::can_proceed_with_continuation(
        &requirement_continuation,
    ) {
        let blocked_payload = requirement_continuation.gate.blocked_payload();
        let reason = requirement_continuation
            .gate
            .reason
            .clone()
            .unwrap_or_else(|| "requirement confirmation is required".to_string());
        let kind = blocked_payload["kind"]
            .as_str()
            .unwrap_or("requirement_contract")
            .to_string();
        if matches!(
            requirement_continuation.kind,
            crate::acp::helpers::requirement_continuation::RequirementContinuationKind::ClarificationRequired
        ) {
            return Ok(json!({
                "ok": true,
                "status": "clarification_required",
                "run_status": "waiting_clarification",
                "kind": kind,
                "reason": reason,
                "next_step": requirement_continuation.next_step.clone(),
                "requirement_gate": blocked_payload,
                "requirement_continuation": requirement_continuation.next_step,
            }));
        }

        return Err(anyhow::anyhow!("{}", reason));
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
    let requirement_gate_payload =
        crate::acp::helpers::requirement_continuation::requirement_gate_payload_for_response(
            &requirement_continuation,
        );
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
    Ok(json!({
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
            "auto_clarification_in_progress": matches!(
                requirement_continuation.kind,
                crate::acp::helpers::requirement_continuation::RequirementContinuationKind::AutoConfirmed
            ),
        },
        "approval_checkpoint": approval_checkpoint,
        "repo_context": repo_context,
        "gates": gates,
        "artifacts": {
            "consultation": artifact_path.display().to_string(),
        },
        "change_bundle": change_bundle,
        "trace_ref": build_trace_ref(
            "workflow.consult",
            None,
            Some(&artifact_path.display().to_string()),
        ),
    }))
}

pub(crate) async fn workflow_generate_payload(
    server: &AcpServer,
    params: Value,
    trace: &RequestTraceContext,
) -> Result<Value> {
    let Some(task) = params.get("task").and_then(Value::as_str) else {
        return Err(anyhow::anyhow!("task is required for workflow.generate"));
    };
    if task.trim().is_empty() {
        return Err(anyhow::anyhow!("task is required for workflow.generate"));
    }

    let ledger = clone_artifact_ledger(server);
    // Use continuation-aware requirement gate (AUTON-02)
    let requirement_continuation =
        crate::acp::helpers::requirement_continuation::evaluate_with_continuation(
            &ledger,
            task,
            &params,
            "workflow.generate",
        );
    if !crate::acp::helpers::requirement_continuation::can_proceed_with_continuation(
        &requirement_continuation,
    ) {
        let blocked_payload = requirement_continuation.gate.blocked_payload();
        let reason = requirement_continuation
            .gate
            .reason
            .clone()
            .unwrap_or_else(|| "requirement confirmation is required".to_string());
        let kind = blocked_payload["kind"]
            .as_str()
            .unwrap_or("requirement_contract")
            .to_string();
        if matches!(
            requirement_continuation.kind,
            crate::acp::helpers::requirement_continuation::RequirementContinuationKind::ClarificationRequired
        ) {
            return Ok(json!({
                "ok": true,
                "status": "clarification_required",
                "run_status": "waiting_clarification",
                "kind": kind,
                "reason": reason,
                "next_step": requirement_continuation.next_step.clone(),
                "requirement_gate": blocked_payload,
                "requirement_continuation": requirement_continuation.next_step,
            }));
        }

        return Err(anyhow::anyhow!("{}", reason));
    }

    let auto_clarification_in_progress = matches!(
        requirement_continuation.kind,
        crate::acp::helpers::requirement_continuation::RequirementContinuationKind::AutoConfirmed
    );

    let mut plan = build_task_plan(task);
    let plan_artifact_path = persist_task_plan(&ledger, &plan)?;
    let mut workflow = build_workflow_generated_artifact(&plan);
    let planner_bridge = crate::acp::helpers::planner_bridge::build_planner_bridge(
        "workflow-generate",
        "generate",
        task,
        &params,
    )
    .await;
    let _dag_order_updated = crate::acp::helpers::planner_bridge::apply_dag_order_to_workflow(
        &mut workflow,
        &planner_bridge,
    );
    let adaptive_planning = apply_learning_plan_feedback(&ledger, &mut plan, &mut workflow);
    let workflow_artifact_path = persist_workflow_generated(&ledger, &workflow)?;
    let requirement_gate_payload =
        crate::acp::helpers::requirement_continuation::requirement_gate_payload_for_response(
            &requirement_continuation,
        );
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

    Ok(json!({
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
            "auto_clarification_in_progress": auto_clarification_in_progress,
        },
        "planner_execution_graph": crate::acp::helpers::planner_bridge::planner_execution_graph_payload(&planner_bridge),
        "approval_checkpoint": approval_checkpoint,
        "repo_context": repo_context,
        "gates": gates,
        "artifacts": {
            "plan": plan_artifact_path.display().to_string(),
            "workflow": workflow_artifact_path.display().to_string(),
        },
        "change_bundle": change_bundle,
    }))
}

pub(super) async fn task_plan_payload(
    server: &AcpServer,
    params: Value,
    trace: &RequestTraceContext,
) -> Result<Value> {
    let Some(task) = params.get("task").and_then(Value::as_str) else {
        return Err(anyhow::anyhow!("task is required for task.plan"));
    };
    if task.trim().is_empty() {
        return Err(anyhow::anyhow!("task is required for task.plan"));
    }

    let ledger = clone_artifact_ledger(server);
    let requirement_continuation =
        crate::acp::helpers::requirement_continuation::evaluate_with_continuation(
            &ledger,
            task,
            &params,
            "task.plan",
        );
    if !crate::acp::helpers::requirement_continuation::can_proceed_with_continuation(
        &requirement_continuation,
    ) {
        let blocked_payload = requirement_continuation.gate.blocked_payload();
        let reason = requirement_continuation
            .gate
            .reason
            .clone()
            .unwrap_or_else(|| "requirement confirmation is required".to_string());
        let kind = blocked_payload["kind"]
            .as_str()
            .unwrap_or("requirement_contract")
            .to_string();
        if matches!(
            requirement_continuation.kind,
            crate::acp::helpers::requirement_continuation::RequirementContinuationKind::ClarificationRequired
        ) {
            return Ok(json!({
                "ok": true,
                "status": "clarification_required",
                "run_status": "waiting_clarification",
                "kind": kind,
                "reason": reason,
                "next_step": requirement_continuation.next_step.clone(),
                "requirement_gate": blocked_payload,
                "requirement_continuation": requirement_continuation.next_step,
            }));
        }

        return Err(anyhow::anyhow!("{}", reason));
    }

    let (memory_graph, memory_recall) = build_task_memory_graph_and_recall(&ledger, task);
    let plan = build_task_plan(task);
    let artifact_path = persist_task_plan(&ledger, &plan)?;
    let requirement_gate_payload =
        crate::acp::helpers::requirement_continuation::requirement_gate_payload_for_response(
            &requirement_continuation,
        );
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

    Ok(json!({
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
            "auto_clarification_in_progress": matches!(
                requirement_continuation.kind,
                crate::acp::helpers::requirement_continuation::RequirementContinuationKind::AutoConfirmed
            ),
        },
        "approval_checkpoint": approval_checkpoint,
        "repo_context": repo_context,
        "gates": gates,
        "artifacts": {
            "plan": artifact_path.display().to_string(),
        },
        "change_bundle": change_bundle,
    }))
}

pub(super) async fn handle_workflow_generate_from_chat(
    server: &AcpServer,
    params: Value,
    _trace: &RequestTraceContext,
) -> Result<DispatchOutput> {
    let messages = parse_messages(&params).unwrap_or_default();

    if messages.is_empty() {
        anyhow::bail!("messages are required for workflow.generate_from_chat");
    }

    // 1. Analyze conversation for repeated task patterns
    let user_messages: Vec<&str> = messages
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .collect();

    // 2. Group similar user requests by keyword analysis
    let mut pattern_groups: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    // Keyword clusters for known task types
    let clusters = [
        (
            "code_review",
            &["review", "audit", "inspect", "check code", "quality"] as &[&str],
        ),
        (
            "testing",
            &["test", "unit test", "integration test", "coverage"],
        ),
        (
            "refactoring",
            &["refactor", "restructure", "clean up", "improve"],
        ),
        ("documentation", &["document", "docs", "readme", "explain"]),
        ("debugging", &["debug", "fix", "error", "crash", "bug"]),
        ("deployment", &["deploy", "release", "ci", "cd", "pipeline"]),
    ];

    for msg in &user_messages {
        let lower = msg.to_ascii_lowercase();
        for (cluster_name, keywords) in &clusters {
            if keywords.iter().any(|kw| lower.contains(kw)) {
                pattern_groups
                    .entry(cluster_name.to_string())
                    .or_default()
                    .push(msg.to_string());
            }
        }
    }

    // 3. For patterns appearing 3+ times, generate a workflow
    let mut workflows: Vec<Value> = Vec::new();
    for (pattern, examples) in &pattern_groups {
        if examples.len() < 3 {
            continue;
        }

        let task_summary = format!(
            "Automated {} workflow based on {} similar requests",
            pattern.replace('_', " "),
            examples.len()
        );

        // Create a workflow definition
        let workflow_definition = json!({
            "name": format!("auto_{}", pattern),
            "description": task_summary,
            "phases": ["coding"],
            "nodes": [
                {
                    "id": format!("analyze_{}", pattern),
                    "type": "task",
                    "description": format!("Analyze and execute {} request", pattern),
                }
            ],
            "edges": [],
            "execution_order": [format!("analyze_{}", pattern)],
            "generated_from": "conversation_analysis",
            "sample_count": examples.len(),
            "sample_queries": examples.iter().take(3).cloned().collect::<Vec<_>>(),
        });

        workflows.push(workflow_definition);
    }

    // 4. Try to create skills for each detected pattern
    let mut created_skills: Vec<String> = Vec::new();
    for wf in &workflows {
        let skill_name = wf["name"].as_str().unwrap_or("auto_workflow");
        let description = wf["description"].as_str().unwrap_or("");

        let exists = server
            .orchestration_deps
            .skill_registry
            .read()
            .ok()
            .map(|registry| registry.get(skill_name).is_some())
            .unwrap_or(false);

        if !exists {
            let result = server.orchestration_deps.skill_registry.write()
                .ok()
                .and_then(|mut registry| {
                    registry.create_skill_from_prompt(
                        skill_name,
                        description,
                        &format!("You are an AI assistant specialized in: {}\n\nAnalyze the request and execute the appropriate steps based on the following pattern:\n{}", description, description),
                        std::collections::HashMap::new(),
                    ).ok()
                });
            if result.is_some() {
                created_skills.push(skill_name.to_string());
            }
        }
    }

    Ok(DispatchOutput::ok(json!({
        "ok": true,
        "workflows": workflows,
        "created_skills": created_skills,
        "patterns_analyzed": pattern_groups.len(),
        "summary": format!(
            "Analyzed {} messages, found {} patterns, created {} workflow(s) and {} skill(s)",
            user_messages.len(),
            pattern_groups.len(),
            workflows.len(),
            created_skills.len(),
        ),
    })))
}

pub(crate) async fn handle_workflow_ask(
    server: &AcpServer,
    params: Value,
    trace: &RequestTraceContext,
) -> Result<DispatchOutput> {
    let Some(task) = params.get("task").and_then(Value::as_str) else {
        anyhow::bail!("task is required for workflow.ask");
    };
    if task.trim().is_empty() {
        anyhow::bail!("task is required for workflow.ask");
    }

    let ledger = clone_artifact_ledger(server);
    let requirement_continuation =
        crate::acp::helpers::requirement_continuation::evaluate_with_continuation(
            &ledger,
            task,
            &params,
            "workflow.ask",
        );
    if !crate::acp::helpers::requirement_continuation::can_proceed_with_continuation(
        &requirement_continuation,
    ) {
        let blocked_payload = requirement_continuation.gate.blocked_payload();
        let reason = requirement_continuation
            .gate
            .reason
            .clone()
            .unwrap_or_else(|| "requirement confirmation is required".to_string());
        let kind = blocked_payload["kind"]
            .as_str()
            .unwrap_or("requirement_contract")
            .to_string();
        if matches!(
            requirement_continuation.kind,
            crate::acp::helpers::requirement_continuation::RequirementContinuationKind::ClarificationRequired
        ) {
            return Ok(DispatchOutput::ok(json!({
                "ok": true,
                "status": "clarification_required",
                "run_status": "waiting_clarification",
                "kind": kind,
                "reason": reason,
                "next_step": requirement_continuation.next_step.clone(),
                "requirement_gate": blocked_payload,
                "requirement_continuation": requirement_continuation.next_step,
            })));
        }

        let err_msg = format!("requirement check failed: {}", reason);
        anyhow::bail!(err_msg);
    }

    let auto_create_skills = params
        .get("auto_create_skills")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let auto_create_workflow = params
        .get("auto_create_workflow")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    // Step 1: Generate workflow plan from task
    let plan = build_task_plan(task);
    let plan_artifact_path = persist_task_plan(&ledger, &plan)?;
    let mut workflow = build_workflow_generated_artifact(&plan);
    let planner_bridge = crate::acp::helpers::planner_bridge::build_planner_bridge(
        "workflow-ask",
        "ask",
        task,
        &params,
    )
    .await;
    let _dag_order_updated = crate::acp::helpers::planner_bridge::apply_dag_order_to_workflow(
        &mut workflow,
        &planner_bridge,
    );
    let workflow_artifact_path = persist_workflow_generated(&ledger, &workflow)?;

    // Step 2: Auto-create skills if needed
    let mut created_skills: Vec<String> = Vec::new();
    if auto_create_skills {
        for (i, node) in workflow.nodes.iter().enumerate() {
            let skill_name = format!("workflow_node_{}", i);
            // Check if skill already exists (lock once, drop before next iteration)
            let exists = {
                let registry = server.orchestration_deps.skill_registry.read();
                registry
                    .ok()
                    .map(|r| r.get(&skill_name).is_some())
                    .unwrap_or(false)
            };
            if !exists {
                // Create a prompt-based skill from the node description
                let result = {
                    let registry = server.orchestration_deps.skill_registry.write();
                    registry.ok().and_then(|mut reg| {
                        reg
                            .create_skill_from_prompt(
                                &skill_name,
                                &node.description,
                                &format!(
                                    "You are an AI assistant specialized in: {}\n\nTask: {}\n\nExecute the following instructions precisely:\n{}",
                                    node.description,
                                    task,
                                    node.description,
                                ),
                                std::collections::HashMap::new(),
                            )
                            .ok()
                    })
                };
                if result.is_some() {
                    created_skills.push(skill_name);
                }
            }
        }
    }

    // Step 2b: Auto-register workflow if enabled
    // Register the generated DAG as a named preset in the global WorkflowRegistry
    // (owned by CapabilityBus), so it becomes observable via capability_bus_profile().
    if auto_create_workflow {
        let preset_name = format!("auto-{}", task.trim().to_lowercase().replace(' ', "-"));
        let preset = crate::orchestration::workflow_registry::WorkflowPreset {
            name: preset_name,
            workflow_type: crate::config::WorkflowType::Custom,
            phases: workflow.execution_order.iter().flatten().cloned().collect(),
            description: format!("Auto-generated workflow for task: {}", task.trim()),
        };
        if let Some(cb) = server.governance_deps.capability_bus.as_ref() {
            if let Some(wr) = cb.workflow_registry.as_ref() {
                let mut registry = wr.lock().unwrap_or_else(|poisoned| {
                    tracing::warn!("workflow registry lock poisoned – recovering");
                    poisoned.into_inner()
                });
                if let Err(err) = registry.register(preset) {
                    // Duplicate preset from a prior run is expected — skip silently.
                    debug!("workflow auto-register skipped: {}", err);
                }
            }
        }
    }

    // Step 3: Execute workflow
    let execute_params = json!({
        "task": task,
        "phase": params.get("phase").cloned().unwrap_or(json!("coding")),
    });
    let execute_result = handle_workflow_execute(server, execute_params, trace).await;

    // Step 4: Return comprehensive result
    let response = json!({
        "ok": true,
        "action": "workflow.ask",
        "task": task,
        "plan": plan,
        "workflow": workflow,
        "created_skills": created_skills,
        "auto_create_skills": auto_create_skills,
        "auto_create_workflow": auto_create_workflow,
        "execution_result": if execute_result.is_ok() { "completed" } else { "failed" },
        "requirement_gate": {
            "confirmed": true,
            "gate": crate::acp::helpers::requirement_continuation::requirement_gate_payload_for_response(&requirement_continuation),
            "auto_clarification_in_progress": matches!(
                requirement_continuation.kind,
                crate::acp::helpers::requirement_continuation::RequirementContinuationKind::AutoConfirmed
            ),
        },
        "plan_artifact_path": plan_artifact_path.display().to_string(),
        "workflow_artifact_path": workflow_artifact_path.display().to_string(),
        "workflow_graph": {
            "nodes": workflow.nodes.iter().map(|n| json!({
                "id": n.id,
                "description": n.description,
                "role": n.role,
                "phase_index": n.phase_index,
                "priority": n.priority,
                "timeout_seconds": n.timeout_seconds,
                "retry_limit": n.retry_limit,
            })).collect::<Vec<_>>(),
            "edges": workflow.edges.iter().map(|e| json!({
                "from": e.from,
                "to": e.to,
            })).collect::<Vec<_>>(),
            "execution_order": workflow.execution_order,
        },
        "planner_execution_graph": crate::acp::helpers::planner_bridge::planner_execution_graph_payload(&planner_bridge),
    });

    Ok(DispatchOutput::ok(response))
}
