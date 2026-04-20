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
- **Protocols**: HTTP, RPC
- **Authentication**: API key, JWT tokens (multi-users mode)

### 3. Observability API
- **Purpose**: Metrics, tracing, logging, health monitoring
- **Protocols**: HTTP, OpenTelemetry
- **Authentication**: API key (metrics endpoints may be public)

### 4. Reliability API
- **Purpose**: Circuit breakers, retries, maintenance operations
- **Protocols**: HTTP, RPC
- **Authentication**: API key

### 5. Workflow and Task API
- **Purpose**: Workflow execution, task planning and management
- **Protocols**: HTTP, RPC
- **Authentication**: API key, JWT tokens

### 6. Learning and Intelligence API
- **Purpose**: Machine learning, reinforcement learning, adaptive selection
- **Protocols**: HTTP, RPC
- **Authentication**: API key

### 7. Optimization and Operations API
- **Purpose**: Cost optimization, performance tuning, operational metrics
- **Protocols**: HTTP, RPC
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
- **Authentication**: Bearer token or API key header

### RPC (Remote Procedure Call)
- **Transport**: HTTP/2 with gRPC or JSON-RPC
- **Serialization**: Protocol Buffers or JSON
- **Features**: Bidirectional streaming, cancellation, deadlines

## Authentication

### API Keys
```bash
# Local mode (optional)
export GO_ON_ENTRY_API_KEY="your-api-key"

# Server modes (required)
export GO_ON_SERVER_API_KEY="server-key"
export GO_ON_ENTRY_API_KEY="entry-key"
```

### JWT Tokens (Multi-Users Mode)
```bash
# Obtain token
curl -X POST http://localhost:8090/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"user","password":"pass"}'

# Use token
curl http://localhost:8090/api/v1/users/me \
  -H "Authorization: Bearer <jwt-token>"
```

### OAuth 2.0 (Enterprise)
- **Providers**: Google, GitHub, Okta, Azure AD
- **Scopes**: `read`, `write`, `admin`
- **Flows**: Authorization code, client credentials

## Rate Limiting

### Default Limits
- **Local mode**: 240 requests per minute, 60 burst
- **Simple server**: 1000 requests per minute, 200 burst
- **Multi-users server**: 5000 requests per minute, 1000 burst

### Headers
```
X-RateLimit-Limit: 1000
X-RateLimit-Remaining: 950
X-RateLimit-Reset: 1614556800
```

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

## Versioning

### API Version Header
```http
Accept: application/vnd.go-on.v1+json
```

### URL Versioning
```
http://localhost:8090/api/v1/health
http://localhost:8090/api/v2/health
```

### Deprecation Policy
- **Warning**: Deprecated endpoints return `X-API-Deprecated: true` header
- **Sunset**: Deprecated for 6 months before removal
- **Migration**: Documentation provides migration guides

## Pagination

### Cursor-based Pagination
```json
{
  "data": [...],
  "pagination": {
    "next_cursor": "eyJpZCI6IjEwMCJ9",
    "has_more": true,
    "total": 1000
  }
}
```

### Limit and Offset
```
GET /api/v1/users?limit=20&offset=40
```

## Filtering and Sorting

### Filtering
```
GET /api/v1/logs?level=error&since=2024-01-01T00:00:00Z
```

### Sorting
```
GET /api/v1/users?sort=created_at&order=desc
```

## Field Selection

### Partial Responses
```
GET /api/v1/users/123?fields=id,name,email
```

### Nested Field Selection
```
GET /api/v1/projects/456?fields=id,name,tasks(id,title,status)
```

## WebSocket Support

### Real-time Updates
```javascript
const ws = new WebSocket('ws://localhost:8090/ws');
ws.onmessage = (event) => {
  const data = JSON.parse(event.data);
  console.log('Update:', data);
};
```

### Events
- `workflow.completed`
- `task.updated`
- `error.occurred`
- `health.status_changed`

## OpenAPI Specification

### Accessing OpenAPI Docs
```
http://localhost:8090/docs
http://localhost:8090/openapi.json
http://localhost:8090/openapi.yaml
```

### Generating Client SDKs
```bash
# TypeScript
npx openapi-typescript http://localhost:8090/openapi.json --output client.ts

# Python
openapi-python-client generate --url http://localhost:8090/openapi.json

# Go
oapi-codegen -package api -generate types,client http://localhost:8090/openapi.json > api.gen.go
```

## Client Libraries

### Official Libraries
- **TypeScript/JavaScript**: `@go-on/client`
- **Python**: `go-on-client`
- **Go**: `github.com/your-org/go-on/client`
- **Rust**: `go-on-client` crate

### Community Libraries
- **Java**: `go-on-java-client`
- **C#**: `GoOn.Client`
- **Ruby**: `go_on_ruby`

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
- **Vector searches**: < 2s

### Throughput
- **Local mode**: ~100 requests/second
- **Simple server**: ~500 requests/second
- **Multi-users server**: ~2000 requests/second

## Security

### TLS/SSL
```bash
# Generate self-signed certificate
openssl req -x509 -newkey rsa:4096 -keyout key.pem -out cert.pem -days 365

# Configure in runtime
[runtime]
tls_cert_path = "cert.pem"
tls_key_path = "key.pem"
```

### CORS Configuration
```toml
[security.cors]
allowed_origins = ["https://example.com", "http://localhost:3000"]
allowed_methods = ["GET", "POST", "PUT", "DELETE", "OPTIONS"]
allowed_headers = ["Authorization", "Content-Type"]
allow_credentials = true
```

## Monitoring

### Health Checks
```
GET /health
GET /health/ready
GET /health/live
```

### Metrics
```
GET /metrics
GET /metrics/prometheus
```

### Tracing
- **Jaeger**: `http://localhost:16686`
- **Zipkin**: `http://localhost:9411`
- **OpenTelemetry**: `http://localhost:4317`

## Next Steps

Explore specific API groups:
- [Core Runtime API](./core-runtime.md)
- [Safety and Governance API](./safety-governance.md)
- [Observability API](./observability.md)
- [Workflow and Task API](./workflow-task.md)
- [Learning and Intelligence API](./learning-intelligence.md)
- [Optimization and Operations API](./optimization-ops.md)