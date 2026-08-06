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
- **协议**：HTTP、JSON-RPC
- **认证**：API 密钥

### 3. 可观测性 API
- **目的**：指标、追踪、日志、健康监控
- **协议**：HTTP、JSON-RPC
- **认证**：API 密钥（健康端点可能公开）

### 4. 可靠性 API
- **目的**：断路器、重试、维护操作
- **协议**：HTTP、JSON-RPC
- **认证**：API 密钥

### 5. 工作流和任务 API
- **目的**：工作流执行、任务规划和管理
- **协议**：HTTP、JSON-RPC
- **认证**：API 密钥

### 6. 学习和智能 API
- **目的**：机器学习、强化学习、自适应选择
- **协议**：HTTP、JSON-RPC
- **认证**：API 密钥

### 7. 优化和操作 API
- **目的**：成本优化、性能调优、操作指标
- **协议**：HTTP、JSON-RPC
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
- **认证**：Bearer 令牌或 API 密钥头（`X-Api-Key` 或 `X-Go-On-Key`）

### JSON-RPC over HTTP
- **端点**：`POST /rpc`
- **序列化**：JSON
- **特性**：通过方法路由的请求/响应（`runtime.health`、`governance.status` 等）

## 认证

### API 密钥
```bash
# 本地模式（可选）
export GO_ON_ENTRY_API_KEY="your-api-key"

# 服务器模式（必需）
export GO_ON_SERVER_API_KEY="server-key"
export GO_ON_ENTRY_API_KEY="entry-key"
```

API 密钥通过 `X-Api-Key` 或 `X-Go-On-Key` HTTP 头发送。认证根据运行时配置 `entry_auth_enabled` 强制执行。

## 速率限制

### 默认限制
- **本地模式**：每分钟 240 个请求，60 个突发
- **简单服务器**：每分钟 1000 个请求，200 个突发
- **多用户服务器**：每分钟 5000 个请求，1000 个突发

速率限制通过每阶段的令牌桶算法内部强制执行。

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

## HTTP 端点

### GET 端点

| 路径 | 描述 |
|---|---|
| `/` | 根能力响应（协议、端点、版本） |
| `/health` | 服务器状态快照（指标、生命周期、治理） |
| `/v1/models` | 列出可用模型（OpenAI 兼容） |
| `/v1/model` | `/v1/models` 的别名 |
| `/models` | `/v1/models` 的别名 |
| `/v1/responses/{id}` | 按 ID 获取响应 |

### POST 端点

| 路径 | 描述 |
|---|---|
| `/chat` | 聊天补全（ACP JSON-RPC 格式） |
| `/chat/stream` | 流式聊天补全（SSE） |
| `/v1/chat/completions` | OpenAI 兼容的聊天补全 |
| `/chat/completions` | OpenAI 兼容的聊天补全 |
| `/rpc` | JSON-RPC 2.0 方法分发（主要接口） |
| `/v1/responses` | OpenAI Responses API |

## 客户端库

### 官方库
- **Python**：`go-on-sdk`（通过 `pip install go-on-sdk` 安装）
- **Rust**：`go_on_sdk` crate（`sdk/rust/`）
- **Node.js**：`go-on-sdk-nodejs`（`sdk/nodejs/`）
- **TypeScript**：`go-on-sdk-typescript`（`sdk/typescript/`）

### 生成自定义客户端
`POST /rpc` 的 JSON-RPC 接口可以直接从任何语言调用：

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"runtime.health","params":{}}'
```

## 测试

### 模拟服务器
```bash
# 以本地 HTTP 运行时启动后端（ACP HTTP，端口 8090）
go-on --config config.toml --protocol-mode adaptive --acp-http-bind 127.0.0.1:8090

# 使用 curl 测试
curl http://127.0.0.1:8090/health
```

### 集成测试
```bash
# 运行结构集成测试套件
cargo test --test structural_tests

# 运行 ACP 运行时 RPC 集成测试
cargo test --test acp_runtime_rpc_integration
```

## 性能

### 响应时间
- **健康检查**：< 100ms
- **简单请求**：< 500ms
- **复杂工作流**：< 5s

### 吞吐量
- **本地模式**：~100 请求/秒
- **简单服务器**：~500 请求/秒
- **多用户服务器**：~2000 请求/秒

## 安全

### CORS 配置
CORS 通过 `config.toml` 中的 `runtime.cors_allowed_origins` 配置（空列表 = 禁用）：

```toml
[runtime]
cors_allowed_origins = ["https://example.com", "http://localhost:3000"]
```

## 监控

### 健康检查
```
GET /health
```

### OpenTelemetry 追踪
go-on 使用 OpenTelemetry 进行聊天补全、代理调用和审查门的内部追踪。追踪数据发送到任何已配置的 OTLP 收集器。

### Prometheus 指标
内部运行时指标（延迟直方图、断路器状态、速率限制器令牌）可通过 JSON-RPC 获取，Prometheus 文本格式则暴露在 `GET /metrics`：

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"metrics.prometheus","params":{}}'
```

## 下一步

探索特定 API 分组：
- [核心运行时 API](./core-runtime.md)
- [安全和治理 API](./safety-governance.md)
- [可观测性 API](./observability.md)
- [工作流和任务 API](./workflow-task.md)
- [学习和智能 API](./learning-intelligence.md)
- [优化和操作 API](./optimization-ops.md)
