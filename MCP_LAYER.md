# MCP Layer - Model Context Protocol Integration

## Overview

The MCP (Model Context Protocol) layer provides a standardized interface for integrating the go-on agent/orchestration system with Claude and other AI models through the Model Context Protocol specification.

## What is MCP?

The Model Context Protocol is a standardized specification developed by Anthropic for enabling AI models to interact with external tools and resources. It provides:

- **Protocol**: JSON-RPC 2.0 over standard transports (stdio, HTTP, WebSockets)
- **Tools**: Define and expose custom tools to AI models
- **Resources**: Expose read-only information and documents
- **Prompts**: Provide dynamic prompts and context

## Architecture

### Modules

#### 1. `src/mcp.rs` - Core MCP Server
- **McpServer**: Main server implementation handling JSON-RPC requests
- **JsonRpcRequest/JsonRpcResponse**: Protocol message structures
- **McpTool**: Tool definitions and schemas
- **McpResource**: Resource definitions
- Implements MCP specification methods:
  - `initialize`: Protocol handshake
  - `tools/list`: Enumerate available tools
  - `tools/call`: Execute tools
  - `resources/list`: List available resources
  - `resources/read`: Read resource content
  - `agents/list`: List available agents
  - `agents/models`: List available models

#### 2. `src/mcp_server.rs` - Transport Implementations
- **McpStdioServer**: JSON-RPC server over stdin/stdout (primary for Claude integration)
- **McpHttpServer**: HTTP/REST transport (future enhancement)
- Handles request/response streaming and error propagation

## Usage

### Integration with go-on Components

```rust
// In main.rs or server initialization:
use std::sync::Arc;
use crate::agent::AgentRegistry;
use crate::tool::ToolRegistry;
use crate::mcp_server::McpStdioServer;

// Create registries
let agent_registry = Arc::new(AgentRegistry::from_config(config.clone(), client)?);
let tool_registry = Arc::new(ToolRegistry::new());

// Initialize MCP server
let mcp_server = McpStdioServer::new(
    agent_registry,
    tool_registry,
    "go-on".to_string(),
    "0.1.0".to_string(),
);

// Run MCP server (communicates over stdio)
mcp_server.run().await?;
```

### Exposing Tools

Tools from the tool registry are automatically exposed through:

```json
{
  "jsonrpc": "2.0",
  "method": "tools/list",
  "id": 1
}
```

Response:
```json
{
  "jsonrpc": "2.0",
  "result": {
    "tools": [
      {
        "name": "read_file",
        "description": "Read contents of a file",
        "input_schema": {
          "type": "object",
          "properties": {
            "path": {"type": "string"},
            "start_line": {"type": "number"},
            "end_line": {"type": "number"}
          }
        }
      }
    ]
  },
  "id": 1
}
```

### Calling Tools

Claude can invoke tools exposed by go-on:

```json
{
  "jsonrpc": "2.0",
  "method": "tools/call",
  "params": {
    "name": "read_file",
    "arguments": {
      "path": "src/main.rs",
      "start_line": 1,
      "end_line": 50
    }
  },
  "id": 2
}
```

## Protocol Flow

```
Claude/LLM                     go-on MCP Server
    |                                |
    |------- initialize ------------>|
    |<------ capabilities -----------|
    |                                |
    |------- tools/list ------------>|
    |<------ tool list --------------|
    |                                |
    |------- tools/call ------------>|
    |<------ tool result ------------|
    |                                |
    |------- resources/list -------->|
    |<------ resource list ---------|
    |                                |
    |------- resources/read -------->|
    |<------ resource content ------|
```

## Supported Transports

### Primary: stdio (in-process stdin/stdout)
- Used when Claude is running MCP server as subprocess
- Simplest integration, lowest latency
- No additional port allocation

### Future: HTTP/REST
- For remote integration
- Allows cross-machine communication
- WebSocket support for real-time updates

## Integration Points

### 1. Agent Discovery
Agents from `AgentRegistry` are exposed through `agents/list`:
```json
{
  "agents": [
    {"name": "openai", "type": "chat", "default_model": "gpt-4"},
    {"name": "deepseek", "type": "chat", "default_model": "deepseek-chat"}
  ]
}
```

### 2. Tool Schema Definition
Tools include JSON Schema for parameter validation:
```json
{
  "type": "object",
  "properties": {
    "path": {"type": "string", "description": "File path"},
    "content": {"type": "string", "description": "File content"}
  },
  "required": ["path", "content"]
}
```

### 3. Model Listing
Available models per agent through `agents/models`:
```json
{
  "models": [
    {"agent": "deepseek", "model": "deepseek-v3", "capabilities": ["chat", "vision"]},
    {"agent": "openai", "model": "gpt-4o", "capabilities": ["chat", "vision", "function_calling"]}
  ]
}
```

## Error Handling

Standard JSON-RPC error codes:

| Code | Error | Description |
|------|-------|-------------|
| -32700 | Parse error | Invalid JSON was received |
| -32600 | Invalid Request | Request object was malformed |
| -32601 | Method not found | Method does not exist |
| -32602 | Invalid params | Invalid method parameters |
| -32603 | Internal error | Internal server error |

Example error response:
```json
{
  "jsonrpc": "2.0",
  "error": {
    "code": -32601,
    "message": "Method not found",
    "data": {"method": "unknown_method"}
  },
  "id": 1
}
```

## Testing

Run tests for MCP components:

```bash
# Test MCP core server
cargo test mcp::tests

# Test MCP stdio server
cargo test mcp_server::tests

# Full test suite
cargo test
```

## Use Cases

### 1. Claude Desktop Integration
Run go-on as MCP server for Claude Desktop application:
```
go-on --mcp-stdio
```

### 2. Agent Context
Claude can use go-on agents with full access to tools:
- Query agent capabilities
- Invoke tools in specific agent context
- Get model recommendations

### 3. Cross-platform Automation
Expose go-on functionality to any MCP client:
- VSCode extensions
- Browser plugins
- Third-party integrations

## Configuration

Add MCP-specific configuration to `config.toml`:

```toml
[mcp]
# Enable MCP server
enabled = true

# Transport mode: "stdio" or "http"
transport = "stdio"

# (Optional) HTTP binding address
listen_addr = "127.0.0.1:8080"

# Server info
name = "go-on"
version = "0.1.2"
```

## Future Enhancements

1. **Resource Management**
   - Expose project files as resources
   - Dynamic resource generation
   - File watching and change notifications

2. **Prompt Templates**
   - Standard prompt formats for agents
   - Reusable context templates
   - Dynamic prompt injection

3. **Logging Integration**
   - Stream execution logs to Claude
   - Real-time progress updates
   - Structured logging

4. **Advanced Features**
   - Sampling parameters customization
   - Streaming tool results
   - Batch tool execution

## References

- [MCP Specification](https://modelcontextprotocol.io/spec/)
- [Claude Documentation](https://docs.anthropic.com/)
- [JSON-RPC 2.0](https://www.jsonrpc.org/specification)
- [JSON Schema](https://json-schema.org/)

## See Also

- [src/acp.rs](src/acp.rs) - ACP protocol implementation
- [src/agent.rs](src/agent.rs) - Agent system
- [src/tool.rs](src/tool.rs) - Tool registry
- [SAFEGUARD_MODE.md](SAFEGUARD_MODE.md) - Mode specification
