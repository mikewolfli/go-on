# 可观测性 API

可观测性 API 提供健康检查、Prometheus 指标和实时状态事件。这些端点供监控基础设施、仪表盘和告警系统使用。

> JSON-RPC 方法分发表见 `src/acp/impl/request.rs`；`docs/protocol-guide.md` 仅介绍协议模式。本页仅记录真实存在的端点。

## 端点

### `GET /health` — 服务器状态

返回完整的服务器状态快照（`ServerStatus`）：请求指标、生命周期状态、熔断器快照、维护状态、治理状态和时间戳。

**HTTP 方法：** `GET`

**响应格式：** `application/json`

**示例响应：**

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

**字段说明：**

| 字段 | 类型 | 描述 |
|---|---|---|
| `metrics` | object | 请求/响应指标快照 |
| `lifecycle` | object | 生命周期状态 |
| `circuit_breakers` | array | 熔断器快照 |
| `maintenance` | object | 维护跟踪器快照 |
| `governance` | object \| null | 治理状态（配置了 harness bus 时） |
| `timestamp` | integer | 快照的 Unix 时间戳 |

---

### `GET /health/ready` — 就绪探针

服务器可接受请求时返回 `200` 及 `{"ok": true, "status": "ready", "healthy": true}`；排空期间返回 `503` 及 `{"ok": false, "status": "draining", "message": "Server is shutting down"}`。

---

### `GET /metrics` — Prometheus 指标

以 Prometheus 文本格式暴露运行时指标。供 Prometheus 服务器抓取。

**HTTP 方法：** `GET`

**响应格式：** `text/plain; version=0.0.4`

**示例响应：**

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

**常用指标族：**

| 指标 | 类型 | 描述 |
|---|---|---|
| `go_on_request_count` | Counter | 处理的请求总数 |
| `go_on_request_duration_seconds` | Gauge | 平均请求时长 |
| `go_on_inflight_requests` / `go_on_active_requests` | Gauge | 当前活动请求数 |
| `go_on_circuit_breaker_state` | Gauge | 打开的熔断器数量 |
| `go_on_agent_success_rate` | Gauge | Agent 成功率（0–100） |
| `go_on_p95_latency_ms` | Gauge | P95 延迟 |
| `go_on_cache_hit_ratio` | Gauge | 缓存命中率（0.0–1.0） |
| `go_on_error_rate` | Gauge | 错误率百分比 |
| `go_on_chat_requests_total` | Counter | 处理的聊天请求数 |
| `go_on_review_gate_total` | Counter | 审查门评估次数 |
| `go_on_vector_search_total` | Counter | 向量搜索操作数 |
| `go_on_successful_requests_total` / `go_on_failed_requests_total` | Counter | 成功/失败总数 |
| `go_on_memory_usage_bytes` | Gauge | RSS 内存使用量（字节） |
| `go_on_lifecycle_healthy` / `go_on_draining` / `go_on_maintenance_mode` | Gauge | 生命周期/维护标志（0/1） |

---

### JSON-RPC — 追踪检查

追踪检查通过 JSON-RPC 提供（经 `POST /rpc`）：

| 方法 | 说明 |
|---|---|
| `trace.get` | 当前请求上下文的追踪载荷 |
| `trace.metrics` | 追踪指标快照 |
| `metrics.prometheus` | 以 JSON-RPC 结果形式返回 Prometheus 格式指标 |

---

### `GET /v1/state/events` — 状态同步 SSE

状态同步事件的服务器推送事件流（`event: state_sync`），带 30 秒心跳：

```http
GET /v1/state/events HTTP/1.1
Accept: text/event-stream
```

| 事件类型 | 载荷 | 触发时机 |
|---|---|---|
| `models_changed` | `{ models: string[] }` | 模型列表更新 |
| `config_reloaded` | `{ changed_keys: string[] }` | 配置文件热重载 |
| `agents_changed` | `{ added: string[], removed: string[] }` | Agent 注册表修改 |
| `backend_restarting` | `{ reason: string, restart_in_ms: number }` | 后端即将重启 |
| `heartbeat` | `{ timestamp: number }` | 周期性保活（30 秒） |

---

### `GET /protocol/version`

返回支持的协议版本和服务器版本：

```json
{
  "supported_versions": [1, 2],
  "latest": 2,
  "server": "go-on",
  "server_version": "1.5.2"
}
```

## 认证

健康端点通常无需认证即可供监控基础设施访问。在生产部署中，通常通过网络策略、反向代理认证或专用 API 密钥来限制访问。

## 后续步骤

- 配置 Prometheus 抓取 `/metrics` 端点。
- 使用 `/health` 和 `/health/ready` 端点配置编排器的健康检查探针。
- 订阅 `/v1/state/events` 获取实时状态变更通知。
