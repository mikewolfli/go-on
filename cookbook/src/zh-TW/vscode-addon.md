# VS Code 插件

VS Code 插件是本倉庫裡功能最完整的編輯器接入面。它暴露了基於運行時的命令，可以探測運行時健康狀態，也允許在設置中覆蓋後端協議模式。

## 插件依賴什麼

插件需要：

- 可訪問的 `go-on` 可執行文件
- 有效的 `config.toml`
- 與當前工作流匹配的協議模式

插件清單當前暴露的協議覆蓋值為：

- `from_config`
- `adaptive`
- `acp_stdio`
- `acp_http`
- `mcp_stdio`
- `mcp_http`

其中 `from_config` 表示跟隨後端配置，其餘值表示顯式強制覆蓋。

## 首次接入建議

1. 先構建後端或準備好可執行文件。
2. 運行 `go-on --setup --setup-level standard`。
3. 如果自動發現不夠穩定，再在 VS Code 設置中顯式填寫可執行文件路徑和配置路徑。
4. 除非在排查特定傳輸問題，否則協議模式保持 `from_config`。

## 各協議模式什麼時候用

- `from_config`：日常默認。
- `adaptive`：希望一個運行時同時兼容多類探測時優先使用。
- `acp_stdio`：插件應驅動拉起 stdio 運行時時使用。
- `acp_http`：後端已作為共享本地 HTTP 服務運行時使用。
- `mcp_stdio`：只有明確需要 MCP stdio 才用。
- `mcp_http`：明確需要 `/v1` HTTP 語義時使用。

## 運行時健康面

插件契約中的健康檢查路徑是：

```text
/health
```

OpenAI 兼容探測路徑是：

```text
/v1/models
```

插件同時也知道這些路徑：

- `/v1/model`
- `/v1/chat/completions`
- `/v1/responses`

## 實用工作區設置示例

```json
{
  "go-on.runtime.protocolMode": "from_config",
  "go-on.runtime.executablePath": "D:/Workspace/RustWorkspace/go-on/target/debug/go-on.exe",
  "go-on.runtime.configPath": "D:/Workspace/RustWorkspace/go-on/config.toml"
}
```

如果要強制共享 HTTP 運行時：

```json
{
  "go-on.runtime.protocolMode": "acp_http"
}
```

## 實際排查順序

對插件來說，推薦按下面順序排查：

1. `go-on --validate-config`
2. `go-on --status`
3. 檢查 VS Code 設置裡的 executable path
4. 檢查 VS Code 設置裡的 config path
5. 最後再看是否需要協議模式覆蓋

## 什麼時候選 HTTP，什麼時候選 stdio

優先選 HTTP：

- 希望 GUI 與 VS Code 共享同一個後端
- 希望手工探測 `/health` 與 `/v1/models`
- 希望後端作為長駐本地服務存在

優先選 stdio：

- 希望 VS Code 自己管理進程啟停
- 希望不同工作區完全隔離

## 常見失敗模式

- 插件能拉起可執行文件，但提示 provider not ready，問題多半在配置或憑證，不在傳輸層。
- 選了 HTTP 模式但 `/health` 不通，說明後端並未用 `--acp-http-bind` 啟動。
- 強制 `mcp_http` 時，要確認當前消費該能力的插件路徑確實需要 `/v1` 語義，而不是 ACP 語義。

## 可用命令

插件在 VS Code 命令面板中註冊了以下命令：

**進程生命週期**

| 命令 | 說明 |
|---|---|
| `go-on.start` | 啟動 Go-On 後端進程 |
| `go-on.stop` | 停止運行中的後端進程 |
| `go-on.shutdown` | 優雅關閉後端 |
| `go-on.healthCheck` | 運行時健康檢查 |
| `go-on.healthProbes` | 查看所有健康探針詳情 |

**運行時診斷**

| 命令 | 說明 |
|---|---|
| `go-on.runtimeSelfModel` | 獲取統一自畫像視圖：運行健康、漂移摘要、約束畫像與建議動作 |
| `go-on.runtimeStability` | 獲取運行時穩定性快照 |
| `go-on.providerStatus` | 獲取 Provider 就緒狀態、降級摘要與 Agent 依賴快照 |
| `go-on.metricsGet` | 獲取當前運行時指標 |
| `go-on.metricsReset` | 重置運行時指標 |
| `go-on.traceMetrics` | 獲取 Trace 級指標 |
| `go-on.traceGet` | 獲取 Trace 條目 |
| `go-on.observabilityAlerts` | 查看可觀測性告警 |
| `go-on.releaseReadiness` | 檢查發佈就緒門禁 |

**治理與質量**

| 命令 | 說明 |
|---|---|
| `go-on.governanceStatus` | 獲取治理狀態 |
| `go-on.governancePlanGet` | 獲取當前治理計劃 |
| `go-on.governanceAuditRecent` | 查看最近審計條目 |
| `go-on.qualityBaseline` | 獲取質量基線快照 |
| `go-on.securityBaseline` | 獲取安全基線 |
| `go-on.rlAlignmentEval` | 運行 RL 對齊離線評估 |
| `go-on.hardnessStatus` | 獲取任務難度狀態 |
| `go-on.costStatus` | 獲取成本優化狀態 |
| `go-on.autotuneStatus` | 獲取自動調參狀態 |
| `go-on.autotuneGet` | 獲取自動調參參數 |
| `go-on.autotuneReset` | 重置自動調參參數 |
| `go-on.selectorStatus` | 獲取模型選擇器狀態 |

**工作流與任務**

| 命令 | 說明 |
|---|---|
| `go-on.workflowExecute` | 執行當前工作流 |
| `go-on.taskPlan` | 規劃任務 |
| `go-on.taskExecute` | 執行已規劃任務 |
| `go-on.harnessStatus` | 獲取測試套件狀態 |
| `go-on.primarySecondarySummary` | 獲取主從 Agent 摘要 |

**學習與優化**

| 命令 | 說明 |
|---|---|
| `go-on.learningSummary` | 獲取學習循環摘要 |
| `go-on.learningGuardrail` | 獲取學習防護狀態 |
| `go-on.learningReplay` | 重放學習數據 |
| `go-on.knowledgeDistill` | 運行知識蒸餾 |
| `go-on.optimizationPeak` | 獲取優化峰值狀態 |
| `go-on.buildRepro` | 運行構建可復現性檢查 |

**配置與運維**

| 命令 | 說明 |
|---|---|
| `go-on.configReload` | 重載運行時配置 |
| `go-on.configBaseline` | 獲取配置基線快照 |
| `go-on.lockStatus` | 獲取鎖狀態 |
| `go-on.breakerStatus` | 獲取熔斷器狀態 |
| `go-on.breakerReset` | 重置熔斷器 |
| `go-on.breakerRecovery` | 運行熔斷器恢復 |
| `go-on.cacheClear` | 清空 ACP 緩存 |
| `go-on.vectorClear` | 清空向量存儲 |
| `go-on.dataLifecycle` | 獲取數據生命週期狀態 |
| `go-on.errorContract` | 獲取錯誤契約摘要 |
| `go-on.checkpointCreate` | 創建運行時檢查點 |
| `go-on.checkpointList` | 列出可用檢查點 |
| `go-on.conversationRollback` | 回滾到某個檢查點 |
| `go-on.maintenanceGc` | 運行垃圾回收 |
| `go-on.actionCheck` | 檢查動作安全性 |
| `go-on.debugPanelGet` | 獲取調試面板數據 |

## 進程輸出通道

所有 Go-On 進程輸出（stdout、stderr、退出碼、進程錯誤）均寫入 VS Code 的 **"Go-On"** 輸出通道。通過 **查看 → 輸出** 打開，再從下拉菜單中選擇 **Go-On** 即可查看。這是啟動失敗和運行時錯誤排查的首選入口。