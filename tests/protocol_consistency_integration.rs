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
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Harness — reused across both protocol modes.
// ---------------------------------------------------------------------------

struct Harness {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout_rx: Receiver<Value>,
    _guard: MutexGuard<'static, ()>,
}

static SUITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn suite_lock() -> &'static Mutex<()> {
    SUITE_LOCK.get_or_init(|| Mutex::new(()))
}

fn binary_path() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_go-on")
        .map(PathBuf::from)
        .expect("CARGO_BIN_EXE_go-on is not set; run via `cargo test`")
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
        thread::spawn(move || {
            for _ in BufReader::new(stderr).lines().map_while(Result::ok) {}
        });

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
    let err = resp
        .get("error")
        .unwrap_or_else(|| panic!("{context}: error response must have an `error` field; got: {resp}"));
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

    let resp = h.send(1, "initialize", Some(json!({ "protocolVersion": "2024-11-05", "clientInfo": { "name": "test" } })));
    assert_success_shape(&resp, "mcp_stdio:initialize");

    let result = &resp["result"];
    let proto = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("mcp_stdio:initialize result must contain protocolVersion; got: {result}"));
    assert!(!proto.is_empty(), "protocolVersion must not be empty");
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
    let resp = h.send(1, "tools/call", Some(json!({ "name": "blue18.nonexistent.tool", "arguments": {} })));
    assert_error_shape(&resp, -32602, "mcp_stdio:tools/call:unknown_tool");
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
    let mut acp_names: Vec<&str> = acp_tool_names
        .iter()
        .map(|s| s.as_str())
        .collect();
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
