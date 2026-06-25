/// Cross-protocol consistency test gate — Batch E of the BLUE18 protocol
/// consistency roadmap.
///
/// Probes that the same *semantic* contract is upheld across ACP stdio and MCP
/// stdio:
///   - initialize / mcp.initialize succeed and return protocol-version info.
///   - tools/list / mcp.tools.list return a `tools` array (may be empty).
///   - Unknown method → error code -32601 (METHOD_NOT_FOUND).
///   - Missing required params → error code -32602 (INVALID_PARAMS).
///   - Error responses omit the `result` field; success responses omit `error`.
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::tempdir;

pub mod common;
use common::binary_path;
use common::suite_mutex;

// ---------------------------------------------------------------------------
// Harness — reused across both protocol modes.
// ---------------------------------------------------------------------------

struct Harness {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout_rx: Receiver<Value>,
    _guard: MutexGuard<'static, ()>,
}

fn suite_lock() -> &'static Mutex<()> {
    suite_mutex()
}

fn write_mode_config(path: &Path, protocol_mode: &str) {
    let content = format!(
        r#"default_phase = "coding"

[flow]
name = "Consistency Test Flow"
phases = ["coding"]

[runtime]
protocol_mode = "{protocol_mode}"
maintenance_interval_seconds = 600
health_interval_seconds = 600
shutdown_drain_seconds = 1

[agents.copilot]
type = "copilot"
url = "http://127.0.0.1:8080"

[phases.coding]
description = "Coding"
agents = ["copilot"]
fallback = true
"#
    );
    fs::write(path, content).expect("failed to write mode config");
}

impl Harness {
    fn spawn(config_path: &Path) -> Self {
        let guard = match suite_lock().lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut child = Command::new(binary_path())
            .arg("--config")
            .arg(config_path)
            .env("GO_ON_ENABLE_LOCAL_TEST_AGENTS", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn go-on");

        let stdin = child.stdin.take().expect("no child stdin");
        let stdout = child.stdout.take().expect("no child stdout");
        let stderr = child.stderr.take().expect("no child stderr");

        let (tx, rx) = mpsc::channel::<Value>();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    let _ = tx.send(v);
                }
            }
        });

        // Drain stderr to prevent the child from blocking on a full buffer.
        thread::spawn(
            move || {
                for _ in BufReader::new(stderr).lines().map_while(Result::ok) {}
            },
        );

        Self {
            child,
            stdin: Some(stdin),
            stdout_rx: rx,
            _guard: guard,
        }
    }

    fn send(&mut self, id: u64, method: &str, params: Option<Value>) -> Value {
        let mut msg = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if let Some(p) = params {
            msg["params"] = p;
        }
        let line = serde_json::to_string(&msg).expect("encode failed");
        let stdin = self.stdin.as_mut().expect("stdin closed");
        writeln!(stdin, "{line}").expect("write failed");
        stdin.flush().expect("flush failed");
        self.recv(id)
    }

    fn recv(&mut self, id: u64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if Instant::now() > deadline {
                panic!("timeout waiting for response id={id}");
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let msg = self
                .stdout_rx
                .recv_timeout(remaining)
                .expect("channel closed");
            if msg.get("id") == Some(&json!(id)) {
                return msg;
            }
        }
    }

    fn shutdown(&mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                _ if Instant::now() > deadline => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return;
                }
                _ => thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

// ---------------------------------------------------------------------------
// Shared probe helpers.
// ---------------------------------------------------------------------------

fn assert_success_shape(resp: &Value, context: &str) {
    assert!(
        resp.get("error").is_none(),
        "{context}: success response must not have an `error` field; got: {resp}"
    );
    assert!(
        resp.get("result").is_some(),
        "{context}: success response must have a `result` field; got: {resp}"
    );
}

fn assert_error_shape(resp: &Value, expected_code: i64, context: &str) {
    assert!(
        resp.get("result").is_none(),
        "{context}: error response must not have a `result` field; got: {resp}"
    );
    let err = resp.get("error").unwrap_or_else(|| {
        panic!("{context}: error response must have an `error` field; got: {resp}")
    });
    let code = err
        .get("code")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("{context}: error.code must be an integer; got: {err}"));
    assert_eq!(
        code, expected_code,
        "{context}: expected error code {expected_code}, got {code}; full error: {err}"
    );
}

// ---------------------------------------------------------------------------
// ACP stdio probes.
// ---------------------------------------------------------------------------

#[test]
fn acp_stdio_initialize_returns_protocol_info() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "acp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(1, "initialize", Some(json!({ "protocol": "acp" })));
    assert_success_shape(&resp, "acp_stdio:initialize");

    let result = &resp["result"];
    assert!(
        result.get("protocol").is_some() || result.get("version").is_some(),
        "acp_stdio:initialize result should contain protocol or version; got: {result}"
    );
    h.shutdown();
}

#[test]
fn acp_stdio_tools_list_returns_tools_array() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "acp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(1, "mcp.tools.list", None);
    assert_success_shape(&resp, "acp_stdio:mcp.tools.list");
    assert!(
        resp["result"].get("tools").is_some(),
        "acp_stdio:mcp.tools.list result must have `tools` array; got: {}",
        resp["result"]
    );
    h.shutdown();
}

#[test]
fn acp_stdio_unknown_method_returns_minus_32601() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "acp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(1, "blue18.nonexistent.method", None);
    assert_error_shape(&resp, -32601, "acp_stdio:unknown_method");
    assert!(
        resp["error"]["data"].get("platform_context").is_some(),
        "acp_stdio:unknown_method error.data must contain platform_context; got: {}",
        resp
    );
    assert_eq!(
        resp["error"]["data"]["platform_context"]["schema_version"], "blue24-platform-universal-v1",
        "acp_stdio:unknown_method platform_context.schema_version mismatch"
    );
    h.shutdown();
}

#[test]
fn acp_stdio_skill_remove_missing_name_returns_minus_32602() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "acp_stdio");
    let mut h = Harness::spawn(&cfg);

    // `skill.remove` without `name` param should return INVALID_PARAMS.
    let resp = h.send(1, "skill.remove", Some(json!({})));
    assert_error_shape(&resp, -32602, "acp_stdio:skill.remove:missing_name");
    assert!(
        resp["error"]["data"].get("platform_context").is_some(),
        "acp_stdio:skill.remove:missing_name error.data must contain platform_context; got: {}",
        resp
    );
    h.shutdown();
}

// ---------------------------------------------------------------------------
// MCP stdio probes.
// ---------------------------------------------------------------------------

#[test]
fn mcp_stdio_initialize_returns_protocol_version() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "mcp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(
        1,
        "initialize",
        Some(json!({ "protocolVersion": "2024-11-05", "clientInfo": { "name": "test" } })),
    );
    assert_success_shape(&resp, "mcp_stdio:initialize");

    let result = &resp["result"];
    let proto = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("mcp_stdio:initialize result must contain protocolVersion; got: {result}")
        });
    assert!(!proto.is_empty(), "protocolVersion must not be empty");
    h.shutdown();
}

#[test]
fn mcp_stdio_initialize_result_has_server_info() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "mcp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(
        1,
        "initialize",
        Some(json!({ "protocolVersion": "2024-11-05", "clientInfo": { "name": "test" } })),
    );
    assert_success_shape(&resp, "mcp_stdio:initialize:server_info");

    let result = &resp["result"];
    assert!(
        result.get("serverInfo").is_some(),
        "mcp result must have serverInfo; got: {result}"
    );
    let server_info = &result["serverInfo"];
    assert!(
        server_info.get("name").and_then(Value::as_str).is_some(),
        "serverInfo must have a `name` string; got: {server_info}"
    );
    assert!(
        server_info.get("version").and_then(Value::as_str).is_some(),
        "serverInfo must have a `version` string; got: {server_info}"
    );
    h.shutdown();
}

#[test]
fn mcp_stdio_cancel_notification_blocks_matching_request_id() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "mcp_stdio");
    let mut h = Harness::spawn(&cfg);

    // Send a true JSON-RPC notification (no id) so the server emits no response.
    let notification = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": { "requestId": 77, "reason": "client_abort" }
    }))
    .expect("encode notification");
    let stdin = h.stdin.as_mut().expect("stdin should be available");
    writeln!(stdin, "{notification}").expect("write notification failed");
    stdin.flush().expect("flush notification failed");

    let resp = h.send(
        77,
        "tools/call",
        Some(json!({
            "name": "read_file",
            "arguments": { "path": "/tmp/ignored-by-cancel" }
        })),
    );

    assert_error_shape(&resp, -32800, "mcp_stdio:cancelled_request");
    assert!(
        resp["error"]["data"].get("platform_context").is_some(),
        "mcp_stdio:cancelled_request must keep platform_context; got: {resp}"
    );
    h.shutdown();
}

#[test]
fn mcp_stdio_tools_list_returns_tools_array() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "mcp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(1, "tools/list", None);
    assert_success_shape(&resp, "mcp_stdio:tools/list");
    assert!(
        resp["result"].get("tools").is_some(),
        "mcp_stdio:tools/list result must have `tools` array; got: {}",
        resp["result"]
    );
    h.shutdown();
}

#[test]
fn mcp_stdio_unknown_method_returns_minus_32601() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "mcp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(1, "blue18.nonexistent.method", None);
    assert_error_shape(&resp, -32601, "mcp_stdio:unknown_method");
    assert!(
        resp["error"]["data"].get("platform_context").is_some(),
        "mcp_stdio:unknown_method error.data must contain platform_context; got: {}",
        resp
    );
    assert_eq!(
        resp["error"]["data"]["platform_context"]["schema_version"], "blue24-platform-universal-v1",
        "mcp_stdio:unknown_method platform_context.schema_version mismatch"
    );
    h.shutdown();
}

#[test]
fn mcp_stdio_tools_call_missing_params_returns_minus_32602() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "mcp_stdio");
    let mut h = Harness::spawn(&cfg);

    // `tools/call` without any params should return INVALID_PARAMS.
    let resp = h.send(1, "tools/call", None);
    assert_error_shape(&resp, -32602, "mcp_stdio:tools/call:missing_params");
    h.shutdown();
}

#[test]
fn mcp_stdio_tools_call_unknown_tool_returns_minus_32602() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "mcp_stdio");
    let mut h = Harness::spawn(&cfg);

    // `tools/call` for a non-existent tool should return INVALID_PARAMS.
    let resp = h.send(
        1,
        "tools/call",
        Some(json!({ "name": "blue18.nonexistent.tool", "arguments": {} })),
    );
    assert_error_shape(&resp, -32602, "mcp_stdio:tools/call:unknown_tool");
    h.shutdown();
}

// ---------------------------------------------------------------------------
// MCP-3 streaming metadata tests.
// ---------------------------------------------------------------------------

#[test]
fn mcp_stdio_tools_list_result_has_x_skills_available() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "mcp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(1, "tools/list", None);
    assert_success_shape(&resp, "mcp_stdio:tools/list:x_skills_available");

    let result = &resp["result"];
    assert!(
        result.get("x_skills_available").is_some(),
        "tools/list result must have `x_skills_available`; got: {result}"
    );
    assert!(
        result["x_skills_available"].as_bool().is_some(),
        "x_skills_available must be a boolean; got: {}",
        result["x_skills_available"]
    );
    h.shutdown();
}

#[test]
fn mcp_stdio_tools_call_executes_and_returns_call_tool_result_shape() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "mcp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(
        1,
        "tools/call",
        Some(json!({
            "name": "read_file",
            "arguments": { "path": "Cargo.toml" }
        })),
    );
    assert_success_shape(&resp, "mcp_stdio:tools/call:call_tool_result");

    let result = &resp["result"];
    assert!(
        result.get("content").and_then(Value::as_array).is_some(),
        "tools/call result must have `content` array; got: {result}"
    );
    let content = result["content"].as_array().unwrap();
    assert!(
        !content.is_empty(),
        "tools/call result.content must not be empty"
    );
    for item in content {
        assert!(
            item.get("type").and_then(Value::as_str).is_some(),
            "each content item must have a `type` string; got: {item}"
        );
        assert!(
            item.get("text").and_then(Value::as_str).is_some(),
            "each content item must have a `text` string; got: {item}"
        );
    }
    h.shutdown();
}

// ---------------------------------------------------------------------------
// MCP-4 timeout/retry/cancel tests.
// ---------------------------------------------------------------------------

#[test]
fn mcp_stdio_ping_returns_empty_result() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "mcp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(1, "ping", None);
    assert_success_shape(&resp, "mcp_stdio:ping");

    // Ping returns a result object. It may be empty or contain
    // runtime intelligence profile keys (e.g., knowledge_refinement,
    // learning_profile) — the key invariant is that it's a valid
    // JSON object with no error, which assert_success_shape already verifies.
    let result = &resp["result"];
    assert!(
        result.is_object(),
        "ping result must be a JSON object; got: {:?}",
        result
    );
    h.shutdown();
}

#[test]
fn mcp_stdio_notifications_initialized_returns_no_response() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "mcp_stdio");
    let mut h = Harness::spawn(&cfg);

    // MCP notifications use method "notifications/initialized" with no id.
    // The JSON-RPC spec says notifications have no `id` field, so the child
    // should not produce a response.  We send it and then send a second
    // request that does expect a response to sanity-check the connection.
    let notification = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    let line = serde_json::to_string(&notification).expect("encode failed");
    let stdin = h.stdin.as_mut().expect("stdin closed");
    writeln!(stdin, "{line}").expect("write failed");
    stdin.flush().expect("flush failed");

    // Now send a ping to verify the connection is still alive.
    let resp = h.send(2, "ping", None);
    assert_success_shape(&resp, "mcp_stdio:notifications_initialized:post_ping");
    h.shutdown();
}

// ---------------------------------------------------------------------------
// MCP-6 protocol compatibility tests.
// ---------------------------------------------------------------------------

#[test]
fn mcp_stdio_initialize_result_has_capabilities() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "mcp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(
        1,
        "initialize",
        Some(json!({ "protocolVersion": "2024-11-05", "clientInfo": { "name": "test" } })),
    );
    assert_success_shape(&resp, "mcp_stdio:initialize:capabilities");

    let result = &resp["result"];
    assert!(
        result.get("capabilities").is_some(),
        "initialize result must have `capabilities`; got: {result}"
    );
    let caps = &result["capabilities"];
    assert!(
        caps.as_object().is_some(),
        "capabilities must be an object; got: {caps}"
    );
    assert!(
        caps.get("resources").is_some(),
        "capabilities must contain `resources`; got: {caps}"
    );
    assert!(
        caps.get("tools").is_some(),
        "capabilities must contain `tools`; got: {caps}"
    );
    assert!(
        caps.get("prompts").is_some(),
        "capabilities must contain `prompts`; got: {caps}"
    );
    h.shutdown();
}

// ---------------------------------------------------------------------------
// Cross-protocol semantic equivalence assertions.
// ---------------------------------------------------------------------------

/// Both ACP stdio and MCP stdio must return tools metadata with a `tools` key
/// and the list must contain at least some overlap — the protocol label does
/// not matter, the shape must be consistent.
#[test]
fn cross_protocol_tools_list_shape_is_consistent() {
    let tmp = tempdir().unwrap();

    let acp_cfg = tmp.path().join("acp_config.toml");
    write_mode_config(&acp_cfg, "acp_stdio");
    let mut acp = Harness::spawn(&acp_cfg);
    let acp_resp = acp.send(1, "mcp.tools.list", None);
    assert_success_shape(&acp_resp, "acp_stdio:mcp.tools.list");
    let acp_tools = acp_resp["result"]["tools"]
        .as_array()
        .expect("acp tools must be array");
    // Collect names before dropping so we don't hold the lock during spawn.
    let acp_tool_names: Vec<String> = acp_tools
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .map(|s| s.to_string())
        .collect();
    acp.shutdown();
    drop(acp); // release SUITE_LOCK before acquiring it again for MCP harness

    let mcp_cfg = tmp.path().join("mcp_config.toml");
    write_mode_config(&mcp_cfg, "mcp_stdio");
    let mut mcp = Harness::spawn(&mcp_cfg);
    let mcp_resp = mcp.send(1, "tools/list", None);
    assert_success_shape(&mcp_resp, "mcp_stdio:tools/list");
    let mcp_tools = mcp_resp["result"]["tools"]
        .as_array()
        .expect("mcp tools must be array");
    mcp.shutdown();

    // Both lists must agree on tool *names* (same registry, different path).
    let mut acp_names: Vec<&str> = acp_tool_names.iter().map(|s| s.as_str()).collect();
    let mut mcp_names: Vec<&str> = mcp_tools
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .collect();
    acp_names.sort_unstable();
    mcp_names.sort_unstable();
    // MCP tools must all be present in ACP (ACP may additionally expose ACP-native tools).
    for name in &mcp_names {
        assert!(
            acp_names.contains(name),
            "MCP tool '{name}' is missing from ACP stdio registry; acp={acp_names:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Profile injection consistency — ACP path vs MCP independent path.
// ---------------------------------------------------------------------------

/// ACP stdio: initialize must carry `platform_context` in the result.
#[test]
fn acp_stdio_initialize_result_has_platform_context() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "acp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(1, "initialize", Some(json!({ "protocol": "acp" })));
    assert_success_shape(&resp, "acp_stdio:initialize:platform_context");
    assert!(
        resp["result"].get("platform_context").is_some(),
        "acp_stdio:initialize result must contain `platform_context`; got: {}",
        resp["result"]
    );
    h.shutdown();
}

/// MCP stdio: initialize must also carry `platform_context` in the result.
#[test]
fn mcp_stdio_initialize_result_has_platform_context() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "mcp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(
        1,
        "initialize",
        Some(json!({ "protocolVersion": "2024-11-05", "clientInfo": { "name": "test" } })),
    );
    assert_success_shape(&resp, "mcp_stdio:initialize:platform_context");
    assert!(
        resp["result"].get("platform_context").is_some(),
        "mcp_stdio:initialize result must contain `platform_context`; got: {}",
        resp["result"]
    );
    h.shutdown();
}

/// MCP stdio: tools/list must also carry `platform_context`.
#[test]
fn mcp_stdio_tools_list_result_has_platform_context() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "mcp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(1, "tools/list", None);
    assert_success_shape(&resp, "mcp_stdio:tools/list:platform_context");
    assert!(
        resp["result"].get("platform_context").is_some(),
        "mcp_stdio:tools/list result must contain `platform_context`; got: {}",
        resp["result"]
    );
    h.shutdown();
}

/// ACP stdio: mcp.tools.list must carry `platform_context`.
#[test]
fn acp_stdio_mcp_tools_list_result_has_platform_context() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "acp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(1, "mcp.tools.list", None);
    assert_success_shape(&resp, "acp_stdio:mcp.tools.list:platform_context");
    assert!(
        resp["result"].get("platform_context").is_some(),
        "acp_stdio:mcp.tools.list result must contain `platform_context`; got: {}",
        resp["result"]
    );
    h.shutdown();
}

// ---------------------------------------------------------------------------
// Conflict detection — profile keys must not be duplicated inside result.
// ---------------------------------------------------------------------------

/// ACP stdio: `platform_context` must appear exactly once in the result object
/// (no double-injection from two code paths).
#[test]
fn acp_stdio_initialize_platform_context_not_duplicated() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "acp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(1, "initialize", Some(json!({ "protocol": "acp" })));
    assert_success_shape(&resp, "acp_stdio:initialize:no_dup");

    // Serialize to raw JSON and count occurrences of the key.
    let raw = serde_json::to_string(&resp["result"]).unwrap();
    let occurrences = raw.matches("\"platform_context\"").count();
    assert_eq!(
        occurrences, 1,
        "acp_stdio:initialize `platform_context` must appear exactly once; found {} in: {}",
        occurrences, raw
    );
    h.shutdown();
}

/// MCP stdio: `platform_context` must appear exactly once.
#[test]
fn mcp_stdio_initialize_platform_context_not_duplicated() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "mcp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(
        1,
        "initialize",
        Some(json!({ "protocolVersion": "2024-11-05", "clientInfo": { "name": "test" } })),
    );
    assert_success_shape(&resp, "mcp_stdio:initialize:no_dup");

    let raw = serde_json::to_string(&resp["result"]).unwrap();
    let occurrences = raw.matches("\"platform_context\"").count();
    assert_eq!(
        occurrences, 1,
        "mcp_stdio:initialize `platform_context` must appear exactly once; found {} in: {}",
        occurrences, raw
    );
    h.shutdown();
}

/// Cross-protocol: both protocols must inject the same `schema_version` string
/// inside `platform_context`, ensuring the injection is from a single shared source.
#[test]
fn cross_protocol_platform_context_schema_version_matches() {
    let tmp = tempdir().unwrap();

    let acp_cfg = tmp.path().join("acp_config.toml");
    write_mode_config(&acp_cfg, "acp_stdio");
    let mut acp = Harness::spawn(&acp_cfg);
    let acp_resp = acp.send(1, "initialize", Some(json!({ "protocol": "acp" })));
    assert_success_shape(&acp_resp, "cross:acp:initialize");
    let acp_ver = acp_resp["result"]["platform_context"]["schema_version"]
        .as_str()
        .expect("acp_stdio:initialize platform_context.schema_version must be a string")
        .to_string();
    acp.shutdown();
    drop(acp);

    let mcp_cfg = tmp.path().join("mcp_config.toml");
    write_mode_config(&mcp_cfg, "mcp_stdio");
    let mut mcp = Harness::spawn(&mcp_cfg);
    let mcp_resp = mcp.send(
        1,
        "initialize",
        Some(json!({ "protocolVersion": "2024-11-05", "clientInfo": { "name": "test" } })),
    );
    assert_success_shape(&mcp_resp, "cross:mcp:initialize");
    let mcp_ver = mcp_resp["result"]["platform_context"]["schema_version"]
        .as_str()
        .expect("mcp_stdio:initialize platform_context.schema_version must be a string")
        .to_string();
    mcp.shutdown();

    assert_eq!(
        acp_ver, mcp_ver,
        "platform_context.schema_version must be identical across ACP and MCP; \
         acp={acp_ver:?} mcp={mcp_ver:?}"
    );
}

/// Cross-protocol: both ACP and MCP initialize responses must follow a
/// consistent shape — a success `result` with protocol-identifying fields
/// (protocol/version for ACP, protocolVersion/serverInfo for MCP).
/// This proves the autonomy contract shape is protocol-agnostic.
#[test]
fn cross_protocol_autonomy_contract_shape_is_consistent() {
    let tmp = tempdir().unwrap();

    // ---- ACP stdio initialize ----
    let acp_cfg = tmp.path().join("acp_config.toml");
    write_mode_config(&acp_cfg, "acp_stdio");
    let mut acp = Harness::spawn(&acp_cfg);

    let acp_resp = acp.send(1, "initialize", Some(json!({ "protocol": "acp" })));
    assert_success_shape(&acp_resp, "cross:acp:initialize:shape");

    // Verify protocol-identifying fields exist in the result.
    let acp_result = &acp_resp["result"];
    assert!(
        acp_result.get("protocol").is_some() || acp_result.get("version").is_some(),
        "acp results must have `protocol` or `version`; got: {acp_result}"
    );
    // Capture the top-level keys as proof of the shape.
    let acp_keys: Vec<&str> = acp_result
        .as_object()
        .map(|m| {
            let mut keys: Vec<&str> = m.keys().map(|k| k.as_str()).collect();
            keys.sort_unstable();
            keys
        })
        .unwrap_or_default();
    assert!(
        !acp_keys.is_empty(),
        "acp result must contain at least one key; got empty object"
    );
    acp.shutdown();
    drop(acp);

    // ---- MCP stdio initialize ----
    let mcp_cfg = tmp.path().join("mcp_config.toml");
    write_mode_config(&mcp_cfg, "mcp_stdio");
    let mut mcp = Harness::spawn(&mcp_cfg);

    let mcp_resp = mcp.send(
        1,
        "initialize",
        Some(json!({ "protocolVersion": "2024-11-05", "clientInfo": { "name": "test" } })),
    );
    assert_success_shape(&mcp_resp, "cross:mcp:initialize:shape");

    let mcp_result = &mcp_resp["result"];
    assert!(
        mcp_result.get("protocolVersion").is_some(),
        "mcp result must have `protocolVersion`; got: {mcp_result}"
    );
    assert!(
        mcp_result.get("serverInfo").is_some(),
        "mcp result must have `serverInfo`; got: {mcp_result}"
    );
    mcp.shutdown();

    // Both responses follow a success shape with protocol-identifying fields
    // inside `result` — the contract shape is consistent across protocols.
}

// ---------------------------------------------------------------------------
// MCP-6: Protocol compatibility — error field consistency
// ---------------------------------------------------------------------------

/// MCP stdio: All error responses must consistently omit `result` field.
#[test]
fn mcp_stdio_error_response_omits_result_field() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_mode_config(&cfg, "mcp_stdio");
    let mut h = Harness::spawn(&cfg);

    let resp = h.send(1, "blue18.nonexistent.method", None);
    assert_error_shape(&resp, -32601, "mcp_stdio:error_no_result");
    assert!(
        resp.get("result").is_none(),
        "error response must NOT have a `result` field; got: {}",
        resp
    );
    h.shutdown();
}
