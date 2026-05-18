# 工作流程配置

go-on 支援多種工作流程類型，適用於不同的使用場景。工作流程在 `config.toml` 的 `[flow]` 部分進行配置。

> 相關文件：[GUI 控制台](gui.md) | [Prompts 系統](prompts.md)

---

## 1. 工作流程類型

| 類型 | 值 | 用途 | 預設階段 |
|------|-----|------|----------|
| Auto | `"auto"` | 根據上下文自動偵測 | 因情況而異 |
| Dev | `"dev"` | 軟體開發 | planning → coding → review → delivery |
| General | `"general"` | 問答、分析、研究 | gathering → thinking → executing → validating → closing |
| Free | `"free"` | 單輪對話，無階段路由 | 無 |
| Custom | `"custom"` | 使用者自訂階段 | 使用者自訂 |

---

## 2. 快速開始

在 `config/templates/` 中提供了兩個即用的配置範本：

### 開發工作流程
```bash
cp config/templates/config.dev.toml config.toml
# 編輯 config.toml 新增您的 API 金鑰
go-on --config config.toml
```

### 通用工作流程
```bash
cp config/templates/config.general.toml config.toml
# 編輯 config.toml 新增您的 API 金鑰
go-on --config config.toml
```

---

## 3. 代理路由流程

**代理並不綁定到階段。** 所有註冊的 AI 提供者全域可用。**CapabilityBus** 根據以下條件動態為每個子任務選擇最佳提供者：

1. **任務上下文** — 正在執行的工作類型（編碼、分析等）
2. **信譽評分** — 類似任務的歷史成功率
3. **能力標籤** — 每個提供者擅長的領域

您可以在不列出代理的情況下定義階段——匯流排會自動選擇最佳代理：

```toml
[phases.coding]
description = "編碼 — 實現功能"
agents = []          # ← 留空！能力匯流排會自動選擇最佳代理。
fallback = true
```

或者透過列出首選代理來給匯流排提示：

```toml
[phases.coding]
agents = ["deepseek", "openai"]   # ← 提示：優先使用這些，但匯流排仍會自行決定
fallback = true
```

---

## 4. 自動偵測（`workflow_type = "auto"`）

當未設定 `workflow_type` 或設定為 `"auto"` 時，系統會在啟動時偵測上下文：

- 如果偵測到程式碼倉儲 → 使用 **Dev** 工作流程（4 個階段）
- 否則 → 使用 **General** 工作流程（5 個階段）

透過在配置中明確設定 `workflow_type` 來覆蓋預設行為。

---

## 5. 自由模式（`workflow_type = "free"`）

自由模式繞過階段路由。每次請求都是單輪互動，不進行階段切換：

```toml
[flow]
name = "自由聊天"
workflow_type = "free"
phases = []   # 無階段 — 僅直接路由
```

---

## 6. 自訂工作流程

定義您自己的工作流程階段和轉換：

```toml
default_phase = "research"
model_selection_mode = "adaptive"

[flow]
name = "我的研究工作流程"
workflow_type = "custom"
phases = ["research", "draft", "polish", "publish"]

[phases.research]
description = "研究 — 收集資料、分析數據"
agents = []
fallback = true

[phases.research.options]
request_timeout_seconds = 180
cache_enabled = true
vector_enabled = true
phase_max_inflight = 8
global_max_inflight = 128
```

每個階段都支援可配置選項，包括超時、快取、向量記憶、並發限制以及高風險多代理投票設定。

---

## 7. 高風險多代理投票

對於高風險階段（醫療、法律、金融、安全關鍵），go-on 支援多代理聯合處理：

1. **風險偵測** — 分析使用者訊息中的領域和決策關鍵字
2. **多代理執行** — 多個代理並行獨立生成回覆
3. **投票** — 回覆去重並排名；獲得共識者勝出
4. **升級處理** — 如未達成共識，由其他模型參與打破平局

透過 `options` 在每個階段進行配置：

```toml
[phases.review.options]
high_risk_vote_enabled = true
high_risk_vote_threshold = 1
high_risk_vote_min_agents = 2
high_risk_vote_max_agents = 4
high_risk_escalate_multi_model_enabled = true
```

---

> 完整的文件（包括階段選項參考、功能配置檔案和技能系統）可在專案 `docs/workflow-config.md` 中找到。
