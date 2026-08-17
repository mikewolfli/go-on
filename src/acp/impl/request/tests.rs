// Test suite for the request dispatch module.
//
// Moved verbatim from the original `mod tests` in `request.rs` (module split
// M0.4). All names resolve through the explicit `super::` imports below, so
// no `use super::*;` glob is needed.

use serde_json::{json, Value};

#[cfg(not(feature = "backend-postgres"))]
use super::collect_vector_context_snippets;
use super::{
    attach_request_dispatch_context, classify_request_error_kind, infer_workflow_parallelism,
    is_acp_request, rebalance_execution_order, session_id_for_task, summarize_lock_health,
    with_error_contract_data, LockHealthSummary,
};
#[cfg(not(feature = "backend-postgres"))]
use crate::vector::VectorStore;
#[cfg(not(feature = "backend-postgres"))]
use std::sync::Arc;
// Tests that touch the process-global CURRENT_TRANSPORT must be serialized
// (the same pattern as chat_tests.rs) so parallel runs cannot race on the
// global transport slot.
use serial_test::serial;

#[tokio::test]
#[serial]
async fn auto_mode_normalizes_bare_mcp_methods() {
    // Regression: in Auto (adaptive) mode a bare MCP method name such as
    // `ping` previously fell into the dispatch `_ =>` branch and was
    // answered with -32601 MethodNotFound, because the Auto arm did not
    // run `normalize_mcp_method` (only the Mcp arm did). Auto mode must
    // normalize bare MCP methods (`ping` -> `mcp.ping`) while keeping
    // `initialize` on ACP semantics.
    use crate::acp::server::ServerBuilder;
    use crate::acp::transport::{with_transport, RpcBufferTransport};
    use std::sync::Arc;

    let buffer = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let transport = Arc::new(RpcBufferTransport::new(buffer.clone()));

    let mut server = ServerBuilder::new().build();
    // Adaptive (Auto) dispatch mode is the default when protocol_mode is
    // unset; pin it explicitly so the test does not depend on defaults.
    server.runtime_config.protocol_mode = Some("adaptive".to_string());

    with_transport(
        transport.clone() as Arc<dyn crate::acp::transport::Transport>,
        async {
            // Bare `ping` must produce the mcp.ping result, not MethodNotFound.
            super::handle_request(
                &server,
                crate::rpc_protocol::JsonRpcRequest {
                    jsonrpc: "2.0".to_string(),
                    method: "ping".to_string(),
                    params: Some(json!({})),
                    id: Some(json!(1)),
                },
                None,
            )
            .await
            .expect("handle_request must complete");
            let ping_resp = transport
                .last_response()
                .await
                .expect("dispatch must emit a JSON-RPC response");
            assert_eq!(ping_resp["id"], json!(1));
            assert!(
                ping_resp.get("error").is_none(),
                "bare ping in Auto mode must not be MethodNotFound, got: {ping_resp}"
            );
            assert!(
                ping_resp.get("result").is_some(),
                "bare ping in Auto mode must produce the mcp.ping result, got: {ping_resp}"
            );

            // Bare `notifications/initialized` is an MCP notification — it must be
            // recognized (normalized to mcp.notifications_initialized) and produce
            // no response, rather than falling into MethodNotFound.
            super::handle_request(
                &server,
                crate::rpc_protocol::JsonRpcRequest {
                    jsonrpc: "2.0".to_string(),
                    method: "notifications/initialized".to_string(),
                    params: Some(json!({})),
                    id: None,
                },
                None,
            )
            .await
            .expect("handle_request must complete");
            assert!(
                !buffer.lock().await.is_empty(),
                "dispatch must have written the notification path output"
            );

            // Negative control: a genuinely unknown method is still MethodNotFound.
            super::handle_request(
                &server,
                crate::rpc_protocol::JsonRpcRequest {
                    jsonrpc: "2.0".to_string(),
                    method: "no.such.method".to_string(),
                    params: Some(json!({})),
                    id: Some(json!(2)),
                },
                None,
            )
            .await
            .expect("handle_request must complete");
            let unknown_resp = transport
                .last_response()
                .await
                .expect("dispatch must emit a JSON-RPC response");
            assert_eq!(unknown_resp["error"]["code"], json!(-32601));
        },
    )
    .await;
}

#[tokio::test]
#[serial]
async fn auto_mode_notifications_cancelled_is_silent_and_marks() {
    // Regression: `notifications/cancelled` in Auto mode previously fell
    // into the dispatch `_ =>` branch and was answered with -32601
    // MethodNotFound, because `is_mcp_request` / `normalize_mcp_method`
    // did not recognize it. It must be normalized to
    // `mcp.notifications_cancelled`, mark the shared cancelled-request
    // registry (the semantics of the native MCP arm's
    // `mark_cancelled_request`), and produce no JSON-RPC response.
    use crate::acp::server::ServerBuilder;
    use crate::acp::transport::{with_transport, RpcBufferTransport};
    use std::sync::Arc;

    let buffer = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let transport = Arc::new(RpcBufferTransport::new(buffer.clone()));

    let mut server = ServerBuilder::new().build();
    server.runtime_config.protocol_mode = Some("adaptive".to_string());

    with_transport(
        transport.clone() as Arc<dyn crate::acp::transport::Transport>,
        async {
            super::handle_request(
                &server,
                crate::rpc_protocol::JsonRpcRequest {
                    jsonrpc: "2.0".to_string(),
                    method: "notifications/cancelled".to_string(),
                    params: Some(json!({ "requestId": 77 })),
                    id: None,
                },
                None,
            )
            .await
            .expect("handle_request must complete");

            // Id-less notifications never produce a response — in particular no
            // MethodNotFound error may be emitted for the recognized method.
            assert!(
                transport.last_response().await.is_none(),
                "notifications/cancelled must not emit a response"
            );
        },
    )
    .await;
    // The cancellation mark must be applied to the shared registry so an
    // in-flight request with this id aborts early.
    assert!(
        super::protocol_pack::is_acp_request_cancelled("77"),
        "notifications/cancelled must mark request id 77 for cancellation"
    );
    // Consume the mark so a later request reusing id 77 is not cancelled.
    super::protocol_pack::clear_acp_request_cancelled("77");
}

#[test]
fn is_acp_request_recognizes_known_methods() {
    // Key protocol methods
    assert!(is_acp_request("initialize"));
    assert!(is_acp_request("chat"));
    assert!(is_acp_request("session/new"));
    assert!(is_acp_request("shutdown"));
    // MCP-bridge methods
    assert!(is_acp_request("mcp.initialize"));
    assert!(is_acp_request("mcp.tools.list"));
    assert!(is_acp_request("mcp.tools.call"));
    // Skill methods
    assert!(is_acp_request("skill.import"));
    assert!(is_acp_request("skill.create"));
    // Workflow methods
    assert!(is_acp_request("workflow.execute"));
    assert!(is_acp_request("workflow.confirm"));
    // Prompt methods
    assert!(is_acp_request("prompts.list"));
    assert!(is_acp_request("prompts.get"));
    // Tool methods (registered in MethodRouter and ACP_METHODS list)
    assert!(is_acp_request("tools/list"));
    assert!(is_acp_request("tools/call"));
    // Terminal + approval methods live in the sorted ACP_METHODS list;
    // binary_search depends on the list staying alphabetically sorted, so
    // these were previously unreachable in ACP mode (see log 20260806-7).
    assert!(is_acp_request("terminal/create"));
    assert!(is_acp_request("terminal/kill"));
    assert!(is_acp_request("terminal/output"));
    assert!(is_acp_request("terminal/wait_for_exit"));
    assert!(is_acp_request("tool.approve"));
    // Unknown methods return false
    assert!(!is_acp_request("unknown.method"));
    assert!(!is_acp_request(""));
}

#[test]
fn acp_methods_list_is_sorted_for_binary_search() {
    // The production `is_acp_request` uses `binary_search`, which silently
    // misses entries when the list is not alphabetically sorted (this made
    // `tool.approve` / `terminal/kill` unreachable in ACP mode — see
    // log 20260806-7 round-2 regression verification). Assert the invariant
    // against the real list to prevent silent regressions.
    let list = super::protocol::ACP_METHODS;
    let mut prev: Option<&str> = None;
    for entry in list {
        if let Some(p) = prev {
            assert!(
                p < *entry,
                "ACP_METHODS out of order: {p:?} must sort before {entry:?}"
            );
        }
        prev = Some(entry);
    }
}

#[test]
fn session_id_for_task_compacts_to_ascii_alnum() {
    let value = session_id_for_task("Fix #123: add review stage and docs");
    // When i18n is loaded, returns formatted template with id.
    // In bare test mode, falls back to i18n key or formatted template.
    assert!(!value.is_empty());
    // The compact id should appear in the formatted result or fallback key
    let has_compact_id = value.contains("Fix123addreviewstageand");
    let has_fallback = value.contains("info.request.session_id_format");
    assert!(has_compact_id || has_fallback, "value: {value}");
}

#[test]
fn session_id_for_task_has_fallback_when_empty() {
    let value = session_id_for_task("!!!");
    // Empty task chars → fallback to "session"
    let has_session = value.contains("session");
    let has_fallback = value.contains("info.request.session_id_format");
    assert!(has_session || has_fallback, "value: {value}");
}

#[test]
fn rebalance_execution_order_splits_wide_phase_by_limit() {
    let execution_order = vec![
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
        vec!["d".to_string()],
    ];
    let rebalanced = rebalance_execution_order(&execution_order, 2);

    assert_eq!(
        rebalanced,
        vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["c".to_string()],
            vec!["d".to_string()]
        ]
    );
}

#[test]
fn rebalance_execution_order_limit_one_serializes_all_nodes() {
    let execution_order = vec![
        vec!["a".to_string(), "b".to_string()],
        vec!["c".to_string()],
    ];
    let rebalanced = rebalance_execution_order(&execution_order, 1);

    assert_eq!(
        rebalanced,
        vec![
            vec!["a".to_string()],
            vec!["b".to_string()],
            vec!["c".to_string()]
        ]
    );
}

#[test]
fn infer_workflow_parallelism_reads_max_phase_width() {
    let workflow = crate::reinforcement::WorkflowGeneratedArtifact {
        generated_at: 0,
        task: "task".to_string(),
        nodes: Vec::new(),
        edges: Vec::new(),
        execution_order: vec![
            vec!["a".to_string()],
            vec!["b".to_string(), "c".to_string(), "d".to_string()],
        ],
        auto_gates: Vec::new(),
        routing_summary: serde_json::json!({}),
    };

    assert_eq!(infer_workflow_parallelism(&workflow), 3);
}

#[cfg(not(feature = "backend-postgres"))]
#[tokio::test]
async fn collect_vector_context_snippets_searches_execution_and_semantic_phase() {
    let dir = tempfile::tempdir().expect("temp dir should be created");
    let db_path = dir.path().join("request-vector-dual-phase.sqlite3");
    let store =
        Arc::new(VectorStore::new(&db_path, 64, 256).expect("vector store should initialize"));

    Arc::clone(&store)
        .upsert(
            "coding",
            "fix retrieval alignment",
            "semantic-phase knowledge",
        )
        .await
        .expect("semantic phase upsert should succeed");

    // No entries under execution phase key; this verifies we still retrieve
    // by semantic phase fallback and avoid false miss caused by key mismatch.
    let phases = vec!["phase-1".to_string(), "coding".to_string()];
    let snippets =
        collect_vector_context_snippets(Arc::clone(&store), &phases, "fix retrieval alignment", 3)
            .await;

    assert!(!snippets.is_empty());
    assert!(snippets
        .iter()
        .any(|s| s.contains("semantic-phase knowledge")));
}

#[test]
fn classify_request_error_kind_detects_pua_violation() {
    let error = anyhow::anyhow!("PUA red line violation: blocked action");
    assert_eq!(classify_request_error_kind(&error), "PuaViolation");
}

#[test]
fn classify_request_error_kind_detects_budget_exceeded() {
    let error = anyhow::anyhow!("budget denied tool 'x' in scope 'y': budget exceeded");
    assert_eq!(classify_request_error_kind(&error), "BudgetExceeded");
}

#[test]
fn classify_request_error_kind_detects_sandbox_blocked() {
    let error = anyhow::anyhow!("hardening policy denied tool 'shell': sandbox strict");
    assert_eq!(classify_request_error_kind(&error), "SandboxBlocked");
}

#[test]
fn with_error_contract_data_infers_retryable_rate_limit() {
    let data = with_error_contract_data(-32029, "rate limited", None)
        .expect("error contract data should be present");
    assert_eq!(data["kind"], Value::String("RateLimited".to_string()));
    assert_eq!(data["retry"]["retryable"], Value::Bool(true));
    assert_eq!(data["retry"]["max_retries"], Value::Number(3.into()));
}

#[test]
fn with_error_contract_data_preserves_explicit_kind_and_detail() {
    let data = with_error_contract_data(
        -32603_i32,
        "generic failure",
        Some(json!({"kind": "PuaViolation", "detail": "acp.handle_request.dispatch"})),
    )
    .expect("error contract data should be present");
    assert_eq!(data["kind"], Value::String("PuaViolation".to_string()));
    assert_eq!(
        data["detail"],
        Value::String("acp.handle_request.dispatch".to_string())
    );
    assert_eq!(data["retry"]["retryable"], Value::Bool(false));
}

#[test]
fn summarize_lock_health_marks_poisoned_components_warn() {
    let summary = summarize_lock_health(&[
        LockHealthSummary {
            status: "warn",
            poisoned_total: 1,
            recovered_total: 1,
            slow_wait_total: 0,
            max_wait_ms: 1.2,
            components_tracked: 1,
        },
        LockHealthSummary {
            status: "healthy",
            poisoned_total: 0,
            recovered_total: 0,
            slow_wait_total: 0,
            max_wait_ms: 0.5,
            components_tracked: 1,
        },
    ]);

    assert_eq!(summary.status, "warn");
    assert_eq!(summary.poisoned_total, 1);
    assert_eq!(summary.recovered_total, 1);
    assert_eq!(summary.components_tracked, 2);
}

#[test]
fn attach_request_dispatch_context_adds_method() {
    let err = anyhow::anyhow!("test error");
    let wrapped = attach_request_dispatch_context(err, "test.method");
    let msg = wrapped.to_string();
    assert!(msg.contains("test.method"));
    assert!(msg.contains("acp.handle_request.dispatch"));
}

// ── Lock health summary ───────────────────────────────────────────

#[test]
fn lock_health_summary_empty_is_not_monitored() {
    let summary = summarize_lock_health(&[]);
    // An empty component set must not claim a vacuous "healthy" state
    // (log-20260622-5): monitoring is disabled, so report that truthfully.
    assert_eq!(summary.status, "not_monitored");
    assert_eq!(summary.components_tracked, 0);
}

#[test]
fn lock_health_summary_no_poisoned_healthy() {
    let summary = summarize_lock_health(&[LockHealthSummary {
        status: "healthy",
        poisoned_total: 0,
        recovered_total: 0,
        slow_wait_total: 0,
        max_wait_ms: 0.5,
        components_tracked: 1,
    }]);
    assert_eq!(summary.status, "healthy");
    assert_eq!(summary.poisoned_total, 0);
}
