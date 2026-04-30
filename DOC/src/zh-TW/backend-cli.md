# 後端 CLI

後端可執行文件是當前系統的權威控制面，負責運行時啟動、setup、健康檢查、任務規劃以及協議模式選擇。

## 調用方式

生產或打包後的二進制：

```bash
go-on --config config.toml
```

開發階段：

```bash
cargo run -- --config config.toml
```

當前幫助入口形式為：

```text
Usage: go-on.exe [OPTIONS]
```

沒有子命令，所有操作由標誌驅動。

## 核心運行時選項

### `--config <CONFIG>`

指定顯式配置文件路徑。如果省略，運行時將從可執行文件所在目錄解析 `config.toml`。

示例：

```bash
go-on --config D:\go-on\config.toml
```

### `--phase <PHASE>`

選擇要運行的特定階段（phase）配置文件。當你的配置定義了多個階段行為，並希望使用一個確定的入口點時使用。

### `--verbose`

啟用詳細日誌輸出。在診斷啟動、配置、傳輸或 Provider 就緒問題時首先使用此選項。

## Phase 與 Sub-Phase 配置

Phase 定義運行時執行的工作流階段。每個 phase 可以包含可選的 sub-phase，實現更精細的控制。

### 在 config.toml 中配置 phase

Phase 在 `[phases.<name>]` 節中配置，由 `flow.phases` 列表引用：

```toml
[flow]
# 執行順序。根據你的工作流增刪 phase。
phases = ["think", "act", "check", "done"]

[phases.think]
description = "Think — 分析、規劃、收集上下文"
# 分配給此 phase 的 agent（空 = setup wizard 會提示填寫）
agents = []
# 為 true 時，即使沒有配置 agent 也繼續執行
fallback = true

[phases.think.options]
request_timeout_seconds = 120
review_timeout_seconds = 60
cache_enabled = true
vector_enabled = true
summary_enabled = true
phase_max_inflight = 8      # 此 phase 內最大併發任務數
global_max_inflight = 128    # 所有 phase 全局最大併發任務數
```

### Sub-phases（子階段）

Sub-phases 提供分層工作流分解。一個 phase 可以定義 `sub_phases` 列表，配合嵌套的 `[phases.<parent>.<child>]` 節：

```toml
[flow]
phases = ["think", "act", "check", "done"]

[phases.act]
description = "主執行階段"
agents = []
fallback = true
# Sub-phases 在此 phase 內按順序執行
sub_phases = ["plan", "code", "test"]

[phases.act.options]
request_timeout_seconds = 300
cache_enabled = true
phase_max_inflight = 24

[phases.act.plan]
description = "實現計劃"
agents = []
fallback = true

[phases.act.plan.options]
request_timeout_seconds = 120
phase_max_inflight = 4

[phases.act.code]
description = "編寫代碼"
agents = []
fallback = true

[phases.act.code.options]
request_timeout_seconds = 180
phase_max_inflight = 12

[phases.act.test]
description = "運行測試"
agents = []
fallback = true

[phases.act.test.options]
request_timeout_seconds = 120
phase_max_inflight = 8
```

Sub-phases 會繼承父級的 `options` 作為默認值，可在每個 sub-phase 中覆蓋。

### Phase-only 與 sub-phase 執行的區別

- **無 sub-phases**：每個 phase 按 `phases` 列表順序從上到下依次執行。
- **有 sub-phases**：父 phase 先編排其 sub-phases 按順序執行，完成後才進入下一個父 phase。
- Sub-phases 是可選的——大多數工作流使用扁平 phase 即可。

### 內置 phase 預設文件

項目內置四個預設配置文件，各有不同的 phase 設置：

| 文件 | Phases | 適用場景 |
|------|--------|----------|
| `config.toml` | think, act, check, done | 通用工作流（默認） |
| `config.coding.toml` | coding | IDE 集成（Zed、VS Code） |
| `config.simple-server.toml` | think, act, check, done | 單服務部署 |
| `config.multi-users-server.toml` | think, act, check, done | 多用戶企業環境 |

### 使用特定 phase 配置

```bash
# 使用編碼階段配置與 IDE 配合
go-on --config config.coding.toml --phase coding

# 使用通用配置配合 HTTP 端點
go-on --config config.toml --protocol-mode adaptive --acp-http-bind 127.0.0.1:8090
```

### 創建自定義 phase

你可以定義任意 phase 名稱——沒有內置限制：

```toml
[flow]
phases = ["research", "draft", "review", "approve", "publish"]

[phases.research]
description = "收集信息和資料"
agents = []
fallback = true

[phases.research.options]
request_timeout_seconds = 180
cache_enabled = true
vector_enabled = true
summary_enabled = true
phase_max_inflight = 4
```

### 每個 phase 的關鍵選項

| 選項 | 默認值 | 說明 |
|--------|---------|------|
| `request_timeout_seconds` | 150 | 此 phase 中單個任務請求的最大時間 |
| `review_timeout_seconds` | 60 | 此 phase 中審查的最大時間 |
| `review_timeout_policy` | `"reject"` | 審查超時時的處理方式（`"reject"` 或 `"warn"`） |
| `review_min_response_chars` | 12 | 審查回覆的最小字符數 |
| `cache_enabled` | true | 在此 phase 中啟用緩存查找 |
| `vector_enabled` | true | 在此 phase 中啟用向量存儲查找 |
| `summary_enabled` | true | 啟用對話摘要 |
| `phase_max_inflight` | 24 | 此 phase 內最大併發任務數 |
| `global_max_inflight` | 128 | 所有 phase 全局最大併發任務數 |
| `autopilot_complexity` | `"auto"` | 複雜度模式：`"auto"`、`"simple"`、`"complex"` |

## 驗證與就緒檢查

### `--validate-config` 或 `--doctor`

驗證配置並退出。在排查更大的運行時問題之前，這是最快的快速檢查。

```bash
go-on --config config.toml --validate-config
```

### `--status` 或 `--check`

打印已配置的 AI Provider 和運行時就緒狀態。

在 setup 之後、編輯 `config.toml` 之後或附加編輯器客戶端之前使用。

```bash
go-on --status
```

### `--healthcheck`

生成運行時健康報告並持久化到 `.goon/` 下。當需要持久化的工件用於後續審查或分類時使用。

```bash
go-on --healthcheck
```

## Setup 與推薦工作流

### `--setup` 或 `--init`

運行交互式設置嚮導。

```bash
go-on --setup
```

### `--setup-profile <SETUP_PROFILE>`

當前接受的值：`adaptive`。

示例：

```bash
go-on --setup --setup-profile adaptive
```

### `--setup-level <SETUP_LEVEL>`

接受的值：

- `quick`
- `standard`
- `custom`

實用指導：

- `quick`：最小路徑，跳過額外的 Agent 提示。
- `standard`：大多數用戶的最佳默認值。
- `custom`：暴露更多手動決策。

### `--setup-secrets <SETUP_SECRETS>`

接受的值：

- `env`
- `keyring`
- `auto`

`auto` 也接受 `autodetect`。

### `--apply-recommended`

將 Provider 能力推薦應用到當前 `config.toml` 並退出。

在接入新 Provider 或更改模型組合後使用。

### `--force`

即使目標文件已存在也強制運行 setup。

謹慎使用，尤其是當你精心維護了一個手寫的 `config.toml` 時。

## 本地模型註冊

### `--add-local-model` 或 `--add-model`

在配置中添加或更新本地模型 Agent 條目。

此標誌通常與下面的 `--local-model-*` 選項組合使用。

### `--local-model-name <NAME>`

邏輯 Agent 名稱。

### `--local-model-url <URL>`

本地 Provider 的端點 URL。

### `--local-model-type <TYPE>`

Provider 類型。默認意圖為 `openai`。

### `--local-model-model <MODEL_ID>`

要存儲在配置中的模型標識符。

### `--local-model-api-key-env <ENV_NAME>`

可選的 API 密鑰環境變量字段。

### `--local-model-secret-key-env <ENV_NAME>`

可選的密鑰環境變量字段。

### `--local-model-register-only`

僅在 `[agents]` 下注冊本地模型，而不自動附加到 phase agent 列表。

示例：

```bash
go-on --add-local-model \
  --local-model-name ollama-local \
  --local-model-url http://127.0.0.1:11434/v1 \
  --local-model-type openai \
  --local-model-model qwen2.5-coder \
  --local-model-register-only
```

## Secret 管理

### `--secret <ACTION>`

接受的動作：

- `set`
- `get`
- `delete`
- `list`

### `--secret-name <SECRET_NAME>`

邏輯 Secret 目標的名稱。

### `--secret-value <SECRET_VALUE>`

與 `set` 一起使用的 Secret 值。

示例：

```bash
go-on --secret list
go-on --secret set --secret-name openai --secret-value YOUR_KEY
go-on --secret get --secret-name openai
go-on --secret delete --secret-name openai
```

## 規劃與製品檢查

### `--action-check <ACTION_CHECK>`

針對 `.goon/` 製品運行操作檢查。

幫助中描述的預期值：

- `all`
- `spec`
- `qa`
- `retest`
- `final`

### `--plan-task <PLAN_TASK>`

為複雜任務構建並持久化一個受控的任務規劃製品。

當你希望運行時在執行前物化一個持久的規劃對象時使用。

## 傳輸模式選擇

### `--protocol-mode <MODE>`

接受的值：

- `adaptive`（推薦默認）
- `acp_stdio`
- `acp_http`
- `mcp_stdio`
- `mcp_http`

推薦用法：

- `adaptive`：當多個客戶端可能連接時的最安全默認值；它保留雙棧請求路由並從運行時前提條件推導啟動傳輸。
- `acp_stdio`：當編輯器將 `go-on` 作為子進程啟動時的最佳選擇。
- `acp_http`：當 ACP 兼容客戶端需要一個共享的長時間運行後端時的最佳選擇。
- `mcp_stdio`：僅當你的客戶端明確期望 MCP over stdio 時使用。
- `mcp_http`：當你的客戶端需要 OpenAI 兼容的 `/v1` HTTP 端點時的最佳選擇。

### `--acp-http-bind <ADDR>`

綁定 HTTP 監聽器並暴露：

- `/health`
- `/chat`
- `/chat/stream`

實踐中，同一運行時也會暴露 OpenAI 兼容的 `/v1` 端點，用於 Zed 模型提供方風格的集成和運行時探測。

示例：

```bash
go-on --config config.toml --protocol-mode adaptive --acp-http-bind 127.0.0.1:8090
```

## 常用命令配方

最小化 setup：

```bash
go-on --setup --setup-level standard --setup-secrets auto
```

驗證然後檢查就緒狀態：

```bash
go-on --config config.toml --validate-config
go-on --config config.toml --status
```

為 GUI、Zed 和探測啟動共享的本地 HTTP 運行時：

```bash
go-on --config config.toml --protocol-mode adaptive --acp-http-bind 127.0.0.1:8090
```

為編輯器啟動的集成運行 ACP over stdio：

```bash
go-on --config config.toml --protocol-mode acp_stdio --verbose
```

## 操作指導

- 在假設傳輸層故障之前，先使用 `--validate-config`。
- 在打開 GUI 或編輯器插件之前，先使用 `--status`。
- 除非你有具體的客戶端契約要求僅 ACP 或僅 MCP 行為，否則使用 `adaptive`。
- 在接入本地 OpenAI 兼容端點時，優先使用 `--add-local-model` 而不是手動編輯配置。