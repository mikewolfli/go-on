# GUI 桌面控制台

GUI 是基於 EGUI（Rust 原生）的桌面圖形界面，位於 `gui/` 目錄下。
它提供後端監控、多會話對話、技能管理和設置編輯等功能，
讓運維和集成調試不必一直停留在終端裡。

## 架構概述

GUI 是一個 Rust 原生桌面應用，使用 EGUI 框架（基於 `eframe`/`egui`）構建。
它通過 ACP+HTTP JSON-RPC 與後端通信，自動管理後端進程的生命週期。

GUI 持有並使用三個核心值：

- 後端可執行文件路徑
- 工作目錄
- 工作目錄中的運行時配置文件

GUI 拉起後端進程時會以工作目錄作為當前目錄，因此要求 `config.toml` 就放在該目錄下。

## 功能面板

### 監控面板 (Monitor)
- 後端健康狀態：通過 `/health` 端點自動輪詢
- AI 供應商狀態：實時顯示 Provider 連接狀況
- 實時指標：請求數、延遲、錯誤率

### 對話界面 (Chat)
- 多會話管理：創建、切換、刪除會話
- 多模型支持：每個會話可選擇不同 AI 模型
- 階段選擇：coding / review / debug / test / deploy
- 模式切換：Ask / Plan / Edit / Safeguard / Full Auto
- 文件附件：支持上傳文件作為對話上下文
- 動態發送按鈕：依據 AI 狀態變化（loading / ready / error）

### 技能管理 (Skills)
- 創建和導入 AI 技能
- 內置 `skill-creator`：讓 AI 自主定義新技能
- 技能列表管理：啟用、禁用、刪除

### 設置面板 (Settings)
- **Provider 管理**：動態環境變量注入（全部 37 家供應商），不再限於 8 個硬編碼
- **配置文件編輯器**：管理 `gui_config.json`，含 JSON 語法驗證
- **主題選擇**：6 種視覺主題（简约 / 国风 / 武侠 / 山水 / Hello Kitty / 為人民服務）
- **語言切換**：繁體中文、簡體中文、English
- **功能開關**：啟用/禁用各項 GUI 功能

## 開發與構建命令

在 `gui/` 目錄下：

```bash
# 開發運行
cargo run

# 構建（release）
cargo build --release

# 從項目根目錄運行
cargo run --manifest-path gui/Cargo.toml
```

## 綁定後端

GUI 可以自動發現後端可執行文件。自動綁定成功後，會把可執行文件所在目錄作為工作目錄，並把日誌落到該目錄下的 `go-on.log`。

如果自動發現失敗，就手工配置：

1. 填寫後端可執行文件路徑
2. 填寫工作目錄
3. 確保該目錄下存在 `config.toml`

## 密鑰管理

GUI 使用雙重存儲機制：

- **系統密鑰環 (keyring)**：優先使用操作系統級別的密鑰存儲（如 Linux 的 Secret Service、macOS 的 Keychain、Windows 的 Credential Manager）
- **配置文件 (config file)**：作爲備份和便攜方案

API Key 無需寫入 `.env.goon`，全部通過 GUI 的 Provider 管理界面動態注入。

## 運行時進程行爲

GUI 啓動後端時，會從當前配置的工作目錄拉起該可執行文件，並把 stdout 與 stderr 都寫到 `go-on.log`。

**自動重啓**：如果後端崩潰，GUI 會在 3 秒冷卻後自動重啓後端進程。

因此最常見的操作錯誤是：二進制路徑正確，但工作目錄錯誤，導致配置文件找不到或加載了錯誤配置。

## 健康檢查與集成探測

GUI 當前會探測：

- `/health` 上的 ACP 或運行時健康狀態
- `/v1/models` 上的 OpenAI 兼容模型列表

這些探測會被解釋成以下前端狀態：

- Zed 的 ACP 或 A2A over HTTP
- Zed 的 MCP 或 `/v1` 模型提供方風格接入
- VS Code 插件運行時健康狀態

## GUI 場景下推薦的後端模式

- `adaptive`：最推薦，適合 GUI 與 Zed、VS Code 共用一個後端。
- `acp_http`：適合只關心 ACP over HTTP。
- `mcp_http`：適合主要關注 `/v1` provider 兼容面。

GUI 本身無論哪種模式都可以繼續管理後端可執行文件，模式差異主要體現在 GUI 啟動之後外部客戶端還能做什麼。

## 推薦操作順序

1. 構建後端：`cargo build`
2. 初始化後端（首次）：`cargo run -- --init`
3. 構建 GUI：`cargo build --manifest-path gui/Cargo.toml`
4. 啟動 GUI：`cargo run --manifest-path gui/Cargo.toml`
5. 使用自動綁定，或手工填寫可執行文件路徑
6. 確認工作目錄中存在 `config.toml`
7. 在 Provider 管理中配置 API Key（自動存儲到系統密鑰環）
8. 啟動後端
9. 查看健康狀態和集成探測結果

## 故障排查

- 如果啟動時報文件錯誤，先重新檢查可執行文件路徑。
- 如果啟動成功但探測失敗，優先檢查協議模式和 Provider 就緒狀態。
- 如果 GUI 顯示健康正常，但編輯器仍失敗，就對照編輯器所需傳輸契約與當前運行時模式是否一致。
- GUI 自身問題：檢查 `gui_config.json` 是否損壞，必要時刪除重置。
