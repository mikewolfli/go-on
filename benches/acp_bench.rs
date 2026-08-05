//! Benchmarks for ACP protocol performance.
//!
//! Measures:
//! - ACP message serialization/deserialization (JSON-RPC wire format)
//! - Session lifecycle operations (new, close) using server builder
//! - Tool descriptor list JSON construction (tools/list payload)
//! - Server status and metrics introspection
//! - Comparison vs simulated in-process (native) agent operations
//!
//! The protocol overhead is quantified by comparing ACP (serde_json encoding
//! on full JSON-RPC messages) against direct in-memory struct construction
//! and manipulation.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use serde_json::{json, Value};
use std::time::Duration;

// ═══════════════════════════════════════════════════════════════════════════
// 1. ACP JSON serialization benchmarks (wire format)
// ── These measure the pure serde_json cost of encoding/decoding the
//    JSON-RPC request/response bodies on the ACP protocol boundary.
// ═══════════════════════════════════════════════════════════════════════════

fn bench_acp_serialize_session_new(c: &mut Criterion) {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "session/new",
        "params": {
            "cwd": "/home/user/project",
            "mode": "ask",
            "work_dirs": ["/home/user/project", "/home/user/other"]
        }
    });

    c.bench_function("acp_serialize/session_new", |b| {
        b.iter(|| serde_json::to_string(black_box(&request)).unwrap());
    });
}

fn bench_acp_deserialize_session_new(c: &mut Criterion) {
    let json_str = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"session/new","params":"#,
        r#"{"cwd":"/home/user/project","mode":"ask","work_dirs":["#,
        r#""/home/user/project","/home/user/other"]}}"#
    );

    c.bench_function("acp_deserialize/session_new", |b| {
        b.iter(|| {
            let _: Value = serde_json::from_str(black_box(json_str)).unwrap();
        });
    });
}

fn bench_acp_serialize_session_close(c: &mut Criterion) {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "session/close",
        "params": {
            "sessionId": "acp-session-abc123"
        }
    });

    c.bench_function("acp_serialize/session_close", |b| {
        b.iter(|| serde_json::to_string(black_box(&request)).unwrap());
    });
}

fn bench_acp_deserialize_session_close(c: &mut Criterion) {
    let json_str = r#"{"jsonrpc":"2.0","id":42,"method":"session/close","params":{"sessionId":"acp-session-abc123"}}"#;

    c.bench_function("acp_deserialize/session_close", |b| {
        b.iter(|| {
            let _: Value = serde_json::from_str(black_box(json_str)).unwrap();
        });
    });
}

fn bench_acp_serialize_session_prompt(c: &mut Criterion) {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/prompt",
        "params": {
            "sessionId": "acp-session-abc123",
            "prompt": [
                {"type": "text", "text": "List all files in the current directory"},
                {"type": "resource", "resource": {
                    "uri": "file:///home/user/project/src/main.rs",
                    "text": "fn main() {}"
                }}
            ]
        }
    });

    c.bench_function("acp_serialize/session_prompt", |b| {
        b.iter(|| serde_json::to_string(black_box(&request)).unwrap());
    });
}

fn bench_acp_deserialize_session_prompt(c: &mut Criterion) {
    let json_str = concat!(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"#,
        r#""sessionId":"acp-session-abc123","prompt":[{"type":"text","#,
        r#""text":"List all files in the current directory"},{"type":"#,
        r#""resource","resource":{"uri":"file:///home/user/project/src/"#,
        r#"main.rs","text":"fn main() {}"}}]}}"#
    );

    c.bench_function("acp_deserialize/session_prompt", |b| {
        b.iter(|| {
            let _: Value = serde_json::from_str(black_box(json_str)).unwrap();
        });
    });
}

fn bench_acp_serialize_tools_list(c: &mut Criterion) {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/list",
        "params": {}
    });

    c.bench_function("acp_serialize/tools_list", |b| {
        b.iter(|| serde_json::to_string(black_box(&request)).unwrap());
    });
}

fn bench_acp_deserialize_tools_list(c: &mut Criterion) {
    let json_str = r#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}"#;

    c.bench_function("acp_deserialize/tools_list", |b| {
        b.iter(|| {
            let _: Value = serde_json::from_str(black_box(json_str)).unwrap();
        });
    });
}

fn bench_acp_serialize_tools_call(c: &mut Criterion) {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "echo_skill",
            "arguments": {
                "input": "hello world"
            },
            "sessionId": "acp-session-abc123"
        }
    });

    c.bench_function("acp_serialize/tools_call", |b| {
        b.iter(|| serde_json::to_string(black_box(&request)).unwrap());
    });
}

fn bench_acp_deserialize_tools_call(c: &mut Criterion) {
    let json_str = concat!(
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"#,
        r#""name":"echo_skill","arguments":{"input":"hello world"},"#,
        r#""sessionId":"acp-session-abc123"}}"#
    );

    c.bench_function("acp_deserialize/tools_call", |b| {
        b.iter(|| {
            let _: Value = serde_json::from_str(black_box(json_str)).unwrap();
        });
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. ACP response serialization benchmarks (server -> client direction)
// ── These measure what happens on the server side when constructing
//    JSON-RPC response payloads.
// ═══════════════════════════════════════════════════════════════════════════

fn bench_acp_response_serialize_session_new(c: &mut Criterion) {
    use go_on::schema::{
        NewSessionResponse, SessionId, SessionMode, SessionModeId, SessionModeState,
    };

    let resp =
        NewSessionResponse::new(SessionId::new("acp-session-abc123")).modes(SessionModeState::new(
            SessionModeId::new("ask"),
            vec![
                SessionMode::new(SessionModeId::new("ask"), "Ask"),
                SessionMode::new(SessionModeId::new("edit"), "Edit"),
            ],
        ));

    c.bench_function("acp_serialize_response/session_new", |b| {
        b.iter(|| serde_json::to_value(black_box(&resp)).unwrap());
    });
}

fn bench_acp_response_serialize_session_close(c: &mut Criterion) {
    use go_on::schema::CloseSessionResponse;

    let resp = CloseSessionResponse { meta: None };

    c.bench_function("acp_serialize_response/session_close", |b| {
        b.iter(|| serde_json::to_value(black_box(&resp)).unwrap());
    });
}

fn bench_acp_response_serialize_prompt(c: &mut Criterion) {
    use go_on::schema::{PromptResponse, StopReason};

    let resp = PromptResponse::new(StopReason::EndTurn);

    c.bench_function("acp_serialize_response/session_prompt", |b| {
        b.iter(|| serde_json::to_value(black_box(&resp)).unwrap());
    });
}

/// Benchmark building a tool list JSON response payload (the core of tools/list)
fn bench_acp_tools_list_response_build(c: &mut Criterion) {
    let tool_names = [
        "acp_trace_get",
        "acp_debug_panel_get",
        "echo_skill",
        "skill-creator",
        "builtin.echo",
        "http_request",
        "workflow_execute",
        "workflow_ask",
        "workflow_generate",
        "import_skill",
        "github_search_skills",
        "prompts_list",
        "prompts_get",
        "skill-finder",
    ];

    c.bench_function("acp_build_tools_list_response", |b| {
        b.iter(|| {
            let tools: Vec<Value> = tool_names
                .iter()
                .map(|name| {
                    json!({
                        "name": name,
                        "description": format!("Tool: {}", name),
                        "input_schema": {"type": "object"}
                    })
                })
                .collect();
            let response = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "tools": tools
                }
            });
            let _ = serde_json::to_string(&response).unwrap();
        });
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Server-level benchmarks (using a minimal AcpServer)
// ── Measures actual server operations: status, metrics, lifecycle checks.
//    NOTE: ServerBuilder::build() calls tokio::spawn internally, so we must
//    create it inside a Tokio runtime.
// ═══════════════════════════════════════════════════════════════════════════

/// Build an AcpServer inside a Tokio runtime (required because build() spawns tasks).
fn build_server_in_runtime() -> go_on::acp::server::AcpServer {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let server = go_on::acp::server::ServerBuilder::new().build();
    // Keep the runtime alive by leaking it so the server's background tasks
    // don't panic when they try to spawn. We only create the server once
    // per benchmark function, so this is fine.
    std::mem::forget(rt);
    server
}

fn bench_acp_server_build(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("acp_server/build", |b| {
        b.iter(|| {
            let _guard = rt.enter();
            let server = go_on::acp::server::ServerBuilder::new().build();
            black_box(server.is_healthy());
            // Server dropped here — fine because we re-enter the runtime for each iter
        });
    });
}

fn bench_acp_server_status(c: &mut Criterion) {
    let server = build_server_in_runtime();

    c.bench_function("acp_server/status", |b| {
        b.iter(|| black_box(server.get_status()));
    });
}

fn bench_acp_server_is_healthy(c: &mut Criterion) {
    let server = build_server_in_runtime();

    c.bench_function("acp_server/is_healthy", |b| {
        b.iter(|| black_box(server.is_healthy()));
    });
}

fn bench_acp_server_shutdown_requested(c: &mut Criterion) {
    let server = build_server_in_runtime();

    c.bench_function("acp_server/shutdown_requested", |b| {
        b.iter(|| black_box(server.shutdown_requested()));
    });
}

fn bench_acp_server_metrics(c: &mut Criterion) {
    let server = build_server_in_runtime();

    c.bench_function("acp_server/metrics", |b| {
        b.iter(|| {
            let m = server.metrics();
            black_box(m.successful_requests());
            black_box(m.failed_requests());
            black_box(m.active_requests());
        });
    });
}

fn bench_acp_server_total_requests(c: &mut Criterion) {
    let server = build_server_in_runtime();

    c.bench_function("acp_server/total_requests", |b| {
        b.iter(|| {
            black_box(server.total_requests());
        });
    });
}

fn bench_acp_server_audit_health(c: &mut Criterion) {
    let server = build_server_in_runtime();

    c.bench_function("acp_server/audit_health", |b| {
        b.iter(|| {
            black_box(server.audit_health());
        });
    });
}

fn bench_acp_server_increment_request(c: &mut Criterion) {
    let server = build_server_in_runtime();

    c.bench_function("acp_server/increment_request", |b| {
        b.iter(|| {
            server.increment_request_counter();
        });
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. Simulated "native" (in-process) comparison benchmarks
// ── These measure the cost of doing equivalent work WITHOUT the JSON-RPC
//    protocol overhead. Comparing these to the ACP benchmarks above gives
//    a direct measurement of protocol overhead.
// ═══════════════════════════════════════════════════════════════════════════

fn bench_native_inprocess_status(c: &mut Criterion) {
    // Simulate what get_status() computes in-process: directly construct
    // status values from internal counters without any serialization.
    let successful: u64 = 42;
    let failed: u64 = 7;
    let active: u64 = 3;

    c.bench_function("native_inprocess/status", |b| {
        b.iter(|| {
            let total = black_box(successful) + black_box(failed) + black_box(active);
            let avg_ms = if total > 0 { 150.0_f64 } else { 0.0 };
            black_box((total, avg_ms));
        });
    });
}

fn bench_native_construct_session_response(c: &mut Criterion) {
    use go_on::schema::{NewSessionResponse, SessionId};

    c.bench_function("native_inprocess/construct_session_new", |b| {
        b.iter(|| {
            let resp = NewSessionResponse::new(SessionId::new("acp-session-bench"));
            black_box(resp.session_id);
        });
    });
}

fn bench_native_construct_prompt_response(c: &mut Criterion) {
    use go_on::schema::{PromptResponse, StopReason};

    c.bench_function("native_inprocess/construct_prompt_response", |b| {
        b.iter(|| {
            let resp = PromptResponse::new(StopReason::EndTurn);
            black_box(resp.stop_reason);
        });
    });
}

fn bench_native_construct_close_response(c: &mut Criterion) {
    use go_on::schema::CloseSessionResponse;

    c.bench_function("native_inprocess/construct_session_close", |b| {
        b.iter(|| {
            let resp = CloseSessionResponse { meta: None };
            black_box(resp.meta);
        });
    });
}

fn bench_native_tool_names_count(c: &mut Criterion) {
    // Simulate what the client does: iterate tool names without any JSON overhead.
    let tool_names = [
        "acp_trace_get",
        "acp_debug_panel_get",
        "goon_workflow_run_list",
        "goon_workflow_run_get",
        "goon_workflow_run_cancel",
        "goon_workflow_run_pause",
        "goon_workflow_run_resume",
        "goon_provider_test_connection",
        "goon_provider_test_completion",
        "goon_provider_capabilities",
        "goon_metrics_window_query",
        "goon_metrics_errors_summary",
        "goon_skill_update",
        "goon_skill_version_list",
        "goon_skill_version_rollback",
        "prompts_list",
        "prompts_get",
        "skill-finder",
        "echo_skill",
        "skill-creator",
        "builtin.echo",
        "http_request",
        "workflow_execute",
        "workflow_ask",
        "workflow_generate",
        "import_skill",
        "github_search_skills",
    ];

    c.bench_function("native_inprocess/tool_names_count", |b| {
        b.iter(|| {
            black_box(tool_names.len());
        });
    });
}

fn bench_native_session_new_struct(c: &mut Criterion) {
    // Full in-process simulation: build a session response struct
    // using the SessionId wrapper (no JSON involved at all).
    use go_on::schema::{
        NewSessionResponse, SessionId, SessionMode, SessionModeId, SessionModeState,
    };

    c.bench_function("native_inprocess/build_session_response_struct", |b| {
        b.iter(|| {
            let session_id = SessionId::new("acp-session-bench");
            let modes = SessionModeState::new(
                SessionModeId::new("ask"),
                vec![
                    SessionMode::new(SessionModeId::new("ask"), "Ask / 对话"),
                    SessionMode::new(SessionModeId::new("edit"), "Edit / 编辑"),
                ],
            );
            let resp = NewSessionResponse::new(session_id).modes(modes);
            black_box(resp.session_id);
        });
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. Mixed JSON-RPC envelope benchmarks (full request/response round-trip)
// ── These simulate the full wire format including the JSON-RPC envelope,
//    which is the true cost of each ACP protocol message exchange.
// ═══════════════════════════════════════════════════════════════════════════

fn bench_acp_roundtrip_session_new(c: &mut Criterion) {
    // Full round-trip: serialize request -> deserialize request -> build response -> serialize response
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "session/new",
        "params": {
            "cwd": "/home/user/project",
            "mode": "ask"
        }
    });
    let json_str = serde_json::to_string(&request).unwrap();

    c.bench_function("acp_roundtrip/session_new", |b| {
        b.iter(|| {
            // Deserialize request
            let req: Value = serde_json::from_str(black_box(&json_str)).unwrap();
            let _method = req.get("method").and_then(Value::as_str);
            let _params = req.get("params");
            // Build response (simplified — just wrap in result)
            let response = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "sessionId": "acp-session-abc123"
                }
            });
            let _ = serde_json::to_string(&response).unwrap();
        });
    });
}

fn bench_acp_roundtrip_tools_list(c: &mut Criterion) {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/list",
        "params": {}
    });
    let json_str = serde_json::to_string(&request).unwrap();

    c.bench_function("acp_roundtrip/tools_list", |b| {
        b.iter(|| {
            let req: Value = serde_json::from_str(black_box(&json_str)).unwrap();
            let _method = req.get("method");
            let response = json!({
                "jsonrpc": "2.0",
                "id": 3,
                "result": {
                    "tools": []
                }
            });
            let _ = serde_json::to_string(&response).unwrap();
        });
    });
}

fn bench_acp_roundtrip_tools_call(c: &mut Criterion) {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "echo_skill",
            "arguments": {"input": "hello"},
            "sessionId": "acp-session-abc"
        }
    });
    let json_str = serde_json::to_string(&request).unwrap();

    c.bench_function("acp_roundtrip/tools_call", |b| {
        b.iter(|| {
            let req: Value = serde_json::from_str(black_box(&json_str)).unwrap();
            let _name = req
                .get("params")
                .and_then(|p| p.get("name"))
                .and_then(Value::as_str);
            let _args = req.get("params").and_then(|p| p.get("arguments"));
            // Build response
            let response = json!({
                "jsonrpc": "2.0",
                "id": 4,
                "result": {
                    "content": [{"type": "text", "text": "done"}]
                }
            });
            let _ = serde_json::to_string(&response).unwrap();
        });
    });
}

fn bench_acp_roundtrip_session_prompt(c: &mut Criterion) {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/prompt",
        "params": {
            "sessionId": "acp-session-abc",
            "prompt": [{"type": "text", "text": "Hello"}]
        }
    });
    let json_str = serde_json::to_string(&request).unwrap();

    c.bench_function("acp_roundtrip/session_prompt", |b| {
        b.iter(|| {
            let req: Value = serde_json::from_str(black_box(&json_str)).unwrap();
            let _session_id = req.get("params").and_then(|p| p.get("sessionId"));
            let _prompt = req.get("params").and_then(|p| p.get("prompt"));
            // Build response
            let response = json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {
                    "stopReason": "end_turn"
                }
            });
            let _ = serde_json::to_string(&response).unwrap();
        });
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// Group definitions
// ═══════════════════════════════════════════════════════════════════════════

criterion_group!(
    name = serialization;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(3))
        .sample_size(100);
    targets =
        bench_acp_serialize_session_new,
        bench_acp_deserialize_session_new,
        bench_acp_serialize_session_close,
        bench_acp_deserialize_session_close,
        bench_acp_serialize_session_prompt,
        bench_acp_deserialize_session_prompt,
        bench_acp_serialize_tools_list,
        bench_acp_deserialize_tools_list,
        bench_acp_serialize_tools_call,
        bench_acp_deserialize_tools_call,
        bench_acp_response_serialize_session_new,
        bench_acp_response_serialize_session_close,
        bench_acp_response_serialize_prompt,
        bench_acp_tools_list_response_build,
);

criterion_group!(
    name = server_ops;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(3))
        .sample_size(100);
    targets =
        bench_acp_server_build,
        bench_acp_server_status,
        bench_acp_server_is_healthy,
        bench_acp_server_shutdown_requested,
        bench_acp_server_metrics,
        bench_acp_server_total_requests,
        bench_acp_server_audit_health,
        bench_acp_server_increment_request,
);

criterion_group!(
    name = native_comparison;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(3))
        .sample_size(100);
    targets =
        bench_native_inprocess_status,
        bench_native_construct_session_response,
        bench_native_construct_prompt_response,
        bench_native_construct_close_response,
        bench_native_tool_names_count,
        bench_native_session_new_struct,
);

criterion_group!(
    name = roundtrip;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(3))
        .sample_size(100);
    targets =
        bench_acp_roundtrip_session_new,
        bench_acp_roundtrip_session_prompt,
        bench_acp_roundtrip_tools_list,
        bench_acp_roundtrip_tools_call,
);

criterion_main!(serialization, server_ops, native_comparison, roundtrip);
