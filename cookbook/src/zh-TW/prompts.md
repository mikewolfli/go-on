# Prompts 系統

## 概述

Prompts 系統是 go-on 內建的提示詞範本管理功能，提供 **84+ 個即用範本**，涵蓋 12 個行業類別。使用者可透過 GUI 瀏覽、搜尋和插入範本，在 Chat 中使用 `/` 指令快速展開範本，或透過 MCP（Model Context Protocol）讓 AI 代理自動呼叫範本。

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

## 12 個行業類別

每個類別包含 7 個範本，共 **84+ 個範本**，覆蓋各行業典型使用場景。

| # | 類別 | 描述 | 代表範本 |
|---|------|------|----------|
| 1 | 軟體開發 | 程式碼生成、除錯、重構、程式碼審查 | `explain_code` 解釋程式碼 / `review_code` 程式碼審查 / `generate_test` 生成單元測試 |
| 2 | 寫作與創意 | 文章寫作、創意寫作、文案撰寫 | `write_article` 撰寫文章 / `creative_story` 創意故事 / `copywriting` 文案寫作 |
| 3 | 學術研究 | 論文寫作、文獻綜述、資料分析 | `write_paper` 論文寫作 / `literature_review` 文獻綜述 / `data_analysis` 資料分析 |
| 4 | 商業分析 | 市場分析、商業模式、競爭分析 | `market_analysis` 市場分析 / `business_model` 商業模式 / `competitive_analysis` 競爭分析 |
| 5 | 市場行銷 | 行銷方案、廣告文案、社交媒體 | `marketing_plan` 行銷計畫 / `ad_copy` 廣告文案 / `social_media` 社交媒體內容 |
| 6 | 法律與合規 | 合約審查、法律諮詢、合規檢查 | `contract_review` 合約審查 / `legal_advice` 法律諮詢 / `compliance_check` 合規檢查 |
| 7 | 醫療與健康 | 醫療諮詢、健康管理、藥品資訊 | `medical_consult` 醫療諮詢 / `health_plan` 健康計畫 / `drug_info` 藥品資訊 |
| 8 | 教育與培訓 | 課程設計、教案編寫、輔導答疑 | `course_design` 課程設計 / `lesson_plan` 教案 / `tutoring` 輔導答疑 |
| 9 | 金融與投資 | 投資分析、風險評估、財務報告 | `investment_analysis` 投資分析 / `risk_assessment` 風險評估 / `financial_report` 財務報告 |
| 10 | 資料科學 | 資料分析、機器學習、資料視覺化 | `data_cleaning` 資料清洗 / `ml_model` 機器學習模型 / `data_viz` 資料視覺化 |
| 11 | 設計與創意 | UI/UX 設計、平面設計、創意腦力激盪 | `ui_design` UI 設計 / `brand_design` 品牌設計 / `creative_brainstorm` 創意腦力激盪 |
| 12 | 系統維運 | 伺服器管理、網路配置、監控告警 | `server_setup` 伺服器設定 / `network_config` 網路配置 / `monitor_setup` 監控設定 |

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
- `/review_code` — 審查程式碼品質
- `/generate_test Rust` — 為 Rust 程式碼生成單元測試
- `/write_article topic: AI 趨勢` — 撰寫指定主題的文章

### 常用指令

| 指令 | 功能 |
|------|------|
| `/explain_code` | 解釋選中的程式碼 |
| `/review_code` | 審查程式碼品質 |
| `/optimize_code` | 最佳化程式碼效能 |
| `/generate_test` | 生成單元測試 |
| `/refactor_code` | 重構程式碼 |
| `/write_doc` | 編寫文件註解 |
| `/write_article` | 撰寫文章 |
| `/market_analysis` | 市場分析 |
| `/contract_review` | 合約審查 |
| `/data_analysis` | 資料分析 |

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

AI 代理透過 MCP 協定發現這些工具，並可在對話過程中自動選擇和應用合適的範本。例如，當使用者說"幫我審查這段程式碼"時，AI 代理可以自動呼叫 `prompts_get` 取得 `review_code` 範本並應用到回覆中。

---

## 設定開關

在 **設定 → 功能開關** 中，可以啟用或停用 **Prompts** 模組。停用後：

- Prompts 標籤頁從標籤列中隱藏
- 相關 RPC 介面停止回應
- 相關 MCP 工具停止回應

> 預設狀態：**啟用**
