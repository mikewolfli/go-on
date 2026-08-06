# 核心运行时 API

## 概述

核心运行时 API 涵盖运行时生命周期、健康检查、配置管理和维护操作。go-on 的主要编程接口是**基于 HTTP 的 JSON-RPC 2.0**（`POST /rpc`）；少量 HTTP GET 端点与 SSE 流式端点用于健康探针、指标抓取和对话流式传输。

完整且权威的 JSON-RPC 方法参考见 `docs/protocol-guide.md`。本页仅记录当前真实存在的端点；未列出的端点不应假定其存在。

## HTTP 端点

### GET 端点

| 路径 | 说明 |
|---|---|
| `/` | 根能力响应（协议、端点、版本） |
| `/health` | 服务器状态快照（见下文） |
| `/health/ready` | 就绪探针——就绪时返回 `200`，排空（draining）时返回 `503` |
| `/metrics` | Prometheus 文本格式指标 |
| `/protocol/version` | 支持的协议版本和服务器版本 |
| `/v1/models` | 列出可用模型（OpenAI 兼容） |
| `/v1/model` | `/v1/models` 的别名 |
| `/models` | `/v1/models` 的别名 |
| `/v1/responses` | 列出 OpenAI Responses API 载荷 |
| `/v1/responses/{id}` | 按 ID 获取响应（OpenAI Responses API） |
| `/v1/state/events` | 状态同步事件的 SSE 流 |

### POST 端点

| 路径 | 说明 |
|---|---|
| `/rpc` | JSON-RPC 2.0 方法分发（主要接口；`/` 也可接受） |
| `/chat` | 对话补全（ACP JSON-RPC 格式） |
| `/chat/stream` | 流式对话补全（SSE） |
| `/v1/chat/completions` | OpenAI 兼容对话补全 |
| `/chat/completions` | OpenAI 兼容对话补全 |
| `/v1/responses` | OpenAI Responses API |

> 下文所有 JSON-RPC 方法名均通过 `POST /rpc` 分发。完整方法参考见
> `docs/protocol-guide.md`。

## 健康检查

### GET /health

返回完整的服务器状态快照（`ServerStatus`）：请求指标、生命周期状态、熔断器快照、维护状态、治理状态和时间戳。

**响应：**

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

就绪探针。服务器可接受请求时返回 `200` 及 `{"ok": true, "status": "ready", "healthy": true}`；排空期间返回 `503` 及 `{"ok": false, "status": "draining", "message": "Server is shutting down"}`。

### JSON-RPC 健康方法

| 方法 | 说明 |
|---|---|
| `health` / `runtime.health` | 运行时健康快照 |
| `health.probes` | 模块级健康探针 |
| `health.check` | 运行完整健康检查；成功时返回 `{"ok": true}` |

示例：

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"runtime.health","params":{}}'
```

## 运行时信息

运行时内省通过 JSON-RPC 方法提供：

| 方法 | 说明 |
|---|---|
| `runtime.stability` | 运行时稳定性指标 |
| `runtime.features` | 已启用的运行时特性 |
| `runtime.self_model` | 自模型快照（稳定性、学习、知识） |
| `provider.status` | 已配置 AI 提供方就绪状态 |
| `provider.catalog` / `provider.list_models` | 提供方/模型目录 |
| `capabilities.list` | 服务器能力 |
| `selector.status` | 模型/工具选择状态 |
| `models.list` / `models/list` | 列出可用模型 |

## 配置管理

配置通过 JSON-RPC 管理，而非 REST：

| 方法 | 说明 |
|---|---|
| `config.reload` | 重新校验并从磁盘加载配置；在相关变更时发布状态同步事件（`ConfigReloaded`、`AgentsChanged`、`ModelsChanged`） |
| `config.baseline` | 生效配置基线与遗留键迁移报告 |
| `debug_panel.get` / `debug.panel.get` | 调试面板载荷 |

注意：`config.reload` 会立即应用运行时设置，但 agent/缓存/向量变更需要重启（响应包含警告数量和配置档位建议）。

示例：

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"config.reload","params":{}}'
```

## 生命周期

### JSON-RPC

| 方法 | 说明 |
|---|---|
| `initialize` | ACP 初始化握手 |
| `shutdown` | 优雅关闭 |
| `session/new`、`session/load`、`session/resume`、`session/close`、`session/list` | 会话生命周期 |
| `authenticate` / `logout` | 认证 |
| `mcp.initialize`、`mcp.ping` | MCP 握手 |

### 命令行

运行时生命周期也可以通过 CLI 驱动（见 `src/main/cli.rs`）：

```bash
go-on --setup                  # 运行设置向导（别名：--init）
go-on --setup-level standard   # quick | standard | custom
go-on --setup-profile PROFILE  # 使用的设置档位
go-on --status                 # 输出运行时就绪状态（别名：--check）
go-on --healthcheck            # 生成运行时健康检查报告到 .goon/
go-on --diagnose               # 运行端到端诊断并给出修复建议
go-on --validate-config        # 校验配置后退出（别名：--doctor）
go-on --config config.toml     # 显式指定配置文件（别名：-c）
go-on --secret --secret-name KEY --secret-value VALUE   # 密钥管理
go-on -b 127.0.0.1:8090        # 绑定 ACP HTTP 服务器（别名：--acp-http-bind / --bind）
go-on -m adaptive              # 协议模式覆盖（别名：--protocol-mode / --mode）
go-on -a                       # 启动交互式终端对话会话
```

子命令：`init`、`status`、`diagnose`、`skill`、`hub`（特性门控）。

## 维护操作

| 方法 | 说明 |
|---|---|
| `maintenance.gc` | 运行维护性垃圾回收 |
| `data.lifecycle` | 数据生命周期审查（重放序列、保留策略） |
| `cache.clear` | 清空缓存 |
| `vector.clear` | 清空向量存储 |
| `breaker.status` / `breaker.reset` / `breaker.recovery` | 熔断器管理 |
| `hardness.status` | 硬度（hardness）状态 |
| `lock.status` | ACP 锁状态 |

示例：

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"maintenance.gc","params":{}}'
```

## 流式传输

### POST /chat/stream

SSE 流式对话补全。服务器向连接写入 SSE 帧（`event: chunk`、`done`、`status`、`telemetry`、`tool_approval`、`error`），直到流结束。

```bash
curl -N http://localhost:8090/chat/stream \
  -H "Content-Type: application/json" \
  -d '{"messages":[{"role":"user","content":"Hello"}]}'
```

## 错误处理

JSON-RPC 响应使用标准的 JSON-RPC 2.0 错误对象：

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

标准错误码：

| 代码 | 含义 |
|---|---|
| `-32700` | 解析错误 |
| `-32600` | 无效请求 |
| `-32601` | 方法不存在 |
| `-32602` | 无效参数 |
| `-32603` | 内部错误 |

HTTP 状态码：`200 OK`、`400 Bad Request`、`401 Unauthorized`、`404 Not Found`、`405 Method Not Allowed`、`429 Too Many Requests`、`500 Internal Server Error`、`502 Bad Gateway`（上游错误）、`503 Service Unavailable`。

## 安全考虑

- 本地模式：API 密钥可选
- 服务器模式：API 密钥必需（通过 `X-Api-Key` / `X-Go-On-Key` 发送）
- RBAC：敏感操作（`shutdown`、`maintenance.gc`）需要管理员权限
- HTTP 处理器对每个请求强制执行入口守卫、认证和 RBAC

## 下一步

- 探索 [安全和治理 API](./safety-governance.md)
- 了解 [可观测性 API](./observability.md)
- 查看 [工作流和任务 API](./workflow-task.md)
