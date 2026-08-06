# 核心運行時 API

## 概述

核心運行時 API 涵蓋運行時生命週期、健康檢查、配置管理和維護操作。go-on 的主要程式化介面是**基於 HTTP 的 JSON-RPC 2.0**（`POST /rpc`）；少量 HTTP GET 端點與 SSE 串流端點用於健康探針、指標抓取和對話串流傳輸。

JSON-RPC 方法分發表在原始碼中（`src/acp/impl/request.rs`），方法白名單見 `src/acp/impl/request/protocol.rs`。`docs/protocol-guide.md` 僅介紹協議模式與編輯器整合。本頁僅記錄當前真實存在的端點；未列出的端點不應假定其存在。

## HTTP 端點

### GET 端點

| 路徑 | 說明 |
|---|---|
| `/` | 根能力回應（協定、端點、版本） |
| `/health` | 伺服器狀態快照（見下文） |
| `/health/ready` | 就緒探針——就緒時回傳 `200`，排空（draining）時回傳 `503` |
| `/metrics` | Prometheus 文字格式指標 |
| `/protocol/version` | 支援的協定版本和伺服器版本 |
| `/v1/models` | 列出可用模型（OpenAI 相容） |
| `/v1/model` | `/v1/models` 的別名 |
| `/models` | `/v1/models` 的別名 |
| `/v1/responses` | 列出 OpenAI Responses API 載荷 |
| `/v1/responses/{id}` | 依 ID 取得回應（OpenAI Responses API） |
| `/v1/state/events` | 狀態同步事件的 SSE 串流 |

### POST 端點

| 路徑 | 說明 |
|---|---|
| `/rpc` | JSON-RPC 2.0 方法分發（主要介面；`/` 也可接受） |
| `/chat` | 對話補全（ACP JSON-RPC 格式） |
| `/chat/stream` | 串流對話補全（SSE） |
| `/v1/chat/completions` | OpenAI 相容對話補全 |
| `/chat/completions` | OpenAI 相容對話補全 |
| `/v1/responses` | OpenAI Responses API |

> 下文所有 JSON-RPC 方法名稱均透過 `POST /rpc` 分發。完整方法分發表見 `src/acp/impl/request.rs`。

## 健康檢查

### GET /health

回傳完整的伺服器狀態快照（`ServerStatus`）：請求指標、生命週期狀態、斷路器快照、維護狀態、治理狀態和時間戳。

**回應：**

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

就緒探針。伺服器可接受請求時回傳 `200` 及 `{"ok": true, "status": "ready", "healthy": true}`；排空期間回傳 `503` 及 `{"ok": false, "status": "draining", "message": "Server is shutting down"}`。

### JSON-RPC 健康方法

| 方法 | 說明 |
|---|---|
| `health` / `runtime.health` | 運行時健康快照 |
| `health.probes` | 模組級健康探針 |
| `health.check` | 執行完整健康檢查；成功時回傳 `{"ok": true}` |

範例：

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"runtime.health","params":{}}'
```

## 運行時資訊

運行時內省透過 JSON-RPC 方法提供：

| 方法 | 說明 |
|---|---|
| `runtime.stability` | 運行時穩定性指標 |
| `runtime.features` | 已啟用的運行時特性 |
| `runtime.self_model` | 自模型快照（穩定性、學習、知識） |
| `provider.status` | 已設定 AI 提供方就緒狀態 |
| `provider.catalog` / `provider.list_models` | 提供方/模型目錄 |
| `capabilities.list` | 伺服器能力 |
| `selector.status` | 模型/工具選擇狀態 |
| `models.list` / `models/list` | 列出可用模型 |

## 配置管理

配置透過 JSON-RPC 管理，而非 REST：

| 方法 | 說明 |
|---|---|
| `config.reload` | 重新驗證並從磁碟載入配置；在相關變更時發布狀態同步事件（`ConfigReloaded`、`AgentsChanged`、`ModelsChanged`） |
| `config.baseline` | 生效配置基線與舊版鍵遷移報告 |
| `debug_panel.get` / `debug.panel.get` | 偵錯面板載荷 |

注意：`config.reload` 會立即套用運行時設定，但 agent/快取/向量變更需要重新啟動（回應包含警告數量與配置檔位建議）。

範例：

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"config.reload","params":{}}'
```

## 生命週期

### JSON-RPC

| 方法 | 說明 |
|---|---|
| `initialize` | ACP 初始化握手 |
| `shutdown` | 優雅關閉 |
| `session/new`、`session/load`、`session/resume`、`session/close`、`session/list` | 會話生命週期 |
| `authenticate` / `logout` | 認證 |
| `mcp.initialize`、`mcp.ping` | MCP 握手 |

### 命令列

運行時生命週期也可以透過 CLI 驅動（見 `src/main/cli.rs`）：

```bash
go-on --setup                  # 執行設定精靈（別名：--init）
go-on --setup-level standard   # quick | standard | custom
go-on --setup-profile PROFILE  # 使用的設定檔位
go-on --status                 # 輸出運行時就緒狀態（別名：--check）
go-on --healthcheck            # 產生運行時健康檢查報告到 .goon/
go-on --diagnose               # 執行端到端診斷並給出修復建議
go-on --validate-config        # 驗證配置後退出（別名：--doctor）
go-on --config config.toml     # 明確指定配置檔案（別名：-c）
go-on --secret --secret-name KEY --secret-value VALUE   # 金鑰管理
go-on -b 127.0.0.1:8090        # 綁定 ACP HTTP 伺服器（別名：--acp-http-bind / --bind）
go-on -m adaptive              # 協定模式覆蓋（別名：--protocol-mode / --mode）
go-on -a                       # 啟動互動式終端對話會話
```

子命令：`init`、`status`、`diagnose`、`skill`、`hub`（特性門控）。

## 維護操作

| 方法 | 說明 |
|---|---|
| `maintenance.gc` | 執行維護性垃圾回收 |
| `data.lifecycle` | 資料生命週期審查（重放序列、保留策略） |
| `cache.clear` | 清空快取 |
| `vector.clear` | 清空向量儲存 |
| `breaker.status` / `breaker.reset` / `breaker.recovery` | 斷路器管理 |
| `hardness.status` | 硬度（hardness）狀態 |
| `lock.status` | ACP 鎖狀態 |

範例：

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"maintenance.gc","params":{}}'
```

## 串流傳輸

### POST /chat/stream

SSE 串流對話補全。伺服器向連線寫入 SSE 幀（`event: chunk`、`done`、`status`、`telemetry`、`tool_approval`、`error`），直到串流結束。

```bash
curl -N http://localhost:8090/chat/stream \
  -H "Content-Type: application/json" \
  -d '{"messages":[{"role":"user","content":"Hello"}]}'
```

## 錯誤處理

JSON-RPC 回應使用標準的 JSON-RPC 2.0 錯誤物件：

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

標準錯誤碼：

| 代碼 | 含義 |
|---|---|
| `-32700` | 解析錯誤 |
| `-32600` | 無效請求 |
| `-32601` | 方法不存在 |
| `-32602` | 無效參數 |
| `-32603` | 內部錯誤 |

HTTP 狀態碼：`200 OK`、`400 Bad Request`、`401 Unauthorized`、`404 Not Found`、`405 Method Not Allowed`、`429 Too Many Requests`、`500 Internal Server Error`、`502 Bad Gateway`（上游錯誤）、`503 Service Unavailable`。

## 安全考量

- 本機模式：API 金鑰可選
- 伺服器模式：API 金鑰必需（透過 `X-Api-Key` / `X-Go-On-Key` 傳送）
- RBAC：敏感操作（`shutdown`、`maintenance.gc`）需要管理員權限
- HTTP 處理器對每個請求強制執行入口守衛、認證和 RBAC

## 下一步

- 探索 [安全和治理 API](./safety-governance.md)
- 了解 [可觀測性 API](./observability.md)
- 檢視 [工作流和任務 API](./workflow-task.md)
