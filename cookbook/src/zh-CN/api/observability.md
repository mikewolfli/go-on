# 可观测性 API

可观测性 API 提供健康检查、Prometheus 指标、OpenTelemetry 追踪导出和结构化日志导出的端点。这些端点供监控基础设施、仪表盘和告警系统使用。

## 端点

### `GET /health` — 健康检查

返回 go-on 实例的当前健康状态，包括版本、运行时间和各模块的健康信息。

**HTTP 方法：** `GET`

**响应格式：** `application/json`

**示例响应：**

```json
{
  "status": "ok",
  "version": "1.5.0",
  "uptime_seconds": 84321,
  "modules": {
    "acp": { "status": "ok" },
    "database": { "status": "ok", "latency_ms": 2 },
    "search": { "status": "ok" }
  }
}
```

**字段说明：**

| 字段 | 类型 | 描述 |
|---|---|---|
| `status` | string | 整体健康状态：`"ok"`、`"degraded"` 或 `"down"` |
| `version` | string | 当前运行版本的 SemVer |
| `uptime_seconds` | integer | 进程启动以来的秒数 |
| `modules` | object | 各模块健康状态；每个模块报告 `"ok"`、`"degraded"` 或 `"down"` |

---

### `GET /metrics` — Prometheus 指标

以 Prometheus 文本格式暴露应用和系统指标。供 Prometheus 服务器抓取使用。

**HTTP 方法：** `GET`

**响应格式：** `text/plain; version=0.0.4`

**示例响应：**

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

**常用指标族：**

| 指标 | 类型 | 描述 |
|---|---|---|
| `go_on_requests_total` | Counter | 按方法、路径和状态码统计的 HTTP 请求总数 |
| `go_on_request_duration_seconds` | Histogram | 请求延迟分布 |
| `go_on_active_connections` | Gauge | 当前活跃的 TCP/WebSocket 连接数 |
| `go_on_memory_bytes` | Gauge | RSS 内存使用量（字节） |
| `go_on_cpu_seconds_total` | Counter | 累计 CPU 时间 |

---

### `GET /traces` — OpenTelemetry 追踪导出

通过 OTLP HTTP 协议接收 OpenTelemetry 追踪数据。供 OpenTelemetry Collector、Jaeger 或 Grafana Tempo 等收集器使用。

**HTTP 方法：** `GET`（健康/协议协商）或 `POST`（追踪数据导出）

**请求格式：** `application/x-protobuf`（OTLP protobuf）或 `application/json`

**响应格式：** `application/json`

**示例（POST 请求）：**

```http
POST /traces HTTP/1.1
Content-Type: application/x-protobuf
```

**示例响应：**

```json
{
  "partialSuccess": {
    "rejectedSpans": 0,
    "rejectedSpanCount": 0
  }
}
```

**请求头：**

| 请求头 | 描述 |
|---|---|
| `Content-Type` | `application/x-protobuf` 或 `application/json` |
| `Content-Encoding` | 可选：`gzip` 用于压缩内容 |

---

### `GET /logs` — 结构化日志导出

接收和暴露结构化日志记录。支持实时流和批量导出。

**HTTP 方法：** `GET`（查询日志） / `POST`（写入日志）

**响应格式：** `application/json`

**示例 GET 请求：**

```http
GET /logs?level=error&since=3600&limit=50 HTTP/1.1
```

**示例响应：**

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

**查询参数：**

| 参数 | 类型 | 描述 |
|---|---|---|
| `level` | string | 按日志级别过滤：`trace`、`debug`、`info`、`warn`、`error` |
| `since` | integer | 返回最近 N 秒内的日志 |
| `until` | string | ISO 8601 时间戳；返回该时间之前的日志 |
| `limit` | integer | 最大返回条目数（默认 100，最大 1000） |
| `target` | string | 按 Rust 模块目标过滤（如 `go_on::server`） |

## 认证

可观测性端点通常无需认证即可被监控基础设施访问。在生产部署中，通常通过网络策略、反向代理认证或专用可观测性 API 密钥来限制访问。

## 后续步骤

- 配置 Prometheus 抓取 `/metrics` 端点。
- 部署 OpenTelemetry Collector 将追踪转发到后端。
- 使用 `/health` 端点配置编排器的健康检查探针。
