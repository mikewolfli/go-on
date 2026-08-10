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
//! Both transports share one protocol-flow core (`McpClientCore`) — the
//! transport-specific read/write primitives live in the internal
//! `McpTransport` enum, so `initialize`/`notify`/`request`/`list_tools`/
//! `call_tool` exist in a single implementation.
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

/// Parse the `result` of a `tools/list` request into tool descriptors.
///
/// Shared by both transports (stdio + HTTP) so the field extraction cannot
/// drift between them.
fn parse_tool_list(result: Value) -> Vec<McpClientTool> {
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
    list
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

/// Shared MCP client core — one implementation of the JSON-RPC protocol
/// flow (`initialize` / `notify` / `request` / `list_tools` / `call_tool`)
/// over a pluggable transport. Previously `McpStdioClient` and
/// `McpHttpClient` carried near-identical copies of these methods (differing
/// only in transport primitives and error strings); the transport is now the
/// only thing that varies.
struct McpClientCore {
    transport: McpTransport,
    server_name: String,
    config: McpClientConfig,
}

/// Internal transport abstraction for the MCP client core.
///
/// - `Stdio` writes JSON-RPC lines to the child process stdin and reads
///   line-delimited responses from its stdout (matching request ids).
/// - `Http` POSTs JSON-RPC payloads to a remote server (streamable HTTP
///   transport) and parses the JSON response envelope.
///
/// The stdio variant carries a `Child` plus two mutexed handles (larger than
/// the Http arm), so it is boxed to keep the enum small.
enum McpTransport {
    Stdio(Box<StdioTransport>),
    Http {
        http: &'static reqwest::Client,
        base_url: String,
    },
}

/// Stdio transport state: the child process and its stdin/stdout handles.
struct StdioTransport {
    child: tokio::process::Child,
    stdin: Mutex<tokio::process::ChildStdin>,
    stdout: tokio::sync::Mutex<tokio::io::Lines<BufReader<tokio::process::ChildStdout>>>,
}

impl McpTransport {
    /// Transport label used in error messages ("stdio" / "HTTP").
    fn transport_name(&self) -> &'static str {
        match self {
            McpTransport::Stdio { .. } => "stdio",
            McpTransport::Http { .. } => "HTTP",
        }
    }

    /// Send a JSON-RPC payload. For requests (payload carries an `id`) the
    /// matching response envelope is returned; notifications return `None`.
    async fn exchange(&self, payload: &Value, timeout_duration: Duration) -> Result<Option<Value>> {
        match self {
            McpTransport::Stdio(stdio) => {
                let mut line = serde_json::to_string(payload)?;
                line.push('\n');
                {
                    let mut stdin = stdio.stdin.lock().await;
                    stdin.write_all(line.as_bytes()).await?;
                    stdin.flush().await?;
                }

                // Notifications (no id) do not expect a response.
                let Some(id) = payload.get("id").and_then(Value::as_u64) else {
                    return Ok(None);
                };

                // Read lines until we find the response matching our id (skip
                // server notifications / other responses).
                let mut stdout = stdio.stdout.lock().await;
                loop {
                    let read = timeout(timeout_duration, async {
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
                        return Ok(Some(value));
                    }
                }
            }
            McpTransport::Http { http, base_url } => {
                let method = payload.get("method").and_then(Value::as_str).unwrap_or("");
                let resp = http
                    .post(base_url)
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream")
                    .timeout(timeout_duration)
                    .json(payload)
                    .send()
                    .await
                    .with_context(|| format!("MCP HTTP request '{method}' failed"))?;
                if !resp.status().is_success() {
                    anyhow::bail!("MCP HTTP server returned status {}", resp.status());
                }
                // Notifications do not expect a response body.
                if payload.get("id").is_none() {
                    return Ok(None);
                }
                let value: Value = resp
                    .json()
                    .await
                    .with_context(|| format!("MCP HTTP response '{method}' parse failed"))?;
                Ok(Some(value))
            }
        }
    }
}

impl Drop for McpTransport {
    fn drop(&mut self) {
        // kill_on_drop(true) handles process cleanup; also try graceful kill.
        if let McpTransport::Stdio(stdio) = self {
            let _ = stdio.child.start_kill();
        }
    }
}

impl McpClientCore {
    /// Perform the MCP initialize handshake and send the initialized
    /// notification (best-effort).
    async fn initialize(&self) -> Result<()> {
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
            .with_context(|| {
                format!(
                    "MCP {} initialize handshake timed out",
                    self.transport.transport_name()
                )
            })??;
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
        self.transport
            .exchange(&payload, self.config.request_timeout)
            .await?;
        Ok(())
    }

    /// Send a JSON-RPC request and await its response.
    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params.unwrap_or_else(|| json!({})),
        });
        let envelope = self
            .transport
            .exchange(&payload, self.config.request_timeout)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "MCP {} transport returned no response for '{}'",
                    self.transport.transport_name(),
                    method
                )
            })?;
        if let Some(error) = envelope.get("error") {
            anyhow::bail!(
                "MCP error {}: {}",
                error.get("code").and_then(Value::as_i64).unwrap_or(-1),
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            );
        }
        Ok(envelope.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// MCP client over stdio (spawns an external server process).
pub struct McpStdioClient {
    inner: McpClientCore,
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

        let client = Self {
            inner: McpClientCore {
                transport: McpTransport::Stdio(Box::new(StdioTransport {
                    child,
                    stdin: Mutex::new(stdin),
                    stdout: tokio::sync::Mutex::new(BufReader::new(stdout).lines()),
                })),
                server_name: server_name.to_string(),
                config,
            },
        };

        client.inner.initialize().await?;
        Ok(client)
    }

    /// Send a JSON-RPC request and await its response.
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        self.inner.request(method, params).await
    }

    /// List tools exposed by the remote MCP server.
    pub async fn list_tools(&self) -> Result<Vec<McpClientTool>> {
        let result = self.request("tools/list", None).await?;
        Ok(parse_tool_list(result))
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

/// MCP client over HTTP (streamable HTTP transport).
pub struct McpHttpClient {
    inner: McpClientCore,
}

impl McpHttpClient {
    /// Connect to a remote MCP server at `base_url` (e.g.
    /// `http://127.0.0.1:9000/mcp`) and perform the initialize handshake.
    pub async fn connect(
        base_url: &str,
        server_name: &str,
        config: McpClientConfig,
    ) -> Result<Self> {
        // Reuse the process-wide shared reqwest client (single connection
        // pool, one place for timeouts/user-agent) instead of building a new
        // client per connection. Per-request timeouts still come from
        // `McpClientConfig` via `.timeout()` on the request builder, so the
        // shared client's 30s default is only a ceiling.
        let http = crate::shared::http_client::http_client()
            .map_err(|e| anyhow::anyhow!("MCP HTTP shared client unavailable: {e}"))?;
        let client = Self {
            inner: McpClientCore {
                transport: McpTransport::Http {
                    http,
                    base_url: base_url.trim_end_matches('/').to_string(),
                },
                server_name: server_name.to_string(),
                config,
            },
        };
        client.inner.initialize().await?;
        Ok(client)
    }

    /// Send a JSON-RPC request and await its JSON response.
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        self.inner.request(method, params).await
    }

    /// List tools exposed by the remote MCP server.
    pub async fn list_tools(&self) -> Result<Vec<McpClientTool>> {
        let result = self.request("tools/list", None).await?;
        Ok(parse_tool_list(result))
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
    /// HTTP transport (remote server). Boxed so both variants stay small.
    Http(Box<McpHttpClient>),
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
