# 优化和操作 API

## 概述

优化和操作 API 为 go-on 部署提供成本管理、性能优化、操作监控和系统调优。该 API 是**基于 HTTP 的 JSON-RPC 2.0**（`POST /rpc`）；这些能力没有专用的 REST 端点。

> 权威的 JSON-RPC 方法参考见 `docs/protocol-guide.md`。

## 方法

所有方法均通过 `POST /rpc` 分发：

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"cost.status","params":{}}'
```

### 成本优化

| 方法 | 说明 |
|---|---|
| `cost.status` | 成本状态 |
| `optimization.peak` | 优化峰值分析 |
| `observability.alerts` | 可观测性告警 |

### 性能与指标

| 方法 | 说明 |
|---|---|
| `metrics.get` | 结构化运行时指标 |
| `metrics` | 指标载荷 |
| `metrics.prometheus` | Prometheus 格式指标 |
| `metrics.window.query` | 查询指标窗口 |
| `metrics.errors.summary` | 错误摘要 |
| `metrics.reset` | 重置指标 |
| `runtime.stability` | 运行时稳定性指标 |
| `trace.get` / `trace.metrics` | 追踪检查 |
| `error.contract` | 错误契约载荷 |

### 操作

| 方法 | 说明 |
|---|---|
| `breaker.status` / `breaker.reset` / `breaker.recovery` | 熔断器管理 |
| `lock.status` | ACP 锁状态 |
| `maintenance.gc` | 维护性垃圾回收 |
| `data.lifecycle` | 数据生命周期审查 |
| `cache.clear` / `vector.clear` | 清空缓存 / 向量存储 |
| `autotune.get` / `autotune.status` / `autotune.reset` | 自动调优管理 |
| `release.readiness` | 发布就绪检查 |
| `harness.status` | Harness 状态（QA/可靠性维度） |
| `hardness.status` | 硬度状态 |

## 下一步

- 探索 [核心运行时 API](./core-runtime.md)
- 参见 [可观测性 API](./observability.md)
- 查看 [安全和治理 API](./safety-governance.md)
