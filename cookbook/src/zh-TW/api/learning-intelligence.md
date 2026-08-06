# 學習和智能 API

## 概述

學習和智能 API 暴露 go-on 的機器學習、強化學習、自適應選擇和知識蒸餾能力。該 API 是**基於 HTTP 的 JSON-RPC 2.0**（`POST /rpc`）；這些能力沒有專用的 REST 端點。

> JSON-RPC 方法分發表在 `src/acp/impl/request.rs`；方法白名單見 `src/acp/impl/request/protocol.rs`。`docs/protocol-guide.md` 僅介紹協議模式。

## 方法

所有方法均透過 `POST /rpc` 分發：

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"learning.summary","params":{}}'
```

### 學習與知識

| 方法 | 說明 |
|---|---|
| `learning.summary` | 任務視窗的學習檔案摘要 |
| `learning.replay` | 學習重放檔案 |
| `learning.guardrail` | 學習護欄摘要（window/limit 參數） |
| `knowledge.distill` | 知識蒸餾（基於證據權重的提取，寫回 `learning.summary` / `knowledge.distill`） |

### 強化學習與自適應選擇

| 方法 | 說明 |
|---|---|
| `rl.alignment.offline_eval` | RL 對齊的離線評估 |
| `selector.status` | 模型/工具選擇器狀態 |
| `phase.policy.replay` | 階段策略重放 |
| `primary_secondary.summary` | 主/次摘要（別名：`summary/primary_secondary`） |
| `optimization.peak` | 優化峰值分析 |
| `cost.status` | 成本狀態 |

### 輔助智能

| 方法 | 說明 |
|---|---|
| `harness.status` | 帶學習/RL 檔案整合的 harness 狀態 |
| `capabilities.list` | 伺服器能力列表 |
| `models.list` / `models/list` | 可用模型 |

## 認證

所有方法都需要具有適當權限的認證（按請求強制執行 RBAC）。

## 下一步

- 探索 [安全和治理 API](./safety-governance.md)
- 檢視 [工作流和任務 API](./workflow-task.md)
- 參見 [優化和操作 API](./optimization-ops.md)
