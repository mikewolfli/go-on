use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tempfile::tempdir;

use super::{JsonRpcRequest, McpServer};
use crate::agent::AgentRegistry;
use crate::tool::{Tool, ToolInput, ToolOutput, ToolRegistry};

fn build_server() -> McpServer {
    let agent_registry = Arc::new(AgentRegistry::new());
    let tool_registry = Arc::new(ToolRegistry::new());
    McpServer::new(
        agent_registry,
        tool_registry,
        "go-on".to_string(),
        "1.1.0".to_string(),
    )
}

#[derive(Clone)]
struct SlowTool;

impl Tool for SlowTool {
    fn name(&self) -> &'static str {
        "slow_test_tool"
    }

    fn run(&self, _input: &ToolInput) -> anyhow::Result<ToolOutput> {
        std::thread::sleep(Duration::from_millis(50));
        Ok(ToolOutput {
            success: true,
            result: Some(json!({"ok": true})),
            error: None,
            verification: None,
            audit_log: None,
            pua_report: None,
        })
    }
}

fn build_server_with_tool<T: Tool + 'static>(tool: T) -> McpServer {
    let agent_registry = Arc::new(AgentRegistry::new());
    let mut tool_registry = ToolRegistry::new_empty();
    tool_registry.register(tool);
    McpServer::new(
        agent_registry,
        Arc::new(tool_registry),
        "go-on".to_string(),
        "1.1.0".to_string(),
    )
}

#[tokio::test]
async fn test_mcp_initialize() {
    let server = build_server();

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "initialize".to_string(),
        params: None,
        id: Some(json!(1)),
    };

    let response = server.handle_request(request).await;
    assert!(response.is_ok(), "Initialize should succeed");
    let resp = response.unwrap();
    assert!(resp.result.is_some(), "Result should be present");
    assert!(resp.error.is_none(), "No error should be present");
}

#[tokio::test]
async fn test_mcp_list_tools() {
    let server = build_server();

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/list".to_string(),
        params: None,
        id: Some(json!(2)),
    };

    let response = server.handle_request(request).await;
    assert!(response.is_ok(), "List tools should succeed");
    let resp = response.unwrap();
    assert!(resp.result.is_some(), "Result should contain tools");
    assert!(resp.error.is_none(), "No error should be present");
    let result = resp.result.expect("tools result should exist");
    let tools = result["tools"].as_array().expect("tools should be array");
    assert!(tools.iter().any(|tool| tool["name"] == "read_file"));
}

#[tokio::test]
async fn test_mcp_error_handling() {
    let server = build_server();

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "unknown_method".to_string(),
        params: None,
        id: Some(json!(3)),
    };

    let response = server.handle_request(request).await;
    assert!(response.is_ok(), "Request should not panic");
    let resp = response.unwrap();
    assert!(
        resp.error.is_some(),
        "Error should be present for unknown method"
    );
}

#[tokio::test]
async fn test_mcp_tool_call_rejects_missing_required_arguments() {
    let server = build_server();

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "read_file",
            "arguments": {}
        })),
        id: Some(json!(4)),
    };

    let response = server
        .handle_request(request)
        .await
        .expect("request should return response envelope");
    assert!(response.error.is_some());
    let message = response
        .error
        .expect("error object should be present")
        .message;
    assert!(message.contains("requires arguments.path"));
}

#[tokio::test]
async fn test_mcp_resource_reads_return_registry_contents() {
    let server = build_server();

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "resources/read".to_string(),
        params: Some(json!({"uri": "go-on://tools"})),
        id: Some(json!(5)),
    };

    let response = server
        .handle_request(request)
        .await
        .expect("read should succeed");
    let result = response.result.expect("resource result should be present");
    let text = result["contents"][0]["text"]
        .as_str()
        .expect("resource text should be string");
    assert!(text.contains("read_file"));
}

#[tokio::test]
async fn test_mcp_tool_call_executes_registered_tool() {
    let temp = tempdir().expect("tempdir should be created");
    let file_path = temp.path().join("sample.txt");
    std::fs::write(&file_path, "hello from mcp").expect("test file should be written");
    let server = build_server();

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "read_file",
            "arguments": {"path": file_path.to_string_lossy().to_string()}
        })),
        id: Some(json!(6)),
    };

    let response = server
        .handle_request(request)
        .await
        .expect("tool call should succeed");
    let result = response.result.expect("tool call result should be present");
    assert_eq!(result["structuredContent"]["success"], true);
    let content = result["structuredContent"]["result"]["content"]
        .as_str()
        .expect("read_file content should be string");
    assert_eq!(content, "hello from mcp");
}

#[tokio::test]
async fn test_mcp_cancelled_request_returns_cancel_error() {
    let server = build_server();

    let cancel = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "notifications/cancelled".to_string(),
        params: Some(json!({"requestId": 42})),
        id: None,
    };
    let cancel_response = server
        .handle_request(cancel)
        .await
        .expect("cancel notification should not fail");
    assert!(cancel_response.id.is_none());
    assert!(cancel_response.result.is_none());

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "read_file",
            "arguments": {"path": "/tmp/unused"}
        })),
        id: Some(json!(42)),
    };

    let response = server
        .handle_request(request)
        .await
        .expect("request should return response envelope");
    let error = response
        .error
        .expect("cancelled request should return error");
    assert_eq!(error.code, super::error_codes::REQUEST_CANCELLED);
}

#[tokio::test]
async fn test_mcp_tool_call_timeout_returns_timeout_error() {
    let server = build_server_with_tool(SlowTool);

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "slow_test_tool",
            "arguments": {},
            "timeoutMs": 1
        })),
        id: Some(json!(99)),
    };

    let response = server
        .handle_request(request)
        .await
        .expect("request should return response envelope");
    let error = response.error.expect("timeout should return error");
    assert_eq!(error.code, super::error_codes::REQUEST_TIMEOUT);
}
