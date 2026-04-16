//! MCP Server implementation with stdio transport
//!
//! Provides a JSON-RPC 2.0 server that communicates over stdin/stdout,
//! implementing the Model Context Protocol specification.

#![allow(dead_code)]

use anyhow::Result;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::agent::AgentRegistry;
use crate::i18n::runtime::{t, tf};
use crate::mcp::{JsonRpcRequest, JsonRpcResponse, McpServer};
use crate::tool::ToolRegistry;

/// MCP Server with stdio transport
pub struct McpStdioServer {
    mcp_server: Arc<McpServer>,
}

impl McpStdioServer {
    /// Create a new MCP stdio server
    pub fn new(
        agent_registry: Arc<AgentRegistry>,
        tool_registry: Arc<ToolRegistry>,
        server_name: String,
        server_version: String,
    ) -> Self {
        let mcp_server = McpServer::new(agent_registry, tool_registry, server_name, server_version);
        Self {
            mcp_server: Arc::new(mcp_server),
        }
    }

    /// Run the server (reads from stdin, writes to stdout)
    pub async fn run(&self) -> Result<()> {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        let mut reader = BufReader::new(stdin);
        let stdout = Arc::new(Mutex::new(stdout));

        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;

            // EOF
            if n == 0 {
                break;
            }

            // Skip empty lines
            if line.trim().is_empty() {
                continue;
            }

            // Parse request
            match serde_json::from_str::<JsonRpcRequest>(&line) {
                Ok(request) => {
                    let response = self.mcp_server.handle_request(request).await;
                    match response {
                        Ok(resp) => {
                            let mut stdout = stdout.lock().await;
                            let response_line = serde_json::to_string(&resp)?;
                            stdout.write_all(response_line.as_bytes()).await?;
                            stdout.write_all(b"\n").await?;
                            stdout.flush().await?;
                        }
                        Err(e) => {
                            warn!(
                                "{}",
                                tf("error.handling_request", &[("error", &format!("{}", e))])
                            );
                        }
                    }
                }
                Err(parse_error) => {
                    let error_response = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        result: None,
                        error: Some(crate::mcp::JsonRpcError {
                            code: crate::mcp::error_codes::PARSE_ERROR,
                            message: tf(
                                "error.parse_error",
                                &[("error", &format!("{}", parse_error))],
                            ),
                            data: None,
                        }),
                        id: None,
                    };

                    let mut stdout = stdout.lock().await;
                    let response_line = serde_json::to_string(&error_response)?;
                    stdout.write_all(response_line.as_bytes()).await?;
                    stdout.write_all(b"\n").await?;
                    stdout.flush().await?;
                }
            }
        }

        Ok(())
    }
}

/// MCP Server with HTTP transport
pub struct McpHttpServer {
    mcp_server: Arc<McpServer>,
    bind_addr: String,
}

impl McpHttpServer {
    /// Create a new MCP HTTP server
    pub fn new(
        agent_registry: Arc<AgentRegistry>,
        tool_registry: Arc<ToolRegistry>,
        server_name: String,
        server_version: String,
        bind_addr: String,
    ) -> Self {
        let mcp_server = McpServer::new(agent_registry, tool_registry, server_name, server_version);
        Self {
            mcp_server: Arc::new(mcp_server),
            bind_addr,
        }
    }

    /// Run the HTTP server
    pub async fn run(&self) -> Result<()> {
        info!(
            "{}",
            tf("info.mcp_server_listening", &[("address", &self.bind_addr)])
        );
        let listener = TcpListener::bind(&self.bind_addr).await?;

        info!("{}", t("info.mcp_server_operational"));
        debug!(
            "{}",
            tf(
                "debug.mcp_server_accepting",
                &[("address", &self.bind_addr)]
            )
        );

        loop {
            let (mut socket, peer_addr) = listener.accept().await?;
            let mcp_server = Arc::clone(&self.mcp_server);

            tokio::spawn(async move {
                if let Err(err) = handle_http_connection(&mut socket, mcp_server).await {
                    warn!(
                        "{}",
                        tf(
                            "error.http_connection",
                            &[
                                ("address", &peer_addr.to_string()),
                                ("error", &format!("{}", err))
                            ]
                        )
                    );
                }
            });
        }
    }
}

async fn handle_http_connection(
    socket: &mut tokio::net::TcpStream,
    mcp_server: Arc<McpServer>,
) -> Result<()> {
    let mut buffer = vec![0u8; 64 * 1024];
    let bytes_read = socket.read(&mut buffer).await?;
    if bytes_read == 0 {
        return Ok(());
    }

    let request_text = String::from_utf8_lossy(&buffer[..bytes_read]);
    let header_end = request_text.find("\r\n\r\n").ok_or_else(|| {
        warn!("MCP HTTP: invalid request — missing header terminator");
        anyhow::anyhow!("invalid HTTP request: missing header terminator")
    })?;

    let (header_part, body_initial_part) = request_text.split_at(header_end + 4);
    let mut lines = header_part.lines();
    let request_line = lines.next().ok_or_else(|| {
        warn!("MCP HTTP: invalid request — missing request line");
        anyhow::anyhow!("invalid HTTP request: missing request line")
    })?;

    let mut request_line_parts = request_line.split_whitespace();
    let method = request_line_parts.next().ok_or_else(|| {
        warn!(
            "MCP HTTP: invalid request — missing method in request line: {}",
            request_line
        );
        anyhow::anyhow!("invalid HTTP request: missing method")
    })?;
    let path = request_line_parts.next().ok_or_else(|| {
        warn!(
            "MCP HTTP: invalid request — missing path in request line: {}",
            request_line
        );
        anyhow::anyhow!("invalid HTTP request: missing path")
    })?;

    if method == "GET" && path == "/health" {
        write_http_json_response(
            socket,
            200,
            serde_json::json!({
                "status": "ok",
                "protocolVersion": crate::mcp::MCP_VERSION,
            }),
        )
        .await?;
        return Ok(());
    }

    if method != "POST" {
        write_http_json_response(
            socket,
            405,
            serde_json::json!({"error": "method not allowed"}),
        )
        .await?;
        return Ok(());
    }

    let content_length = extract_content_length(header_part).unwrap_or(0);
    let mut body_bytes = body_initial_part.as_bytes().to_vec();
    if body_bytes.len() < content_length {
        let mut remaining = vec![0u8; content_length - body_bytes.len()];
        socket.read_exact(&mut remaining).await?;
        body_bytes.extend_from_slice(&remaining);
    }
    body_bytes.truncate(content_length);

    let body_str = String::from_utf8_lossy(&body_bytes);
    let request = match serde_json::from_str::<JsonRpcRequest>(&body_str) {
        Ok(req) => req,
        Err(parse_error) => {
            warn!(
                "MCP HTTP: JSON-RPC parse error from {} {}: {}",
                method, path, parse_error
            );
            let error_response = JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: None,
                error: Some(crate::mcp::JsonRpcError {
                    code: crate::mcp::error_codes::PARSE_ERROR,
                    message: format!("Parse error: {}", parse_error),
                    data: None,
                }),
                id: None,
            };
            write_http_json_response(socket, 200, serde_json::to_value(error_response)?).await?;
            return Ok(());
        }
    };

    let response = mcp_server.handle_request(request).await?;
    debug!("MCP HTTP: dispatched {} {} -> ok", method, path);
    write_http_json_response(socket, 200, serde_json::to_value(response)?).await?;

    Ok(())
}

fn extract_content_length(headers: &str) -> Option<usize> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            value.trim().parse::<usize>().ok()
        } else {
            None
        }
    })
}

async fn write_http_json_response(
    socket: &mut tokio::net::TcpStream,
    status: u16,
    body: serde_json::Value,
) -> Result<()> {
    let body_text = serde_json::to_string(&body)?;
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "OK",
    };

    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        status_text,
        body_text.len(),
        body_text
    );

    socket.write_all(response.as_bytes()).await?;
    socket.flush().await?;
    Ok(())
}

#[cfg(test)]
fn parse_request_target_for_test(raw_request: &str) -> Option<(String, String)> {
    let first_line = raw_request.lines().next()?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    Some((method, path))
}

#[cfg(test)]
fn content_length_for_test(headers: &str) -> Option<usize> {
    extract_content_length(headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mcp_stdio_server_creation() {
        let agent_registry = Arc::new(AgentRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new());
        let _server = McpStdioServer::new(
            agent_registry,
            tool_registry,
            "go-on".to_string(),
            "1.0.0".to_string(),
        );

        // Server was created successfully
    }

    #[tokio::test]
    async fn test_mcp_http_server_creation() {
        let agent_registry = Arc::new(AgentRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new());
        let server = McpHttpServer::new(
            agent_registry,
            tool_registry,
            "go-on".to_string(),
            "1.0.0".to_string(),
            "127.0.0.1:8080".to_string(),
        );

        // Verify server was created successfully
        assert_eq!(
            server.bind_addr, "127.0.0.1:8080",
            "Bind address should be set correctly"
        );
    }

    #[test]
    fn test_extract_content_length() {
        let headers = "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 42\r\n\r\n";
        assert_eq!(content_length_for_test(headers), Some(42));
    }

    #[test]
    fn test_parse_request_target() {
        let request = "GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let parsed = parse_request_target_for_test(request).expect("request line should parse");
        assert_eq!(parsed.0, "GET");
        assert_eq!(parsed.1, "/health");
    }
}
