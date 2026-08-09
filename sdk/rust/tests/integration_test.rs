//! Basic integration test for go-on Rust SDK.
//!
//! These tests verify client construction and configuration.
//! They do not require a running backend — they only test that the
//! SDK types and builders are wired correctly.

use go_on_sdk::GoOnClientBuilder;

#[test]
fn test_client_builder_defaults() {
    let client = GoOnClientBuilder::new("http://localhost:8090")
        .build()
        .expect("building client with defaults should succeed");

    assert_eq!(
        client.base_url(),
        "http://localhost:8090",
        "base_url should be set to the value passed to new()"
    );
    assert_eq!(client.max_retries(), 3, "default max_retries should be 3");
}

#[test]
fn test_client_builder_custom_timeout() {
    let client = GoOnClientBuilder::new("http://localhost:8090")
        .with_timeout(std::time::Duration::from_secs(60))
        .build()
        .expect("building client with custom timeout should succeed");

    assert_eq!(
        client.timeout(),
        Some(std::time::Duration::from_secs(60)),
        "custom timeout should be applied"
    );
}

#[test]
fn test_client_builder_custom_retries() {
    let client = GoOnClientBuilder::new("http://localhost:8090")
        .with_max_retries(5)
        .with_retry_delay(std::time::Duration::from_secs(2))
        .build()
        .expect("building client with custom retries should succeed");

    assert_eq!(
        client.max_retries(),
        5,
        "custom max_retries should be applied"
    );
    assert_eq!(
        client.retry_delay(),
        std::time::Duration::from_secs(2),
        "custom retry_delay should be applied"
    );
}

#[test]
fn test_builder_chain_all_options() {
    let client = GoOnClientBuilder::new("http://localhost:8090")
        .with_timeout(std::time::Duration::from_secs(15))
        .with_max_retries(10)
        .with_retry_delay(std::time::Duration::from_millis(500))
        .build()
        .expect("building client with all options should succeed");

    assert_eq!(
        client.base_url(),
        "http://localhost:8090",
        "base_url should be preserved when chaining options"
    );
    assert_eq!(
        client.timeout(),
        Some(std::time::Duration::from_secs(15)),
        "timeout should be set when chaining options"
    );
    assert_eq!(
        client.max_retries(),
        10,
        "max_retries should be set when chaining options"
    );
    assert_eq!(
        client.retry_delay(),
        std::time::Duration::from_millis(500),
        "retry_delay should be set when chaining options"
    );
}

#[test]
fn test_error_sdk_display() {
    use go_on_sdk::SdkError;

    let err = SdkError::Timeout { elapsed_secs: 30 };
    let msg = err.to_string();
    assert!(
        msg.contains("30"),
        "Timeout error should include elapsed seconds"
    );
}

// ---------------------------------------------------------------------------
// ACP contract parameter tests
//
// The Rust SDK has no HTTP mock facility (it uses reqwest directly), so these
// tests pin the wire contract by serializing the request types and asserting
// the exact JSON keys the backend reads (src/acp/impl/request/).
// ---------------------------------------------------------------------------

#[test]
fn test_session_new_request_serializes_backend_contract() {
    use go_on_sdk::AcpSessionNewRequest;

    let request = AcpSessionNewRequest {
        mode: Some("safeguard".to_string()),
        cwd: Some("/tmp".to_string()),
        work_dirs: vec!["/tmp/a".to_string()],
        additional_directories: vec!["/tmp/b".to_string()],
    };

    let value = serde_json::to_value(&request).expect("request should serialize");
    assert_eq!(value["mode"], "safeguard");
    assert_eq!(value["cwd"], "/tmp");
    assert_eq!(value["work_dirs"], serde_json::json!(["/tmp/a"]));
    assert_eq!(
        value["additionalDirectories"],
        serde_json::json!(["/tmp/b"])
    );
}

#[test]
fn test_session_new_request_omits_unset_fields() {
    use go_on_sdk::AcpSessionNewRequest;

    let request = AcpSessionNewRequest::default();
    let value = serde_json::to_value(&request).expect("request should serialize");
    assert_eq!(
        value,
        serde_json::json!({}),
        "unset fields should be omitted"
    );
}

#[test]
fn test_session_prompt_request_serializes_backend_contract() {
    use go_on_sdk::{AcpSessionPromptRequest, PromptContentBlock, PromptContentBlockType};

    let request = AcpSessionPromptRequest {
        session_id: "sess-1".to_string(),
        prompt: vec![PromptContentBlock {
            kind: PromptContentBlockType::Text,
            text: Some("Hello".to_string()),
            uri: None,
            name: None,
            resource: None,
        }],
        mode: None,
        cwd: Some("/tmp".to_string()),
        additional_directories: vec![],
    };

    let value = serde_json::to_value(&request).expect("request should serialize");
    assert_eq!(value["sessionId"], "sess-1");
    assert_eq!(value["prompt"][0]["type"], "text");
    assert_eq!(value["prompt"][0]["text"], "Hello");
    assert_eq!(value["cwd"], "/tmp");
}

#[test]
fn test_session_list_response_parses_minimal_shape() {
    use go_on_sdk::AcpSessionListResponse;

    // Backend `session/list` emits `[{ "id": sid }]` — no mode/cwd/timestamps.
    let json = serde_json::json!({
        "sessions": [{ "id": "sess-1" }]
    });
    let parsed: AcpSessionListResponse =
        serde_json::from_value(json).expect("response should parse");
    assert_eq!(parsed.sessions.len(), 1);
    assert_eq!(parsed.sessions[0].id, "sess-1");
}

#[test]
fn test_tool_info_parses_backend_descriptor() {
    use go_on_sdk::ToolInfo;

    // Backend `tools/list` emits descriptors with the snake_case input_schema key.
    let json = serde_json::json!({
        "name": "read_file",
        "description": "Read a file",
        "input_schema": { "type": "object" },
    });
    let info: ToolInfo = serde_json::from_value(json).expect("tool descriptor should parse");
    assert_eq!(info.name, "read_file");
    assert_eq!(info.description, "Read a file");
    assert_eq!(info.input_schema["type"], "object");
}

// ---------------------------------------------------------------------------
// OpenAI-compatible endpoint contract tests
//
// The Rust SDK has no HTTP mock facility, so these tests spin up a minimal
// in-process TCP HTTP server that captures the request line + body and answers
// with a canned OpenAI-shaped response. This verifies the real wire contract:
// method, path and verbatim body forwarding.
// ---------------------------------------------------------------------------

/// Result of capturing one HTTP request on the in-process test server.
struct CapturedRequest {
    method: String,
    path: String,
    body: String,
}

/// Spawn a minimal HTTP server on 127.0.0.1:0 that captures the first request
/// and answers with `response_json`. Returns (base_url, captured_receiver).
/// Must be called from inside a tokio runtime (tokio::net::TcpListener).
async fn spawn_capture_server(
    response_json: serde_json::Value,
) -> (String, tokio::sync::oneshot::Receiver<CapturedRequest>) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener local addr");
    let response_body = response_json.to_string();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept test connection");
        let mut buffer = [0u8; 8192];
        let n = tokio::io::AsyncReadExt::read(&mut socket, &mut buffer)
            .await
            .expect("read request head");
        let head = String::from_utf8_lossy(&buffer[..n]).to_string();
        let mut lines = head.lines();
        let request_line = lines.next().unwrap_or_default();
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let path = parts.next().unwrap_or_default().to_string();

        // Extract the body after the blank line (single-shot test server:
        // the whole request is assumed to arrive in one read).
        let body = match head.find("\r\n\r\n") {
            Some(idx) => head[idx + 4..].to_string(),
            None => String::new(),
        };

        let _ = tx.send(CapturedRequest { method, path, body });

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );
        let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes()).await;
    });

    (format!("http://{}", addr), rx)
}

#[tokio::test]
async fn test_chat_completions_posts_openai_wire_format() {
    let response = serde_json::json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}}],
    });
    let (base_url, rx) = spawn_capture_server(response).await;
    let client = go_on_sdk::GoOnClient::new(base_url);

    let result = client
        .chat_completions(serde_json::json!({
            "model": "go-on",
            "messages": [{"role": "user", "content": "hi"}],
        }))
        .await
        .expect("chat_completions should succeed");

    assert_eq!(result["object"], "chat.completion");
    let captured = rx.await.expect("server should capture one request");
    assert_eq!(captured.method, "POST");
    assert_eq!(captured.path, "/v1/chat/completions");
    let sent: serde_json::Value =
        serde_json::from_str(&captured.body).expect("request body should be JSON");
    assert_eq!(sent["model"], "go-on");
    assert_eq!(sent["messages"][0]["role"], "user");
    assert_eq!(sent["messages"][0]["content"], "hi");
}

#[tokio::test]
async fn test_responses_create_posts_to_v1_responses() {
    let response = serde_json::json!({
        "id": "resp_1",
        "object": "response",
        "status": "completed",
        "output": [],
    });
    let (base_url, rx) = spawn_capture_server(response).await;
    let client = go_on_sdk::GoOnClient::new(base_url);

    let result = client
        .responses_create(serde_json::json!({
            "model": "go-on",
            "input": "hi",
        }))
        .await
        .expect("responses_create should succeed");

    assert_eq!(result["status"], "completed");
    let captured = rx.await.expect("server should capture one request");
    assert_eq!(captured.method, "POST");
    assert_eq!(captured.path, "/v1/responses");
    let sent: serde_json::Value =
        serde_json::from_str(&captured.body).expect("request body should be JSON");
    assert_eq!(sent["model"], "go-on");
    assert_eq!(sent["input"], "hi");
}

#[tokio::test]
async fn test_responses_get_targets_v1_responses_id() {
    let response = serde_json::json!({
        "id": "resp_1",
        "object": "response",
        "status": "completed",
    });
    let (base_url, rx) = spawn_capture_server(response).await;
    let client = go_on_sdk::GoOnClient::new(base_url);

    let result = client
        .responses_get("resp_1")
        .await
        .expect("responses_get should succeed");

    assert_eq!(result["id"], "resp_1");
    let captured = rx.await.expect("server should capture one request");
    assert_eq!(captured.method, "GET");
    assert_eq!(captured.path, "/v1/responses/resp_1");
}

#[tokio::test]
async fn test_models_list_targets_v1_models() {
    let response = serde_json::json!({
        "object": "list",
        "data": [{"id": "go-on", "object": "model"}],
    });
    let (base_url, rx) = spawn_capture_server(response).await;
    let client = go_on_sdk::GoOnClient::new(base_url);

    let result = client
        .models_list()
        .await
        .expect("models_list should succeed");

    assert_eq!(result["data"][0]["id"], "go-on");
    let captured = rx.await.expect("server should capture one request");
    assert_eq!(captured.method, "GET");
    assert_eq!(captured.path, "/v1/models");
}
