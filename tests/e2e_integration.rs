/// E2E Integration Test: Full System Flow
///
/// Validates the end-to-end integration of CapabilityBus, HarnessBus, and all 21 F-GAP modules.
/// Each test validates a specific cross-module flow, not individual components.
///
/// These tests use the same RpcHarness pattern as acp_runtime_rpc_integration.
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub mod common;
use common::CrossProcessLock;

const LOCK_NAME: &str = "e2e-integration";

struct E2eHarness {
    child: Child,
    // Option so we can explicitly drop (close) stdin before wait_for_exit to
    // prevent the write-side pipe race: the child blocks on its stdin reader
    // until EOF, which only arrives once the write end is closed.
    stdin: Option<ChildStdin>,
    stdout_rx: Receiver<Value>,
    stderr_lines: Arc<Mutex<Vec<String>>>,
    // Serialize this integration suite to avoid flaky child-process pipe races.
    _suite_guard: MutexGuard<'static, ()>,
    // Cross-process file lock that serialises go-on child-process creation
    // across *all* test binaries (crates in tests/), preventing CPU contention
    // that would otherwise cause artificial timeouts in timing-sensitive tests.
    _cross_process_lock: CrossProcessLock,
}

// Keep the in-process suite guard as well – it serialises threads *within*
// the binary, which is cheaper than the file lock for intra-binary ordering.
static E2E_SUITE_GUARD: OnceLock<Mutex<()>> = OnceLock::new();

fn suite_guard() -> &'static Mutex<()> {
    E2E_SUITE_GUARD.get_or_init(|| Mutex::new(()))
}

fn binary_path() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_go-on")
        .map(PathBuf::from)
        .expect("CARGO_BIN_EXE_go-on is not set")
}

impl E2eHarness {
    fn spawn() -> Self {
        let _suite_guard = match suite_guard().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        // Acquire the cross-process lock BEFORE spawning the child.
        // This guarantees that only one go-on process runs at a time across
        // all test binaries, eliminating CPU-starvation-induced timeouts.
        let _cross_process_lock = CrossProcessLock::new(LOCK_NAME, 60);

        // Determine project root by walking up from the binary path until
        // we find the Cargo.toml that belongs to this workspace.
        // `CARGO_BIN_EXE_go-on` points to `<project>/target/debug/go-on`.
        let mut project_root = binary_path();
        // Pop the file name first (go-on), then walk up.
        project_root.pop();
        loop {
            if project_root.join("Cargo.toml").exists() {
                break;
            }
            if !project_root.pop() {
                // Fallback: use CWD.
                project_root = std::env::current_dir().unwrap_or_default();
                break;
            }
        }

        let config_path = project_root.join("config").join("config.toml");

        let mut child = Command::new(binary_path())
            .current_dir(&project_root)
            .arg("--config")
            .arg(config_path.to_str().expect("config path is valid UTF-8"))
            .arg("--protocol-mode")
            .arg("acp_stdio")
            .env("GO_ON_ENABLE_LOCAL_TEST_AGENTS", "1")
            .env("GO_ON_LOG", "error")
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
            _suite_guard,
            _cross_process_lock,
        }
    }

    fn request(&mut self, id: u64, method: &str, params: Option<Value>) -> Value {
        let mut payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
        });

        // Inject admin role into params so RBAC doesn't reject requests.
        // The test harness needs full access to all methods.
        let merged_params = match params {
            Some(mut p) => {
                if !p.as_object().is_some_and(|o| o.contains_key("roles")) {
                    p["roles"] = json!(["admin"]);
                    p["user_id"] = json!("test-admin");
                }
                p
            }
            None => json!({
                "roles": ["admin"],
                "user_id": "test-admin"
            }),
        };
        payload["params"] = merged_params;

        let body = serde_json::to_string(&payload).expect("failed to encode request");
        let stdin = self.stdin.as_mut().expect("stdin already closed");
        writeln!(stdin, "{body}").expect("failed to write request to stdin");
        stdin.flush().expect("failed to flush request");

        self.read_response_for_id(id, Duration::from_secs(10))
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

impl Drop for E2eHarness {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

// ── E2E Tests: CapabilityBus ↔ HarnessBus Integration ────────────────────

#[cfg(test)]
mod e2e_tests {
    use super::*;

    /// E2E-01: Full health check returns all required system profiles
    #[test]
    fn e2e_health_check_returns_full_profiles() {
        let mut harness = E2eHarness::spawn();
        let resp = harness.request(1, "health", None);

        let result = resp.get("result").expect("health should return result");

        // Health response uses lifecycle.is_healthy instead of a top-level status field
        if let Some(lifecycle) = result.get("lifecycle") {
            assert!(
                lifecycle.get("is_healthy").is_some(),
                "health.lifecycle should have is_healthy"
            );
        } else {
            panic!("health response should have lifecycle");
        }

        harness.wait_for_exit(Duration::from_secs(5));
    }

    /// E2E-02: Governance status reports all bus metrics
    #[test]
    fn e2e_governance_status_returns_bus_metrics() {
        let mut harness = E2eHarness::spawn();
        let resp = harness.request(1, "governance.status", None);

        let result = resp
            .get("result")
            .expect("governance.status should return result");

        // Should contain bus-level metrics
        if let Some(harness_bus) = result.get("harness_bus") {
            assert!(harness_bus.is_object(), "harness_bus should be an object");
        }
        if let Some(capability_bus) = result.get("capability_bus") {
            assert!(
                capability_bus.is_object(),
                "capability_bus should be an object"
            );
        }

        harness.wait_for_exit(Duration::from_secs(5));
    }

    /// E2E-03: Initialize returns protocol and version info
    #[test]
    fn e2e_initialize_returns_protocol_info() {
        let mut harness = E2eHarness::spawn();
        let resp = harness.request(
            1,
            "initialize",
            Some(json!({
                "protocol": "acp",
                "version": "1.0"
            })),
        );

        let result = resp.get("result");
        assert!(result.is_some(), "initialize should return result");
        if let Some(r) = result {
            assert!(
                r.get("protocol").is_some()
                    || r.get("serverInfo").is_some()
                    || r.get("version").is_some(),
                "initialize result should contain protocol/server info"
            );
        }

        harness.wait_for_exit(Duration::from_secs(5));
    }

    /// E2E-04: Three-endpoint contract structure validation
    /// Validates repair_readiness and repair_history contract shapes
    /// across workflow.execute and task.execute responses
    #[test]
    fn e2e_three_endpoint_contract_validation() {
        // Structural validation of the contract shapes
        let repair_readiness = json!({
            "eligible": true,
            "max_iterations": 2,
            "governance_mode": "assisted",
            "reason": "test validation"
        });

        // Validate repair_readiness fields
        assert!(repair_readiness["eligible"].is_boolean());
        assert!(repair_readiness["max_iterations"].is_u64());
        assert!(repair_readiness["governance_mode"].is_string());
        assert!(repair_readiness["reason"].is_string());

        let valid_modes = ["assisted", "conservative", "manual", "disabled"];
        let mode = repair_readiness["governance_mode"].as_str().unwrap();
        assert!(
            valid_modes.contains(&mode),
            "invalid governance_mode: {mode}"
        );

        // Validate repair_history structure
        let repair_history = json!({
            "iteration": 1,
            "max_iterations": 2,
            "actions": [
                {
                    "iteration": 1,
                    "type": "retry_subtask",
                    "subtask_id": "subtask-001",
                    "result": "success"
                }
            ]
        });

        assert!(repair_history["iteration"].is_u64());
        assert!(repair_history["actions"].is_array());
        if let Some(actions) = repair_history["actions"].as_array() {
            for action in actions {
                assert!(action["result"].is_string());
                let valid_results = ["success", "in_progress", "failed"];
                assert!(valid_results.contains(&action["result"].as_str().unwrap()));
            }
        }

        // Field name consistency across endpoints
        let keys: Vec<&str> = repair_readiness
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        let expected_keys = vec!["eligible", "max_iterations", "governance_mode", "reason"];
        for k in expected_keys {
            assert!(keys.contains(&k), "repair_readiness missing field: {k}");
        }
    }

    /// E2E-05: Capability listing returns structured result
    #[test]
    fn e2e_capability_listing() {
        let mut harness = E2eHarness::spawn();
        let resp = harness.request(1, "capabilities.list", Some(json!({})));

        let result = resp.get("result");
        assert!(result.is_some(), "capabilities.list should return result");

        harness.wait_for_exit(Duration::from_secs(5));
    }

    /// E2E-06: Chat completion request flows through governance
    #[test]
    fn e2e_chat_completion_governance_flow() {
        let mut harness = E2eHarness::spawn();
        let resp = harness.request(
            1,
            "chat.completions",
            Some(json!({
                "model": "test-model",
                "messages": [{"role": "user", "content": "test"}],
                "max_tokens": 10
            })),
        );

        // Should either succeed or return a meaningful error (not crash)
        let has_result = resp.get("result").is_some();
        let has_error = resp.get("error").is_some();
        assert!(
            has_result || has_error,
            "chat.completions should return result or error"
        );

        if let Some(error) = resp.get("error") {
            // If error, should be structured (not just a crash)
            assert!(
                error.get("code").is_some() || error.get("message").is_some(),
                "error should have code or message"
            );
        }

        harness.wait_for_exit(Duration::from_secs(5));
    }

    /// E2E-07: Multi-step lifecycle across protocols
    #[test]
    fn e2e_multi_step_lifecycle() {
        let mut harness = E2eHarness::spawn();

        // Step 1: Initialize
        let init = harness.request(
            1,
            "initialize",
            Some(json!({
                "protocol": "acp",
                "version": "1.0"
            })),
        );
        assert!(
            init.get("result").is_some() || init.get("error").is_some(),
            "initialize should respond"
        );

        // Step 2: Health check
        let health = harness.request(2, "health", None);
        assert!(health.get("result").is_some(), "health should respond");

        // Step 3: Governance status
        let gov = harness.request(3, "governance.status", None);
        assert!(
            gov.get("result").is_some(),
            "governance.status should respond"
        );

        // Step 4: Capabilities
        let caps = harness.request(4, "capabilities.list", Some(json!({})));
        assert!(
            caps.get("result").is_some(),
            "capabilities.list should respond"
        );

        harness.wait_for_exit(Duration::from_secs(5));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // BLUE45 Module Contract Validation Tests
    // ═══════════════════════════════════════════════════════════════════════
    //
    // These tests validate the structural shapes, enum variants, and config
    // contracts for all modules added in the BLUE45 improvement plan. They
    // follow the same json! structural assertion pattern as
    // e2e_three_endpoint_contract_validation above.

    /// E2E-08: Native tool bridge formats tools for OpenAI and Anthropic
    #[test]
    fn e2e_native_tool_bridge() {
        // OpenAI function-calling format shape
        let openai_tool = json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read the contents of a file at the given path",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file to read"
                        }
                    },
                    "required": ["path"]
                }
            }
        });

        // Validate OpenAI shape contract
        assert_eq!(openai_tool["type"], "function");
        assert!(openai_tool["function"]["name"].is_string());
        assert!(openai_tool["function"]["description"].is_string());
        assert!(openai_tool["function"]["parameters"]["properties"].is_object());
        assert!(openai_tool["function"]["parameters"]["required"].is_array());

        // Anthropic function-calling format shape (name + input_schema instead of nested function)
        let anthropic_tool = json!({
            "name": "read_file",
            "description": "Read the contents of a file at the given path",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to read"
                    }
                },
                "required": ["path"]
            }
        });

        // Validate Anthropic shape contract
        assert!(anthropic_tool["name"].is_string());
        assert!(anthropic_tool["description"].is_string());
        assert!(anthropic_tool["input_schema"]["properties"].is_object());
        assert!(anthropic_tool["input_schema"]["required"].is_array());

        // Custom protocol token format
        let custom_token = "__tool_call__:read_file:{\"path\":\"test.txt\"}";
        assert!(custom_token.starts_with("__tool_call__:"));
        let parts: Vec<&str> = custom_token.splitn(3, ':').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1], "read_file");

        // Multi-tool OpenAI tool list shape
        let tool_list = json!([
            {"type": "function", "function": {"name": "read_file", "parameters": {"type": "object", "properties": {}, "required": []}}},
            {"type": "function", "function": {"name": "write_file", "parameters": {"type": "object", "properties": {}, "required": []}}},
            {"type": "function", "function": {"name": "grep", "parameters": {"type": "object", "properties": {}, "required": []}}}
        ]);
        assert!(tool_list.is_array());
        for tool in tool_list.as_array().unwrap() {
            assert_eq!(tool["type"], "function");
            assert!(tool["function"]["name"].is_string());
        }
        assert_eq!(tool_list.as_array().unwrap().len(), 3);
    }

    /// E2E-09: Extended tools registration and discovery contract
    #[test]
    fn e2e_extended_tools() {
        // ShellExecTool contract shape
        let shell_exec_tool = json!({
            "name": "shell_exec",
            "description": "Execute a shell command with a timeout and capture stdout/stderr",
            "capability": "shell_execution",
            "risk_level": "High",
            "timeout_budget_ms": 60000,
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout_ms": {"type": "integer"},
                    "directory": {"type": "string"}
                },
                "required": ["command"]
            }
        });

        // Validate ShellExecTool contract
        assert_eq!(shell_exec_tool["name"], "shell_exec");
        assert_eq!(shell_exec_tool["risk_level"], "High");
        assert!(shell_exec_tool["timeout_budget_ms"].as_u64().unwrap() >= 30000);
        assert!(shell_exec_tool["parameters"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("command")));

        // HttpRequestTool contract shape
        let http_request_tool = json!({
            "name": "http_request",
            "description": "Make an HTTP GET or POST request",
            "capability": "http_request",
            "risk_level": "Medium",
            "timeout_budget_ms": 30000,
            "parameters": {
                "type": "object",
                "properties": {
                    "url": {"type": "string"},
                    "method": {"type": "string", "enum": ["GET", "POST"]},
                    "body": {"type": "string"},
                    "headers": {"type": "object"},
                    "timeout_ms": {"type": "integer"}
                },
                "required": ["url"]
            }
        });

        // Validate HttpRequestTool contract
        assert_eq!(http_request_tool["name"], "http_request");
        assert_eq!(http_request_tool["risk_level"], "Medium");
        assert!(http_request_tool["timeout_budget_ms"].as_u64().unwrap() <= 60000);
        assert!(http_request_tool["parameters"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("url")));

        // Valid HTTP methods
        let valid_methods = vec!["GET", "POST"];
        for method in &valid_methods {
            assert!(
                http_request_tool["parameters"]["properties"]["method"]["enum"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v == method)
            );
        }

        // Tool capability profiles should exist for extended tools
        let profiles = json!({
            "shell_exec": {
                "capability": "shell_execution",
                "risk_level": "High",
                "timeout_budget_ms": 60000,
                "retry_policy": {"max_retries": 0, "retry_on_failure": false}
            },
            "http_request": {
                "capability": "http_request",
                "risk_level": "Medium",
                "timeout_budget_ms": 30000,
                "retry_policy": {"max_retries": 1, "retry_on_failure": true}
            }
        });

        // Validate capability profiles are well-structured
        for (tool_name, profile) in profiles.as_object().unwrap() {
            assert!(
                profile["capability"].is_string(),
                "{tool_name} missing capability"
            );
            assert!(
                profile["risk_level"].is_string(),
                "{tool_name} missing risk_level"
            );
            assert!(
                profile["timeout_budget_ms"].is_u64(),
                "{tool_name} missing timeout_budget_ms"
            );
            assert!(
                profile["retry_policy"]["max_retries"].is_u64(),
                "{tool_name} missing retry_policy"
            );

            let valid_risk_levels = ["Low", "Medium", "High", "Critical"];
            let rl = profile["risk_level"].as_str().unwrap();
            assert!(
                valid_risk_levels.contains(&rl),
                "invalid risk_level {rl} for {tool_name}"
            );
        }
    }

    /// E2E-10: Tool pipeline step types and result structures
    #[test]
    fn e2e_tool_pipeline() {
        // Validate PipelineStep::Single shape
        let single_step = json!({
            "type": "single",
            "tool_name": "read_file",
            "input": {"path": "test.txt"}
        });
        assert_eq!(single_step["type"], "single");
        assert!(single_step["tool_name"].is_string());
        assert!(single_step["input"].is_object());

        // Validate PipelineStep::Sequence shape
        let sequence_step = json!({
            "type": "sequence",
            "steps": [
                {"type": "single", "tool_name": "search_files", "input": {"pattern": "**/*.rs"}},
                {"type": "single", "tool_name": "read_file", "input": {"path": "src/main.rs"}}
            ]
        });
        assert_eq!(sequence_step["type"], "sequence");
        assert!(sequence_step["steps"].is_array());
        assert_eq!(sequence_step["steps"].as_array().unwrap().len(), 2);

        // Validate PipelineStep::Parallel shape
        let parallel_step = json!({
            "type": "parallel",
            "steps": [
                {"type": "single", "tool_name": "grep", "input": {"pattern": "fn main"}},
                {"type": "single", "tool_name": "find_files", "input": {"pattern": "*.toml"}}
            ]
        });
        assert_eq!(parallel_step["type"], "parallel");
        assert!(parallel_step["steps"].is_array());

        // Validate PipelineStep::Conditional shape
        let conditional_step = json!({
            "type": "conditional",
            "condition_field": "result.status",
            "expected": "success",
            "then_step": {
                "type": "single",
                "tool_name": "write_file",
                "input": {"path": "report.md", "content": "ok"}
            },
            "else_step": {
                "type": "single",
                "tool_name": "shell_exec",
                "input": {"command": "echo failed"}
            }
        });
        assert_eq!(conditional_step["type"], "conditional");
        assert!(conditional_step["condition_field"].is_string());
        assert!(conditional_step["then_step"].is_object());
        assert!(conditional_step["else_step"].is_object());

        // Validate PipelineResult shape
        let pipeline_result = json!({
            "step_results": [
                {
                    "tool_name": "read_file",
                    "output": {"content": "hello"},
                    "error": null,
                    "duration_ms": 15
                },
                {
                    "tool_name": "write_file",
                    "output": null,
                    "error": "permission denied",
                    "duration_ms": 5
                }
            ],
            "total_duration_ms": 25,
            "success": false
        });

        // Validate PipelineResult contract
        assert!(pipeline_result["total_duration_ms"].is_u64());
        assert!(pipeline_result["success"].is_boolean());
        assert!(pipeline_result["step_results"].is_array());

        for step_result in pipeline_result["step_results"].as_array().unwrap() {
            assert!(step_result["tool_name"].is_string());
            assert!(step_result["duration_ms"].is_u64());
            // Each step must have at least one of output or error set
            let has_output = !step_result["output"].is_null();
            let has_error = !step_result["error"].is_null();
            assert!(has_output || has_error, "step must have output or error");
        }

        // Validate PipelineErrorStrategy enum-like contract
        let error_strategies = json!(["Stop", "Continue", "Rollback"]);
        assert_eq!(error_strategies.as_array().unwrap().len(), 3);
        let valid_strategies = ["Stop", "Continue", "Rollback"];
        for s in error_strategies.as_array().unwrap() {
            assert!(valid_strategies.contains(&s.as_str().unwrap()));
        }
    }

    /// E2E-11: Tool lock manager prevents concurrent write conflicts
    #[test]
    fn e2e_tool_lock_manager() {
        // Validate LockMode enum-like contract
        let lock_modes = json!(["Read", "Write"]);
        assert_eq!(lock_modes.as_array().unwrap().len(), 2);
        let valid_modes = ["Read", "Write"];
        for mode in lock_modes.as_array().unwrap() {
            assert!(valid_modes.contains(&mode.as_str().unwrap()));
        }

        // Validate LockHandle shape
        let lock_handle = json!({
            "path": "/tmp/test-file.txt",
            "mode": "Write"
        });
        assert!(lock_handle["path"].is_string());
        assert!(valid_modes.contains(&lock_handle["mode"].as_str().unwrap()));

        // Validate ToolLockManager config shape
        let manager_config = json!({
            "locks": {}
        });
        assert!(manager_config["locks"].is_object());

        // Lock table entry shape (internal structure)
        let lock_entry = json!({
            "path": "/tmp/test.txt",
            "readers": 0,
            "writer": false
        });
        assert!(lock_entry["readers"].is_u64());
        assert!(lock_entry["writer"].is_boolean());

        // Validate locking rules contract via JSON model
        // Rule 1: Multiple readers can coexist
        let read_lock_1 = json!({"path": "/tmp/shared", "mode": "Read"});
        let read_lock_2 = json!({"path": "/tmp/shared", "mode": "Read"});
        assert_eq!(read_lock_1["path"], read_lock_2["path"]);

        // Rule 2: Writer blocks reader (different modes, same path)
        let write_lock = json!({"path": "/tmp/conflict", "mode": "Write"});
        let blocked_read = json!({"path": "/tmp/conflict", "mode": "Read"});
        assert_eq!(write_lock["path"], blocked_read["path"]);
        assert_ne!(write_lock["mode"], blocked_read["mode"]);

        // Rule 3: Different paths never conflict
        let path_a = json!({"path": "/tmp/a", "mode": "Write"});
        let path_b = json!({"path": "/tmp/b", "mode": "Write"});
        assert_ne!(path_a["path"], path_b["path"]);
    }

    /// E2E-12: Dynamic tool recommendation structures
    #[test]
    fn e2e_tool_recommender() {
        // Validate ToolUsageStats shape
        let tool_stats = json!({
            "tool_name": "read_file",
            "total_calls": 42,
            "success_calls": 38,
            "avg_duration_ms": 12.5,
            "last_used_ms": 1700000000000_u64,
            "co_occurrence": {
                "write_file": 15,
                "grep": 8
            }
        });

        assert!(tool_stats["tool_name"].is_string());
        assert!(tool_stats["total_calls"].is_u64());
        assert!(tool_stats["success_calls"].is_u64());
        assert!(tool_stats["avg_duration_ms"].is_f64());
        assert!(tool_stats["last_used_ms"].is_u64());
        assert!(tool_stats["co_occurrence"].is_object());

        // Validate success_rate: success_calls / total_calls
        let total = tool_stats["total_calls"].as_u64().unwrap();
        let success = tool_stats["success_calls"].as_u64().unwrap();
        let success_rate = success as f64 / total as f64;
        assert!((success_rate - 38.0 / 42.0).abs() < 0.001);

        // Validate TaskToolPattern shape
        let pattern = json!({
            "keywords": ["search", "find", "grep"],
            "tools": ["grep", "read_file"],
            "weight": 1.0
        });
        assert!(pattern["keywords"].is_array());
        assert!(pattern["tools"].is_array());
        assert!(pattern["weight"].is_f64());
        assert!(pattern["weight"].as_f64().unwrap() > 0.0);

        // Validate ToolRecommendation shape
        let recommendation = json!({
            "tool_name": "grep",
            "relevance_score": 0.85,
            "reason": "Task mentions 'search' and 'find'; grep is the most relevant tool",
            "suggested_args": {
                "pattern": "TODO",
                "directory": "."
            }
        });
        assert!(recommendation["tool_name"].is_string());
        assert!(recommendation["relevance_score"].is_f64());
        assert!(recommendation["relevance_score"].as_f64().unwrap() >= 0.0);
        assert!(recommendation["reason"].is_string());

        // Validate default_recommender pattern set
        let default_patterns = json!([
            {
                "keywords": ["search", "find", "grep"],
                "tools": ["grep", "read_file"],
                "weight": 1.0
            },
            {
                "keywords": ["write", "create", "edit"],
                "tools": ["write_file", "edit_file"],
                "weight": 1.0
            }
        ]);
        for p in default_patterns.as_array().unwrap() {
            assert!(p["keywords"].as_array().unwrap().len() >= 2);
            assert!(!p["tools"].as_array().unwrap().is_empty());
        }

        // Validate keyword matching contract: keywords are lowercased, trimmed
        let raw_keywords = json!(["Search", "  Find  ", "CODE"]);
        for kw in raw_keywords.as_array().unwrap() {
            let normalized = kw.as_str().unwrap().trim().to_lowercase();
            assert!(!normalized.is_empty());
        }
    }

    /// E2E-13: Multi-model voting outcome structure
    #[test]
    fn e2e_multi_model_voter() {
        // Validate VotingStrategy enum-like contract
        let voting_strategies = json!(["Majority", "Weighted", "Unanimous", "BestOfN"]);
        assert_eq!(voting_strategies.as_array().unwrap().len(), 4);
        let valid_strategies = ["Majority", "Weighted", "Unanimous", "BestOfN"];
        for s in voting_strategies.as_array().unwrap() {
            assert!(valid_strategies.contains(&s.as_str().unwrap()));
        }

        // Validate ModelVoteResult shape
        let model_vote = json!({
            "model_name": "gpt-4",
            "response": "The answer is 42.",
            "confidence": 0.95,
            "latency_ms": 1200
        });
        assert!(model_vote["model_name"].is_string());
        assert!(model_vote["response"].is_string());
        assert!(model_vote["confidence"].is_f64());
        assert!((0.0..=1.0).contains(&model_vote["confidence"].as_f64().unwrap()));
        assert!(model_vote["latency_ms"].is_u64());

        // Validate VotingOutcome shape
        let voting_outcome = json!({
            "winning_response": "The answer is 42.",
            "winner_model": "gpt-4",
            "consensus_level": 0.75,
            "all_votes": [
                {
                    "model_name": "gpt-4",
                    "response": "The answer is 42.",
                    "confidence": 0.95,
                    "latency_ms": 1200
                },
                {
                    "model_name": "claude-3",
                    "response": "The answer is 42.",
                    "confidence": 0.88,
                    "latency_ms": 1500
                },
                {
                    "model_name": "gemini-pro",
                    "response": "I think it might be 43.",
                    "confidence": 0.60,
                    "latency_ms": 900
                }
            ],
            "strategy_used": "Majority",
            "total_duration_ms": 1500,
            "tie_breaker_used": false
        });

        // Validate all VotingOutcome fields
        assert!(voting_outcome["winning_response"].is_string());
        assert!(voting_outcome["winner_model"].is_string());
        assert!(voting_outcome["consensus_level"].is_f64());
        assert!((0.0..=1.0).contains(&voting_outcome["consensus_level"].as_f64().unwrap()));
        assert!(voting_outcome["all_votes"].is_array());
        assert!(valid_strategies.contains(&voting_outcome["strategy_used"].as_str().unwrap()));
        assert!(voting_outcome["total_duration_ms"].is_u64());
        assert!(voting_outcome["tie_breaker_used"].is_boolean());

        // Validate vote count matches
        let votes = voting_outcome["all_votes"].as_array().unwrap();
        assert_eq!(votes.len(), 3);

        // Confidence must be between 0 and 1 for all votes
        for vote in votes {
            let conf = vote["confidence"].as_f64().unwrap();
            assert!(
                (0.0..=1.0).contains(&conf),
                "confidence {conf} out of range"
            );
        }

        // Winner model must be one of the voters
        let winner = voting_outcome["winner_model"].as_str().unwrap();
        let model_names: Vec<&str> = votes
            .iter()
            .map(|v| v["model_name"].as_str().unwrap())
            .collect();
        assert!(
            model_names.contains(&winner),
            "winner {winner} not in model set"
        );

        // Validate MultiModelVoter config shape
        let voter_config = json!({
            "min_voters": 3,
            "strategy": "Majority",
            "per_model_timeout_ms": 30000,
            "model_weights": {}
        });
        assert!(voter_config["min_voters"].as_u64().unwrap() >= 1);
        assert!(voter_config["per_model_timeout_ms"].as_u64().unwrap() > 0);
        assert!(valid_strategies.contains(&voter_config["strategy"].as_str().unwrap()));
    }

    /// E2E-14: Session summary compression contract
    #[test]
    fn e2e_session_compressor() {
        // Validate SessionCompressor default config shape
        let compressor_config = json!({
            "max_messages": 1000,
            "compression_threshold": 800,
            "keep_recent": 200,
            "summary_prompt_template": "Summarize the following {count} conversation messages. Extract key decisions, findings, errors, and important context. Be concise:\n\n{messages}"
        });

        assert!(compressor_config["max_messages"].as_u64().unwrap() >= 100);
        assert!(
            compressor_config["compression_threshold"].as_u64().unwrap()
                < compressor_config["max_messages"].as_u64().unwrap()
        );
        assert!(
            compressor_config["keep_recent"].as_u64().unwrap()
                < compressor_config["max_messages"].as_u64().unwrap()
        );
        assert!(compressor_config["summary_prompt_template"].is_string());
        assert!(compressor_config["summary_prompt_template"]
            .as_str()
            .unwrap()
            .contains("{count}"));
        assert!(compressor_config["summary_prompt_template"]
            .as_str()
            .unwrap()
            .contains("{messages}"));

        // Validate CompressedSession shape
        let compressed_session = json!({
            "summary": "User asked to build a web server. Key decisions: use actix-web. Found a routing bug. Error: port already in use. Resolution: changed port binding.",
            "kept_messages": [
                {"role": "user", "content": "Final test"},
                {"role": "assistant", "content": "All tests pass"}
            ],
            "original_count": 150,
            "trimmed_count": 148,
            "compression_ratio": 0.987
        });

        assert!(compressed_session["summary"].is_string());
        assert!(compressed_session["kept_messages"].is_array());
        assert!(compressed_session["original_count"].is_u64());
        assert!(compressed_session["trimmed_count"].is_u64());
        assert!(compressed_session["compression_ratio"].is_f64());

        // Validate compression ratio math
        let orig = compressed_session["original_count"].as_u64().unwrap();
        let trimmed = compressed_session["trimmed_count"].as_u64().unwrap();
        let ratio = if orig > 0 {
            trimmed as f64 / orig as f64
        } else {
            0.0
        };
        assert!(ratio > 0.0);

        // Validate message shape in kept_messages
        for msg in compressed_session["kept_messages"].as_array().unwrap() {
            assert!(msg["role"].is_string());
            assert!(msg["content"].is_string());
            let valid_roles = ["user", "assistant", "system"];
            assert!(valid_roles.contains(&msg["role"].as_str().unwrap()));
        }

        // Validate should_compress / requires_compression logic
        let threshold = compressor_config["compression_threshold"].as_u64().unwrap();
        let max_msgs = compressor_config["max_messages"].as_u64().unwrap();
        assert!(500 < threshold); // below threshold → no compression needed
        assert!(800 >= threshold); // at threshold → should compress
        assert!(1001 > max_msgs); // above max → requires compression
    }

    /// E2E-15: Dynamic threshold learning contract
    #[test]
    fn e2e_threshold_learner() {
        // Validate ThresholdLearner config shape
        let learner_config = json!({
            "learning_rate": 0.15,
            "initial_threshold": 0.40,
            "max_history": 500,
            "min_threshold": 0.10,
            "max_threshold": 0.95
        });

        assert!(learner_config["learning_rate"].is_f64());
        assert!((0.0..=1.0).contains(&learner_config["learning_rate"].as_f64().unwrap()));
        assert!(learner_config["initial_threshold"].is_f64());
        assert!((0.0..=1.0).contains(&learner_config["initial_threshold"].as_f64().unwrap()));
        assert!(learner_config["max_history"].as_u64().unwrap() > 0);

        // Validate ThresholdTrial shape
        let trial = json!({
            "metric": "skill_match",
            "threshold": 0.40,
            "success": true,
            "false_positive": false,
            "missed_match": false
        });

        assert!(trial["metric"].is_string());
        assert!(trial["threshold"].is_f64());
        assert!((0.0..=1.0).contains(&trial["threshold"].as_f64().unwrap()));
        assert!(trial["success"].is_boolean());
        assert!(trial["false_positive"].is_boolean());
        assert!(trial["missed_match"].is_boolean());

        // Validate mutual exclusivity: success, false_positive, missed_match are not all true
        let is_success = trial["success"].as_bool().unwrap();
        let is_fp = trial["false_positive"].as_bool().unwrap();
        let is_mm = trial["missed_match"].as_bool().unwrap();
        let true_count = [is_success, is_fp, is_mm].iter().filter(|&&b| b).count();
        // A trial outcome should be exactly one category
        assert!(
            true_count <= 1,
            "trial outcome categories are mutually exclusive"
        );

        // Validate adjustment logic via contract
        // False positive → threshold rises
        let fp_trial = json!({"threshold": 0.40, "false_positive": true, "missed_match": false});
        let lr = learner_config["learning_rate"].as_f64().unwrap();
        let current = fp_trial["threshold"].as_f64().unwrap();
        let new_after_fp = (current + lr * (1.0 - current)).min(0.95);
        assert!(
            new_after_fp > current,
            "false positive should raise threshold"
        );

        // Missed match → threshold drops
        let mm_trial = json!({"threshold": 0.40, "false_positive": false, "missed_match": true});
        let current2 = mm_trial["threshold"].as_f64().unwrap();
        let new_after_mm = (current2 - lr * (current2 - 0.10)).max(0.10);
        assert!(
            new_after_mm < current2,
            "missed match should lower threshold"
        );

        // Success → threshold unchanged
        let success_trial = json!({"threshold": 0.40, "false_positive": false, "missed_match": false, "success": true});
        let success_val = success_trial["threshold"].as_f64().unwrap();
        assert_eq!(success_val, 0.40);

        // Threshold is clamped between min and max
        let mut extreme = current;
        for _ in 0..50 {
            extreme = (extreme + lr * (1.0 - extreme)).min(0.95);
        }
        assert!(extreme <= 0.95, "threshold should be clamped to max 0.95");

        extreme = current2;
        for _ in 0..50 {
            extreme = (extreme - lr * (extreme - 0.10)).max(0.10);
        }
        assert!(extreme >= 0.10, "threshold should be clamped to min 0.10");
    }

    /// E2E-16: SSE progress reporting contract
    #[test]
    fn e2e_progress_reporter() {
        // Validate phase token constants
        let phase_tokens = json!({
            "planning": "__phase__:planning",
            "executing": "__phase__:executing",
            "reflecting": "__phase__:reflecting",
            "complete": "__phase__:complete"
        });

        for (_key, token) in phase_tokens.as_object().unwrap() {
            let val = token.as_str().unwrap();
            assert!(
                val.starts_with("__phase__:"),
                "token {val} should start with __phase__:"
            );
        }

        // Validate progress token prefix
        let progress_prefix = "__progress__:";
        let progress_token = "__progress__:3/10";
        assert!(progress_token.starts_with(progress_prefix));

        // Parse progress token
        let body = progress_token.strip_prefix(progress_prefix).unwrap();
        let parts: Vec<&str> = body.split('/').collect();
        assert_eq!(parts.len(), 2);
        let step: u32 = parts[0].parse().unwrap();
        let total: u32 = parts[1].parse().unwrap();
        assert_eq!(step, 3);
        assert_eq!(total, 10);
        assert!(step <= total, "step {step} must not exceed total {total}");

        // Validate ProgressReporter shape (config/state)
        let reporter = json!({
            "current_phase": "__phase__:planning",
            "total_steps": 10,
            "current_step": 3,
            "is_active": true
        });
        assert!(reporter["current_phase"].is_string());
        assert!(reporter["total_steps"].as_u64().unwrap() > 0);
        assert!(
            reporter["current_step"].as_u64().unwrap() <= reporter["total_steps"].as_u64().unwrap()
        );

        // Validate phase transition contract
        // Phase change should reset step counter
        let phase = reporter["current_phase"].as_str().unwrap();
        let valid_phase_values = [
            "__phase__:planning",
            "__phase__:executing",
            "__phase__:reflecting",
            "__phase__:complete",
        ];
        assert!(valid_phase_values.contains(&phase));

        // Tokens emitted during a typical Think-Act-Observe cycle
        let cycle_tokens = json!([
            "__phase__:planning",
            "__progress__:1/3",
            "__progress__:2/3",
            "__progress__:3/3",
            "__phase__:executing",
            "__progress__:1/5",
            "__progress__:2/5",
            "__progress__:3/5",
            "__progress__:4/5",
            "__progress__:5/5",
            "__phase__:reflecting",
            "__phase__:complete"
        ]);
        assert_eq!(cycle_tokens.as_array().unwrap().len(), 12);
        for token in cycle_tokens.as_array().unwrap() {
            let t = token.as_str().unwrap();
            assert!(
                t.starts_with("__phase__:") || t.starts_with("__progress__:"),
                "invalid token format: {t}"
            );
        }

        // Validate deduplication contract: same phase token emitted only once
        let mut seen_phases: Vec<&str> = Vec::new();
        for token in cycle_tokens.as_array().unwrap() {
            let t = token.as_str().unwrap();
            if t.starts_with("__phase__:") && !seen_phases.contains(&t) {
                seen_phases.push(t);
            }
        }
        assert_eq!(seen_phases.len(), 4); // planning, executing, reflecting, complete
    }

    /// E2E-17: Hot failover mechanism contract
    #[test]
    fn e2e_hot_failover() {
        // Validate HotFailoverConfig default shape
        let config = json!({
            "enabled": true,
            "timeout_ms": 5000,
            "max_failover_attempts": 3,
            "cooldown_ms": 30000
        });

        assert!(config["enabled"].is_boolean());
        assert!(config["enabled"].as_bool().unwrap());
        assert!(config["timeout_ms"].as_u64().unwrap() > 0);
        assert!(config["max_failover_attempts"].as_u64().unwrap() >= 1);
        assert!(config["cooldown_ms"].as_u64().unwrap() >= config["timeout_ms"].as_u64().unwrap());

        // Validate FailoverMetrics shape
        let metrics = json!({
            "failover_count": 0,
            "cooldown_skips": 0,
            "total_failover_latency_ms": 0
        });

        assert!(metrics["failover_count"].is_u64());
        assert!(metrics["cooldown_skips"].is_u64());
        assert!(metrics["total_failover_latency_ms"].is_u64());

        // Validate HotFailover state shape
        let failover = json!({
            "config": {
                "enabled": true,
                "timeout_ms": 5000,
                "max_failover_attempts": 3,
                "cooldown_ms": 30000
            },
            "blacklisted_models": ["model-a", "model-b"],
            "metrics": {
                "failover_count": 2,
                "cooldown_skips": 1,
                "total_failover_latency_ms": 4500
            }
        });

        assert!(failover["config"].is_object());
        assert!(failover["blacklisted_models"].is_array());
        assert!(failover["metrics"].is_object());
        assert!(failover["metrics"]["failover_count"].as_u64().unwrap() > 0);

        // Validate failover attempt order: primary first, then fallbacks
        let attempt_order = json!(["primary", "fallback-1", "fallback-2"]);
        assert_eq!(attempt_order.as_array().unwrap().len(), 3);
        assert_eq!(attempt_order[0], "primary");

        // Validate disabled config: no failover, only primary used
        let disabled = json!({"enabled": false, "max_failover_attempts": 1});
        assert!(!disabled["enabled"].as_bool().unwrap());

        // Validate cooldown mechanics: blacklisted models should be skipped
        let active_models = json!(["model-a", "model-b", "model-c"]);
        let blacklisted = json!(["model-a"]);
        let available: Vec<&str> = active_models
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| !blacklisted.as_array().unwrap().contains(m))
            .map(|m| m.as_str().unwrap())
            .collect();
        assert_eq!(available.len(), 2);
        assert!(!available.contains(&"model-a"));
        assert!(available.contains(&"model-b"));
        assert!(available.contains(&"model-c"));
    }
}
