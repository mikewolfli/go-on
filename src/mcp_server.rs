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
        // Placeholder for HTTP server implementation
        // Would use actix-web, hyper, or similar framework
        log::info!("MCP HTTP server listening on {}", self.bind_addr);
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    async fn test_mcp_stdio_server_creation() {
        // Placeholder for stdio server tests
    }

    #[tokio::test]
    async fn test_mcp_http_server_creation() {
        // Placeholder for HTTP server tests
    }
}
