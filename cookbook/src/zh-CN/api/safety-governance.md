# 安全和治理 API

## 概述

安全和治理 API 为 go-on 部署提供安全策略执行、审计追踪维护、合规性监控和访问控制。该 API 是**基于 HTTP 的 JSON-RPC 2.0**（`POST /rpc`）；这些能力没有专用的 REST 端点。

> JSON-RPC 方法分发表在 `src/acp/impl/request.rs`；方法白名单见 `src/acp/impl/request/protocol.rs`。`docs/protocol-guide.md` 仅介绍协议模式。

## 方法

所有方法均通过 `POST /rpc` 分发：

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"governance.status","params":{}}'
```

### 治理

| 方法 | 说明 |
|---|---|
| `governance.status` | 治理状态（HarnessBus 档案、策略、门控） |
| `governance.plan.get` | 获取治理计划 |
| `governance.plan.update` | 更新治理计划 |
| `governance.audit.recent` | 最近的审计日志条目 |
| `governance.audit.verify` | 验证防篡改审计哈希链 |
| `governance.remediate` | 运行治理修复 |
| `governance.config.save` | 保存治理配置 |

### 安全

| 方法 | 说明 |
|---|---|
| `security.baseline` | 安全基线与风险报告 |
| `harness.status` | HarnessBus 状态（策略、漂移、弹性、审计维度） |
| `tool.approve` | 批准工具执行（参数：`tool_name`） |

### 访问控制

认证和 RBAC 按请求强制执行：

- `authenticate` — 认证会话
- `logout` — 结束会话
- RBAC 将每个方法映射到权限级别（`Admin`、`ManageUsers`、`ManageConfig`、`Read`、`Execute`）；敏感方法（`shutdown`、`maintenance.gc`）需要管理员权限

## 审计追踪

配置更改和维护操作会记录到审计日志中，审计哈希链可通过 `governance.audit.verify` 验证。

## 下一步

- 探索 [核心运行时 API](./core-runtime.md)
- 参见 [优化和操作 API](./optimization-ops.md)
- 查看 [可观测性 API](./observability.md)
