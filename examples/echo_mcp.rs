//! Minimal echo MCP server — used by `McpStdioClient` tests (BLUE72 P1).
//!
//! Speaks the MCP protocol over stdio: handles `initialize`, `tools/list`
//! (exposing a single `echo` tool), and `tools/call` (echoing `text` back
//! inside the standard `content` array). Build with:
//!
//! ```sh
//! cargo build --example echo_mcp
//! ```

use serde_json::{json, Value};
use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let id = request.get("id").cloned();

        let result = match method {
            "initialize" => Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "echo-mcp", "version": "1.0.0" },
            })),
            "tools/list" => Some(json!({
                "tools": [{
                    "name": "echo",
                    "description": "Echo the provided text back",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "text": { "type": "string" } },
                        "required": ["text"],
                    },
                }],
            })),
            "tools/call" => {
                let arguments = request.get("params").and_then(|p| p.get("arguments"));
                let text = arguments
                    .and_then(|a| a.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                Some(json!({
                    "content": [{ "type": "text", "text": text }],
                }))
            }
            _ => None,
        };

        let response = match (id, result) {
            (Some(id), Some(result)) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            (Some(id), None) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": "Method not found" },
            }),
            // Notification — no response.
            (None, _) => continue,
        };
        let mut line = serde_json::to_string(&response).expect("serialize response");
        line.push('\n');
        let _ = out.write_all(line.as_bytes());
        let _ = out.flush();
    }
}
