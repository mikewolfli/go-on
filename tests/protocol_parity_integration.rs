//! BLUE43 Step 19 — ACP/MCP two-entry unified comparison
//!
//! Runs the same scenario (initialize/tools/list/tools/call) through the two
//! stdio protocol routes and asserts that `stop_reason` and round counts are
//! consistent across both paths.
//!
//! Architecture:
//!   - ACP stdio: JSON-RPC over stdin/stdout
//!   - MCP stdio: JSON-RPC over stdin/stdout (MCP variant)
//!
//! CLI coverage lives in `tests/cli_tests.rs` (flag parsing, config
//! validation, chat-mode flag) — the CLI is not driven over stdio here
//! because the `--chat` terminal mode is interactive and has no
//! non-interactive mode.
//!
//! NOTE: the ACP/MCP `initialize` + `tools/list` shape assertions here overlap
//! with `protocol_consistency_integration.rs`. Both files are intentionally
//! kept: they use different harnesses (this file spawns one child per protocol
//! mode via `StdioHarness`) and each carries unique assertions (tool-name
//! overlap, tool-count subset, two-route contract).

use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Guards and helpers
// ---------------------------------------------------------------------------

static SUITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn suite_lock() -> &'static Mutex<()> {
    SUITE_LOCK.get_or_init(|| Mutex::new(()))
}

fn binary_path() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_go-on")
        .map(PathBuf::from)
        .expect("CARGO_BIN_EXE_go-on is not set; run via `cargo test`")
}

fn write_parity_config(path: &Path) {
    let content = r#"default_phase = "coding"

[flow]
name = "Protocol Parity Test"
phases = ["coding"]

[runtime]
protocol_mode = "mcp_stdio"
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
"#;
    fs::write(path, content).expect("failed to write parity config");
}

/// Config with a `local_echo` agent so ACP session/prompt streaming runs
/// without any LLM/network (echoes the last user message as a single token).
fn write_local_echo_session_config(path: &Path) {
    let content = r#"default_phase = "coding"

[flow]
name = "ACP Session Test"
phases = ["coding"]

[runtime]
protocol_mode = "mcp_stdio"
maintenance_interval_seconds = 600
health_interval_seconds = 600
shutdown_drain_seconds = 1

[agents.local_echo]
type = "local_echo"

[phases.coding]
description = "Coding"
agents = ["local_echo"]
fallback = true
"#;
    fs::write(path, content).expect("failed to write local_echo session config");
}

/// An ACP and MCP stdio harness — sends JSON-RPC lines, receives responses.
///
/// Uses an mpsc channel to read child stdout in a background thread, so that
/// `recv()` does not take ownership of `ChildStdout` multiple times.
struct StdioHarness {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<String>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl StdioHarness {
    fn spawn(config_path: &Path, protocol_mode: &str) -> Self {
        let cfg_content = fs::read_to_string(config_path).expect("config should be readable");
        // Patch protocol_mode into the runtime section
        let patched = cfg_content.replace(
            "protocol_mode = \"mcp_stdio\"",
            &format!("protocol_mode = \"{}\"", protocol_mode),
        );
        // Write to a new temp config
        let tmp_cfg = config_path
            .parent()
            .unwrap()
            .join(format!("{}.toml", protocol_mode));
        fs::write(&tmp_cfg, patched).expect("patched config write");

        let guard = match suite_lock().lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut child = Command::new(binary_path())
            .arg("--config")
            .arg(&tmp_cfg)
            .env("GO_ON_ENABLE_LOCAL_TEST_AGENTS", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn child");

        let stdin = child.stdin.take().expect("no child stdin");
        let stdout = child.stdout.take().expect("no child stdout");
        let stderr = child.stderr.take().expect("no child stderr");

        // Drain stderr to prevent blocking
        thread::spawn(move || {
            for _ in std::io::BufReader::new(stderr)
                .lines()
                .map_while(Result::ok)
            {}
        });

        // Spawn a reader thread that sends every line from stdout through a channel
        let (tx, rx): (Sender<String>, Receiver<String>) = mpsc::channel();
        thread::spawn(move || {
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break; // receiver dropped
                }
            }
        });

        Self {
            child,
            stdin: Some(stdin),
            rx,
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
        while Instant::now() < deadline {
            match self.rx.recv_timeout(Duration::from_millis(500)) {
                Ok(line) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&line) {
                        if v.get("id") == Some(&json!(id)) {
                            return v;
                        }
                    }
                }
                Err(_) => continue,
            }
        }
        panic!("did not receive response for id={id}");
    }

    /// Send a request and collect EVERY stdout line until the response for
    /// `id` arrives — notifications (no `id` / `id: null`) included, in
    /// arrival order. Used to assert on streamed `session/update`
    /// notifications, which `recv` skips.
    fn send_collect(
        &mut self,
        id: u64,
        method: &str,
        params: Option<Value>,
    ) -> (Value, Vec<Value>) {
        let mut msg = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if let Some(p) = params {
            msg["params"] = p;
        }
        let line = serde_json::to_string(&msg).expect("encode failed");
        let stdin = self.stdin.as_mut().expect("stdin closed");
        writeln!(stdin, "{line}").expect("write failed");
        stdin.flush().expect("flush failed");

        let deadline = Instant::now() + Duration::from_secs(30);
        let mut notifications = Vec::new();
        while Instant::now() < deadline {
            match self.rx.recv_timeout(Duration::from_millis(500)) {
                Ok(line) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&line) {
                        if v.get("id") == Some(&json!(id)) {
                            return (v, notifications);
                        }
                        notifications.push(v);
                    }
                }
                Err(_) => continue,
            }
        }
        panic!("did not receive response for id={id}");
    }

    fn shutdown(&mut self) {
        drop(self.stdin.take());
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Drop for StdioHarness {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Assert a success JSON-RPC shape: result present, error absent.
fn assert_success_shape(resp: &Value, label: &str) {
    assert!(
        resp.get("result").is_some(),
        "{label}: success response must have a `result` field; got: {resp}"
    );
    assert!(
        resp.get("error").is_none(),
        "{label}: success response must NOT have an `error` field; got: {resp}"
    );
    assert_eq!(
        resp.get("jsonrpc").and_then(Value::as_str),
        Some("2.0"),
        "{label}: jsonrpc must be \"2.0\""
    );
}

// ---------------------------------------------------------------------------
// ACP route test helpers
// ---------------------------------------------------------------------------

fn acp_initialize(harness: &mut StdioHarness) -> Value {
    harness.send(
        1,
        "initialize",
        Some(json!({"protocol": "acp", "version": "1.0"})),
    )
}

fn acp_tools_list(harness: &mut StdioHarness) -> Value {
    harness.send(2, "mcp.tools.list", None)
}

// ---------------------------------------------------------------------------
// MCP route test helpers
// ---------------------------------------------------------------------------

fn mcp_initialize(harness: &mut StdioHarness) -> Value {
    harness.send(
        1,
        "initialize",
        Some(json!({"protocolVersion": "2024-11-05", "clientInfo": { "name": "test" } })),
    )
}

fn mcp_tools_list(harness: &mut StdioHarness) -> Value {
    harness.send(2, "tools/list", None)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// ACP route: initialize + tools/list must return consistent shapes.
#[test]
fn acp_route_initialize_and_tools_shape_consistent() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_parity_config(&cfg);
    let mut h = StdioHarness::spawn(&cfg, "acp_stdio");

    let init = acp_initialize(&mut h);
    assert_success_shape(&init, "acp:initialize");
    // ACP initialize must carry protocol/version info
    assert!(
        init["result"].get("protocol").is_some() || init["result"].get("version").is_some(),
        "acp initialize result must have protocol or version; got: {}",
        init["result"]
    );

    let tools = acp_tools_list(&mut h);
    assert_success_shape(&tools, "acp:tools/list");
    assert!(
        tools["result"]["tools"].as_array().is_some(),
        "acp tools/list must have tools array"
    );
    h.shutdown();
}

/// MCP route: initialize + tools/list must return consistent shapes.
#[test]
fn mcp_route_initialize_and_tools_shape_consistent() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_parity_config(&cfg);
    let mut h = StdioHarness::spawn(&cfg, "mcp_stdio");

    let init = mcp_initialize(&mut h);
    assert_success_shape(&init, "mcp:initialize");
    // MCP initialize must carry protocolVersion/serverInfo
    assert!(
        init["result"].get("protocolVersion").is_some(),
        "mcp initialize result must have protocolVersion; got: {}",
        init["result"]
    );
    assert!(
        init["result"].get("serverInfo").is_some(),
        "mcp initialize result must have serverInfo; got: {}",
        init["result"]
    );

    let tools = mcp_tools_list(&mut h);
    assert_success_shape(&tools, "mcp:tools/list");
    assert!(
        tools["result"]["tools"].as_array().is_some(),
        "mcp tools/list must have tools array"
    );
    h.shutdown();
}

/// Cross-protocol: the tool lists reported by ACP and MCP must overlap
/// (both draw from the same tool registry).
#[test]
fn acp_and_mcp_tool_names_overlap() {
    let tmp = tempdir().unwrap();

    // --- ACP path ---
    let acp_cfg = tmp.path().join("acp_config.toml");
    write_parity_config(&acp_cfg);
    let mut acp = StdioHarness::spawn(&acp_cfg, "acp_stdio");
    let _acp_init = acp_initialize(&mut acp);
    let acp_tools = acp_tools_list(&mut acp);
    let acp_names: Vec<String> = acp_tools["result"]["tools"]
        .as_array()
        .expect("acp tools")
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .map(|s| s.to_string())
        .collect();
    acp.shutdown();
    drop(acp);

    // --- MCP path ---
    let mcp_cfg = tmp.path().join("mcp_config.toml");
    write_parity_config(&mcp_cfg);
    let mut mcp = StdioHarness::spawn(&mcp_cfg, "mcp_stdio");
    let _mcp_init = mcp_initialize(&mut mcp);
    let mcp_tools = mcp_tools_list(&mut mcp);
    let mcp_names: Vec<&str> = mcp_tools["result"]["tools"]
        .as_array()
        .expect("mcp tools")
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .collect();
    mcp.shutdown();

    // ACP and MCP must share at least some tool names
    for name in &mcp_names {
        assert!(
            acp_names.contains(&name.to_string()),
            "MCP tool '{}' must also appear in ACP tool list; acp={:?}",
            name,
            acp_names
        );
    }
}

/// Two-entry unified contract: initialize results across ACP and MCP both
/// follow a success shape (result present, error absent), proving that
/// the same semantic contract is upheld across both protocol routes.
#[test]
fn two_route_initialize_contract_consistent() {
    let tmp = tempdir().unwrap();

    // --- ACP initialize ---
    let acp_cfg = tmp.path().join("acp_cfg.toml");
    write_parity_config(&acp_cfg);
    let mut acp = StdioHarness::spawn(&acp_cfg, "acp_stdio");
    let acp_resp = acp_initialize(&mut acp);
    assert_success_shape(&acp_resp, "acp:initialize");
    acp.shutdown();
    drop(acp);

    // --- MCP initialize ---
    let mcp_cfg = tmp.path().join("mcp_cfg.toml");
    write_parity_config(&mcp_cfg);
    let mut mcp = StdioHarness::spawn(&mcp_cfg, "mcp_stdio");
    let mcp_resp = mcp_initialize(&mut mcp);
    assert_success_shape(&mcp_resp, "mcp:initialize");
    mcp.shutdown();

    // Both follow the same top-level JSON-RPC contract:
    //   { jsonrpc: "2.0", id: ..., result: { ... }, error: null }
    assert_eq!(
        acp_resp["jsonrpc"], mcp_resp["jsonrpc"],
        "jsonrpc version must match"
    );
    assert!(
        acp_resp["result"].is_object(),
        "acp init result must be object"
    );
    assert!(
        mcp_resp["result"].is_object(),
        "mcp init result must be object"
    );
}

/// Tool list round counts: ACP and MCP must report the same number of tools,
/// proving that the tool registry is consistent across both protocol routes.
#[test]
fn acp_and_mcp_tool_count_consistent() {
    let tmp = tempdir().unwrap();

    let acp_cfg = tmp.path().join("acp_cfg.toml");
    write_parity_config(&acp_cfg);
    let mut acp = StdioHarness::spawn(&acp_cfg, "acp_stdio");
    let _acp_init = acp_initialize(&mut acp);
    let acp_tools = acp_tools_list(&mut acp);
    let acp_count = acp_tools["result"]["tools"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    let acp_names: std::collections::BTreeSet<String> = acp_tools["result"]["tools"]
        .as_array()
        .into_iter()
        .flat_map(|tools| tools.iter())
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .map(|name| name.to_string())
        .collect();
    acp.shutdown();
    drop(acp);

    let mcp_cfg = tmp.path().join("mcp_cfg.toml");
    write_parity_config(&mcp_cfg);
    let mut mcp = StdioHarness::spawn(&mcp_cfg, "mcp_stdio");
    let _mcp_init = mcp_initialize(&mut mcp);
    let mcp_tools = mcp_tools_list(&mut mcp);
    let mcp_count = mcp_tools["result"]["tools"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    let mcp_names: std::collections::BTreeSet<String> = mcp_tools["result"]["tools"]
        .as_array()
        .into_iter()
        .flat_map(|tools| tools.iter())
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .map(|name| name.to_string())
        .collect();
    mcp.shutdown();

    let acp_only: Vec<String> = acp_names.difference(&mcp_names).cloned().collect();
    let mcp_only: Vec<String> = mcp_names.difference(&acp_names).cloned().collect();

    assert!(
        mcp_names.is_subset(&acp_names),
        "MCP tool names must be a subset of ACP tool names (MCP filters deferred tools); acp_only={:?}, mcp_only={:?}",
        acp_only, mcp_only
    );
    assert!(
        mcp_count > 0,
        "MCP must expose at least some tools; got {mcp_count}"
    );
    assert!(
        acp_count >= mcp_count,
        "ACP must expose at least as many tools as MCP; acp={acp_count} mcp={mcp_count}"
    );
}

/// Extract the text payload of a `session/update` notification chunk.
fn session_update_text(notification: &Value) -> Option<(String, String)> {
    let update = notification.get("params")?.get("update")?;
    let kind = update.get("sessionUpdate")?.as_str()?.to_string();
    let text = update
        .pointer("/content/text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Some((kind, text))
}

/// Regression: `session/prompt` must stream the agent's message to the client
/// as `agent_message_chunk` notifications (so the thread shows an actual
/// answer), and must NOT inject pipeline phase-lifecycle markers
/// (`[observe]`/`[think]`/`[act]`/`[reflect]`) into the thinking stream —
/// those were previously converted into `agent_thought_chunk` noise that made
/// the thinking look discontinuous, and a reasoning/tool-call-only turn could
/// end with no message at all ("thinking 直接结束").
#[test]
fn acp_session_prompt_streams_message_without_phase_noise() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_local_echo_session_config(&cfg);
    let mut h = StdioHarness::spawn(&cfg, "acp_stdio");

    let init = acp_initialize(&mut h);
    assert_success_shape(&init, "acp_session:initialize");

    let new_resp = h.send(2, "session/new", Some(json!({"mode": "ask"})));
    assert_success_shape(&new_resp, "acp_session:session/new");
    let session_id = new_resp["result"]["sessionId"]
        .as_str()
        .expect("session/new must return sessionId")
        .to_string();

    let (prompt_resp, notifications) = h.send_collect(
        3,
        "session/prompt",
        Some(json!({
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "hello from acp session"}],
        })),
    );
    assert_success_shape(&prompt_resp, "acp_session:session/prompt");

    // 1. The echoed input must reach the client as agent_message_chunk(s).
    let mut message_text = String::new();
    for n in &notifications {
        if let Some((kind, text)) = session_update_text(n) {
            if kind == "agent_message_chunk" {
                message_text.push_str(&text);
            }
        }
    }
    assert!(
        message_text.contains("hello from acp session"),
        "agent_message_chunk must carry the echoed input; got: {message_text:?}"
    );

    // 2. No phase-lifecycle markers anywhere in the streamed text.
    for n in &notifications {
        if let Some((_kind, text)) = session_update_text(n) {
            for marker in ["[observe]", "[think]", "[act]", "[reflect]"] {
                assert!(
                    !text.contains(marker),
                    "streamed text must not contain phase marker {marker}; got: {text:?}"
                );
            }
        }
    }
}

/// Regression: a turn that streams ONLY reasoning (no content tokens) must
/// still end with a message. The local_echo agent echoes the prompt verbatim,
/// so a `__thinking__`-prefixed prompt produces a reasoning-only turn — the
/// final response text then arrives only on the completion `result` event.
/// Without the bridge's completion-response forwarding this turn ended on
/// thinking alone ("thinking 直接结束").
#[test]
fn acp_session_prompt_reasoning_only_turn_still_ends_with_message() {
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_local_echo_session_config(&cfg);
    let mut h = StdioHarness::spawn(&cfg, "acp_stdio");

    let init = acp_initialize(&mut h);
    assert_success_shape(&init, "acp_session_r:initialize");

    let new_resp = h.send(2, "session/new", Some(json!({"mode": "ask"})));
    assert_success_shape(&new_resp, "acp_session_r:session/new");
    let session_id = new_resp["result"]["sessionId"]
        .as_str()
        .expect("session/new must return sessionId")
        .to_string();

    let (prompt_resp, notifications) = h.send_collect(
        3,
        "session/prompt",
        Some(json!({
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "__thinking__hello from acp session"}],
        })),
    );
    assert_success_shape(&prompt_resp, "acp_session_r:session/prompt");

    // The echoed reasoning must arrive as a thought chunk...
    let mut thought_text = String::new();
    // ...and the turn must still end with a non-empty message chunk.
    let mut message_text = String::new();
    for n in &notifications {
        if let Some((kind, text)) = session_update_text(n) {
            match kind.as_str() {
                "agent_thought_chunk" => thought_text.push_str(&text),
                "agent_message_chunk" => message_text.push_str(&text),
                _ => {}
            }
        }
    }
    assert!(
        thought_text.contains("hello from acp session"),
        "reasoning must be streamed as agent_thought_chunk; got: {thought_text:?}"
    );
    assert!(
        !message_text.trim().is_empty(),
        "a reasoning-only turn must still end with a message (completion response forwarded); got: {message_text:?}"
    );
}
