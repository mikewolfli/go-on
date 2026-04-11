use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::tempdir;

struct RpcHarness {
    child: Child,
    // Option so we can explicitly drop (close) stdin before wait_for_exit to
    // prevent the write-side pipe race: the child blocks on its stdin reader
    // until EOF, which only arrives once the write end is closed.
    stdin: Option<ChildStdin>,
    stdout_rx: Receiver<Value>,
    stderr_lines: Arc<Mutex<Vec<String>>>,
    // Serialize this integration suite to avoid flaky child-process pipe races.
    _suite_guard: MutexGuard<'static, ()>,
}

static RPC_SUITE_GUARD: OnceLock<Mutex<()>> = OnceLock::new();

fn suite_guard() -> &'static Mutex<()> {
    RPC_SUITE_GUARD.get_or_init(|| Mutex::new(()))
}

impl RpcHarness {
    fn spawn(config_path: &Path) -> Self {
        let suite_guard = match suite_guard().lock() {
            Ok(guard) => guard,
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

        let stdin = child.stdin.take().expect("failed to capture child stdin");
        let stdout = child.stdout.take().expect("failed to capture child stdout");
        let stderr = child.stderr.take().expect("failed to capture child stderr");
        let stderr_lines = Arc::new(Mutex::new(Vec::new()));

        let (stdout_tx, stdout_rx) = mpsc::channel();

        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else {
                    break;
                };
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    let _ = stdout_tx.send(value);
                }
            }
        });

        // Drain stderr so a verbose process cannot block on a full stderr buffer.
        let stderr_lines_clone = Arc::clone(&stderr_lines);
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                let Ok(line) = line else {
                    break;
                };
                if let Ok(mut guard) = stderr_lines_clone.lock() {
                    guard.push(line);
                    if guard.len() > 200 {
                        let overflow = guard.len() - 200;
                        guard.drain(0..overflow);
                    }
                }
            }
        });

        Self {
            child,
            stdin: Some(stdin),
            stdout_rx,
            stderr_lines,
            _suite_guard: suite_guard,
        }
    }

    fn request(&mut self, id: u64, method: &str, params: Option<Value>) -> Value {
        let mut payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
        });

        if let Some(params) = params {
            payload["params"] = params;
        }

        let body = serde_json::to_string(&payload).expect("failed to encode request");
        let stdin = self.stdin.as_mut().expect("stdin already closed");
        writeln!(stdin, "{body}").expect("failed to write request to stdin");
        stdin.flush().expect("failed to flush request");

        self.read_response_for_id(id, Duration::from_secs(8))
    }

    fn raw_request(&mut self, payload: &Value) {
        let body = serde_json::to_string(payload).expect("failed to encode raw request");
        let stdin = self
            .stdin
            .as_mut()
            .expect("stdin already closed in raw_request");
        writeln!(stdin, "{body}").expect("failed to write raw request");
        stdin.flush().expect("failed to flush raw request");
    }

    /// Close the write end of the child's stdin pipe so the child sees EOF.
    /// Must be called after sending the final request (e.g. "shutdown") and
    /// before `wait_for_exit` to avoid a pipe-write race that causes flaky hangs.
    fn close_stdin(&mut self) {
        drop(self.stdin.take());
    }

    fn read_response_for_id(&mut self, id: u64, timeout: Duration) -> Value {
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                panic!("timed out waiting for response id {id}");
            }
            let remaining = deadline.saturating_duration_since(now);
            let msg = match self.stdout_rx.recv_timeout(remaining) {
                Ok(msg) => msg,
                Err(err) => {
                    let status = self.child.try_wait().ok().flatten();
                    let stderr_tail = self.stderr_tail(30);
                    panic!(
                        "stdout closed while waiting for response id {id}: {err}; child status: {:?}; stderr tail:\n{}",
                        status,
                        stderr_tail
                    );
                }
            };
            if msg.get("id") == Some(&json!(id)) {
                return msg;
            }
        }
    }

    fn wait_for_exit(&mut self, timeout: Duration) {
        // Close the write end of stdin so the child sees EOF and can exit cleanly.
        // Without this, the child's stdin-reader blocks and the process never terminates,
        // causing a timing race in the multi-process pipe harness.
        self.close_stdin();
        let deadline = Instant::now() + timeout;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    assert!(status.success(), "child exited with status: {status}");
                    return;
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        panic!("timed out waiting for child exit");
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                Err(err) => panic!(
                    "failed to wait for child: {err}; stderr tail:\n{}",
                    self.stderr_tail(30)
                ),
            }
        }
    }

    fn stderr_tail(&self, limit: usize) -> String {
        if let Ok(guard) = self.stderr_lines.lock() {
            let start = guard.len().saturating_sub(limit);
            let lines = guard[start..].to_vec();
            if lines.is_empty() {
                "<empty>".to_string()
            } else {
                lines.join("\n")
            }
        } else {
            "<stderr lock poisoned>".to_string()
        }
    }
}

impl Drop for RpcHarness {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn binary_path() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_go-on")
        .map(PathBuf::from)
        .expect("CARGO_BIN_EXE_go-on is not set")
}

fn write_test_config(path: &Path, maintenance: u64, health: u64, shutdown: u64) {
    let config = format!(
        r#"default_phase = "coding"

[flow]
name = "Test Flow"
phases = ["coding"]

[runtime]
maintenance_interval_seconds = {maintenance}
health_interval_seconds = {health}
shutdown_drain_seconds = {shutdown}

[agents.copilot]
type = "copilot"
url = "http://127.0.0.1:8080"

[phases.coding]
description = "Coding"
agents = ["copilot"]
fallback = true
"#
    );

    fs::write(path, config).expect("failed to write config file");
}

fn write_warning_config(path: &Path) {
    let config = r#"default_phase = "coding"

[flow]
name = "Warning Flow"
phases = ["coding", "review"]

[runtime]
maintenance_interval_seconds = 30
health_interval_seconds = 45
shutdown_drain_seconds = 7

[agents.copilot]
type = "copilot"
url = "http://127.0.0.1:8080"

[agents.reviewer_a]
type = "copilot"
url = "http://127.0.0.1:8080"

[agents.reviewer_b]
type = "copilot"
url = "http://127.0.0.1:8080"

[phases.coding]
description = "Coding"
agents = ["copilot"]
fallback = true

[phases.coding.options]
autopilot_complexity = "complex"
full_auto_review_agents = ["reviewer_a", "reviewer_b"]

[phases.review]
description = "Review"
agents = ["reviewer_a", "reviewer_b"]
fallback = false
"#;

    fs::write(path, config).expect("failed to write warning config file");
}

fn write_provider_failure_degrade_config(path: &Path) {
    let config = r#"default_phase = "coding"

[flow]
name = "Provider Failure Degrade"
phases = ["coding"]

[runtime]
maintenance_interval_seconds = 30
health_interval_seconds = 45
shutdown_drain_seconds = 7

[agents.bad_provider]
type = "copilot"
url = "http://127.0.0.1:65535"

[agents.local_fallback]
type = "local_echo"

[phases.coding]
description = "Coding"
agents = ["bad_provider", "local_fallback"]
fallback = true

[phases.coding.options]
request_timeout_seconds = 1
"#;

    fs::write(path, config).expect("failed to write provider failure config file");
}

fn write_review_timeout_collision_config(path: &Path) {
    let config = r#"default_phase = "coding"

[flow]
name = "Review Timeout Collision"
phases = ["coding", "review"]

[runtime]
maintenance_interval_seconds = 30
health_interval_seconds = 45
shutdown_drain_seconds = 7

[agents.main_agent]
type = "local_echo"

[agents.reviewer_fast]
type = "local_approve"

[agents.reviewer_slow]
type = "local_slow_approve"

[phases.coding]
description = "Coding"
agents = ["main_agent"]
fallback = true

[phases.coding.options]
autopilot_complexity = "complex"
full_auto_review_agents = ["reviewer_fast", "reviewer_slow"]
review_timeout_seconds = 1

[phases.coding.options.extra]
review_timeout_policy = "degrade_single"
min_reviewers = 2
required_approvals = 2
review_gate_timeout_seconds = 8

[phases.review]
description = "Review"
agents = ["reviewer_fast", "reviewer_slow"]
fallback = false
"#;

    fs::write(path, config).expect("failed to write review timeout collision config file");
}

fn write_cache_vector_unavailable_config(path: &Path, cache_path: &str, vector_path: &str) {
    let config = format!(
        r#"default_phase = "coding"

[flow]
name = "Cache Vector Unavailable"
phases = ["coding"]

[runtime]
maintenance_interval_seconds = 30
health_interval_seconds = 45
shutdown_drain_seconds = 7

[agents.copilot]
type = "copilot"
url = "http://127.0.0.1:8080"

[cache]
enabled = true
path = "{cache_path}"
default_ttl_seconds = 60
max_entries = 100

[vector]
enabled = true
path = "{vector_path}"
dimensions = 192
top_k = 2
min_similarity = 0.82
max_entries = 100
summary_enabled = true
summary_trigger_messages = 8
summary_max_chars = 1200

[phases.coding]
description = "Coding"
agents = ["copilot"]
fallback = true
"#,
        cache_path = cache_path,
        vector_path = vector_path,
    );

    fs::write(path, config).expect("failed to write cache/vector unavailable config file");
}

fn write_rate_limit_saturation_config(path: &Path) {
    let config = r#"default_phase = "coding"

[flow]
name = "Rate Limit Saturation"
phases = ["coding"]

[runtime]
maintenance_interval_seconds = 30
health_interval_seconds = 45
shutdown_drain_seconds = 7

[agents.local_echo]
type = "local_echo"

[phases.coding]
description = "Coding"
agents = ["local_echo"]
fallback = true

[phases.coding.options]
rate_limit_rpm = 1
rate_limit_burst = 1
"#;

    fs::write(path, config).expect("failed to write rate-limit saturation config file");
}

fn write_workflow_governance_config(path: &Path) {
    let config = r#"default_phase = "coding"

[flow]
name = "Workflow Governance"
phases = ["coding"]

[runtime]
maintenance_interval_seconds = 30
health_interval_seconds = 45
shutdown_drain_seconds = 7

[agents.local_echo]
type = "local_echo"

[phases.coding]
description = "Coding"
agents = ["local_echo"]
fallback = true

[phases.coding.options.extra]
review_min_level = "standard"
review_required_reviews = 1
review_timeout_policy = "reject"
review_required_checks = []
"#;

    fs::write(path, config).expect("failed to write workflow governance config file");
}

fn write_workflow_dual_review_config(path: &Path) {
    let config = r#"default_phase = "coding"

[flow]
name = "Workflow Dual Review"
phases = ["coding", "review"]

[runtime]
maintenance_interval_seconds = 30
health_interval_seconds = 45
shutdown_drain_seconds = 7

[agents.main_agent]
type = "local_echo"

[agents.reviewer_a]
type = "local_approve"

[agents.reviewer_b]
type = "local_approve"

[phases.coding]
description = "Coding"
agents = ["main_agent"]
fallback = true

[phases.coding.options]
review_timeout_seconds = 2

[phases.coding.options.extra]
review_min_level = "enhanced"
review_required_reviews = 2
review_timeout_policy = "reject"
min_reviewers = 2
required_approvals = 2

[phases.review]
description = "Review"
agents = ["reviewer_a", "reviewer_b"]
fallback = false
"#;

    fs::write(path, config).expect("failed to write workflow dual review config file");
}

fn write_autotune_enabled_config(path: &Path, state_path: &Path) {
    let escaped_state_path = state_path.display().to_string().replace('\\', "\\\\");
    let config = format!(
        r#"default_phase = "coding"

[flow]
name = "Autotune Enabled"
phases = ["coding"]

[runtime]
maintenance_interval_seconds = 30
health_interval_seconds = 45
shutdown_drain_seconds = 7

[agents.local_echo]
type = "local_echo"

[phases.coding]
description = "Coding"
agents = ["local_echo"]
fallback = true

[autotune]
enabled = true
state_path = "{state_path}"
"#,
        state_path = escaped_state_path,
    );

    fs::write(path, config).expect("failed to write autotune-enabled config file");
}

fn write_http_stream_config(path: &Path, bind_addr: &str) {
    let config = format!(
        r#"default_phase = "coding"

[flow]
name = "HTTP Stream"
phases = ["coding"]

[runtime]
maintenance_interval_seconds = 30
health_interval_seconds = 45
shutdown_drain_seconds = 7
acp_http_bind_addr = "{bind_addr}"

[agents.local_echo]
type = "local_echo"

[phases.coding]
description = "Coding"
agents = ["local_echo"]
fallback = true
"#,
        bind_addr = bind_addr,
    );

    fs::write(path, config).expect("failed to write HTTP stream config file");
}

fn find_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

fn http_request(
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> std::io::Result<String> {
    let mut stream = std::net::TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let body = body.unwrap_or("");
    let request = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        method,
        path,
        addr,
        body.len(),
        body,
    );
    stream.write_all(request.as_bytes())?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

#[test]
fn rpc_initialize_health_phase_and_shutdown() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);

    let mut harness = RpcHarness::spawn(&config_path);

    let initialize = harness.request(1, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");
    assert_eq!(initialize["result"]["protocol"], "acp");
    assert_eq!(initialize["result"]["capabilities"]["chat"], true);
    assert_eq!(initialize["result"]["capabilities"]["phase"], true);

    let health = harness.request(2, "runtime.health", None);
    assert_eq!(health["result"]["lifecycle"]["shutting_down"], false);
    assert!(health["result"]["maintenance"].is_object());
    assert_eq!(health["result"]["review_gate"]["total"], 0);
    assert_eq!(health["result"]["review_gate"]["timeout"], 0);
    assert_eq!(health["result"]["review_gate"]["degraded"], 0);
    assert_eq!(health["result"]["review_gate"]["invalid_response"], 0);

    let phase_status = harness.request(3, "phase.status", None);
    assert!(phase_status["result"]["rate_limiter"].is_object());
    assert!(phase_status["result"]["inflight"].is_object());

    let prometheus = harness.request(30, "metrics.prometheus", None);
    let prometheus_text = prometheus["result"]["text"]
        .as_str()
        .expect("prometheus text should be string");
    assert!(prometheus_text.contains("acp_review_gate_timeout_total 0"));
    assert!(prometheus_text.contains("acp_review_gate_degraded_total 0"));
    assert!(prometheus_text.contains("acp_review_gate_invalid_response_total 0"));
    assert!(prometheus_text.contains("acp_chat_latency_seconds_count"));
    assert!(prometheus_text.contains("acp_agent_latency_seconds_count"));
    assert!(prometheus_text.contains("acp_review_latency_seconds_count"));

    let shutdown = harness.request(4, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);

    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn http_chat_stream_emits_sse_and_persists_knowledge() {
    let _suite_guard = match suite_guard().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    let bind_addr = format!("127.0.0.1:{}", find_free_port());
    write_http_stream_config(&config_path, &bind_addr);

    let mut child = Command::new(binary_path())
        .arg("--config")
        .arg(&config_path)
        .env("GO_ON_ENABLE_LOCAL_TEST_AGENTS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn go-on for HTTP stream test");

    let started = Instant::now();
    loop {
        if started.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            panic!("timed out waiting for ACP HTTP server");
        }
        if let Ok(response) = http_request(&bind_addr, "GET", "/health", None) {
            if response.contains("200 OK") {
                break;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }

    let body = json!({
        "mode": "ask",
        "messages": [{"role": "user", "content": "Explain how to validate rust changes with tests and clippy."}],
        "phase": "coding",
        "conversation_id": "http-conv",
        "branch_id": "main"
    })
    .to_string();
    let response = http_request(&bind_addr, "POST", "/chat/stream", Some(&body))
        .expect("HTTP SSE request should succeed");

    assert!(response.contains("200 OK"));
    assert!(response.contains("Content-Type: text/event-stream"));
    assert!(response.contains("event: chunk"));
    assert!(response.contains("event: done"));
    assert!(response.contains("event: result"));

    let knowledge_path = temp
        .path()
        .join(".goon")
        .join("spec")
        .join("latest-knowledge.json");
    let raw = fs::read_to_string(&knowledge_path).expect("knowledge artifact should exist");
    assert!(raw.contains("reusable_insights"));
    assert!(raw.contains("verification_steps"));

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn rpc_debug_panel_snapshot_contains_runtime_and_conversation_data() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);

    let mut harness = RpcHarness::spawn(&config_path);

    let initialize = harness.request(1, "initialize", None);
    assert_eq!(initialize["result"]["capabilities"]["debug_panel"], true);

    let _checkpoint = harness.request(
        2,
        "conversation.checkpoint.create",
        Some(json!({
            "conversation_id": "panel-conv",
            "branch_id": "main",
            "messages": [
                {"role": "user", "content": "hello panel"},
                {"role": "assistant", "content": "hello"}
            ],
            "note": "panel warmup"
        })),
    );

    let panel = harness.request(
        3,
        "debug.panel.get",
        Some(json!({
            "limit": 50
        })),
    );

    assert_eq!(panel["result"]["ok"], true);
    assert!(panel["result"]["panel"]["trace"]["stage_transitions"].is_array());
    assert!(panel["result"]["panel"]["selected_agents"].is_array());
    assert!(panel["result"]["panel"]["review_outcomes"].is_array());
    assert!(panel["result"]["panel"]["runtime_health"].is_object());
    assert!(panel["result"]["panel"]["review_gate"].is_object());
    assert_eq!(panel["result"]["panel"]["conversations"]["count"], 1);
    assert_eq!(panel["result"]["panel"]["conversations"]["checkpoints"], 1);

    let shutdown = harness.request(4, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_mcp_adapter_initialize_list_and_call() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);

    let mut harness = RpcHarness::spawn(&config_path);

    let initialize = harness.request(1, "initialize", None);
    assert_eq!(initialize["result"]["capabilities"]["mcp_adapter"], true);

    let mcp_init = harness.request(2, "mcp.initialize", Some(json!({})));
    assert_eq!(mcp_init["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(mcp_init["result"]["serverInfo"]["name"], "go-on");

    let tools = harness.request(3, "mcp.tools.list", Some(json!({})));
    assert!(tools["result"]["tools"].is_array());
    let tools_arr = tools["result"]["tools"].as_array().expect("tools array");
    assert!(tools_arr
        .iter()
        .any(|tool| tool["name"] == "acp_debug_panel_get"));

    let called = harness.request(
        4,
        "mcp.tools.call",
        Some(json!({
            "name": "acp_trace_get",
            "arguments": {"limit": 5}
        })),
    );
    assert!(called["result"]["content"].is_array());
    assert_eq!(called["result"]["structuredContent"]["ok"], true);
    assert!(called["result"]["structuredContent"]["events"].is_array());
    assert!(
        called["result"]["structuredContent"]["total"]
            .as_u64()
            .expect("mcp trace total should be integer")
            >= 1
    );
    assert_eq!(called["result"]["structuredContent"]["limit"], json!(5));

    let unknown = harness.request(
        5,
        "mcp.tools.call",
        Some(json!({
            "name": "unknown_tool",
            "arguments": {}
        })),
    );
    assert_eq!(unknown["error"]["code"], -32602);

    let shutdown = harness.request(6, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_conversation_checkpoint_and_rollback() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);

    let mut harness = RpcHarness::spawn(&config_path);
    harness.request(1, "initialize", None);

    let messages = json!([
        {"role": "user", "content": "initial query"},
        {"role": "assistant", "content": "initial response"}
    ]);

    // Create first checkpoint
    let first = harness.request(
        40,
        "conversation.checkpoint.create",
        Some(json!({
            "conversation_id": "conv-test",
            "branch_id": "main",
            "messages": messages,
            "note": "first checkpoint"
        })),
    );
    assert_eq!(first["result"]["ok"], true);
    let first_cp_id = first["result"]["checkpoint"]["checkpoint_id"]
        .as_str()
        .expect("checkpoint_id should be string")
        .to_string();
    assert!(first_cp_id.starts_with("cp-"));

    // Create second checkpoint on same conversation
    let second = harness.request(
        41,
        "conversation.checkpoint.create",
        Some(json!({
            "conversation_id": "conv-test",
            "branch_id": "main",
            "messages": [{"role": "user", "content": "follow-up"}],
            "note": "second checkpoint"
        })),
    );
    assert_eq!(second["result"]["ok"], true);
    let second_cp_id = second["result"]["checkpoint"]["checkpoint_id"]
        .as_str()
        .expect("checkpoint_id should be string")
        .to_string();
    assert_ne!(first_cp_id, second_cp_id);

    // List checkpoints — should return 2
    let listed = harness.request(
        42,
        "conversation.checkpoint.list",
        Some(json!({"conversation_id": "conv-test", "branch_id": "main"})),
    );
    assert_eq!(listed["result"]["ok"], true);
    assert_eq!(listed["result"]["count"], 2);

    // Rollback to first checkpoint, creating a new branch
    let rolled = harness.request(
        43,
        "conversation.rollback",
        Some(json!({
            "conversation_id": "conv-test",
            "checkpoint_id": first_cp_id,
            "branch_id": "hotfix"
        })),
    );
    assert_eq!(rolled["result"]["ok"], true);
    assert_eq!(rolled["result"]["branch_id"], "hotfix");
    assert_ne!(rolled["result"]["checkpoint"]["checkpoint_id"], first_cp_id);
    assert_eq!(
        rolled["result"]["checkpoint"]["parent_checkpoint_id"],
        first_cp_id
    );
    let hotfix_rollback_cp_id = rolled["result"]["checkpoint"]["checkpoint_id"]
        .as_str()
        .expect("hotfix rollback checkpoint id should be string")
        .to_string();

    // Prune: remove old checkpoints from main, keeping only 1
    let pruned = harness.request(
        44,
        "conversation.checkpoint.prune",
        Some(json!({
            "conversation_id": "conv-test",
            "branch_id": "main",
            "keep": 1
        })),
    );
    assert_eq!(pruned["result"]["ok"], true);
    assert_eq!(pruned["result"]["removed"], 1);
    assert!(pruned["result"]["repaired_heads"].is_number());
    assert!(pruned["result"]["dropped_heads"].is_number());

    // List again: main should now have 1 checkpoint
    let listed2 = harness.request(
        45,
        "conversation.checkpoint.list",
        Some(json!({"conversation_id": "conv-test", "branch_id": "main"})),
    );
    assert_eq!(listed2["result"]["count"], 1);

    // After prune, creating a hotfix checkpoint should not reference a removed parent.
    let hotfix_after_prune = harness.request(
        46,
        "conversation.checkpoint.create",
        Some(json!({
            "conversation_id": "conv-test",
            "branch_id": "hotfix",
            "messages": [{"role": "assistant", "content": "hotfix after prune"}],
            "note": "hotfix checkpoint"
        })),
    );
    assert_eq!(hotfix_after_prune["result"]["ok"], true);
    assert_eq!(
        hotfix_after_prune["result"]["checkpoint"]["parent_checkpoint_id"],
        hotfix_rollback_cp_id
    );

    let hotfix_list = harness.request(
        461,
        "conversation.checkpoint.list",
        Some(json!({"conversation_id": "conv-test", "branch_id": "hotfix"})),
    );
    assert_eq!(hotfix_list["result"]["ok"], true);
    assert!(hotfix_list["result"]["checkpoints"]
        .as_array()
        .expect("hotfix checkpoints should be array")
        .iter()
        .any(|item| item["checkpoint_id"] == hotfix_rollback_cp_id));

    // Missing checkpoint_id should return an error
    let bad_rollback = harness.request(
        47,
        "conversation.rollback",
        Some(json!({"conversation_id": "conv-test"})),
    );
    assert_eq!(bad_rollback["error"]["code"], -32602);

    let shutdown = harness.request(48, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_breaker_status_and_reset() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);

    let mut harness = RpcHarness::spawn(&config_path);
    harness.request(1, "initialize", None);

    // Breaker status should be empty initially
    let status = harness.request(50, "breaker.status", None);
    assert!(status["result"].is_object());

    // Breaker reset of non-existent agent returns 0 removed
    let reset_single = harness.request(51, "breaker.reset", Some(json!({"agent": "nonexistent"})));
    assert_eq!(reset_single["result"]["ok"], true);
    assert_eq!(reset_single["result"]["removed"], 0);

    // Breaker reset all also succeeds
    let reset_all = harness.request(52, "breaker.reset", None);
    assert_eq!(reset_all["result"]["ok"], true);

    let shutdown = harness.request(53, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_cache_clear_and_checkpoint_missing_messages() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);

    let mut harness = RpcHarness::spawn(&config_path);
    harness.request(1, "initialize", None);

    // cache.clear should succeed even with empty caches
    let clear = harness.request(60, "cache.clear", None);
    assert_eq!(clear["result"]["ok"], true);
    assert!(clear["result"]["memory_removed"].is_number());
    assert!(clear["result"]["sqlite_removed"].is_number());

    // conversation.checkpoint.create without messages should error
    let no_messages = harness.request(
        61,
        "conversation.checkpoint.create",
        Some(json!({"conversation_id": "conv-x"})),
    );
    assert_eq!(no_messages["error"]["code"], -32602);

    // conversation.checkpoint.list for unknown conversation returns empty
    let empty_list = harness.request(
        62,
        "conversation.checkpoint.list",
        Some(json!({"conversation_id": "conv-unknown"})),
    );
    assert_eq!(empty_list["result"]["count"], 0);

    // invalid conversation_id/branch_id should fail validation
    let bad_identifiers = harness.request(
        621,
        "conversation.checkpoint.create",
        Some(json!({
            "conversation_id": "  ",
            "branch_id": "bad branch",
            "messages": [{"role": "user", "content": "x"}]
        })),
    );
    assert_eq!(bad_identifiers["error"]["code"], -32602);

    let bad_keep = harness.request(
        622,
        "conversation.checkpoint.prune",
        Some(json!({
            "conversation_id": "conv-x",
            "keep": 0
        })),
    );
    assert_eq!(bad_keep["error"]["code"], -32602);

    // metrics.reset should succeed
    let reset = harness.request(63, "metrics.reset", None);
    assert_eq!(reset["result"]["ok"], true);

    let shutdown = harness.request(64, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_unknown_method_and_config_reload() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);

    let mut harness = RpcHarness::spawn(&config_path);

    let unknown = harness.request(10, "unknown.method", None);
    assert_eq!(unknown["error"]["code"], -32601);
    let message = unknown["error"]["message"]
        .as_str()
        .expect("error message should be string");
    assert!(message.contains("unknown method"));

    let metrics_after_unknown = harness.request(1010, "metrics", None);
    assert!(
        metrics_after_unknown["result"]["metrics"]["failed_requests"]
            .as_u64()
            .expect("failed_requests should be integer")
            >= 1
    );

    write_test_config(&config_path, 30, 45, 7);

    let reload = harness.request(11, "config.reload", None);
    assert_eq!(reload["result"]["ok"], true);
    let reload_note = reload["result"]["note"]
        .as_str()
        .expect("reload note should be string");
    assert!(
        reload_note == "flow/registry/cache/vector/autotune resources reloaded"
            || reload_note == "info.resources_reloaded"
    );
    let reload_path = reload["result"]["path"]
        .as_str()
        .expect("reload path should be string");
    assert!(Path::new(reload_path).ends_with("config.toml"));
    assert_eq!(reload["result"]["warning_count"], 0);
    assert_eq!(reload["result"]["warnings"], json!([]));
    assert_eq!(reload["result"]["profile_recommendation"], "minimal");
    assert!(reload["result"]["recommendations"].is_array());
    assert_eq!(reload["result"]["health"]["score"], 100);
    assert_eq!(reload["result"]["health"]["critical_count"], 0);
    assert_eq!(reload["result"]["health"]["warn_count"], 0);
    assert_eq!(reload["result"]["health"]["info_count"], 0);

    let shutdown = harness.request(12, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);

    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_action_vector_maintenance_and_trace_metrics() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);

    let mut harness = RpcHarness::spawn(&config_path);
    harness.request(200, "initialize", None);

    let action = harness.request(
        201,
        "action.check",
        Some(json!({
            "kind": "all"
        })),
    );
    assert!(action["result"]["report"].is_object());

    let vector = harness.request(202, "vector.clear", None);
    assert_eq!(vector["result"]["ok"], true);
    assert!(vector["result"]["vector_removed"].is_u64());
    assert!(vector["result"]["summary_removed"].is_u64());

    let maintenance = harness.request(203, "maintenance.gc", None);
    assert_eq!(maintenance["result"]["ok"], true);
    assert!(maintenance["result"]["memory_expired_removed"].is_u64());
    assert!(maintenance["result"]["sqlite_expired_removed"].is_u64());
    assert!(maintenance["result"]["cache_vacuumed"].is_boolean());
    assert!(maintenance["result"]["vector_vacuumed"].is_boolean());
    assert!(maintenance["result"]["maintenance"].is_object());

    let trace_metrics = harness.request(204, "trace.metrics", None);
    assert!(trace_metrics["result"]["sampling_rate"].is_number());
    assert!(trace_metrics["result"]["buffered_events"].is_u64());
    assert!(trace_metrics["result"]["slow_requests_top_n"].is_array());
    assert!(trace_metrics["result"]["phase_latency"].is_object());
    assert!(trace_metrics["result"]["pua_stage_counts"].is_object());

    let shutdown = harness.request(205, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_legacy_method_aliases_remain_compatible() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    let state_path = temp.path().join("compat_autotune_state.json");
    write_autotune_enabled_config(&config_path, &state_path);

    let mut harness = RpcHarness::spawn(&config_path);
    harness.request(300, "initialize", None);

    let metrics = harness.request(301, "metrics.get", None);
    assert!(metrics["result"]["total_requests"].is_u64());
    assert!(metrics["result"]["active_requests"].is_u64());

    let autotune = harness.request(302, "autotune.get", None);
    assert!(autotune["result"]["current_min_query_chars"].is_u64());
    assert!(autotune["result"]["current_top_k"].is_u64());

    let plan = harness.request(
        303,
        "task.plan",
        Some(json!({
            "task": "Plan a governed refactor for ACP routing compatibility",
            "requirement_confirmed": true,
            "goal": "restore legacy ACP compatibility routes",
            "scope": "request alias compatibility only",
            "acceptance_criteria": ["legacy methods resolve", "tests cover aliases"]
        })),
    );
    assert_eq!(plan["result"]["ok"], true);
    assert!(plan["result"]["plan"].is_object());
    assert!(plan["result"]["artifact_path"].is_string());
    assert_eq!(plan["result"]["requirement_gate"]["confirmed"], true);

    let workflow = harness.request(
        304,
        "workflow.generate",
        Some(json!({
            "task": "Generate workflow for ACP route compatibility rollout",
            "requirement_confirmed": true,
            "goal": "generate execution workflow",
            "scope": "compatibility alias rollout",
            "acceptance_criteria": ["workflow emitted", "plan emitted"]
        })),
    );
    assert_eq!(workflow["result"]["ok"], true);
    assert!(workflow["result"]["plan"].is_object());
    assert!(workflow["result"]["workflow"].is_object());
    assert!(workflow["result"]["plan_artifact_path"].is_string());
    assert!(workflow["result"]["workflow_artifact_path"].is_string());
    assert_eq!(workflow["result"]["requirement_gate"]["confirmed"], true);

    let shutdown = harness.request(305, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_rejects_non_2_0_jsonrpc_version() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);

    let mut harness = RpcHarness::spawn(&config_path);

    harness.raw_request(&json!({
        "jsonrpc": "1.0",
        "id": 21,
        "method": "initialize"
    }));

    let invalid = harness.read_response_for_id(21, Duration::from_secs(8));
    assert_eq!(invalid["error"]["code"], -32600);
    let message = invalid["error"]["message"]
        .as_str()
        .expect("error message should be string");
    assert!(message.contains("jsonrpc must be 2.0"));

    let shutdown = harness.request(22, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);

    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_chat_rejects_invalid_params() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);

    let mut harness = RpcHarness::spawn(&config_path);

    let invalid = harness.request(31, "chat", Some(json!({ "mode": "ask" })));
    assert_eq!(invalid["error"]["code"], -32602);
    let message = invalid["error"]["message"]
        .as_str()
        .expect("error message should be string");
    assert!(
        message.contains("invalid chat params") || message.contains("error.invalid_chat_params")
    );

    let shutdown = harness.request(32, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);

    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_config_reload_reports_runtime_warnings() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);

    let rules_dir = temp.path().join("RULES");
    fs::create_dir_all(&rules_dir).expect("failed to create RULES dir");
    fs::write(
        rules_dir.join("local.md"),
        "# empty\n\n```md\nignored\n```\n",
    )
    .expect("failed to write empty local rules file");

    let mut harness = RpcHarness::spawn(&config_path);

    write_warning_config(&config_path);

    let reload = harness.request(33, "config.reload", None);
    assert_eq!(reload["result"]["ok"], true);
    let warning_count = reload["result"]["warning_count"]
        .as_u64()
        .expect("warning_count should be integer");
    assert!(warning_count >= 2);
    assert!(reload["result"]["profile_recommendation"].is_string());
    assert!(reload["result"]["recommendations"].is_array());
    assert!(
        reload["result"]["health"]["score"]
            .as_u64()
            .expect("health score should be integer")
            < 100
    );
    assert!(
        reload["result"]["health"]["critical_count"]
            .as_u64()
            .expect("critical_count should be integer")
            >= 1
    );

    let warnings = reload["result"]["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|value| {
        value
            .as_str()
            .map(|text| text.contains("local.md") && text.contains("usable rule lines"))
            .unwrap_or(false)
    }));
    assert!(warnings.iter().any(|value| {
        value
            .as_str()
            .map(|text| text.contains("review gate may hang too long"))
            .unwrap_or(false)
    }));

    let shutdown = harness.request(34, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);

    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_chat_provider_failure_degrades_to_fallback_agent() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_provider_failure_degrade_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);

    let initialize = harness.request(71, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let chat = harness.request(
        72,
        "chat",
        Some(json!({
            "messages": [{"role": "user", "content": "provider failure fallback check"}],
            "mode": "ask"
        })),
    );

    assert_eq!(chat["result"]["done"], true);
    assert_eq!(chat["result"]["agent"], "local_fallback");

    let trace = harness.request(73, "trace.get", Some(json!({"limit": 200})));
    let events = trace["result"]["events"]
        .as_array()
        .expect("trace events should be array");
    assert!(events.iter().any(|event| {
        event["event_type"] == "phase.agent"
            && event["status"] == "ok"
            && event["inputs"]["attributes"]["agent"] == "local_fallback"
    }));
    assert!(events.iter().any(|event| {
        event["event_type"] == "request.end"
            && event["status"] == "ok"
            && event["inputs"]["attributes"]["method"] == "chat"
    }));

    let shutdown = harness.request(74, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_chat_review_timeout_collision_reports_timeout_and_gate_outcome() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_review_timeout_collision_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(81, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let chat = harness.request(
        82,
        "chat",
        Some(json!({
            "messages": [{"role": "user", "content": "review timeout collision"}],
            "mode": "full_auto"
        })),
    );

    if chat["error"].is_object() {
        assert_eq!(chat["error"]["code"], -32603);
        let reviews = chat["error"]["data"]["reviews"]
            .as_array()
            .expect("reviews should be array");
        assert!(!reviews.is_empty());
        assert_eq!(reviews[0]["verdict"], "APPROVE");
    } else {
        assert_eq!(chat["result"]["done"], true);
        let reviews = chat["result"]["reviews"]
            .as_array()
            .expect("reviews should be array");
        assert!(!reviews.is_empty());
        assert_eq!(reviews[0]["verdict"], "APPROVE");
    }

    let health = harness.request(83, "runtime.health", None);
    assert!(
        health["result"]["review_gate"]["timeout"]
            .as_u64()
            .expect("timeout count should be integer")
            >= 1
    );
    let rejected = health["result"]["review_gate"]["rejected"]
        .as_u64()
        .expect("rejected count should be integer");
    let degraded = health["result"]["review_gate"]["degraded"]
        .as_u64()
        .expect("degraded count should be integer");
    assert!(rejected >= 1 || degraded >= 1);

    let shutdown = harness.request(84, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn startup_fails_when_cache_vector_paths_are_unavailable() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    let cache_dir = temp.path().join("cache_dir_as_file");
    let vector_dir = temp.path().join("vector_dir_as_file");
    fs::create_dir_all(&cache_dir).expect("failed to create cache dir");
    fs::create_dir_all(&vector_dir).expect("failed to create vector dir");

    let cache_path = cache_dir.to_string_lossy().replace('\\', "\\\\");
    let vector_path = vector_dir.to_string_lossy().replace('\\', "\\\\");
    write_cache_vector_unavailable_config(&config_path, &cache_path, &vector_path);

    let output = Command::new(binary_path())
        .arg("--config")
        .arg(&config_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to run go-on for startup failure scenario");

    assert!(
        !output.status.success(),
        "startup should fail on unavailable sqlite file paths"
    );
    let stderr_text = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr_text.contains("unable to open database file")
            || stderr_text.contains("fatal error")
            || stderr_text.contains("error.fatal")
            || stderr_text.contains("database")
    );
}

#[test]
fn rpc_chat_rate_limit_saturation_returns_rate_limited_error() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_rate_limit_saturation_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(100, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let first = harness.request(
        101,
        "chat",
        Some(json!({
            "messages": [{"role": "user", "content": "first rate-limited request"}],
            "mode": "ask"
        })),
    );
    assert_eq!(first["result"]["done"], true);

    let second = harness.request(
        102,
        "chat",
        Some(json!({
            "messages": [{"role": "user", "content": "second rate-limited request"}],
            "mode": "ask"
        })),
    );
    assert_eq!(second["error"]["code"], -32029);
    let message = second["error"]["message"]
        .as_str()
        .expect("rate-limit error message should be string");
    assert!(message.contains("rate limited"));

    let shutdown = harness.request(103, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_task_execute_blocks_when_requirement_not_confirmed() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_workflow_governance_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(110, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let execute = harness.request(
        111,
        "task.execute",
        Some(json!({
            "task": "Refactor auth and billing modules with security hardening, migration plan, and regression verification"
        })),
    );

    assert_eq!(execute["error"]["code"], -32006);
    assert_eq!(execute["error"]["data"]["kind"], "requirement_contract");
    assert_eq!(
        execute["error"]["data"]["next_step"]["method"],
        "workflow.clarify"
    );

    let shutdown = harness.request(112, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_workflow_execute_returns_review_policy_and_learning_feedback_fields() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_workflow_governance_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(120, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let task = "Refactor auth and billing modules with security hardening, migration plan, and regression verification";
    let confirmed = harness.request(
        121,
        "workflow.confirm",
        Some(json!({
            "task": task,
            "user_confirmed": true,
            "ready_to_confirm": true,
            "requirement_contract": {
                "goal": "Harden release path while preserving behavior",
                "scope": "auth and billing modules",
                "non_goals": ["rewrite architecture"],
                "acceptance_criteria": [
                    "all existing tests pass",
                    "no regression in auth and billing integration"
                ],
                "constraints": [
                    "no breaking API changes",
                    "must keep migration reversible"
                ]
            }
        })),
    );
    assert_eq!(confirmed["result"]["ok"], true);

    let execute = harness.request(
        122,
        "workflow.execute",
        Some(json!({
            "task": task,
            "requirement_confirmed": true,
            "capability_decision": "degrade",
            "capability_confirm": true,
            "clarification_rounds": 3,
            "clarification_quality_score": 0.85,
            "requirement_change_count": 2,
            "auto_gates": false
        })),
    );

    assert_eq!(execute["result"]["ok"], true);
    assert_eq!(
        execute["result"]["review_policy"]["min_review_level"],
        "standard"
    );
    assert_eq!(execute["result"]["review_policy"]["required_reviews"], 1);

    let learning_artifact_path = execute["result"]["learning_artifact_path"]
        .as_str()
        .expect("learning_artifact_path should be string");
    let learning_raw = fs::read_to_string(learning_artifact_path)
        .expect("failed to read latest-learning artifact");
    let learning_json: Value =
        serde_json::from_str(&learning_raw).expect("latest-learning should be valid json");
    let events = learning_json["events"]
        .as_array()
        .expect("learning events should be array");
    let event = events.last().expect("learning events should not be empty");
    assert_eq!(event["clarification_rounds"], 3);
    let quality = event["clarification_quality_score"]
        .as_f64()
        .expect("clarification_quality_score should be number");
    assert!((quality - 0.85).abs() < 1e-6);
    assert_eq!(event["requirement_change_count"], 2);
    assert_eq!(event["review_reject_root_cause"], "");

    let shutdown = harness.request(123, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_workflow_execute_enforces_dual_review_and_returns_decisions() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_workflow_dual_review_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(124, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let task = "Refactor auth and billing modules with safety checks and verification";
    let confirmed = harness.request(
        125,
        "workflow.confirm",
        Some(json!({
            "task": task,
            "user_confirmed": true,
            "ready_to_confirm": true,
            "requirement_contract": {
                "goal": "harden auth and billing safely",
                "scope": "auth and billing modules",
                "non_goals": ["full architecture rewrite"],
                "acceptance_criteria": ["all tests pass"],
                "constraints": ["no API break"]
            }
        })),
    );
    assert_eq!(confirmed["result"]["ok"], true);

    let execute = harness.request(
        126,
        "workflow.execute",
        Some(json!({
            "task": task,
            "requirement_confirmed": true,
            "auto_gates": false
        })),
    );
    assert!(execute["error"].is_null());
    assert!(execute["result"].is_object());
    assert_eq!(execute["result"]["review_policy"]["required_reviews"], 2);

    let reviews = execute["result"]["reviews"]
        .as_array()
        .expect("reviews should be array when dual review is enforced");
    assert_eq!(reviews.len(), 2);
    assert_eq!(reviews[0]["verdict"], "APPROVE");
    assert_eq!(reviews[1]["verdict"], "APPROVE");

    let shutdown = harness.request(127, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_learning_summary_aggregates_clarification_feedback_metrics() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_workflow_governance_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(130, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let task = "Refactor auth and billing modules with security hardening, migration plan, and regression verification";
    let confirmed = harness.request(
        131,
        "workflow.confirm",
        Some(json!({
            "task": task,
            "user_confirmed": true,
            "ready_to_confirm": true,
            "requirement_contract": {
                "goal": "Harden release path while preserving behavior",
                "scope": "auth and billing modules",
                "non_goals": ["rewrite architecture"],
                "acceptance_criteria": [
                    "all existing tests pass",
                    "no regression in auth and billing integration"
                ],
                "constraints": [
                    "no breaking API changes",
                    "must keep migration reversible"
                ]
            }
        })),
    );
    assert_eq!(confirmed["result"]["ok"], true);

    let execute = harness.request(
        132,
        "workflow.execute",
        Some(json!({
            "task": task,
            "requirement_confirmed": true,
            "capability_decision": "degrade",
            "capability_confirm": true,
            "clarification_rounds": 4,
            "clarification_quality_score": 0.9,
            "requirement_change_count": 3,
            "auto_gates": false
        })),
    );
    assert_eq!(execute["result"]["ok"], true);

    let summary = harness.request(
        133,
        "learning.summary",
        Some(json!({
            "limit": 20
        })),
    );
    assert_eq!(summary["result"]["ok"], true);

    let sampled = summary["result"]["summary"]["sampled_events"]
        .as_u64()
        .expect("sampled_events should be integer");
    assert!(sampled >= 1);
    assert_eq!(
        summary["result"]["summary"]["totals"]["requirement_change_count"],
        3
    );

    let rounds = summary["result"]["summary"]["averages"]["clarification_rounds"]
        .as_f64()
        .expect("clarification_rounds average should be number");
    assert!(rounds >= 4.0);

    let quality = summary["result"]["summary"]["averages"]["clarification_quality_score"]
        .as_f64()
        .expect("clarification_quality_score average should be number");
    assert!(quality >= 0.9);

    let gates_pass_rate = summary["result"]["summary"]["rates"]["gates_pass_rate"]
        .as_f64()
        .expect("gates_pass_rate should be number");
    assert!(gates_pass_rate >= 1.0);

    let shutdown = harness.request(134, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_primary_secondary_policy_artifact_is_persisted_and_response_contains_policy() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_workflow_governance_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(140, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let task = "Build secure payment gateway with input validation and audit logging";
    let _confirmed = harness.request(
        141,
        "workflow.confirm",
        Some(json!({
            "task": task,
            "user_confirmed": true,
            "requirement_contract": {
                "goal": "Secure payment gateway",
                "scope": "payment module",
                "non_goals": ["rewrite existing billing"],
                "acceptance_criteria": ["all payment flows pass tests"],
                "constraints": ["pci-dss compliant"]
            }
        })),
    );

    let execute = harness.request(
        142,
        "workflow.execute",
        Some(json!({
            "task": task,
            "requirement_confirmed": true,
            "auto_gates": false
        })),
    );
    assert_eq!(execute["result"]["ok"], true);

    // primary_secondary_policy must be present in the execute response under blue5
    let policy = &execute["result"]["blue5"]["primary_secondary_policy"];
    assert!(
        policy.is_object(),
        "primary_secondary_policy must be object in response"
    );
    assert!(
        !policy["primary_agent"].as_str().unwrap_or("").is_empty(),
        "primary_agent must not be empty"
    );
    assert!(
        policy["failover_policy"].is_string(),
        "failover_policy must be string"
    );

    assert!(
        execute["result"]["primary_failover_artifact_path"].is_string(),
        "primary_failover_artifact_path must be present"
    );
    assert!(
        execute["result"]["primary_failover_report"].is_object(),
        "primary_failover_report must be object"
    );
    assert!(
        execute["result"]["primary_failover_report"]["failover_policy"].is_string(),
        "primary_failover_report.failover_policy must be string"
    );
    assert!(
        execute["result"]["primary_failover_report"]["reports"].is_array(),
        "primary_failover_report.reports must be array"
    );

    // The policy artifact path must be present
    let _artifact_path = execute["result"]["artifact_path"]
        .as_str()
        .expect("artifact_path should be string");

    let shutdown = harness.request(143, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_primary_secondary_summary_reports_stability_and_failover_metrics() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_workflow_governance_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(150, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let task = "Migrate legacy auth service to OAuth2 with rollback plan";
    let _confirmed = harness.request(
        151,
        "workflow.confirm",
        Some(json!({
            "task": task,
            "user_confirmed": true,
            "requirement_contract": {
                "goal": "Migrate auth to OAuth2",
                "scope": "auth service",
                "non_goals": ["migrate billing"],
                "acceptance_criteria": ["OAuth2 tests pass"],
                "constraints": ["must rollback in < 5 minutes"]
            }
        })),
    );

    let execute = harness.request(
        152,
        "workflow.execute",
        Some(json!({
            "task": task,
            "requirement_confirmed": true,
            "auto_gates": false
        })),
    );
    assert_eq!(execute["result"]["ok"], true);

    // primary_secondary.summary must return ok with correct shape
    let summary = harness.request(
        153,
        "primary_secondary.summary",
        Some(json!({ "limit": 20 })),
    );
    assert_eq!(summary["result"]["ok"], true);

    let s = &summary["result"]["summary"];
    assert!(
        s["total_events"].as_u64().unwrap_or(0) >= 1,
        "total_events must be >= 1 after an execute"
    );
    assert!(
        s["averages"]["primary_stability_score"].is_number(),
        "primary_stability_score must be a number"
    );
    assert!(
        s["averages"]["secondary_utilization_rate"].is_number(),
        "secondary_utilization_rate must be a number"
    );
    assert!(
        s["totals"]["failover_count"].is_number(),
        "failover_count must be a number"
    );
    assert!(
        s["failover_root_causes"].is_object(),
        "failover_root_causes must be an object"
    );

    let shutdown = harness.request(154, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_workflow_consult_returns_artifact_and_consensus_signal() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_workflow_governance_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(160, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let consult = harness.request(
        161,
        "workflow.consult",
        Some(json!({
            "task": "Design safe data migration strategy with rollback and evidence plan",
            "trigger_reason": "unclear requirement and conflicting constraints",
            "consultation_confidence_threshold": 0.5
        })),
    );
    assert_eq!(consult["result"]["ok"], true);
    assert!(consult["result"]["artifact"].is_object());
    assert!(consult["result"]["artifact_path"].is_string());
    assert!(consult["result"]["artifact"]["participants"].is_array());
    assert!(consult["result"]["artifact"]["consensus_plan"].is_string());

    let shutdown = harness.request(162, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_workflow_research_persists_artifact_and_plan() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_workflow_governance_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(165, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let research = harness.request(
        166,
        "workflow.research",
        Some(json!({
            "task": "Research cross-module impact of introducing stricter audit evidence",
            "research_focus": "impact analysis, migration risk, and rollback plan"
        })),
    );
    assert_eq!(research["result"]["ok"], true);
    assert!(research["result"]["artifact"].is_object());
    assert!(research["result"]["artifact"]["planner_output"].is_string());
    assert!(research["result"]["artifact"]["researcher_output"].is_string());
    assert!(research["result"]["artifact"]["reviewer_output"].is_string());
    assert!(research["result"]["artifact"]["recommended_plan"].is_string());
    assert!(research["result"]["planned_subtasks"].is_number());

    let artifact_path = research["result"]["artifact_path"]
        .as_str()
        .expect("research artifact path should be string");
    assert!(Path::new(artifact_path).exists());

    let plan_artifact_path = research["result"]["plan_artifact_path"]
        .as_str()
        .expect("research plan artifact path should be string");
    assert!(Path::new(plan_artifact_path).exists());

    let artifact_raw = fs::read_to_string(artifact_path).expect("read research artifact");
    let artifact_json: Value =
        serde_json::from_str(&artifact_raw).expect("parse research artifact");
    assert_eq!(
        artifact_json["task"],
        "Research cross-module impact of introducing stricter audit evidence"
    );

    let shutdown = harness.request(167, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_confirm_requires_ready_to_confirm_and_respects_clarification_rounds() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_workflow_governance_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(170, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let task = "Clarify security hardening scope for auth and billing";
    let clarify = harness.request(
        171,
        "workflow.clarify",
        Some(json!({
            "task": task,
            "clarify_collaboration_mode": "multi_ai",
            "round_index": 1,
            "ready_to_confirm": false
        })),
    );
    assert_eq!(clarify["result"]["ok"], true);
    assert_eq!(clarify["result"]["clarification_session"]["round_index"], 1);
    assert_eq!(
        clarify["result"]["clarification_session"]["ready_to_confirm"],
        false
    );
    assert!(clarify["result"]["clarification_session_artifact_path"].is_string());

    let blocked_confirm = harness.request(
        172,
        "workflow.confirm",
        Some(json!({
            "task": task,
            "user_confirmed": true,
            "requirement_contract": {
                "goal": "harden auth and billing",
                "scope": "auth,billing modules",
                "non_goals": ["architecture rewrite"],
                "acceptance_criteria": ["all regression tests pass"],
                "constraints": ["no api break"]
            }
        })),
    );
    assert_eq!(blocked_confirm["error"]["code"], -32006);
    assert_eq!(
        blocked_confirm["error"]["data"]["kind"],
        "clarification_session"
    );
    assert_eq!(
        blocked_confirm["error"]["data"]["next_step"]["method"],
        "workflow.clarify"
    );

    let confirmed = harness.request(
        173,
        "workflow.confirm",
        Some(json!({
            "task": task,
            "user_confirmed": true,
            "ready_to_confirm": true,
            "requirement_contract": {
                "goal": "harden auth and billing",
                "scope": "auth,billing modules",
                "non_goals": ["architecture rewrite"],
                "acceptance_criteria": ["all regression tests pass"],
                "constraints": ["no api break"]
            }
        })),
    );
    assert_eq!(confirmed["result"]["ok"], true);
    assert_eq!(
        confirmed["result"]["clarification_session"]["ready_to_confirm"],
        true
    );

    let shutdown = harness.request(174, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_autotune_reset_restores_default_state_and_persists() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    let state_path = temp.path().join("acp_autotune_state.json");
    write_autotune_enabled_config(&config_path, &state_path);

    let custom_state = json!({
        "current_min_query_chars": 240,
        "current_top_k": 4,
        "window_phase": 9,
        "high_precision_count": 7,
        "low_precision_count": 3,
        "vector_search_count": 18,
        "cooldown_remaining": 2
    });
    fs::write(
        &state_path,
        serde_json::to_string_pretty(&custom_state).expect("serialize custom state"),
    )
    .expect("write custom autotune state");

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(180, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let status_before = harness.request(181, "autotune.status", None);
    assert_eq!(status_before["result"]["enabled"], true);
    assert_eq!(
        status_before["result"]["state"]["current_min_query_chars"],
        240
    );

    let reset = harness.request(182, "autotune.reset", None);
    assert_eq!(reset["result"]["ok"], true);
    assert_eq!(reset["result"]["reset"], true);
    assert_eq!(reset["result"]["enabled"], true);
    assert_eq!(
        reset["result"]["state_before"]["current_min_query_chars"],
        240
    );
    assert_eq!(
        reset["result"]["state_after"]["current_min_query_chars"],
        40
    );

    let status_after = harness.request(183, "autotune.status", None);
    assert_eq!(
        status_after["result"]["state"]["current_min_query_chars"],
        40
    );
    assert_eq!(status_after["result"]["state"]["window_phase"], 0);
    assert_eq!(status_after["result"]["state"]["cooldown_remaining"], 0);

    let persisted_raw = fs::read_to_string(&state_path).expect("read persisted autotune state");
    let persisted: Value =
        serde_json::from_str(&persisted_raw).expect("parse persisted autotune state");
    assert_eq!(persisted["current_min_query_chars"], 40);
    assert_eq!(persisted["window_phase"], 0);

    let shutdown = harness.request(184, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn rpc_workflow_execute_auto_consultation_blocks_without_consensus() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_workflow_governance_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(180, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let task = "Refactor core auth pipeline with high-risk migration";
    let _confirmed = harness.request(
        181,
        "workflow.confirm",
        Some(json!({
            "task": task,
            "user_confirmed": true,
            "ready_to_confirm": true,
            "requirement_contract": {
                "goal": "safe auth migration",
                "scope": "auth module",
                "non_goals": ["billing refactor"],
                "acceptance_criteria": ["all auth tests pass"],
                "constraints": ["no downtime"]
            }
        })),
    );

    let execute = harness.request(
        182,
        "workflow.execute",
        Some(json!({
            "task": task,
            "requirement_confirmed": true,
            "consultation_required": true,
            "consultation_confidence_threshold": 0.95,
            "auto_gates": false
        })),
    );
    assert_eq!(execute["error"]["code"], -32007);
    assert_eq!(execute["error"]["data"]["kind"], "consultation_blocked");
    assert!(execute["error"]["data"]["consultation_artifact_path"].is_string());

    let shutdown = harness.request(183, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}
