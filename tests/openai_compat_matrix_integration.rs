use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::header::CONTENT_TYPE;
use serde_json::{json, Value};
use tempfile::tempdir;

struct HttpHarness {
    child: Child,
    bind_addr: String,
}

impl HttpHarness {
    fn spawn(config_path: &Path, bind_addr: String) -> Self {
        let child = Command::new(binary_path())
            .arg("--config")
            .arg(config_path)
            .arg("--acp-http-bind")
            .arg(&bind_addr)
            .env("GO_ON_ENABLE_LOCAL_TEST_AGENTS", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn go-on http harness");

        Self { child, bind_addr }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.bind_addr)
    }
}

impl Drop for HttpHarness {
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

fn ephemeral_bind_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind ephemeral port");
    let port = listener.local_addr().expect("missing local addr").port();
    drop(listener);
    format!("127.0.0.1:{port}")
}

fn write_http_test_config(path: &Path) {
    let config = r#"default_phase = "coding"

[flow]
name = "HTTP Compatibility Flow"
phases = ["coding"]

[runtime]
maintenance_interval_seconds = 10
health_interval_seconds = 10
shutdown_drain_seconds = 3

[agents.local_echo]
type = "local_echo"

[phases.coding]
description = "Coding"
agents = ["local_echo"]
fallback = true
"#;

    fs::write(path, config).expect("failed to write test config");
}

fn write_http_unavailable_provider_config(path: &Path) {
    let config = r#"default_phase = "coding"

[flow]
name = "HTTP Unavailable Provider"
phases = ["coding"]

[runtime]
maintenance_interval_seconds = 10
health_interval_seconds = 10
shutdown_drain_seconds = 3

[agents.bad_provider]
type = "copilot"
url = "http://127.0.0.1:65535"

[phases.coding]
description = "Coding"
agents = ["bad_provider"]
fallback = true

[phases.coding.options]
request_timeout_seconds = 1
"#;

    fs::write(path, config).expect("failed to write unavailable provider config");
}

async fn wait_healthy(client: &reqwest::Client, base_url: &str, timeout: Duration) {
    // CI hosts can be bursty; enforce a reasonable minimum startup window.
    let effective_timeout = timeout.max(Duration::from_secs(60));
    let deadline = Instant::now() + effective_timeout;
    let mut last_error = String::new();
    while Instant::now() < deadline {
        match client.get(format!("{base_url}/health")).send().await {
            Ok(resp) if resp.status().is_success() => return,
            Ok(resp) => {
                last_error = format!("health status={}", resp.status());
            }
            Err(err) => {
                last_error = err.to_string();
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    panic!(
        "timed out waiting for /health after {:?}, last_error={}",
        effective_timeout, last_error
    );
}

#[tokio::test(flavor = "current_thread")]
async fn openai_http_request_matrix_regression() {
    let dir = tempdir().expect("failed to create tempdir");
    let config_path = dir.path().join("config.toml");
    write_http_test_config(&config_path);

    let harness = HttpHarness::spawn(&config_path, ephemeral_bind_addr());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .expect("failed to build reqwest client");

    wait_healthy(&client, &harness.base_url(), Duration::from_secs(10)).await;

    let models_json: Value = client
        .get(format!("{}/v1/models", harness.base_url()))
        .send()
        .await
        .expect("/v1/models request failed")
        .json()
        .await
        .expect("invalid /v1/models json");
    assert!(models_json["data"]
        .as_array()
        .map(|arr| !arr.is_empty())
        .unwrap_or(false));

    let model_alias_json: Value = client
        .get(format!("{}/v1/model", harness.base_url()))
        .send()
        .await
        .expect("/v1/model request failed")
        .json()
        .await
        .expect("invalid /v1/model json");
    assert!(model_alias_json["data"]
        .as_array()
        .map(|arr| !arr.is_empty())
        .unwrap_or(false));

    let request_matrix = vec![
        json!({
            "model": "local-echo",
            "messages": [{"role":"user","content":"hello matrix"}],
            "stream": false,
        }),
        json!({
            "model": "local-echo",
            "messages": [
                {"role":"system","content":"be concise"},
                {"role":"user","content":[{"type":"text","text":"hello"},{"type":"text","text":"world"}]}
            ],
            "temperature": 0.2,
            "top_p": 0.9,
            "max_tokens": 64,
            "stop": ["END"],
            "response_format": {"type":"json_object"},
            "tools": [{"type":"function","function":{"name":"noop","parameters":{"type":"object"}}}],
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "stream": false,
        }),
        json!({
            "model": "local-echo",
            "messages": [
                {"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"sum","arguments":"{}"}}]},
                {"role":"tool","tool_call_id":"call_1","content":{"result":3}},
                {"role":"user","content":"final user prompt"}
            ],
            "function_call": "auto",
            "functions": [{"name":"sum","parameters":{"type":"object"}}],
            "stream": false,
        }),
    ];

    for payload in request_matrix {
        let body: Value = client
            .post(format!("{}/v1/chat/completions", harness.base_url()))
            .json(&payload)
            .send()
            .await
            .expect("chat completion request failed")
            .json()
            .await
            .expect("invalid completion json");
        assert!(
            body.get("error").is_none(),
            "unexpected completion error: {body}"
        );

        let text = body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default();
        assert!(
            !text.is_empty(),
            "assistant content should be non-empty for payload: {payload}"
        );
    }

    let stream_payload = json!({
        "model": "local-echo",
        "messages": [{"role":"user","content":"stream ok"}],
        "stream": true,
    });

    let stream_text = client
        .post(format!("{}/v1/chat/completions", harness.base_url()))
        .json(&stream_payload)
        .send()
        .await
        .expect("stream request failed")
        .text()
        .await
        .expect("failed to read stream response");

    assert!(
        stream_text.contains("data: "),
        "stream output missing SSE data frame"
    );
    assert!(
        stream_text.contains("[DONE]"),
        "stream output missing done marker"
    );
}

/// Phase R1 baseline: verify /v1/responses returns a structured `response` object,
/// and that missing required fields return a Responses-style error.
#[tokio::test(flavor = "current_thread")]
async fn responses_api_r1_minimal_request() {
    let dir = tempdir().expect("failed to create tempdir");
    let config_path = dir.path().join("config.toml");
    write_http_test_config(&config_path);

    let harness = HttpHarness::spawn(&config_path, ephemeral_bind_addr());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .expect("failed to build reqwest client");

    wait_healthy(&client, &harness.base_url(), Duration::from_secs(10)).await;

    // 1. Happy path: minimal valid request.
    let valid_payload = json!({
        "model": "local-echo",
        "input": "hello from responses api",
    });
    let resp: Value = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&valid_payload)
        .send()
        .await
        .expect("/v1/responses request failed")
        .json()
        .await
        .expect("invalid /v1/responses json");

    assert_eq!(
        resp["object"].as_str().unwrap_or_default(),
        "response",
        "object field must be 'response', got: {resp}"
    );
    let output = resp["output"].as_array().expect("output must be an array");
    assert!(!output.is_empty(), "output must be non-empty");
    assert_eq!(
        output[0]["type"].as_str().unwrap_or_default(),
        "message",
        "first output item type must be 'message'"
    );
    let content = output[0]["content"]
        .as_array()
        .expect("content must be an array");
    assert!(!content.is_empty(), "content must be non-empty");
    assert_eq!(
        content[0]["type"].as_str().unwrap_or_default(),
        "output_text",
        "first content item type must be 'output_text'"
    );
    let response_id = resp["id"].as_str().expect("response id should exist");
    assert!(
        response_id.starts_with("resp_"),
        "response id should use resp_ prefix"
    );
    assert!(
        response_id.matches('_').count() >= 2,
        "response id should include timestamp + sequence segments"
    );

    // 1.1 Two rapid requests should still produce unique response IDs.
    let second_resp: Value = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&valid_payload)
        .send()
        .await
        .expect("second /v1/responses request failed")
        .json()
        .await
        .expect("invalid second /v1/responses json");
    let second_response_id = second_resp["id"]
        .as_str()
        .expect("second response id should exist");
    assert_ne!(
        response_id, second_response_id,
        "rapid consecutive responses should not reuse the same id"
    );
    assert!(
        second_response_id.matches('_').count() >= 2,
        "second response id should include timestamp + sequence segments"
    );

    let get_resp: Value = client
        .get(format!(
            "{}/v1/responses/{}",
            harness.base_url(),
            response_id
        ))
        .send()
        .await
        .expect("/v1/responses/{id} request failed")
        .json()
        .await
        .expect("invalid /v1/responses/{id} json");
    assert_eq!(
        get_resp["id"].as_str().unwrap_or_default(),
        response_id,
        "GET /v1/responses/{{id}} should return stored response"
    );
    assert_eq!(
        get_resp["status"].as_str().unwrap_or_default(),
        "completed",
        "stored response should be completed"
    );
    let status_history = get_resp["status_history"]
        .as_array()
        .expect("status_history must be an array");
    let statuses: Vec<&str> = status_history
        .iter()
        .filter_map(|item| item.get("status").and_then(|value| value.as_str()))
        .collect();
    assert_eq!(
        statuses,
        vec!["queued", "in_progress", "completed"],
        "status_history should record queued -> in_progress -> completed transitions"
    );

    let list_resp: Value = client
        .get(format!("{}/v1/responses", harness.base_url()))
        .send()
        .await
        .expect("/v1/responses list request failed")
        .json()
        .await
        .expect("invalid /v1/responses list json");
    assert_eq!(
        list_resp["object"].as_str().unwrap_or_default(),
        "list",
        "GET /v1/responses should return a list object"
    );
    let list_data = list_resp["data"].as_array().expect("data must be an array");
    assert_eq!(
        list_data.first().and_then(|item| item["id"].as_str()),
        Some(second_response_id),
        "list endpoint should return newest response first"
    );
    assert!(
        list_data
            .iter()
            .any(|item| item["id"].as_str() == Some(response_id)),
        "list endpoint should contain the created response id"
    );
    assert!(
        list_data
            .iter()
            .any(|item| item["id"].as_str() == Some(second_response_id)),
        "list endpoint should contain the second created response id"
    );

    let missing_get_resp = client
        .get(format!("{}/v1/responses/resp_missing", harness.base_url()))
        .send()
        .await
        .expect("missing response lookup failed");
    assert_eq!(
        missing_get_resp.status().as_u16(),
        404,
        "unknown response id should return 404"
    );
    let missing_get_json: Value = missing_get_resp
        .json()
        .await
        .expect("invalid missing response error json");
    assert_eq!(
        missing_get_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "not_found",
        "missing response lookup should use not_found code"
    );

    // 2. Missing model → Responses-style error with `code` field.
    let no_model = json!({"input": "hi"});
    let err_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&no_model)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        err_resp.status().as_u16(),
        400,
        "missing model should return 400"
    );
    let err_body: Value = err_resp.json().await.expect("invalid error json");
    assert!(
        err_body["error"]["code"].is_string(),
        "error must have a 'code' field (Responses API style): {err_body}"
    );

    // 3. Missing input → Responses-style error.
    let no_input = json!({"model": "local-echo"});
    let err_resp2 = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&no_input)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        err_resp2.status().as_u16(),
        400,
        "missing input should return 400"
    );
    let err_body2: Value = err_resp2.json().await.expect("invalid error json");
    assert!(
        err_body2["error"]["code"].is_string(),
        "error must have a 'code' field: {err_body2}"
    );

    // 4. stream=true (R3): should return SSE with response.created + response.completed + [DONE].
    let stream_req = json!({
        "model": "local-echo",
        "input": "stream please",
        "stream": true,
    });
    let stream_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&stream_req)
        .send()
        .await
        .expect("stream request failed");
    assert_eq!(
        stream_resp.status().as_u16(),
        200,
        "stream=true should return 200 SSE response"
    );
    assert!(
        stream_resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .contains("text/event-stream"),
        "stream=true must return text/event-stream content type"
    );
    let stream_body = stream_resp
        .text()
        .await
        .expect("failed to read stream body");
    assert!(
        stream_body.contains("response.created"),
        "stream must emit response.created event: {stream_body}"
    );
    assert!(
        stream_body.contains("response.completed"),
        "stream must emit response.completed event: {stream_body}"
    );
    assert!(
        stream_body.contains("response.token_economy"),
        "stream must emit response.token_economy event: {stream_body}"
    );
    assert!(
        stream_body.contains("[DONE]"),
        "stream must end with [DONE]: {stream_body}"
    );
    assert!(
        stream_body.contains("response.output_text.delta"),
        "stream must emit output_text.delta event: {stream_body}"
    );

    // 5. Optional R1 fields should be accepted and still return response object.
    let optional_fields_req = json!({
        "model": "local-echo",
        "input": [{"role":"user","content":"hello optional fields"}],
        "metadata": {"trace":"r1"},
        "reasoning": {"effort":"medium"},
        "max_output_tokens": 32,
        "temperature": 0.2,
        "tools": [{
            "type": "function",
            "function": {
                "name": "lookup_weather",
                "description": "Look up weather for a city",
                "parameters": {"type": "object"}
            }
        }],
        "tool_choice": {
            "type": "function",
            "function": {"name": "lookup_weather"}
        }
    });
    let optional_resp: Value = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&optional_fields_req)
        .send()
        .await
        .expect("optional-field request failed")
        .json()
        .await
        .expect("invalid optional-field response json");
    assert_eq!(
        optional_resp["object"].as_str().unwrap_or_default(),
        "response",
        "optional fields should still produce response object"
    );
    assert_eq!(
        optional_resp["status"].as_str().unwrap_or_default(),
        "completed",
        "optional fields with tool_choice object should still complete"
    );

    // 5.1 tool_choice=required should return an incomplete tool_call response.
    let required_tool_call_req = json!({
        "model": "local-echo",
        "input": [{"role":"user","content":"need weather tool"}],
        "tools": [{
            "type": "function",
            "function": {
                "name": "lookup_weather",
                "parameters": {"type": "object"}
            }
        }],
        "tool_choice": "required"
    });
    let required_tool_call_resp: Value = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&required_tool_call_req)
        .send()
        .await
        .expect("required tool_call request failed")
        .json()
        .await
        .expect("invalid required tool_call response json");
    assert_eq!(
        required_tool_call_resp["status"]
            .as_str()
            .unwrap_or_default(),
        "incomplete",
        "tool_choice=required should produce incomplete response"
    );
    assert_eq!(
        required_tool_call_resp["output"][0]["type"]
            .as_str()
            .unwrap_or_default(),
        "tool_call",
        "incomplete response should expose tool_call output item"
    );
    let pending_response_id = required_tool_call_resp["id"]
        .as_str()
        .expect("pending response id should exist");
    let pending_tool_call_id = required_tool_call_resp["output"][0]["id"]
        .as_str()
        .expect("pending tool_call id should exist");

    // 5.2 previous_response_id + matching tool result should complete the loop.
    let tool_result_continue_req = json!({
        "model": "local-echo",
        "previous_response_id": pending_response_id,
        "input": [{
            "role": "tool",
            "tool_call_id": pending_tool_call_id,
            "content": "weather is sunny"
        }]
    });
    let tool_result_continue_resp: Value = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&tool_result_continue_req)
        .send()
        .await
        .expect("tool result continuation request failed")
        .json()
        .await
        .expect("invalid tool result continuation response json");
    assert_eq!(
        tool_result_continue_resp["status"]
            .as_str()
            .unwrap_or_default(),
        "completed",
        "tool result continuation should complete response"
    );
    assert_eq!(
        tool_result_continue_resp["output"][0]["type"]
            .as_str()
            .unwrap_or_default(),
        "tool_result",
        "completion should include tool_result output item"
    );
    assert_eq!(
        tool_result_continue_resp["output"][0]["tool_call_id"]
            .as_str()
            .unwrap_or_default(),
        pending_tool_call_id,
        "tool_result should reference the pending tool_call"
    );

    // 5.3 Mismatched tool_call_id should return tool_error.
    let mismatched_tool_result_req = json!({
        "model": "local-echo",
        "previous_response_id": pending_response_id,
        "input": [{
            "role": "tool",
            "tool_call_id": "call_other",
            "content": "weather is rainy"
        }]
    });
    let mismatched_tool_result_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&mismatched_tool_result_req)
        .send()
        .await
        .expect("mismatched tool result request failed");
    assert_eq!(
        mismatched_tool_result_resp.status().as_u16(),
        400,
        "mismatched tool result should return 400"
    );
    let mismatched_tool_result_json: Value = mismatched_tool_result_resp
        .json()
        .await
        .expect("invalid mismatched tool result error json");
    assert_eq!(
        mismatched_tool_result_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "tool_error",
        "mismatched tool result should return tool_error"
    );

    // 5.4 Continuing from a completed response should return tool_error.
    let no_pending_tool_call_req = json!({
        "model": "local-echo",
        "previous_response_id": response_id,
        "input": [{
            "role": "tool",
            "tool_call_id": "call_none",
            "content": "anything"
        }]
    });
    let no_pending_tool_call_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&no_pending_tool_call_req)
        .send()
        .await
        .expect("no-pending-tool-call request failed");
    assert_eq!(
        no_pending_tool_call_resp.status().as_u16(),
        400,
        "continuation without pending tool_call should return 400"
    );
    let no_pending_tool_call_json: Value = no_pending_tool_call_resp
        .json()
        .await
        .expect("invalid no-pending-tool-call error json");
    assert_eq!(
        no_pending_tool_call_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "tool_error",
        "continuation without pending tool_call should return tool_error"
    );

    // 6. Invalid input type (object) should be rejected.
    let invalid_input_type = json!({
        "model": "local-echo",
        "input": {"text": "bad-shape"}
    });
    let invalid_type_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&invalid_input_type)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        invalid_type_resp.status().as_u16(),
        400,
        "object input should return 400"
    );
    let invalid_type_body: Value = invalid_type_resp.json().await.expect("invalid error json");
    assert_eq!(
        invalid_type_body["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "invalid input type should return invalid_input"
    );

    // 7. Empty user input should be rejected.
    let empty_input = json!({
        "model": "local-echo",
        "input": "   "
    });
    let empty_input_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&empty_input)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        empty_input_resp.status().as_u16(),
        400,
        "empty input should return 400"
    );
    let empty_input_body: Value = empty_input_resp.json().await.expect("invalid error json");
    assert_eq!(
        empty_input_body["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "empty input should return invalid_input"
    );

    // 8. Assistant-only input should be rejected (must include user message).
    let assistant_only_input = json!({
        "model": "local-echo",
        "input": [{"role": "assistant", "content": "I am assistant only"}]
    });
    let assistant_only_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&assistant_only_input)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        assistant_only_resp.status().as_u16(),
        400,
        "assistant-only input should return 400"
    );
    let assistant_only_body: Value = assistant_only_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        assistant_only_body["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "assistant-only input should return invalid_input"
    );

    // 9. Empty body should return Responses-style structured error.
    let empty_body_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        empty_body_resp.status().as_u16(),
        400,
        "empty body should return 400"
    );
    let empty_body_json: Value = empty_body_resp.json().await.expect("invalid error json");
    assert_eq!(
        empty_body_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "missing_required_field",
        "empty body should return missing_required_field"
    );

    // 10. Malformed JSON should return Responses-style structured error.
    let malformed_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .header(CONTENT_TYPE, "application/json")
        .body("{not-json")
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        malformed_resp.status().as_u16(),
        400,
        "malformed json should return 400"
    );
    let malformed_json: Value = malformed_resp.json().await.expect("invalid error json");
    assert_eq!(
        malformed_json["error"]["code"].as_str().unwrap_or_default(),
        "invalid_request_error",
        "malformed json should return invalid_request_error"
    );
    assert_eq!(
        malformed_json["error"]["type"].as_str().unwrap_or_default(),
        "invalid_request_error",
        "malformed json should include invalid_request_error type"
    );

    // 11. Non-object JSON body should be rejected.
    let non_object_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .header(CONTENT_TYPE, "application/json")
        .body("\"just a string\"")
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        non_object_resp.status().as_u16(),
        400,
        "non-object json body should return 400"
    );
    let non_object_json: Value = non_object_resp.json().await.expect("invalid error json");
    assert_eq!(
        non_object_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_request_error",
        "non-object body should return invalid_request_error"
    );

    // 12. max_output_tokens must be > 0.
    let zero_max_tokens = json!({
        "model": "local-echo",
        "input": "hello",
        "max_output_tokens": 0
    });
    let zero_max_tokens_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&zero_max_tokens)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        zero_max_tokens_resp.status().as_u16(),
        400,
        "max_output_tokens=0 should return 400"
    );
    let zero_max_tokens_json: Value = zero_max_tokens_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        zero_max_tokens_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "max_output_tokens=0 should return invalid_input"
    );

    // 13. model cannot be an empty string.
    let empty_model = json!({
        "model": "",
        "input": "hello"
    });
    let empty_model_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&empty_model)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        empty_model_resp.status().as_u16(),
        400,
        "empty model should return 400"
    );
    let empty_model_json: Value = empty_model_resp.json().await.expect("invalid error json");
    assert_eq!(
        empty_model_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "empty model should return invalid_input"
    );

    // 14. model cannot be whitespace-only.
    let whitespace_model = json!({
        "model": "   ",
        "input": "hello"
    });
    let whitespace_model_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&whitespace_model)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        whitespace_model_resp.status().as_u16(),
        400,
        "whitespace model should return 400"
    );
    let whitespace_model_json: Value = whitespace_model_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        whitespace_model_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "whitespace model should return invalid_input"
    );

    // 15. model type must be string.
    let non_string_model = json!({
        "model": 123,
        "input": "hello"
    });
    let non_string_model_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&non_string_model)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        non_string_model_resp.status().as_u16(),
        400,
        "non-string model should return 400"
    );
    let non_string_model_json: Value = non_string_model_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        non_string_model_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "non-string model should return invalid_input"
    );

    // 16. max_output_tokens must be integer.
    let fractional_max_tokens = json!({
        "model": "local-echo",
        "input": "hello",
        "max_output_tokens": 1.5
    });
    let fractional_max_tokens_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&fractional_max_tokens)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        fractional_max_tokens_resp.status().as_u16(),
        400,
        "fractional max_output_tokens should return 400"
    );
    let fractional_max_tokens_json: Value = fractional_max_tokens_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        fractional_max_tokens_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "fractional max_output_tokens should return invalid_input"
    );

    // 17. metadata must be object.
    let invalid_metadata = json!({
        "model": "local-echo",
        "input": "hello",
        "metadata": "bad"
    });
    let invalid_metadata_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&invalid_metadata)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        invalid_metadata_resp.status().as_u16(),
        400,
        "non-object metadata should return 400"
    );
    let invalid_metadata_json: Value = invalid_metadata_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        invalid_metadata_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "non-object metadata should return invalid_input"
    );

    // 18. reasoning must be object.
    let invalid_reasoning = json!({
        "model": "local-echo",
        "input": "hello",
        "reasoning": ["bad"]
    });
    let invalid_reasoning_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&invalid_reasoning)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        invalid_reasoning_resp.status().as_u16(),
        400,
        "non-object reasoning should return 400"
    );
    let invalid_reasoning_json: Value = invalid_reasoning_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        invalid_reasoning_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "non-object reasoning should return invalid_input"
    );

    // 19. tools must be an array.
    let invalid_tools = json!({
        "model": "local-echo",
        "input": "hello",
        "tools": {"type": "function"}
    });
    let invalid_tools_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&invalid_tools)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        invalid_tools_resp.status().as_u16(),
        400,
        "non-array tools should return 400"
    );
    let invalid_tools_json: Value = invalid_tools_resp.json().await.expect("invalid error json");
    assert_eq!(
        invalid_tools_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "non-array tools should return invalid_input"
    );

    // 20. tool_choice must be a string or object.
    let invalid_tool_choice = json!({
        "model": "local-echo",
        "input": "hello",
        "tool_choice": 7
    });
    let invalid_tool_choice_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&invalid_tool_choice)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        invalid_tool_choice_resp.status().as_u16(),
        400,
        "invalid tool_choice type should return 400"
    );
    let invalid_tool_choice_json: Value = invalid_tool_choice_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        invalid_tool_choice_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "invalid tool_choice type should return invalid_input"
    );

    // 21. temperature must be numeric.
    let invalid_temperature_type = json!({
        "model": "local-echo",
        "input": "hello",
        "temperature": "warm"
    });
    let invalid_temperature_type_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&invalid_temperature_type)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        invalid_temperature_type_resp.status().as_u16(),
        400,
        "non-numeric temperature should return 400"
    );
    let invalid_temperature_type_json: Value = invalid_temperature_type_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        invalid_temperature_type_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "non-numeric temperature should return invalid_input"
    );

    // 22. temperature must stay within the supported range.
    let invalid_temperature_range = json!({
        "model": "local-echo",
        "input": "hello",
        "temperature": 2.5
    });
    let invalid_temperature_range_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&invalid_temperature_range)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        invalid_temperature_range_resp.status().as_u16(),
        400,
        "out-of-range temperature should return 400"
    );
    let invalid_temperature_range_json: Value = invalid_temperature_range_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        invalid_temperature_range_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "out-of-range temperature should return invalid_input"
    );

    // 23. tools entries must be objects.
    let invalid_tool_entry = json!({
        "model": "local-echo",
        "input": "hello",
        "tools": ["bad-tool"]
    });
    let invalid_tool_entry_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&invalid_tool_entry)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        invalid_tool_entry_resp.status().as_u16(),
        400,
        "non-object tool entry should return 400"
    );
    let invalid_tool_entry_json: Value = invalid_tool_entry_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        invalid_tool_entry_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "non-object tool entry should return invalid_input"
    );

    // 24. tool_choice string values must be supported.
    let invalid_tool_choice_string = json!({
        "model": "local-echo",
        "input": "hello",
        "tool_choice": "sometimes"
    });
    let invalid_tool_choice_string_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&invalid_tool_choice_string)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        invalid_tool_choice_string_resp.status().as_u16(),
        400,
        "unsupported tool_choice string should return 400"
    );
    let invalid_tool_choice_string_json: Value = invalid_tool_choice_string_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        invalid_tool_choice_string_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "unsupported tool_choice string should return invalid_input"
    );

    // 25. tools entries must use type=function and include function.name.
    let invalid_tool_shape = json!({
        "model": "local-echo",
        "input": "hello",
        "tools": [{"type": "code_interpreter"}]
    });
    let invalid_tool_shape_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&invalid_tool_shape)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        invalid_tool_shape_resp.status().as_u16(),
        400,
        "invalid tool shape should return 400"
    );
    let invalid_tool_shape_json: Value = invalid_tool_shape_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        invalid_tool_shape_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "invalid tool shape should return invalid_input"
    );

    // 26. tool_choice object must use type=function and include function.name.
    let invalid_tool_choice_object = json!({
        "model": "local-echo",
        "input": "hello",
        "tool_choice": {"type": "function", "function": {"name": "   "}}
    });
    let invalid_tool_choice_object_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&invalid_tool_choice_object)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        invalid_tool_choice_object_resp.status().as_u16(),
        400,
        "invalid tool_choice object should return 400"
    );
    let invalid_tool_choice_object_json: Value = invalid_tool_choice_object_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        invalid_tool_choice_object_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "invalid tool_choice object should return invalid_input"
    );

    // 27. tool function.description must be a string.
    let invalid_tool_description = json!({
        "model": "local-echo",
        "input": "hello",
        "tools": [{
            "type": "function",
            "function": {
                "name": "lookup_weather",
                "description": 99
            }
        }]
    });
    let invalid_tool_description_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&invalid_tool_description)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        invalid_tool_description_resp.status().as_u16(),
        400,
        "non-string tool description should return 400"
    );
    let invalid_tool_description_json: Value = invalid_tool_description_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        invalid_tool_description_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "non-string tool description should return invalid_input"
    );

    // 28. tool function.parameters must be an object.
    let invalid_tool_parameters = json!({
        "model": "local-echo",
        "input": "hello",
        "tools": [{
            "type": "function",
            "function": {
                "name": "lookup_weather",
                "parameters": "bad"
            }
        }]
    });
    let invalid_tool_parameters_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&invalid_tool_parameters)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        invalid_tool_parameters_resp.status().as_u16(),
        400,
        "non-object tool parameters should return 400"
    );
    let invalid_tool_parameters_json: Value = invalid_tool_parameters_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        invalid_tool_parameters_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "non-object tool parameters should return invalid_input"
    );

    // 29. tool_choice=required must not be used without tools.
    let required_without_tools = json!({
        "model": "local-echo",
        "input": "hello",
        "tool_choice": "required"
    });
    let required_without_tools_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&required_without_tools)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        required_without_tools_resp.status().as_u16(),
        400,
        "tool_choice=required without tools should return 400"
    );
    let required_without_tools_json: Value = required_without_tools_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        required_without_tools_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "tool_choice=required without tools should return invalid_input"
    );

    // 30. tool_choice object must reference a declared tool.
    let tool_choice_unknown_tool = json!({
        "model": "local-echo",
        "input": "hello",
        "tools": [{
            "type": "function",
            "function": {
                "name": "lookup_weather",
                "parameters": {"type": "object"}
            }
        }],
        "tool_choice": {
            "type": "function",
            "function": {"name": "lookup_stock"}
        }
    });
    let tool_choice_unknown_tool_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&tool_choice_unknown_tool)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        tool_choice_unknown_tool_resp.status().as_u16(),
        400,
        "tool_choice object with undeclared tool should return 400"
    );
    let tool_choice_unknown_tool_json: Value = tool_choice_unknown_tool_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        tool_choice_unknown_tool_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "tool_choice object with undeclared tool should return invalid_input"
    );

    // 31. tool_choice object requires tools to be provided.
    let tool_choice_without_tools = json!({
        "model": "local-echo",
        "input": "hello",
        "tool_choice": {
            "type": "function",
            "function": {"name": "lookup_weather"}
        }
    });
    let tool_choice_without_tools_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&tool_choice_without_tools)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        tool_choice_without_tools_resp.status().as_u16(),
        400,
        "tool_choice object without tools should return 400"
    );
    let tool_choice_without_tools_json: Value = tool_choice_without_tools_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        tool_choice_without_tools_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "tool_choice object without tools should return invalid_input"
    );

    // 32. tool function.parameters.type must be object.
    let invalid_tool_parameters_type = json!({
        "model": "local-echo",
        "input": "hello",
        "tools": [{
            "type": "function",
            "function": {
                "name": "lookup_weather",
                "parameters": {"type": "string"}
            }
        }]
    });
    let invalid_tool_parameters_type_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&invalid_tool_parameters_type)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        invalid_tool_parameters_type_resp.status().as_u16(),
        400,
        "non-object parameters.type should return 400"
    );
    let invalid_tool_parameters_type_json: Value = invalid_tool_parameters_type_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        invalid_tool_parameters_type_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "non-object parameters.type should return invalid_input"
    );

    // 33. tool function.parameters.properties must be an object.
    let invalid_tool_parameters_properties = json!({
        "model": "local-echo",
        "input": "hello",
        "tools": [{
            "type": "function",
            "function": {
                "name": "lookup_weather",
                "parameters": {
                    "type": "object",
                    "properties": ["bad"]
                }
            }
        }]
    });
    let invalid_tool_parameters_properties_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&invalid_tool_parameters_properties)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        invalid_tool_parameters_properties_resp.status().as_u16(),
        400,
        "non-object parameters.properties should return 400"
    );
    let invalid_tool_parameters_properties_json: Value = invalid_tool_parameters_properties_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        invalid_tool_parameters_properties_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "non-object parameters.properties should return invalid_input"
    );

    // 34. tool function.parameters.required must be an array of strings.
    let invalid_tool_parameters_required = json!({
        "model": "local-echo",
        "input": "hello",
        "tools": [{
            "type": "function",
            "function": {
                "name": "lookup_weather",
                "parameters": {
                    "type": "object",
                    "required": [1, 2]
                }
            }
        }]
    });
    let invalid_tool_parameters_required_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&invalid_tool_parameters_required)
        .send()
        .await
        .expect("request failed");
    assert_eq!(
        invalid_tool_parameters_required_resp.status().as_u16(),
        400,
        "non-string parameters.required entries should return 400"
    );
    let invalid_tool_parameters_required_json: Value = invalid_tool_parameters_required_resp
        .json()
        .await
        .expect("invalid error json");
    assert_eq!(
        invalid_tool_parameters_required_json["error"]["code"]
            .as_str()
            .unwrap_or_default(),
        "invalid_input",
        "non-string parameters.required entries should return invalid_input"
    );
}

/// Phase R4: golden field matrix — field completeness and event ordering assertions
/// separated from validation-edge-case tests for clarity and faster triage.
#[tokio::test(flavor = "current_thread")]
async fn responses_api_r4_complete_field_matrix() {
    let dir = tempdir().expect("failed to create tempdir");
    let config_path = dir.path().join("config.toml");
    write_http_test_config(&config_path);

    let harness = HttpHarness::spawn(&config_path, ephemeral_bind_addr());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .expect("failed to build reqwest client");

    wait_healthy(&client, &harness.base_url(), Duration::from_secs(10)).await;

    // R4-1: Text response field completeness — all 9 top-level required fields present.
    let resp: Value = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&json!({"model": "local-echo", "input": "golden field test"}))
        .send()
        .await
        .expect("r4 field matrix request failed")
        .json()
        .await
        .expect("r4 field matrix invalid json");

    let required_fields = [
        "id",
        "object",
        "created_at",
        "model",
        "status",
        "output",
        "usage",
        "error",
        "incomplete_details",
    ];
    for field in &required_fields {
        assert!(
            resp.get(field).is_some(),
            "response missing required field '{field}': {resp}"
        );
    }
    assert_eq!(resp["object"].as_str().unwrap_or_default(), "response");
    assert_eq!(resp["status"].as_str().unwrap_or_default(), "completed");
    assert!(resp["id"].as_str().unwrap_or_default().starts_with("resp_"));
    assert!(resp["created_at"].is_number(), "created_at must be numeric");
    assert!(resp["usage"].is_object(), "usage must be object");
    assert!(
        resp["usage"]["input_tokens"].as_u64().unwrap_or(0) > 0,
        "usage.input_tokens should reflect estimated request tokens"
    );
    assert!(
        resp["usage"]["output_tokens"].as_u64().unwrap_or(0) > 0,
        "usage.output_tokens should reflect estimated response tokens"
    );
    assert!(
        resp["usage"]["total_tokens"].as_u64().unwrap_or(0)
            >= resp["usage"]["input_tokens"].as_u64().unwrap_or(0),
        "usage.total_tokens must be at least input_tokens"
    );
    assert!(resp["token_economy"].is_object(), "token_economy must be present");
    assert!(
        resp["token_economy"]["compression_ratio"].is_number(),
        "token_economy.compression_ratio must be numeric"
    );
    assert!(resp["error"].is_null(), "error must be null on success");
    assert!(
        resp["incomplete_details"].is_null(),
        "incomplete_details must be null on success"
    );

    let output = resp["output"].as_array().expect("output must be array");
    assert!(!output.is_empty(), "output must be non-empty");
    assert_eq!(output[0]["type"].as_str().unwrap_or_default(), "message");
    assert_eq!(output[0]["role"].as_str().unwrap_or_default(), "assistant");
    assert_eq!(
        output[0]["status"].as_str().unwrap_or_default(),
        "completed"
    );
    let content = output[0]["content"]
        .as_array()
        .expect("content must be array");
    assert!(!content.is_empty(), "content must be non-empty");
    assert_eq!(
        content[0]["type"].as_str().unwrap_or_default(),
        "output_text"
    );
    assert!(
        content[0]["text"].is_string(),
        "content text must be string"
    );

    // R4-2: Error field completeness — error.code, error.type, error.message all present.
    let err_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&json!({"model": "local-echo"}))
        .send()
        .await
        .expect("r4 error field matrix request failed");
    assert_eq!(
        err_resp.status().as_u16(),
        400,
        "missing input should be 400"
    );
    let err_body: Value = err_resp.json().await.expect("invalid error json");
    assert!(
        err_body.get("error").is_some(),
        "error response must have 'error' key"
    );
    let error_obj = &err_body["error"];
    assert!(error_obj["code"].is_string(), "error.code must be string");
    assert!(error_obj["type"].is_string(), "error.type must be string");
    assert!(
        error_obj["message"].is_string(),
        "error.message must be string"
    );
    assert!(
        !error_obj["code"].as_str().unwrap_or_default().is_empty(),
        "error.code must not be empty"
    );

    // R4-3: Stream event ordering — response.created must precede response.completed.
    let stream_resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&json!({"model": "local-echo", "input": "stream ordering test", "stream": true}))
        .send()
        .await
        .expect("r4 stream request failed");
    assert_eq!(stream_resp.status().as_u16(), 200, "stream must return 200");
    let stream_body = stream_resp
        .text()
        .await
        .expect("failed to read stream body");

    // Verify event ordering: created → [delta*] → token_economy → completed → [DONE]
    let created_pos = stream_body.find("response.created").unwrap_or(usize::MAX);
    let delta_pos = stream_body
        .find("response.output_text.delta")
        .unwrap_or(usize::MAX);
    let telemetry_pos = stream_body
        .find("response.token_economy")
        .unwrap_or(usize::MAX);
    let completed_pos = stream_body.find("response.completed").unwrap_or(usize::MAX);
    let done_pos = stream_body.find("[DONE]").unwrap_or(usize::MAX);

    assert!(
        created_pos < usize::MAX,
        "stream must contain response.created"
    );
    assert!(
        completed_pos < usize::MAX,
        "stream must contain response.completed"
    );
    assert!(done_pos < usize::MAX, "stream must end with [DONE]");
    assert!(
        created_pos < completed_pos,
        "response.created must precede response.completed: created@{created_pos} completed@{completed_pos}"
    );
    assert!(
        completed_pos < done_pos,
        "response.completed must precede [DONE]: completed@{completed_pos} done@{done_pos}"
    );
    if delta_pos < usize::MAX {
        assert!(
            created_pos < delta_pos,
            "response.created must precede response.output_text.delta"
        );
        assert!(
            delta_pos < telemetry_pos,
            "response.output_text.delta must precede response.token_economy"
        );
    }
    if telemetry_pos < usize::MAX {
        assert!(
            telemetry_pos < completed_pos,
            "response.token_economy must precede response.completed"
        );
    }

    // R4-4: Verify stream response.created contains in_progress status.
    let created_event_start = stream_body.find("event: response.created").unwrap_or(0);
    let created_event_slice = &stream_body[created_event_start..];
    assert!(
        created_event_slice.contains("in_progress"),
        "response.created event must carry in_progress status"
    );

    // R4-5: Verify stream response.completed contains completed status.
    let completed_event_start = stream_body.find("event: response.completed").unwrap_or(0);
    let completed_event_slice = &stream_body[completed_event_start..];
    assert!(
        completed_event_slice.contains("completed"),
        "response.completed event must carry completed status"
    );
}

/// Phase R4.1: route contract assertions for root capabilities and unsupported methods.
#[tokio::test(flavor = "current_thread")]
async fn responses_api_r4_route_contracts() {
    let dir = tempdir().expect("failed to create tempdir");
    let config_path = dir.path().join("config.toml");
    write_http_test_config(&config_path);

    let harness = HttpHarness::spawn(&config_path, ephemeral_bind_addr());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .expect("failed to build reqwest client");

    wait_healthy(&client, &harness.base_url(), Duration::from_secs(10)).await;

    let root: Value = client
        .get(format!("{}/", harness.base_url()))
        .send()
        .await
        .expect("root capabilities request failed")
        .json()
        .await
        .expect("invalid root capabilities json");

    assert_eq!(
        root["service"].as_str().unwrap_or_default(),
        "go-on",
        "root capabilities should expose service name"
    );
    assert_eq!(
        root["protocol"].as_str().unwrap_or_default(),
        "acp-http",
        "root capabilities should expose protocol name"
    );
    assert_eq!(
        root["health"].as_str().unwrap_or_default(),
        "/health",
        "root capabilities should expose health path"
    );
    let responses_endpoints = root["endpoints"]["responses"]
        .as_array()
        .expect("root capabilities responses endpoints must be array");
    assert_eq!(
        responses_endpoints
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>(),
        vec!["/v1/responses", "/v1/responses/{id}"],
        "root capabilities should advertise responses endpoints"
    );

    let created: Value = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&json!({"model": "local-echo", "input": "route contract"}))
        .send()
        .await
        .expect("create response for delete contract failed")
        .json()
        .await
        .expect("invalid create response json");
    let response_id = created["id"]
        .as_str()
        .expect("created response id must be present");

    let delete_resp = client
        .delete(format!(
            "{}/v1/responses/{}",
            harness.base_url(),
            response_id
        ))
        .send()
        .await
        .expect("delete response contract request failed");
    assert_eq!(
        delete_resp.status().as_u16(),
        405,
        "DELETE /v1/responses/{{id}} should return 405"
    );
    let delete_json: Value = delete_resp.json().await.expect("invalid delete 405 json");
    assert_eq!(
        delete_json["error"].as_str().unwrap_or_default(),
        "method not allowed",
        "DELETE /v1/responses/{{id}} should use generic method not allowed payload"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn responses_api_stream_degrades_setup_unavailable() {
    let dir = tempdir().expect("failed to create tempdir");
    let config_path = dir.path().join("config.toml");
    write_http_unavailable_provider_config(&config_path);

    let harness = HttpHarness::spawn(&config_path, ephemeral_bind_addr());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .expect("failed to build reqwest client");

    wait_healthy(&client, &harness.base_url(), Duration::from_secs(10)).await;

    let stream_body = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&json!({
            "model": "go-on",
            "input": "degrade stream please",
            "stream": true
        }))
        .send()
        .await
        .expect("responses stream degrade request failed")
        .text()
        .await
        .expect("failed to read stream degrade body");

    assert!(
        stream_body.contains("response.created"),
        "degraded stream must emit response.created: {stream_body}"
    );
    assert!(
        stream_body.contains("response.output_text.delta"),
        "degraded stream must emit response.output_text.delta: {stream_body}"
    );
    assert!(
        stream_body.contains("response.completed"),
        "degraded stream must emit response.completed instead of failed: {stream_body}"
    );
    assert!(
        stream_body.contains("response.token_economy"),
        "degraded stream must emit response.token_economy: {stream_body}"
    );
    assert!(
        !stream_body.contains("response.failed"),
        "degraded stream should not emit response.failed for setup/upstream unavailable: {stream_body}"
    );
    assert!(
        stream_body.contains("go-on is running, but upstream model service is unavailable."),
        "degraded stream must include the degraded fallback message: {stream_body}"
    );
    assert!(
        stream_body.contains("[DONE]"),
        "degraded stream must terminate with [DONE]: {stream_body}"
    );

    let completed_event = stream_body
        .lines()
        .find_map(|line| {
            let data = line.strip_prefix("data: ")?;
            if data.trim() == "[DONE]" {
                return None;
            }
            let payload: Value = serde_json::from_str(data).ok()?;
            if payload["type"].as_str() == Some("response.completed") {
                Some(payload)
            } else {
                None
            }
        })
        .expect("degraded stream should contain response.completed payload");

    let response_id = completed_event["response"]["id"]
        .as_str()
        .expect("completed event should carry response id")
        .to_string();

    let stored_resp: Value = client
        .get(format!(
            "{}/v1/responses/{}",
            harness.base_url(),
            response_id
        ))
        .send()
        .await
        .expect("get degraded stream response failed")
        .json()
        .await
        .expect("invalid degraded stream stored response json");

    assert_eq!(
        stored_resp["id"].as_str().unwrap_or_default(),
        response_id,
        "stored response id should match stream completed id"
    );
    assert_eq!(
        stored_resp["status"].as_str().unwrap_or_default(),
        "completed",
        "degraded stream response should be stored as completed"
    );
    assert!(
        stored_resp["error"].is_null(),
        "degraded stream stored response should not carry error"
    );
    assert!(
        stored_resp
            .to_string()
            .contains("go-on is running, but upstream model service is unavailable."),
        "stored response should include degraded fallback text"
    );

    let status_history = stored_resp["status_history"]
        .as_array()
        .expect("status_history must be present for degraded stream response");
    let statuses: Vec<&str> = status_history
        .iter()
        .filter_map(|item| item.get("status").and_then(|value| value.as_str()))
        .collect();
    assert_eq!(
        statuses,
        vec!["queued", "in_progress", "completed"],
        "degraded stream response should preserve queued->in_progress->completed history"
    );

    let list_resp: Value = client
        .get(format!("{}/v1/responses", harness.base_url()))
        .send()
        .await
        .expect("list responses after degraded stream failed")
        .json()
        .await
        .expect("invalid responses list json");
    assert!(
        list_resp["data"]
            .as_array()
            .expect("list data must be array")
            .iter()
            .any(|item| item["id"].as_str() == Some(response_id.as_str())),
        "responses list should include degraded stream response id"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn responses_api_non_stream_degrades_setup_unavailable() {
    let dir = tempdir().expect("failed to create tempdir");
    let config_path = dir.path().join("config.toml");
    write_http_unavailable_provider_config(&config_path);

    let harness = HttpHarness::spawn(&config_path, ephemeral_bind_addr());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .expect("failed to build reqwest client");

    wait_healthy(&client, &harness.base_url(), Duration::from_secs(10)).await;

    let resp = client
        .post(format!("{}/v1/responses", harness.base_url()))
        .json(&json!({
            "model": "go-on",
            "input": "degrade non-stream please"
        }))
        .send()
        .await
        .expect("responses non-stream degrade request failed");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "non-stream setup/unavailable should degrade with 200 response"
    );
    let body: Value = resp
        .json()
        .await
        .expect("invalid non-stream degrade response json");

    let response_id = body["id"]
        .as_str()
        .expect("non-stream degraded response id should exist")
        .to_string();

    assert_eq!(
        body["status"].as_str().unwrap_or_default(),
        "completed",
        "non-stream setup/unavailable should be completed"
    );
    assert!(
        body["error"].is_null(),
        "non-stream setup/unavailable should not include error object"
    );
    assert!(
        body.to_string()
            .contains("go-on is running, but upstream model service is unavailable."),
        "non-stream degraded response should include fallback text"
    );

    let stored_resp: Value = client
        .get(format!(
            "{}/v1/responses/{}",
            harness.base_url(),
            response_id
        ))
        .send()
        .await
        .expect("get non-stream degraded response failed")
        .json()
        .await
        .expect("invalid non-stream degraded stored response json");
    assert_eq!(
        stored_resp["status"].as_str().unwrap_or_default(),
        "completed",
        "stored non-stream degraded response should remain completed"
    );
    let status_history = stored_resp["status_history"]
        .as_array()
        .expect("status_history must exist for non-stream degraded response");
    let statuses: Vec<&str> = status_history
        .iter()
        .filter_map(|item| item.get("status").and_then(|value| value.as_str()))
        .collect();
    assert_eq!(
        statuses,
        vec!["queued", "in_progress", "completed"],
        "non-stream degraded response should preserve queued->in_progress->completed history"
    );

    let list_resp: Value = client
        .get(format!("{}/v1/responses", harness.base_url()))
        .send()
        .await
        .expect("list responses after non-stream degraded request failed")
        .json()
        .await
        .expect("invalid responses list json");
    assert!(
        list_resp["data"]
            .as_array()
            .expect("list data must be array")
            .iter()
            .any(|item| item["id"].as_str() == Some(response_id.as_str())),
        "responses list should include non-stream degraded response id"
    );
}
