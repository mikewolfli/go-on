# 架構總覽

`go-on` 是一個圍繞 Rust 後端構建的三端運行時體系：

- **後端**：負責配置加載、Provider 選擇、路由、setup、健康檢查、協議協商、stdio 或 HTTP 傳輸層，以及包含 14 條總線和 21 個 F-GAP 模塊的能力架構。
- **GUI**：Tauri 桌面控制台，負責後端發現、進程生命週期、集成探測和本地運維。
- **VS Code 插件**：負責拉起或探測運行時，暴露基於 RPC 的命令，並可在工作區級別覆蓋協議模式。

## 版本

- 後端 Runtime：**0.8.4**
- GUI 桌面端：**0.8.4**
- VS Code 插件：**0.8.4**

## 構建配置文件

三種構建配置文件適配不同的部署場景：

| 配置文件 | 後端 | 目標 | 構建命令 |
|---------|------|------|----------|
| `profile-local` | SQLite + sqlite-vec | 單用戶本地工具 | `cargo build`（默認） |
| `profile-simple-server` | SQLite + sqlite-vec | 單服務部署 | `cargo build --no-default-features -F profile-simple-server` |
| `profile-multi-users-server` | PostgreSQL + pgvector | 多用戶生產環境 | `cargo build --no-default-features -F profile-multi-users-server` |

## 驗證狀態（Phase 4 完成）

| 配置文件 | `cargo check` | `cargo clippy -D warnings` | `cargo test` |
|---------|:-----------:|:------------------------:|:----------:|
| **profile-local** | ✅ 0 errors, 0 warnings | ✅ 0 errors | ✅ **866 通過**（766 單元 + 86 RPC + 14 transport） |
| **profile-simple-server** | ✅ 0 errors, 0 warnings | ✅ 0 errors | ✅ **905 通過** |
| **profile-multi-users-server** | ✅ 0 errors, 0 warnings | ✅ 0 errors | ✅ **898 通過** |

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

## F-GAP 模塊（Phase 4 — 21/21 全部完成 ✅）

go-on 實現了 21 個 FutureDesign 模塊，分佈在六個能力領域：

### 編排與執行（F-GAP-09, 10, 15, 17）
- **OmnipotentMode 全能模式**（F-GAP-09）：EscalationToken 頒發/驗證/吊銷、RAII 會話守衛、審計日誌
- **ArtifactLayer 製品層**（F-GAP-10）：製品模式註冊、存儲、TTL 裁剪
- **RemoteSkill 遠程技能**（F-GAP-10）：遠程 MCP 端點包裝為 Skill trait
- **OrchestrationCouncil 編排委員會**（F-GAP-15）：多 Agent 協調委員會
- **BrainLoop 腦回路**（F-GAP-17）：Plan→Execute→Reflect→Replan 全循環

### 智能與學習（F-GAP-11, 12, 16, 18, 19, 21, 22, 23, 24, 25）
- **DiscoveryCenter 方案發現中心**（F-GAP-11）：解決方案模式註冊與搜索
- **ScenarioMatcher 場景匹配器**（F-GAP-12）：多維度場景匹配
- **ConsensusEngine 共識引擎**（F-GAP-16）：分佈式投票與共識
- **EvolutionGraph 演化圖譜**（F-GAP-18）：6 階段能力演化生命週期
- **FederatedRL 聯邦強化學習**（F-GAP-19）：FedAvg/FedWeighted/FedMedian 聚合
- **SelfModelCore 自模型核心**（F-GAP-21）：自我能力評估與置信度
- **MetacognitiveController 元認知控制器**（F-GAP-22）：6 階段思維鏈、卡頓檢測
- **WorldModel 世界模型**（F-GAP-23）：世界模型流水線
- **ContinuousLearningCenter 持續學習中心**（F-GAP-24）：持續學習編排
- **ConsciousnessMetrics 意識代理指標**（F-GAP-25）：5 維度意識度量

### 治理與安全（F-GAP-14, 26）
- **SecurityGovernor 安全治理器**（F-GAP-14）：安全策略治理
- **DriftProtection 漂移防護**（F-GAP-26）：5 種漂移類型、4 級嚴重度、趨勢檢測

### 彈性與容錯（F-GAP-27, 28）
- **HyperResilienceEngine 超彈性引擎**（F-GAP-27）：熔斷器、故障切換、自愈
- **FaultToleranceEngine 跨節點容錯引擎**（F-GAP-28）：節點心跳、隔離、自動恢復、集群健康評分

### 協議與傳輸（F-GAP-29）
- **MultiChannelTransport 多渠道消息傳輸**（F-GAP-29）：6 通道、4 級優先級、QoS、去重、Peek

### Agent 基礎設施（F-GAP-13）
- **AgentFactory Agent 工廠**（F-GAP-13）：特性門控的 Agent 實例化

## 38 維度滿星評級

```
治理與合規 (5/5):    ★★★★★ 溯源賬本, 漂移防護, 策略評估器, Token 門控鏈, 安全治理器
彈性與容錯 (2/2):    ★★★★★ 超彈性引擎, 跨節點容錯引擎
編排與執行 (6/6):    ★★★★★ 編排總線, 任務調度器, 執行圖, 全能模式, 製品層, 腦回路
路由與調度 (7/7):    ★★★★★ 能力圖譜, 信譽存儲, Q學習Agent, 場景匹配器, 發現中心, 工作流注冊表, Agent工廠
協議與傳輸 (2/2):    ★★★★★ 協議總線, 多渠道消息傳輸
記憶與緩存 (2/2):    ★★★★★ 內存總線, 分佈式內存總線
觀測與優化 (3/3):    ★★★★★ 可觀測總線, 優化總線, 工具總線
智能認知 (5/5):      ★★★★★ 深度知識萃取, 強化深度學習, 技能保持傳承, AI進化, 自建Skills
自我認知 (5/5):      ★★★★★ 自模型核心, 意識代理指標, 元認知控制器, 世界模型, 共識引擎
總計 (38/38):        100% ★★★★★
```

## 整體完成率

```
Phase 0: 核心雙總線           ████████████████████ 100%
Phase 1: 子總線接入            ████████████████████ 100%
Phase 2: 剩餘修復              ████████████████████ 100%
Phase 3: ARCH 擴展點           ████████████████████ 100%
Phase 4: FutureDesign (F-GAP)  ████████████████████ 100% (21/21)
Phase 5: 生產硬化              ████████████████████ 100%
────────────────────────────────────────────────────────
總體:                         ████████████████████ 100%
```

## 國際化（i18n）

go-on 在後端實現了約 **95%** 的全鏈路國際化覆蓋：

| 語言 | 文件 | 鍵值數 |
|:-----|:-----|:------:|
| 英語（美國） | `languages/en_US.json` | 372+ |
| 簡體中文 | `languages/zh_CN.json` | 372+ |
| 繁體中文 | `languages/zh_TW.json` | 372+ |

覆蓋層：ACP/MCP HTTP 錯誤（100%）、Agent 供應商模塊（100%）、配置驗證（100%）、CLI 初始化（100%）、API 處理錯誤（100%）、編排層（100%）、GUI（約 98%）、VS Code 插件（70+ 鍵值）。

## 與架構對應的倉庫目錄

- `src/`：後端運行時、CLI、setup、ACP 與 MCP 實現。
  - `src/acp/`：ACP 服務、請求路由、workflow/task/chat/checkpoint
  - `src/agents/`：Provider 適配器（OpenAI、Anthropic、DeepSeek、Ollama），AgentFactory
  - `src/core/`：配置、初始化、就緒性檢查、錯誤模型
  - `src/governance/`：策略/規則治理、審計、安全治理器、漂移防護
  - `src/intelligence/`：選擇器、強化學習、能力總線、發現、共識、演化
  - `src/orchestration/`：流程/模式/路由編排、腦回路、全能模式、製品層
  - `src/fault_tolerance.rs`：跨節點容錯引擎
  - `src/resilience/`：超彈性引擎
  - `src/protocol/`：協議服務、JSON-RPC、多渠道消息傳輸
  - `src/i18n/`：語言運行時
- `GUI/`：Tauri 桌面控制台
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