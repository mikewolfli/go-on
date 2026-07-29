# Observability API

The Observability API exposes endpoints for health checking, Prometheus metrics, OpenTelemetry trace export, and structured log export. These endpoints are used by monitoring infrastructure, dashboards, and alerting systems.

## Endpoints

### `GET /health` — Health Check

Returns the current health status of the go-on instance, including version, uptime, and per-module health information.

**HTTP Method:** `GET`

**Response format:** `application/json`

**Example response:**

```json
{
  "status": "ok",
  "version": "1.4.3",
  "uptime_seconds": 84321,
  "modules": {
    "acp": { "status": "ok" },
    "database": { "status": "ok", "latency_ms": 2 },
    "search": { "status": "ok" }
  }
}
```

**Fields:**

| Field | Type | Description |
|---|---|---|
| `status` | string | Overall health: `"ok"` or `"degraded"` or `"down"` |
| `version` | string | SemVer version of the running build |
| `uptime_seconds` | integer | Seconds since the process started |
| `modules` | object | Per-module health; each module reports `"ok"`, `"degraded"`, or `"down"` |

---

### `GET /metrics` — Prometheus Metrics

Exposes application and system metrics in Prometheus text-based exposition format. Designed to be scraped by a Prometheus server.

**HTTP Method:** `GET`

**Response format:** `text/plain; version=0.0.4`

**Example response:**

```
# HELP go_on_requests_total Total number of HTTP requests
# TYPE go_on_requests_total counter
go_on_requests_total{method="GET",path="/health",status="200"} 1024
# HELP go_on_request_duration_seconds Request duration histogram
# TYPE go_on_request_duration_seconds histogram
go_on_request_duration_seconds_bucket{le="0.005"} 512
go_on_request_duration_seconds_bucket{le="0.01"} 768
go_on_request_duration_seconds_bucket{le="0.025"} 896
go_on_request_duration_seconds_bucket{le="+Inf"} 1024
go_on_request_duration_seconds_sum 12.345
go_on_request_duration_seconds_count 1024
# HELP go_on_active_connections Current number of active connections
# TYPE go_on_active_connections gauge
go_on_active_connections 42
```

**Common metric families:**

| Metric | Type | Description |
|---|---|---|
| `go_on_requests_total` | Counter | Total HTTP requests by method, path, and status |
| `go_on_request_duration_seconds` | Histogram | Request latency distribution |
| `go_on_active_connections` | Gauge | Active TCP/WebSocket connections |
| `go_on_memory_bytes` | Gauge | RSS memory usage in bytes |
| `go_on_cpu_seconds_total` | Counter | Cumulative CPU time |

---

### `GET /traces` — OpenTelemetry Trace Export

Accepts OpenTelemetry trace data via the OTLP HTTP protocol. Intended for ingestion by a collector such as the OpenTelemetry Collector, Jaeger, or Grafana Tempo.

**HTTP Method:** `GET` (health/protocol negotiation) or `POST` (trace data export)

**Request format:** `application/x-protobuf` (OTLP protobuf) or `application/json`

**Response format:** `application/json`

**Example (POST request):**

```http
POST /traces HTTP/1.1
Content-Type: application/x-protobuf
```

**Example response:**

```json
{
  "partialSuccess": {
    "rejectedSpans": 0,
    "rejectedSpanCount": 0
  }
}
```

**Headers:**

| Header | Description |
|---|---|
| `Content-Type` | `application/x-protobuf` or `application/json` |
| `Content-Encoding` | Optional: `gzip` for compressed payloads |

---

### `GET /logs` — Structured Log Export

Accepts and exposes structured log records. Supports both real-time streaming and batch export.

**HTTP Method:** `GET` (query logs) / `POST` (ingest logs)

**Response format:** `application/json`

**Example GET request:**

```http
GET /logs?level=error&since=3600&limit=50 HTTP/1.1
```

**Example response:**

```json
[
  {
    "timestamp": "2026-07-29T10:00:00Z",
    "level": "error",
    "target": "go_on::server",
    "message": "connection refused: dial tcp 10.0.0.1:5432",
    "fields": {
      "peer": "10.0.0.1:5432",
      "component": "database"
    }
  }
]
```

**Query parameters:**

| Parameter | Type | Description |
|---|---|---|
| `level` | string | Filter by log level: `trace`, `debug`, `info`, `warn`, `error` |
| `since` | integer | Return logs from the last N seconds |
| `until` | string | ISO 8601 timestamp; latest log entry to return |
| `limit` | integer | Maximum number of entries to return (default 100, max 1000) |
| `target` | string | Filter by Rust module target (e.g., `go_on::server`) |

## Authentication

Observability endpoints are generally accessible to monitoring infrastructure without authentication. In production deployments, access is typically restricted via network policy, reverse proxy authentication, or a dedicated observability API key.

## Next Steps

- Configure Prometheus to scrape the `/metrics` endpoint.
- Deploy an OpenTelemetry Collector to forward traces to your backend.
- Set up health-check probes in your orchestrator using `/health`.
