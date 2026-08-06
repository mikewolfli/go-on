# 安全和治理 API

## 概述

安全和治理 API 為 go-on 部署提供安全策略執行、稽核追蹤維護、合規性監控和存取控制。該 API 是**基於 HTTP 的 JSON-RPC 2.0**（`POST /rpc`）；這些能力沒有專用的 REST 端點。

> 權威的 JSON-RPC 方法參考見 `docs/protocol-guide.md`。

## 方法

所有方法均透過 `POST /rpc` 分發：

```bash
curl http://localhost:8090/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"governance.status","params":{}}'
```

### 治理

| 方法 | 說明 |
|---|---|
| `governance.status` | 治理狀態（HarnessBus 檔案、策略、門控） |
| `governance.plan.get` | 取得治理計畫 |
| `governance.plan.update` | 更新治理計畫 |
| `governance.audit.recent` | 最近的稽核日誌條目 |
| `governance.audit.verify` | 驗證防篡改稽核雜湊鏈 |
| `governance.remediate` | 執行治理修復 |
| `governance.config.save` | 儲存治理配置 |

### 安全

| 方法 | 說明 |
|---|---|
| `security.baseline` | 安全基線與風險報告 |
| `harness.status` | HarnessBus 狀態（策略、漂移、韌性、稽核維度） |
| `tool.approve` | 批准工具執行（參數：`tool_name`） |

### 存取控制

認證和 RBAC 按請求強制執行：

- `authenticate` — 認證會話
- `logout` — 結束會話
- RBAC 將每個方法對應到權限層級（`Admin`、`ManageUsers`、`ManageConfig`、`Read`、`Execute`）；敏感方法（`shutdown`、`maintenance.gc`）需要管理員權限

## 稽核追蹤

配置變更和維護操作會記錄到稽核日誌中，稽核雜湊鏈可透過 `governance.audit.verify` 驗證。

## 下一步

- 探索 [核心運行時 API](./core-runtime.md)
- 參見 [優化和操作 API](./optimization-ops.md)
- 檢視 [可觀測性 API](./observability.md)
