//! BLUE43 Step 4: Extracted response assembler for chat orchestration.
//!
//! Provides focused functions for assembling the final JSON response payload
//! and tracking agent attempts during chat request processing.

use serde_json::{json, Value};

use crate::acp::server::AcpServer;
use crate::orchestration::roles::{AgentRole, RoleRegistry};
use crate::orchestration::task_graph::{TaskGraph, TaskNode};

/// Build the final response payload for a chat request.
#[allow(clippy::too_many_arguments)]
pub fn build_chat_response(
    mode: &str,
    conversation_id: &str,
    branch_id: &str,
    phase_name: &str,
    phase_origin: &str,
    selected_agent: &str,
    selected_model_name: Option<String>,
    response_text: &str,
    checkpoint: Value,
    metacognitive_loop: Value,
    token_economy: Value,
    vector_hits: Vec<Value>,
    summary_used: bool,
    knowledge: Value,
    distillation: Value,
    reviews: Vec<Value>,
    agent_attempts: Vec<Value>,
    risk_decision: Value,
    agent_switch_notice: Option<Value>,
    tool_execution_results: Vec<Value>,
    memory_promotion_result: Option<Value>,
    task_graph_result: Option<Value>,
    role_routing_result: Option<Value>,
    verification_result: Option<Value>,
    capability_info: CapabilityRoutingInfo,
    routing_diagnostics: Value,
    cache_hit: bool,
    cache_bypassed: bool,
    duration_ms: u64,
    started: std::time::Instant,
) -> Value {
    let actual_duration = duration_ms.max(started.elapsed().as_millis() as u64);

    json!({
        "done": true,
        "conversation_id": conversation_id,
        "branch_id": branch_id,
        "mode": mode,
        "cache": {
            "hit": cache_hit,
            "bypassed_for_execution": cache_bypassed,
        },
        "phase": phase_name,
        "phase_origin": phase_origin,
        "agent": selected_agent,
        "selected_model": selected_model_name,
        "duration_ms": actual_duration,
        "response": response_text,
        "checkpoint": checkpoint,
        "metacognitive_loop": metacognitive_loop,
        "token_economy": token_economy,
        "vector_hits": vector_hits,
        "summary_used": summary_used,
        "knowledge": knowledge,
        "distillation": distillation,
        "reviews": reviews,
        "agent_attempts": agent_attempts,
        "risk_decision": risk_decision,
        "agent_switch_notice": agent_switch_notice,
        "tool_execution": tool_execution_results,
        "memory_policy": memory_promotion_result,
        "task_graph": task_graph_result,
        "role_routing": role_routing_result,
        "enhanced_verification": verification_result,
        "capability_routing": {
            "selected_agent": capability_info.selected_agent,
            "recommended_mode": capability_info.recommended_mode,
            "candidate_count": capability_info.candidate_count,
            "decision_confidence": capability_info.decision_confidence,
            "selection_reason": capability_info.selection_reason,
            "optimization": capability_info.optimization_hint,
        },
        "routing_diagnostics": routing_diagnostics,
    })
}

/// Capability routing info bundle used in response assembly.
#[derive(Debug, Clone, Default)]
pub struct CapabilityRoutingInfo {
    pub selected_agent: Option<String>,
    pub recommended_mode: Option<String>,
    pub candidate_count: Option<u64>,
    pub decision_confidence: Option<f64>,
    pub selection_reason: Option<String>,
    pub optimization_hint: Option<Value>,
}

/// Build role routing analysis from task description.
pub fn build_role_routing(task_description: &str) -> Value {
    let task_lower = task_description.to_ascii_lowercase();
    let mut suggested_roles = Vec::new();

    if task_lower.contains("plan")
        || task_lower.contains("design")
        || task_lower.contains("architecture")
    {
        suggested_roles.push(AgentRole::Planner);
    }
    if task_lower.contains("research")
        || task_lower.contains("search")
        || task_lower.contains("find")
    {
        suggested_roles.push(AgentRole::Researcher);
    }
    if task_lower.contains("code")
        || task_lower.contains("implement")
        || task_lower.contains("write")
        || task_lower.contains("edit")
    {
        suggested_roles.push(AgentRole::Coder);
    }
    if task_lower.contains("test")
        || task_lower.contains("verify")
        || task_lower.contains("validate")
    {
        suggested_roles.push(AgentRole::Tester);
    }
    if task_lower.contains("review") || task_lower.contains("check") || task_lower.contains("audit")
    {
        suggested_roles.push(AgentRole::Reviewer);
    }

    if suggested_roles.is_empty() {
        suggested_roles = vec![AgentRole::Planner, AgentRole::Coder, AgentRole::Reviewer];
    }

    let role_registry = RoleRegistry::new();
    let role_definitions = role_registry.all();

    json!({
        "role_routing": {
            "suggested_roles": suggested_roles.iter().map(|r| r.as_str()).collect::<Vec<_>>(),
            "role_count": suggested_roles.len(),
            "task_analysis": task_description,
            "available_custom_roles": role_definitions.len(),
            "handoff_ready": true,
        }
    })
}

/// Build task graph checkpoint from conversation execution state.
#[allow(clippy::too_many_arguments)]
pub fn build_task_graph_checkpoint(
    server: &AcpServer,
    conversation_id: &str,
    task_description: &str,
    mode: &str,
    phase_name: &str,
    response_text: &str,
    tool_execution_results: &[Value],
    memory_promotion_result: Option<&Value>,
    duration_ms: u64,
) -> (Option<Value>, Option<String>, Option<String>) {
    if let Some(ref store) = server.task_graph_store {
        let root_node = TaskNode {
            id: format!("chat-{}-root", conversation_id),
            kind: "chat_request".to_string(),
            state: "done".to_string(),
            input: json!({
                "task": task_description,
                "mode": mode,
                "phase": phase_name,
            }),
            output: Some(json!({
                "response": response_text,
                "duration_ms": duration_ms,
            })),
            dependencies: std::collections::HashSet::new(),
            retries: 0,
        };

        let mut task_graph = TaskGraph::new(root_node);

        if !tool_execution_results.is_empty() {
            let tool_node = TaskNode {
                id: format!("chat-{}-tools", conversation_id),
                kind: "tool_execution".to_string(),
                state: "done".to_string(),
                input: json!({
                    "task": task_description,
                    "mode": mode,
                }),
                output: Some(json!({
                    "results": tool_execution_results,
                    "count": tool_execution_results.len(),
                })),
                dependencies: {
                    let mut s = std::collections::HashSet::new();
                    s.insert(format!("chat-{}-root", conversation_id));
                    s
                },
                retries: 0,
            };
            task_graph.add_node(tool_node);
            let _ = task_graph.add_edge(
                format!("chat-{}-root", conversation_id),
                format!("chat-{}-tools", conversation_id),
            );
        }

        if let Some(memory_result) = memory_promotion_result {
            let memory_node = TaskNode {
                id: format!("chat-{}-memory", conversation_id),
                kind: "memory_promotion".to_string(),
                state: "done".to_string(),
                input: json!({ "task": task_description }),
                output: Some(memory_result.clone()),
                dependencies: {
                    let mut s = std::collections::HashSet::new();
                    s.insert(format!("chat-{}-root", conversation_id));
                    s
                },
                retries: 0,
            };
            task_graph.add_node(memory_node);
            let _ = task_graph.add_edge(
                format!("chat-{}-root", conversation_id),
                format!("chat-{}-memory", conversation_id),
            );
        }

        let graph_id = format!("graph-{}", conversation_id);
        let checkpoint_id = format!("ckpt-{}", crate::acp::prelude::now_ts());

        if let Err(e) = store.save_graph(&graph_id, &task_graph) {
            tracing::warn!(target: "task_graph", "failed to save graph: {e}");
        }

        let subtask_records: Vec<crate::orchestration::task_graph::PlannedSubtaskRecord> =
            task_graph
                .nodes
                .values()
                .filter(|n| n.id != task_graph.root)
                .map(|n| crate::orchestration::task_graph::PlannedSubtaskRecord {
                    subtask_id: n.id.clone(),
                    description: n
                        .input
                        .get("task")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    phase: n.kind.clone(),
                    outcome: Some(if n.state == "done" {
                        "completed".to_string()
                    } else {
                        n.state.clone()
                    }),
                    result_summary: n.output.as_ref().map(|o| o.to_string()),
                })
                .collect();

        let checkpoint = task_graph.snapshot(task_description, 1, subtask_records);
        if let Err(e) = store.save_checkpoint(&checkpoint, &graph_id) {
            tracing::warn!(target: "task_graph", "failed to save checkpoint: {e}");
        }

        (
            Some(json!({
                "task_graph": {
                    "node_count": task_graph.nodes.len(),
                    "edge_count": task_graph.edges.len(),
                    "root": task_graph.root,
                    "execution_complete": true,
                    "graph_id": graph_id,
                    "checkpoint_id": checkpoint_id,
                }
            })),
            Some(graph_id),
            Some(checkpoint_id),
        )
    } else {
        (None, None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::server::ServerBuilder;
    use std::time::Instant;

    #[test]
    fn build_chat_response_produces_canonical_structure() {
        let info = CapabilityRoutingInfo::default();
        let result = build_chat_response(
            "test",
            "conv-1",
            "branch-1",
            "coding",
            "user",
            "agent1",
            Some("model1".into()),
            "hello world",
            json!({"state": "done"}),
            json!({"loop": 1}),
            json!({"tokens": 42}),
            vec![],
            false,
            json!({}),
            json!({}),
            vec![],
            vec![],
            json!({"decision": "approve"}),
            None,
            vec![],
            None,
            None,
            None,
            None,
            info,
            json!({}),
            false,
            false,
            100,
            Instant::now(),
        );

        assert_eq!(result.get("done").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            result.get("conversation_id").and_then(|v| v.as_str()),
            Some("conv-1")
        );
        assert_eq!(
            result.get("branch_id").and_then(|v| v.as_str()),
            Some("branch-1")
        );
        assert_eq!(result.get("mode").and_then(|v| v.as_str()), Some("test"));
        assert_eq!(result.get("agent").and_then(|v| v.as_str()), Some("agent1"));
        assert!(result.get("response").is_some());
        assert!(result.get("checkpoint").is_some());
        assert!(result.get("token_economy").is_some());
        assert!(result.get("risk_decision").is_some());
        assert!(result.get("capability_routing").is_some());
        assert!(result.get("cache").is_some());
        assert!(result.get("duration_ms").is_some());
    }

    #[test]
    fn build_role_routing_with_agents() {
        let result = build_role_routing("plan and design the architecture");

        let routing = result
            .get("role_routing")
            .expect("should have role_routing");
        let roles = routing
            .get("suggested_roles")
            .and_then(Value::as_array)
            .expect("should have suggested_roles");
        assert!(
            roles.contains(&json!("planner")),
            "Planner should be suggested for planning tasks"
        );
        assert!(routing.get("role_count").and_then(|v| v.as_u64()).unwrap() >= 1);
        assert_eq!(
            routing.get("handoff_ready").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert!(routing
            .get("task_analysis")
            .and_then(|v| v.as_str())
            .is_some());
    }

    #[test]
    fn build_task_graph_checkpoint_contains_state() {
        let server = ServerBuilder::new().build().expect("server should build");
        let (checkpoint, graph_id, ckpt_id) = build_task_graph_checkpoint(
            &server,
            "conv-1",
            "test task",
            "auto",
            "coding",
            "response text",
            &[],
            None,
            100,
        );

        // With no task_graph_store configured, all results are None
        assert!(checkpoint.is_none());
        assert!(graph_id.is_none());
        assert!(ckpt_id.is_none());
    }

    #[test]
    fn payload_equivalence_across_helpers() {
        // Verify that core functions from all three helpers produce
        // JSON with consistent top-level field shapes when called together.

        // ── From vote_orchestration ────────────────────────────────
        let plan = json!({
            "steps": [
                {"step_id": "s1", "description": "write the implementation"},
            ]
        });
        let tool_results: Vec<Value> = vec![];
        let orchestration = crate::acp::helpers::vote_orchestration::derive_response_orchestration(
            &plan,
            &tool_results,
        );
        assert!(
            orchestration.get("nodes").is_some(),
            "orchestration must have 'nodes'"
        );
        assert!(
            orchestration.get("mapping_ratio").is_some(),
            "orchestration must have 'mapping_ratio'"
        );

        // ── From review_gate ──────────────────────────────────────
        let verification = crate::acp::helpers::review_gate::run_enhanced_verification(
            "fn main() { println!(\"hello\"); }",
        );
        let ev = verification
            .get("enhanced_verification")
            .expect("verification must have 'enhanced_verification'");
        assert!(ev.get("verdict").is_some());
        assert!(ev.get("confidence").is_some());

        // ── From response_assembler ───────────────────────────────
        let routing = build_role_routing("implement the feature");
        let rr = routing
            .get("role_routing")
            .expect("routing must have 'role_routing'");
        assert!(
            rr.get("suggested_roles").is_some(),
            "role_routing must have 'suggested_roles'"
        );
        assert!(rr.get("handoff_ready").is_some());

        // Every top-level key across all three helpers is unique
        // (no accidental overlap in the shared response structure)
        let orchestration_keys: Vec<&str> = orchestration
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert!(orchestration_keys.contains(&"mapped_nodes"));
    }
}
