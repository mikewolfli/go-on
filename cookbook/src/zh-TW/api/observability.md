# 可觀測性 API

可觀測性 API 提供健康檢查、Prometheus 指標、OpenTelemetry 追蹤匯出和結構化日誌匯出的端點。這些端點供監控基礎設施、儀表板和告警系統使用。

## 端點

### `GET /health` — 健康檢查

返回 go-on 實例的當前健康狀態，包括版本、運行時間和各模組的健康資訊。

**HTTP 方法：** `GET`

**回應格式：** `application/json`

**範例回應：**

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

**欄位說明：**

| 欄位 | 類型 | 描述 |
|---|---|---|
| `status` | string | 整體健康狀態：`"ok"`、`"degraded"` 或 `"down"` |
| `version` | string | 當前運行版本的 SemVer |
| `uptime_seconds` | integer | 行程啟動以來的秒數 |
| `modules` | object | 各模組健康狀態；每個模組報告 `"ok"`、`"degraded"` 或 `"down"` |

---

### `GET /metrics` — Prometheus 指標

以 Prometheus 文字格式暴露應用和系統指標。供 Prometheus 伺服器抓取使用。

**HTTP 方法：** `GET`

**回應格式：** `text/plain; version=0.0.4`

**範例回應：**

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

**常用指標族：**

| 指標 | 類型 | 描述 |
|---|---|---|
| `go_on_requests_total` | Counter | 按方法、路徑和狀態碼統計的 HTTP 請求總數 |
| `go_on_request_duration_seconds` | Histogram | 請求延遲分佈 |
| `go_on_active_connections` | Gauge | 當前活躍的 TCP/WebSocket 連線數 |
| `go_on_memory_bytes` | Gauge | RSS 記憶體使用量（位元組） |
| `go_on_cpu_seconds_total` | Counter | 累計 CPU 時間 |

---

### `GET /traces` — OpenTelemetry 追蹤匯出

透過 OTLP HTTP 協定接收 OpenTelemetry 追蹤資料。供 OpenTelemetry Collector、Jaeger 或 Grafana Tempo 等收集器使用。

**HTTP 方法：** `GET`（健康/協定協商）或 `POST`（追蹤資料匯出）

**請求格式：** `application/x-protobuf`（OTLP protobuf）或 `application/json`

**回應格式：** `application/json`

**範例（POST 請求）：**

```http
POST /traces HTTP/1.1
Content-Type: application/x-protobuf
```

**範例回應：**

```json
{
  "partialSuccess": {
    "rejectedSpans": 0,
    "rejectedSpanCount": 0
  }
}
```

**請求頭：**

| 請求頭 | 描述 |
|---|---|
| `Content-Type` | `application/x-protobuf` 或 `application/json` |
| `Content-Encoding` | 可選：`gzip` 用於壓縮內容 |

---

### `GET /logs` — 結構化日誌匯出

接收和暴露結構化日誌記錄。支援即時串流和批次匯出。

**HTTP 方法：** `GET`（查詢日誌） / `POST`（寫入日誌）

**回應格式：** `application/json`

**範例 GET 請求：**

```http
GET /logs?level=error&since=3600&limit=50 HTTP/1.1
```

**範例回應：**

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

**查詢參數：**

| 參數 | 類型 | 描述 |
|---|---|---|
| `level` | string | 按日誌級別過濾：`trace`、`debug`、`info`、`warn`、`error` |
| `since` | integer | 返回最近 N 秒內的日誌 |
| `until` | string | ISO 8601 時間戳；返回該時間之前的日誌 |
| `limit` | integer | 最大返回條目數（預設 100，最大 1000） |
| `target` | string | 按 Rust 模組目標過濾（如 `go_on::server`） |

## 認證

可觀測性端點通常無需認證即可被監控基礎設施存取。在生產部署中，通常透過網路策略、反向代理認證或專用可觀測性 API 金鑰來限制存取。

## 後續步驟

- 設定 Prometheus 抓取 `/metrics` 端點。
- 部署 OpenTelemetry Collector 將追蹤轉發到後端。
- 使用 `/health` 端點設定編排器的健康檢查探針。
