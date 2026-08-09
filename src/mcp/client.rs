//! MCP client — connect to and consume external MCP tool servers (BLUE72 P1).
//!
//! go-on has always been an MCP **server** (stdio + HTTP). This module adds
//! the missing client direction: an agent can connect to an external MCP
//! server (e.g. Playwright, GitHub MCP, a team's internal tool server),
//! discover its tools, and call them.
//!
//! Two transports are supported:
//!
//! - **stdio** — spawns an external MCP server process and speaks JSON-RPC
//!   over its stdin/stdout (the standard MCP "local" transport).
//! - **http** — speaks JSON-RPC over HTTP to a remote MCP server (the
//!   standard MCP "streamable HTTP" transport).
//!
//! The protocol flow follows the MCP spec: `initialize` → `tools/list` →
//! `tools/call`. Notifications (`notifications/initialized`) are sent after
//! initialize per spec, but the client does not require the server to
//! support them.
//!
//! # Feature gates
//!
//! The client compiles in all profiles; stdio transport requires the
//! `sub-bus-distributed-memory`-independent `process` feature of tokio which
//! is enabled at the workspace level.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::timeout;

/// Shared request-id counter for JSON-RPC requests.
static NEXT_REQ_ID: AtomicU64 = AtomicU64::new(1);

/// MCP protocol version advertised by the client.
pub const CLIENT_MCP_VERSION: &str = "2024-11-05";

/// A tool discovered on a remote MCP server.
#[derive(Debug, Clone)]
pub struct McpClientTool {
    /// Tool name (as registered on the remote server).
    pub name: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// JSON Schema for the tool's input.
    pub input_schema: Option<Value>,
}

/// Configuration for an MCP client connection.
#[derive(Debug, Clone)]
pub struct McpClientConfig {
    /// Per-request timeout (default 60s).
    pub request_timeout: Duration,
    /// Maximum protocol-version handshake time (default 10s).
    #[doc(hidden)]
    pub init_timeout: Duration,
}

impl Default for McpClientConfig {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(60),
            init_timeout: Duration::from_secs(10),
        }
    }
}

/// MCP client over stdio (spawns an external server process).
pub struct McpStdioClient {
    child: tokio::process::Child,
    stdin: Mutex<tokio::process::ChildStdin>,
    stdout: tokio::sync::Mutex<tokio::io::Lines<BufReader<tokio::process::ChildStdout>>>,
    server_name: String,
    config: McpClientConfig,
}

impl McpStdioClient {
    /// Spawn an external MCP server process and perform the initialize
    /// handshake. `args` are the command-line arguments (e.g.
    /// `["--port", "9000"]` for a Node-based server).
    pub async fn connect(
        program: &str,
        args: &[&str],
        server_name: &str,
        config: McpClientConfig,
    ) -> Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn MCP server process: {program}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("MCP server stdin not available"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("MCP server stdout not available"))?;

        let mut client = Self {
            child,
            stdin: Mutex::new(stdin),
            stdout: tokio::sync::Mutex::new(BufReader::new(stdout).lines()),
            server_name: server_name.to_string(),
            config,
        };

        client.initialize().await?;
        Ok(client)
    }

    /// Perform the MCP initialize handshake and send the initialized
    /// notification (best-effort).
    async fn initialize(&mut self) -> Result<()> {
        let request = self.request(
            "initialize",
            Some(json!({
                "protocolVersion": CLIENT_MCP_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "go-on", "version": env!("CARGO_PKG_VERSION") },
            })),
        );
        let result = timeout(self.config.init_timeout, request)
            .await
            .context("MCP stdio initialize handshake timed out")??;
        let protocol_version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(CLIENT_MCP_VERSION);
        tracing::info!(
            server = %self.server_name,
            protocol_version,
            "MCP client initialized"
        );
        // Best-effort notification (spec requires it; servers tolerate absence).
        let _ = self
            .notify("notifications/initialized", Some(json!({})))
            .await;
        Ok(())
    }

    /// Send a notification (no response expected).
    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or_else(|| json!({})),
        });
        let mut line = serde_json::to_string(&payload)?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    /// Send a JSON-RPC request and await its response.
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params.unwrap_or_else(|| json!({})),
        });
        let mut line = serde_json::to_string(&payload)?;
        line.push('\n');

        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await?;
        }

        // Read lines until we find the response matching our id (skip server
        // notifications / other responses).
        let mut stdout = self.stdout.lock().await;
        loop {
            let read = timeout(self.config.request_timeout, async {
                let next = stdout.next_line().await?;
                Ok::<Option<String>, anyhow::Error>(next)
            })
            .await
            .context("MCP request timed out")??;
            let Some(read) = read else {
                anyhow::bail!("MCP server closed stdout while awaiting response");
            };
            let value: Value = serde_json::from_str(&read)?;
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = value.get("error") {
                    anyhow::bail!(
                        "MCP error {}: {}",
                        error.get("code").and_then(Value::as_i64).unwrap_or(-1),
                        error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                    );
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }

    /// List tools exposed by the remote MCP server.
    pub async fn list_tools(&self) -> Result<Vec<McpClientTool>> {
        let result = self.request("tools/list", None).await?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut list = Vec::with_capacity(tools.len());
        for tool in tools {
            list.push(McpClientTool {
                name: tool
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                description: tool
                    .get("description")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                input_schema: tool
                    .get("inputSchema")
                    .or_else(|| tool.get("input_schema"))
                    .cloned(),
            });
        }
        Ok(list)
    }

    /// Call a tool on the remote MCP server.
    ///
    /// Returns the raw result value (the `result` field of the JSON-RPC
    /// response, which contains `content`, `structuredContent`, and
    /// `isError` per the MCP spec).
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value> {
        self.request(
            "tools/call",
            Some(json!({ "name": name, "arguments": arguments })),
        )
        .await
    }
}

impl Drop for McpStdioClient {
    fn drop(&mut self) {
        // kill_on_drop(true) handles process cleanup; also try graceful stdin close.
        let _ = self.child.start_kill();
    }
}

/// MCP client over HTTP (streamable HTTP transport).
pub struct McpHttpClient {
    http: reqwest::Client,
    base_url: String,
    server_name: String,
    config: McpClientConfig,
}

impl McpHttpClient {
    /// Connect to a remote MCP server at `base_url` (e.g.
    /// `http://127.0.0.1:9000/mcp`) and perform the initialize handshake.
    pub async fn connect(
        base_url: &str,
        server_name: &str,
        config: McpClientConfig,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .context("build MCP HTTP client")?;
        let client = Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            server_name: server_name.to_string(),
            config,
        };
        client.initialize().await?;
        Ok(client)
    }

    async fn initialize(&self) -> Result<()> {
        let result = timeout(
            self.config.init_timeout,
            self.request(
                "initialize",
                Some(json!({
                    "protocolVersion": CLIENT_MCP_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": "go-on", "version": env!("CARGO_PKG_VERSION") },
                })),
            ),
        )
        .await
        .context("MCP HTTP initialize handshake timed out")??;
        let protocol_version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(CLIENT_MCP_VERSION);
        tracing::info!(
            server = %self.server_name,
            protocol_version,
            "MCP HTTP client initialized"
        );
        let _ = self
            .notify("notifications/initialized", Some(json!({})))
            .await;
        Ok(())
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or_else(|| json!({})),
        });
        let _ = self
            .http
            .post(&self.base_url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&payload)
            .send()
            .await?;
        Ok(())
    }

    /// Send a JSON-RPC request and await its JSON response.
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params.unwrap_or_else(|| json!({})),
        });
        let resp = self
            .http
            .post(&self.base_url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .timeout(self.config.request_timeout)
            .json(&payload)
            .send()
            .await
            .with_context(|| format!("MCP HTTP request '{method}' failed"))?;
        if !resp.status().is_success() {
            anyhow::bail!("MCP HTTP server returned status {}", resp.status());
        }
        let value: Value = resp
            .json()
            .await
            .with_context(|| format!("MCP HTTP response '{method}' parse failed"))?;
        if let Some(error) = value.get("error") {
            anyhow::bail!(
                "MCP error {}: {}",
                error.get("code").and_then(Value::as_i64).unwrap_or(-1),
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            );
        }
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    }

    /// List tools exposed by the remote MCP server.
    pub async fn list_tools(&self) -> Result<Vec<McpClientTool>> {
        let result = self.request("tools/list", None).await?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut list = Vec::with_capacity(tools.len());
        for tool in tools {
            list.push(McpClientTool {
                name: tool
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                description: tool
                    .get("description")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                input_schema: tool
                    .get("inputSchema")
                    .or_else(|| tool.get("input_schema"))
                    .cloned(),
            });
        }
        Ok(list)
    }

    /// Call a tool on the remote MCP server.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value> {
        self.request(
            "tools/call",
            Some(json!({ "name": name, "arguments": arguments })),
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Client registry (process-wide, for the ACP `mcp.client.*` surface)
// ---------------------------------------------------------------------------

/// A connected MCP client, abstracted over the two transports.
pub enum McpClientHandle {
    /// stdio transport (external server process).
    Stdio(Box<McpStdioClient>),
    /// HTTP transport (remote server).
    Http(McpHttpClient),
}

impl McpClientHandle {
    /// List tools exposed by the remote server.
    pub async fn list_tools(&self) -> Result<Vec<McpClientTool>> {
        match self {
            McpClientHandle::Stdio(c) => c.list_tools().await,
            McpClientHandle::Http(c) => c.list_tools().await,
        }
    }

    /// Call a tool on the remote server.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value> {
        match self {
            McpClientHandle::Stdio(c) => c.call_tool(name, arguments).await,
            McpClientHandle::Http(c) => c.call_tool(name, arguments).await,
        }
    }
}

/// Process-wide registry of connected MCP clients.
///
/// Keyed by a caller-supplied client id (e.g. `"playwright"`); the ACP
/// `mcp.client.*` methods create, list, and call through this registry.
#[derive(Default)]
pub struct McpClientRegistry {
    clients: Mutex<std::collections::HashMap<String, Arc<McpClientHandle>>>,
}

impl McpClientRegistry {
    /// Create a new (empty) registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a connected client under `client_id` (replaces any existing).
    pub async fn register(&self, client_id: String, client: McpClientHandle) {
        let mut guard = self.clients.lock().await;
        guard.insert(client_id, Arc::new(client));
    }

    /// Remove a client by id. Returns true if it existed.
    pub async fn unregister(&self, client_id: &str) -> bool {
        let mut guard = self.clients.lock().await;
        guard.remove(client_id).is_some()
    }

    /// Look up a client by id.
    pub async fn get(&self, client_id: &str) -> Option<Arc<McpClientHandle>> {
        let guard = self.clients.lock().await;
        guard.get(client_id).cloned()
    }

    /// List all registered client ids.
    pub async fn ids(&self) -> Vec<String> {
        let guard = self.clients.lock().await;
        let mut ids: Vec<String> = guard.keys().cloned().collect();
        ids.sort();
        ids
    }
}

/// Return the process-wide MCP client registry (created once).
pub fn global_mcp_client_registry() -> Arc<McpClientRegistry> {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<Arc<McpClientRegistry>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| Arc::new(McpClientRegistry::new()))
        .clone()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_tool_shape_parse() {
        // Parsing must accept both camelCase (MCP spec) and snake_case keys.
        let value = json!({
            "tools": [
                { "name": "read_file", "description": "Read a file", "inputSchema": { "type": "object" } },
            ]
        });
        let tools: Vec<McpClientTool> = value["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| McpClientTool {
                name: t
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                description: t
                    .get("description")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                input_schema: t
                    .get("inputSchema")
                    .or_else(|| t.get("input_schema"))
                    .cloned(),
            })
            .collect();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "read_file");
        assert!(tools[0].input_schema.is_some());
    }

    #[tokio::test]
    async fn test_stdio_client_handshake_with_echo_server() {
        // Spawn a tiny in-process MCP server binary via the `echo-mcp`
        // example if present; otherwise skip. This test exercises the real
        // stdio transport end-to-end.
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        let bin = std::path::Path::new(&manifest_dir).join("target/debug/examples/echo_mcp");
        if !bin.exists() {
            eprintln!("skipping: echo_mcp example not built (cargo build --example echo_mcp)");
            return;
        }
        let client = McpStdioClient::connect(
            bin.to_str().unwrap(),
            &[],
            "echo-mcp-test",
            McpClientConfig::default(),
        )
        .await
        .expect("stdio client should connect");

        let tools = client
            .list_tools()
            .await
            .expect("list_tools should succeed");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");

        let result = client
            .call_tool("echo", json!({ "text": "hello" }))
            .await
            .expect("call_tool should succeed");
        assert_eq!(result["content"][0]["text"], "hello");
    }

    #[tokio::test]
    async fn test_http_client_rejects_unreachable_server() {
        // Unreachable server must produce a connection error (not hang).
        let result = McpHttpClient::connect(
            "http://127.0.0.1:1/mcp",
            "unreachable-test",
            McpClientConfig {
                request_timeout: Duration::from_secs(2),
                init_timeout: Duration::from_secs(2),
            },
        )
        .await;
        assert!(result.is_err(), "unreachable MCP server should error");
    }
}
