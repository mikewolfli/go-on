//! ACP handlers for the MCP **client** surface (BLUE72 P1, retained minimal set).
//!
//! go-on has always been an MCP server; these handlers let an agent connect
//! to an external MCP tool server (stdio or HTTP), discover its tools, and
//! call them — the missing client direction.
//!
//! Retained methods (minimal usable surface, per BLUE72 audit A):
//! - `mcp.client.connect` — connect `{ "transport": "stdio"|"http", "client_id", "program"|"base_url", "args"? }`
//! - `mcp.client.list`    — list connected client ids
//! - `mcp.client.call`    — call a tool `{ "client_id", "tool", "arguments" }`

use anyhow::Result;
use serde_json::{json, Value};

use crate::mcp::client::{
    global_mcp_client_registry, McpClientConfig, McpClientHandle, McpHttpClient, McpStdioClient,
};

/// `mcp.client.connect` — establish a connection to an external MCP server.
pub async fn mcp_client_connect_payload(params: Value) -> Result<Value> {
    let client_id = params
        .get("client_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing 'client_id' (string)"))?;
    let transport = params
        .get("transport")
        .and_then(Value::as_str)
        .unwrap_or("http");

    let config = McpClientConfig::default();

    let client = match transport {
        "stdio" => {
            let program = params
                .get("program")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("stdio transport requires 'program'"))?;
            let args: Vec<String> = params
                .get("args")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let client = McpStdioClient::connect(program, &arg_refs, client_id, config).await?;
            McpClientHandle::Stdio(Box::new(client))
        }
        "http" => {
            let base_url = params
                .get("base_url")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("http transport requires 'base_url'"))?;
            let client = McpHttpClient::connect(base_url, client_id, config).await?;
            McpClientHandle::Http(Box::new(client))
        }
        other => anyhow::bail!("unsupported MCP client transport '{other}' (stdio|http)"),
    };

    global_mcp_client_registry()
        .register(client_id.to_string(), client)
        .await;

    Ok(json!({
        "ok": true,
        "client_id": client_id,
        "transport": transport,
    }))
}

/// `mcp.client.list` — list connected client ids and their tool counts.
pub async fn mcp_client_list_payload() -> Result<Value> {
    let registry = global_mcp_client_registry();
    let ids = registry.ids().await;

    // Query all connected clients in parallel: each `list_tools()` is a
    // network round-trip to a remote MCP server, so serialising them would
    // accumulate N × RTT on the request path.
    let tool_lists = futures_util::future::join_all(ids.iter().map(|id| {
        let registry = &registry;
        let id = id.clone();
        async move {
            match registry.get(&id).await {
                Some(client) => client.list_tools().await.unwrap_or_default(),
                None => Vec::new(),
            }
        }
    }))
    .await;

    let mut clients = Vec::with_capacity(ids.len());
    for (id, tools) in ids.iter().zip(tool_lists) {
        let tools_json: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
            .collect();
        clients.push(json!({
            "client_id": id,
            "tool_count": tools.len(),
            "tools": tools_json,
        }));
    }
    Ok(json!({ "ok": true, "clients": clients }))
}

/// `mcp.client.call` — call a tool on a connected client.
///
/// Params: `{ "client_id", "tool", "arguments"? }`. Returns the raw MCP
/// result (`content` array, `structuredContent`, `isError`).
pub async fn mcp_client_call_payload(params: Value) -> Result<Value> {
    let client_id = params
        .get("client_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing 'client_id' (string)"))?;
    let tool = params
        .get("tool")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing 'tool' (string)"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let registry = global_mcp_client_registry();
    let client = registry
        .get(client_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("no MCP client registered as '{client_id}'"))?;
    match client.call_tool(tool, arguments).await {
        Ok(result) => Ok(json!({
            "ok": true,
            "client_id": client_id,
            "tool": tool,
            "result": result,
        })),
        Err(e) => {
            // A call failure usually means the remote server is gone; drop the
            // stale client so the next connect starts fresh.
            let _ = registry.unregister(client_id).await;
            Err(e)
        }
    }
}
