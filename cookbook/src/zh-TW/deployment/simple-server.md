# 簡單服務器模式部署

## 概述

簡單服務器模式（`simple-server`）專為單服務器部署設計，提供比本地模式更好的性能和可靠性，同時保持簡單性。它使用帶有必需向量擴展的 SQLite，適用於小團隊或類似生產環境。

## 特性

### 增強能力
- **單服務器部署**：專為專用服務器環境設計
- **必需向量擴展**：使用 `sqlite-vec`（無 JSON 回退）
- **改進的性能**：針對服務器工作負載優化
- **更好的可靠性**：增強的錯誤處理和恢復
- **生產就緒**：適用於小規模生產使用
- **完整 Phase 4 架構**：所有 14 條總線和 21 個 F-GAP 模塊，包括 AgentFactory 和 OrchestrationCouncil
- **條件編譯模塊**：AgentFactory 和 Council 使用 `#[cfg(feature = "simple-server")]` 門控

### 架構
```
簡單服務器架構：
├── 運行時：專用服務器進程
├── 存儲：帶向量擴展的 SQLite
├── 網絡：HTTP/HTTPS 端點
└── 監控：增強的可觀測性
```

## 配置

### 服務器配置
創建 `config/simple-server.toml`：

```toml
# config/simple-server.toml
default_phase = "coding"
model_selection_mode = "adaptive"

[protocol]
mode = "acp_http"  # 服務器部署的 HTTP 模式

[runtime]
acp_http_bind_addr = "0.0.0.0:8090"  # 綁定到所有接口
production_strict = true
entry_auth_enabled = true
entry_auth_api_key_env = "GO_ON_SERVER_API_KEY"
entry_rate_limit_rpm = 1000
entry_rate_limit_burst = 200

[cache]
enabled = true
path = "/var/lib/go-on/cache.sqlite3"
default_ttl_seconds = 7200
max_entries = 20000

[vector]
enabled = true
auto_mode = false  # 需要 sqlite-vec
use_json_fallback = false
path = "/var/lib/go-on/vector.sqlite3"
dimensions = 384  # 更高維度以獲得更好準確性
top_k = 5
min_similarity = 0.75

[observability]
otel_enabled = true
otel_exporter = "otlp"
otel_endpoint = "http://localhost:4317"
metrics_port = 9090
```

### 特性標誌
簡單服務器模式需要：
- `backend-sqlite`：SQLite 數據庫支持
- `sqlite-vec`：向量擴展（必需，無回退）

## 安裝

### 為服務器部署構建
```bash
# 使用 simple-server 配置文件構建
cargo build --no-default-features -F simple-server

# 或顯式啟用特性
cargo build --features "backend-sqlite sqlite-vec"
```

### 系統要求
- **CPU**：推薦 2+ 核心
- **內存**：4GB+ RAM
- **存儲**：10GB+ 可用空間（推薦 SSD）
- **網絡**：穩定的互聯網連接用於模型供應商

### Systemd 服務設置
創建 `/etc/systemd/system/go-on.service`：

```ini
[Unit]
Description=go-on Simple Server
After=network.target

[Service]
Type=simple
User=go-on
Group=go-on
WorkingDirectory=/opt/go-on
Environment="GO_ON_SERVER_API_KEY=your-api-key-here"
Environment="RUST_LOG=info"
ExecStart=/opt/go-on/go-on --config /opt/go-on/config/simple-server.toml
Restart=on-failure
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

## 設置

### 目錄結構
```bash
# 創建目錄
sudo mkdir -p /opt/go-on /var/lib/go-on /var/log/go-on
sudo chown -R go-on:go-on /opt/go-on /var/lib/go-on /var/log/go-on

# 複製配置
sudo cp config/simple-server.toml /opt/go-on/config/
sudo cp scripts/start-go-on.sh /opt/go-on/
sudo chmod +x /opt/go-on/start-go-on.sh
```

### 數據庫初始化
```bash
# 以 go-on 用戶身份初始化
sudo -u go-on /opt/go-on/go-on --init --config /opt/go-on/config/simple-server.toml

# 檢查配置
sudo -u go-on /opt/go-on/go-on --check --config /opt/go-on/config/simple-server.toml
```

### 用戶和權限
```bash
# 創建系統用戶
sudo useradd -r -s /bin/false -m -d /opt/go-on go-on

# 設置權限
sudo chown -R go-on:go-on /opt/go-on
sudo chmod 750 /opt/go-on
```

## 運行

### 啟動服務器
```bash
# 使用 systemd
sudo systemctl daemon-reload
sudo systemctl enable go-on
sudo systemctl start go-on

# 檢查狀態
sudo systemctl status go-on

# 查看日誌
sudo journalctl -u go-on -f
```

### 手動啟動
```bash
# 以 go-on 用戶身份
sudo -u go-on /opt/go-on/go-on --config /opt/go-on/config/simple-server.toml

# 帶環境變量
GO_ON_SERVER_API_KEY="your-key" sudo -u go-on /opt/go-on/go-on --config /opt/go-on/config/simple-server.toml
```

### 健康和監控
```bash
# 健康端點
curl http://localhost:8090/health

# 指標端點
curl http://localhost:9090/metrics

# Prometheus 指標
curl http://localhost:9090/metrics/prometheus
```

## 網絡配置

### 防火牆規則
```bash
# 允許 HTTP 端口
sudo ufw allow 8090/tcp

# 允許指標端口
sudo ufw allow 9090/tcp

# 啟用防火牆
sudo ufw enable
```

### 反向代理（Nginx）
創建 `/etc/nginx/sites-available/go-on`：

```nginx
server {
    listen 80;
    server_name go-on.example.com;
    
    location / {
        proxy_pass http://127.0.0.1:8090;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection 'upgrade';
        proxy_set_header Host $host;
        proxy_cache_bypass $http_upgrade;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
    
    # 健康檢查端點
    location /health {
        proxy_pass http://127.0.0.1:8090/health;
        access_log off;
    }
}
```

### SSL/TLS 配置
```bash
# 使用 Let's Encrypt 和 Certbot
sudo certbot --nginx -d go-on.example.com

# 或手動 SSL 配置
ssl_certificate /etc/ssl/certs/go-on.crt;
ssl_certificate_key /etc/ssl/private/go-on.key;
```

## 存儲管理

### 數據庫維護
```bash
# 定期壓縮（每週）
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3 "VACUUM;"
sudo -u go-on sqlite3 /var/lib/go-on/vector.sqlite3 "VACUUM;"

# 分析查詢優化
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3 "ANALYZE;"
sudo -u go-on sqlite3 /var/lib/go-on/vector.sqlite3 "ANALYZE;"
```

### 備份策略
```bash
# 每日備份腳本
#!/bin/bash
BACKUP_DIR="/backup/go-on"
DATE=$(date +%Y%m%d)

# 備份數據庫
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3 ".backup $BACKUP_DIR/cache-$DATE.sqlite3"
sudo -u go-on sqlite3 /var/lib/go-on/vector.sqlite3 ".backup $BACKUP_DIR/vector-$DATE.sqlite3"

# 備份配置
cp /opt/go-on/config/simple-server.toml $BACKUP_DIR/config-$DATE.toml

# 輪轉舊備份（保留 30 天）
find $BACKUP_DIR -name "*.sqlite3" -mtime +30 -delete
find $BACKUP_DIR -name "*.toml" -mtime +30 -delete
```

### 磁盤空間監控
```bash
# 檢查數據庫大小
du -h /var/lib/go-on/*.sqlite3

# 監控增長
df -h /var/lib/go-on
```

## 性能調優

### 內存優化
```toml
[runtime]
# 根據服務器內存調整
cache_max_memory_mb = 1024
vector_max_memory_mb = 2048
max_connections = 100
```

### 併發設置
```toml
[concurrency]
max_inflight_requests = 100
max_parallel_tasks = 16
worker_threads = 8
```

### 超時配置
```toml
[timeouts]
request_timeout_seconds = 180
health_check_timeout_seconds = 10
shutdown_timeout_seconds = 120
database_timeout_seconds = 30
```

## 安全

### API 密鑰管理
```bash
# 在環境中設置 API 密鑰
export GO_ON_SERVER_API_KEY="secure-random-key-here"

# 或使用密鑰環
keyring set go-on server-api-key
```

### 速率限制
```toml
[security]
rate_limit_enabled = true
rate_limit_rpm = 1000
rate_limit_burst = 200
rate_limit_by_ip = true
```

### 訪問控制
```toml
[access]
allowed_ips = ["192.168.1.0/24", "10.0.0.0/8"]
blocked_ips = []
require_https = true
cors_allowed_origins = ["https://your-domain.com"]
```

## 監控和日誌

### 日誌配置
```toml
[logging]
level = "info"
file_path = "/var/log/go-on/go-on.log"
max_file_size_mb = 100
max_files = 10
json_format = true
```

### 指標收集
```toml
[metrics]
enabled = true
port = 9090
path = "/metrics"
collect_interval_seconds = 30
```

### 告警
```bash
# Prometheus 告警規則示例
groups:
- name: go-on-alerts
  rules:
  - alert: GoOnHighErrorRate
    expr: rate(go_on_errors_total[5m]) > 0.1
    for: 2m
    labels:
      severity: warning
    annotations:
      summary: "檢測到高錯誤率"
      description: "錯誤率為每秒 {{ $value }}"
```

## 擴展考慮

### 何時擴展
- CPU 使用率持續高於 70%
- 內存使用率持續高於 80%
- 響應時間顯著增加
- 併發用戶超過 50

### 擴展選項
1. **垂直擴展**：升級服務器資源
2. **水平擴展**：遷移到多用戶服務器模式
3. **負載均衡**：添加多個簡單服務器實例

## 遷移

### 從本地模式遷移
```bash
# 從本地模式導出數據
cargo run -- --export --config config/config.toml --output local-export.json

# 導入到簡單服務器
sudo -u go-on /opt/go-on/go-on --import --config /opt/go-on/config/simple-server.toml --input local-export.json
```

### 備份和恢復
```bash
# 完整備份
tar czf go-on-backup-$(date +%Y%m%d).tar.gz /opt/go-on /var/lib/go-on

# 恢復
tar xzf go-on-backup-20240101.tar.gz -C /
```

## 故障排除

### 常見問題

#### 服務無法啟動
```bash
# 檢查日誌
sudo journalctl -u go-on --no-pager -n 50

# 檢查權限
sudo ls -la /opt/go-on/
sudo ls -la /var/lib/go-on/

# 手動測試
sudo -u go-on /opt/go-on/go-on --config /opt/go-on/config/simple-server.toml --dry-run
```

#### 數據庫問題
```bash
# 檢查 SQLite 完整性
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3 "PRAGMA integrity_check;"
sudo -u go-on sqlite3 /var/lib/go-on/vector.sqlite3 "PRAGMA integrity_check;"

# 需要時修復
sudo -u go-on cp /var/lib/go-on/cache.sqlite3 /var/lib/go-on/cache.sqlite3.backup
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3.backup ".recover" | sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3
```

#### 性能問題
```bash
# 監控資源使用情況
top -u go-on
iotop -u go-on

# 檢查數據庫性能
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3 "EXPLAIN QUERY PLAN SELECT * FROM cache WHERE key = 'test';"
```

## 下一步

設置簡單服務器模式後，您可以：
1. 配置 [監控和告警](./monitoring.md)
2. 設置 [備份策略](./backup.md)
3. 探索 [API 文檔](../api/overview.md)
4. 考慮 [多用戶服務器模式](./multi-users-server.md) 用於更大規模部署