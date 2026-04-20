# API 概述

## 简介

go-on 为智能体编排、治理和生产操作提供了全面的 API 接口。API 按逻辑分组组织，对应系统的不同方面。

## API 分组

### 1. 核心运行时 API
- **目的**：系统初始化、关闭和基本运行时操作
- **协议**：ACP over stdio/HTTP，MCP over stdio/HTTP
- **认证**：API 密钥（本地模式可选，服务器模式必需）

### 2. 安全和治理 API
- **目的**：安全策略、审计日志、合规性监控
- **协议**：HTTP、RPC
- **认证**：API 密钥、JWT 令牌（多用户模式）

### 3. 可观测性 API
- **目的**：指标、追踪、日志、健康监控
- **协议**：HTTP、OpenTelemetry
- **认证**：API 密钥（指标端点可能公开）

### 4. 可靠性 API
- **目的**：断路器、重试、维护操作
- **协议**：HTTP、RPC
- **认证**：API 密钥

### 5. 工作流和任务 API
- **目的**：工作流执行、任务规划和管理
- **协议**：HTTP、RPC
- **认证**：API 密钥、JWT 令牌

### 6. 学习和智能 API
- **目的**：机器学习、强化学习、自适应选择
- **协议**：HTTP、RPC
- **认证**：API 密钥

### 7. 优化和操作 API
- **目的**：成本优化、性能调优、操作指标
- **协议**：HTTP、RPC
- **认证**：API 密钥

## 协议支持

### ACP（智能体协调协议）
- **Over stdio**：用于编辑器集成（Zed、VS Code）
- **Over HTTP**：用于远程访问和服务器部署
- **特性**：双向流、请求/响应、通知

### MCP（模型上下文协议）
- **Over stdio**：用于模型供应商集成
- **Over HTTP**：用于基于 Web 的集成
- **特性**：工具调用、上下文管理、流式传输

### HTTP REST API
- **基础 URL**：`http://localhost:8090`（默认）
- **Content-Type**：`application/json`
- **认证**：Bearer 令牌或 API 密钥头

### RPC（远程过程调用）
- **传输**：HTTP/2 带 gRPC 或 JSON-RPC
- **序列化**：Protocol Buffers 或 JSON
- **特性**：双向流、取消、截止时间

## 认证

### API 密钥
```bash
# 本地模式（可选）
export GO_ON_ENTRY_API_KEY="your-api-key"

# 服务器模式（必需）
export GO_ON_SERVER_API_KEY="server-key"
export GO_ON_ENTRY_API_KEY="entry-key"
```

### JWT 令牌（多用户模式）
```bash
# 获取令牌
curl -X POST http://localhost:8090/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"user","password":"pass"}'

# 使用令牌
curl http://localhost:8090/api/v1/users/me \
  -H "Authorization: Bearer <jwt-token>"
```

### OAuth 2.0（企业版）
- **供应商**：Google、GitHub、Okta、Azure AD
- **范围**：`read`、`write`、`admin`
- **流程**：授权码、客户端凭证

## 速率限制

### 默认限制
- **本地模式**：每分钟 240 个请求，60 个突发
- **简单服务器**：每分钟 1000 个请求，200 个突发
- **多用户服务器**：每分钟 5000 个请求，1000 个突发

### 头部
```
X-RateLimit-Limit: 1000
X-RateLimit-Remaining: 950
X-RateLimit-Reset: 1614556800
```

## 错误处理

### HTTP 状态码
- `200 OK`：成功
- `400 Bad Request`：无效输入
- `401 Unauthorized`：需要认证
- `403 Forbidden`：权限不足
- `404 Not Found`：资源未找到
- `429 Too Many Requests`：超出速率限制
- `500 Internal Server Error`：服务器错误
- `503 Service Unavailable`：服务暂时不可用

### 错误响应格式
```json
{
  "error": {
    "code": "RATE_LIMIT_EXCEEDED",
    "message": "超出速率限制。请稍后重试。",
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

### API 版本头部
```http
Accept: application/vnd.go-on.v1+json
```

### URL 版本控制
```
http://localhost:8090/api/v1/health
http://localhost:8090/api/v2/health
```

### 弃用策略
- **警告**：弃用的端点返回 `X-API-Deprecated: true` 头部
- **淘汰**：弃用 6 个月后移除
- **迁移**：文档提供迁移指南

## 分页

### 基于游标的分页
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

## 过滤和排序

### 过滤
```
GET /api/v1/logs?level=error&since=2024-01-01T00:00:00Z
```

### 排序
```
GET /api/v1/users?sort=created_at&order=desc
```

## 字段选择

### 部分响应
```
GET /api/v1/users/123?fields=id,name,email
```

### 嵌套字段选择
```
GET /api/v1/projects/456?fields=id,name,tasks(id,title,status)
```

## WebSocket 支持

### 实时更新
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

## OpenAPI 规范

### 访问 OpenAPI 文档
```
http://localhost:8090/docs
http://localhost:8090/openapi.json
http://localhost:8090/openapi.yaml
```

### 生成客户端 SDK
```bash
# TypeScript
npx openapi-typescript http://localhost:8090/openapi.json --output client.ts

# Python
openapi-python-client generate --url http://localhost:8090/openapi.json

# Go
oapi-codegen -package api -generate types,client http://localhost:8090/openapi.json > api.gen.go
```

## 客户端库

### 官方库
- **TypeScript/JavaScript**：`@go-on/client`
- **Python**：`go-on-client`
- **Go**：`github.com/your-org/go-on/client`
- **Rust**：`go-on-client` crate

### 社区库
- **Java**：`go-on-java-client`
- **C#**：`GoOn.Client`
- **Ruby**：`go_on_ruby`

## 测试

### 模拟服务器
```bash
# 启动模拟服务器
go-on --mock --port 8080

# 使用 curl 测试
curl http://localhost:8080/health
```

### 集成测试
```bash
# 运行 API 测试
cargo test --test api

# 运行特定测试组
cargo test --test api_health
```

## 性能

### 响应时间
- **健康检查**：< 100ms
- **简单请求**：< 500ms
- **复杂工作流**：< 5s
- **向量搜索**：< 2s

### 吞吐量
- **本地模式**：~100 请求/秒
- **简单服务器**：~500 请求/秒
- **多用户服务器**：~2000 请求/秒

## 安全

### TLS/SSL
```bash
# 生成自签名证书
openssl req -x509 -newkey rsa:4096 -keyout key.pem -out cert.pem -days 365

# 在运行时中配置
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

## 监控

### 健康检查
```
GET /health
GET /health/ready
GET /health/live
```

### 指标
```
GET /metrics
GET /metrics/prometheus
```

### 追踪
- **Jaeger**：`http://localhost:16686`
- **Zipkin**：`http://localhost:9411`
- **OpenTelemetry**：`http://localhost:4317`

## 下一步

探索特定 API 分组：
- [核心运行时 API](./core-runtime.md)
- [安全和治理 API](./safety-governance.md)
- [可观测性 API](./observability.md)
- [工作流和任务 API](./workflow-task.md)
- [学习和智能 API](./learning-intelligence.md)
- [优化和操作 API](./optimization-ops.md)