# Prompts 系統

## 概述

Prompts 系統是 go-on 內建的提示詞範本管理功能，提供 **149 個即用範本**，涵蓋 16 個類別（以 `prompts/en.json` 為準）。使用者可透過 GUI 瀏覽、搜尋和插入範本，在 Chat 中使用 `/` 指令快速展開範本，或透過 MCP（Model Context Protocol）讓 AI 代理自動呼叫範本。

> 相關文件：[GUI 控制台](gui.md) | [工作流程配置](workflow-config.md)

---

## 目錄結構

提示詞範本按分層目錄結構組織：

```
prompts/
├── en.json           # 英文內建範本（預設）
├── zh-CN.json        # 簡體中文內建範本
├── zh-TW.json        # 繁體中文內建範本
└── custom/
    ├── en.json       # 英文自訂範本
    ├── zh-CN.json    # 簡體中文自訂範本
    └── zh-TW.json    # 繁體中文自訂範本
```

- `prompts/{lang}.json` — 內建範本，隨系統發佈，唯讀
- `prompts/custom/{lang}.json` — 自訂範本，使用者可自由建立、編輯和刪除
- 語言自動切換：當 GUI 語言改變時，系統自動載入對應語言的範本檔案

### 支援的語言

| 語言代碼 | 名稱 |
|----------|------|
| `en` | English |
| `zh-CN` | 簡體中文 |
| `zh-TW` | 繁體中文 |

切換 GUI 語言時，提示詞範本系統會自動載入對應的語言檔案，無需手動切換。

---

## 16 個類別

範本按類別組織，各類數量不一，合計 **149 個範本**：

| # | 類別 | 範本數 | 代表範本 |
|---|------|--------|----------|
| 1 | 軟體開發 | 10 | `explain_code` 解釋程式碼 / `code_review` 程式碼審查 / `generate_unit_test` 生成單元測試 |
| 2 | 寫作與創意 | 12 | `blog_post_outline` 部落格大綱 / `proofread_text` 校對 / `creative_story` 創意故事 |
| 3 | 學術研究 | 8 | `literature_review` 文獻綜述 / `abstract_generation` 摘要生成 / `peer_review` 同儕評審 |
| 4 | 商業分析 | 9 | `swot_analysis` SWOT / `business_plan` 商業計畫 / `competitive_analysis` 競爭分析 |
| 5 | 市場行銷 | 13 | `marketing_strategy` 行銷策略 / `ad_copy` 廣告文案 / `social_media_content` 社媒內容 |
| 6 | 法律與合規 | 11 | `contract_review` 合約審查 / `contract_clause` 合約條款 / `compliance_checklist` 合規清單 |
| 7 | 醫療與健康 | 8 | `symptom_analysis` 症狀分析 / `medication_guide` 用藥說明 / `treatment_plan` 治療方案 |
| 8 | 教育與培訓 | 10 | `lesson_plan` 教案 / `quiz_generation` 測驗生成 / `explain_concept` 概念講解 |
| 9 | 金融與投資 | 11 | `investment_analysis` 投資分析 / `budget_planning` 預算規劃 / `financial_report` 財務報告 |
| 10 | 資料科學 | 11 | `eda_plan` 探索性分析 / `model_selection` 模型選擇 / `sql_query` SQL 查詢 |
| 11 | 設計與創意 | 11 | `ux_review` UX 評審 / `design_brief` 設計簡報 / `accessibility_audit` 無障礙審計 |
| 12 | 系統維運 | 10 | `incident_response` 故障回應 / `monitoring_setup` 監控設定 / `security_hardening` 安全加固 |
| 13 | 效率提升 | 8 | `requirements_breakdown` 需求拆解 / `prd_draft` PRD 草稿 / `meeting_minutes` 會議紀要 |
| 14 | 工程交付 | 6 | `release_notes` 發布說明 / `rca_report` 根因報告 / `rollback_plan` 回滾計畫 |
| 15 | 營運支援 | 6 | `customer_reply` 客服回覆 / `kb_article` 知識庫文章 / `faq_builder` FAQ 生成 |
| 16 | Go-On 代理技能 | 5 | `skill_discovery` 技能發現 / `tool_selection` 工具選擇 / `best_practices` 代理最佳實踐 |

---

## GUI 操作

在 GUI 的 **Prompts 標籤頁**中，您可以：

- **瀏覽** — 按行業類別分組檢視所有範本（內建 + 自訂）
- **搜尋** — 按關鍵字搜尋範本標題和內容
- **建立** — 建立新的自訂範本，選擇類別和語言
- **編輯** — 修改已有的自訂範本
- **刪除** — 移除不需要的自訂範本

### 操作流程

1. 切換到 Prompts 標籤頁
2. 在左側選擇行業類別進行篩選
3. 使用搜尋框按關鍵字搜尋
4. 點擊範本卡片檢視詳情
5. 點擊 **插入到 Chat** 按鈕，將範本內容插入到 Chat 輸入框
6. 在 Chat 中微調範本內容後發送

---

## Chat `/` 指令

在 Chat 輸入框中輸入 `/` 可觸發指令補全。輸入範本 ID 後按 Enter 即可直接展開範本內容。

### 指令格式

```
/<template_id> <參數>
```

範例：
- `/explain_code` — 解釋選中的程式碼
- `/code_review` — 審查程式碼品質
- `/generate_unit_test Rust` — 為 Rust 程式碼生成單元測試
- `/blog_post_outline topic: AI 趨勢` — 撰寫指定主題的文章

### 常用指令

| 指令 | 功能 |
|------|------|
| `/explain_code` | 解釋選中的程式碼 |
| `/code_review` | 審查程式碼品質 |
| `/refactor_suggestion` | 重構建議 |
| `/generate_unit_test` | 生成單元測試 |
| `/debug_error` | 除錯錯誤 |
| `/generate_documentation` | 編寫文件註解 |
| `/blog_post_outline` | 撰寫文章大綱 |
| `/marketing_strategy` | 行銷策略 |
| `/contract_review` | 合約審查 |
| `/literature_review` | 文獻綜述 |

輸入 `/` 時，系統會顯示自動補全列表，支援模糊搜尋、類別篩選和完整範本 ID 比對。

---

## 自訂範本

### 建立範本

1. 在 Prompts 標籤頁中，點擊 **建立新範本**
2. 填寫範本詳細資訊：
   - **ID** — `/` 指令呼叫的唯一識別碼（例如 `my_custom_template`）
   - **標題** — 範本的顯示名稱
   - **類別** — 所屬行業類別
   - **語言** — 範本語言
   - **內容** — 提示詞文字，支援 `{{variable}}` 佔位符
   - **描述** — 簡要說明範本用途
3. 儲存 — 範本將寫入 `prompts/custom/{lang}.json`

### 編輯範本

在 Prompts 標籤頁中找到自訂範本，點擊編輯按鈕進行修改。儲存後，系統將更新 `prompts/custom/{lang}.json`。

### 刪除範本

在 Prompts 標籤頁中找到自訂範本，點擊刪除按鈕並確認。

> ⚠️ 注意：只能刪除自訂範本，內建範本為唯讀。

---

## 後端 RPC 介面

提示詞系統提供以下 RPC 介面：

| RPC | 方法 | 描述 |
|-----|------|------|
| `prompts.list` | List | 取得所有範本，可按類別和語言篩選 |
| `prompts.search` | Search | 按關鍵字搜尋範本 |
| `prompts.get` | Get | 取得單個範本的詳細資訊 |
| `prompts.create` | Create | 建立新的自訂範本 |
| `prompts.update` | Update | 更新自訂範本 |
| `prompts.delete` | Delete | 刪除自訂範本 |

---

## MCP 工具

透過 MCP（Model Context Protocol），AI 代理可以自動發現並呼叫提示詞範本：

| MCP 工具 | 功能 |
|----------|------|
| `prompts_list` | 列出所有可用的提示詞範本 |
| `prompts_get` | 取得指定範本的詳細內容 |

AI 代理透過 MCP 協定發現這些工具，並可在對話過程中自動選擇和應用合適的範本。例如，當使用者說"幫我審查這段程式碼"時，AI 代理可以自動呼叫 `prompts_get` 取得 `code_review` 範本並應用到回覆中。

---

## 設定開關

在 **設定 → 功能開關** 中，可以啟用或停用 **Prompts** 標籤頁。停用後：

- Prompts 標籤頁從標籤列中隱藏

該開關僅控制 GUI 顯示 — 後端的 `prompts.*` RPC 介面和 MCP
`prompts_list` / `prompts_get` 工具對用戶端始終可用，不受 GUI 開關影響。

> 預設狀態：**啟用**
