# 工作流和任務 API

## 概述

工作流和任務 API 支持複雜工作流的編排、任務規劃、執行管理和結果追蹤。該 API 是**基於 HTTP 的 JSON-RPC 2.0**（`POST /rpc`）；這些能力沒有專用的 REST 端點。

> 權威的 JSON-RPC 方法參考見 `docs/protocol-guide.md`。

## 方法

所有方法均透過 `POST /rpc` 分發：

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"task.plan","params":{}}'
```

### 工作流

| 方法 | 說明 |
|---|---|
| `workflow.execute` | 執行工作流 |
| `workflow.generate` | 根據提示生成工作流 |
| `workflow.generate_from_chat` | 根據目前聊天上下文生成工作流 |
| `workflow.confirm` | 確認工作流步驟 |
| `workflow.clarify` | 在工作流期間請求澄清 |
| `workflow.research` | 執行研究步驟 |
| `workflow.consult` | 工作流執行期間諮詢 |
| `workflow.ask` | 工作流執行期間提問 |
| `workflow.run.list` | 列出工作流執行 |
| `workflow.run.get` | 依 ID 取得工作流執行 |
| `workflow.run.cancel` | 取消工作流執行 |
| `workflow.run.pause` | 暫停工作流執行 |
| `workflow.run.resume` | 恢復工作流執行 |

### 任務

| 方法 | 說明 |
|---|---|
| `task.plan` | 規劃任務（受控任務計畫產物） |
| `task.execute` | 執行任務 |
| `action.check` | 針對 `.goon/` 產物執行動作檢查（all/spec/qa/retest/final） |

## 認證

所有方法都需要具有適當權限的認證（按請求強制執行 RBAC）。

## 下一步

- 探索 [學習和智能 API](./learning-intelligence.md)
- 參見 [優化和操作 API](./optimization-ops.md)
- 檢視 [安全和治理 API](./safety-governance.md)
