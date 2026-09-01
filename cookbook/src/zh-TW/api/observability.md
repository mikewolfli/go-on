# 可觀測性 API

可觀測性 API 提供健康檢查、Prometheus 指標和即時狀態事件。這些端點供監控基礎設施、儀表板和警示系統使用。

> JSON-RPC 方法分發表見 `src/acp/impl/request.rs`；`docs/protocol-guide.md` 僅介紹協議模式。本頁僅記錄真實存在的端點。

## 端點

### `GET /health` — 伺服器狀態

回傳完整的伺服器狀態快照（`ServerStatus`）：請求指標、生命週期狀態、斷路器快照、維護狀態、治理狀態和時間戳。

**HTTP 方法：** `GET`

**回應格式：** `application/json`

**範例回應：**

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

**欄位說明：**

| 欄位 | 類型 | 描述 |
|---|---|---|
| `metrics` | object | 請求/回應指標快照 |
| `lifecycle` | object | 生命週期狀態 |
| `circuit_breakers` | array | 斷路器快照 |
| `maintenance` | object | 維護追蹤器快照 |
| `governance` | object \| null | 治理狀態（設定了 harness bus 時） |
| `timestamp` | integer | 快照的 Unix 時間戳 |

---

### `GET /health/ready` — 就緒探針

伺服器可接受請求時回傳 `200` 及 `{"ok": true, "status": "ready", "healthy": true}`；排空期間回傳 `503` 及 `{"ok": false, "status": "draining", "message": "Server is shutting down"}`。

---

### `GET /metrics` — Prometheus 指標

以 Prometheus 文字格式暴露運行時指標。供 Prometheus 伺服器抓取。

**HTTP 方法：** `GET`

**回應格式：** `text/plain; version=0.0.4`

**範例回應：**

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

**常用指標族：**

| 指標 | 類型 | 描述 |
|---|---|---|
| `go_on_request_count` | Counter | 處理的請求總數 |
| `go_on_request_duration_seconds` | Gauge | 平均請求時長 |
| `go_on_inflight_requests` / `go_on_active_requests` | Gauge | 目前活動請求數 |
| `go_on_circuit_breaker_state` | Gauge | 開啟的斷路器數量 |
| `go_on_agent_success_rate` | Gauge | Agent 成功率（0–100） |
| `go_on_p95_latency_ms` | Gauge | P95 延遲 |
| `go_on_cache_hit_ratio` | Gauge | 快取命中率（0.0–1.0） |
| `go_on_error_rate` | Gauge | 錯誤率百分比 |
| `go_on_chat_requests_total` | Counter | 處理的聊天請求數 |
| `go_on_review_gate_total` | Counter | 審查門評估次數 |
| `go_on_vector_search_total` | Counter | 向量搜尋操作數 |
| `go_on_successful_requests_total` / `go_on_failed_requests_total` | Counter | 成功/失敗總數 |
| `go_on_memory_usage_bytes` | Gauge | RSS 記憶體使用量（位元組） |
| `go_on_lifecycle_healthy` / `go_on_draining` / `go_on_maintenance_mode` | Gauge | 生命週期/維護旗標（0/1） |

---

### JSON-RPC — 追蹤檢查

追蹤檢查透過 JSON-RPC 提供（經 `POST /rpc`）：

| 方法 | 說明 |
|---|---|
| `trace.get` | 目前請求上下文的追蹤載荷 |
| `trace.metrics` | 追蹤指標快照 |
| `metrics.prometheus` | 以 JSON-RPC 結果形式回傳 Prometheus 格式指標 |

---

### `GET /v1/state/events` — 狀態同步 SSE

狀態同步事件的伺服器推送事件串流（`event: state_sync`），帶 30 秒心跳：

```http
GET /v1/state/events HTTP/1.1
Accept: text/event-stream
```

| 事件類型 | 載荷 | 觸發時機 |
|---|---|---|
| `models_changed` | `{ models: string[] }` | 模型列表更新 |
| `config_reloaded` | `{ changed_keys: string[] }` | 設定檔熱重載 |
| `agents_changed` | `{ added: string[], removed: string[] }` | Agent 登錄檔修改 |
| `backend_restarting` | `{ reason: string, restart_in_ms: number }` | 後端即將重啟 |
| `heartbeat` | `{ timestamp: number }` | 週期性保活（30 秒） |

---

### `GET /protocol/version`

回傳支援的協定版本和伺服器版本：

```json
{
  "supported_versions": [1, 2],
  "latest": 2,
  "server": "go-on",
  "server_version": "1.6.0"
}
```

## 認證

健康端點通常無需認證即可供監控基礎設施存取。在生產部署中，通常透過網路策略、反向代理認證或專用 API 金鑰來限制存取。

## 後續步驟

- 設定 Prometheus 抓取 `/metrics` 端點。
- 使用 `/health` 和 `/health/ready` 端點設定編排器的健康檢查探針。
- 訂閱 `/v1/state/events` 取得即時狀態變更通知。
