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
