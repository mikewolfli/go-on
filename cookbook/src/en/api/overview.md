# API Overview

## Introduction

go-on provides a comprehensive API surface for agent orchestration, governance, and production operations. The API is organized into logical groups that correspond to different aspects of the system.

## API Groups

### 1. Core Runtime API
- **Purpose**: System initialization, shutdown, and basic runtime operations
- **Protocols**: ACP over stdio/HTTP, MCP over stdio/HTTP
- **Authentication**: API key (optional for local mode, required for server modes)

### 2. Safety and Governance API
- **Purpose**: Security policies, audit logging, compliance monitoring
- **Protocols**: HTTP, JSON-RPC
- **Authentication**: API key

### 3. Observability API
- **Purpose**: Metrics, tracing, logging, health monitoring
- **Protocols**: HTTP, JSON-RPC
- **Authentication**: API key (health endpoint may be public)

### 4. Reliability API
- **Purpose**: Circuit breakers, retries, maintenance operations
- **Protocols**: HTTP, JSON-RPC
- **Authentication**: API key

### 5. Workflow and Task API
- **Purpose**: Workflow execution, task planning and management
- **Protocols**: HTTP, JSON-RPC
- **Authentication**: API key

### 6. Learning and Intelligence API
- **Purpose**: Machine learning, reinforcement learning, adaptive selection
- **Protocols**: HTTP, JSON-RPC
- **Authentication**: API key

### 7. Optimization and Operations API
- **Purpose**: Cost optimization, performance tuning, operational metrics
- **Protocols**: HTTP, JSON-RPC
- **Authentication**: API key

## Protocol Support

### ACP (Agent Coordination Protocol)
- **Over stdio**: For editor integrations (Zed, VS Code)
- **Over HTTP**: For remote access and server deployments
- **Features**: Bidirectional streaming, request/response, notifications

### MCP (Model Context Protocol)
- **Over stdio**: For model provider integrations
- **Over HTTP**: For web-based integrations
- **Features**: Tool calling, context management, streaming

### HTTP REST API
- **Base URL**: `http://localhost:8090` (default)
- **Content-Type**: `application/json`
- **Authentication**: Bearer token or API key header (`X-Api-Key` or `X-Go-On-Key`)

### JSON-RPC over HTTP
- **Endpoint**: `POST /v1/responses`
- **Serialization**: JSON
- **Features**: Request/response with method routing (`runtime.health`, `governance.status`, etc.)

## Authentication

### API Key
```bash
# Local mode (optional)
export GO_ON_ENTRY_API_KEY="your-api-key"

# Server modes (required)
export GO_ON_SERVER_API_KEY="server-key"
export GO_ON_ENTRY_API_KEY="entry-key"
```

API keys are sent via the `X-Api-Key` or `X-Go-On-Key` HTTP header. Authentication is enforced based on the `entry_auth_enabled` runtime configuration.

## Rate Limiting

### Default Limits
- **Local mode**: 240 requests per minute, 60 burst
- **Simple server**: 1000 requests per minute, 200 burst
- **Multi-users server**: 5000 requests per minute, 1000 burst

Rate limiting is enforced internally via a token bucket algorithm per phase.

## Error Handling

### HTTP Status Codes
- `200 OK`: Success
- `400 Bad Request`: Invalid input
- `401 Unauthorized`: Authentication required
- `403 Forbidden`: Insufficient permissions
- `404 Not Found`: Resource not found
- `429 Too Many Requests`: Rate limit exceeded
- `500 Internal Server Error`: Server error
- `503 Service Unavailable`: Service temporarily unavailable

### Error Response Format
```json
{
  "error": {
    "code": "RATE_LIMIT_EXCEEDED",
    "message": "Rate limit exceeded. Please try again later.",
    "details": {
      "limit": 1000,
      "remaining": 0,
      "reset_at": "2024-01-01T00:00:00Z"
    },
    "request_id": "req_1234567890abcdef"
  }
}
```

## HTTP Endpoints

### GET Endpoints

| Path | Description |
|---|---|
| `/` | Root capabilities response (protocol, endpoints, version) |
| `/health` | Health check (status, version, uptime) |
| `/v1/models` | List available models (OpenAI-compatible) |
| `/v1/model` | Alias for `/v1/models` |
| `/models` | Alias for `/v1/models` |
| `/v1/responses/{id}` | Get a response by ID |

### POST Endpoints

| Path | Description |
|---|---|
| `/chat` | Chat completion (ACP JSON-RPC format) |
| `/chat/stream` | Streaming chat completion (SSE) |
| `/v1/chat/completions` | OpenAI-compatible chat completions |
| `/chat/completions` | OpenAI-compatible chat completions |
| `/v1/responses` | JSON-RPC 2.0 method dispatch |

## Client Libraries

### Official Libraries
- **Python**: `go-on-sdk` (install via `pip install go-on-sdk`)
- **Rust**: `go-on-client` crate

### Generating a Custom Client
The JSON-RPC interface at `POST /v1/responses` is straightforward to call from any language:

```bash
curl http://localhost:8090/v1/responses \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"runtime.health","params":{}}'
```

## Testing

### Mock Server
```bash
# Start mock server
go-on --mock --port 8080

# Test with curl
curl http://localhost:8080/health
```

### Integration Tests
```bash
# Run API tests
cargo test --test api

# Run specific test group
cargo test --test api_health
```

## Performance

### Response Times
- **Health check**: < 100ms
- **Simple requests**: < 500ms
- **Complex workflows**: < 5s

### Throughput
- **Local mode**: ~100 requests/second
- **Simple server**: ~500 requests/second
- **Multi-users server**: ~2000 requests/second

## Security

### CORS Configuration
```toml
[security.cors]
allowed_origins = ["https://example.com", "http://localhost:3000"]
allowed_methods = ["GET", "POST", "PUT", "DELETE", "OPTIONS"]
allowed_headers = ["Authorization", "Content-Type", "X-Api-Key", "X-Go-On-Key"]
allow_credentials = true
```

## Monitoring

### Health Checks
```
GET /health
```

### OpenTelemetry Tracing
go-on uses OpenTelemetry for internal tracing of chat completions, agent calls, and review gates. Traces are emitted to any configured OTLP collector.

### Prometheus Metrics
Internal runtime metrics (latency histograms, circuit breaker states, rate limiter tokens) are available via JSON-RPC:

```bash
curl http://localhost:8090/v1/responses \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"metrics.prometheus","params":{}}'
```

## Next Steps

Explore specific API groups:
- [Core Runtime API](./core-runtime.md)
- [Safety and Governance API](./safety-governance.md)
- [Observability API](./observability.md)
- [Workflow and Task API](./workflow-task.md)
- [Learning and Intelligence API](./learning-intelligence.md)
- [Optimization and Operations API](./optimization-ops.md)
