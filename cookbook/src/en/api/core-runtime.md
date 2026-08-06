# Core Runtime API

## Overview

The Core Runtime API covers runtime lifecycle, health, configuration, and maintenance operations. go-on's primary programmatic interface is **JSON-RPC 2.0 over HTTP** (`POST /rpc`); a small set of HTTP GET endpoints and the SSE streaming endpoint complement it for health probes, metrics scraping, and chat streaming.

The complete, authoritative JSON-RPC method reference lives in `docs/protocol-guide.md`. This page documents the endpoints that exist today; anything not listed here should not be assumed to exist.

## HTTP Endpoints

### GET Endpoints

| Path | Description |
|---|---|
| `/` | Root capabilities response (protocol, endpoints, version) |
| `/health` | Server status snapshot (see below) |
| `/health/ready` | Readiness probe — `200` when ready, `503` while draining |
| `/metrics` | Prometheus text-format metrics |
| `/protocol/version` | Supported protocol versions and server version |
| `/v1/models` | List available models (OpenAI-compatible) |
| `/v1/model` | Alias for `/v1/models` |
| `/models` | Alias for `/v1/models` |
| `/v1/responses` | List OpenAI Responses API payloads |
| `/v1/responses/{id}` | Get a response by ID (OpenAI Responses API) |
| `/v1/state/events` | SSE stream of state sync events |

### POST Endpoints

| Path | Description |
|---|---|
| `/rpc` | JSON-RPC 2.0 method dispatch (primary interface; `/` also accepts it) |
| `/chat` | Chat completion (ACP JSON-RPC format) |
| `/chat/stream` | Streaming chat completion (SSE) |
| `/v1/chat/completions` | OpenAI-compatible chat completions |
| `/chat/completions` | OpenAI-compatible chat completions |
| `/v1/responses` | OpenAI Responses API |

> All JSON-RPC method names below are dispatched via `POST /rpc`. For the full
> method reference, see `docs/protocol-guide.md`.

## Health Checks

### GET /health

Returns the full server status snapshot (`ServerStatus`): request metrics, lifecycle state, circuit breaker snapshots, maintenance status, governance status, and a timestamp.

**Response:**

```json
{
  "metrics": {
    "total_requests": 1000,
    "successful_requests": 950,
    "failed_requests": 50,
    "avg_request_duration_ms": 42.5,
    "active_requests": 3,
    "cache_hit_rate": 0.8,
    "chat_requests_total": 400
  },
  "lifecycle": { "state": "running" },
  "circuit_breakers": [],
  "maintenance": { "active": false },
  "governance": { "status": "healthy" },
  "timestamp": 1760000000
}
```

### GET /health/ready

Readiness probe. Returns `200` with `{"ok": true, "status": "ready", "healthy": true}` when the server can accept requests, and `503` with `{"ok": false, "status": "draining", "message": "Server is shutting down"}` while draining.

### JSON-RPC health methods

| Method | Description |
|---|---|
| `health` / `runtime.health` | Runtime health snapshot |
| `health.probes` | Module-level health probes |
| `health.check` | Runs a full health check; returns `{"ok": true}` on success |

Example:

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"runtime.health","params":{}}'
```

## Runtime Information

Runtime introspection is available through JSON-RPC methods:

| Method | Description |
|---|---|
| `runtime.stability` | Runtime stability metrics |
| `runtime.features` | Enabled runtime features |
| `runtime.self_model` | Self-model snapshot (stability, learning, knowledge) |
| `provider.status` | Configured AI provider readiness |
| `provider.catalog` / `provider.list_models` | Provider/model catalog |
| `capabilities.list` | Server capabilities |
| `selector.status` | Model/tool selection status |
| `models.list` / `models/list` | List available models |

## Configuration Management

Configuration is managed via JSON-RPC, not REST:

| Method | Description |
|---|---|
| `config.reload` | Re-validate and reload configuration from disk; publishes state-sync events (`ConfigReloaded`, `AgentsChanged`, `ModelsChanged`) when relevant |
| `config.baseline` | Effective configuration baseline and legacy-key migration report |
| `debug_panel.get` / `debug.panel.get` | Debug panel payload |

Note: `config.reload` applies runtime settings immediately, but agent/cache/vector changes require a restart (the response includes a warning count and profile recommendation).

Example:

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"config.reload","params":{}}'
```

## Lifecycle

### JSON-RPC

| Method | Description |
|---|---|
| `initialize` | ACP initialize handshake |
| `shutdown` | Graceful shutdown |
| `session/new`, `session/load`, `session/resume`, `session/close`, `session/list` | Session lifecycle |
| `authenticate` / `logout` | Authentication |
| `mcp.initialize`, `mcp.ping` | MCP handshake |

### Command Line

Runtime lifecycle is also driven from the CLI (see `src/main/cli.rs`):

```bash
go-on --setup                  # run setup wizard (alias: --init)
go-on --setup-level standard   # quick | standard | custom
go-on --setup-profile PROFILE  # setup profile to use
go-on --status                 # print runtime readiness (alias: --check)
go-on --healthcheck            # generate a runtime healthcheck report into .goon/
go-on --diagnose               # run end-to-end diagnosis with remediation hints
go-on --validate-config        # validate configuration and exit (alias: --doctor)
go-on --config config.toml     # explicit config file (alias: -c)
go-on --secret --secret-name KEY --secret-value VALUE   # secret management
go-on -b 127.0.0.1:8090        # bind the ACP HTTP server (alias: --acp-http-bind / --bind)
go-on -m adaptive              # protocol mode override (alias: --protocol-mode / --mode)
go-on -a                       # start interactive terminal chat session
```

Subcommands: `init`, `status`, `diagnose`, `skill`, and `hub` (feature-gated).

## Maintenance Operations

| Method | Description |
|---|---|
| `maintenance.gc` | Run maintenance garbage collection |
| `data.lifecycle` | Data lifecycle review (replay sequence, retention) |
| `cache.clear` | Clear the cache |
| `vector.clear` | Clear the vector store |
| `breaker.status` / `breaker.reset` / `breaker.recovery` | Circuit breaker management |
| `hardness.status` | Harness hardness status |
| `lock.status` | ACP lock status |

Example:

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"maintenance.gc","params":{}}'
```

## Streaming

### POST /chat/stream

SSE streaming chat completion. The server writes SSE frames (`event: chunk`, `done`, `status`, `telemetry`, `tool_approval`, `error`) to the connection until the stream terminates.

```bash
curl -N http://localhost:8090/chat/stream \
  -H "Content-Type: application/json" \
  -d '{"messages":[{"role":"user","content":"Hello"}]}'
```

## Error Handling

JSON-RPC responses use the standard JSON-RPC 2.0 error object:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32602,
    "message": "invalid params: missing field `name`",
    "data": { "code": "DISPATCH_ERROR" }
  }
}
```

Standard codes:

| Code | Meaning |
|---|---|
| `-32700` | Parse error |
| `-32600` | Invalid request |
| `-32601` | Method not found |
| `-32602` | Invalid params |
| `-32603` | Internal error |

HTTP status codes: `200 OK`, `400 Bad Request`, `401 Unauthorized`, `404 Not Found`, `405 Method Not Allowed`, `429 Too Many Requests`, `500 Internal Server Error`, `502 Bad Gateway` (upstream error), `503 Service Unavailable`.

## Security Considerations

- Local mode: API key optional
- Server modes: API key required (sent via `X-Api-Key` / `X-Go-On-Key`)
- RBAC: sensitive operations (`shutdown`, `maintenance.gc`) require admin privileges
- Entry guard, auth, and RBAC are enforced per request by the HTTP handler

## Next Steps

- Explore [Safety and Governance API](./safety-governance.md)
- Learn about [Observability API](./observability.md)
- Check [Workflow and Task API](./workflow-task.md)
