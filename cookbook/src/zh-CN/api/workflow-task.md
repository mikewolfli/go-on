# 工作流和任务 API

## 概述

工作流和任务 API 支持复杂工作流的编排、任务规划、执行管理和结果跟踪。该 API 是**基于 HTTP 的 JSON-RPC 2.0**（`POST /rpc`）；这些能力没有专用的 REST 端点。

> 权威的 JSON-RPC 方法参考见 `docs/protocol-guide.md`。

## 方法

所有方法均通过 `POST /rpc` 分发：

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"task.plan","params":{}}'
```

### 工作流

| 方法 | 说明 |
|---|---|
| `workflow.execute` | 执行工作流 |
| `workflow.generate` | 根据提示生成工作流 |
| `workflow.generate_from_chat` | 根据当前聊天上下文生成工作流 |
| `workflow.confirm` | 确认工作流步骤 |
| `workflow.clarify` | 在工作流期间请求澄清 |
| `workflow.research` | 运行研究步骤 |
| `workflow.consult` | 工作流执行期间咨询 |
| `workflow.ask` | 工作流执行期间提问 |
| `workflow.run.list` | 列出工作流运行 |
| `workflow.run.get` | 按 ID 获取工作流运行 |
| `workflow.run.cancel` | 取消工作流运行 |
| `workflow.run.pause` | 暂停工作流运行 |
| `workflow.run.resume` | 恢复工作流运行 |

### 任务

| 方法 | 说明 |
|---|---|
| `task.plan` | 规划任务（受控任务计划产物） |
| `task.execute` | 执行任务 |
| `action.check` | 针对 `.goon/` 产物运行动作检查（all/spec/qa/retest/final） |

## 认证

所有方法都需要具有适当权限的认证（按请求强制执行 RBAC）。

## 下一步

- 探索 [学习和智能 API](./learning-intelligence.md)
- 参见 [优化和操作 API](./optimization-ops.md)
- 查看 [安全和治理 API](./safety-governance.md)
