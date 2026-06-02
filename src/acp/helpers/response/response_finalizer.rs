//! Final Response Post-Processing Pipeline for chat requests.
//!
//! Extracted from `process_chat_request` in `chat.rs` to isolate the
//! multi-step post-processing logic that runs after the primary agent
//! response is built.  This pipeline handles provenance, scheduling,
//! capability feedback, budget tracking, promotion/optimizer evaluation,
//! planner integration, orchestration alignment, fork registry cleanup,
//! evaluation scoring, and final result augmentation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use tracing::{info, warn};

use crate::acp::helpers::agent_router::record_task_agent_outcome;
use crate::acp::helpers::autonomy_metrics::{
    record_orchestration_alignment, record_orchestration_node_mapping,
};
use crate::acp::helpers::orchestration_alignment::derive_plan_trace_alignment;
use crate::acp::helpers::vote_orchestration::derive_response_orchestration;
use crate::acp::server::AcpServer;
use crate::orchestration::planner_executor::Planner;
use crate::orchestration::workflow_optimizer::OptimizationContext;
use crate::rpc_protocol::RequestTraceContext;

/// Execution metrics extracted from the agent result for downstream use.
struct AgentExecutionMetrics {
    elapsed_ms: u64,
    used_tokens: u64,
    #[allow(dead_code)] // F-GAP-49 — reserved for future use
    request_succeeded: bool,
}

/// Metadata computed during finalization, ready for injection into the response.
struct ResponseMetadata {
    promotion_decisions: Vec<String>,
    optimizer_recommendations: Vec<Value>,
    execution_plan: Value,
    orchestration_alignment: Value,
    orchestration_node_decisions: Value,
    fork_id: Option<String>,
    evaluation_results: Vec<Value>,
}

/// Run the full final response post-processing pipeline.
///
/// This function encapsulates all post-agent-execution steps:
/// provenance logging, scheduler bookkeeping, capability bus feedback,
/// budget accounting, promotion/optimizer evaluation, plan generation,
/// alignment analysis, fork tracking, evaluation suite scoring, and
/// result augmentation.
///
/// Returns the augmented response `Value` with all new fields injected.
#[allow(clippy::too_many_arguments)]
pub fn finalize_chat_response(
    server: &AcpServer,
    trace: &RequestTraceContext,
    mode: &str,
    phase_name: &str,
    selected_agent: &str,
    selected_model_name: &Option<String>,
    response_text: &str,
    reasoning_text: &str,
    tenant_id: &str,
    started: Instant,
    mut result: Value,
    conversation_id: &str,
    schema_warnings: Vec<String>,
    schema_error: Option<String>,
    layered_prompt_segments_len: usize,
    tool_execution_results: &[Value],
    sched_task_id: &str,
    // Routing / diagnostic context
    candidate_agents: &[String],
    _routing_provenance: &[String],
    _reputation_scores: &HashMap<String, f64>,
    _selected_agent_reputation: Option<f64>,
    _council_decision: &Option<Value>,
    _vote_winner: &Option<String>,
    _fallback_reason: &Option<String>,
    _cache_hit: bool,
    _cache_bypassed: bool,
    // Planner input
    first_message_content: &str,
) -> Value {
    // ── Step 1: Collect agent outputs & record side-effects ────────────
    let metrics = collect_agent_outputs(
        server,
        trace,
        mode,
        phase_name,
        selected_agent,
        selected_model_name,
        response_text,
        tenant_id,
        started,
        &result,
        conversation_id,
        sched_task_id,
        candidate_agents,
    );

    // ── Step 2: Build response metadata ───────────────────────────────
    let metadata = build_response_metadata(
        server,
        &result,
        mode,
        phase_name,
        selected_agent,
        conversation_id,
        first_message_content,
        tool_execution_results,
        response_text,
        &metrics,
    );

    // ── Step 3: Format the final response body ────────────────────────
    format_response_body(
        &mut result,
        reasoning_text,
        schema_warnings,
        schema_error,
        layered_prompt_segments_len,
        tenant_id,
        &metadata,
    );

    result
}

/// Gather all agent execution outputs and record side-effects.
///
/// Performs provenance logging, scheduler completion, capability bus
/// feedback, agent outcome recording, and tenant budget tracking.
/// Returns execution metrics for downstream metadata construction.
#[allow(clippy::too_many_arguments)]
fn collect_agent_outputs(
    server: &AcpServer,
    trace: &RequestTraceContext,
    mode: &str,
    phase_name: &str,
    selected_agent: &str,
    selected_model_name: &Option<String>,
    response_text: &str,
    tenant_id: &str,
    started: Instant,
    result: &Value,
    conversation_id: &str,
    sched_task_id: &str,
    candidate_agents: &[String],
) -> AgentExecutionMetrics {
    // ── Request-level routing provenance (BLUE41 Step 7) ──────────────
    if let Some(ref ledger) = server.governance_deps.provenance_ledger {
        let route_input = json!({
            "request_id": trace.request_id.clone(),
            "mode": mode,
            "phase": phase_name,
            "candidate_agents": candidate_agents,
            "capability_routing": result.get("capability_routing").cloned().unwrap_or_default(),
            "routing_diagnostics": result.get("routing_diagnostics").cloned().unwrap_or_default(),
        });
        let route_output = json!({
            "selected_agent": selected_agent,
            "selected_model": selected_model_name,
            "duration_ms": started.elapsed().as_millis() as u64,
            "success": !response_text.trim().is_empty(),
        });
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        ledger.append(crate::observability::provenance::ProvenanceEntry {
            id: format!("routing:{}", trace.request_id),
            task_id: trace.request_id.clone(),
            phase: phase_name.to_string(),
            agent: selected_agent.to_string(),
            tool: "routing.deliberation".to_string(),
            input_digest: crate::observability::provenance::ProvenanceLedger::digest(&route_input),
            output_digest: crate::observability::provenance::ProvenanceLedger::digest(
                &route_output,
            ),
            upstream_ids: Vec::new(),
            timestamp_ms,
            metadata: json!({
                "route_input": route_input,
                "route_output": route_output,
            }),
        });
    }

    // ── Scheduler task completion (ARCH-02) ────────────────────────────
    if let Some(ref sched) = server.orchestration_deps.scheduler {
        if let Err(e) = sched.level1.complete(sched_task_id) {
            tracing::warn!("scheduler complete failed: {}", e);
        }
    }

    let elapsed = started.elapsed().as_millis() as u64;
    let used_tokens = result
        .get("token_economy")
        .and_then(|v| v.get("total_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let request_succeeded = !response_text.trim().is_empty();

    // ── CapabilityBus feedback on execution outcome ────────────────────
    if let Some(ref cb) = server.governance_deps.capability_bus {
        cb.feedback(
            selected_agent,
            phase_name,
            conversation_id,
            request_succeeded,
            elapsed,
            used_tokens,
            1.0,
        );
        // Also update the reinforcement learning loop with the outcome (spawn to avoid blocking)
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let cb = Arc::clone(cb);
            let phase = phase_name.to_string();
            let agent = selected_agent.to_string();
            handle.spawn(async move {
                let _ = cb.evolve(
                    &(phase.clone(), agent.clone()),
                    "execute",
                    &(phase, agent),
                    used_tokens,
                    request_succeeded,
                    1.0,
                ).await;
            });
        }
    }

    record_task_agent_outcome(phase_name, selected_agent, request_succeeded);

    // ── TenantBudgetEnforcer record usage (F-GAP-08) ───────────────────
    {
        let mut budget = server.tenant_budget.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("tenant_budget lock poisoned in collect_agent_outputs");
            poisoned.into_inner()
        });
        budget.record_usage(tenant_id, used_tokens as usize, 1);
    }

    AgentExecutionMetrics {
        elapsed_ms: elapsed,
        used_tokens,
        request_succeeded,
    }
}

/// Construct response metadata from agent execution results.
///
/// Computes promotion evaluations, optimizer recommendations,
/// execution plans, orchestration alignment, fork registry entries,
/// and evaluation suite scores.
#[allow(clippy::too_many_arguments)]
fn build_response_metadata(
    server: &AcpServer,
    _result: &Value,
    mode: &str,
    phase_name: &str,
    selected_agent: &str,
    conversation_id: &str,
    first_message_content: &str,
    tool_execution_results: &[Value],
    response_text: &str,
    metrics: &AgentExecutionMetrics,
) -> ResponseMetadata {
    let elapsed = metrics.elapsed_ms;
    let used_tokens = metrics.used_tokens;

    // ── PromotionPlugin evaluation (ARCH-10) ──────────────────────────
    let promotion_decisions: Vec<String> = {
        let success_rate = if response_text.trim().is_empty() {
            0.0
        } else {
            1.0
        };
        let latency_ms = elapsed as f64;
        let cost_score = (used_tokens as f64 / 100_000.0).min(1.0);
        let reg = server.promotion_registry.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("promotion_registry lock poisoned in build_response_metadata");
            poisoned.into_inner()
        });
        reg.evaluate_all(selected_agent, success_rate, latency_ms, cost_score)
            .into_iter()
            .map(|d| format!("{:?}", d))
            .collect()
    };
    info!(
        agent = %selected_agent,
        decisions = ?promotion_decisions,
        "promotion plugin evaluation"
    );

    // ── OptimizerRegistry recommendations (ARCH-11) ────────────────────
    let optimizer_recommendations: Vec<Value> = {
        let reg = server.optimizer_registry.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("optimizer_registry lock poisoned in build_response_metadata");
            poisoned.into_inner()
        });
        let _historical_success_rate = server
            .governance_deps
            .capability_bus
            .as_ref()
            .and_then(|cb| {
                cb.learning_bus
                    .read()
                    .ok()
                    .and_then(|lb| lb.agent_success_rate(selected_agent))
            })
            .unwrap_or(1.0);
        reg.optimize_all(&OptimizationContext {
            workflow_type: phase_name.to_string(),
            phases: vec![phase_name.to_string()],
            history: vec![],
            token_usage: 0,
            latency_ms: elapsed,
        })
        .into_iter()
        .map(|r| {
            json!({
                "strategy": r.suggestion_type,
                "expected_improvement": r.estimated_improvement,
                "description": r.description,
            })
        })
        .collect()
    };

    // ── Planner/Executor integration (F-GAP-05) ────────────────────────
    let execution_plan = {
        let envelope = crate::agent::AgentTaskEnvelope {
            task_id: conversation_id.to_string(),
            phase: phase_name.to_string(),
            role: selected_agent.to_string(),
            objective: first_message_content.to_string(),
            constraints: Some("600".to_string()),
            evidence: None,
            input: json!({
                "mode": mode,
                "message_count": 1,
            }),
        };
        let plan = Planner::plan(&envelope);
        json!({
            "plan_id": plan.plan_id,
            "steps": plan.steps.iter().map(|s| json!({
                "step_id": s.step_id,
                "description": s.description,
                "depends_on": s.depends_on,
            })).collect::<Vec<_>>(),
        })
    };

    let orchestration_alignment =
        derive_plan_trace_alignment(&execution_plan, tool_execution_results);
    let alignment_coverage = orchestration_alignment
        .get("coverage_ratio")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    record_orchestration_alignment(alignment_coverage);

    let orchestration_node_decisions =
        derive_response_orchestration(&execution_plan, tool_execution_results);
    let mapped_nodes = orchestration_node_decisions
        .get("mapped_nodes")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let unmapped_nodes = orchestration_node_decisions
        .get("unmapped_nodes")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    record_orchestration_node_mapping(mapped_nodes, unmapped_nodes);

    // ── ForkRegistry cleanup (ARCH-05) ─────────────────────────────────
    let fork_id = {
        let fr = server.fork_registry.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("fork_registry lock poisoned in build_response_metadata");
            poisoned.into_inner()
        });
        match fr.register(conversation_id) {
            Ok(Some(fid)) => {
                if let Err(e) = fr.complete(&fid) {
                    tracing::warn!(%conversation_id, fork_id = %fid, error = %e, "response_finalizer: failed to complete fork entry");
                }
                Some(fid)
            }
            Ok(None) => None,
            Err(e) => {
                warn!("ForkRegistry error: {e}");
                None
            }
        }
    };

    // ── Evaluation Suite scoring (F-GAP-06) ────────────────────────────
    let evaluation_results: Vec<Value> = {
        let suite = server.evaluation_suite.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("evaluation_suite lock poisoned in build_response_metadata");
            poisoned.into_inner()
        });
        let mut agent_outputs = HashMap::new();
        for case in suite.all() {
            agent_outputs.insert(case.id.clone(), response_text.to_string());
        }
        crate::intelligence::evaluation::ReplayEngine::run_suite(&suite, &agent_outputs)
            .into_iter()
            .map(|run| {
                json!({
                    "case_id": run.case_id,
                    "passed": run.passed,
                    "overall_score": run.score.overall(),
                    "details": run.details,
                })
            })
            .collect()
    };

    ResponseMetadata {
        promotion_decisions,
        optimizer_recommendations,
        execution_plan,
        orchestration_alignment,
        orchestration_node_decisions,
        fork_id,
        evaluation_results,
    }
}

/// Inject all computed metadata into the final response object.
fn format_response_body(
    result: &mut Value,
    reasoning_text: &str,
    schema_warnings: Vec<String>,
    schema_error: Option<String>,
    layered_prompt_segments_len: usize,
    tenant_id: &str,
    metadata: &ResponseMetadata,
) {
    if let Some(obj) = result.as_object_mut() {
        obj.insert("schema_warnings".to_string(), json!(schema_warnings));
        obj.insert("schema_error".to_string(), json!(schema_error));
        obj.insert(
            "layered_prompt_segments".to_string(),
            json!(layered_prompt_segments_len),
        );
        obj.insert(
            "promotion_decisions".to_string(),
            json!(metadata.promotion_decisions),
        );
        obj.insert(
            "optimizer_recommendations".to_string(),
            json!(metadata.optimizer_recommendations),
        );
        obj.insert(
            "execution_plan".to_string(),
            metadata.execution_plan.clone(),
        );
        obj.insert(
            "orchestration_alignment".to_string(),
            metadata.orchestration_alignment.clone(),
        );
        obj.insert(
            "orchestration_node_decisions".to_string(),
            metadata.orchestration_node_decisions.clone(),
        );
        obj.insert("fork_id".to_string(), json!(metadata.fork_id));
        obj.insert(
            "evaluation_results".to_string(),
            json!(metadata.evaluation_results),
        );
        obj.insert("tenant_id".to_string(), json!(tenant_id));
        if !reasoning_text.is_empty() {
            obj.insert("thinking".to_string(), json!(reasoning_text));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::server::ServerBuilder;
    use std::time::Instant;

    fn make_trace() -> crate::rpc_protocol::RequestTraceContext {
        crate::rpc_protocol::RequestTraceContext {
            trace_id: "test-trace".to_string(),
            span_id: "test-span".to_string(),
            method: "chat".to_string(),
            request_id: "test-req-1".to_string(),
        }
    }

    #[test]
    fn test_finalize_chat_response_returns_augmented_result() {
        let server = ServerBuilder::new().build().expect("server should build");
        let trace = make_trace();
        let started = Instant::now();
        let result_input = json!({
            "response": "This is the agent response",
            "token_economy": {"total_tokens": 150}
        });

        let augmented = finalize_chat_response(
            &server,
            &trace,
            "ask",
            "execute",
            "coder-agent",
            &Some("gpt-4".to_string()),
            "This is the agent response",
            "",
            "test-tenant",
            started,
            result_input,
            "conv-1",
            vec![],
            None,
            1,
            &[],
            "sched-task-1",
            &["coder-agent".to_string()],
            &[],
            &HashMap::new(),
            None,
            &None,
            &None,
            &None,
            false,
            false,
            "Fix the bug",
        );

        // Verify the result includes the augmented fields
        assert!(
            augmented.get("response").is_some(),
            "should retain original fields"
        );
        assert!(
            augmented.get("execution_plan").is_some(),
            "should have execution_plan"
        );
        assert!(
            augmented.get("orchestration_alignment").is_some(),
            "should have orchestration_alignment"
        );
        assert!(
            augmented.get("schema_warnings").is_some(),
            "should have schema_warnings"
        );
        assert!(
            augmented.get("tenant_id").is_some(),
            "should have tenant_id"
        );
    }

    #[test]
    fn test_finalize_chat_response_handles_empty_tools() {
        let server = ServerBuilder::new().build().expect("server should build");
        let trace = make_trace();
        let started = Instant::now();
        let result_input = json!({"response": "empty tools test"});

        let augmented = finalize_chat_response(
            &server,
            &trace,
            "edit",
            "execute",
            "tester-agent",
            &None,
            "empty response",
            "some reasoning",
            "tenant-2",
            started,
            result_input,
            "conv-2",
            vec!["warning: something".to_string()],
            Some("error: something".to_string()),
            2,
            &[],
            "sched-task-2",
            &[],
            &[],
            &HashMap::new(),
            None,
            &None,
            &None,
            &None,
            false,
            false,
            "Test objective",
        );

        assert!(augmented.get("execution_plan").is_some());
        assert!(augmented.get("schema_error").is_some());
        assert!(augmented.get("schema_warnings").is_some());

        // With reasoning_text non-empty, thinking field should be present
        assert!(
            augmented.get("thinking").is_some(),
            "should have thinking field when reasoning_text is non-empty"
        );
    }

    #[test]
    fn test_finalize_chat_response_with_tool_results() {
        let server = ServerBuilder::new().build().expect("server should build");
        let trace = make_trace();
        let started = Instant::now();
        let result_input = json!({
            "response": "tool-based response",
            "token_economy": {"total_tokens": 300}
        });

        let tool_results = vec![json!({
            "trace": {
                "iterations": [
                    {"stage": "act", "tool": "search_files"},
                    {"stage": "act", "tool": "read_file"}
                ]
            }
        })];

        let augmented = finalize_chat_response(
            &server,
            &trace,
            "agent",
            "planning",
            "researcher-agent",
            &Some("claude-3".to_string()),
            "Researched the codebase and found the issue",
            "",
            "tenant-3",
            started,
            result_input,
            "conv-3",
            vec![],
            None,
            1,
            &tool_results,
            "sched-task-3",
            &["researcher-agent".to_string()],
            &[],
            &HashMap::new(),
            None,
            &None,
            &None,
            &None,
            false,
            false,
            "Research the codebase for the authentication bug",
        );

        // Verify orchestration fields are computed with tool results
        let alignment = augmented
            .get("orchestration_alignment")
            .expect("should have orchestration_alignment");
        assert!(alignment.get("coverage_ratio").is_some());
        assert!(alignment.get("executed_tools").is_some());
    }
}
