// Suite-level serialization: all tests acquire a global MutexGuard before starting.
// The guard is intentionally held across .await points to serialize test execution.
// This is safe because no cross-lock deadlock chains exist and all tests share the
// same current_thread tokio runtime.
#![allow(clippy::await_holding_lock)]

/// Transport parity gate — BLUE25
///
/// Verifies that all four transport paths inject `platform_context` consistently:
///   - ACP stdio  (via send_result → inject_platform_profiles_if_absent)
///   - ACP HTTP   (via inject added at every HTTP response point in BLUE25)
///   - MCP stdio  (via mcp/handlers.rs → inject_platform_profiles_if_absent)
///   - MCP HTTP   (via McpServer::handle_request → same inject)
///
/// ACP HTTP paths under test:
///   - /chat                 → profile_class == "infrastructure"
///   - /v1/chat/completions  → profile_class == "infrastructure"
///   - /v1/responses         → profile_class == "infrastructure"
///
/// Schema version must be "blue24-platform-universal-v1" across all paths.
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::tempdir;
use tokio::time::sleep;

pub mod common;
use common::binary_path;
use common::find_free_port;
use common::suite_mutex;
use common::CrossProcessLock;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

#[allow(clippy::await_holding_lock)]
fn lock_suite_guard() -> std::sync::MutexGuard<'static, ()> {
    suite_mutex().lock().unwrap_or_else(|err| err.into_inner())
}

fn ephemeral_bind_addr() -> String {
    format!("127.0.0.1:{}", find_free_port())
}

fn write_local_echo_config(path: &Path) {
    let config = r#"default_phase = "coding"
schema_version = "1.0.0"

[flow]
name = "Transport Parity Test"
phases = ["coding"]

[runtime]
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
    fs::write(path, config).expect("failed to write local_echo config");
}

struct HttpHarness {
    child: Child,
    base_url: String,
    _cross_process_lock: CrossProcessLock,
}

impl HttpHarness {
    fn spawn(config_path: &Path, bind_addr: String) -> Self {
        Self::spawn_with_mode(config_path, bind_addr, "acp_http")
    }

    fn spawn_with_mode(config_path: &Path, bind_addr: String, mode: &str) -> Self {
        let lock = CrossProcessLock::new("transport-parity", 60);
        Self::spawn_with_mode_and_lock(config_path, bind_addr, mode, lock)
    }

    fn spawn_with_mode_and_lock(
        config_path: &Path,
        bind_addr: String,
        mode: &str,
        lock: CrossProcessLock,
    ) -> Self {
        let child = Command::new(binary_path())
            .arg("--config")
            .arg(config_path)
            .arg("--protocol-mode")
            .arg(mode)
            .arg("--acp-http-bind")
            .arg(&bind_addr)
            .env("GO_ON_ENABLE_LOCAL_TEST_AGENTS", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn go-on http harness");

        let base_url = format!("http://{bind_addr}");
        HttpHarness {
            child,
            base_url,
            _cross_process_lock: lock,
        }
    }
}

impl Drop for HttpHarness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn wait_healthy(client: &reqwest::Client, base_url: &str, timeout: Duration) {
    let effective = timeout.max(Duration::from_secs(30));
    let deadline = Instant::now() + effective;
    while Instant::now() < deadline {
        if let Ok(r) = client.get(format!("{base_url}/health")).send().await {
            if r.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server at {base_url} did not become healthy within {timeout:?}");
}

/// MCP HTTP requires two-phase initialization: send `initialize` first,
/// otherwise the server rejects later requests with SERVER_NOT_INITIALIZED (-32002).
async fn mcp_http_initialize(client: &reqwest::Client, base_url: &str) {
    let resp: Value = post_mcp_json_with_retry(
        client,
        &format!("{base_url}/"),
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "clientInfo": { "name": "go-on-parity-test" }
            }
        }),
        3,
    )
    .await;
    assert!(
        resp.get("result").is_some(),
        "mcp http initialize must succeed; got: {resp}"
    );
}

/// POST JSON to an MCP HTTP endpoint, retrying on transport-level errors.
///
/// Integration test binaries run in parallel and `find_free_port()` has a
/// small TOCTOU window, so a fresh connection can occasionally hit a
/// half-closed/stale port. Retrying the connection (not the HTTP status)
/// removes that flake.
async fn post_mcp_json_with_retry(
    client: &reqwest::Client,
    url: &str,
    payload: &Value,
    attempts: usize,
) -> Value {
    let total = attempts.max(1);
    let mut last_err: Option<reqwest::Error> = None;
    for i in 0..total {
        match client.post(url).json(payload).send().await {
            Ok(resp) => {
                return resp
                    .json()
                    .await
                    .unwrap_or_else(|e| panic!("invalid json from {url}: {e}"));
            }
            Err(err) => {
                last_err = Some(err);
                if i + 1 < total {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
            }
        }
    }
    panic!(
        "request to {url} failed after {total} attempts: {:?}",
        last_err
    );
}

async fn post_json_with_retry(
    client: &reqwest::Client,
    url: &str,
    payload: &Value,
    attempts: usize,
) -> reqwest::Response {
    let total = attempts.max(1);
    for i in 0..total {
        match client
            .post(url)
            .header("Connection", "close")
            .json(payload)
            .send()
            .await
        {
            Ok(resp) => return resp,
            Err(err) if i + 1 < total => {
                eprintln!(
                    "post_json_with_retry attempt {}/{} failed for {}: {}",
                    i + 1,
                    total,
                    url,
                    err
                );
                tokio::time::sleep(Duration::from_millis(120)).await;
            }
            Err(err) => panic!(
                "request failed after {} attempts for {}: {}",
                total, url, err
            ),
        }
    }

    unreachable!("attempt loop must return or panic")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// ACP HTTP /chat must return platform_context.profile_class == "infrastructure".
#[tokio::test(flavor = "current_thread")]
async fn acp_http_chat_response_has_platform_context() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let tmp = tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.toml");
    write_local_echo_config(&cfg);
    let harness = HttpHarness::spawn(&cfg, ephemeral_bind_addr());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build client");
    wait_healthy(&client, &harness.base_url, Duration::from_secs(15)).await;

    let body: Value = post_json_with_retry(
        &client,
        &format!("{}/chat", harness.base_url),
        &json!({
            "mode": "ask",
            "messages": [{"role": "user", "content": "hello parity"}]
        }),
        4,
    )
    .await
    .json()
    .await
    .expect("invalid /chat json");

    assert!(
        body.get("platform_context").is_some(),
        "acp_http:/chat response must contain `platform_context`; got: {body}"
    );
    assert_eq!(
        body["platform_context"]["profile_class"], "infrastructure",
        "acp_http:/chat platform_context.profile_class must be 'infrastructure'; got: {body}"
    );
    assert_eq!(
        body["platform_context"]["schema_version"], "blue24-platform-universal-v1",
        "acp_http:/chat platform_context.schema_version must be 'blue24-platform-universal-v1'"
    );
}

/// ACP HTTP /health must also include platform_context for transport baseline parity.
#[tokio::test(flavor = "current_thread")]
async fn acp_http_health_response_has_platform_context() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let tmp = tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.toml");
    write_local_echo_config(&cfg);
    let harness = HttpHarness::spawn(&cfg, ephemeral_bind_addr());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build client");
    wait_healthy(&client, &harness.base_url, Duration::from_secs(15)).await;

    let body: Value = client
        .get(format!("{}/health", harness.base_url))
        .send()
        .await
        .expect("/health request failed")
        .json()
        .await
        .expect("invalid /health json");

    assert!(
        body.get("platform_context").is_some(),
        "acp_http:/health must contain platform_context; got: {body}"
    );
    assert_eq!(
        body["platform_context"]["profile_class"], "infrastructure",
        "acp_http:/health platform_context.profile_class must be infrastructure"
    );
}

/// ACP HTTP /v1/chat/completions must return platform_context.profile_class == "infrastructure".
#[tokio::test(flavor = "current_thread")]
async fn acp_http_openai_completions_response_has_platform_context() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let tmp = tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.toml");
    write_local_echo_config(&cfg);
    let harness = HttpHarness::spawn(&cfg, ephemeral_bind_addr());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build client");
    wait_healthy(&client, &harness.base_url, Duration::from_secs(15)).await;

    let body: Value = client
        .post(format!("{}/v1/chat/completions", harness.base_url))
        .json(&json!({
            "model": "local-echo",
            "messages": [{"role": "user", "content": "hello parity"}],
            "stream": false
        }))
        .send()
        .await
        .expect("/v1/chat/completions request failed")
        .json()
        .await
        .expect("invalid /v1/chat/completions json");

    assert!(
        body.get("platform_context").is_some(),
        "acp_http:/v1/chat/completions response must contain `platform_context`; got: {body}"
    );
    assert_eq!(
        body["platform_context"]["profile_class"], "infrastructure",
        "acp_http:/v1/chat/completions platform_context.profile_class must be 'infrastructure'"
    );
    assert_eq!(
        body["platform_context"]["schema_version"], "blue24-platform-universal-v1",
        "acp_http:/v1/chat/completions schema_version must be 'blue24-platform-universal-v1'"
    );
}

/// ACP HTTP /v1/responses must return platform_context.profile_class == "infrastructure".
#[tokio::test(flavor = "current_thread")]
async fn acp_http_responses_api_response_has_platform_context() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let tmp = tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.toml");
    write_local_echo_config(&cfg);
    let harness = HttpHarness::spawn(&cfg, ephemeral_bind_addr());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build client");
    wait_healthy(&client, &harness.base_url, Duration::from_secs(15)).await;

    let body: Value = client
        .post(format!("{}/v1/responses", harness.base_url))
        .json(&json!({
            "model": "local-echo",
            "input": "hello parity"
        }))
        .send()
        .await
        .expect("/v1/responses request failed")
        .json()
        .await
        .expect("invalid /v1/responses json");

    assert!(
        body.get("platform_context").is_some(),
        "acp_http:/v1/responses response must contain `platform_context`; got: {body}"
    );
    assert_eq!(
        body["platform_context"]["profile_class"], "infrastructure",
        "acp_http:/v1/responses platform_context.profile_class must be 'infrastructure'"
    );
    assert_eq!(
        body["platform_context"]["schema_version"], "blue24-platform-universal-v1",
        "acp_http:/v1/responses schema_version must be 'blue24-platform-universal-v1'"
    );
}

/// Cross-transport: ACP stdio and ACP HTTP must report identical schema_version,
/// confirming they draw from the same single injection source.
#[tokio::test(flavor = "current_thread")]
async fn acp_stdio_and_acp_http_share_same_schema_version() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let tmp = tempdir().expect("tempdir");

    // --- ACP stdio schema_version ---
    let acp_cfg = tmp.path().join("acp_config.toml");
    write_local_echo_config(&acp_cfg);
    let acp_ver = tokio::task::spawn_blocking(move || {
        use std::io::BufRead;
        let mut child = Command::new(binary_path())
            .arg("--config")
            .arg(&acp_cfg)
            .env("GO_ON_ENABLE_LOCAL_TEST_AGENTS", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn acp_stdio");

        let stdout = child.stdout.take().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","method":"initialize","params":{"protocol":"acp","version":"1.0"},"id":1})
        )
        .unwrap();

        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let resp: Value = serde_json::from_str(&line).expect("invalid acp_stdio json");

        let _ = child.kill();
        let _ = child.wait();

        resp["result"]["platform_context"]["schema_version"]
            .as_str()
            .expect("acp_stdio platform_context.schema_version must be a string")
            .to_string()
    })
    .await
    .expect("acp_stdio task");

    // --- ACP HTTP schema_version ---
    let http_cfg = tmp.path().join("http_config.toml");
    write_local_echo_config(&http_cfg);
    let harness = HttpHarness::spawn(&http_cfg, ephemeral_bind_addr());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build client");
    wait_healthy(&client, &harness.base_url, Duration::from_secs(15)).await;

    let body: Value = post_json_with_retry(
        &client,
        &format!("{}/chat", harness.base_url),
        &json!({
            "mode": "ask",
            "messages": [{"role": "user", "content": "schema version check"}]
        }),
        4,
    )
    .await
    .json()
    .await
    .expect("invalid /chat json");

    let http_ver = body["platform_context"]["schema_version"]
        .as_str()
        .expect("acp_http:/chat platform_context.schema_version must be a string")
        .to_string();

    assert_eq!(
        acp_ver, http_ver,
        "ACP stdio and ACP HTTP must share the same platform_context.schema_version; \
         stdio={acp_ver:?} http={http_ver:?}"
    );
}

/// ACP HTTP compatibility endpoints must keep platform_context on 4xx error payloads.
#[tokio::test(flavor = "current_thread")]
async fn acp_http_error_payloads_keep_platform_context() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let tmp = tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.toml");
    write_local_echo_config(&cfg);
    let harness = HttpHarness::spawn(&cfg, ephemeral_bind_addr());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build client");
    wait_healthy(&client, &harness.base_url, Duration::from_secs(15)).await;

    let bad_openai: Value = client
        .post(format!("{}/v1/chat/completions", harness.base_url))
        .json(&json!({ "messages": [{"role": "user", "content": "missing model"}] }))
        .send()
        .await
        .expect("/v1/chat/completions bad request failed")
        .json()
        .await
        .expect("invalid openai error json");
    assert!(
        bad_openai.get("platform_context").is_some(),
        "openai 4xx payload must include platform_context; got: {bad_openai}"
    );

    let bad_responses: Value = client
        .post(format!("{}/v1/responses", harness.base_url))
        .json(&json!({ "model": "local-echo" }))
        .send()
        .await
        .expect("/v1/responses bad request failed")
        .json()
        .await
        .expect("invalid responses error json");
    assert!(
        bad_responses.get("platform_context").is_some(),
        "responses.api 4xx payload must include platform_context; got: {bad_responses}"
    );
    assert_eq!(
        bad_responses["platform_context"]["profile_class"], "infrastructure",
        "responses.api error platform_context must use infrastructure class"
    );
}

/// ACP HTTP Responses API 502 branch must keep the context-aware writer.
#[tokio::test(flavor = "current_thread")]
async fn acp_http_responses_api_upstream_502_branch_keeps_context_writer() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let runtime_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/acp/impl/runtime/openai_compat.rs");
    let source = fs::read_to_string(&runtime_path).expect("runtime source must be readable");
    // The 502 branch lives in handle_response_create which is called by handle_responses_api.
    let handler_start = source
        .find("async fn handle_response_create(")
        .expect("handle_response_create marker must exist");
    let handler_end = source[handler_start..]
        .find("async fn handle_response_stream(")
        .map(|offset| handler_start + offset)
        .expect("handle_response_stream marker must exist");
    let section = &source[handler_start..handler_end];

    assert!(
        section.contains("write_http_json_response_with_context")
            && section.contains("502")
            && section.contains("responses.api"),
        "responses.api 502 branch must use context-aware writer; section={section}"
    );
    assert!(
        !section.contains("write_http_json_response(socket, 502, payload)"),
        "responses.api 502 branch must not bypass platform_context injection; section={section}"
    );
}

/// ACP HTTP Responses API stream failed event must inject platform_context before SSE write.
/// ACP HTTP /v1/responses/stream failed branch must inject platform_context.
#[tokio::test(flavor = "current_thread")]
async fn acp_http_responses_api_stream_failed_branch_keeps_platform_context() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let runtime_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/acp/impl/runtime/openai_compat.rs");
    let source = fs::read_to_string(&runtime_path).expect("runtime source must be readable");
    // Check inside handle_response_stream for platform_context injection
    let stream_start = source
        .find("async fn handle_response_stream(")
        .expect("handle_response_stream marker must exist");
    let stream_end = source[stream_start..]
        .find("// Tests")
        .map(|offset| stream_start + offset)
        .expect("Tests section marker must exist");
    let section = &source[stream_start..stream_end];

    assert!(
        section.contains("inject_platform_profiles_if_absent(")
            && section.contains("\"responses.api\"")
            && section.contains("inject_platform_profiles_if_absent(failed, \"responses.api\")"),
        "responses.api stream failed branch must inject platform_context before SSE emission"
    );
}

/// ACP HTTP /chat/stream error branches must inject platform_context before SSE write.
#[tokio::test(flavor = "current_thread")]
async fn acp_http_chat_stream_error_branches_keep_platform_context() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let runtime_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/acp/impl/runtime/http.rs");
    let source = fs::read_to_string(&runtime_path).expect("runtime source must be readable");
    // /chat/stream error handling is in route_http_post, after the chat/stream match arm.
    let handler_start = source
        .find("async fn route_http_post(")
        .expect("route_http_post marker must exist");
    let handler_end = source[handler_start..]
        .find("pub(crate) fn compute_cors_response_headers(")
        .map(|offset| handler_start + offset)
        .expect("compute_cors_response_headers marker must exist");
    let section = &source[handler_start..handler_end];

    assert!(
        section.contains("inject_platform_profiles_if_absent(")
            && section.contains("\"chat\""),
        "/chat/stream task error branch must inject platform_context before SSE emission; section={section}"
    );
    assert!(
        section.contains("inject_platform_profiles_if_absent(")
            && section.contains("\"chat\""),
        "/chat/stream panic branch must inject platform_context before SSE emission; section={section}"
    );
}

/// ACP HTTP 405 responses must retain platform_context on method-not-allowed baseline path.
#[tokio::test(flavor = "current_thread")]
async fn acp_http_method_not_allowed_has_platform_context() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let tmp = tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.toml");
    write_local_echo_config(&cfg);
    let harness = HttpHarness::spawn(&cfg, ephemeral_bind_addr());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build client");
    wait_healthy(&client, &harness.base_url, Duration::from_secs(15)).await;

    let body: Value = client
        .delete(format!("{}/v1/responses/some-id", harness.base_url))
        .send()
        .await
        .expect("acp http delete request failed")
        .json()
        .await
        .expect("invalid acp http 405 json");

    let err_msg = body["error"].as_str().unwrap_or_default();
    assert!(
        err_msg.contains("method not allowed") || err_msg.contains("error.method_not_allowed"),
        "acp http 405 payload should keep method-not-allowed error; got: {err_msg}"
    );
    assert!(
        body.get("platform_context").is_some(),
        "acp http 405 payload must include platform_context; got: {body}"
    );
}

/// MCP HTTP errors (unknown method + parse error) must include platform_context in error.data.
#[tokio::test(flavor = "current_thread")]
async fn mcp_http_error_data_keeps_platform_context() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let tmp = tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.toml");
    write_local_echo_config(&cfg);
    let harness = HttpHarness::spawn_with_mode(&cfg, ephemeral_bind_addr(), "mcp_http");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build client");
    wait_healthy(&client, &harness.base_url, Duration::from_secs(15)).await;
    mcp_http_initialize(&client, &harness.base_url).await;

    let unknown: Value = post_mcp_json_with_retry(
        &client,
        &format!("{}/", harness.base_url),
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "blue25.unknown.method",
            "params": {}
        }),
        3,
    )
    .await;

    assert!(
        unknown["error"]["data"].get("platform_context").is_some(),
        "mcp http unknown-method error.data must include platform_context; got: {unknown}"
    );

    let parse: Value = client
        .post(format!("{}/", harness.base_url))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body("{ this is invalid json }")
        .send()
        .await
        .expect("mcp parse error request failed")
        .json()
        .await
        .expect("invalid mcp parse-error json");

    assert!(
        parse["error"]["data"].get("platform_context").is_some(),
        "mcp http parse-error error.data must include platform_context; got: {parse}"
    );
    assert_eq!(
        parse["error"]["data"]["platform_context"]["schema_version"],
        "blue24-platform-universal-v1",
        "mcp http parse-error platform_context.schema_version mismatch"
    );
}

/// MCP HTTP /health must include platform_context for parity with ACP HTTP/stdio health paths.
#[tokio::test(flavor = "current_thread")]
async fn mcp_http_health_response_has_platform_context() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let tmp = tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.toml");
    write_local_echo_config(&cfg);
    let harness = HttpHarness::spawn_with_mode(&cfg, ephemeral_bind_addr(), "mcp_http");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build client");
    wait_healthy(&client, &harness.base_url, Duration::from_secs(15)).await;

    let body: Value = client
        .get(format!("{}/health", harness.base_url))
        .send()
        .await
        .expect("mcp /health request failed")
        .json()
        .await
        .expect("invalid mcp /health json");

    assert!(
        body.get("platform_context").is_some(),
        "mcp_http:/health must contain platform_context; got: {body}"
    );
    assert_eq!(
        body["platform_context"]["schema_version"], "blue24-platform-universal-v1",
        "mcp_http:/health platform_context.schema_version mismatch"
    );
}

/// MCP HTTP must support initialize -> tools/list -> tools/call on the JSON-RPC root path.
#[tokio::test(flavor = "current_thread")]
async fn mcp_http_initialize_list_and_call_succeeds() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let tmp = tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.toml");
    write_local_echo_config(&cfg);
    let sample = tmp.path().join("mcp-http-sample.txt");
    fs::write(&sample, "hello from mcp http").expect("sample file should exist");

    let harness = HttpHarness::spawn_with_mode(&cfg, ephemeral_bind_addr(), "mcp_http");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build client");
    wait_healthy(&client, &harness.base_url, Duration::from_secs(15)).await;

    let init: Value = post_mcp_json_with_retry(
        &client,
        &format!("{}/", harness.base_url),
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "clientInfo": { "name": "test" }
            }
        }),
        3,
    )
    .await;
    assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(init["result"]["serverInfo"]["name"], "go-on");

    let tools: Value = post_mcp_json_with_retry(
        &client,
        &format!("{}/", harness.base_url),
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        3,
    )
    .await;
    let tools_arr = tools["result"]["tools"]
        .as_array()
        .expect("mcp tools/list should return tools array");
    assert!(
        tools_arr.iter().any(|tool| tool["name"] == "read_file"),
        "mcp_http tools/list should expose read_file; got: {tools}"
    );

    let called: Value = post_mcp_json_with_retry(
        &client,
        &format!("{}/", harness.base_url),
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "read_file",
                "arguments": { "path": sample.to_string_lossy().to_string() }
            }
        }),
        3,
    )
    .await;
    assert_eq!(called["result"]["structuredContent"]["success"], true);
    assert_eq!(
        called["result"]["structuredContent"]["result"]["content"],
        "hello from mcp http"
    );
}

/// MCP HTTP 405 responses must keep platform_context on method-not-allowed baseline path.
#[tokio::test(flavor = "current_thread")]
async fn mcp_http_method_not_allowed_has_platform_context() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let tmp = tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.toml");
    write_local_echo_config(&cfg);
    let harness = HttpHarness::spawn_with_mode(&cfg, ephemeral_bind_addr(), "mcp_http");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build client");
    wait_healthy(&client, &harness.base_url, Duration::from_secs(15)).await;

    // Retry the GET request with backoff to handle transient subprocess startup races
    let body = 'retry: {
        for attempt in 1..=3 {
            match client.get(format!("{}/", harness.base_url)).send().await {
                Ok(resp) => match resp.json::<Value>().await {
                    Ok(json) => break 'retry json,
                    Err(_) if attempt < 3 => {
                        sleep(Duration::from_millis(500 * attempt)).await;
                    }
                    _ => {}
                },
                Err(_) if attempt < 3 => {
                    sleep(Duration::from_millis(500 * attempt)).await;
                }
                _ => {}
            }
        }
        panic!("mcp http get / request failed after retries");
    };

    let err_msg = body["error"].as_str().unwrap_or_default();
    assert!(
        err_msg.contains("method not allowed") || err_msg.contains("error.method_not_allowed"),
        "mcp http 405 payload should keep method-not-allowed error; got: {err_msg}"
    );
    assert!(
        body.get("platform_context").is_some(),
        "mcp http 405 payload must include platform_context; got: {body}"
    );
    assert_eq!(
        body["platform_context"]["schema_version"], "blue24-platform-universal-v1",
        "mcp http 405 platform_context.schema_version mismatch"
    );
}

/// MCP HTTP cancellation semantics: notifications/cancelled must suppress subsequent
/// request execution for the same request id.
#[tokio::test(flavor = "current_thread")]
async fn mcp_http_cancel_notification_blocks_matching_request_id() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let tmp = tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.toml");
    write_local_echo_config(&cfg);
    let harness = HttpHarness::spawn_with_mode(&cfg, ephemeral_bind_addr(), "mcp_http");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build client");
    wait_healthy(&client, &harness.base_url, Duration::from_secs(15)).await;

    let cancel_resp: Value = client
        .post(format!("{}/", harness.base_url))
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": {
                "requestId": 7001,
                "reason": "client_abort"
            }
        }))
        .send()
        .await
        .expect("mcp cancel notification request failed")
        .json()
        .await
        .expect("invalid mcp cancel response json");

    assert_eq!(
        cancel_resp,
        Value::Null,
        "notification should return null body"
    );

    let called: Value = client
        .post(format!("{}/", harness.base_url))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 7001,
            "method": "tools/call",
            "params": {
                "name": "read_file",
                "arguments": { "path": "/tmp/ignored-by-cancel" }
            }
        }))
        .send()
        .await
        .expect("mcp tools/call request failed")
        .json()
        .await
        .expect("invalid mcp tools/call cancel json");

    assert_eq!(
        called["error"]["code"],
        json!(-32800),
        "cancelled request should return REQUEST_CANCELLED code"
    );
    assert!(
        called["error"]["data"].get("platform_context").is_some(),
        "cancelled request error must preserve platform_context"
    );
}

#[test]
fn acp_http_route_inventory_changes_require_transport_gate_update() {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: Suite-level serialization lock; held only in sync block
        // (catch_unwind) — no .await in this scope.
        let _guard = lock_suite_guard();
        use std::collections::BTreeSet;

        let runtime_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/acp/impl/runtime/http.rs");
        let source = fs::read_to_string(&runtime_path).expect("runtime source must be readable");
        // Routes are split between route_http_get (GET) and route_http_post (POST).
        let get_start = source
            .find("async fn route_http_get(")
            .expect("route_http_get marker must exist");
        let get_end = source[get_start..]
            .find("async fn route_http_post(")
            .map(|offset| get_start + offset)
            .expect("route_http_post marker must exist");
        let post_start = source
            .find("async fn route_http_post(")
            .expect("route_http_post marker must exist");
        let post_end = source[post_start..]
            .find("pub(crate) fn compute_cors_response_headers(")
            .map(|offset| post_start + offset)
            .expect("compute_cors_response_headers marker must exist");
        let combined = format!(
            "{}\n{}",
            &source[get_start..get_end],
            &source[post_start..post_end]
        );

        let mut discovered = BTreeSet::new();
        let bytes = combined.as_bytes();
        let mut idx = 0usize;
        while idx < bytes.len() {
            if bytes[idx] == b'"' {
                let rest = &combined[idx + 1..];
                if let Some(close) = rest.find('"') {
                    let candidate = &rest[..close];
                    if candidate.starts_with('/')
                        && !candidate.contains('{')
                        && !candidate.contains(' ')
                        && !candidate.contains("\\r")
                        && !candidate.contains('\n')
                        && !candidate.contains("//")
                    {
                        discovered.insert(candidate.to_string());
                    }
                    idx += close + 2;
                    continue;
                }
            }
            idx += 1;
        }

        let expected = BTreeSet::from([
            "/".to_string(),
            "/chat".to_string(),
            "/chat/chat/completions".to_string(),
            "/chat/completions".to_string(),
            "/chat/stream".to_string(),
            "/health".to_string(),
            "/health/ready".to_string(),
            "/metrics".to_string(),
            "/models".to_string(),
            "/protocol/version".to_string(),
            "/rpc".to_string(),
            "/v1/chat/completions".to_string(),
            "/v1/model".to_string(),
            "/v1/models".to_string(),
            "/v1/responses".to_string(),
            "/v1/state/events".to_string(),
        ]);

        assert_eq!(
            discovered, expected,
            "ACP HTTP route inventory changed. Update transport parity coverage and this gate before adding new endpoints. discovered={discovered:?} expected={expected:?}"
        );
    }));
    if let Err(e) = result {
        panic!(
            "acp_http_route_inventory_changes_require_transport_gate_update panicked: {:?}",
            e
        );
    }
}

// ---------------------------------------------------------------------------
// BLUE43 Step 18 — MCP streaming/cancel/timeout consistency across transports
// ---------------------------------------------------------------------------

/// MCP stdio and HTTP must produce identical response shapes for the same
/// `tools/call` invocation, proving that the shared `McpServer::handle_request`
/// code path is equivalent regardless of transport layer.
#[tokio::test(flavor = "current_thread")]
async fn mcp_stdio_and_http_tool_call_shapes_match() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let tmp = tempdir().expect("tempdir");

    // --- MCP stdio tool call ---
    let stdio_cfg = tmp.path().join("stdio_config.toml");
    write_local_echo_config(&stdio_cfg);
    let stdio_resp = tokio::task::spawn_blocking(move || {
        use std::io::BufRead;
        let mut child = Command::new(binary_path())
            .arg("--config")
            .arg(&stdio_cfg)
            .arg("--protocol-mode")
            .arg("mcp_stdio")
            .env("GO_ON_ENABLE_LOCAL_TEST_AGENTS", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn mcp_stdio harness");

        let stdout = child.stdout.take().unwrap();
        let mut stdin = child.stdin.take().unwrap();

        // Initialize
        writeln!(
            stdin,
            "{}",
            json!({
                "jsonrpc": "2.0",
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "clientInfo": { "name": "test" }
                },
                "id": 100
            })
        )
        .unwrap();

        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();

        // Read init response
        line.clear();
        reader.read_line(&mut line).expect("read init response");
        let init_resp: Value = serde_json::from_str(&line).expect("mcp stdio init response parse");
        assert!(
            init_resp.get("result").is_some(),
            "mcp stdio init should succeed; got: {}",
            init_resp
        );

        // Send tools/list and read response
        writeln!(
            stdin,
            "{}",
            json!({
                "jsonrpc": "2.0",
                "method": "tools/list",
                "id": 101
            })
        )
        .unwrap();
        stdin.flush().unwrap();

        line.clear();
        reader
            .read_line(&mut line)
            .expect("read tools/list response");
        let list_resp: Value =
            serde_json::from_str(&line).expect("mcp stdio tools/list response parse");

        let _ = child.kill();
        let _ = child.wait();
        list_resp
    })
    .await
    .expect("stdio blocking task");

    // --- MCP HTTP tool list ---
    let http_cfg = tmp.path().join("http_config_mcp.toml");
    write_local_echo_config(&http_cfg);
    let harness = HttpHarness::spawn_with_mode(&http_cfg, ephemeral_bind_addr(), "mcp_http");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build client");
    wait_healthy(&client, &harness.base_url, Duration::from_secs(15)).await;
    mcp_http_initialize(&client, &harness.base_url).await;

    let http_resp: Value = post_mcp_json_with_retry(
        &client,
        &format!("{}/", harness.base_url),
        &json!({
            "jsonrpc": "2.0",
            "method": "tools/list",
            "id": 2
        }),
        3,
    )
    .await;

    // Both responses must be success shapes with identical key structure
    assert!(
        stdio_resp.get("result").is_some(),
        "stdio tools/list must have result"
    );
    assert!(
        http_resp.get("result").is_some(),
        "http tools/list must have result"
    );
    assert!(
        stdio_resp.get("error").is_none(),
        "stdio tools/list must not have error"
    );
    assert!(
        http_resp.get("error").is_none(),
        "http tools/list must not have error"
    );

    // Both results must have a `tools` array
    let stdio_tools = stdio_resp["result"]["tools"]
        .as_array()
        .expect("stdio tools list must be array");
    let http_tools = http_resp["result"]["tools"]
        .as_array()
        .expect("http tools list must be array");

    // Tool names must match across transports
    let stdio_names: Vec<&str> = stdio_tools
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .collect();
    let http_names: Vec<&str> = http_tools
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .collect();

    assert_eq!(
        stdio_names.len(),
        http_names.len(),
        "stdio and http must report same number of tools"
    );
    for name in &stdio_names {
        assert!(
            http_names.contains(name),
            "http tools/list missing tool '{}' present in stdio",
            name
        );
    }
}

/// MCP stdio and HTTP timeout handling must produce equivalent error codes.
#[tokio::test(flavor = "current_thread")]
async fn mcp_stdio_and_http_timeout_codes_match() {
    // SAFETY: Suite-level serialization lock; intentionally held across
    // .await to serialize test execution. No cross-runtime deadlock risk.
    let _guard = lock_suite_guard();
    let tmp = tempdir().expect("tempdir");

    // --- MCP stdio timeout ---
    let stdio_cfg = tmp.path().join("stdio_timeout_config.toml");
    write_local_echo_config(&stdio_cfg);
    let stdio_code = tokio::task::spawn_blocking(move || {
        use std::io::BufRead;
        let mut child = Command::new(binary_path())
            .arg("--config")
            .arg(&stdio_cfg)
            .arg("--protocol-mode")
            .arg("mcp_stdio")
            .env("GO_ON_ENABLE_LOCAL_TEST_AGENTS", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn mcp_stdio for timeout");

        let stdout = child.stdout.take().unwrap();
        let mut stdin = child.stdin.take().unwrap();

        // Initialize first
        writeln!(
            stdin,
            "{}",
            json!({
                "jsonrpc": "2.0",
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "clientInfo": { "name": "test" }
                },
                "id": 1
            })
        )
        .unwrap();

        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();
        // Read init response
        line.clear();
        reader.read_line(&mut line).expect("read init response");
        let init_resp: Value = serde_json::from_str(&line).expect("parse init response");
        assert!(init_resp.get("result").is_some(), "init should succeed");

        // Now send a tool call with an extremely small timeout
        writeln!(
            stdin,
            "{}",
            json!({
                "jsonrpc": "2.0",
                "method": "tools/call",
                "params": {
                    "name": "read_file",
                    "arguments": { "path": "/tmp/test-timeout.txt" },
                    "timeoutMs": 1
                },
                "id": 99
            })
        )
        .unwrap();
        stdin.flush().unwrap();

        // Read timeout response
        line.clear();
        reader.read_line(&mut line).expect("read timeout response");
        let resp: Value = serde_json::from_str(&line).expect("parse timeout response");
        let code = resp["error"]["code"].as_i64().unwrap_or(0);

        let _ = child.kill();
        let _ = child.wait();
        code
    })
    .await
    .expect("stdio timeout blocking task");

    // --- MCP HTTP timeout ---
    let http_cfg = tmp.path().join("http_timeout_config.toml");
    write_local_echo_config(&http_cfg);
    let harness = HttpHarness::spawn_with_mode(&http_cfg, ephemeral_bind_addr(), "mcp_http");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build client");
    wait_healthy(&client, &harness.base_url, Duration::from_secs(15)).await;
    mcp_http_initialize(&client, &harness.base_url).await;

    let http_resp: Value = post_mcp_json_with_retry(
        &client,
        &format!("{}/", harness.base_url),
        &json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": {
                "name": "read_file",
                "arguments": { "path": "/tmp/test-timeout.txt" },
                "timeoutMs": 1
            },
            "id": 99
        }),
        3,
    )
    .await;

    let http_code = http_resp["error"]["code"].as_i64().unwrap_or(0);

    // Both must return REQUEST_TIMEOUT (-32801)
    assert_eq!(
        stdio_code, -32801,
        "mcp stdio timeout must return REQUEST_TIMEOUT (-32801); got {}",
        stdio_code
    );
    assert_eq!(
        http_code, -32801,
        "mcp http timeout must return REQUEST_TIMEOUT (-32801); got {}",
        http_code
    );
    assert_eq!(
        stdio_code, http_code,
        "mcp stdio and http timeout error codes must match; stdio={} http={}",
        stdio_code, http_code
    );
}
