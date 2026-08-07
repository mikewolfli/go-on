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
