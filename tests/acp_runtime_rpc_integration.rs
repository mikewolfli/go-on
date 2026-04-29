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

fn assert_blue22_execution_cycle_shape(result: &Value) {
    assert!(result["capability_profile"].is_object());
    assert!(result["capability_profile"]["platform_mode"].is_string());
    assert!(result["capability_profile"]["phase_compat"].is_object());
    assert!(result["governance_profile"].is_object());
    assert!(result["governance_profile"]["risk_band"].is_string());
    assert!(result["governance_profile"]["budget"].is_object());
    assert!(result["learning_profile"].is_object());
    assert!(result["learning_profile"]["learning_mode"].is_string());
    assert!(result["learning_profile"]["cognition"].is_object());
    assert!(result["token_economy"].is_object());
    assert!(result["token_economy"]["budget"]["request_token_budget"].is_number());
    assert!(result["token_economy"]["multi_round_strategy"]["enabled"].is_boolean());
    assert!(result["token_economy"]["multi_round_strategy"]["max_rounds"].is_number());
    assert!(result["knowledge_refinement"].is_object());
    assert!(result["knowledge_refinement"]["distillation"]["enabled"].is_boolean());
    assert!(result["knowledge_refinement"]["self_evolution"]["mode"].is_string());
    assert!(result["sandbox_profile"].is_object());
    assert!(result["sandbox_profile"]["selected"].is_string());
    assert!(result["approval_checkpoint"].is_object());
    assert!(result["approval_checkpoint"]["required"].is_boolean());
    assert!(result["approval_checkpoint"]["resume_token"].is_string());
    assert!(result["repo_context"].is_object());
    assert!(result["repo_context"]["repository"].is_string());
    assert!(result["repo_context"]["patch_set"]["count"].is_number());
    assert!(result["execution_cycle"].is_object());
    assert!(result["execution_cycle"]["cycle_id"].is_string());
    assert!(result["execution_cycle"]["current_cycle"].is_object());
    assert!(result["execution_cycle"]["cycles"].is_array());
    assert!(result["execution_cycle"]["history_summary"]["total_cycles"].is_number());
    assert!(result["execution_cycle"]["history_summary"]["pending_repair_iterations"].is_number());
    assert!(result["execution_cycle"]["auto_repair"].is_object());
    assert!(result["execution_cycle"]["current_cycle"]["plan_version"].is_string());
}

fn assert_blue22_runtime_execute_cycle_shape(result: &Value) {
    assert_blue22_execution_cycle_shape(result);
    assert!(result["execution_cycle"]["history_summary"]["current_iteration"].is_number());
    assert!(result["execution_cycle"]["history_summary"]["repair_iterations"].is_number());
    assert!(result["execution_cycle"]["auto_repair"]["status"].is_string());
    assert!(result["execution_cycle"]["current_cycle"]["patch_set"].is_array());
    assert!(result["execution_cycle"]["current_cycle"]["patch_set_size"].is_number());
    assert!(result["execution_cycle"]["auto_repair"]["target_subtasks"].is_array());
    assert!(result["execution_cycle"]["auto_repair"]
        .get("next_cycle_preview")
        .is_some());
    assert!(result["multi_agent"].is_object());
    assert!(result["multi_agent"]["agent_session"].is_object());
    assert!(result["multi_agent"]["subtask_sessions"].is_array());
    assert!(result["multi_agent"]["merge_session"].is_object());
    // B26-S11: task_graph_checkpoint must be present and resumable in execution_cycle
    assert!(result["execution_cycle"]["task_graph_checkpoint"].is_object());
    assert!(result["execution_cycle"]["task_graph_checkpoint"]["checkpoint_id"].is_string());
    assert!(result["execution_cycle"]["task_graph_checkpoint"]["resume_eligible"].is_boolean());
    assert!(result["execution_cycle"]["task_graph_checkpoint"]["phases_completed"].is_number());
    // B26-S12: tool_loop (think-act-observe) must be in execution_cycle
    assert!(result["execution_cycle"]["tool_loop"].is_object());
    assert!(result["execution_cycle"]["tool_loop"]["phase"].is_string());
    assert!(result["execution_cycle"]["tool_loop"]["safety_gate_passed"].is_boolean());
    assert!(result["execution_cycle"]["tool_loop"]["governance"].is_object());
    // B26-S13: multi_agent must have handoff_protocol + conflict_resolution
    assert!(result["multi_agent"]["handoff_protocol"].is_object());
    assert!(result["multi_agent"]["handoff_protocol"]["schema_version"].is_string());
    assert!(result["multi_agent"]["conflict_resolution"].is_object());
    assert!(result["multi_agent"]["conflict_resolution"]["resolved"].is_boolean());
    // B26-S5: memory_graph profile must be present
    assert!(result["memory_graph"].is_object());
    assert!(result["memory_graph"]["drift_detected"].is_boolean());
    // B26-S6: review_adjudication must be structured
    assert!(result["review_adjudication"].is_object());
    assert!(result["review_adjudication"]["adjudication"].is_string());
    assert!(result["review_adjudication"]["evidence_bound"].is_boolean());
    // B26-S7: replay_scoring (3D) must be present
    assert!(result["replay_scoring"].is_object());
    assert!(result["replay_scoring"]["quality_score"].is_number());
    assert!(result["replay_scoring"]["stability_score"].is_number());
    assert!(result["replay_scoring"]["cost_score"].is_number());
    assert!(result["replay_scoring"]["gate_passed"].is_boolean());
}

fn assert_blue22_change_bundle_shape(result: &Value) {
    assert!(result["change_bundle"].is_object());
    assert!(result["change_bundle"]["files"].is_array());
    assert!(result["change_bundle"]["file_change_summary"].is_array());
    assert!(result["change_bundle"]["risk"].is_object());
    assert!(result["change_bundle"]["gate_results"].is_object());
    assert!(result["change_bundle"]["rollback_recommendation"].is_object());
    assert!(result["change_bundle"]["commit_suggestion"].is_object());
    assert!(result["change_bundle"]["rollback"].is_object());
    assert!(result["change_bundle"]["commit"].is_object());
    assert!(result["change_bundle"]["commit_bundle"].is_object());
    assert!(result["change_bundle"]["pr_bundle"].is_object());
    assert!(result["change_bundle"]["test_coverage"].is_object());
}

struct AdvancedRpcHarness {
    inner: RpcHarness,
    mock_responses: std::collections::HashMap<String, Value>,
}

struct TestScenario {
    name: String,
    requests: Vec<Value>,
    expected_outcomes: Vec<ScenarioOutcome>,
}

enum ScenarioOutcome {
    Success,
    ErrorContains(String),
}

fn load_scenarios_from_dir(dir: &Path) -> Vec<TestScenario> {
    let mut entries = fs::read_dir(dir)
        .expect("scenario directory should be readable")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("ndjson"))
        .collect::<Vec<_>>();
    entries.sort();

    entries
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown")
                .to_string();
            let content = fs::read_to_string(&path).expect("scenario file should be readable");
            let requests = content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    serde_json::from_str::<Value>(line)
                        .expect("scenario request should be valid json")
                })
                .collect::<Vec<_>>();
            let expected_outcomes = requests
                .iter()
                .map(|_| ScenarioOutcome::Success)
                .collect::<Vec<_>>();

            TestScenario {
                name,
                requests,
                expected_outcomes,
            }
        })
        .collect()
}

impl AdvancedRpcHarness {
    fn new(config_path: &Path) -> Self {
        Self {
            inner: RpcHarness::spawn(config_path),
            mock_responses: std::collections::HashMap::new(),
        }
    }

    fn register_mock(&mut self, method: &str, response: Value) {
        self.mock_responses.insert(method.to_string(), response);
    }

    fn send_request(&mut self, request: Value) -> Result<Value, String> {
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| "request missing method".to_string())?;

        if let Some(mock) = self.mock_responses.get(method) {
            return Ok(mock.clone());
        }

        let id = request
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("request '{}' missing numeric id", method))?;

        let params = request.get("params").cloned();
        Ok(self.inner.request(id, method, params))
    }

    fn send_concurrent(&mut self, request: Value, n: usize) -> Vec<Result<Value, String>> {
        let method = match request.get("method").and_then(Value::as_str) {
            Some(method) => method.to_string(),
            None => return vec![Err("request missing method".to_string())],
        };
        let params = request.get("params").cloned();
        let start_id = request.get("id").and_then(Value::as_u64).unwrap_or(1_000);

        if let Some(mock) = self.mock_responses.get(&method) {
            return (0..n).map(|_| Ok(mock.clone())).collect();
        }

        for offset in 0..n {
            let payload = json!({
                "jsonrpc": "2.0",
                "id": start_id + offset as u64,
                "method": method,
                "params": params.clone().unwrap_or(Value::Null),
            });
            self.inner.raw_request(&payload);
        }

        (0..n)
            .map(|offset| {
                Ok(self
                    .inner
                    .read_response_for_id(start_id + offset as u64, Duration::from_secs(8)))
            })
            .collect()
    }

    fn run_scenario_file(&mut self, path: &Path) -> Vec<(Value, Result<Value, String>)> {
        let content = fs::read_to_string(path).expect("scenario file should be readable");
        content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let request: Value =
                    serde_json::from_str(line).expect("scenario line should be valid json");
                let result = self.send_request(request.clone());
                (request, result)
            })
            .collect()
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

fn write_managed_service_config(path: &Path, maintenance: u64, health: u64, shutdown: u64) {
    write_test_config(path, maintenance, health, shutdown);
    let mut config = fs::read_to_string(path).expect("failed to read base config");
    let marker = format!("shutdown_drain_seconds = {}\n", shutdown);
    let replacement = format!(
        "shutdown_drain_seconds = {}\ndeployment_target = \"managed-service\"\n",
        shutdown
    );
    config = config.replacen(&marker, &replacement, 1);
    fs::write(path, config).expect("failed to write managed-service config");
}

fn write_unknown_deployment_target_config(path: &Path) {
    write_test_config(path, 60, 120, 5);
    let mut config = fs::read_to_string(path).expect("failed to read base config");
    // Use a deployment_target value that is not in the managed-service inference list.
    config = config.replacen(
        "shutdown_drain_seconds = 5\n",
        "shutdown_drain_seconds = 5\ndeployment_target = \"custom-enterprise-deploy\"\n",
        1,
    );
    fs::write(path, config).expect("failed to write unknown deployment_target config");
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

fn write_shutdown_drain_validation_config(path: &Path) {
    let config = r#"default_phase = "coding"

[flow]
name = "Shutdown Drain Validation"
phases = ["coding"]

[runtime]
maintenance_interval_seconds = 30
health_interval_seconds = 45
shutdown_drain_seconds = 5

[agents.slow_main]
type = "local_slow_approve"

[phases.coding]
description = "Coding"
agents = ["slow_main"]
fallback = true

[phases.coding.options]
request_timeout_seconds = 10
"#;

    fs::write(path, config).expect("failed to write shutdown drain validation config file");
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

fn write_auto_protocol_http_config(path: &Path, bind_addr: &str) {
    let config = format!(
        r#"default_phase = "coding"

[protocol]
mode = "auto"

[flow]
name = "Auto Protocol Coexistence"
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

    fs::write(path, config).expect("failed to write auto protocol coexistence config file");
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

fn http_json_body(response: &str) -> Value {
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b)
        .expect("http response body should exist");
    serde_json::from_str(body).expect("http response body should be json")
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

mod advanced {
    use super::*;

    #[test]
    fn concurrent_requests_return_consistent_responses() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.send_concurrent(
            json!({"jsonrpc":"2.0","method":"runtime.health","id":200}),
            5,
        );

        assert_eq!(results.len(), 5);
        assert!(results.iter().all(|result| result.is_ok()));

        let first = results[0].as_ref().expect("first result should be ok");
        let first_result = &first["result"];
        assert_eq!(first_result["lifecycle"]["is_healthy"], true);
        assert_eq!(first_result["review_gate"]["total"], 0);
        for result in results.iter().skip(1) {
            let response = result.as_ref().expect("concurrent response should be ok");
            let current = &response["result"];
            assert_eq!(
                current["lifecycle"]["is_healthy"],
                first_result["lifecycle"]["is_healthy"]
            );
            assert_eq!(
                current["lifecycle"]["shutting_down"],
                first_result["lifecycle"]["shutting_down"]
            );
            assert_eq!(current["review_gate"], first_result["review_gate"]);
            assert!(
                current["maintenance"]["cycles_total"].as_u64().unwrap_or(0)
                    >= first_result["maintenance"]["cycles_total"]
                        .as_u64()
                        .unwrap_or(0)
            );
        }

        let shutdown = harness.inner.request(299, "shutdown", None);
        assert_eq!(shutdown["result"]["ok"], true);
        harness.inner.wait_for_exit(Duration::from_secs(8));
    }

    #[test]
    fn run_scenario_file_executes_runtime_health_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        harness.register_mock(
            "metrics.prometheus",
            json!({"jsonrpc":"2.0","id":3,"result":{"text":"mocked-prometheus"}}),
        );
        let results = harness.run_scenario_file(Path::new("tests/requests/runtime-health.ndjson"));

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "runtime.health");
        assert_eq!(results[2].0["method"], "metrics.prometheus");
        assert_eq!(
            results[2]
                .1
                .as_ref()
                .expect("mocked scenario response should be ok")["result"]["text"],
            "mocked-prometheus"
        );

        let shutdown = harness.inner.request(399, "shutdown", None);
        assert_eq!(shutdown["result"]["ok"], true);
        harness.inner.wait_for_exit(Duration::from_secs(8));
    }

    #[test]
    fn run_scenario_file_executes_quality_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results =
            harness.run_scenario_file(Path::new("tests/requests/quality-benchmark.ndjson"));

        assert_eq!(results.len(), 5);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "runtime.health");
        assert_eq!(results[2].0["method"], "metrics.get");
        assert_eq!(results[3].0["method"], "trace.metrics");
        assert_eq!(results[4].0["method"], "shutdown");

        let metrics = results[2].1.as_ref().expect("metrics.get should succeed");
        assert!(
            metrics.get("result").is_some(),
            "metrics.get should return result payload"
        );

        let trace = results[3].1.as_ref().expect("trace.metrics should succeed");
        assert!(
            trace["result"].get("timeouts").is_some(),
            "trace.metrics should include timeout benchmark dimensions"
        );
    }

    #[test]
    fn run_scenario_file_executes_model_selector_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results =
            harness.run_scenario_file(Path::new("tests/requests/model-selector-benchmark.ndjson"));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "selector.status");
        assert_eq!(results[2].0["method"], "metrics.get");
        assert_eq!(results[3].0["method"], "shutdown");

        let selector = results[1]
            .1
            .as_ref()
            .expect("selector.status should succeed");
        assert!(
            selector["result"].get("selector").is_some(),
            "selector.status should return selector snapshot"
        );
        assert!(
            selector["result"]["selector"]
                .get("exploration_bias")
                .is_some(),
            "selector.status should include exploration bias"
        );
    }

    #[test]
    fn run_scenario_file_executes_learning_replay_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results =
            harness.run_scenario_file(Path::new("tests/requests/learning-replay-benchmark.ndjson"));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "learning.replay");
        assert_eq!(results[2].0["method"], "learning.summary");
        assert_eq!(results[3].0["method"], "shutdown");

        let replay = results[1]
            .1
            .as_ref()
            .expect("learning.replay should succeed");
        assert!(
            replay["result"].get("replay").is_some(),
            "learning.replay should return replay payload"
        );
        assert!(
            replay["result"]["replay"].get("records").is_some(),
            "learning.replay should include recent records"
        );
    }

    #[test]
    fn run_scenario_file_executes_learning_loop_guardrail_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/learning-loop-guardrail-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "learning.guardrail");
        assert_eq!(results[2].0["method"], "learning.summary");
        assert_eq!(results[3].0["method"], "shutdown");

        let guardrail = results[1]
            .1
            .as_ref()
            .expect("learning.guardrail should succeed");
        assert!(
            guardrail["result"].get("guardrail").is_some(),
            "learning.guardrail should return guardrail payload"
        );
        assert!(
            guardrail["result"]["guardrail"].get("status").is_some(),
            "learning.guardrail should include status"
        );
        assert!(
            guardrail["result"].get("learning_profile").is_some(),
            "learning.guardrail should return learning_profile"
        );
        assert!(
            guardrail["result"].get("knowledge_refinement").is_some(),
            "learning.guardrail should return knowledge_refinement"
        );

        let summary = results[2]
            .1
            .as_ref()
            .expect("learning.summary should succeed");
        assert!(
            summary["result"].get("guardrail").is_some(),
            "learning.summary should embed guardrail payload"
        );
        assert!(
            summary["result"].get("learning_profile").is_some(),
            "learning.summary should return learning_profile"
        );
        assert!(
            summary["result"].get("knowledge_refinement").is_some(),
            "learning.summary should return knowledge_refinement"
        );
    }

    #[test]
    fn run_scenario_file_executes_governance_dynamic_rules_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/governance-dynamic-rules-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 5);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "governance.plan.get");
        assert_eq!(results[2].0["method"], "governance.plan.update");
        assert_eq!(results[3].0["method"], "governance.audit.recent");
        assert_eq!(results[4].0["method"], "shutdown");

        let plan_get = results[1]
            .1
            .as_ref()
            .expect("governance.plan.get should succeed");
        assert!(
            plan_get["result"].get("plan").is_some(),
            "governance.plan.get should return plan payload"
        );

        let plan_update = results[2]
            .1
            .as_ref()
            .expect("governance.plan.update should succeed");
        assert_eq!(
            plan_update["result"]["plan"]["escalation_level"], "L2",
            "governance.plan.update should apply escalation level"
        );

        let audit = results[3]
            .1
            .as_ref()
            .expect("governance.audit.recent should succeed");
        assert!(
            audit["result"]["audit"].get("events").is_some(),
            "governance.audit.recent should include events"
        );
    }

    #[test]
    fn run_scenario_file_executes_breaker_recovery_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/breaker-recovery-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 6);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "breaker.status");
        assert_eq!(results[2].0["method"], "breaker.recovery");
        assert_eq!(results[3].0["method"], "breaker.recovery");
        assert_eq!(results[4].0["method"], "breaker.status");
        assert_eq!(results[5].0["method"], "shutdown");

        let status_before = results[1]
            .1
            .as_ref()
            .expect("breaker.status should succeed");
        assert!(
            status_before["result"].get("degraded_services").is_some(),
            "breaker.status should include degraded services"
        );
        assert!(
            status_before["result"].get("learning_profile").is_some(),
            "breaker.status should have lazy-injected learning_profile"
        );
        assert!(
            status_before["result"]
                .get("knowledge_refinement")
                .is_some(),
            "breaker.status should have lazy-injected knowledge_refinement"
        );

        let dry_run = results[2]
            .1
            .as_ref()
            .expect("breaker.recovery dry-run should succeed");
        assert_eq!(
            dry_run["result"]["dry_run"],
            Value::Bool(true),
            "breaker.recovery dry-run should report dry_run=true"
        );

        let recovery = results[3]
            .1
            .as_ref()
            .expect("breaker.recovery should succeed");
        assert_eq!(
            recovery["result"]["dry_run"],
            Value::Bool(false),
            "breaker.recovery execute should report dry_run=false"
        );
        assert!(
            recovery["result"]
                .get("remaining_degraded_services")
                .is_some(),
            "breaker.recovery should report remaining degraded services"
        );
    }

    #[test]
    fn run_scenario_file_executes_observability_alerts_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/observability-alerts-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 5);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "runtime.health");
        assert_eq!(results[2].0["method"], "health.probes");
        assert_eq!(results[3].0["method"], "observability.alerts");
        assert_eq!(results[4].0["method"], "shutdown");

        let alerts = results[3]
            .1
            .as_ref()
            .expect("observability.alerts should succeed");
        assert!(
            alerts["result"].get("alerts").is_some(),
            "observability.alerts should return alerts summary"
        );
        assert!(
            alerts["result"]["alerts"].get("items").is_some(),
            "observability.alerts should include alert items"
        );
        assert!(
            alerts["result"].get("learning_profile").is_some(),
            "observability.alerts should have lazy-injected learning_profile"
        );
        assert!(
            alerts["result"].get("knowledge_refinement").is_some(),
            "observability.alerts should have lazy-injected knowledge_refinement"
        );
    }

    #[test]
    fn run_scenario_file_executes_runtime_stability_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/runtime-stability-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 5);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "runtime.stability");
        assert_eq!(results[2].0["method"], "config.reload");
        assert_eq!(results[3].0["method"], "runtime.health");
        assert_eq!(results[4].0["method"], "shutdown");

        let stability = results[1]
            .1
            .as_ref()
            .expect("runtime.stability should succeed");
        assert!(
            stability["result"].get("stability").is_some(),
            "runtime.stability should return stability payload"
        );
        assert!(
            stability["result"]["stability"].get("checks").is_some(),
            "runtime.stability should include baseline checks"
        );
        assert!(
            stability["result"]["stability"]
                .get("safe_restart_ready")
                .is_some(),
            "runtime.stability should report safe restart readiness"
        );
    }

    #[test]
    fn run_scenario_file_executes_runtime_self_model_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/runtime-self-model-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0["method"], "initialize");

        assert_eq!(results[1].0["method"], "runtime.self_model");
        assert_eq!(results[2].0["method"], "shutdown");

        let self_model = results[1]
            .1
            .as_ref()
            .expect("runtime.self_model should succeed");
        assert!(
            self_model["result"].get("self_model").is_some(),
            "runtime.self_model should return self_model payload"
        );
        assert!(
            self_model["result"]["self_model"].get("health").is_some(),
            "runtime.self_model should include health summary"
        );
        assert!(
            self_model["result"]["self_model"]
                .get("stability")
                .is_some(),
            "runtime.self_model should include stability summary"
        );
        assert!(
            self_model["result"]["self_model"].get("drift").is_some(),
            "runtime.self_model should include drift summary"
        );
        assert!(
            self_model["result"]["self_model"]
                .get("recommendations")
                .is_some(),
            "runtime.self_model should include recommendations"
        );
        assert!(
            self_model["result"].get("learning_profile").is_some(),
            "runtime.self_model should return learning_profile"
        );
        assert!(
            self_model["result"].get("knowledge_refinement").is_some(),
            "runtime.self_model should return knowledge_refinement"
        );
    }

    #[test]
    fn run_scenario_file_executes_provider_status_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results =
            harness.run_scenario_file(Path::new("tests/requests/provider-status-benchmark.ndjson"));

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "provider.status");
        assert_eq!(results[2].0["method"], "shutdown");

        let provider = results[1]
            .1
            .as_ref()
            .expect("provider.status should succeed");
        assert!(
            provider["result"].get("provider_status").is_some(),
            "provider.status should return provider_status payload"
        );
        assert!(
            provider["result"]["provider_status"]
                .get("summary")
                .is_some(),
            "provider.status should include summary"
        );
        assert!(
            provider["result"]["provider_status"]
                .get("configured_agents")
                .is_some(),
            "provider.status should include configured agent snapshot"
        );
    }

    #[test]
    fn run_scenario_file_executes_security_baseline_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/security-baseline-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 5);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "runtime.health");
        assert_eq!(results[2].0["method"], "security.baseline");
        assert_eq!(results[3].0["method"], "governance.status");
        assert_eq!(results[4].0["method"], "shutdown");

        let baseline = results[2]
            .1
            .as_ref()
            .expect("security.baseline should succeed");
        assert!(
            baseline["result"].get("baseline").is_some(),
            "security.baseline should return baseline payload"
        );
        assert!(
            baseline["result"]["baseline"].get("risks").is_some(),
            "security.baseline should include risk items"
        );
    }

    #[test]
    fn run_scenario_file_executes_harness_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results =
            harness.run_scenario_file(Path::new("tests/requests/harness-benchmark.ndjson"));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "harness.status");
        assert_eq!(results[2].0["method"], "metrics.get");
        assert_eq!(results[3].0["method"], "shutdown");

        let harness_status = results[1]
            .1
            .as_ref()
            .expect("harness.status should succeed");
        assert!(
            harness_status["result"].get("harness").is_some(),
            "harness.status should return harness payload"
        );
        assert!(
            harness_status["result"]["harness"].get("suites").is_some(),
            "harness.status should include suite summary"
        );
    }

    #[test]
    fn run_scenario_file_executes_knowledge_distillation_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/knowledge-distillation-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "learning.replay");
        assert_eq!(results[2].0["method"], "knowledge.distill");
        assert_eq!(results[3].0["method"], "shutdown");

        let distill = results[2]
            .1
            .as_ref()
            .expect("knowledge.distill should succeed");
        assert!(
            distill["result"].get("distillation").is_some(),
            "knowledge.distill should return distillation payload"
        );
        assert!(
            distill["result"]["distillation"]["layers"]
                .get("strategy")
                .is_some(),
            "knowledge.distill should include strategy layer"
        );
        assert!(
            distill["result"].get("learning_profile").is_some(),
            "knowledge.distill should return learning_profile"
        );
        assert!(
            distill["result"].get("knowledge_refinement").is_some(),
            "knowledge.distill should return knowledge_refinement"
        );
    }

    #[test]
    fn run_scenario_file_executes_rl_alignment_offline_eval_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/rl-alignment-offline-eval-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "learning.replay");
        assert_eq!(results[2].0["method"], "rl.alignment.offline_eval");
        assert_eq!(results[3].0["method"], "shutdown");

        let eval = results[2]
            .1
            .as_ref()
            .expect("rl.alignment.offline_eval should succeed");
        assert!(
            eval["result"].get("offline_eval").is_some(),
            "rl.alignment.offline_eval should return offline_eval payload"
        );
        assert!(
            eval["result"]["offline_eval"].get("decision").is_some(),
            "rl.alignment.offline_eval should include decision payload"
        );
        assert!(
            eval["result"].get("learning_profile").is_some(),
            "rl.alignment.offline_eval should return learning_profile"
        );
        assert!(
            eval["result"].get("knowledge_refinement").is_some(),
            "rl.alignment.offline_eval should return knowledge_refinement"
        );
    }

    #[test]
    fn run_scenario_file_executes_hardness_routing_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/hardness-routing-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "hardness.status");
        assert_eq!(results[2].0["method"], "task.execute");
        assert_eq!(results[3].0["method"], "shutdown");

        let hardness = results[1]
            .1
            .as_ref()
            .expect("hardness.status should succeed");
        assert!(
            hardness["result"].get("hardness").is_some(),
            "hardness.status should return hardness payload"
        );
        assert!(
            hardness["result"]["hardness"].get("budget").is_some(),
            "hardness.status should include budget profile"
        );
        assert!(
            hardness["result"].get("learning_profile").is_some(),
            "hardness.status should have lazy-injected learning_profile"
        );
        assert!(
            hardness["result"].get("knowledge_refinement").is_some(),
            "hardness.status should have lazy-injected knowledge_refinement"
        );

        let execution = results[2].1.as_ref().expect("task.execute should succeed");
        assert!(
            execution["result"].get("adaptive").is_some(),
            "task.execute should include adaptive payload"
        );
        assert!(
            execution["result"]["adaptive"]["execution_defaults"]
                .get("hardness")
                .is_some(),
            "task.execute adaptive defaults should include hardness"
        );
    }

    #[test]
    fn run_scenario_file_executes_token_cost_governance_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/token-cost-governance-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "cost.status");
        assert_eq!(results[2].0["method"], "task.execute");
        assert_eq!(results[3].0["method"], "shutdown");

        let cost = results[1].1.as_ref().expect("cost.status should succeed");
        assert!(
            cost["result"].get("cost").is_some(),
            "cost.status should return governance payload"
        );
        assert!(
            cost["result"]["cost"].get("budget").is_some(),
            "cost.status should include budget profile"
        );

        let execution = results[2].1.as_ref().expect("task.execute should succeed");
        assert!(
            execution["result"]["adaptive"]["execution_defaults"]
                .get("cost")
                .is_some(),
            "task.execute adaptive defaults should include cost profile"
        );
    }

    #[test]
    fn run_scenario_file_executes_config_baseline_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results =
            harness.run_scenario_file(Path::new("tests/requests/config-baseline-benchmark.ndjson"));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "config.baseline");
        assert_eq!(results[2].0["method"], "config.reload");
        assert_eq!(results[3].0["method"], "shutdown");

        let baseline = results[1]
            .1
            .as_ref()
            .expect("config.baseline should succeed");
        assert!(
            baseline["result"].get("baseline").is_some(),
            "config.baseline should return baseline payload"
        );
        assert!(
            baseline["result"]["baseline"]
                .get("source_precedence")
                .is_some(),
            "config.baseline should include source precedence"
        );
        assert!(
            baseline["result"]["baseline"].get("migration").is_some(),
            "config.baseline should include migration summary"
        );
        assert!(
            baseline["result"].get("learning_profile").is_some(),
            "config.baseline should have lazy-injected learning_profile"
        );
        assert!(
            baseline["result"].get("knowledge_refinement").is_some(),
            "config.baseline should have lazy-injected knowledge_refinement"
        );
    }

    #[test]
    fn run_scenario_file_executes_error_contract_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results =
            harness.run_scenario_file(Path::new("tests/requests/error-contract-benchmark.ndjson"));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "error.contract");
        assert_eq!(results[2].0["method"], "runtime.health");
        assert_eq!(results[3].0["method"], "shutdown");

        let contract = results[1]
            .1
            .as_ref()
            .expect("error.contract should succeed");
        assert!(
            contract["result"].get("contract").is_some(),
            "error.contract should return contract payload"
        );
        assert!(
            contract["result"]["contract"].get("kinds").is_some(),
            "error.contract should include contract kinds"
        );
        assert!(
            contract["result"]["contract"].get("version").is_some(),
            "error.contract should include contract version"
        );
        assert!(
            contract["result"].get("learning_profile").is_some(),
            "error.contract should have lazy-injected learning_profile"
        );
        assert!(
            contract["result"].get("knowledge_refinement").is_some(),
            "error.contract should have lazy-injected knowledge_refinement"
        );

        let health = results[2]
            .1
            .as_ref()
            .expect("runtime.health should succeed");
        assert!(
            health["result"].get("lifecycle").is_some(),
            "runtime.health should include lifecycle payload"
        );
    }

    #[test]
    fn run_scenario_file_executes_build_repro_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results =
            harness.run_scenario_file(Path::new("tests/requests/build-repro-benchmark.ndjson"));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "build.repro");
        assert_eq!(results[2].0["method"], "runtime.health");
        assert_eq!(results[3].0["method"], "shutdown");

        let build_repro = results[1].1.as_ref().expect("build.repro should succeed");
        assert!(
            build_repro["result"].get("build").is_some(),
            "build.repro should return build payload"
        );
        assert!(
            build_repro["result"]["build"]
                .get("dependency_locks")
                .is_some(),
            "build.repro should include dependency lock metadata"
        );
        assert!(
            build_repro["result"]["build"]
                .get("release_manifest")
                .is_some(),
            "build.repro should include release manifest metadata"
        );
        assert!(
            build_repro["result"].get("learning_profile").is_some(),
            "build.repro should have lazy-injected learning_profile"
        );
        assert!(
            build_repro["result"].get("knowledge_refinement").is_some(),
            "build.repro should have lazy-injected knowledge_refinement"
        );
    }

    #[test]
    fn run_scenario_file_executes_data_lifecycle_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results =
            harness.run_scenario_file(Path::new("tests/requests/data-lifecycle-benchmark.ndjson"));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "data.lifecycle");
        assert_eq!(results[2].0["method"], "runtime.health");
        assert_eq!(results[3].0["method"], "shutdown");

        let lifecycle = results[1]
            .1
            .as_ref()
            .expect("data.lifecycle should succeed");
        assert!(
            lifecycle["result"].get("lifecycle").is_some(),
            "data.lifecycle should return lifecycle payload"
        );
        assert!(
            lifecycle["result"]["lifecycle"].get("policy").is_some(),
            "data.lifecycle should include lifecycle policy"
        );
        assert!(
            lifecycle["result"]["lifecycle"]["storage"]
                .get("waterline")
                .is_some(),
            "data.lifecycle should include storage waterline summary"
        );
        assert!(
            lifecycle["result"].get("learning_profile").is_some(),
            "data.lifecycle should have lazy-injected learning_profile"
        );
        assert!(
            lifecycle["result"].get("knowledge_refinement").is_some(),
            "data.lifecycle should have lazy-injected knowledge_refinement"
        );
    }

    #[test]
    fn run_scenario_file_executes_optimization_peak_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/optimization-peak-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "optimization.peak");
        assert_eq!(results[2].0["method"], "governance.status");
        assert_eq!(results[3].0["method"], "shutdown");

        let peak = results[1]
            .1
            .as_ref()
            .expect("optimization.peak should succeed");
        assert!(
            peak["result"].get("peak").is_some(),
            "optimization.peak should return peak payload"
        );
        assert!(
            peak["result"]["peak"].get("gates").is_some(),
            "optimization.peak should include gate matrix"
        );
        assert!(
            peak["result"]["peak"].get("overall_pass").is_some(),
            "optimization.peak should include overall pass flag"
        );
        assert!(
            peak["result"]["peak"]["indicators"].is_object(),
            "optimization.peak should include benchmark indicators"
        );
        assert!(
            peak["result"]["peak"]["indicators"]["task_success_rate"].is_number(),
            "optimization.peak indicators should include task_success_rate"
        );
        assert!(
            peak["result"]["peak"]["scorecard"].is_object(),
            "optimization.peak should include blue23 scorecard"
        );
        assert!(
            peak["result"]["peak"]["scorecard"]["dimensions"].is_object(),
            "optimization.peak scorecard should include dimensions"
        );
        assert!(
            peak["result"]["peak"]["scorecard"]["dimensions"]["knowledge_refinement_score"]
                .is_number(),
            "optimization.peak scorecard should include knowledge_refinement_score"
        );

        let governance = results[2]
            .1
            .as_ref()
            .expect("governance.status should succeed");
        assert!(
            governance["result"].get("governance").is_some(),
            "governance.status should return governance payload"
        );
        assert_eq!(
            governance["result"]["governance"]["schema_version"], "blue26-governance-v1",
            "governance.status should expose governance schema_version"
        );
        assert_eq!(
            governance["result"]["governance"]["artifact_contract"]["schema_version"],
            "blue26-governance-v1",
            "governance.status should expose artifact contract schema version"
        );
        assert!(
            governance["result"]["governance"]["tool_matrix"]["summary"]["tool_total"].is_number(),
            "governance.status should include tool matrix summary"
        );
        assert!(
            governance["result"]["governance"]["platform_mode"]["active"].is_string(),
            "governance.status should expose platform mode"
        );
        assert!(
            governance["result"]["governance"]["metrics_reconciliation"]["phase_view"].is_object(),
            "governance.status should include phase metrics view"
        );
        assert!(
            governance["result"]["governance"]["metrics_reconciliation"]["universal_view"]
                .is_object(),
            "governance.status should include universal metrics view"
        );
        assert!(
            governance["result"]["governance"]["metrics_reconciliation"]["ok"].is_boolean(),
            "governance.status should include reconciliation status"
        );
        assert!(
            governance["result"]["governance"]["learning_cognition"].is_object(),
            "governance.status should include learning cognition view"
        );
        assert!(
            governance["result"]["governance"]["token_economy"].is_object(),
            "governance.status should include token economy view"
        );
        assert!(
            governance["result"]["governance"]["knowledge_refinement"].is_object(),
            "governance.status should include knowledge refinement view"
        );
        assert!(
            governance["result"]["governance"]["org_policy"]["bundle_version"].is_string(),
            "governance.status should include org policy bundle version"
        );
        assert!(
            governance["result"]["governance"]["org_policy"]["exceptions"]["active_total"]
                .is_number(),
            "governance.status should include org policy exception summary"
        );
        assert!(
            governance["result"]["governance"]["multi_user_server"].is_object(),
            "governance.status should include multi_user_server view"
        );
        assert!(
            governance["result"]["governance"]["multi_user_server"]["tenant_context"]
                ["tenant_id_required"]
                .is_boolean(),
            "governance.status should expose tenant_id requirement flag"
        );
        assert!(
            governance["result"]["governance"]["multi_user_server"]["components"]["authn_authz"]
                ["status"]
                .is_string(),
            "governance.status should include authn/authz component status"
        );
        assert!(
            governance["result"]["governance"]["multi_user_server"]["release_gate"]["ready"]
                .is_boolean(),
            "governance.status should include multi_user release gate readiness"
        );
        assert!(
            governance["result"]["governance"]["multi_user_server"]["inference"]["source"]
                .is_string(),
            "governance.status should include server mode inference source"
        );
        assert!(
            governance["result"]["governance"]["multi_user_server"]["inference"]
                ["deployment_target"]
                .is_string(),
            "governance.status should include server mode deployment target"
        );
        assert!(
            governance["result"]["governance"]["multi_user_server"]["lifecycle"]["ready"]
                .is_boolean(),
            "governance.status should include multi-user lifecycle readiness"
        );
        assert!(
            governance["result"]["governance"]["multi_user_server"]["lifecycle"]["blocking_issues"]
                .is_array(),
            "governance.status should include multi-user lifecycle blocking issues"
        );
        assert!(
            governance["result"]["governance"]["dual_track_consistency"]["ready"].is_boolean(),
            "governance.status should include dual-track consistency readiness"
        );
        assert!(
            governance["result"]["governance"]["dual_track_consistency"]["issues"].is_array(),
            "governance.status should include dual-track consistency issues"
        );
        assert!(
            governance["result"]["governance"]["zero_trust_compliance"].is_object(),
            "governance.status should include zero trust compliance profile"
        );
        assert!(
            governance["result"]["governance"]["rbac_policy_engine"].is_object(),
            "governance.status should include RBAC policy engine profile"
        );
        assert!(
            governance["result"]["governance"]["sla_governance"].is_object(),
            "governance.status should include SLA governance profile"
        );
        assert!(
            governance["result"]["governance"]["skill_engine_core"].is_object(),
            "governance.status should include skill engine core profile"
        );
        assert!(
            governance["result"]["governance"]["workflow_to_skill_conversion"].is_object(),
            "governance.status should include workflow-to-skill conversion profile"
        );
        assert!(
            governance["result"]["governance"]["workflow_skill_chain_integration"].is_object(),
            "governance.status should include workflow-skill chain integration profile"
        );
        assert!(
            governance["result"]["governance"]["skill_management_console"].is_object(),
            "governance.status should include skill management console profile"
        );
        assert!(
            governance["result"]["governance"]["enterprise_skill_controls"].is_object(),
            "governance.status should include enterprise skill controls profile"
        );
        assert!(
            governance["result"]["governance"]["core_mode_consistency"].is_object(),
            "governance.status should include three-mode core consistency profile"
        );
        assert!(
            governance["result"]["governance"]["mode_scenario_adaptability"].is_object(),
            "governance.status should include mode scenario adaptability profile"
        );
        assert!(
            governance["result"]["governance"]["cross_mode_quality_assurance"].is_object(),
            "governance.status should include cross-mode quality assurance profile"
        );
        assert!(
            governance["result"]["governance"]["mode_issue_prevention"].is_object(),
            "governance.status should include mode issue prevention profile"
        );
        assert!(
            governance["result"]["governance"]["subagent_architecture"].is_object(),
            "governance.status should include subagent architecture profile"
        );
        assert!(
            governance["result"]["governance"]["subagent_collaboration"].is_object(),
            "governance.status should include subagent collaboration profile"
        );
        assert!(
            governance["result"]["governance"]["subagent_observability"].is_object(),
            "governance.status should include subagent observability profile"
        );
        assert!(
            governance["result"]["governance"]["knowledge_management"].is_object(),
            "governance.status should include knowledge management profile"
        );
        assert!(
            governance["result"]["governance"]["performance_optimization"].is_object(),
            "governance.status should include performance optimization profile"
        );
        assert!(
            governance["result"]["governance"]["enterprise_deploy_ops"].is_object(),
            "governance.status should include enterprise deploy ops profile"
        );
        assert!(
            governance["result"]["governance"]["ecosystem_extensibility"].is_object(),
            "governance.status should include ecosystem extensibility profile"
        );
        assert!(
            governance["result"]["governance"]["shared_learning_mainchain"].is_object(),
            "governance.status should include shared learning main-chain profile"
        );
        assert!(
            governance["result"]["governance"]["self_evolution_mainchain"].is_object(),
            "governance.status should include self evolution main-chain profile"
        );
        assert!(
            governance["result"]["governance"]["capability_consistency_mainchain"].is_object(),
            "governance.status should include capability consistency main-chain profile"
        );
        assert!(
            governance["result"]["governance"]["shared_learning_data_flow"].is_object(),
            "governance.status should include shared learning data-flow profile"
        );
        assert!(
            governance["result"]["governance"]["self_evolution_flow"].is_object(),
            "governance.status should include self evolution flow profile"
        );
        // BLUE27 S0-S17
        assert!(
            governance["result"]["governance"]["task_graph_persistence"].is_object(),
            "governance.status should include task graph persistence profile"
        );
        assert!(
            governance["result"]["governance"]["evaluation_harness_baseline"].is_object(),
            "governance.status should include evaluation harness baseline profile"
        );
        assert!(
            governance["result"]["governance"]["memory_write_policy"].is_object(),
            "governance.status should include memory write policy profile"
        );
        assert!(
            governance["result"]["governance"]["task_routing_mainchain"].is_object(),
            "governance.status should include task routing main-chain profile"
        );
        assert!(
            governance["result"]["governance"]["tool_budget_enforcement"].is_object(),
            "governance.status should include tool budget enforcement profile"
        );
        assert!(
            governance["result"]["governance"]["state_store_trait"].is_object(),
            "governance.status should include state store trait profile"
        );
        assert!(
            governance["result"]["governance"]["adversarial_verification"].is_object(),
            "governance.status should include adversarial verification profile"
        );
        assert!(
            governance["result"]["governance"]["planner_executor_separation"].is_object(),
            "governance.status should include planner executor separation profile"
        );
        assert!(
            governance["result"]["governance"]["multi_agent_handoff"].is_object(),
            "governance.status should include multi-agent handoff profile"
        );
        assert!(
            governance["result"]["governance"]["evaluation_replay_engine"].is_object(),
            "governance.status should include evaluation replay engine profile"
        );
        assert!(
            governance["result"]["governance"]["trace_model_agent_graph"].is_object(),
            "governance.status should include trace model agent graph profile"
        );
        assert!(
            governance["result"]["governance"]["dynamic_workflow_optimization"].is_object(),
            "governance.status should include dynamic workflow optimization profile"
        );
        assert!(
            governance["result"]["governance"]["think_act_observe_loop"].is_object(),
            "governance.status should include think-act-observe loop profile"
        );
        assert!(
            governance["result"]["governance"]["model_degradation_detection"].is_object(),
            "governance.status should include model degradation detection profile"
        );
        assert!(
            governance["result"]["governance"]["task_decomposition_pipeline"].is_object(),
            "governance.status should include task decomposition pipeline profile"
        );
        assert!(
            governance["result"]["governance"]["omnipotent_mode_readiness"].is_object(),
            "governance.status should include omnipotent mode readiness profile"
        );
        assert!(
            governance["result"]["governance"]["sota_gap_benchmark"].is_object(),
            "governance.status should include SOTA gap benchmark profile"
        );
        assert!(
            governance["result"]["governance"]["blue27_release_closure"].is_object(),
            "governance.status should include blue27 release closure profile"
        );
        // BLUE28 S0-S17
        assert!(
            governance["result"]["governance"]["schema_migration_versioning"].is_object(),
            "governance.status should include schema_migration_versioning profile"
        );
        assert!(
            governance["result"]["governance"]["tenant_auth_api_key"].is_object(),
            "governance.status should include tenant_auth_api_key profile"
        );
        assert!(
            governance["result"]["governance"]["sqlite_postgres_migration"].is_object(),
            "governance.status should include sqlite_postgres_migration profile"
        );
        assert!(
            governance["result"]["governance"]["solution_discovery_hub"].is_object(),
            "governance.status should include solution_discovery_hub profile"
        );
        assert!(
            governance["result"]["governance"]["scenario_matcher"].is_object(),
            "governance.status should include scenario_matcher profile"
        );
        assert!(
            governance["result"]["governance"]["subai_factory"].is_object(),
            "governance.status should include subai_factory profile"
        );
        assert!(
            governance["result"]["governance"]["training_orchestrator"].is_object(),
            "governance.status should include training_orchestrator profile"
        );
        assert!(
            governance["result"]["governance"]["auto_integration_runtime"].is_object(),
            "governance.status should include auto_integration_runtime profile"
        );
        assert!(
            governance["result"]["governance"]["reinforcement_loop"].is_object(),
            "governance.status should include reinforcement_loop profile"
        );
        assert!(
            governance["result"]["governance"]["coordinator_council"].is_object(),
            "governance.status should include coordinator_council profile"
        );
        assert!(
            governance["result"]["governance"]["worker_swarm"].is_object(),
            "governance.status should include worker_swarm profile"
        );
        assert!(
            governance["result"]["governance"]["consensus_engine"].is_object(),
            "governance.status should include consensus_engine profile"
        );
        assert!(
            governance["result"]["governance"]["brain_loop"].is_object(),
            "governance.status should include brain_loop profile"
        );
        assert!(
            governance["result"]["governance"]["node_reputation"].is_object(),
            "governance.status should include node_reputation profile"
        );
        assert!(
            governance["result"]["governance"]["self_model_core"].is_object(),
            "governance.status should include self_model_core profile"
        );
        assert!(
            governance["result"]["governance"]["meta_cognition"].is_object(),
            "governance.status should include meta_cognition profile"
        );
        assert!(
            governance["result"]["governance"]["drift_guard"].is_object(),
            "governance.status should include drift_guard profile"
        );
        assert!(
            governance["result"]["governance"]["blue28_release_closure"].is_object(),
            "governance.status should include blue28 release closure profile"
        );
        assert!(
            governance["result"]["governance"]["federated_rl"].is_object(),
            "governance.status should include federated_rl profile"
        );
        assert!(
            governance["result"]["governance"]["distributed_memory_bus"].is_object(),
            "governance.status should include distributed_memory_bus profile"
        );
        assert!(
            governance["result"]["governance"]["adaptive_swarm_optimizer"].is_object(),
            "governance.status should include adaptive_swarm_optimizer profile"
        );
        assert!(
            governance["result"]["governance"]["hyper_node_network"].is_object(),
            "governance.status should include hyper_node_network profile"
        );
        assert!(
            governance["result"]["governance"]["world_model_pipeline"].is_object(),
            "governance.status should include world_model_pipeline profile"
        );
        assert!(
            governance["result"]["governance"]["continual_learning_hub"].is_object(),
            "governance.status should include continual_learning_hub profile"
        );
        assert!(
            governance["result"]["governance"]["blue29_release_closure"].is_object(),
            "governance.status should include blue29 release closure profile"
        );
        assert!(
            governance["result"]["governance"]["multi_channel_messaging"].is_object(),
            "governance.status should include multi_channel_messaging profile"
        );
        assert!(
            governance["result"]["governance"]["collaboration_game_engine"].is_object(),
            "governance.status should include collaboration_game_engine profile"
        );
        assert!(
            governance["result"]["governance"]["consciousness_proxy_metrics"].is_object(),
            "governance.status should include consciousness_proxy_metrics profile"
        );
        assert!(
            governance["result"]["governance"]["hyper_resilience"].is_object(),
            "governance.status should include hyper_resilience profile"
        );
        assert!(
            governance["result"]["governance"]["dual_track_awakening_parity"].is_object(),
            "governance.status should include dual_track_awakening_parity profile"
        );
        assert!(
            governance["result"]["governance"]["cicd_awareness_gate"].is_object(),
            "governance.status should include cicd_awareness_gate profile"
        );
        assert!(
            governance["result"]["governance"]["blue30_release_closure"].is_object(),
            "governance.status should include blue30 release closure profile"
        );
        assert!(
            governance["result"]["governance"]["autonomy_boundary_governance"].is_object(),
            "governance.status should include autonomy_boundary_governance profile"
        );
        assert!(
            governance["result"]["governance"]["emergency_stop_protocol"].is_object(),
            "governance.status should include emergency_stop_protocol profile"
        );
        assert!(
            governance["result"]["governance"]["collaboration_ab_evaluation"].is_object(),
            "governance.status should include collaboration_ab_evaluation profile"
        );
        assert!(
            governance["result"]["governance"]["hypernode_topology"].is_object(),
            "governance.status should include hypernode_topology profile"
        );
        assert!(
            governance["result"]["governance"]["cross_region_priority_routing"].is_object(),
            "governance.status should include cross_region_priority_routing profile"
        );
        assert!(
            governance["result"]["governance"]["meta_controller_replan"].is_object(),
            "governance.status should include meta_controller_replan profile"
        );
        assert!(
            governance["result"]["governance"]["blue31_release_closure"].is_object(),
            "governance.status should include blue31 release closure profile"
        );
        assert!(
            governance["result"]["governance"]["game_theory_balancer"].is_object(),
            "governance.status should include game_theory_balancer profile"
        );
        assert!(
            governance["result"]["governance"]["federated_rl_v2_guardrail"].is_object(),
            "governance.status should include federated_rl_v2_guardrail profile"
        );
        assert!(
            governance["result"]["governance"]["continuous_learning_distillation"].is_object(),
            "governance.status should include continuous_learning_distillation profile"
        );
        assert!(
            governance["result"]["governance"]["drift_auto_takeover"].is_object(),
            "governance.status should include drift_auto_takeover profile"
        );
        assert!(
            governance["result"]["governance"]["byzantine_fault_injection"].is_object(),
            "governance.status should include byzantine_fault_injection profile"
        );
        assert!(
            governance["result"]["governance"]["recovery_consistency_recheck"].is_object(),
            "governance.status should include recovery_consistency_recheck profile"
        );
        assert!(
            governance["result"]["governance"]["blue32_release_closure"].is_object(),
            "governance.status should include blue32 release closure profile"
        );
        assert!(
            governance["result"]["governance"]["local_reflection_track"].is_object(),
            "governance.status should include local_reflection_track profile"
        );
        assert!(
            governance["result"]["governance"]["server_awakening_track"].is_object(),
            "governance.status should include server_awakening_track profile"
        );
        assert!(
            governance["result"]["governance"]["ci_gate_continuous_green"].is_object(),
            "governance.status should include ci_gate_continuous_green profile"
        );
        assert!(
            governance["result"]["governance"]["staged_rollout_guard"].is_object(),
            "governance.status should include staged_rollout_guard profile"
        );
        assert!(
            governance["result"]["governance"]["release_train_freeze"].is_object(),
            "governance.status should include release_train_freeze profile"
        );
        assert!(
            governance["result"]["governance"]["rollout_audit_replay"].is_object(),
            "governance.status should include rollout_audit_replay profile"
        );
        assert!(
            governance["result"]["governance"]["blue33_release_closure"].is_object(),
            "governance.status should include blue33 release closure profile"
        );
        assert!(
            governance["result"]["governance"]["autonomy_scope_matrix"].is_object(),
            "governance.status should include autonomy_scope_matrix profile"
        );
        assert!(
            governance["result"]["governance"]["redline_policy_runtime"].is_object(),
            "governance.status should include redline_policy_runtime profile"
        );
        assert!(
            governance["result"]["governance"]["human_approval_checkpoint"].is_object(),
            "governance.status should include human_approval_checkpoint profile"
        );
        assert!(
            governance["result"]["governance"]["supernode_hot_standby"].is_object(),
            "governance.status should include supernode_hot_standby profile"
        );
        assert!(
            governance["result"]["governance"]["cross_zone_state_snapshot"].is_object(),
            "governance.status should include cross_zone_state_snapshot profile"
        );
        assert!(
            governance["result"]["governance"]["failover_recovery_drill"].is_object(),
            "governance.status should include failover_recovery_drill profile"
        );
        assert!(
            governance["result"]["governance"]["blue33_remaining_closure"].is_object(),
            "governance.status should include blue33 remaining closure profile"
        );
        assert!(
            governance["result"]["governance"]["dual_track_boundary_freeze"].is_object(),
            "governance.status should include dual_track_boundary_freeze profile"
        );
        assert!(
            governance["result"]["governance"]["state_vector_store_trait_unified"].is_object(),
            "governance.status should include state_vector_store_trait_unified profile"
        );
        assert!(
            governance["result"]["governance"]["local_server_profile_matrix"].is_object(),
            "governance.status should include local_server_profile_matrix profile"
        );
        assert!(
            governance["result"]["governance"]["postgres_pgvector_schema_versioning"].is_object(),
            "governance.status should include postgres_pgvector_schema_versioning profile"
        );
        assert!(
            governance["result"]["governance"]["sqlite_to_pg_migration_dryrun"].is_object(),
            "governance.status should include sqlite_to_pg_migration_dryrun profile"
        );
        assert!(
            governance["result"]["governance"]["planner_executor_taskgraph_resume"].is_object(),
            "governance.status should include planner_executor_taskgraph_resume profile"
        );
        assert!(
            governance["result"]["governance"]["think_act_observe_tool_governance"].is_object(),
            "governance.status should include think_act_observe_tool_governance profile"
        );
        assert!(
            governance["result"]["governance"]["role_handoff_schema_and_conflict_arbiter"]
                .is_object(),
            "governance.status should include role_handoff_schema_and_conflict_arbiter profile"
        );
        assert!(
            governance["result"]["governance"]["deterministic_adversarial_double_checks"]
                .is_object(),
            "governance.status should include deterministic_adversarial_double_checks profile"
        );
        assert!(
            governance["result"]["governance"]["memory_write_promotion_gc_policy"].is_object(),
            "governance.status should include memory_write_promotion_gc_policy profile"
        );
        assert!(
            governance["result"]["governance"]["benchmark_replay_and_3d_scoring"].is_object(),
            "governance.status should include benchmark_replay_and_3d_scoring profile"
        );
        assert!(
            governance["result"]["governance"]["capability_discovery_registry_baseline"]
                .is_object(),
            "governance.status should include capability_discovery_registry_baseline profile"
        );
        assert!(
            governance["result"]["governance"]["staged_rollout_canary_rollback_gate"].is_object(),
            "governance.status should include staged_rollout_canary_rollback_gate profile"
        );
        assert!(
            governance["result"]["governance"]["distributed_node_registry_heartbeat"].is_object(),
            "governance.status should include distributed_node_registry_heartbeat profile"
        );
        assert!(
            governance["result"]["governance"]["consensus_with_dissent_preservation"].is_object(),
            "governance.status should include consensus_with_dissent_preservation profile"
        );
        assert!(
            governance["result"]["governance"]["brain_loop_artifact_and_safe_degrade"].is_object(),
            "governance.status should include brain_loop_artifact_and_safe_degrade profile"
        );
        assert!(
            governance["result"]["governance"]["fault_injection_recovery_recheck"].is_object(),
            "governance.status should include fault_injection_recovery_recheck profile"
        );
        assert!(
            governance["result"]["governance"]["blue34_release_closure"].is_object(),
            "governance.status should include blue34 release closure profile"
        );
    }

    #[test]
    fn run_scenario_file_executes_release_readiness_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/release-readiness-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "release.readiness");
        assert_eq!(results[2].0["method"], "shutdown");

        let readiness = results[1]
            .1
            .as_ref()
            .expect("release.readiness should succeed");
        assert!(
            readiness["result"].get("readiness").is_some(),
            "release.readiness should return readiness payload"
        );
        assert_eq!(
            readiness["result"]["readiness"]["schema_version"], "blue26-release-readiness-v2",
            "release.readiness should expose readiness schema_version"
        );
        assert_eq!(
            readiness["result"]["readiness"]["artifact_contract"]["schema_version"],
            "blue26-release-readiness-v2",
            "release.readiness should expose artifact contract schema version"
        );
        assert!(
            readiness["result"]["readiness"].get("gates").is_some(),
            "release.readiness should include gate matrix"
        );
        assert!(
            readiness["result"]["readiness"]
                .get("overall_pass")
                .is_some(),
            "release.readiness should include overall gate result"
        );
        assert!(
            readiness["result"]["readiness"]["multi_user_server"].is_object(),
            "release.readiness should include multi_user_server summary"
        );
        assert!(
            readiness["result"]["readiness"]["multi_user_server"]["release_gate_ready"]
                .is_boolean(),
            "release.readiness should include multi-user gate ready flag"
        );
        assert!(
            readiness["result"]["readiness"]["multi_user_server"]["inference"]["source"]
                .is_string(),
            "release.readiness should include server mode inference source"
        );
        assert!(
            readiness["result"]["readiness"]["summary"]["multi_user_inference_source"].is_string(),
            "release.readiness summary should include multi-user inference source"
        );
        assert!(
            readiness["result"]["readiness"]["blocked_gate_names"].is_array(),
            "release.readiness should include blocked gate names list"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "multi_user_server"),
            "release.readiness should include multi_user_server gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "dual_track_consistency"),
            "release.readiness should include dual_track_consistency gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "multi_user_lifecycle_ops"),
            "release.readiness should include multi_user_lifecycle_ops gate"
        );
        assert!(
            readiness["result"]["readiness"]["summary"]["multi_user_lifecycle_ready"].is_boolean(),
            "release.readiness summary should include multi-user lifecycle readiness"
        );
        assert!(
            readiness["result"]["readiness"]["summary"]["dual_track_consistency_ready"]
                .is_boolean(),
            "release.readiness summary should include dual-track consistency readiness"
        );
        assert!(
            readiness["result"]["readiness"]["multi_user_server"]["lifecycle"]["ready"]
                .is_boolean(),
            "release.readiness should include lifecycle readiness in multi_user_server"
        );
        assert!(
            readiness["result"]["readiness"]["dual_track_consistency"]["ready"].is_boolean(),
            "release.readiness should include dual-track consistency object"
        );
        assert!(
            readiness["result"]["readiness"]["zero_trust_compliance"].is_object(),
            "release.readiness should include zero trust compliance profile"
        );
        assert!(
            readiness["result"]["readiness"]["rbac_policy_engine"].is_object(),
            "release.readiness should include RBAC policy engine profile"
        );
        assert!(
            readiness["result"]["readiness"]["sla_governance"].is_object(),
            "release.readiness should include SLA governance profile"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "zero_trust_compliance"),
            "release.readiness should include zero_trust_compliance gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "rbac_policy_engine"),
            "release.readiness should include rbac_policy_engine gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "sla_governance"),
            "release.readiness should include sla_governance gate"
        );
        assert!(
            readiness["result"]["readiness"]["skill_engine_core"].is_object(),
            "release.readiness should include skill engine core profile"
        );
        assert!(
            readiness["result"]["readiness"]["workflow_to_skill_conversion"].is_object(),
            "release.readiness should include workflow-to-skill conversion profile"
        );
        assert!(
            readiness["result"]["readiness"]["workflow_skill_chain_integration"].is_object(),
            "release.readiness should include workflow-skill chain integration profile"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "skill_engine_core"),
            "release.readiness should include skill_engine_core gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "workflow_to_skill_conversion"),
            "release.readiness should include workflow_to_skill_conversion gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "workflow_skill_chain_integration"),
            "release.readiness should include workflow_skill_chain_integration gate"
        );
        assert!(
            readiness["result"]["readiness"]["skill_management_console"].is_object(),
            "release.readiness should include skill management console profile"
        );
        assert!(
            readiness["result"]["readiness"]["enterprise_skill_controls"].is_object(),
            "release.readiness should include enterprise skill controls profile"
        );
        assert!(
            readiness["result"]["readiness"]["core_mode_consistency"].is_object(),
            "release.readiness should include three-mode core consistency profile"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "skill_management_console"),
            "release.readiness should include skill_management_console gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "enterprise_skill_controls"),
            "release.readiness should include enterprise_skill_controls gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "core_mode_consistency"),
            "release.readiness should include core_mode_consistency gate"
        );
        assert!(
            readiness["result"]["readiness"]["mode_scenario_adaptability"].is_object(),
            "release.readiness should include mode scenario adaptability profile"
        );
        assert!(
            readiness["result"]["readiness"]["cross_mode_quality_assurance"].is_object(),
            "release.readiness should include cross-mode quality assurance profile"
        );
        assert!(
            readiness["result"]["readiness"]["mode_issue_prevention"].is_object(),
            "release.readiness should include mode issue prevention profile"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "mode_scenario_adaptability"),
            "release.readiness should include mode_scenario_adaptability gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "cross_mode_quality_assurance"),
            "release.readiness should include cross_mode_quality_assurance gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "mode_issue_prevention"),
            "release.readiness should include mode_issue_prevention gate"
        );
        assert!(
            readiness["result"]["readiness"]["subagent_architecture"].is_object(),
            "release.readiness should include subagent architecture profile"
        );
        assert!(
            readiness["result"]["readiness"]["subagent_collaboration"].is_object(),
            "release.readiness should include subagent collaboration profile"
        );
        assert!(
            readiness["result"]["readiness"]["subagent_observability"].is_object(),
            "release.readiness should include subagent observability profile"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "subagent_architecture"),
            "release.readiness should include subagent_architecture gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "subagent_collaboration"),
            "release.readiness should include subagent_collaboration gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "subagent_observability"),
            "release.readiness should include subagent_observability gate"
        );
        assert!(
            readiness["result"]["readiness"]["knowledge_management"].is_object(),
            "release.readiness should include knowledge management profile"
        );
        assert!(
            readiness["result"]["readiness"]["performance_optimization"].is_object(),
            "release.readiness should include performance optimization profile"
        );
        assert!(
            readiness["result"]["readiness"]["enterprise_deploy_ops"].is_object(),
            "release.readiness should include enterprise deploy ops profile"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "knowledge_management"),
            "release.readiness should include knowledge_management gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "performance_optimization"),
            "release.readiness should include performance_optimization gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "enterprise_deploy_ops"),
            "release.readiness should include enterprise_deploy_ops gate"
        );
        assert!(
            readiness["result"]["readiness"]["ecosystem_extensibility"].is_object(),
            "release.readiness should include ecosystem extensibility profile"
        );
        assert!(
            readiness["result"]["readiness"]["shared_learning_mainchain"].is_object(),
            "release.readiness should include shared learning main-chain profile"
        );
        assert!(
            readiness["result"]["readiness"]["self_evolution_mainchain"].is_object(),
            "release.readiness should include self evolution main-chain profile"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "ecosystem_extensibility"),
            "release.readiness should include ecosystem_extensibility gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "shared_learning_mainchain"),
            "release.readiness should include shared_learning_mainchain gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "self_evolution_mainchain"),
            "release.readiness should include self_evolution_mainchain gate"
        );
        assert!(
            readiness["result"]["readiness"]["capability_consistency_mainchain"].is_object(),
            "release.readiness should include capability consistency main-chain profile"
        );
        assert!(
            readiness["result"]["readiness"]["shared_learning_data_flow"].is_object(),
            "release.readiness should include shared learning data-flow profile"
        );
        assert!(
            readiness["result"]["readiness"]["self_evolution_flow"].is_object(),
            "release.readiness should include self evolution flow profile"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "capability_consistency_mainchain"),
            "release.readiness should include capability_consistency_mainchain gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "shared_learning_data_flow"),
            "release.readiness should include shared_learning_data_flow gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "self_evolution_flow"),
            "release.readiness should include self_evolution_flow gate"
        );
        // BLUE27 S0-S17
        assert!(
            readiness["result"]["readiness"]["task_graph_persistence"].is_object(),
            "release.readiness should include task graph persistence profile"
        );
        assert!(
            readiness["result"]["readiness"]["evaluation_harness_baseline"].is_object(),
            "release.readiness should include evaluation harness baseline profile"
        );
        assert!(
            readiness["result"]["readiness"]["memory_write_policy"].is_object(),
            "release.readiness should include memory write policy profile"
        );
        assert!(
            readiness["result"]["readiness"]["task_routing_mainchain"].is_object(),
            "release.readiness should include task routing main-chain profile"
        );
        assert!(
            readiness["result"]["readiness"]["tool_budget_enforcement"].is_object(),
            "release.readiness should include tool budget enforcement profile"
        );
        assert!(
            readiness["result"]["readiness"]["state_store_trait"].is_object(),
            "release.readiness should include state store trait profile"
        );
        assert!(
            readiness["result"]["readiness"]["adversarial_verification"].is_object(),
            "release.readiness should include adversarial verification profile"
        );
        assert!(
            readiness["result"]["readiness"]["planner_executor_separation"].is_object(),
            "release.readiness should include planner executor separation profile"
        );
        assert!(
            readiness["result"]["readiness"]["multi_agent_handoff"].is_object(),
            "release.readiness should include multi-agent handoff profile"
        );
        assert!(
            readiness["result"]["readiness"]["evaluation_replay_engine"].is_object(),
            "release.readiness should include evaluation replay engine profile"
        );
        assert!(
            readiness["result"]["readiness"]["trace_model_agent_graph"].is_object(),
            "release.readiness should include trace model agent graph profile"
        );
        assert!(
            readiness["result"]["readiness"]["dynamic_workflow_optimization"].is_object(),
            "release.readiness should include dynamic workflow optimization profile"
        );
        assert!(
            readiness["result"]["readiness"]["think_act_observe_loop"].is_object(),
            "release.readiness should include think-act-observe loop profile"
        );
        assert!(
            readiness["result"]["readiness"]["model_degradation_detection"].is_object(),
            "release.readiness should include model degradation detection profile"
        );
        assert!(
            readiness["result"]["readiness"]["task_decomposition_pipeline"].is_object(),
            "release.readiness should include task decomposition pipeline profile"
        );
        assert!(
            readiness["result"]["readiness"]["omnipotent_mode_readiness"].is_object(),
            "release.readiness should include omnipotent mode readiness profile"
        );
        assert!(
            readiness["result"]["readiness"]["sota_gap_benchmark"].is_object(),
            "release.readiness should include SOTA gap benchmark profile"
        );
        assert!(
            readiness["result"]["readiness"]["blue27_release_closure"].is_object(),
            "release.readiness should include blue27 release closure profile"
        );
        // BLUE28 S0-S17
        assert!(
            readiness["result"]["readiness"]["schema_migration_versioning"].is_object(),
            "release.readiness should include schema_migration_versioning profile"
        );
        assert!(
            readiness["result"]["readiness"]["tenant_auth_api_key"].is_object(),
            "release.readiness should include tenant_auth_api_key profile"
        );
        assert!(
            readiness["result"]["readiness"]["sqlite_postgres_migration"].is_object(),
            "release.readiness should include sqlite_postgres_migration profile"
        );
        assert!(
            readiness["result"]["readiness"]["solution_discovery_hub"].is_object(),
            "release.readiness should include solution_discovery_hub profile"
        );
        assert!(
            readiness["result"]["readiness"]["scenario_matcher"].is_object(),
            "release.readiness should include scenario_matcher profile"
        );
        assert!(
            readiness["result"]["readiness"]["subai_factory"].is_object(),
            "release.readiness should include subai_factory profile"
        );
        assert!(
            readiness["result"]["readiness"]["training_orchestrator"].is_object(),
            "release.readiness should include training_orchestrator profile"
        );
        assert!(
            readiness["result"]["readiness"]["auto_integration_runtime"].is_object(),
            "release.readiness should include auto_integration_runtime profile"
        );
        assert!(
            readiness["result"]["readiness"]["reinforcement_loop"].is_object(),
            "release.readiness should include reinforcement_loop profile"
        );
        assert!(
            readiness["result"]["readiness"]["coordinator_council"].is_object(),
            "release.readiness should include coordinator_council profile"
        );
        assert!(
            readiness["result"]["readiness"]["worker_swarm"].is_object(),
            "release.readiness should include worker_swarm profile"
        );
        assert!(
            readiness["result"]["readiness"]["consensus_engine"].is_object(),
            "release.readiness should include consensus_engine profile"
        );
        assert!(
            readiness["result"]["readiness"]["brain_loop"].is_object(),
            "release.readiness should include brain_loop profile"
        );
        assert!(
            readiness["result"]["readiness"]["node_reputation"].is_object(),
            "release.readiness should include node_reputation profile"
        );
        assert!(
            readiness["result"]["readiness"]["self_model_core"].is_object(),
            "release.readiness should include self_model_core profile"
        );
        assert!(
            readiness["result"]["readiness"]["meta_cognition"].is_object(),
            "release.readiness should include meta_cognition profile"
        );
        assert!(
            readiness["result"]["readiness"]["drift_guard"].is_object(),
            "release.readiness should include drift_guard profile"
        );
        assert!(
            readiness["result"]["readiness"]["blue28_release_closure"].is_object(),
            "release.readiness should include blue28 release closure profile"
        );
        assert!(
            readiness["result"]["readiness"]["federated_rl"].is_object(),
            "release.readiness should include federated_rl profile"
        );
        assert!(
            readiness["result"]["readiness"]["distributed_memory_bus"].is_object(),
            "release.readiness should include distributed_memory_bus profile"
        );
        assert!(
            readiness["result"]["readiness"]["adaptive_swarm_optimizer"].is_object(),
            "release.readiness should include adaptive_swarm_optimizer profile"
        );
        assert!(
            readiness["result"]["readiness"]["hyper_node_network"].is_object(),
            "release.readiness should include hyper_node_network profile"
        );
        assert!(
            readiness["result"]["readiness"]["world_model_pipeline"].is_object(),
            "release.readiness should include world_model_pipeline profile"
        );
        assert!(
            readiness["result"]["readiness"]["continual_learning_hub"].is_object(),
            "release.readiness should include continual_learning_hub profile"
        );
        assert!(
            readiness["result"]["readiness"]["blue29_release_closure"].is_object(),
            "release.readiness should include blue29 release closure profile"
        );
        assert!(
            readiness["result"]["readiness"]["multi_channel_messaging"].is_object(),
            "release.readiness should include multi_channel_messaging profile"
        );
        assert!(
            readiness["result"]["readiness"]["collaboration_game_engine"].is_object(),
            "release.readiness should include collaboration_game_engine profile"
        );
        assert!(
            readiness["result"]["readiness"]["consciousness_proxy_metrics"].is_object(),
            "release.readiness should include consciousness_proxy_metrics profile"
        );
        assert!(
            readiness["result"]["readiness"]["hyper_resilience"].is_object(),
            "release.readiness should include hyper_resilience profile"
        );
        assert!(
            readiness["result"]["readiness"]["dual_track_awakening_parity"].is_object(),
            "release.readiness should include dual_track_awakening_parity profile"
        );
        assert!(
            readiness["result"]["readiness"]["cicd_awareness_gate"].is_object(),
            "release.readiness should include cicd_awareness_gate profile"
        );
        assert!(
            readiness["result"]["readiness"]["blue30_release_closure"].is_object(),
            "release.readiness should include blue30 release closure profile"
        );
        assert!(
            readiness["result"]["readiness"]["autonomy_boundary_governance"].is_object(),
            "release.readiness should include autonomy_boundary_governance profile"
        );
        assert!(
            readiness["result"]["readiness"]["emergency_stop_protocol"].is_object(),
            "release.readiness should include emergency_stop_protocol profile"
        );
        assert!(
            readiness["result"]["readiness"]["collaboration_ab_evaluation"].is_object(),
            "release.readiness should include collaboration_ab_evaluation profile"
        );
        assert!(
            readiness["result"]["readiness"]["hypernode_topology"].is_object(),
            "release.readiness should include hypernode_topology profile"
        );
        assert!(
            readiness["result"]["readiness"]["cross_region_priority_routing"].is_object(),
            "release.readiness should include cross_region_priority_routing profile"
        );
        assert!(
            readiness["result"]["readiness"]["meta_controller_replan"].is_object(),
            "release.readiness should include meta_controller_replan profile"
        );
        assert!(
            readiness["result"]["readiness"]["blue31_release_closure"].is_object(),
            "release.readiness should include blue31 release closure profile"
        );
        assert!(
            readiness["result"]["readiness"]["game_theory_balancer"].is_object(),
            "release.readiness should include game_theory_balancer profile"
        );
        assert!(
            readiness["result"]["readiness"]["federated_rl_v2_guardrail"].is_object(),
            "release.readiness should include federated_rl_v2_guardrail profile"
        );
        assert!(
            readiness["result"]["readiness"]["continuous_learning_distillation"].is_object(),
            "release.readiness should include continuous_learning_distillation profile"
        );
        assert!(
            readiness["result"]["readiness"]["drift_auto_takeover"].is_object(),
            "release.readiness should include drift_auto_takeover profile"
        );
        assert!(
            readiness["result"]["readiness"]["byzantine_fault_injection"].is_object(),
            "release.readiness should include byzantine_fault_injection profile"
        );
        assert!(
            readiness["result"]["readiness"]["recovery_consistency_recheck"].is_object(),
            "release.readiness should include recovery_consistency_recheck profile"
        );
        assert!(
            readiness["result"]["readiness"]["blue32_release_closure"].is_object(),
            "release.readiness should include blue32 release closure profile"
        );
        assert!(
            readiness["result"]["readiness"]["local_reflection_track"].is_object(),
            "release.readiness should include local_reflection_track profile"
        );
        assert!(
            readiness["result"]["readiness"]["server_awakening_track"].is_object(),
            "release.readiness should include server_awakening_track profile"
        );
        assert!(
            readiness["result"]["readiness"]["ci_gate_continuous_green"].is_object(),
            "release.readiness should include ci_gate_continuous_green profile"
        );
        assert!(
            readiness["result"]["readiness"]["staged_rollout_guard"].is_object(),
            "release.readiness should include staged_rollout_guard profile"
        );
        assert!(
            readiness["result"]["readiness"]["release_train_freeze"].is_object(),
            "release.readiness should include release_train_freeze profile"
        );
        assert!(
            readiness["result"]["readiness"]["rollout_audit_replay"].is_object(),
            "release.readiness should include rollout_audit_replay profile"
        );
        assert!(
            readiness["result"]["readiness"]["blue33_release_closure"].is_object(),
            "release.readiness should include blue33 release closure profile"
        );
        assert!(
            readiness["result"]["readiness"]["autonomy_scope_matrix"].is_object(),
            "release.readiness should include autonomy_scope_matrix profile"
        );
        assert!(
            readiness["result"]["readiness"]["redline_policy_runtime"].is_object(),
            "release.readiness should include redline_policy_runtime profile"
        );
        assert!(
            readiness["result"]["readiness"]["human_approval_checkpoint"].is_object(),
            "release.readiness should include human_approval_checkpoint profile"
        );
        assert!(
            readiness["result"]["readiness"]["supernode_hot_standby"].is_object(),
            "release.readiness should include supernode_hot_standby profile"
        );
        assert!(
            readiness["result"]["readiness"]["cross_zone_state_snapshot"].is_object(),
            "release.readiness should include cross_zone_state_snapshot profile"
        );
        assert!(
            readiness["result"]["readiness"]["failover_recovery_drill"].is_object(),
            "release.readiness should include failover_recovery_drill profile"
        );
        assert!(
            readiness["result"]["readiness"]["blue33_remaining_closure"].is_object(),
            "release.readiness should include blue33 remaining closure profile"
        );
        assert!(
            readiness["result"]["readiness"]["dual_track_boundary_freeze"].is_object(),
            "release.readiness should include dual_track_boundary_freeze profile"
        );
        assert!(
            readiness["result"]["readiness"]["state_vector_store_trait_unified"].is_object(),
            "release.readiness should include state_vector_store_trait_unified profile"
        );
        assert!(
            readiness["result"]["readiness"]["local_server_profile_matrix"].is_object(),
            "release.readiness should include local_server_profile_matrix profile"
        );
        assert!(
            readiness["result"]["readiness"]["postgres_pgvector_schema_versioning"].is_object(),
            "release.readiness should include postgres_pgvector_schema_versioning profile"
        );
        assert!(
            readiness["result"]["readiness"]["sqlite_to_pg_migration_dryrun"].is_object(),
            "release.readiness should include sqlite_to_pg_migration_dryrun profile"
        );
        assert!(
            readiness["result"]["readiness"]["planner_executor_taskgraph_resume"].is_object(),
            "release.readiness should include planner_executor_taskgraph_resume profile"
        );
        assert!(
            readiness["result"]["readiness"]["think_act_observe_tool_governance"].is_object(),
            "release.readiness should include think_act_observe_tool_governance profile"
        );
        assert!(
            readiness["result"]["readiness"]["role_handoff_schema_and_conflict_arbiter"]
                .is_object(),
            "release.readiness should include role_handoff_schema_and_conflict_arbiter profile"
        );
        assert!(
            readiness["result"]["readiness"]["deterministic_adversarial_double_checks"].is_object(),
            "release.readiness should include deterministic_adversarial_double_checks profile"
        );
        assert!(
            readiness["result"]["readiness"]["memory_write_promotion_gc_policy"].is_object(),
            "release.readiness should include memory_write_promotion_gc_policy profile"
        );
        assert!(
            readiness["result"]["readiness"]["benchmark_replay_and_3d_scoring"].is_object(),
            "release.readiness should include benchmark_replay_and_3d_scoring profile"
        );
        assert!(
            readiness["result"]["readiness"]["capability_discovery_registry_baseline"].is_object(),
            "release.readiness should include capability_discovery_registry_baseline profile"
        );
        assert!(
            readiness["result"]["readiness"]["staged_rollout_canary_rollback_gate"].is_object(),
            "release.readiness should include staged_rollout_canary_rollback_gate profile"
        );
        assert!(
            readiness["result"]["readiness"]["distributed_node_registry_heartbeat"].is_object(),
            "release.readiness should include distributed_node_registry_heartbeat profile"
        );
        assert!(
            readiness["result"]["readiness"]["consensus_with_dissent_preservation"].is_object(),
            "release.readiness should include consensus_with_dissent_preservation profile"
        );
        assert!(
            readiness["result"]["readiness"]["brain_loop_artifact_and_safe_degrade"].is_object(),
            "release.readiness should include brain_loop_artifact_and_safe_degrade profile"
        );
        assert!(
            readiness["result"]["readiness"]["fault_injection_recovery_recheck"].is_object(),
            "release.readiness should include fault_injection_recovery_recheck profile"
        );
        assert!(
            readiness["result"]["readiness"]["blue34_release_closure"].is_object(),
            "release.readiness should include blue34 release closure profile"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "task_graph_persistence"),
            "release.readiness should include task_graph_persistence gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "evaluation_harness_baseline"),
            "release.readiness should include evaluation_harness_baseline gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "memory_write_policy"),
            "release.readiness should include memory_write_policy gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "tool_budget_enforcement"),
            "release.readiness should include tool_budget_enforcement gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "adversarial_verification"),
            "release.readiness should include adversarial_verification gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "planner_executor_separation"),
            "release.readiness should include planner_executor_separation gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "multi_agent_handoff"),
            "release.readiness should include multi_agent_handoff gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "think_act_observe_loop"),
            "release.readiness should include think_act_observe_loop gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "omnipotent_mode_readiness"),
            "release.readiness should include omnipotent_mode_readiness gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "blue27_release_closure"),
            "release.readiness should include blue27_release_closure gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "blue28_release_closure"),
            "release.readiness should include blue28_release_closure gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "blue29_release_closure"),
            "release.readiness should include blue29_release_closure gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "blue30_release_closure"),
            "release.readiness should include blue30_release_closure gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "blue31_release_closure"),
            "release.readiness should include blue31_release_closure gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "blue32_release_closure"),
            "release.readiness should include blue32_release_closure gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "blue33_release_closure"),
            "release.readiness should include blue33_release_closure gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "blue33_remaining_closure"),
            "release.readiness should include blue33_remaining_closure gate"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "blue34_release_closure"),
            "release.readiness should include blue34_release_closure gate"
        );
    }

    #[test]
    fn managed_service_target_infers_multi_user_mode_on_main_chain() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_managed_service_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);

        let governance = harness
            .send_request(json!({
                "jsonrpc": "2.0",
                "id": 8801,
                "method": "governance.status",
                "params": {}
            }))
            .expect("governance.status should succeed");
        assert_eq!(
            governance["result"]["governance"]["multi_user_server"]["mode"], "multi_user",
            "managed-service target should infer multi_user mode in governance.status"
        );
        assert_eq!(
            governance["result"]["governance"]["multi_user_server"]["inference"]["source"],
            "deployment_target",
            "governance.status should report deployment_target inference source"
        );
        assert_eq!(
            governance["result"]["governance"]["schema_version"], "blue26-governance-v1",
            "managed-service path should preserve governance schema_version"
        );

        let readiness = harness
            .send_request(json!({
                "jsonrpc": "2.0",
                "id": 8802,
                "method": "release.readiness",
                "params": {}
            }))
            .expect("release.readiness should succeed");
        assert_eq!(
            readiness["result"]["readiness"]["multi_user_server"]["mode"], "multi_user",
            "managed-service target should infer multi_user mode in release.readiness"
        );
        assert_eq!(
            readiness["result"]["readiness"]["multi_user_server"]["inference"]["source"],
            "deployment_target",
            "release.readiness should report deployment_target inference source"
        );
        assert_eq!(
            readiness["result"]["readiness"]["schema_version"], "blue26-release-readiness-v2",
            "managed-service path should preserve readiness schema_version"
        );
        assert!(
            readiness["result"]["readiness"]["multi_user_server"]["lifecycle"]["ready"]
                .is_boolean(),
            "managed-service path should include lifecycle readiness state"
        );
        assert!(
            readiness["result"]["readiness"]["dual_track_consistency"]["ready"].is_boolean(),
            "managed-service path should include dual-track consistency state"
        );
        assert!(
            readiness["result"]["readiness"]["blocked_gate_names"].is_array(),
            "release.readiness should include blocked gate names list in managed-service inference path"
        );
    }

    #[test]
    fn run_scenario_file_executes_release_readiness_drill_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results =
            harness.run_scenario_file(Path::new("tests/requests/release-readiness-drill.ndjson"));

        assert_eq!(results.len(), 6);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "runtime.stability");
        assert_eq!(results[2].0["method"], "security.baseline");
        assert_eq!(results[3].0["method"], "observability.alerts");
        assert_eq!(results[4].0["method"], "optimization.peak");
        assert_eq!(results[5].0["method"], "shutdown");

        let alerts = results[3]
            .1
            .as_ref()
            .expect("observability.alerts should succeed");
        assert!(
            alerts["result"]["alerts"].get("items").is_some(),
            "observability.alerts should include items"
        );
    }

    #[test]
    fn run_scenario_file_executes_multi_user_lifecycle_drill_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_managed_service_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/multi-user-lifecycle-drill.ndjson",
        ));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "governance.status");
        assert_eq!(results[2].0["method"], "release.readiness");
        assert_eq!(results[3].0["method"], "shutdown");

        let governance = results[1]
            .1
            .as_ref()
            .expect("governance.status should succeed");
        assert_eq!(
            governance["result"]["governance"]["multi_user_server"]["mode"], "multi_user",
            "governance.status drill should run in multi_user mode"
        );
        assert!(
            governance["result"]["governance"]["multi_user_server"]["lifecycle"]["ready"]
                .is_boolean(),
            "governance.status drill should expose lifecycle ready flag"
        );
        assert!(
            governance["result"]["governance"]["multi_user_server"]["lifecycle"]["blocking_issues"]
                .is_array(),
            "governance.status drill should expose lifecycle blocking issues"
        );

        let readiness = results[2]
            .1
            .as_ref()
            .expect("release.readiness should succeed");
        assert_eq!(
            readiness["result"]["readiness"]["multi_user_server"]["mode"], "multi_user",
            "release.readiness drill should run in multi_user mode"
        );
        assert!(
            readiness["result"]["readiness"]["summary"]["multi_user_lifecycle_ready"].is_boolean(),
            "release.readiness drill should expose summary lifecycle readiness"
        );
        assert!(
            readiness["result"]["readiness"]["gates"]
                .as_array()
                .expect("gates should be array")
                .iter()
                .any(|gate| gate["name"] == "multi_user_lifecycle_ops"),
            "release.readiness drill should include multi_user_lifecycle_ops gate"
        );
    }

    #[test]
    fn run_scenario_file_executes_lock_status_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results =
            harness.run_scenario_file(Path::new("tests/requests/lock-status-benchmark.ndjson"));

        assert_eq!(results.len(), 5);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "health.probes");
        assert_eq!(results[2].0["method"], "lock.status");
        assert_eq!(results[3].0["method"], "observability.alerts");
        assert_eq!(results[4].0["method"], "shutdown");

        let lock_status = results[2].1.as_ref().expect("lock.status should succeed");
        assert!(
            lock_status["result"]["locks"]
                .get("contention_top")
                .is_some(),
            "lock.status should expose contention_top"
        );
        assert!(
            lock_status["result"]["locks"].get("components").is_some(),
            "lock.status should expose full component snapshots"
        );
    }

    #[test]
    fn run_scenario_file_executes_autotune_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results =
            harness.run_scenario_file(Path::new("tests/requests/autotune-benchmark.ndjson"));

        assert_eq!(results.len(), 5);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "autotune.status");
        assert_eq!(results[2].0["method"], "autotune.get");
        assert_eq!(results[3].0["method"], "runtime.health");
        assert_eq!(results[4].0["method"], "shutdown");

        let status = results[1]
            .1
            .as_ref()
            .expect("autotune.status should succeed");
        assert!(
            status["result"].get("autotune").is_some(),
            "autotune.status should return autotune payload"
        );
        let get = results[2].1.as_ref().expect("autotune.get should succeed");
        assert!(
            get["result"].get("params").is_some() || get["result"].get("autotune").is_some(),
            "autotune.get should return params or autotune payload"
        );
    }

    #[test]
    fn run_scenario_file_executes_maintenance_gc_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results =
            harness.run_scenario_file(Path::new("tests/requests/maintenance-gc-benchmark.ndjson"));

        assert_eq!(results.len(), 5);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "health.probes");
        assert_eq!(results[2].0["method"], "maintenance.gc");
        assert_eq!(results[3].0["method"], "runtime.health");
        assert_eq!(results[4].0["method"], "shutdown");

        let gc = results[2]
            .1
            .as_ref()
            .expect("maintenance.gc should succeed");
        assert!(
            gc["result"].get("ok").is_some() || gc["result"].get("gc").is_some(),
            "maintenance.gc should return ok or gc payload"
        );
    }

    #[test]
    fn run_scenario_file_executes_conversation_checkpoint_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/conversation-checkpoint-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 5);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "conversation.checkpoint.create");
        assert_eq!(results[2].0["method"], "conversation.checkpoint.list");
        assert_eq!(results[3].0["method"], "conversation.checkpoint.prune");
        assert_eq!(results[4].0["method"], "shutdown");

        let created = results[1]
            .1
            .as_ref()
            .expect("checkpoint.create should succeed");
        assert!(
            created["result"].get("checkpoint").is_some() || created["result"].get("ok").is_some(),
            "checkpoint.create should return checkpoint or ok"
        );
        let list = results[2]
            .1
            .as_ref()
            .expect("checkpoint.list should succeed");
        assert!(
            list["result"].get("checkpoints").is_some() || list["result"].get("ok").is_some(),
            "checkpoint.list should return checkpoints or ok"
        );
        let pruned = results[3]
            .1
            .as_ref()
            .expect("checkpoint.prune should succeed");
        assert_eq!(
            pruned["result"]["ok"], true,
            "checkpoint.prune should return ok"
        );
    }

    #[test]
    fn run_scenario_file_executes_primary_secondary_summary_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/primary-secondary-summary-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "primary_secondary.summary");
        assert_eq!(results[2].0["method"], "shutdown");

        let summary = results[1]
            .1
            .as_ref()
            .expect("primary_secondary.summary should succeed");
        assert!(
            summary["result"].get("summary").is_some() || summary["result"].get("ok").is_some(),
            "primary_secondary.summary should return summary or ok"
        );
    }

    #[test]
    fn run_scenario_file_executes_metrics_trace_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results =
            harness.run_scenario_file(Path::new("tests/requests/metrics-trace-benchmark.ndjson"));

        assert_eq!(results.len(), 5);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "metrics.get");
        assert_eq!(results[2].0["method"], "trace.get");
        assert_eq!(results[3].0["method"], "metrics.reset");
        assert_eq!(results[4].0["method"], "shutdown");

        let metrics = results[1].1.as_ref().expect("metrics.get should succeed");
        assert!(
            metrics["result"].get("metrics").is_some() || metrics["result"].get("ok").is_some(),
            "metrics.get should return metrics or ok"
        );
        let trace = results[2].1.as_ref().expect("trace.get should succeed");
        assert!(
            trace["result"].get("trace").is_some()
                || trace["result"].get("events").is_some()
                || trace["result"].get("ok").is_some(),
            "trace.get should return trace, events, or ok"
        );
    }

    #[test]
    fn run_scenario_file_executes_phase_policy_replay_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/phase-policy-replay-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "phase.policy.replay");
        assert_eq!(results[2].0["method"], "shutdown");

        let replay = results[1]
            .1
            .as_ref()
            .expect("phase.policy.replay should succeed");
        assert!(
            replay["result"].get("replay").is_some() || replay["result"].get("ok").is_some(),
            "phase.policy.replay should return replay or ok"
        );
    }

    #[test]
    fn ndjson_scenario_files_all_pass() {
        let scenarios = load_scenarios_from_dir(Path::new("tests/requests"));
        assert_eq!(scenarios.len(), 40, "expected forty request scenario files");

        for scenario in scenarios {
            let temp = tempdir().expect("failed to create temp dir");
            let config_path = temp.path().join("config.toml");
            write_test_config(&config_path, 60, 120, 5);

            let mut harness = AdvancedRpcHarness::new(&config_path);
            let mut saw_shutdown = false;

            for (request, expected) in scenario.requests.iter().zip(&scenario.expected_outcomes) {
                if request.get("method").and_then(Value::as_str) == Some("shutdown") {
                    saw_shutdown = true;
                }

                let result = harness.send_request(request.clone());
                match expected {
                    ScenarioOutcome::Success => {
                        let response = result.unwrap_or_else(|err| {
                            panic!(
                                "scenario '{}' request {:?} failed: {}",
                                scenario.name, request, err
                            )
                        });
                        assert!(
                            response.get("error").is_none() || response["error"].is_null(),
                            "scenario '{}' request {:?} returned error response: {}",
                            scenario.name,
                            request,
                            response
                        );
                    }
                    ScenarioOutcome::ErrorContains(msg) => {
                        let err = result.expect_err("scenario should return expected error");
                        assert!(
                            err.contains(msg),
                            "scenario '{}' expected error containing '{}' but got '{}'",
                            scenario.name,
                            msg,
                            err
                        );
                    }
                }
            }

            if !saw_shutdown {
                let shutdown = harness.inner.request(999, "shutdown", None);
                assert_eq!(
                    shutdown["result"]["ok"], true,
                    "scenario '{}' synthetic shutdown failed",
                    scenario.name
                );
            }
            harness.inner.wait_for_exit(Duration::from_secs(8));
        }
    }

    #[test]
    fn scenario_outcome_error_contains_keeps_error_expectation_path_alive() {
        let expected = ScenarioOutcome::ErrorContains("blocked".to_string());
        match expected {
            ScenarioOutcome::ErrorContains(message) => assert_eq!(message, "blocked"),
            ScenarioOutcome::Success => panic!("expected error outcome variant"),
        }
    }

    // ── B16-R1: debug_panel.get / debug.panel.get ─────────────────────────────
    #[test]
    fn run_scenario_file_executes_debug_panel_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results =
            harness.run_scenario_file(Path::new("tests/requests/debug-panel-benchmark.ndjson"));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "debug_panel.get");
        assert_eq!(results[2].0["method"], "debug.panel.get");
        assert_eq!(results[3].0["method"], "shutdown");

        let panel1 = results[1]
            .1
            .as_ref()
            .expect("debug_panel.get should succeed");
        assert_eq!(
            panel1["result"]["ok"], true,
            "debug_panel.get should return ok"
        );
        assert!(
            panel1["result"].get("panel").is_some(),
            "debug_panel.get should return panel"
        );

        let panel2 = results[2]
            .1
            .as_ref()
            .expect("debug.panel.get should succeed");
        assert_eq!(
            panel2["result"]["ok"], true,
            "debug.panel.get should return ok"
        );
    }

    // ── B16-R2: action.check ──────────────────────────────────────────────────
    #[test]
    fn run_scenario_file_executes_action_check_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results =
            harness.run_scenario_file(Path::new("tests/requests/action-check-benchmark.ndjson"));

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "action.check");
        assert_eq!(results[2].0["method"], "shutdown");

        let check = results[1].1.as_ref().expect("action.check should succeed");
        assert!(
            check["result"].get("report").is_some(),
            "action.check should return report"
        );
    }

    // ── B16-R4 + R5: task.plan + task.execute ────────────────────────────────
    #[test]
    fn run_scenario_file_executes_task_plan_execute_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/task-plan-execute-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "task.plan");
        assert_eq!(results[2].0["method"], "task.execute");
        assert_eq!(results[3].0["method"], "shutdown");

        let plan = results[1].1.as_ref().expect("task.plan should succeed");
        assert!(
            plan["result"].get("plan").is_some() || plan["result"].get("ok").is_some(),
            "task.plan should return plan or ok"
        );
        assert_blue22_execution_cycle_shape(&plan["result"]);
        assert!(plan["result"]["gates"].is_object());
        assert!(plan["result"]["artifacts"].is_object());
        assert!(plan["result"]["run_mode"].is_string());
        assert!(plan["result"]["memory_graph"].is_object());
        assert!(plan["result"]["memory_recall"].is_object());
        assert!(plan["result"]["memory_recall"]["hit_count"].is_number());
        assert!(plan["result"]["memory_recall"]["evidence"].is_array());
        assert_blue22_change_bundle_shape(&plan["result"]);
        assert!(plan["result"]["trace_ref"].is_object());

        let execute = results[2].1.as_ref().expect("task.execute should succeed");
        assert_eq!(
            execute["result"]["ok"], true,
            "task.execute should return ok"
        );
        assert_blue22_runtime_execute_cycle_shape(&execute["result"]);
        assert!(execute["result"]["gates"].is_object());
        assert!(execute["result"]["artifacts"].is_object());
        assert!(execute["result"]["run_mode"].is_string());
        assert_blue22_change_bundle_shape(&execute["result"]);
        assert!(execute["result"]["trace_ref"].is_object());

        let plan_mode = plan["result"]["run_mode"].as_str().unwrap_or_default();
        let execute_mode = execute["result"]["run_mode"].as_str().unwrap_or_default();
        assert!(
            ["manual", "assisted", "autonomous"].contains(&plan_mode),
            "task.plan run_mode must be one of manual/assisted/autonomous"
        );
        assert!(
            ["manual", "assisted", "autonomous"].contains(&execute_mode),
            "task.execute run_mode must be one of manual/assisted/autonomous"
        );
    }

    // ── B16-R6 (standalone workflow.execute) ─────────────────────────────────
    #[test]
    fn run_scenario_file_executes_workflow_execute_standalone_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/workflow-execute-standalone-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "workflow.execute");
        assert_eq!(results[2].0["method"], "shutdown");

        let exe = results[1]
            .1
            .as_ref()
            .expect("workflow.execute should succeed");
        assert_eq!(
            exe["result"]["ok"], true,
            "workflow.execute should return ok"
        );
        assert_blue22_runtime_execute_cycle_shape(&exe["result"]);
        assert!(exe["result"]["gates"].is_object());
        assert!(exe["result"]["artifacts"].is_object());
        assert_blue22_change_bundle_shape(&exe["result"]);
        assert!(exe["result"]["trace_ref"].is_object());
    }

    // ── B16-R7: workflow sub-commands (clarify + research) ────────────────────
    #[test]
    fn run_scenario_file_executes_workflow_subcommands_benchmark_requests() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = AdvancedRpcHarness::new(&config_path);
        let results = harness.run_scenario_file(Path::new(
            "tests/requests/workflow-subcommands-benchmark.ndjson",
        ));

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0["method"], "initialize");
        assert_eq!(results[1].0["method"], "workflow.clarify");
        assert_eq!(results[2].0["method"], "workflow.research");
        assert_eq!(results[3].0["method"], "shutdown");

        let clarify = results[1]
            .1
            .as_ref()
            .expect("workflow.clarify should succeed");
        assert_eq!(
            clarify["result"]["ok"], true,
            "workflow.clarify should return ok"
        );

        let research = results[2]
            .1
            .as_ref()
            .expect("workflow.research should succeed");
        assert_eq!(
            research["result"]["ok"], true,
            "workflow.research should return ok"
        );
        assert_blue22_execution_cycle_shape(&research["result"]);
        assert!(research["result"]["gates"].is_object());
        assert!(research["result"]["artifacts"].is_object());
        assert_blue22_change_bundle_shape(&research["result"]);
        assert!(research["result"]["trace_ref"].is_object());
    }

    // ── B16-R3: conversation.rollback (direct — needs dynamic checkpoint_id) ──
    #[test]
    fn rpc_conversation_rollback_restores_checkpoint() {
        let temp = tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("config.toml");
        write_test_config(&config_path, 60, 120, 5);

        let mut harness = RpcHarness::spawn(&config_path);
        let initialize = harness.request(1, "initialize", None);
        assert_eq!(initialize["result"]["name"], "go-on");

        let created = harness.request(
            2,
            "conversation.checkpoint.create",
            Some(json!({
                "conversation_id": "b16-rollback-test",
                "messages": [{"role": "user", "content": "hello rollback"}]
            })),
        );
        assert_eq!(
            created["result"]["ok"], true,
            "checkpoint.create should succeed"
        );
        let checkpoint_id = created["result"]["checkpoint"]["checkpoint_id"]
            .as_str()
            .expect("create should return checkpoint_id");

        let rollback = harness.request(
            3,
            "conversation.rollback",
            Some(json!({
                "conversation_id": "b16-rollback-test",
                "checkpoint_id": checkpoint_id
            })),
        );
        assert_eq!(
            rollback["result"]["ok"], true,
            "conversation.rollback should succeed"
        );
        assert_eq!(
            rollback["result"]["conversation_id"], "b16-rollback-test",
            "rollback should echo conversation_id"
        );

        let shutdown = harness.request(4, "shutdown", None);
        assert_eq!(shutdown["result"]["ok"], true);
        harness.wait_for_exit(Duration::from_secs(8));
    }
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
    assert!(response.contains("event: telemetry"));
    assert!(response.contains("event: done"));
    assert!(response.contains("event: result"));
    assert!(response.contains("compression_ratio"));

    let knowledge_path = temp
        .path()
        .join(".goon")
        .join("spec")
        .join("latest-knowledge.json");
    let raw = fs::read_to_string(&knowledge_path).expect("knowledge artifact should exist");
    assert!(raw.contains("reusable_insights"));
    assert!(raw.contains("verification_steps"));

    let distillation_path = temp
        .path()
        .join(".goon")
        .join("spec")
        .join("latest-session-distillation.json");
    let distillation_raw =
        fs::read_to_string(&distillation_path).expect("session distillation artifact should exist");
    assert!(distillation_raw.contains("learning_profile"));
    assert!(distillation_raw.contains("knowledge_refinement"));

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn http_chat_completions_updates_health_metrics_and_emits_latency_log() {
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
        .arg("--verbose")
        .env("GO_ON_ENABLE_LOCAL_TEST_AGENTS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn go-on for HTTP completions metric test");

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

    let completion_body = json!({
        "model": "local-echo",
        "messages": [{"role": "user", "content": "health metric smoke"}],
        "stream": false
    })
    .to_string();

    let completion_response = http_request(
        &bind_addr,
        "POST",
        "/v1/chat/completions",
        Some(&completion_body),
    )
    .expect("HTTP chat completions request should succeed");
    assert!(completion_response.contains("HTTP/1.1 200 OK"));

    let health_started = Instant::now();
    let mut seen_total = 0_u64;
    loop {
        if health_started.elapsed() > Duration::from_secs(5) {
            break;
        }
        if let Ok(response) = http_request(&bind_addr, "GET", "/health", None) {
            if response.contains("200 OK") {
                let body = http_json_body(&response);
                seen_total = body["metrics"]["total_requests"].as_u64().unwrap_or(0);
                if seen_total >= 1 {
                    break;
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        seen_total >= 1,
        "expected /health metrics.total_requests >= 1 after completion request"
    );

    let _ = child.kill();
    let _ = child.wait();

    let mut stderr_text = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut stderr_text);
    }
    assert!(
        stderr_text.contains("HTTP /v1/chat/completions completed in"),
        "expected latency log in stderr, got: {stderr_text}"
    );
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
fn rpc_auto_mode_http_root_acp_and_mcp_coexist() {
    // Session A: auto mode HTTP root capability assertions.
    let temp_http = tempdir().expect("failed to create temp dir");
    let config_http = temp_http.path().join("config.toml");
    let bind_addr = format!("127.0.0.1:{}", find_free_port());
    write_auto_protocol_http_config(&config_http, &bind_addr);

    let mut http_child = Command::new(binary_path())
        .arg("--config")
        .arg(&config_http)
        .env("GO_ON_ENABLE_LOCAL_TEST_AGENTS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn go-on for auto HTTP protocol test");

    let started = Instant::now();
    loop {
        if started.elapsed() > Duration::from_secs(5) {
            let _ = http_child.kill();
            panic!("timed out waiting for ACP HTTP server in auto protocol mode");
        }
        if let Ok(response) = http_request(&bind_addr, "GET", "/health", None) {
            if response.contains("200 OK") {
                break;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }

    let root = http_request(&bind_addr, "GET", "/", None).expect("GET / should succeed");
    assert!(root.contains("200 OK"));
    assert!(root.contains("\"service\":\"go-on\""));
    assert!(root.contains("\"protocol\":\"acp-http\""));
    assert!(root.contains("\"responses\":[\"/v1/responses\",\"/v1/responses/{id}\"]"));

    let _ = http_child.kill();
    let _ = http_child.wait();

    // Session B: ACP + MCP adapter coexistence assertions in RPC stdio path.
    let temp_rpc = tempdir().expect("failed to create temp dir");
    let config_rpc = temp_rpc.path().join("config.toml");
    write_test_config(&config_rpc, 60, 120, 5);

    let mut harness = RpcHarness::spawn(&config_rpc);
    let initialize = harness.request(1, "initialize", None);
    assert_eq!(initialize["result"]["protocol"], "acp");
    assert_eq!(initialize["result"]["capabilities"]["mcp_adapter"], true);

    let mcp_init = harness.request(2, "mcp.initialize", Some(json!({})));
    assert_eq!(mcp_init["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(mcp_init["result"]["serverInfo"]["name"], "go-on");

    let shutdown = harness.request(3, "shutdown", None);
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
    assert_eq!(reload["result"]["warning_count"], 1);
    assert_eq!(reload["result"]["profile_recommendation"], "balanced");
    assert!(reload["result"]["recommendations"].is_array());
    assert_eq!(reload["result"]["health"]["score"], 85);
    assert_eq!(reload["result"]["health"]["critical_count"], 0);
    assert_eq!(reload["result"]["health"]["warn_count"], 1);
    assert_eq!(reload["result"]["health"]["info_count"], 0);
    assert!(reload["result"]["warnings"]
        .as_array()
        .expect("warnings should be array")
        .iter()
        .filter_map(|warning| warning.as_str())
        .any(|warning| warning.contains("runtime.production_strict=false")));

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
    assert_blue22_execution_cycle_shape(&plan["result"]);
    assert!(plan["result"]["gates"].is_object());
    assert!(plan["result"]["artifacts"].is_object());
    assert_blue22_change_bundle_shape(&plan["result"]);
    assert!(plan["result"]["trace_ref"].is_object());

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
    assert_blue22_execution_cycle_shape(&workflow["result"]);
    assert!(workflow["result"]["gates"].is_object());
    assert!(workflow["result"]["artifacts"].is_object());
    assert_blue22_change_bundle_shape(&workflow["result"]);
    assert!(workflow["result"]["trace_ref"].is_object());

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
    // The exact text depends on whether i18n is initialized:
    // - When i18n is active: "jsonrpc must be 2.0"
    // - When i18n fallback (key): "error.jsonrpc_must_be_2_0"
    // Either is acceptable — what matters is the error code.
    assert!(
        message.contains("2.0") || message.contains("jsonrpc_must_be_2_0"),
        "error message should reference jsonrpc 2.0 requirement; got: {message}"
    );

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
fn rpc_shutdown_waits_for_inflight_chat_completion() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_shutdown_drain_validation_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(91, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    harness.raw_request(&json!({
        "jsonrpc": "2.0",
        "id": 92,
        "method": "chat",
        "params": {
            "messages": [{"role": "user", "content": "shutdown drain validation"}],
            "mode": "ask"
        }
    }));

    harness.raw_request(&json!({
        "jsonrpc": "2.0",
        "id": 93,
        "method": "shutdown"
    }));

    let chat = harness.read_response_for_id(92, Duration::from_secs(12));
    assert_eq!(chat["result"]["done"], true);
    assert!(
        chat["result"].get("agent").is_some() || chat["result"].get("response").is_some(),
        "chat should complete and include agent or response payload before shutdown"
    );

    let shutdown = harness.read_response_for_id(93, Duration::from_secs(8));
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

    #[cfg(all(
        feature = "profile-local",
        not(feature = "profile-simple-server"),
        not(feature = "profile-multi-users-server")
    ))]
    {
        assert!(
            output.status.success(),
            "profile-local should degrade gracefully when sqlite paths are unavailable"
        );
        let stderr_text = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr_text.contains("continuing without cache")
                || stderr_text.contains("continuing without vector")
                || stderr_text.contains("sqlite")
        );
    }

    #[cfg(not(all(
        feature = "profile-local",
        not(feature = "profile-simple-server"),
        not(feature = "profile-multi-users-server")
    )))]
    {
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

// B26-S11: task.execute must return task_graph_checkpoint with checkpoint_id + resume_eligible
#[test]
fn task_execute_returns_task_graph_checkpoint() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_workflow_governance_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(200, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let execute = harness.request(
        201,
        "task.execute",
        Some(json!({
            "task": "implement login feature",
            "requirement_confirmed": true,
            "requirement_contract": {
                "goal": "Add user login capability",
                "scope": "auth module",
                "acceptance_criteria": ["user can log in"],
                "constraints": ["must be secure"],
                "user_confirmed": true
            }
        })),
    );
    let result = &execute["result"];
    assert_eq!(result["ok"], true, "task.execute should succeed");
    assert_blue22_runtime_execute_cycle_shape(result);

    let ckpt = &result["execution_cycle"]["task_graph_checkpoint"];
    assert!(
        ckpt["checkpoint_id"].is_string(),
        "checkpoint_id must be string"
    );
    assert!(
        ckpt["resume_eligible"].is_boolean(),
        "resume_eligible must be boolean"
    );
    assert!(
        ckpt["phases_completed"].is_number(),
        "phases_completed must be number"
    );
    assert_eq!(
        ckpt["schema_version"], "blue26-taskgraph-checkpoint-v1",
        "schema_version must be blue26-taskgraph-checkpoint-v1"
    );

    let shutdown = harness.request(202, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

// B26-S12: task.execute must return tool_loop (think-act-observe) in execution_cycle
#[test]
fn task_execute_returns_tool_loop_safety_governance() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_workflow_governance_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(1200, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let execute = harness.request(
        1201,
        "task.execute",
        Some(json!({
            "task": "implement search feature",
            "requirement_confirmed": true,
            "requirement_contract": {
                "goal": "Add search functionality",
                "scope": "search module",
                "acceptance_criteria": ["user can search"],
                "constraints": ["must be fast"],
                "user_confirmed": true
            }
        })),
    );
    let result = &execute["result"];
    assert_eq!(result["ok"], true, "task.execute should succeed");

    let tl = &result["execution_cycle"]["tool_loop"];
    assert!(tl.is_object(), "tool_loop must be object");
    assert_eq!(tl["schema_version"], "blue26-tool-loop-v1");
    assert!(tl["phase"].is_string(), "tool_loop.phase must be string");
    assert!(
        tl["safety_gate_passed"].is_boolean(),
        "safety_gate_passed must be boolean"
    );
    assert!(tl["governance"].is_object(), "governance must be object");
    assert!(tl["governance"]["dangerous_ops_intercepted"].is_number());
    assert!(tl["governance"]["whitelist_bypass_count"].is_number());

    let shutdown = harness.request(1202, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

// B26-S13: task.execute must return handoff_protocol + conflict_resolution in multi_agent
#[test]
fn task_execute_returns_role_handoff_conflict_resolution() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_workflow_governance_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    let initialize = harness.request(1300, "initialize", None);
    assert_eq!(initialize["result"]["name"], "go-on");

    let execute = harness.request(
        1301,
        "task.execute",
        Some(json!({
            "task": "refactor authentication module",
            "requirement_confirmed": true,
        })),
    );
    let result = &execute["result"];
    assert_eq!(result["ok"], true, "task.execute should succeed");

    let hp = &result["multi_agent"]["handoff_protocol"];
    assert!(hp.is_object(), "handoff_protocol must be object");
    assert_eq!(hp["schema_version"], "blue26-handoff-v1");
    assert!(hp["objective_transfer"].is_boolean());
    assert!(hp["total_handoffs"].is_number());

    let cr = &result["multi_agent"]["conflict_resolution"];
    assert!(cr.is_object(), "conflict_resolution must be object");
    assert_eq!(cr["schema_version"], "blue26-conflict-resolution-v1");
    assert!(cr["resolved"].is_boolean());
    assert!(cr["adjudicator"].is_string());

    let shutdown = harness.request(1302, "shutdown", None);
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
    assert_blue22_runtime_execute_cycle_shape(&execute["result"]);
    assert!(execute["result"]["gates"].is_object());
    assert!(execute["result"]["artifacts"].is_object());
    assert_blue22_change_bundle_shape(&execute["result"]);
    assert!(execute["result"]["trace_ref"].is_object());
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
            "consultation_confidence_threshold": 0.5,
            "requirement_contract": {
                "goal": "produce a governed migration consultation artifact with a rollback-ready consensus plan",
                "scope": "migration approach, evidence expectations, rollback path, and operator handoff",
                "acceptance_criteria": [
                    "consultation artifact is persisted",
                    "consensus plan is returned",
                    "rollback and evidence concerns are covered"
                ],
                "constraints": [
                    "must preserve backward compatibility",
                    "must keep audit evidence available"
                ],
                "user_confirmed": true
            },
            "requirement_confirmed": true
        })),
    );
    assert_eq!(consult["result"]["ok"], true);
    assert!(consult["result"]["artifact"].is_object());
    assert!(consult["result"]["artifact_path"].is_string());
    assert!(consult["result"]["artifact"]["participants"].is_array());
    assert!(consult["result"]["artifact"]["consensus_plan"].is_string());
    assert_blue22_execution_cycle_shape(&consult["result"]);
    assert!(consult["result"]["gates"].is_object());
    assert!(consult["result"]["artifacts"].is_object());
    assert_blue22_change_bundle_shape(&consult["result"]);
    assert!(consult["result"]["trace_ref"].is_object());

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
            "research_focus": "impact analysis, migration risk, and rollback plan",
            "requirement_contract": {
                "goal": "produce a governed research artifact that explains impact, migration risk, and rollout guidance",
                "scope": "cross-module audit evidence changes, persistence implications, and rollback planning",
                "acceptance_criteria": [
                    "research artifact is persisted",
                    "recommended plan is returned",
                    "risk and rollback guidance are included"
                ],
                "constraints": [
                    "must not change runtime behavior during research",
                    "must keep outputs compatible with current workflow artifacts"
                ],
                "user_confirmed": true
            },
            "requirement_confirmed": true
        })),
    );
    assert_eq!(research["result"]["ok"], true);
    assert!(research["result"]["artifact"].is_object());
    assert!(research["result"]["artifact"]["planner_output"].is_string());
    assert!(research["result"]["artifact"]["researcher_output"].is_string());
    assert!(research["result"]["artifact"]["reviewer_output"].is_string());
    assert!(research["result"]["artifact"]["recommended_plan"].is_string());
    assert!(research["result"]["planned_subtasks"].is_number());
    assert_blue22_execution_cycle_shape(&research["result"]);
    assert!(research["result"]["gates"].is_object());
    assert!(research["result"]["artifacts"].is_object());
    assert_blue22_change_bundle_shape(&research["result"]);
    assert!(research["result"]["trace_ref"].is_object());

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

// ---------------------------------------------------------------------------
// BLUE24 — AI meta-cognition, token economy v2, knowledge refinement v2
// ---------------------------------------------------------------------------

/// Verify meta_cognition block is present and well-formed in learning_profile.
/// Uses the existing task-plan-execute benchmark to ensure proper request ordering.
#[test]
fn blue24_learning_profile_has_meta_cognition_block() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);
    let mut harness = AdvancedRpcHarness::new(&config_path);

    let results = harness.run_scenario_file(Path::new(
        "tests/requests/task-plan-execute-benchmark.ndjson",
    ));
    assert_eq!(results.len(), 4);

    // task.plan — planning-class
    let plan = results[1].1.as_ref().expect("task.plan should succeed");
    let plan_lp = &plan["result"]["learning_profile"];
    assert!(
        plan_lp.is_object(),
        "task.plan must return learning_profile"
    );
    assert_eq!(
        plan_lp["schema_version"], "blue24-learning-profile-v2",
        "learning_profile schema_version must be blue24-learning-profile-v2"
    );
    assert!(
        plan_lp["meta_cognition"].is_object(),
        "learning_profile must contain meta_cognition block"
    );
    assert!(
        plan_lp["meta_cognition"]["reflection_depth"].is_string(),
        "meta_cognition.reflection_depth must be a string"
    );
    assert!(
        plan_lp["meta_cognition"]["strategy_evaluation"]["current_strategy"].is_string(),
        "meta_cognition.strategy_evaluation.current_strategy must be a string"
    );
    assert!(
        plan_lp["meta_cognition"]["strategy_evaluation"]["adaptation_signal"].is_string(),
        "meta_cognition.strategy_evaluation.adaptation_signal must be a string"
    );
    assert!(
        plan_lp["meta_cognition"]["self_improvement"]["bottleneck_awareness"].is_boolean(),
        "meta_cognition.self_improvement.bottleneck_awareness must be boolean"
    );
    assert!(
        plan_lp["meta_cognition"]["cognitive_load_estimate"].is_string(),
        "meta_cognition.cognitive_load_estimate must be a string"
    );
    assert_eq!(
        plan_lp["meta_cognition"]["awareness_level"], "operational",
        "meta_cognition.awareness_level must be 'operational'"
    );
    assert_eq!(
        plan_lp["meta_cognition"]["reflection_depth"], "standard",
        "task.plan meta_cognition.reflection_depth must be 'standard'"
    );

    // task.execute — execution-class; reflection_depth must be "deep"
    let execute = results[2].1.as_ref().expect("task.execute should succeed");
    let exec_lp = &execute["result"]["learning_profile"];
    assert!(
        exec_lp.is_object(),
        "task.execute must return learning_profile"
    );
    assert_eq!(
        exec_lp["meta_cognition"]["reflection_depth"], "deep",
        "task.execute meta_cognition.reflection_depth must be 'deep'"
    );
}

/// Verify token_economy v2 has dynamic compression fields.
/// Uses the existing task-plan-execute benchmark.
#[test]
fn blue24_token_economy_has_dynamic_compression() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);
    let mut harness = AdvancedRpcHarness::new(&config_path);

    let results = harness.run_scenario_file(Path::new(
        "tests/requests/task-plan-execute-benchmark.ndjson",
    ));
    assert_eq!(results.len(), 4);

    let execute = results[2].1.as_ref().expect("task.execute should succeed");
    let te = &execute["result"]["token_economy"];
    assert!(te.is_object(), "task.execute must return token_economy");
    assert_eq!(
        te["schema_version"], "blue24-token-economy-v2",
        "token_economy schema_version must be blue24-token-economy-v2"
    );
    assert!(
        te["budget"]["per_round_budget"].is_number(),
        "token_economy.budget must include per_round_budget"
    );
    assert!(
        te["compression"].is_object(),
        "token_economy must include compression block"
    );
    assert!(
        te["compression"]["level"].is_string(),
        "token_economy.compression.level must be a string"
    );
    assert!(
        te["compression"]["task_complexity"].is_string(),
        "token_economy.compression.task_complexity must be a string"
    );
    assert!(
        te["optimization"]["cumulative_saving_estimate_tokens"].is_number(),
        "token_economy.optimization must include cumulative_saving_estimate_tokens"
    );
    assert!(
        te["multi_round_strategy"]["cross_round_kv_cache"].is_boolean(),
        "token_economy.multi_round_strategy must include cross_round_kv_cache"
    );
}

/// Verify knowledge_refinement v2 has cross_round distillation block.
/// Uses the existing task-plan-execute benchmark.
#[test]
fn blue24_knowledge_refinement_has_cross_round_distillation() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);
    let mut harness = AdvancedRpcHarness::new(&config_path);

    let results = harness.run_scenario_file(Path::new(
        "tests/requests/task-plan-execute-benchmark.ndjson",
    ));
    assert_eq!(results.len(), 4);

    let execute = results[2].1.as_ref().expect("task.execute should succeed");
    let kr = &execute["result"]["knowledge_refinement"];
    assert!(
        kr.is_object(),
        "task.execute must return knowledge_refinement"
    );
    assert_eq!(
        kr["schema_version"], "blue24-knowledge-refinement-v2",
        "knowledge_refinement schema_version must be blue24-knowledge-refinement-v2"
    );
    assert!(
        kr["cross_round"].is_object(),
        "knowledge_refinement must include cross_round block"
    );
    assert!(
        kr["cross_round"]["stale_knowledge_detection"].is_boolean(),
        "cross_round.stale_knowledge_detection must be boolean"
    );
    assert!(
        kr["cross_round"]["staleness_risk"].is_string(),
        "cross_round.staleness_risk must be a string"
    );
    assert!(
        kr["cross_round"]["writeback_on_convergence"].is_boolean(),
        "cross_round.writeback_on_convergence must be boolean"
    );
    let confidence = kr["self_evolution"]["confidence"]
        .as_f64()
        .expect("knowledge_refinement.self_evolution.confidence must be a float");
    assert!(
        (0.0..=1.0).contains(&confidence),
        "confidence must be in [0.0, 1.0]; got {confidence}"
    );
}
/// Verify runtime.self_model now includes a meta_cognition block.
#[test]
fn blue24_self_model_has_meta_cognition_block() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);
    let mut harness = RpcHarness::spawn(&config_path);

    let init = harness.request(330, "initialize", None);
    assert_eq!(init["result"]["protocol"], "acp");

    let result = harness.request(331, "runtime.self_model", None);
    let sm = &result["result"]["self_model"];
    assert!(
        sm.is_object(),
        "runtime.self_model result must include self_model"
    );
    assert!(
        sm["meta_cognition"].is_object(),
        "self_model must contain meta_cognition block"
    );
    assert!(
        sm["meta_cognition"]["self_consistency_score"].is_number(),
        "meta_cognition.self_consistency_score must be a number"
    );
    let score = sm["meta_cognition"]["self_consistency_score"]
        .as_f64()
        .expect("self_consistency_score must be a float");
    assert!(
        (0.0..=1.0).contains(&score),
        "self_consistency_score must be in [0.0, 1.0]; got {score}"
    );
    assert!(
        sm["meta_cognition"]["goal_stability"].is_string(),
        "meta_cognition.goal_stability must be a string"
    );
    assert!(
        sm["meta_cognition"]["capability_boundary"]["known_limits"].is_array(),
        "capability_boundary.known_limits must be an array"
    );
    assert!(
        sm["meta_cognition"]["metacognitive_loop"]["active"].is_boolean(),
        "metacognitive_loop.active must be boolean"
    );
    assert!(
        sm["meta_cognition"]["world_model"]["runtime_state_known"].is_boolean(),
        "world_model.runtime_state_known must be boolean"
    );
    assert_eq!(
        sm["meta_cognition"]["schema_version"], "blue24-self-model-meta-cognition-v1",
        "meta_cognition.schema_version must be blue24-self-model-meta-cognition-v1"
    );

    let shutdown = harness.request(332, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

// ── BLUE26 S14: adversarial / negative-path tests ─────────────────────────────
// These tests verify that the system handles unexpected, invalid, or edge-case
// inputs robustly — a prerequisite for the deterministic+adversarial dual-track gate.

#[test]
fn adversarial_unknown_deployment_target_defaults_to_single_user_mode() {
    // A deployment_target value not in the managed-service recognition list must
    // result in single_user mode for both governance.status and release.readiness.
    // This prevents unintended privilege escalation via mis-typed deployment targets.
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_unknown_deployment_target_config(&config_path);

    let mut harness = RpcHarness::spawn(&config_path);
    harness.request(9901, "initialize", None);

    let governance = harness.request(9902, "governance.status", None);
    let gov_mode = governance["result"]["governance"]["multi_user_server"]["mode"]
        .as_str()
        .expect("governance multi_user_server.mode should be string");
    assert_eq!(
        gov_mode, "single_user",
        "unknown deployment_target must default to single_user in governance.status"
    );
    let gov_source = governance["result"]["governance"]["multi_user_server"]["inference"]["source"]
        .as_str()
        .expect("inference.source should be string");
    assert_eq!(
        gov_source, "default",
        "unknown deployment_target should yield inference.source=default"
    );

    let readiness = harness.request(9903, "release.readiness", None);
    let read_mode = readiness["result"]["readiness"]["multi_user_server"]["mode"]
        .as_str()
        .expect("readiness multi_user_server.mode should be string");
    assert_eq!(
        read_mode, "single_user",
        "unknown deployment_target must default to single_user in release.readiness"
    );

    let shutdown = harness.request(9904, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn adversarial_explicit_single_user_param_overrides_managed_service_inference() {
    // Even when deployment_target=managed-service would normally infer multi_user,
    // an explicit server_mode=single_user param in the request must take precedence.
    // This verifies that explicit request intent always wins over environment inference.
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_managed_service_config(&config_path, 60, 120, 5);

    let mut harness = RpcHarness::spawn(&config_path);
    harness.request(9910, "initialize", None);

    // Without override: managed-service should infer multi_user.
    let governance_inferred = harness.request(9911, "governance.status", None);
    assert_eq!(
        governance_inferred["result"]["governance"]["multi_user_server"]["mode"], "multi_user",
        "managed-service config should infer multi_user by default"
    );

    // With explicit single_user override: must respect the explicit param.
    let governance_override = harness.request(
        9912,
        "governance.status",
        Some(json!({ "server_mode": "single_user" })),
    );
    let override_mode = governance_override["result"]["governance"]["multi_user_server"]["mode"]
        .as_str()
        .expect("overridden mode should be string");
    assert_eq!(
        override_mode, "single_user",
        "explicit server_mode=single_user must override managed-service inference"
    );
    let override_source = governance_override["result"]["governance"]["multi_user_server"]
        ["inference"]["source"]
        .as_str()
        .expect("inference.source should be string after override");
    assert_eq!(
        override_source, "request",
        "inference.source must be 'request' when explicit server_mode is provided"
    );

    let shutdown = harness.request(9913, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn adversarial_governance_and_readiness_return_valid_structure_with_empty_params() {
    // Governance and readiness must return a valid, fully-structured response
    // even when called with empty params ({}) or null — no panics, no missing fields.
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);

    let mut harness = RpcHarness::spawn(&config_path);
    harness.request(9920, "initialize", None);

    // governance.status with empty object params
    let gov_empty = harness.request(9921, "governance.status", Some(json!({})));
    assert!(
        gov_empty.get("error").is_none() || gov_empty["error"].is_null(),
        "governance.status with empty params must not return error"
    );
    assert!(
        gov_empty["result"]["governance"].is_object(),
        "governance.status must return governance object with empty params"
    );
    assert_eq!(
        gov_empty["result"]["governance"]["schema_version"], "blue26-governance-v1",
        "governance.status with empty params must preserve schema_version"
    );

    // release.readiness with empty object params
    let read_empty = harness.request(9922, "release.readiness", Some(json!({})));
    assert!(
        read_empty.get("error").is_none() || read_empty["error"].is_null(),
        "release.readiness with empty params must not return error"
    );
    assert!(
        read_empty["result"]["readiness"].is_object(),
        "release.readiness must return readiness object with empty params"
    );
    assert_eq!(
        read_empty["result"]["readiness"]["schema_version"], "blue26-release-readiness-v2",
        "release.readiness with empty params must preserve schema_version"
    );
    assert!(
        read_empty["result"]["readiness"]["blocked_gate_names"].is_array(),
        "release.readiness must include blocked_gate_names with empty params"
    );

    let shutdown = harness.request(9923, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn adversarial_invalid_method_returns_jsonrpc_error_does_not_crash_process() {
    // Sending an unknown method must return a JSON-RPC -32601 error and must NOT
    // terminate or corrupt the process — subsequent valid requests must still work.
    // This is the deterministic adversarial gate for robustness under invalid input.
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);

    let mut harness = RpcHarness::spawn(&config_path);
    harness.request(9930, "initialize", None);

    // Unknown method must return error code -32601.
    let unknown = harness.request(9931, "blue26.adversarial.nonexistent.method", None);
    assert_eq!(
        unknown["error"]["code"], -32601,
        "unknown method must return JSON-RPC error code -32601"
    );
    let err_msg = unknown["error"]["message"]
        .as_str()
        .expect("error.message should be string");
    assert!(
        err_msg.contains("unknown method") || err_msg.contains("method not found"),
        "error.message should describe unknown method, got: {err_msg}"
    );

    // Process must still be alive and responsive after the error.
    let health = harness.request(9932, "runtime.health", None);
    assert!(
        health.get("error").is_none() || health["error"].is_null(),
        "runtime.health must succeed after an unknown method error"
    );
    assert!(
        health["result"]["lifecycle"].is_object(),
        "runtime.health lifecycle should be present after adversarial request"
    );

    let shutdown = harness.request(9933, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

// ── BLUE35 S1-S17: full profile coverage assertions ───────────────────────────

#[test]
fn blue35_governance_profiles_present_for_s1_s16() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);

    let mut harness = RpcHarness::spawn(&config_path);
    harness.request(19001, "initialize", None);

    let governance = harness.request(19002, "governance.status", None);
    let gov = &governance["result"]["governance"];

    // S1: custom_role_registry
    assert!(
        gov["custom_role_registry"].is_object(),
        "governance should include custom_role_registry"
    );
    assert!(
        gov["custom_role_registry"]["ready"].is_boolean(),
        "custom_role_registry.ready must be boolean"
    );

    // S2: custom_role_dynamic_matching
    assert!(
        gov["custom_role_dynamic_matching"].is_object(),
        "governance should include custom_role_dynamic_matching"
    );

    // S3: compliance_audit_metadata
    assert!(
        gov["compliance_audit_metadata"].is_object(),
        "governance should include compliance_audit_metadata"
    );
    assert!(
        gov["compliance_audit_metadata"]["compliance_framework_profile"].is_object(),
        "compliance_audit_metadata must include compliance_framework_profile"
    );

    // S4: self_rationalization_guard with profile
    assert!(
        gov["self_rationalization_guard"].is_object(),
        "governance should include self_rationalization_guard"
    );
    assert!(
        gov["self_rationalization_guard"]["self_rationalization_guard_profile"].is_object(),
        "self_rationalization_guard must include self_rationalization_guard_profile"
    );
    assert!(
        gov["self_rationalization_guard"]["self_rationalization_guard_profile"]
            ["reexamine_triggered_count"]
            .is_number(),
        "self_rationalization_guard_profile must include reexamine_triggered_count"
    );
    assert!(
        gov["self_rationalization_guard"]["self_rationalization_guard_profile"]
            ["weak_evidence_blocked_count"]
            .is_number(),
        "self_rationalization_guard_profile must include weak_evidence_blocked_count"
    );

    // S5: startup_context_loader
    assert!(
        gov["startup_context_loader"].is_object(),
        "governance should include startup_context_loader"
    );
    assert!(
        gov["startup_context_loader"]["enabled"].is_boolean(),
        "startup_context_loader.enabled must be boolean"
    );

    // S6: layered_prompt_builder with profile
    assert!(
        gov["layered_prompt_builder"].is_object(),
        "governance should include layered_prompt_builder"
    );
    assert!(
        gov["layered_prompt_builder"]["prompt_layer_profile"].is_object(),
        "layered_prompt_builder must include prompt_layer_profile"
    );
    assert!(
        gov["layered_prompt_builder"]["prompt_layer_profile"]["static_layers_cached"].is_number(),
        "prompt_layer_profile must include static_layers_cached"
    );

    // S7: layered_token_trigger with profile
    assert!(
        gov["layered_token_trigger"].is_object(),
        "governance should include layered_token_trigger"
    );
    assert!(
        gov["layered_token_trigger"]["layered_token_trigger_profile"].is_object(),
        "layered_token_trigger must include layered_token_trigger_profile"
    );
    assert!(
        gov["layered_token_trigger"]["layered_token_trigger_profile"]["l1_cache_hit_count"]
            .is_number(),
        "layered_token_trigger_profile must include l1_cache_hit_count"
    );

    // S8: multi_priority_scheduler with dual_level_scheduler_profile
    assert!(
        gov["multi_priority_scheduler"].is_object(),
        "governance should include multi_priority_scheduler"
    );
    assert!(
        gov["multi_priority_scheduler"]["dual_level_scheduler_profile"].is_object(),
        "multi_priority_scheduler must include dual_level_scheduler_profile"
    );
    assert!(
        gov["multi_priority_scheduler"]["dual_level_scheduler_profile"]["l1_queue_depth"]
            .is_number(),
        "dual_level_scheduler_profile must include l1_queue_depth"
    );

    // S9: worker_scheduler_backpressure with priority_queue_profile
    assert!(
        gov["worker_scheduler_backpressure"].is_object(),
        "governance should include worker_scheduler_backpressure"
    );
    assert!(
        gov["worker_scheduler_backpressure"]["priority_queue_profile"].is_object(),
        "worker_scheduler_backpressure must include priority_queue_profile"
    );
    assert!(
        gov["worker_scheduler_backpressure"]["priority_queue_profile"]
            ["starvation_events_prevented"]
            .is_number(),
        "priority_queue_profile must include starvation_events_prevented"
    );

    // S10: fork_isolation_guard with fork_isolation_profile
    assert!(
        gov["fork_isolation_guard"].is_object(),
        "governance should include fork_isolation_guard"
    );
    assert!(
        gov["fork_isolation_guard"]["fork_isolation_profile"].is_object(),
        "fork_isolation_guard must include fork_isolation_profile"
    );
    assert!(
        gov["fork_isolation_guard"]["fork_isolation_profile"]["zombie_reaped_count"].is_number(),
        "fork_isolation_profile must include zombie_reaped_count"
    );

    // S11: capability_graph with capability_graph_profile
    assert!(
        gov["capability_graph"].is_object(),
        "governance should include capability_graph"
    );
    assert!(
        gov["capability_graph"]["capability_graph_profile"].is_object(),
        "capability_graph must include capability_graph_profile"
    );
    assert!(
        gov["capability_graph"]["capability_graph_profile"]["node_count"].is_number(),
        "capability_graph_profile must include node_count"
    );

    // S12: provenance_ledger with provenance_ledger_profile
    assert!(
        gov["provenance_ledger"].is_object(),
        "governance should include provenance_ledger"
    );
    assert!(
        gov["provenance_ledger"]["provenance_ledger_profile"].is_object(),
        "provenance_ledger must include provenance_ledger_profile"
    );
    assert!(
        gov["provenance_ledger"]["provenance_ledger_profile"]["entry_count"].is_number(),
        "provenance_ledger_profile must include entry_count"
    );

    // S13: node_reputation_tracker with node_reputation_profile
    assert!(
        gov["node_reputation_tracker"].is_object(),
        "governance should include node_reputation_tracker"
    );
    assert!(
        gov["node_reputation_tracker"]["node_reputation_profile"].is_object(),
        "node_reputation_tracker must include node_reputation_profile"
    );
    assert!(
        gov["node_reputation_tracker"]["node_reputation_profile"]["tracked_agent_count"]
            .is_number(),
        "node_reputation_profile must include tracked_agent_count"
    );

    // S14: k8s_delivery_pack with cloud_native_profile
    assert!(
        gov["k8s_delivery_pack"].is_object(),
        "governance should include k8s_delivery_pack"
    );
    assert!(
        gov["k8s_delivery_pack"]["cloud_native_profile"].is_object(),
        "k8s_delivery_pack must include cloud_native_profile"
    );
    assert!(
        gov["k8s_delivery_pack"]["cloud_native_profile"]["health_endpoint_ready"].is_boolean(),
        "cloud_native_profile must include health_endpoint_ready"
    );

    // S15: sdk_multi_language_stub with developer_sdk_profile
    assert!(
        gov["sdk_multi_language_stub"].is_object(),
        "governance should include sdk_multi_language_stub"
    );
    assert!(
        gov["sdk_multi_language_stub"]["developer_sdk_profile"].is_object(),
        "sdk_multi_language_stub must include developer_sdk_profile"
    );

    // S16: workflow_type_tri_mode with workflow_profile
    assert!(
        gov["workflow_type_tri_mode"].is_object(),
        "governance should include workflow_type_tri_mode"
    );
    assert!(
        gov["workflow_type_tri_mode"]["workflow_profile"].is_object(),
        "workflow_type_tri_mode must include workflow_profile"
    );
    assert!(
        gov["workflow_type_tri_mode"]["workflow_profile"]["configured_workflow_type"].is_string(),
        "workflow_profile must include configured_workflow_type"
    );
    assert!(
        gov["workflow_type_tri_mode"]["workflow_profile"]["effective_workflow_type"].is_string(),
        "workflow_profile must include effective_workflow_type"
    );

    // S17: blue35_release_closure
    assert!(
        gov["blue35_release_closure"].is_object(),
        "governance should include blue35_release_closure"
    );
    assert!(
        gov["blue35_release_closure"]["ready"].is_boolean(),
        "blue35_release_closure.ready must be boolean"
    );

    let shutdown = harness.request(19003, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn blue35_readiness_profiles_present_for_s1_s17() {
    let temp = tempdir().expect("failed to create temp dir");
    let config_path = temp.path().join("config.toml");
    write_test_config(&config_path, 60, 120, 5);

    let mut harness = RpcHarness::spawn(&config_path);
    harness.request(19010, "initialize", None);

    let readiness = harness.request(19011, "release.readiness", None);
    let r = &readiness["result"]["readiness"];

    assert!(
        r["custom_role_registry"].is_object(),
        "readiness.custom_role_registry must be object"
    );
    assert!(
        r["custom_role_dynamic_matching"].is_object(),
        "readiness.custom_role_dynamic_matching must be object"
    );
    assert!(
        r["compliance_audit_metadata"].is_object(),
        "readiness.compliance_audit_metadata must be object"
    );
    assert!(
        r["self_rationalization_guard"].is_object(),
        "readiness.self_rationalization_guard must be object"
    );
    assert!(
        r["startup_context_loader"].is_object(),
        "readiness.startup_context_loader must be object"
    );
    assert!(
        r["layered_prompt_builder"].is_object(),
        "readiness.layered_prompt_builder must be object"
    );
    assert!(
        r["layered_token_trigger"].is_object(),
        "readiness.layered_token_trigger must be object"
    );
    assert!(
        r["multi_priority_scheduler"].is_object(),
        "readiness.multi_priority_scheduler must be object"
    );
    assert!(
        r["worker_scheduler_backpressure"].is_object(),
        "readiness.worker_scheduler_backpressure must be object"
    );
    assert!(
        r["fork_isolation_guard"].is_object(),
        "readiness.fork_isolation_guard must be object"
    );
    assert!(
        r["capability_graph"].is_object(),
        "readiness.capability_graph must be object"
    );
    assert!(
        r["provenance_ledger"].is_object(),
        "readiness.provenance_ledger must be object"
    );
    assert!(
        r["node_reputation_tracker"].is_object(),
        "readiness.node_reputation_tracker must be object"
    );
    assert!(
        r["k8s_delivery_pack"].is_object(),
        "readiness.k8s_delivery_pack must be object"
    );
    assert!(
        r["sdk_multi_language_stub"].is_object(),
        "readiness.sdk_multi_language_stub must be object"
    );
    assert!(
        r["workflow_type_tri_mode"].is_object(),
        "readiness.workflow_type_tri_mode must be object"
    );
    assert!(
        r["blue35_release_closure"].is_object(),
        "readiness.blue35_release_closure must be object"
    );

    // Verify key sub-fields
    assert!(
        r["self_rationalization_guard"]["ready"].is_boolean(),
        "self_rationalization_guard.ready must be boolean"
    );
    assert!(
        r["capability_graph"]["node_dependency_graph"].is_boolean(),
        "capability_graph.node_dependency_graph must be boolean"
    );
    assert!(
        r["workflow_type_tri_mode"]["auto_detection"].is_boolean(),
        "workflow_type_tri_mode.auto_detection must be boolean"
    );
    assert!(
        r["fork_isolation_guard"]["zombie_reap"].is_boolean(),
        "fork_isolation_guard.zombie_reap must be boolean"
    );
    assert!(
        r["layered_token_trigger"]["gate_chain"].is_array(),
        "layered_token_trigger.gate_chain must be array"
    );

    let shutdown = harness.request(19012, "shutdown", None);
    assert_eq!(shutdown["result"]["ok"], true);
    harness.wait_for_exit(Duration::from_secs(8));
}
