# 架構總覽

`go-on` 是一個圍繞 Rust 後端構建的三端運行時體系：

- **後端**：負責配置加載、Provider 選擇、路由、setup、健康檢查、協議協商、stdio 或 HTTP 傳輸層，以及包含 14 條總線和認知模塊的能力架構。
- **GUI**：EGUI（Rust 原生）桌面圖形界面，負責後端發現、進程生命週期、集成探測、監控、對話和配置管理。
- **VS Code 插件**：負責拉起或探測運行時，暴露基於 RPC 的命令，並可在工作區級別覆蓋協議模式。

## 版本

- 后端 Runtime：**1.1.0**
- GUI 桌面端：**1.1.0**
- VS Code 插件：**1.1.0**

## GUI 桌面應用

基於 EGUI 的桌面圖形界面（`gui/`）提供監控、對話和配置管理：

```bash
cargo run --manifest-path gui/Cargo.toml
```

主要功能：
- **監控面板**：後端健康狀態、AI 供應商狀態、實時指標
- **對話界面**：多會話管理、階段選擇（coding/review/debug/test/deploy）、模式切換（Ask/Plan/Edit/Safeguard/Full Auto）、文件附件、動態發送按鈕（依據 AI 狀態變化）
- **技能管理**：創建和導入 AI 技能；內置 `skill-creator` 讓 AI 自主定義新技能
- **設置**：功能開關、語言切換（en/zh-CN/zh-TW）、5 種視覺主題
- **後端連接**：ACP+HTTP JSON-RPC，自動健康輪詢

## 構建配置文件

三種構建配置文件適配不同的部署場景，外加 `profile-full` 用於 CI：

| 配置文件 | 後端 | 使用場景 | 構建命令 |
|:--------|:------|:---------|:--------|
| `profile-local` | SQLite + sqlite-vec | 單用戶本地工具 | `cargo build`（默認） |
| `profile-simple-server` | SQLite + sqlite-vec | 單服務部署 | `cargo build --no-default-features -F profile-simple-server` |
| `profile-multi-users-server` | PostgreSQL + pgvector | 多用戶生產環境 | `cargo build --no-default-features -F profile-multi-users-server` |
| `profile-full` | SQLite（全部特性） | CI / 開發 | `cargo build --no-default-features -F profile-full` |

## 驗證狀態

| 配置文件 | `cargo clippy -D warnings` | 測試數 |
|:--------|:--------------------------:|:------:|
| **profile-local** | ✅ **零警告** | **4699** |
| **profile-simple-server** | ✅ **零警告** | **3400+** |
| **profile-full** | ✅ **零警告** | **4000+** |
| **profile-multi-users-server** | ✅ **零警告** | **3800+** |

所有 19 個測試二進制文件均可編譯通過。23 個 E2e 測試（需要基礎設施）標記為 `#[ignore]`，不會在本地運行中執行。

## 運行時協議模式

後端支持 5 種訪問模式：

- `adaptive`（推薦默認）：雙棧協議能力加按請求類型路由。
- `acp_stdio`：通過 stdio 提供 ACP，適合編輯器拉起子進程。
- `acp_http`：通過 HTTP 暴露 ACP 風格接口，適合共享長駐後端。
- `mcp_stdio`：通過 stdio 提供 MCP。
- `mcp_http`：通過 HTTP 暴露 MCP 與 OpenAI 兼容接口。

當以後端 `--acp-http-bind` 啟動時，默認會圍繞 `http://127.0.0.1:8090` 暴露實際可用的 HTTP 面：

- `/health`
- `/chat`
- `/chat/stream`
- `/v1/models`
- `/v1/model`
- `/v1/chat/completions`
- `/v1/responses`

這也是三端分工的關鍵：

- Zed 既可以走 ACP stdio，也可以走 ACP HTTP。
- Zed 也可以把後端當成 OpenAI 兼容的 `/v1` 模型提供方。
- VS Code 插件既可以走拉起式 stdio RPC，也可以探測 HTTP 運行時。
- GUI 依賴本地後端可執行文件，並要求工作目錄中存在 `config.toml`。

## 架構：多總線能力系統

go-on 實現了以 **CapabilityBus** 和 **HarnessBus** 為核心的 **14 條總線架構**。

### 核心總線

| 總線 | 模塊 | 說明 |
|:----|------|------|
| **CapabilityBus** | `src/intelligence/capability_bus/core.rs` | 中央智能總線，編排 sense/decide/evolve 生命週期 |
| **HarnessBus** | `src/governance/harness_bus.rs` | 治理入口，策略評估、漂移/彈性/安全檢查 |

### 子總線（Phase 4）

| 總線 | 模塊 | 說明 |
|:----|------|------|
| **ToolBus** | `capability_bus/tool_bus.rs` | 統一工具/Skill 調用，能力矩陣，Agent-工具匹配 |
| **ObservabilityBus** | `capability_bus/observability_bus.rs` | 統一可觀測：延遲、錯誤率、Agent 健康 |
| **OptimizationBus** | `capability_bus/optimization_bus.rs` | 成本/速度/可靠性推薦，熔斷器 |
| **MemoryBus** | `capability_bus/memory_bus.rs` | 級聯緩存（L1→L2→L3），向量存儲查找 |
| **ProtocolBus** | `capability_bus/protocol_bus.rs` | 協議感知路由，健康/延遲追蹤 |
| **OrchestrationBus** | `capability_bus/orchestration_bus.rs` | 流程/模式/路由編排，模式推薦 |
| **DistributedMemoryBus** | `capability_bus/distributed_memory_bus.rs` | 跨節點記憶共享（特性門控） |

### 總線生命週期

```
sense()   →  聚合 Agent 健康、可用模式、優化推薦
decide()  →  結合模式推薦與工具-Agent 匹配
evolve()  →  更新 Q 表、記錄共識投票、發送進化事件
execute_tool() → HarnessBus evaluate() → ToolBus execute() → ObservabilityBus record()
```

## 安全特性

| 功能 | 描述 |
|:----|:------|
| **mTLS** | ACP HTTP 監聽器的雙向 TLS，支援證書鎖定和過期監控 |
| **請求簽名** | 使用 Ed25519 或 HMAC-SHA256 對 JSON-RPC 請求進行簽名認證 |
| **Vault 集成** | 集成 HashiCorp Vault 進行金鑰生命週期管理 |
| **系統金鑰環** | macOS Keychain、Linux Secret Service、Windows Credential Manager |
| **內容安全** | 運行時內容掃描，可配置安全策略（SafeGuard 模式） |
| **提示注入檢測** | 運行時掃描注入模式，可配置檢測閾值 |

## 可觀測性

go-on 提供生產級的可觀測能力：

| 能力 | 詳情 |
|:-----|:-----|
| **Prometheus `/metrics` 端點** | 16+ 指標，包括延遲、吞吐量、緩存命中率 |
| **OpenTelemetry 追蹤** | OTLP 導出（默認端點 `localhost:4317`），路由/執行/選擇的跨度 |
| **治理狀態端點** | 通過 `governance.status` JSON-RPC 獲取即時 p95 延遲、DAG 指標、緩存統計 |
| **OTel stdout 導出器** | 當無 OTLP 收集器時可回退到標準輸出導出跟蹤 |

## 國際化（i18n）

go-on 在後端實現了約 **95%** 的全鏈路國際化覆蓋：

| 語言 | 文件 | 鍵值數 |
|:-----|:-----|:------:|
| 英語（美國） | `languages/en_US.json` | 448+ |
| 簡體中文 | `languages/zh_CN.json` | 448+ |
| 繁體中文 | `languages/zh_TW.json` | 448+ |

覆蓋層：ACP/MCP HTTP 錯誤（100%）、Agent 供應商模塊（100%，35 家供應商）、配置驗證（100%）、CLI 初始化（100%）、API 處理錯誤（100%）、編排層（100%）、GUI（約 98%）、VS Code 插件（70+ 鍵值）。

## 與架構對應的倉庫目錄

- `src/`：後端運行時、CLI、setup、ACP 與 MCP 實現。
  - `src/acp/`：ACP 服務、請求路由、workflow/task/chat/checkpoint
  - `src/agents/`：Provider 適配器（OpenAI、Anthropic、DeepSeek、Gemini、xAI Grok、SiliconFlow 等 35 家），AgentFactory
  - `src/core/`：配置、初始化、就緒性檢查、錯誤模型
  - `src/governance/`：策略/規則治理、審計、安全治理器、漂移防護
  - `src/intelligence/`：選擇器、強化學習、能力總線、發現、共識、演化
  - `src/orchestration/`：流程/模式/路由編排、腦回路、全能模式、製品層
  - `src/fault_tolerance.rs`：跨節點容錯引擎
  - `src/resilience/`：超彈性引擎
  - `src/protocol/`：協議服務、JSON-RPC、多渠道消息傳輸
  - `src/i18n/`：語言運行時
- `gui/`：EGUI（Rust 原生）桌面圖形界面
- `vscode-addon/`：VS Code 插件（支持 en_US、zh_CN、zh_TW 多語言）
- `config/`：配置文件
- `tests/`：集成測試與回放資產
- `scripts/`：質量/發佈門禁腳本

## 推薦運維路徑

新機器或新工作目錄，最短路徑通常是：

1. 構建或準備 `go-on` 後端可執行文件。
2. 運行 `go-on --setup --setup-level standard`。
3. 用 `go-on --status` 檢查運行時就緒狀態。
4. 如果前端要走 HTTP，使用 `--protocol-mode adaptive --acp-http-bind 127.0.0.1:8090` 啟動後端。
5. 再接入 Zed、VS Code 插件或 GUI。

後續章節分別展開說明。