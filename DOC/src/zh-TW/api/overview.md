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
- **協議**：HTTP、RPC
- **認證**：API 密鑰、JWT 令牌（多用戶模式）

### 3. 可觀測性 API
- **目的**：指標、追蹤、日誌、健康監控
- **協議**：HTTP、OpenTelemetry
- **認證**：API 密鑰（指標端點可能公開）

### 4. 可靠性 API
- **目的**：斷路器、重試、維護操作
- **協議**：HTTP、RPC
- **認證**：API 密鑰

### 5. 工作流和任務 API
- **目的**：工作流執行、任務規劃和管理
- **協議**：HTTP、RPC
- **認證**：API 密鑰、JWT 令牌

### 6. 學習和智能 API
- **目的**：機器學習、強化學習、自適應選擇
- **協議**：HTTP、RPC
- **認證**：API 密鑰

### 7. 優化和操作 API
- **目的**：成本優化、性能調優、操作指標
- **協議**：HTTP、RPC
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
- **認證**：Bearer 令牌或 API 密鑰頭

### RPC（遠程過程調用）
- **傳輸**：HTTP/2 帶 gRPC 或 JSON-RPC
- **序列化**：Protocol Buffers 或 JSON
- **特性**：雙向流、取消、截止時間

## 認證

### API 密鑰
```bash
# 本地模式（可選）
export GO_ON_ENTRY_API_KEY="your-api-key"

# 服務器模式（必需）
export GO_ON_SERVER_API_KEY="server-key"
export GO_ON_ENTRY_API_KEY="entry-key"
```

### JWT 令牌（多用戶模式）
```bash
# 獲取令牌
curl -X POST http://localhost:8090/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"user","password":"pass"}'

# 使用令牌
curl http://localhost:8090/api/v1/users/me \
  -H "Authorization: Bearer <jwt-token>"
```

### OAuth 2.0（企業版）
- **供應商**：Google、GitHub、Okta、Azure AD
- **範圍**：`read`、`write`、`admin`
- **流程**：授權碼、客戶端憑證

## 速率限制

### 默認限制
- **本地模式**：每分鐘 240 個請求，60 個突發
- **簡單服務器**：每分鐘 1000 個請求，200 個突發
- **多用戶服務器**：每分鐘 5000 個請求，1000 個突發

### 頭部
```
X-RateLimit-Limit: 1000
X-RateLimit-Remaining: 950
X-RateLimit-Reset: 1614556800
```

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

## 版本控制

### API 版本頭部
```http
Accept: application/vnd.go-on.v1+json
```

### URL 版本控制
```
http://localhost:8090/api/v1/health
http://localhost:8090/api/v2/health
```

### 棄用策略
- **警告**：棄用的端點返回 `X-API-Deprecated: true` 頭部
- **淘汰**：棄用 6 個月後移除
- **遷移**：文檔提供遷移指南

## 分頁

### 基於游標的分頁
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

### 限制和偏移
```
GET /api/v1/users?limit=20&offset=40
```

## 過濾和排序

### 過濾
```
GET /api/v1/logs?level=error&since=2024-01-01T00:00:00Z
```

### 排序
```
GET /api/v1/users?sort=created_at&order=desc
```

## 字段選擇

### 部分響應
```
GET /api/v1/users/123?fields=id,name,email
```

### 嵌套字段選擇
```
GET /api/v1/projects/456?fields=id,name,tasks(id,title,status)
```

## WebSocket 支持

### 實時更新
```javascript
const ws = new WebSocket('ws://localhost:8090/ws');
ws.onmessage = (event) => {
  const data = JSON.parse(event.data);
  console.log('更新:', data);
};
```

### 事件
- `workflow.completed`
- `task.updated`
- `error.occurred`
- `health.status_changed`

## OpenAPI 規範

### 訪問 OpenAPI 文檔
```
http://localhost:8090/docs
http://localhost:8090/openapi.json
http://localhost:8090/openapi.yaml
```

### 生成客戶端 SDK
```bash
# TypeScript
npx openapi-typescript http://localhost:8090/openapi.json --output client.ts

# Python
openapi-python-client generate --url http://localhost:8090/openapi.json

# Go
oapi-codegen -package api -generate types,client http://localhost:8090/openapi.json > api.gen.go
```

## 客戶端庫

### 官方庫
- **TypeScript/JavaScript**：`@go-on/client`
- **Python**：`go-on-client`
- **Go**：`github.com/your-org/go-on/client`
- **Rust**：`go-on-client` crate

### 社區庫
- **Java**：`go-on-java-client`
- **C#**：`GoOn.Client`
- **Ruby**：`go_on_ruby`

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
- **向量搜索**：< 2s

### 吞吐量
- **本地模式**：~100 請求/秒
- **簡單服務器**：~500 請求/秒
- **多用戶服務器**：~2000 請求/秒

## 安全

### TLS/SSL
```bash
# 生成自簽名證書
openssl req -x509 -newkey rsa:4096 -keyout key.pem -out cert.pem -days 365

# 在運行時中配置
[runtime]
tls_cert_path = "cert.pem"
tls_key_path = "key.pem"
```

### CORS 配置
```toml
[security.cors]
allowed_origins = ["https://example.com", "http://localhost:3000"]
allowed_methods = ["GET", "POST", "PUT", "DELETE", "OPTIONS"]
allowed_headers = ["Authorization", "Content-Type"]
allow_credentials = true
```

## 監控

### 健康檢查
```
GET /health
GET /health/ready
GET /health/live
```

### 指標
```
GET /metrics
GET /metrics/prometheus
```

### 追蹤
- **Jaeger**：`http://localhost:16686`
- **Zipkin**：`http://localhost:9411`
- **OpenTelemetry**：`http://localhost:4317`

## 下一步

探索特定 API 分組：
- [核心運行時 API](./core-runtime.md)
- [安全和治理 API](./safety-governance.md)
- [可觀測性 API](./observability.md)
- [工作流和任務 API](./workflow-task.md)
- [學習和智能 API](./learning-intelligence.md)
- [優化和操作 API](./optimization-ops.md)