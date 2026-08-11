//! Final Response Post-Processing Pipeline for chat requests.
//!
//! Extracted from `process_chat_request` in `chat.rs` to isolate the
//! multi-step post-processing logic that runs after the primary agent
//! response is built.  This pipeline handles provenance, capability
//! feedback, budget tracking, planner-driven orchestration alignment
//! counters, and final result augmentation.

use std::time::Instant;

use serde_json::{json, Value};

use crate::acp::helpers::agent_router::record_task_agent_outcome;
use crate::acp::helpers::autonomy_metrics::{
    record_orchestration_alignment, record_orchestration_node_mapping,
};
use crate::acp::helpers::orchestration_alignment::derive_orchestration_node_decisions;
use crate::acp::helpers::orchestration_alignment::derive_plan_trace_alignment;
use crate::acp::server::AcpServer;
use crate::orchestration::brain_loop::plan_construction::Planner;
use crate::rpc_protocol::RequestTraceContext;

/// Run the final response post-processing pipeline.
///
/// This function encapsulates all post-agent-execution steps:
/// provenance logging, capability bus feedback, budget accounting,
/// orchestration alignment counters, and result augmentation.
///
/// Returns the augmented response `Value` with the retained fields injected.
#[allow(clippy::too_many_arguments)]
pub async fn finalize_chat_response(
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
    // Routing / diagnostic context
    candidate_agents: &[String],
    // Planner input
    first_message_content: &str,
) -> Value {
    // ── Step 1+2: Collect agent outputs & record orchestration alignment
    // counters. They are independent (both consume shared references only),
    // so run them concurrently instead of serializing their awaits.
    let (_, _) = tokio::join!(
        collect_agent_outputs(
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
            candidate_agents,
        ),
        record_plan_alignment(
            mode,
            phase_name,
            selected_agent,
            conversation_id,
            first_message_content,
            tool_execution_results,
        ),
    );

    // ── Step 3: Format the final response body ────────────────────────
    format_response_body(
        &mut result,
        reasoning_text,
        schema_warnings,
        schema_error,
        layered_prompt_segments_len,
        tenant_id,
    );

    result
}

/// Gather all agent execution outputs and record side-effects.
///
/// Performs provenance logging, capability bus feedback, agent outcome
/// recording, and tenant budget tracking.
#[allow(clippy::too_many_arguments)]
async fn collect_agent_outputs(
    server: &AcpServer,
    trace: &RequestTraceContext,
    mode: &str,
    phase_name: &str,
    selected_agent: &str,
    selected_model_name: &Option<String>,
    response_text: &str,
    _tenant_id: &str,
    started: Instant,
    result: &Value,
    conversation_id: &str,
    candidate_agents: &[String],
) {
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
        let timestamp_ms = crate::shared::timestamps::now_ts_ms_u64();
        ledger.append(crate::shared::provenance_helpers::ProvenanceEntry {
            id: format!("routing:{}", trace.request_id),
            task_id: trace.request_id.clone(),
            phase: phase_name.to_string(),
            agent: selected_agent.to_string(),
            tool: "routing.deliberation".to_string(),
            input_digest: crate::shared::provenance_helpers::digest(&route_input),
            output_digest: crate::shared::provenance_helpers::digest(&route_output),
            upstream_ids: Vec::new(),
            timestamp_ms,
            rationale: None,
            metadata: json!({
                "route_input": route_input,
                "route_output": route_output,
            }),
        });
    }

    let elapsed = started.elapsed().as_millis() as u64;
    let used_tokens = result
        .get("token_economy")
        .and_then(|v| v.get("total_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let request_succeeded = !response_text.trim().is_empty();

    // NOTE: LivePerformanceFeed is deliberately NOT written here — the
    // per-attempt write in fallback.rs (record_agent_intelligence_outcome) is
    // the single production write point. A request-level write here would
    // double-record the winning fallback agent (once per attempt, once per
    // request) and pollute the EMA. Autonomy/vote paths still don't feed the
    // feed; closing that gap requires a per-attempt write on those paths
    // (design debt, see docs/log/log-20260811-2.md).

    // ── CapabilityBus feedback on execution outcome ────────────────────
    // Single feedback point per request (weight 1.0, stable conversation_id,
    // real elapsed/used_tokens). NOTE: the per-request evolve() spawn was
    // removed — it bypassed the evolve_interval throttle (~1.5s of background
    // RL/cognitive CPU per request). Evolve is driven by the throttled path
    // in capability_bus_feedback (every evolve_interval requests).
    if let Some(ref cb) = server.governance_deps.capability_bus {
        cb.feedback(
            selected_agent,
            phase_name,
            conversation_id,
            request_succeeded,
            elapsed,
            used_tokens,
            1.0,
        )
        .await;
    }

    record_task_agent_outcome(phase_name, selected_agent, request_succeeded);

    // ── TenantBudgetEnforcer record usage (F-GAP-08) ───────────────────
    #[cfg(feature = "multi-users-server")]
    {
        let mut budget = server
            .rate_limiting
            .tenant_budget
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("tenant_budget lock poisoned in collect_agent_outputs");
                poisoned.into_inner()
            });
        budget.record_usage(_tenant_id, used_tokens as usize, 1);
    }
}

/// Record orchestration alignment counters from the generated execution plan.
///
/// The counters feed `autonomy_metrics_snapshot` (status / debug panel). The
/// rich plan/alignment payloads previously injected into the response were
/// removed — no consumer reads them.
#[allow(clippy::too_many_arguments)]
async fn record_plan_alignment(
    mode: &str,
    phase_name: &str,
    selected_agent: &str,
    conversation_id: &str,
    first_message_content: &str,
    tool_execution_results: &[Value],
) {
    // ── Planner/Executor integration (activated, formerly F-GAP-05) ────
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
        Planner::plan(&envelope)
    };
    let plan_value = json!({
        "plan_id": execution_plan.plan_id,
        "steps": execution_plan.steps.iter().map(|s| json!({
            "step_id": s.step_id,
            "description": s.description,
            "depends_on": s.depends_on,
        })).collect::<Vec<_>>(),
    });

    let orchestration_alignment = derive_plan_trace_alignment(&plan_value, tool_execution_results);
    let alignment_coverage = orchestration_alignment
        .get("coverage_ratio")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    record_orchestration_alignment(alignment_coverage);

    let orchestration_node_decisions =
        derive_orchestration_node_decisions(&plan_value, tool_execution_results);
    let mapped_nodes = orchestration_node_decisions
        .get("mapped_nodes")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let unmapped_nodes = orchestration_node_decisions
        .get("unmapped_nodes")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    record_orchestration_node_mapping(mapped_nodes, unmapped_nodes);
}

/// Inject the retained fields into the final response object.
///
/// The zero-consumer metadata fields (promotion_decisions,
/// optimizer_recommendations, execution_plan, orchestration_alignment,
/// orchestration_node_decisions, fork_id, evaluation_results) are no longer
/// injected — no consumer reads them.
fn format_response_body(
    result: &mut Value,
    reasoning_text: &str,
    schema_warnings: Vec<String>,
    schema_error: Option<String>,
    layered_prompt_segments_len: usize,
    _tenant_id: &str,
) {
    if let Some(obj) = result.as_object_mut() {
        obj.insert("schema_warnings".to_string(), json!(schema_warnings));
        obj.insert("schema_error".to_string(), json!(schema_error));
        obj.insert(
            "layered_prompt_segments".to_string(),
            json!(layered_prompt_segments_len),
        );
        obj.insert("_tenant_id".to_string(), json!(_tenant_id));
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

    #[tokio::test]
    async fn test_finalize_chat_response_returns_augmented_result() {
        let server = ServerBuilder::new().build();
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
            &["coder-agent".to_string()],
            "Fix the bug",
        )
        .await;

        // Verify the result includes the augmented fields
        assert!(
            augmented.get("response").is_some(),
            "should retain original fields"
        );
        assert!(
            augmented.get("schema_warnings").is_some(),
            "should have schema_warnings"
        );
        assert!(
            augmented.get("_tenant_id").is_some(),
            "should have _tenant_id"
        );
        // Zero-consumer metadata payloads are no longer injected.
        assert!(
            augmented.get("execution_plan").is_none(),
            "execution_plan injection removed (no consumer)"
        );
    }

    #[tokio::test]
    async fn test_finalize_chat_response_handles_empty_tools() {
        let server = ServerBuilder::new().build();
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
            &[],
            "Test objective",
        )
        .await;

        assert!(augmented.get("schema_error").is_some());
        assert!(augmented.get("schema_warnings").is_some());

        // With reasoning_text non-empty, thinking field should be present
        assert!(
            augmented.get("thinking").is_some(),
            "should have thinking field when reasoning_text is non-empty"
        );
    }

    #[tokio::test]
    async fn test_finalize_chat_response_with_tool_results() {
        let server = ServerBuilder::new().build();
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
            &["researcher-agent".to_string()],
            "Research the codebase for the authentication bug",
        )
        .await;

        // The retained contract: original fields + schema/_tenant_id markers.
        assert!(
            augmented.get("response").is_some(),
            "should retain original response field"
        );
        assert!(augmented.get("schema_warnings").is_some());
        assert!(augmented.get("_tenant_id").is_some());
        // Zero-consumer metadata payloads are no longer injected.
        assert!(
            augmented.get("orchestration_alignment").is_none(),
            "orchestration_alignment injection removed (no consumer)"
        );
    }
}
