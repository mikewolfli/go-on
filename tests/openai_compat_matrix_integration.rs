use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

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

async fn wait_healthy(client: &reqwest::Client, base_url: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(resp) = client.get(format!("{base_url}/health")).send().await {
            if resp.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    panic!("timed out waiting for /health");
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
    assert!(
        models_json["data"]
            .as_array()
            .map(|arr| !arr.is_empty())
            .unwrap_or(false)
    );

    let model_alias_json: Value = client
        .get(format!("{}/v1/model", harness.base_url()))
        .send()
        .await
        .expect("/v1/model request failed")
        .json()
        .await
        .expect("invalid /v1/model json");
    assert!(
        model_alias_json["data"]
            .as_array()
            .map(|arr| !arr.is_empty())
            .unwrap_or(false)
    );

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
