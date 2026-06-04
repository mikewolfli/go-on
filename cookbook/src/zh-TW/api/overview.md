# API 概述

## 簡介

go-on 為智能體編排、治理和生產操作提供了全面的 API 接口。API 按邏輯分組組織，對應系統的不同方面。

## API 分組

### 1. 核心運行時 API
- **目的**：系統初始化、關閉和基本運行時操作
- **協議**：ACP over stdio/HTTP，MCP over stdio/HTTP
- **認證**：API 密鑰（本地模式可選，服務器模式必需）

### 2. 安全和治理 API
- **目的**：安全策略、審計日誌、合規性監控
- **協議**：HTTP、JSON-RPC
- **認證**：API 密鑰

### 3. 可觀測性 API
- **目的**：指標、追蹤、日誌、健康監控
- **協議**：HTTP、JSON-RPC
- **認證**：API 密鑰（健康端點可能公開）

### 4. 可靠性 API
- **目的**：斷路器、重試、維護操作
- **協議**：HTTP、JSON-RPC
- **認證**：API 密鑰

### 5. 工作流和任務 API
- **目的**：工作流執行、任務規劃和管理
- **協議**：HTTP、JSON-RPC
- **認證**：API 密鑰

### 6. 學習和智能 API
- **目的**：機器學習、強化學習、自適應選擇
- **協議**：HTTP、JSON-RPC
- **認證**：API 密鑰

### 7. 優化和操作 API
- **目的**：成本優化、性能調優、操作指標
- **協議**：HTTP、JSON-RPC
- **認證**：API 密鑰

## 協議支持

### ACP（智能體協調協議）
- **Over stdio**：用於編輯器集成（Zed、VS Code）
- **Over HTTP**：用於遠程訪問和服務器部署
- **特性**：雙向流、請求/響應、通知

### MCP（模型上下文協議）
- **Over stdio**：用於模型供應商集成
- **Over HTTP**：用於基於 Web 的集成
- **特性**：工具調用、上下文管理、流式傳輸

### HTTP REST API
- **基礎 URL**：`http://localhost:8090`（默認）
- **Content-Type**：`application/json`
- **認證**：Bearer 令牌或 API 密鑰頭（`X-Api-Key` 或 `X-Go-On-Key`）

### JSON-RPC over HTTP
- **端點**：`POST /v1/responses`
- **序列化**：JSON
- **特性**：通過方法路由的請求/響應（`runtime.health`、`governance.status` 等）

## 認證

### API 密鑰
```bash
# 本地模式（可選）
export GO_ON_ENTRY_API_KEY="your-api-key"

# 服務器模式（必需）
export GO_ON_SERVER_API_KEY="server-key"
export GO_ON_ENTRY_API_KEY="entry-key"
```

API 密鑰通過 `X-Api-Key` 或 `X-Go-On-Key` HTTP 頭髮送。認證根據運行時配置 `entry_auth_enabled` 強制執行。

## 速率限制

### 默認限制
- **本地模式**：每分鐘 240 個請求，60 個突發
- **簡單服務器**：每分鐘 1000 個請求，200 個突發
- **多用戶服務器**：每分鐘 5000 個請求，1000 個突發

速率限制通過每階段的令牌桶算法內部強制執行。

## 錯誤處理

### HTTP 狀態碼
- `200 OK`：成功
- `400 Bad Request`：無效輸入
- `401 Unauthorized`：需要認證
- `403 Forbidden`：權限不足
- `404 Not Found`：資源未找到
- `429 Too Many Requests`：超出速率限制
- `500 Internal Server Error`：服務器錯誤
- `503 Service Unavailable`：服務暫時不可用

### 錯誤響應格式
```json
{
  "error": {
    "code": "RATE_LIMIT_EXCEEDED",
    "message": "超出速率限制。請稍後重試。",
    "details": {
      "limit": 1000,
      "remaining": 0,
      "reset_at": "2024-01-01T00:00:00Z"
    },
    "request_id": "req_1234567890abcdef"
  }
}
```

## HTTP 端點

### GET 端點

| 路徑 | 描述 |
|---|---|
| `/` | 根能力響應（協議、端點、版本） |
| `/health` | 健康檢查（狀態、版本、運行時間） |
| `/v1/models` | 列出可用模型（OpenAI 兼容） |
| `/v1/model` | `/v1/models` 的別名 |
| `/models` | `/v1/models` 的別名 |
| `/v1/responses/{id}` | 按 ID 獲取響應 |

### POST 端點

| 路徑 | 描述 |
|---|---|
| `/chat` | 聊天補全（ACP JSON-RPC 格式） |
| `/chat/stream` | 流式聊天補全（SSE） |
| `/v1/chat/completions` | OpenAI 兼容的聊天補全 |
| `/chat/completions` | OpenAI 兼容的聊天補全 |
| `/v1/responses` | JSON-RPC 2.0 方法分發 |

## 客戶端庫

### 官方庫
- **Python**：`go-on-sdk`（通過 `pip install go-on-sdk` 安裝）
- **Rust**：`go-on-client` crate

### 生成自定義客戶端
`POST /v1/responses` 的 JSON-RPC 接口可以直接從任何語言調用：

```bash
curl http://localhost:8090/v1/responses \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"runtime.health","params":{}}'
```

## 測試

### 模擬服務器
```bash
# 啟動模擬服務器
go-on --mock --port 8080

# 使用 curl 測試
curl http://localhost:8080/health
```

### 集成測試
```bash
# 運行 API 測試
cargo test --test api

# 運行特定測試組
cargo test --test api_health
```

## 性能

### 響應時間
- **健康檢查**：< 100ms
- **簡單請求**：< 500ms
- **複雜工作流**：< 5s

### 吞吐量
- **本地模式**：~100 請求/秒
- **簡單服務器**：~500 請求/秒
- **多用戶服務器**：~2000 請求/秒

## 安全

### CORS 配置
```toml
[security.cors]
allowed_origins = ["https://example.com", "http://localhost:3000"]
allowed_methods = ["GET", "POST", "PUT", "DELETE", "OPTIONS"]
allowed_headers = ["Authorization", "Content-Type", "X-Api-Key", "X-Go-On-Key"]
allow_credentials = true
```

## 監控

### 健康檢查
```
GET /health
```

### OpenTelemetry 追蹤
go-on 使用 OpenTelemetry 進行聊天補全、代理調用和審查門的內部追蹤。追蹤數據發送到任何已配置的 OTLP 收集器。

### Prometheus 指標
內部運行時指標（延遲直方圖、斷路器狀態、速率限制器令牌）可通過 JSON-RPC 獲取：

```bash
curl http://localhost:8090/v1/responses \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"metrics.prometheus","params":{}}'
```

## 下一步

探索特定 API 分組：
- [核心運行時 API](./core-runtime.md)
- [安全和治理 API](./safety-governance.md)
- [可觀測性 API](./observability.md)
- [工作流和任務 API](./workflow-task.md)
- [學習和智能 API](./learning-intelligence.md)
- [優化和操作 API](./optimization-ops.md)
