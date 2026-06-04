# 設置嚮導

設置嚮導由後端實現，是新機器或新工作目錄初始化的推薦入口。

## 會寫入什麼

setup 會以目標配置路徑為核心，寫出一組默認文件：

- `config.toml`
- 配置目錄附近的默認規則文件
- 供環境變量或 keyring 使用的 secret 引用

當前 setup 走的是 adaptive 模板族。

## 入口方式

交互式：

```bash
go-on --setup
```

偏非交互式：

```bash
go-on --setup --setup-profile adaptive --setup-level standard --setup-secrets auto
```

覆蓋已有文件：

```bash
go-on --setup --force
```

## setup profile

當前只接受：

- `adaptive`

這和當前架構一致，即一份配置儘量同時服務 ACP 與 MCP 風格前端。

## setup level

### `quick`

適合追求最快成功初始化。

實現特徵：

- 為了減少流程長度，會跳過額外 agent 提示

### `standard`

默認推薦。

適合大多數用戶，在引導性和可控性之間比較平衡。

### `custom`

適合想手動控制更多 Provider 與 agent 選擇的場景。

## secret 模式

### `env`

把 secret 繼續作為環境變量驅動。

適合已經有 shell、`.env`、CI 或進程管理器注入方案的場景。

### `keyring`

把 secret 存入操作系統 keyring，讓配置引用 keyring-backed 值。

適合桌面本地使用，希望減少明文暴露的場景。

### `auto`

自動選擇。

實現行為：

- 如果機器上已經有可用環境變量，setup 會優先走 `env`
- 否則 setup 會繼續詢問 secret 處理方式

## Provider 檢測行為

setup 會根據 secret 模式和當前機器狀態檢測可用 Provider。

流程大致是：

1. 檢測 Provider
2. 讓用戶選擇要啟用的 Provider
3. 應用 setup level 對應默認值
4. 生成 adaptive 配置
5. 如有需要，把 secret 寫入 keyring

如果最終沒有選中任何 Provider，setup 會直接失敗，而不是產出一個表面成功但不可運行的配置。

## keyring 行為

選擇 keyring 模式後，生成的配置會從環境變量佔位符轉換成 keyring 引用。

本倉庫當前使用的引用形式是：

```text
keyring://go-on/<account>
```

## setup 之外的 secret 管理

setup 不是唯一入口，後續也可以單獨用 CLI 管理：

```bash
go-on --secret list
go-on --secret set --secret-name openai --secret-value YOUR_KEY
```

這也是 setup 正常但憑證後續變更時最乾淨的修復路徑。

## 推薦初始化順序

對大多數使用者：

1. 運行 `go-on --setup --setup-level standard --setup-secrets auto`。
2. 運行 `go-on --status`。
3. 如果要讓 Zed 或 GUI 走 HTTP，使用 `--protocol-mode adaptive --acp-http-bind 127.0.0.1:8090` 啟動後端。
4. 如果是編輯器自拉起的 stdio 場景，再切到 `acp_stdio` 或 `mcp_stdio`。

## 什麼時候重跑 setup

以下場景建議重跑：

- 更換機器
- 更換 Provider 組合
- 從 env 模式切換到 keyring 模式
- `config.toml` 丟失或損壞

除非明確要替換現有文件集，否則不要輕易帶 `--force` 重跑。