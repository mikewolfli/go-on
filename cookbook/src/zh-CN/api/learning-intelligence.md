# 学习和智能 API

## 概述

学习和智能 API 暴露 go-on 的机器学习、强化学习、自适应选择和知识蒸馏能力。该 API 是**基于 HTTP 的 JSON-RPC 2.0**（`POST /rpc`）；这些能力没有专用的 REST 端点。

> 权威的 JSON-RPC 方法参考见 `docs/protocol-guide.md`。

## 方法

所有方法均通过 `POST /rpc` 分发：

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"learning.summary","params":{}}'
```

### 学习与知识

| 方法 | 说明 |
|---|---|
| `learning.summary` | 任务窗口的学习档案摘要 |
| `learning.replay` | 学习重放档案 |
| `learning.guardrail` | 学习护栏摘要（window/limit 参数） |
| `knowledge.distill` | 知识蒸馏（基于证据权重的提取，写回 `learning.summary` / `knowledge.distill`） |

### 强化学习与自适应选择

| 方法 | 说明 |
|---|---|
| `rl.alignment.offline_eval` | RL 对齐的离线评估 |
| `selector.status` | 模型/工具选择器状态 |
| `phase.policy.replay` | 阶段策略重放 |
| `primary_secondary.summary` | 主/次摘要（别名：`summary/primary_secondary`） |
| `optimization.peak` | 优化峰值分析 |
| `cost.status` | 成本状态 |

### 辅助智能

| 方法 | 说明 |
|---|---|
| `harness.status` | 带学习/RL 档案集成的 harness 状态 |
| `capabilities.list` | 服务器能力列表 |
| `models.list` / `models/list` | 可用模型 |

## 认证

所有方法都需要具有适当权限的认证（按请求强制执行 RBAC）。

## 下一步

- 探索 [安全和治理 API](./safety-governance.md)
- 查看 [工作流和任务 API](./workflow-task.md)
- 参见 [优化和操作 API](./optimization-ops.md)
