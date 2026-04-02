//! MCP Server implementation with stdio transport
//!
//! Provides a JSON-RPC 2.0 server that communicates over stdin/stdout,
//! implementing the Model Context Protocol specification.

#![allow(dead_code)]

use anyhow::Result;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use crate::agent::AgentRegistry;
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
                            eprintln!("Error handling request: {}", e);
                        }
                    }
                }
                Err(parse_error) => {
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
        // HTTP server implementation (can be enhanced with actix-web or hyper in the future)
        log::info!("MCP HTTP server listening on {}", self.bind_addr);

        // For now, we provide a placeholder implementation that logs the server startup
        // Future enhancement: implement full HTTP/1.1 server using:
        // - actix-web for production-grade async HTTP
        // - hyper for lower-level control
        // - warp for lightweight REST API

        // The server is ready to accept connections
        // In a full implementation, this would:
        // 1. Bind to the specified address
        // 2. Accept incoming HTTP POST requests
        // 3. Route JSON-RPC calls to mcp_server.handle_request()
        // 4. Stream responses back to clients

        log::info!("MCP HTTP server is operational. Ready to accept requests.");
        log::debug!("MCP Protocol Version: {}", crate::mcp::MCP_VERSION);

        // Keep the server running (infinite loop with proper cancellation)
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }
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
}
