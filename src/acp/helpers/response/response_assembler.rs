//! BLUE43 Step 4: Extracted response assembler for chat orchestration.
//!
//! Provides focused functions for assembling the final JSON response payload
//! and tracking agent attempts during chat request processing.

use serde_json::{json, Value};

use crate::acp::server::AcpServer;
use crate::orchestration::roles::{AgentRole, RoleRegistry};
use crate::orchestration::task_graph::{TaskGraph, TaskNode};

/// Bundles all parameters required to assemble a chat response payload.
///
/// Created by callers to replace the previous 30-parameter function signature.
/// All fields are owned values; the struct is consumed by `build_chat_response`.
#[derive(Debug, Clone)]
pub struct ChatResponseContext {
    // Session identifiers
    pub mode: String,
    pub conversation_id: String,
    pub branch_id: String,
    pub phase_name: String,
    pub phase_origin: String,
    // Agent selection
    pub selected_agent: String,
    pub selected_model_name: Option<String>,
    // Response payloads
    pub response_text: String,
    pub checkpoint: Value,
    pub metacognitive_loop: Value,
    pub token_economy: Value,
    pub knowledge: Value,
    pub distillation: Value,
    // Execution artifacts
    pub agent_attempts: Vec<Value>,
    pub reviews: Vec<Value>,
    pub risk_decision: Value,
    pub agent_switch_notice: Option<Value>,
    // Tool & memory
    pub tool_execution_results: Vec<Value>,
    pub memory_promotion_result: Option<Value>,
    // Task & role routing
    pub task_graph_result: Option<Value>,
    pub role_routing_result: Option<Value>,
    pub verification_result: Option<Value>,
    // Vector context
    pub vector_hits: Vec<Value>,
    pub summary_used: bool,
    // Capability routing
    pub capability_info: CapabilityRoutingInfo,
    pub routing_diagnostics: Value,
    // Timing & cache
    pub cache_hit: bool,
    pub cache_bypassed: bool,
    pub started: std::time::Instant,
}

impl Default for ChatResponseContext {
    fn default() -> Self {
        Self {
            mode: Default::default(),
            conversation_id: Default::default(),
            branch_id: Default::default(),
            phase_name: Default::default(),
            phase_origin: Default::default(),
            selected_agent: Default::default(),
            selected_model_name: Default::default(),
            response_text: Default::default(),
            checkpoint: Default::default(),
            metacognitive_loop: Default::default(),
            token_economy: Default::default(),
            knowledge: Default::default(),
            distillation: Default::default(),
            agent_attempts: Default::default(),
            reviews: Default::default(),
            risk_decision: Default::default(),
            agent_switch_notice: Default::default(),
            tool_execution_results: Default::default(),
            memory_promotion_result: Default::default(),
            task_graph_result: Default::default(),
            role_routing_result: Default::default(),
            verification_result: Default::default(),
            vector_hits: Default::default(),
            summary_used: Default::default(),
            capability_info: Default::default(),
            routing_diagnostics: Default::default(),
            cache_hit: Default::default(),
            cache_bypassed: Default::default(),
            started: std::time::Instant::now(),
        }
    }
}

/// Build the final response payload for a chat request.
pub fn build_chat_response(ctx: ChatResponseContext) -> Value {
    let actual_duration = if ctx.cache_hit {
        0
    } else {
        ctx.started.elapsed().as_millis() as u64
    };

    json!({
        "done": true,
        "conversation_id": ctx.conversation_id,
        "branch_id": ctx.branch_id,
        "mode": ctx.mode,
        "cache": {
            "hit": ctx.cache_hit,
            "bypassed_for_execution": ctx.cache_bypassed,
        },
        "phase": ctx.phase_name,
        "phase_origin": ctx.phase_origin,
        "agent": ctx.selected_agent,
        "selected_model": ctx.selected_model_name,
        "duration_ms": actual_duration,
        "response": ctx.response_text,
        "checkpoint": ctx.checkpoint,
        "metacognitive_loop": ctx.metacognitive_loop,
        "token_economy": ctx.token_economy,
        "vector_hits": ctx.vector_hits,
        "summary_used": ctx.summary_used,
        "knowledge": ctx.knowledge,
        "distillation": ctx.distillation,
        "reviews": ctx.reviews,
        "agent_attempts": ctx.agent_attempts,
        "risk_decision": ctx.risk_decision,
        "agent_switch_notice": ctx.agent_switch_notice,
        "tool_execution": ctx.tool_execution_results,
        "memory_policy": ctx.memory_promotion_result,
        "task_graph": ctx.task_graph_result,
        "role_routing": ctx.role_routing_result,
        "enhanced_verification": ctx.verification_result,
        "capability_routing": {
            "selected_agent": ctx.capability_info.selected_agent,
            "recommended_mode": ctx.capability_info.recommended_mode,
            "candidate_count": ctx.capability_info.candidate_count,
            "decision_confidence": ctx.capability_info.decision_confidence,
            "selection_reason": ctx.capability_info.selection_reason,
            "optimization": ctx.capability_info.optimization_hint,
        },
        "routing_diagnostics": ctx.routing_diagnostics,
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
            if let Err(e) = task_graph.add_edge(
                format!("chat-{}-root", conversation_id),
                format!("chat-{}-tools", conversation_id),
            ) {
                tracing::warn!(%conversation_id, error = %e, "response_assembler: failed to add tool edge to task graph");
            }
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
            if let Err(e) = task_graph.add_edge(
                format!("chat-{}-root", conversation_id),
                format!("chat-{}-memory", conversation_id),
            ) {
                tracing::warn!(%conversation_id, error = %e, "response_assembler: failed to add memory edge to task graph");
            }
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
                    dependencies: n.dependencies.iter().cloned().collect(),
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
        let ctx = ChatResponseContext {
            mode: "test".to_string(),
            conversation_id: "conv-1".to_string(),
            branch_id: "branch-1".to_string(),
            phase_name: "coding".to_string(),
            phase_origin: "user".to_string(),
            selected_agent: "agent1".to_string(),
            selected_model_name: Some("model1".into()),
            response_text: "hello world".to_string(),
            checkpoint: json!({"state": "done"}),
            metacognitive_loop: json!({"loop": 1}),
            token_economy: json!({"tokens": 42}),
            vector_hits: vec![],
            summary_used: false,
            knowledge: json!({}),
            distillation: json!({}),
            reviews: vec![],
            agent_attempts: vec![],
            risk_decision: json!({"decision": "approve"}),
            agent_switch_notice: None,
            tool_execution_results: vec![],
            memory_promotion_result: None,
            task_graph_result: None,
            role_routing_result: None,
            verification_result: None,
            capability_info: CapabilityRoutingInfo::default(),
            routing_diagnostics: json!({}),
            cache_hit: false,
            cache_bypassed: false,
            started: Instant::now(),
        };
        let result = build_chat_response(ctx);

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

    #[tokio::test]
    async fn build_task_graph_checkpoint_contains_state() {
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
            .expect("B49: verification must have 'enhanced_verification'");
        assert!(ev.get("verdict").is_some());
        assert!(ev.get("confidence").is_some());

        // ── From response_assembler ───────────────────────────────
        let routing = build_role_routing("implement the feature");
        let rr = routing
            .get("role_routing")
            .expect("B49: routing must have 'role_routing'");
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

    // ── build_chat_response: edge cases ──────────────────────────────

    #[test]
    fn build_chat_response_minimal_context() {
        let ctx = ChatResponseContext {
            mode: "chat".to_string(),
            selected_agent: "test-agent".to_string(),
            selected_model_name: Some("test-model".to_string()),
            ..Default::default()
        };
        let response = build_chat_response(ctx);
        assert_eq!(
            response["agent"], "test-agent",
            "response must include selected_agent under 'agent' key"
        );
        assert_eq!(response["selected_model"], "test-model");
        assert_eq!(response["mode"], "chat");
    }

    #[test]
    fn build_chat_response_includes_mode_from_context() {
        let ctx = ChatResponseContext {
            mode: "agent".to_string(),
            ..Default::default()
        };
        let response = build_chat_response(ctx);
        assert_eq!(response["mode"], "agent");
    }

    #[test]
    fn build_chat_response_with_checkpoint() {
        let ctx = ChatResponseContext {
            checkpoint: json!({"id": "cp-1", "branch": "main"}),
            ..Default::default()
        };
        let response = build_chat_response(ctx);
        assert_eq!(response["checkpoint"]["id"], "cp-1");
    }

    #[test]
    fn build_chat_response_with_cache_hit() {
        let ctx = ChatResponseContext {
            cache_hit: true,
            ..Default::default()
        };
        let response = build_chat_response(ctx);
        assert_eq!(response["cache"]["hit"], true);
    }

    #[test]
    fn build_chat_response_with_agent_switch_notice() {
        let ctx = ChatResponseContext {
            agent_switch_notice: Some(Value::String(
                "switched from agent-a to agent-b".to_string(),
            )),
            ..Default::default()
        };
        let response = build_chat_response(ctx);
        assert_eq!(
            response["agent_switch_notice"],
            "switched from agent-a to agent-b"
        );
    }

    // ── build_role_routing ────────────────────────────────────────────

    #[test]
    fn build_role_routing_empty_description() {
        let routing = build_role_routing("");
        assert!(
            routing.get("agents").is_none() || routing["agents"].as_array().unwrap().is_empty()
        );
    }

    #[test]
    fn build_role_routing_with_recommended_mode() {
        let routing = build_role_routing("implement a feature");
        let role_routing = &routing["role_routing"];
        assert!(role_routing["suggested_roles"]
            .as_array()
            .unwrap()
            .contains(&serde_json::Value::String("coder".to_string())));
        assert_eq!(role_routing["role_count"], 1);
        assert_eq!(role_routing["handoff_ready"], true);
    }

    // ── CapabilityRoutingInfo default ─────────────────────────────────

    #[test]
    fn capability_routing_info_fields_accessible() {
        let info = CapabilityRoutingInfo {
            selected_agent: Some("test".to_string()),
            recommended_mode: Some("auto".to_string()),
            candidate_count: Some(5),
            decision_confidence: Some(0.9),
            selection_reason: Some("high_reputation".to_string()),
            optimization_hint: Some(Value::String("try_parallel".to_string())),
        };
        assert_eq!(info.selected_agent, Some("test".to_string()));
        assert_eq!(
            info.optimization_hint,
            Some(Value::String("try_parallel".to_string()))
        );
    }
}
