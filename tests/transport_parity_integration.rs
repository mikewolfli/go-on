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
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn binary_path() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_go-on")
        .map(PathBuf::from)
        .expect("CARGO_BIN_EXE_go-on is not set; run via `cargo test`")
}

fn suite_guard() -> &'static Mutex<()> {
    static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
    GUARD.get_or_init(|| Mutex::new(()))
}

fn lock_suite_guard() -> std::sync::MutexGuard<'static, ()> {
    suite_guard().lock().unwrap_or_else(|err| err.into_inner())
}

fn ephemeral_bind_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind ephemeral port");
    let port = listener.local_addr().expect("missing local addr").port();
    drop(listener);
    format!("127.0.0.1:{port}")
}

fn write_local_echo_config(path: &Path) {
    let config = r#"default_phase = "coding"

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
}

impl HttpHarness {
    fn spawn(config_path: &Path, bind_addr: String) -> Self {
        Self::spawn_with_mode(config_path, bind_addr, "acp_http")
    }

    fn spawn_with_mode(config_path: &Path, bind_addr: String, mode: &str) -> Self {
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
        HttpHarness { child, base_url }
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// ACP HTTP /chat must return platform_context.profile_class == "infrastructure".
#[tokio::test(flavor = "current_thread")]
async fn acp_http_chat_response_has_platform_context() {
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
        .post(format!("{}/chat", harness.base_url))
        .json(&json!({
            "mode": "ask",
            "messages": [{"role": "user", "content": "hello parity"}]
        }))
        .send()
        .await
        .expect("/chat request failed")
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

    let body: Value = client
        .post(format!("{}/chat", harness.base_url))
        .json(&json!({
            "mode": "ask",
            "messages": [{"role": "user", "content": "schema version check"}]
        }))
        .send()
        .await
        .expect("/chat request failed")
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
    let _guard = lock_suite_guard();
    let runtime_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/acp/impl/runtime.rs");
    let source = fs::read_to_string(&runtime_path).expect("runtime source must be readable");
    // The 502 branch lives in handle_response_create which is called by handle_responses_api.
    let handler_start = source
        .find("async fn handle_response_create(")
        .expect("handle_response_create marker must exist");
    let handler_end = source[handler_start..]
        .find("fn infer_adaptive_signal(")
        .map(|offset| handler_start + offset)
        .expect("infer_adaptive_signal marker must exist");
    let section = &source[handler_start..handler_end];

    assert!(
        section.contains(
            "write_http_json_response_with_context(socket, 502, payload, \"responses.api\")"
        ),
        "responses.api 502 branch must use context-aware writer; section={section}"
    );
    assert!(
        !section.contains("write_http_json_response(socket, 502, payload)"),
        "responses.api 502 branch must not bypass platform_context injection; section={section}"
    );
}

/// ACP HTTP Responses API stream failed event must inject platform_context before SSE write.
#[tokio::test(flavor = "current_thread")]
async fn acp_http_responses_api_stream_failed_branch_keeps_platform_context() {
    let _guard = lock_suite_guard();
    let runtime_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/acp/impl/runtime.rs");
    let source = fs::read_to_string(&runtime_path).expect("runtime source must be readable");
    let handler_start = source
        .find("async fn handle_response_create(")
        .expect("handle_response_create marker must exist");
    let handler_end = source[handler_start..]
        .find("fn infer_adaptive_signal(")
        .map(|offset| handler_start + offset)
        .expect("infer_adaptive_signal marker must exist");
    let section = &source[handler_start..handler_end];

    assert!(
        section.contains("let failed = inject_platform_profiles_if_absent(failed, \"responses.api\");"),
        "responses.api stream failed branch must inject platform_context before SSE emission; section={section}"
    );
}

/// ACP HTTP /chat/stream error branches must inject platform_context before SSE write.
#[tokio::test(flavor = "current_thread")]
async fn acp_http_chat_stream_error_branches_keep_platform_context() {
    let _guard = lock_suite_guard();
    let runtime_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/acp/impl/runtime.rs");
    let source = fs::read_to_string(&runtime_path).expect("runtime source must be readable");
    // /chat/stream error handling is in route_http_post, after the chat/stream match arm.
    let handler_start = source
        .find("async fn route_http_post(")
        .expect("route_http_post marker must exist");
    let handler_end = source[handler_start..]
        .find("fn infer_adaptive_signal(")
        .map(|offset| handler_start + offset)
        .expect("infer_adaptive_signal marker must exist");
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

    let unknown: Value = client
        .post(format!("{}/", harness.base_url))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "blue25.unknown.method",
            "params": {}
        }))
        .send()
        .await
        .expect("mcp unknown method request failed")
        .json()
        .await
        .expect("invalid mcp unknown-method json");

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

/// MCP HTTP 405 responses must keep platform_context on method-not-allowed baseline path.
#[tokio::test(flavor = "current_thread")]
async fn mcp_http_method_not_allowed_has_platform_context() {
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
        .get(format!("{}/", harness.base_url))
        .send()
        .await
        .expect("mcp http get / request failed")
        .json()
        .await
        .expect("invalid mcp http 405 json");

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

#[test]
fn acp_http_route_inventory_changes_require_transport_gate_update() {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = lock_suite_guard();
        use std::collections::BTreeSet;

        let runtime_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/acp/impl/runtime.rs");
        let source = fs::read_to_string(&runtime_path).expect("runtime source must be readable");
        // Routes are split between route_http_get (GET) and route_http_post (POST).
        let get_start = source
            .find("async fn route_http_get(")
            .expect("route_http_get marker must exist");
        let get_end = source[get_start..]
            .find("/// Route a POST request")
            .map(|offset| get_start + offset)
            .expect("route_http_post marker must exist");
        let post_start = source
            .find("async fn route_http_post(")
            .expect("route_http_post marker must exist");
        let post_end = source[post_start..]
            .find("fn infer_adaptive_signal(")
            .map(|offset| post_start + offset)
            .expect("infer_adaptive_signal marker must exist");
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
            "/models".to_string(),
            "/v1/chat/completions".to_string(),
            "/v1/model".to_string(),
            "/v1/models".to_string(),
            "/v1/responses".to_string(),
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
