use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::tempdir;

struct RpcHarness {
    child: Child,
    stdin: ChildStdin,
    stdout_rx: Receiver<Value>,
}

impl RpcHarness {
    fn spawn(config_path: &Path) -> Self {
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
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for _line in reader.lines() {
                // best-effort drain
            }
        });

        Self {
            child,
            stdin,
            stdout_rx,
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
        writeln!(self.stdin, "{body}").expect("failed to write request to stdin");
        self.stdin.flush().expect("failed to flush request");

        self.read_response_for_id(id, Duration::from_secs(8))
    }

    fn raw_request(&mut self, payload: &Value) {
        let body = serde_json::to_string(payload).expect("failed to encode raw request");
        writeln!(self.stdin, "{body}").expect("failed to write raw request");
        self.stdin.flush().expect("failed to flush raw request");
    }

    fn read_response_for_id(&mut self, id: u64, timeout: Duration) -> Value {
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                panic!("timed out waiting for response id {id}");
            }
            let remaining = deadline.saturating_duration_since(now);
            let msg = self
                .stdout_rx
                .recv_timeout(remaining)
                .expect("stdout closed while waiting for response");
            if msg.get("id") == Some(&json!(id)) {
                return msg;
            }
        }
    }

    fn wait_for_exit(&mut self, timeout: Duration) {
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
                Err(err) => panic!("failed to wait for child: {err}"),
            }
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

    let shutdown = harness.request(4, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);

    harness.wait_for_exit(Duration::from_secs(8));
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
    assert_eq!(rolled["result"]["checkpoint"]["checkpoint_id"], first_cp_id);

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

    // List again: main should now have 1 checkpoint
    let listed2 = harness.request(
        45,
        "conversation.checkpoint.list",
        Some(json!({"conversation_id": "conv-test", "branch_id": "main"})),
    );
    assert_eq!(listed2["result"]["count"], 1);

    // Missing checkpoint_id should return an error
    let bad_rollback = harness.request(
        46,
        "conversation.rollback",
        Some(json!({"conversation_id": "conv-test"})),
    );
    assert_eq!(bad_rollback["error"]["code"], -32602);

    let shutdown = harness.request(47, "shutdown", None);
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

    write_test_config(&config_path, 30, 45, 7);

    let reload = harness.request(11, "config.reload", None);
    assert_eq!(reload["result"]["ok"], true);
    assert_eq!(
        reload["result"]["note"],
        "flow/registry/cache/vector/autotune resources reloaded"
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
    assert!(message.contains("invalid chat params"));

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
fn rpc_chat_review_timeout_collision_reports_timeout_and_rejection() {
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
        stderr_text.contains("unable to open database file") || stderr_text.contains("fatal error")
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
