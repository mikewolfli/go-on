# Observability API

The Observability API exposes health checking, Prometheus metrics, and real-time state events. These endpoints are used by monitoring infrastructure, dashboards, and alerting systems.

> The backend JSON-RPC dispatch table lives in `src/acp/impl/request.rs`; `docs/protocol-guide.md` covers protocol modes only. Only endpoints that exist are documented here.

## Endpoints

### `GET /health` — Server Status

Returns the full server status snapshot (`ServerStatus`): request metrics, lifecycle state, circuit breaker snapshots, maintenance status, governance status, and a timestamp.

**HTTP Method:** `GET`

**Response format:** `application/json`

**Example response:**

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

**Fields:**

| Field | Type | Description |
|---|---|---|
| `metrics` | object | Request/response metrics snapshot |
| `lifecycle` | object | Lifecycle state |
| `circuit_breakers` | array | Circuit breaker snapshots |
| `maintenance` | object | Maintenance tracker snapshot |
| `governance` | object \| null | Governance status (when a harness bus is configured) |
| `timestamp` | integer | Unix timestamp of the snapshot |

---

### `GET /health/ready` — Readiness Probe

Returns `200` with `{"ok": true, "status": "ready", "healthy": true}` when the server can accept requests, and `503` with `{"ok": false, "status": "draining", "message": "Server is shutting down"}` while draining.

---

### `GET /metrics` — Prometheus Metrics

Exposes runtime metrics in Prometheus text-based exposition format. Designed to be scraped by a Prometheus server.

**HTTP Method:** `GET`

**Response format:** `text/plain; version=0.0.4`

**Example response:**

```
# HELP go_on_request_count Total number of requests processed
# TYPE go_on_request_count counter
go_on_request_count 1024
# HELP go_on_request_duration_seconds Request duration in seconds
# TYPE go_on_request_duration_seconds gauge
go_on_request_duration_seconds 0.042
# HELP go_on_inflight_requests Currently active requests
# TYPE go_on_inflight_requests gauge
go_on_inflight_requests 3
# HELP go_on_circuit_breaker_state Number of open circuit breakers
# TYPE go_on_circuit_breaker_state gauge
go_on_circuit_breaker_state 0
```

**Common metric families:**

| Metric | Type | Description |
|---|---|---|
| `go_on_request_count` | Counter | Total requests processed |
| `go_on_request_duration_seconds` | Gauge | Average request duration |
| `go_on_inflight_requests` / `go_on_active_requests` | Gauge | Currently active requests |
| `go_on_circuit_breaker_state` | Gauge | Open circuit breakers |
| `go_on_agent_success_rate` | Gauge | Agent success rate (0–100) |
| `go_on_p95_latency_ms` | Gauge | P95 latency |
| `go_on_cache_hit_ratio` | Gauge | Cache hit ratio (0.0–1.0) |
| `go_on_error_rate` | Gauge | Error rate percentage |
| `go_on_chat_requests_total` | Counter | Chat requests processed |
| `go_on_review_gate_total` | Counter | Review gate evaluations |
| `go_on_vector_search_total` | Counter | Vector search operations |
| `go_on_successful_requests_total` / `go_on_failed_requests_total` | Counter | Success/failure totals |
| `go_on_memory_usage_bytes` | Gauge | RSS memory usage in bytes |
| `go_on_lifecycle_healthy` / `go_on_draining` / `go_on_maintenance_mode` | Gauge | Lifecycle/maintenance flags (0/1) |

---

### JSON-RPC — Trace Inspection

Trace inspection is available through JSON-RPC (via `POST /rpc`):

| Method | Description |
|---|---|
| `trace.get` | Trace payload for the current request context |
| `trace.metrics` | Trace metrics snapshot |
| `metrics.prometheus` | Prometheus-format metrics as a JSON-RPC result |

---

### `GET /v1/state/events` — State Sync SSE

Server-sent events stream of state sync events (`event: state_sync`) with a 30-second heartbeat:

```http
GET /v1/state/events HTTP/1.1
Accept: text/event-stream
```

| Event type | Payload | Trigger |
|---|---|---|
| `models_changed` | `{ models: string[] }` | Model list updated |
| `config_reloaded` | `{ changed_keys: string[] }` | Config file hot-reloaded |
| `agents_changed` | `{ added: string[], removed: string[] }` | Agent registry modified |
| `backend_restarting` | `{ reason: string, restart_in_ms: number }` | Backend about to restart |
| `heartbeat` | `{ timestamp: number }` | Periodic keep-alive (30s) |

---

### `GET /protocol/version`

Returns supported protocol versions and the server version:

```json
{
  "supported_versions": [1, 2],
  "latest": 2,
  "server": "go-on",
  "server_version": "1.5.0"
}
```

## Authentication

Health endpoints are generally accessible to monitoring infrastructure without authentication. In production deployments, access is typically restricted via network policy, reverse proxy authentication, or a dedicated API key.

## Next Steps

- Configure Prometheus to scrape the `/metrics` endpoint.
- Set up health-check probes in your orchestrator using `/health` and `/health/ready`.
- Subscribe to `/v1/state/events` for real-time state change notifications.
