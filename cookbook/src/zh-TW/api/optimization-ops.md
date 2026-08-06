# 優化和操作 API

## 概述

優化和操作 API 為 go-on 部署提供成本管理、效能最佳化、操作監控和系統調校。該 API 是**基於 HTTP 的 JSON-RPC 2.0**（`POST /rpc`）；這些能力沒有專用的 REST 端點。

> 權威的 JSON-RPC 方法參考見 `docs/protocol-guide.md`。

## 方法

所有方法均透過 `POST /rpc` 分發：

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"cost.status","params":{}}'
```

### 成本最佳化

| 方法 | 說明 |
|---|---|
| `cost.status` | 成本狀態 |
| `optimization.peak` | 最佳化峰值分析 |
| `observability.alerts` | 可觀測性警示 |

### 效能與指標

| 方法 | 說明 |
|---|---|
| `metrics.get` | 結構化運行時指標 |
| `metrics` | 指標載荷 |
| `metrics.prometheus` | Prometheus 格式指標 |
| `metrics.window.query` | 查詢指標視窗 |
| `metrics.errors.summary` | 錯誤摘要 |
| `metrics.reset` | 重置指標 |
| `runtime.stability` | 運行時穩定性指標 |
| `trace.get` / `trace.metrics` | 追蹤檢查 |
| `error.contract` | 錯誤契約載荷 |

### 操作

| 方法 | 說明 |
|---|---|
| `breaker.status` / `breaker.reset` / `breaker.recovery` | 斷路器管理 |
| `lock.status` | ACP 鎖狀態 |
| `maintenance.gc` | 維護性垃圾回收 |
| `data.lifecycle` | 資料生命週期審查 |
| `cache.clear` / `vector.clear` | 清空快取 / 向量儲存 |
| `autotune.get` / `autotune.status` / `autotune.reset` | 自動調校管理 |
| `release.readiness` | 發布就緒檢查 |
| `harness.status` | Harness 狀態（QA/可靠性維度） |
| `hardness.status` | 硬度狀態 |

## 下一步

- 探索 [核心運行時 API](./core-runtime.md)
- 參見 [可觀測性 API](./observability.md)
- 檢視 [安全和治理 API](./safety-governance.md)
