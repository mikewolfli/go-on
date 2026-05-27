//! Final Response Post-Processing Pipeline for chat requests.
//!
//! Extracted from `process_chat_request` in `chat.rs` to isolate the
//! multi-step post-processing logic that runs after the primary agent
//! response is built.  This pipeline handles provenance, scheduling,
//! capability feedback, budget tracking, promotion/optimizer evaluation,
//! planner integration, orchestration alignment, fork registry cleanup,
//! evaluation scoring, and final result augmentation.

use std::collections::HashMap;
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
    // ── Request-level routing provenance (BLUE41 Step 7) ──────────────
    // Persist route decisions and diagnostics so future learning and audits
    // can explain why this request selected a specific execution path.
    if let Some(ref ledger) = server.provenance_ledger {
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
    // Mark the scheduled task as completed so the active-worker counter
    // decrements and queue depth reflects the true in-flight load.
    if let Some(ref sched) = server.scheduler {
        if let Err(e) = sched.level1.complete(sched_task_id) {
            tracing::warn!("scheduler complete failed: {}", e);
        }
    }

    // ── CapabilityBus feedback on execution outcome ────────────────────
    // Record the execution result back into the sub-buses (learning,
    // reputation, etc.) so that subsequent routes benefit from this
    // experience.
    if let Some(ref cb) = server.capability_bus {
        let elapsed = started.elapsed().as_millis() as u64;
        let used_tokens = result
            .get("token_economy")
            .and_then(|v| v.get("total_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let request_succeeded = !response_text.trim().is_empty();
        cb.feedback(
            selected_agent,
            phase_name,
            conversation_id,
            request_succeeded,
            elapsed,
            used_tokens,
            1.0,
        );
        // Also update the reinforcement learning loop with the outcome
        cb.evolve(
            &(phase_name.to_string(), selected_agent.to_string()),
            "execute",
            &(phase_name.to_string(), selected_agent.to_string()),
            used_tokens,
            request_succeeded,
            1.0,
        );
    }

    record_task_agent_outcome(phase_name, selected_agent, !response_text.trim().is_empty());

    // ── TenantBudgetEnforcer record usage (F-GAP-08) ───────────────────
    // Record resource consumption after task completion so subsequent
    // pre-route checks can enforce per-tenant quotas.
    if let Ok(mut budget) = server.tenant_budget.lock() {
        let used_tokens = result
            .get("token_economy")
            .and_then(|v| v.get("total_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        budget.record_usage(tenant_id, used_tokens, 1);
    }

    // ── PromotionPlugin evaluation (ARCH-10) ──────────────────────────
    // Evaluate promotion/demotion decisions for the selected agent based
    // on execution outcome.  Results are logged and could feed back into
    // routing weights in a future iteration.
    let promotion_decisions: Vec<String> = {
        let elapsed = started.elapsed().as_millis() as u64;
        let used_tokens = result
            .get("token_economy")
            .and_then(|v| v.get("total_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as f64;
        let success_rate = if response_text.trim().is_empty() {
            0.0
        } else {
            1.0
        };
        let latency_ms = elapsed as f64;
        let cost_score = (used_tokens / 100_000.0).min(1.0);
        if let Ok(reg) = server.promotion_registry.lock() {
            reg.evaluate_all(selected_agent, success_rate, latency_ms, cost_score)
                .into_iter()
                .map(|d| format!("{:?}", d))
                .collect()
        } else {
            vec![]
        }
    };
    info!(
        agent = %selected_agent,
        decisions = ?promotion_decisions,
        "promotion plugin evaluation"
    );

    // ── OptimizerRegistry recommendations (ARCH-11) ────────────────────
    // Collect optimization recommendations based on execution metrics.
    // These can be applied in future routing decisions.
    let optimizer_recommendations: Vec<serde_json::Value> = {
        let elapsed = started.elapsed().as_millis() as u64;
        if let Ok(reg) = server.optimizer_registry.lock() {
            // Use a rolling success rate from the capability bus if available,
            // otherwise default to 1.0 for the current request.
            let _historical_success_rate = server
                .capability_bus
                .as_ref()
                .and_then(|cb| {
                    cb.learning_bus
                        .lock()
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
                serde_json::json!({
                    "strategy": r.suggestion_type,
                    "expected_improvement": r.estimated_improvement,
                    "description": r.description,
                })
            })
            .collect()
        } else {
            vec![]
        }
    };

    // ── Planner/Executor integration (F-GAP-05) ────────────────────────
    // Build a lightweight execution plan for observability/debugging.
    // The plan shows the 3-phase decomposition (plan → execute → review)
    // that was implicitly followed by the mode runtime.
    let execution_plan = {
        let envelope = crate::agent::AgentTaskEnvelope {
            task_id: conversation_id.to_string(),
            phase: phase_name.to_string(),
            role: selected_agent.to_string(),
            objective: first_message_content.to_string(),
            constraints: Some("600".to_string()),
            evidence: None,
            input: serde_json::json!({
                "mode": mode,
                "message_count": 1,
            }),
        };
        let plan = Planner::plan(&envelope);
        serde_json::json!({
            "plan_id": plan.plan_id,
            "steps": plan.steps.iter().map(|s| serde_json::json!({
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
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    record_orchestration_alignment(alignment_coverage);

    let orchestration_node_decisions =
        derive_response_orchestration(&execution_plan, tool_execution_results);
    let mapped_nodes = orchestration_node_decisions
        .get("mapped_nodes")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let unmapped_nodes = orchestration_node_decisions
        .get("unmapped_nodes")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    record_orchestration_node_mapping(mapped_nodes, unmapped_nodes);

    // ── ForkRegistry cleanup (ARCH-05) ─────────────────────────────────
    // Register a fork entry for this execution to track sub-agent
    // isolation boundaries.  Completed forks are cleaned up immediately
    // so the registry stays within its capacity.
    let fork_id = {
        if let Ok(fr) = server.fork_registry.lock() {
            match fr.register(conversation_id) {
                Ok(Some(fid)) => {
                    let _ = fr.complete(&fid);
                    Some(fid)
                }
                Ok(None) => None,
                Err(e) => {
                    warn!("ForkRegistry lock poisoned: {e}");
                    None
                }
            }
        } else {
            None
        }
    };

    // ── Evaluation Suite scoring (F-GAP-06) ────────────────────────────
    // Run benchmark evaluations against the response text for quality
    // measurement.  Results are embedded in the response for observability.
    let evaluation_results: Vec<serde_json::Value> = {
        if let Ok(suite) = server.evaluation_suite.lock() {
            let mut agent_outputs = std::collections::HashMap::new();
            for case in suite.all() {
                agent_outputs.insert(case.id.clone(), response_text.to_string());
            }
            crate::intelligence::evaluation::ReplayEngine::run_suite(&suite, &agent_outputs)
                .into_iter()
                .map(|run| {
                    serde_json::json!({
                        "case_id": run.case_id,
                        "passed": run.passed,
                        "overall_score": run.score.overall(),
                        "details": run.details,
                    })
                })
                .collect()
        } else {
            vec![]
        }
    };

    // ── Augment result with new wiring fields ──────────────────────────
    if let Some(obj) = result.as_object_mut() {
        obj.insert(
            "schema_warnings".to_string(),
            serde_json::json!(schema_warnings),
        );
        obj.insert("schema_error".to_string(), serde_json::json!(schema_error));
        obj.insert(
            "layered_prompt_segments".to_string(),
            serde_json::json!(layered_prompt_segments_len),
        );
        obj.insert(
            "promotion_decisions".to_string(),
            serde_json::json!(promotion_decisions),
        );
        obj.insert(
            "optimizer_recommendations".to_string(),
            serde_json::json!(optimizer_recommendations),
        );
        obj.insert("execution_plan".to_string(), execution_plan);
        obj.insert(
            "orchestration_alignment".to_string(),
            orchestration_alignment,
        );
        obj.insert(
            "orchestration_node_decisions".to_string(),
            orchestration_node_decisions,
        );
        obj.insert("fork_id".to_string(), serde_json::json!(fork_id));
        obj.insert(
            "evaluation_results".to_string(),
            serde_json::json!(evaluation_results),
        );
        obj.insert("tenant_id".to_string(), serde_json::json!(tenant_id));
        if !reasoning_text.is_empty() {
            obj.insert("thinking".to_string(), serde_json::json!(reasoning_text));
        }
    }

    result
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
