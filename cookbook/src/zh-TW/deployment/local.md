# 本地模式部署

## 概述

本地模式（`local`）是 go-on 的默認部署配置，專為單用戶開發環境設計。它提供了一個輕量級、自包含的運行時，具有基於 SQLite 的存儲和自適應向量能力。

## 特性

### 核心能力
- **單用戶操作**：專為個人開發者設計
- **SQLite 存儲**：基於本地文件的緩存和向量存儲
- **自適應向量存儲**：當 `sqlite-vec` 可用時使用，否則回退到 JSON 嵌入
- **零外部依賴**：無需數據庫服務器
- **所有核心子總線均已包含**：完整的工具、編排、可觀測、優化、內存和協議子總線支持（distributed-memory 僅伺服器配置啟用）

### 存儲架構
```
本地模式存儲：
├── 緩存：SQLite 數據庫 (acp_cache.sqlite3)
├── 向量存儲：帶向量擴展的 SQLite
└── 配置：本地 config.toml 文件
```

## 配置

### 默認配置
本地模式使用 `config/config.toml` 作為默認配置：

```toml
# config/config.toml（local 默認配置；精簡摘錄）
schema_version = "1.0.0"
default_phase = "think"
model_selection_mode = "adaptive"

# 根級治理開關（經 #[serde(flatten)]）
governance_enabled = true
governance_policy_mode = "advisory"

[cache]
enabled = true
path = "sqlite3/acp_cache.sqlite3"
default_ttl_seconds = 3600
max_entries = 5000

[vector]
enabled = true
auto_mode = true
path = "sqlite3/acp_vector.sqlite3"
dimensions = 192
min_query_chars = 80
top_k = 2
min_similarity = 0.82
max_snippet_chars = 800
max_entries = 10000
summary_trigger_messages = 8
summary_max_chars = 1200

[runtime]
acp_http_bind_addr = "127.0.0.1:8090"
maintenance_interval_seconds = 60
health_interval_seconds = 120
shutdown_drain_seconds = 30
entry_auth_enabled = false
entry_rate_limit_rpm = 240
entry_rate_limit_burst = 60
i18n_default_language = "en-US"
skills_enabled = true
skills_cache_dir = "skills_cache"

# OpenTelemetry
otel_enabled = true
otel_exporter = "otlp"
otel_endpoint = "http://localhost:4317"

[protocol]
mode = "adaptive"
```

### 特性標誌
本地模式啟用以下 Cargo 特性：
- `backend-sqlite`：SQLite 數據庫支持
- `rusqlite`：帶捆綁 SQLite 的 SQLite 綁定
- `sqlite-vec`：SQLite 向量擴展（可選）

## 安裝

### 從源碼構建
```bash
# 默認構建（local）
cargo build

# 顯式本地模式構建
cargo build --no-default-features -F local

# 包含所有特性
cargo build --features "backend-sqlite"
```

### 二進制分發
```bash
# 下載預構建二進制文件
curl -L https://github.com/your-org/go-on/releases/latest/download/go-on-x86_64-unknown-linux-gnu.tar.gz | tar xz

# 設為可執行
chmod +x go-on
```

## 設置

### 初始配置
```bash
# 使用默認配置初始化
cargo run -- --init --config config/config.toml

# 檢查配置
cargo run -- --check --config config/config.toml
```

### 可選設置級別
```bash
# 快速設置（最小化配置）
cargo run -- --init --setup-level quick --config config/config.toml

# 標準設置（推薦）
cargo run -- --init --setup-level standard --config config/config.toml

# 自定義設置（高級）
cargo run -- --init --setup-level custom --config config/config.toml
```

## 運行

### 啟動運行時
```bash
# 使用啟動腳本
./scripts/start-go-on.sh

# 直接執行
cargo run -- --config config/config.toml

# 使用特定協議模式
cargo run -- --config config/config.toml --protocol-mode adaptive
```

### 健康檢查
```bash
# 默認健康端點
curl http://127.0.0.1:8090/health
```

## 開發工作流

### 典型使用模式
1. **啟動運行時**：`./scripts/start-go-on.sh`
2. **連接 IDE**：配置 Zed 或 VS Code 使用本地 go-on
3. **開發**：使用 AI 輔助編碼功能
4. **監控**：檢查健康端點狀態
5. **停止**：使用 `./scripts/stop-go-on.sh` 或 Ctrl+C

### IDE 集成
- **Zed**：使用 ACP over stdio 或 HTTP
- **VS Code**：使用 go-on 擴展與本地運行時
- **GUI 控制台**：基於 EGUI（Rust 原生）的桌面圖形界面

## 存儲管理

### 緩存位置
- **默認**：`sqlite3/acp_cache.sqlite3`（見 `config/config.toml`）
- **自定義**：在配置中設置 `cache.path`
- **大小限制**：默認 5000 條記錄（見 `config/config.toml` 的 `max_entries`）

### 向量存儲
- **位置**：`sqlite3/acp_vector.sqlite3`（見 `config/config.toml`）
- **維度**：192 維嵌入（見 `config/config.toml` 的 `dimensions`）
- **自動模式**：啟用 autotune 對向量查詢參數（`min_query_chars` / `top_k` / `min_similarity`）的自動調參（見 `config/config.toml` 的 `auto_mode`）

### 維護
```bash
# 清理緩存（手動）
rm -f sqlite3/acp_cache.sqlite3 sqlite3/acp_cache.sqlite3-*

# 重置向量存儲
rm -f sqlite3/acp_vector.sqlite3

# 壓縮 SQLite 數據庫
sqlite3 sqlite3/acp_cache.sqlite3 "VACUUM;"
sqlite3 sqlite3/acp_vector.sqlite3 "VACUUM;"
```

## 性能調優

### 併發與超時
並發限制透過 `[phases.<name>.options]` 按階段設定（`phase_max_inflight` /
`global_max_inflight`）。不存在 `[concurrency]` 或 `[timeouts]` 頂層區段。

## 故障排除

### 常見問題

#### SQLite 錯誤
```bash
# 檢查 SQLite 版本
sqlite3 --version

# 修復損壞的數據庫
sqlite3 sqlite3/acp_cache.sqlite3 ".recover" | sqlite3 sqlite3/acp_cache_fixed.sqlite3
```

#### 向量存儲問題
```bash
# 檢查 sqlite-vec 可用性
cargo build --features backend-sqlite
```

向量存儲自動解析模式：`local` profile 下 sqlite-vec 不可用時自動回退到 JSON 嵌入表；`simple-server` / `multi-users-server` 需要 sqlite-vec（或 pgvector）。`auto_mode` 僅控制 autotune 調參（對 `min_query_chars` / `top_k` / `min_similarity` 等查詢參數的自動調整），與 JSON 回退無關，且不存在 `use_json_fallback` 配置開關。

#### 端口衝突
```bash
# 檢查端口使用情況
lsof -i :8090

# 在配置中更改端口
[runtime]
acp_http_bind_addr = "127.0.0.1:8091"
```

### 日誌
```bash
# 啟用調試日誌
RUST_LOG=debug ./scripts/start-go-on.sh

# 查看日誌
tail -f go-on.log
```

## 遷移

### 從舊版本遷移
```bash
# 備份現有數據
cp sqlite3/acp_cache.sqlite3 sqlite3/acp_cache.sqlite3.backup
cp sqlite3/acp_vector.sqlite3 sqlite3/acp_vector.sqlite3.backup

# 配置 schema 帶版本號（schema_version），啟動時驗證並遷移受支援的 schema。
# 不存在 --migrate CLI 標誌。
```

### 遷移到其他部署模式
本地模式可以遷移到：
- **簡單服務器模式**：用於單服務器部署
- **多用戶服務器模式**：用於生產多用戶環境

## 最佳實踐

### 安全
- 將配置文件保存在版本控制中（排除密鑰）
- 使用環境變量存儲 API 密鑰
- 定期更新到最新版本

### 性能
- 將 SQLite 文件放在快速存儲上（SSD）
- 監控磁盤空間使用情況
- 定期維護（壓縮、分析）

### 開發
- 為不同項目使用單獨的配置
- 備份重要的向量存儲
- 使用不同的模型供應商進行測試

## 限制

### 已知約束
- **僅限單用戶**：不支持併發多用戶訪問
- **本地存儲**：性能取決於本地磁盤速度
- **內存限制**：受可用系統內存限制
- **無高可用性**：單點故障

### 何時考慮其他模式
考慮升級到：
- **簡單服務器模式**：需要更好性能時
- **多用戶服務器模式**：需要多用戶支持時

## 下一步

設置本地模式後，您可以：
1. 探索 [API 文檔](../api/overview.md)
2. 了解[簡單服務器模式](./simple-server.md)
3. 加入[社區討論](https://github.com/mikewolfli/go-on/discussions)