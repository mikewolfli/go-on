# 多用戶服務器模式部署

## 概述

多用戶服務器模式（`profile-multi-users-server`）是 go-on 的企業級部署配置，專為具有多個併發用戶的生產環境設計。它使用 PostgreSQL 和 pgvector 進行可擴展存儲，並提供安全、監控和高可用性的高級功能。

## 特性

### 企業能力
- **多用戶支持**：專為多個用戶併發訪問設計
- **PostgreSQL 存儲**：帶 pgvector 擴展的可擴展數據庫
- **CORS 支援**：可配置的允許來源、預檢（OPTIONS）處理、所有 HTTP/SSE 響應上的 CORS 標頭
- **入口認證（閘道）**：通過 `Authorization: Bearer`、`X-Api-Key` 或 `X-Go-On-Key` 標頭驗證的共享 API 金鑰
- **用戶認證（HMAC 權杖）**：基於 HMAC-SHA256 簽名的每用戶 JWT 類權杖，自動配置，可配置 TTL
- **RBAC 授權**：基於角色的訪問控制（admin/user/viewer/monitor），在 ACP+HTTP 和 MCP+HTTP 路徑上均進行按端點權限檢查
- **租戶預算控制**：每個租戶的每日權杖/併發任務/API 調用配額，自動配置
- **對話隔離**：帶租戶前綴的命名空間對話 ID，防止跨用戶數據洩露
- **關閉排空**：優雅關閉期間處理進行中連接的可配置排空期
- **信號處理**：所有平台上的 SIGINT（Ctrl+C）和 SIGTERM，實現乾淨關閉
- **執行緒安全金鑰管理**：`SECRET_OVERRIDE_MAP` 替代 `std::env::set_var()`（在多執行緒上下文中已記錄為未定義行為）；`KEYRING_CACHE` 帶 30 秒 TTL，避免在異步熱路徑中阻塞金鑰環 I/O
- **熱重載配置**：通過 `config.reload` RPC 進行運行時配置重載
- **高可用性**：內置冗餘和故障轉移支持
- **企業監控**：全面的可觀測性堆棧
- **可擴展性**：水平擴展能力
- **完整 Phase 4 架構**：所有 14 條總線和 21 個 F-GAP 模塊
- **分佈式內存總線**：通過 DistributedMemoryBus 跨節點記憶共享
- **跨節點容錯引擎**：跨節點故障隔離和自動恢復
- **多渠道消息傳輸**：6 通道、QoS 啟用的消息傳輸

### 架構
```
多用戶服務器架構：
├── 應用層：go-on 運行時實例
│   ├── ACP HTTP 服務器（端口 8090）：CORS → 入口認證 → 用戶認證 → RBAC → 路由
│   └── MCP HTTP 服務器（端口 8090）：CORS → 入口認證 → 用戶認證 → RBAC → 分發
├── 數據庫層：帶 pgvector 的 PostgreSQL
├── 緩存層：Redis（可選）
├── 負載均衡器：流量分發
├── 監控：Prometheus、Grafana、ELK
└── 備份：自動化備份系統

請求流程（ACP+HTTP）：
客戶端 → CORS 標頭 → 入口認證（API 金鑰）→ 用戶會話（HMAC 權杖）
       → RBAC 授權 → 租戶預算檢查 → 命名空間對話
       → AI 處理 → 帶 CORS 標頭的響應

請求流程（MCP+HTTP）：
客戶端 → CORS 標頭 → 入口認證（API 金鑰）→ 用戶會話（HMAC 權杖）
       → RBAC 授權 → MCP 方法分發 → 帶 CORS 的 JSON-RPC 響應
```

## 配置

### 服務器配置
創建 `config/multi-users-server.toml`：

```toml
# config/multi-users-server.toml
default_phase = "coding"
model_selection_mode = "adaptive"

[protocol]
mode = "acp_http"  # 用於多用戶訪問的 ACP over HTTP（使用 mcp_http 用於 MCP）

[runtime]
acp_http_bind_addr = "0.0.0.0:8090"
production_strict = true

# ── 網關入口認證（所有入站流量的共享 API 金鑰）────
entry_auth_enabled = true
entry_auth_api_key_env = "GO_ON_ENTRY_API_KEY"
entry_rate_limit_rpm = 5000
entry_rate_limit_burst = 1000

# ── 用戶認證（每用戶 HMAC 權杖）─────────────
user_auth_enabled = true
user_auth_token_secret_env = "GO_ON_USER_AUTH_TOKEN_SECRET"
user_auth_token_ttl_seconds = 86400

# ── 跨域資源共享（CORS）───────────────────
cors_allowed_origins = ["https://my-frontend.example.com"]
# 僅用於開發使用 "*"：
# cors_allowed_origins = ["*"]

# ── 租戶資源配額 ────────────────────────────
tenant_default_daily_token_limit = 1000000
tenant_default_concurrent_tasks = 10
tenant_default_daily_api_calls = 10000

[database]
type = "postgres"
host = "localhost"
port = 5432
database = "go_on_production"
username_env = "GO_ON_DB_USERNAME"
password_env = "GO_ON_DB_PASSWORD"
pool_size = 20
connection_timeout_seconds = 30
ssl_mode = "prefer"

[cache]
enabled = true
type = "database"  # 使用 PostgreSQL 作為緩存

[vector]
enabled = true
type = "pgvector"
dimensions = 768
top_k = 10
min_similarity = 0.7

[observability]
otel_enabled = true
otel_exporter = "otlp"
otel_endpoint = "http://localhost:4317"
metrics_port = 9090
tracing_sampling_rate = 0.1
```

### 特性標誌
多用戶服務器模式需要：
- `backend-postgres`：PostgreSQL 數據庫支持
- `postgres`：PostgreSQL 客戶端庫
- `pgvector`：PostgreSQL 的向量擴展

### 運行時配置字段

以下是多用戶模式特有的運行時配置字段：

| 字段 | 描述 | 默認值 |
|-------|-------------|---------|
| `entry_auth_enabled` | 啟用網關 API 金鑰認證 | `false` |
| `entry_auth_api_key_env` | 入口 API 金鑰的環境變量名 | `GO_ON_ENTRY_API_KEY` |
| `entry_rate_limit_rpm` | 每源 IP 的速率限制 | `240` |
| `entry_rate_limit_burst` | 令牌桶突發容量 | `60` |
| `user_auth_enabled` | 啟用每用戶 HMAC 權杖認證 | `false` |
| `user_auth_token_secret` | HMAC 簽名金鑰（配置文件） | `go-on-multi-user-secret` |
| `user_auth_token_secret_env` | HMAC 金鑰的環境變量覆蓋 | `GO_ON_USER_AUTH_TOKEN_SECRET` |
| `user_auth_token_ttl_seconds` | 權杖 TTL（秒） | `86400`（24小時） |
| `cors_allowed_origins` | 允許的 CORS 來源（空 = 禁用） | `[]` |
| `tenant_default_daily_token_limit` | 每租戶每日權杖限制 | `1000000` |
| `tenant_default_concurrent_tasks` | 每租戶最大併發任務數 | `10` |
| `tenant_default_daily_api_calls` | 每租戶每日 API 調用限制 | `10000` |
| `shutdown_drain_seconds` | 優雅關閉排空期 | `30` |

## 安裝

### 為企業部署構建
```bash
# 使用 multi-users-server 配置文件構建
cargo build --no-default-features -F profile-multi-users-server

# 或顯式啟用特性
cargo build --features "backend-postgres postgres pgvector"
```

### 系統要求
- **CPU**：推薦 4+ 核心
- **內存**：8GB+ RAM
- **存儲**：50GB+ 可用空間（需要 SSD）
- **網絡**：高速、可靠的連接
- **數據庫**：PostgreSQL 14+ 帶 pgvector 擴展

### Docker Compose 設置
創建 `docker-compose.yml`：

```yaml
version: '3.8'

services:
  postgres:
    image: pgvector/pgvector:pg16
    environment:
      POSTGRES_DB: go_on_production
      POSTGRES_USER: go_on
      POSTGRES_PASSWORD: ${DB_PASSWORD}
    volumes:
      - postgres_data:/var/lib/postgresql/data
      - ./init.sql:/docker-entrypoint-initdb.d/init.sql
    ports:
      - "5432:5432"
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U go_on"]
      interval: 10s
      timeout: 5s
      retries: 5

  redis:
    image: redis:7-alpine
    ports:
      - "6379:6379"
    volumes:
      - redis_data:/data
    command: redis-server --appendonly yes

  go-on:
    build:
      context: .
      dockerfile: Dockerfile.multi-users
    environment:
      GO_ON_DB_USERNAME: go_on
      GO_ON_DB_PASSWORD: ${DB_PASSWORD}
      GO_ON_ENTRY_API_KEY: ${ENTRY_API_KEY}
      RUST_LOG: info
    ports:
      - "8090:8090"
      - "9090:9090"
    depends_on:
      postgres:
        condition: service_healthy
      redis:
        condition: service_started
    volumes:
      - ./config/multi-users-server.toml:/app/config.toml
      - ./logs:/app/logs

  prometheus:
    image: prom/prometheus:latest
    ports:
      - "9091:9090"
    volumes:
      - ./prometheus.yml:/etc/prometheus/prometheus.yml
      - prometheus_data:/prometheus

  grafana:
    image: grafana/grafana:latest
    ports:
      - "3000:3000"
    environment:
      GF_SECURITY_ADMIN_PASSWORD: ${GRAFANA_PASSWORD}
    volumes:
      - grafana_data:/var/lib/grafana
      - ./grafana/dashboards:/etc/grafana/provisioning/dashboards

volumes:
  postgres_data:
  redis_data:
  prometheus_data:
  grafana_data:
```

## 數據庫設置

### PostgreSQL 配置
創建 `init.sql` 用於數據庫初始化：

```sql
-- 創建數據庫和擴展
CREATE DATABASE go_on_production;
\c go_on_production;

-- 啟用所需擴展
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;
CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- 創建表
CREATE TABLE IF NOT EXISTS cache (
    id SERIAL PRIMARY KEY,
    key VARCHAR(512) UNIQUE NOT NULL,
    value TEXT NOT NULL,
    ttl_seconds INTEGER DEFAULT 3600,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    expires_at TIMESTAMP GENERATED ALWAYS AS (created_at + ttl_seconds * INTERVAL '1 second') STORED
);

CREATE TABLE IF NOT EXISTS vector_store (
    id SERIAL PRIMARY KEY,
    embedding vector(768),
    content TEXT NOT NULL,
    metadata JSONB,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS users (
    id SERIAL PRIMARY KEY,
    username VARCHAR(255) UNIQUE NOT NULL,
    email VARCHAR(255) UNIQUE NOT NULL,
    password_hash VARCHAR(255) NOT NULL,
    role VARCHAR(50) DEFAULT 'user',
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS sessions (
    id SERIAL PRIMARY KEY,
    user_id INTEGER REFERENCES users(id),
    session_token VARCHAR(255) UNIQUE NOT NULL,
    expires_at TIMESTAMP NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS audit_logs (
    id SERIAL PRIMARY KEY,
    user_id INTEGER REFERENCES users(id),
    action VARCHAR(255) NOT NULL,
    resource VARCHAR(255),
    details JSONB,
    ip_address INET,
    user_agent TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- 創建索引
CREATE INDEX idx_cache_key ON cache(key);
CREATE INDEX idx_cache_expires_at ON cache(expires_at);
CREATE INDEX idx_vector_embedding ON vector_store USING ivfflat (embedding vector_cosine_ops);
CREATE INDEX idx_users_username ON users(username);
CREATE INDEX idx_sessions_token ON sessions(session_token);
CREATE INDEX idx_audit_logs_created_at ON audit_logs(created_at);

-- 創建角色和權限
CREATE ROLE go_on_app WITH LOGIN PASSWORD '${APP_DB_PASSWORD}';
GRANT CONNECT ON DATABASE go_on_production TO go_on_app;
GRANT USAGE ON SCHEMA public TO go_on_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO go_on_app;
GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO go_on_app;
```

### 數據庫性能調優
更新 `postgresql.conf`：

```ini
# 連接設置
max_connections = 200
shared_buffers = 2GB
effective_cache_size = 6GB

# 性能設置
maintenance_work_mem = 512MB
checkpoint_completion_target = 0.9
wal_buffers = 16MB
default_statistics_target = 100

# 查詢優化
random_page_cost = 1.1
effective_io_concurrency = 200
work_mem = 32MB
```

## 部署

### Kubernetes 部署
創建 `kubernetes/deployment.yaml`：

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: go-on
  namespace: go-on-production
spec:
  replicas: 3
  selector:
    matchLabels:
      app: go-on
  template:
    metadata:
      labels:
        app: go-on
    spec:
      containers:
      - name: go-on
        image: your-registry/go-on:multi-users-latest
        ports:
        - containerPort: 8090
          name: http
        - containerPort: 9090
          name: metrics
        env:
        - name: GO_ON_DB_USERNAME
          valueFrom:
            secretKeyRef:
              name: go-on-secrets
              key: db-username
        - name: GO_ON_DB_PASSWORD
          valueFrom:
            secretKeyRef:
              name: go-on-secrets
              key: db-password
        - name: GO_ON_ENTRY_API_KEY
          valueFrom:
            secretKeyRef:
              name: go-on-secrets
              key: entry-api-key
        resources:
          requests:
            memory: "2Gi"
            cpu: "1000m"
          limits:
            memory: "4Gi"
            cpu: "2000m"
        livenessProbe:
          httpGet:
            path: /health
            port: 8090
          initialDelaySeconds: 30
          periodSeconds: 10
        readinessProbe:
          httpGet:
            path: /health
            port: 8090
          initialDelaySeconds: 5
          periodSeconds: 5
---
apiVersion: v1
kind: Service
metadata:
  name: go-on-service
  namespace: go-on-production
spec:
  selector:
    app: go-on
  ports:
  - port: 80
    targetPort: 8090
    name: http
  - port: 9090
    targetPort: 9090
    name: metrics
  type: LoadBalancer
```

### 負載均衡器配置
創建 `nginx/load-balancer.conf`：

```nginx
upstream go_on_backend {
    least_conn;
    server go-on-1:8090;
    server go-on-2:8090;
    server go-on-3:8090;
    keepalive 32;
}

server {
    listen 443 ssl http2;
    server_name go-on.example.com;
    
    ssl_certificate /etc/ssl/certs/go-on.crt;
    ssl_certificate_key /etc/ssl/private/go-on.key;
    
    # SSL 配置
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_ciphers ECDHE-RSA-AES256-GCM-SHA512:DHE-RSA-AES256-GCM-SHA512;
    ssl_prefer_server_ciphers off;
    
    location / {
        proxy_pass http://go_on_backend;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection 'upgrade';
        proxy_set_header Host $host;
        proxy_cache_bypass $http_upgrade;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        
        # 超時設置
        proxy_connect_timeout 60s;
        proxy_send_timeout 60s;
        proxy_read_timeout 60s;
    }
    
    location /health {
        proxy_pass http://go_on_backend/health;
        access_log off;
    }
    
    location /metrics {
        proxy_pass http://go_on_backend/metrics;
        access_log off;
    }
}
```

## 安全

### 認證和授權

多用戶服務器提供**雙層認證**系統：

**第一層：網關入口認證（共享 API 金鑰）**
- 對每個 HTTP 請求驗證預先共享的 API 金鑰
- 金鑰來自 `entry_auth_api_key_env` 指定的環境變量（默認：`GO_ON_ENTRY_API_KEY`）
- 接受的標頭：`Authorization: Bearer <key>`、`X-Api-Key`、`X-Go-On-Key`
- 如果缺失/無效，返回 401 `ENTRY_AUTH_REQUIRED`
- 如果環境變量未設置，返回 503 `ENTRY_AUTH_MISCONFIGURED`

**第二層：用戶認證（HMAC-SHA256 權杖）**
- 發行使用 HMAC-SHA256 簽名的每用戶權杖
- 權杖格式：`user_id:base64_hmac:expires_at_ms`
- 權杖金鑰解析順序：環境變量（`user_auth_token_secret_env`）> 配置文件 > 默認值
- 自動配置：有效權杖無需預註冊即可獲得會話
- 降級回退：當 `user_auth_enabled=false` 時，所有請求視為管理員

**RBAC 授權**
- 內置角色：`admin`（完全訪問）、`user`（R/W/X）、`viewer`（唯讀）、`monitor`
- 權限映射：`GET` → 讀取，`POST /chat` → 執行，`POST /rpc` → 執行
- 在 ACP+HTTP 和 MCP+HTTP 路徑上一致應用
- 返回 401 `AUTH_REQUIRED`（無會話）、403 `ACCESS_DENIED`（無權限）、403 `PRIVILEGE_ESCALATION_REQUIRED`（角色不足）

**免認證端點：** `/` 和 `/health` 繞過所有認證。

### 發行用戶權杖

用戶權杖可以通過內部 `SessionManager` API 編程方式發行。權杖格式為 `user_id:base64_hmac_sig:expires_at_ms`：

```rust
// Rust：編程方式發行權杖
use crate::acp::r#impl::session::{AuthConfig, SessionManager};

let auth_cfg = AuthConfig::from(&runtime_config);
let mgr = SessionManager::with_auth_config(auth_cfg);
let token = mgr.issue_token("alice", &["admin"], None, 86400).unwrap();
// 返回："alice:<base64_hmac>:<expires_at_ms>"
```

### CORS 配置

允許的來源默認為 `[]`（空 = 禁用 CORS）。要啟用跨域請求：

```toml
[runtime]
cors_allowed_origins = ["https://my-frontend.example.com"]
# 多個來源：
# cors_allowed_origins = ["https://app1.example.com", "https://app2.example.com"]
# 僅用於開發：
# cors_allowed_origins = ["*"]
```

配置後，所有 HTTP 響應（JSON、SSE、錯誤）都會自動包含 CORS 標頭。

### 租戶預算控制

啟用用戶認證後，系統會自動配置默認租戶配額：

```toml
[runtime]
tenant_default_daily_token_limit = 1000000
tenant_default_concurrent_tasks = 10
tenant_default_daily_api_calls = 10000
```

對話 ID 使用租戶 ID 前綴進行命名空間隔離，以防止跨用戶數據洩露：
```
內部 conversation_id = "tenant_id:user_provided_conversation_id"
```

### 對話隔離

所有對話狀態按租戶隔離：
- 當 `user_auth_enabled` 時，對話 ID 以 `tenant_id:` 為前綴
- 預算控制使用用戶會話中的 `tenant_id`（而非原始的 `conversation_id`）
- 防止跨用戶對話歷史洩露

### 執行緒安全金鑰管理

項目將所有 `std::env::set_var()` 調用（在多執行緒上下文中已記錄為**未定義行為**）替換為內存中的 `HashMap`：

```rust
// 執行緒安全：設置金鑰覆蓋
crate::shared::secret_override::set_secret_override("GITHUB_TOKEN", "ghp_xxx");

// 執行緒安全：讀取（先檢查覆蓋映射，再檢查環境變量）
let value = crate::shared::secret_override::get_secret("GITHUB_TOKEN");
```

金鑰環查找緩存 30 秒 TTL，以避免在異步上下文中阻塞 I/O：

```rust
// 使用 30 秒 TTL 的緩存
let value = crate::shared::secret_override::get_keyring_cached("go-on", "copilot_api_key");
```

**安全合規：**
- ✅ 生產代碼中 0 個 `std::env::set_var()` 調用
- ✅ 0 個 `.expect("lock poisoned")` panic——所有鎖中毒通過 `unwrap_or_else(|e| e.into_inner())` 恢復
- ✅ 生產代碼中 0 個 `unsafe` 塊
- ✅ 所有 HTTP 響應（JSON、SSE、錯誤）包含 CORS 標頭
- ✅ MCP HTTP 服務器與 ACP HTTP 服務器共享相同的認證管道

### 網絡安全
```bash
# 防火牆規則
sudo ufw allow 443/tcp  # HTTPS
sudo ufw allow 5432/tcp  # PostgreSQL
sudo ufw allow 6379/tcp  # Redis
sudo ufw allow 8090/tcp  # go-on ACP/MCP HTTP

# 啟用防火牆
sudo ufw --force enable
```

### 密鑰管理
```bash
# 設置所需的環境變量
export GO_ON_ENTRY_API_KEY=$(openssl rand -base64 48)
export GO_ON_USER_AUTH_TOKEN_SECRET=$(openssl rand -base64 64)

# 可選：PostgreSQL 憑據
export GO_ON_DB_USERNAME="go_on"
export GO_ON_DB_PASSWORD=$(openssl rand -base64 32)
```

## 監控和可觀測性

### Prometheus 配置
創建 `prometheus.yml`：

```yaml
global:
  scrape_interval: 15s
  evaluation_interval: 15s

scrape_configs:
  - job_name: 'go-on'
    static_configs:
      - targets: ['go-on:9090']
    metrics_path: '/metrics/prometheus'
    
  - job_name: 'postgres'
    static_configs:
      - targets: ['postgres-exporter:9187']
      
  - job_name: 'redis'
    static_configs:
      - targets: ['redis-exporter:9121']

alerting:
  alertmanagers:
    - static_configs:
        - targets: ['alertmanager:9093']

rule_files:
  - 'alerts.yml'
```

### Grafana 儀表板
創建 `grafana/dashboards/go-on.json` 包含關鍵指標：
- 請求率和延遲
- 錯誤率和類型
- 數據庫性能
- 緩存命中率
- 用戶活動
- 資源利用率

### 日誌聚合
```toml
[logging]
level = "info"
format = "json"
outputs = ["file", "stdout", "loki"]

[logging.loki]
enabled = true
url = "http://loki:3100"
labels = ["app=go-on", "environment=production"]
batch_size = 1000
batch_timeout_seconds = 5
```

## 備份和災難恢復

### 數據庫備份
```bash
#!/bin/bash
# backup.sh
BACKUP_DIR="/backup/go-on"
DATE=$(date +%Y%m%d_%H%M%S)

# 備份 PostgreSQL
pg_dump -h localhost -U go_on -d go_on_production \
  --format=custom \
  --file="$BACKUP_DIR/go-on-$DATE.dump"

# 備份配置
cp /opt/go-on/config/multi-users-server.toml "$BACKUP_DIR/config-$DATE.toml"

# 上傳到雲存儲
aws s3 cp "$BACKUP_DIR/go-on-$DATE.dump" s3://go-on-backups/
aws s3 cp "$BACKUP_DIR/config-$DATE.toml" s3://go-on-backups/

# 清理舊備份（保留 30 天）
find $BACKUP_DIR -name "*.dump" -mtime +30 -delete
find $BACKUP_DIR -name "*.toml" -mtime +30 -delete
```

### 災難恢復計劃
1. **恢復時間目標 (RTO)**：1 小時
2. **恢復點目標 (RPO)**：15 分鐘
3. **備份頻率**：每小時增量，每日完整
4. **測試**：每月恢復演練

## 擴展

### 水平擴展
```yaml
# Kubernetes 水平 Pod 自動擴展器
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: go-on-hpa
  namespace: go-on-production
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: go-on
  minReplicas: 3
  maxReplicas: 10
  metrics:
  - type: Resource
    resource:
      name: cpu
      target:
        type: Utilization
        averageUtilization: 70
  - type: Resource
    resource:
      name: memory
      target:
        type: Utilization
        averageUtilization: 80
```

### 數據庫擴展
```sql
-- 用於擴展讀取的只讀副本
CREATE PUBLICATION go_on_publication FOR ALL TABLES;
CREATE SUBSCRIPTION go_on_subscription
  CONNECTION 'host=replica-server port=5432 dbname=go_on_production user=replication_user password=secret'
  PUBLICATION go_on_publication;

-- 大表分區
CREATE TABLE cache_partitioned (
    LIKE cache INCLUDING ALL
) PARTITION BY RANGE (created_at);

CREATE TABLE cache_2024_q1 PARTITION OF cache_partitioned
    FOR VALUES FROM ('2024-01-01') TO ('2024-04-01');
CREATE TABLE cache_2024_q2 PARTITION OF cache_partitioned
    FOR VALUES FROM ('2024-04-01') TO ('2024-07-01');
```

## 遷移

### 從簡單服務器模式遷移
```bash
# 從簡單服務器導出數據
pg_dump -h simple-server -U go_on -d go_on_simple \
  --format=custom \
  --file=simple-server-export.dump

# 導入到多用戶服務器
pg_restore -h multi-users-server -U go_on -d go_on_production \
  --clean \
  --if-exists \
  simple-server-export.dump

# 更新配置
cp config/simple-server.toml config/multi-users-server.toml
# 在新配置中更新數據庫設置
```

### 零停機遷移
1. **階段 1**：在舊系統旁邊設置新基礎設施
2. **階段 2**：開始向兩個系統雙重寫入
3. **階段 3**：將讀取流量遷移到新系統
4. **階段 4**：將寫入流量遷移到新系統
5. **階段 5**：停用舊系統

## 故障排除

### 性能問題

#### 數據庫瓶頸
```sql
-- 識別慢查詢
SELECT query, calls, total_time, mean_time
FROM pg_stat_statements
ORDER BY mean_time DESC
LIMIT 10;

-- 檢查表大小
SELECT schemaname, tablename, pg_size_pretty(pg_total_relation_size(schemaname||'.'||tablename))
FROM pg_tables
WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
ORDER BY pg_total_relation_size(schemaname||'.'||tablename) DESC;

-- 檢查索引使用情況
SELECT schemaname, tablename, indexname, idx_scan, idx_tup_read, idx_tup_fetch
FROM pg_stat_user_indexes
ORDER BY idx_scan;
```

#### 應用問題
```bash
# 檢查應用日誌
kubectl logs -l app=go-on --tail=100
kubectl logs -l app=go-on --since=1h

# 檢查資源使用情況
kubectl top pods -l app=go-on
kubectl describe pods -l app=go-on

# 檢查網絡連接
kubectl exec go-on-pod -- curl http://postgres:5432
kubectl exec go-on-pod -- curl http://redis:6379
```

### 安全問題

#### 審計日誌審查
```sql
-- 審查最近的安全事件
SELECT *
FROM audit_logs
WHERE action IN ('login_failed', 'access_denied', 'privilege_escalation')
ORDER BY created_at DESC
LIMIT 100;

-- 檢查可疑活動
SELECT user_id, COUNT(*) as failed_attempts, MIN(created_at) as first_attempt, MAX(created_at) as last_attempt
FROM audit_logs
WHERE action = 'login_failed'
  AND created_at > NOW() - INTERVAL '1 hour'
GROUP BY user_id
HAVING COUNT(*) > 5;
```

#### 安全掃描
```bash
# 運行漏洞掃描
trivy image your-registry/go-on:multi-users-latest

# 檢查代碼中的密鑰
gitleaks detect --source . --verbose

# 網絡安全掃描
nmap -sV -p 80,443,5432,6379,8090,9090 go-on.example.com
```

## 維護

### 定期維護任務
```bash
#!/bin/bash
# maintenance.sh

# 1. 數據庫維護
psql -h localhost -U go_on -d go_on_production -c "VACUUM ANALYZE;"
psql -h localhost -U go_on -d go_on_production -c "REINDEX DATABASE go_on_production;"

# 2. 日誌輪轉
find /var/log/go-on -name "*.log" -mtime +7 -delete

# 3. 緩存清理
redis-cli FLUSHDB

# 4. 備份驗證
# （驗證最新備份可以恢復）
```

### 更新流程
1. **更新前檢查清單**：
   - 驗證備份是最新的
   - 通知用戶維護窗口
   - 準備回滾計劃

2. **更新過程**：
   ```bash
   # 1. 部署新版本到暫存環境
   kubectl apply -f kubernetes/staging/
   
   # 2. 運行冒煙測試
   ./scripts/smoke-tests.sh
   
   # 3. 部署到生產環境（金絲雀）
   kubectl set image deployment/go-on go-on=your-registry/go-on:new-version
   
   # 4. 監控指標
   watch kubectl get pods -l app=go-on
   
   # 5. 完全推出
   kubectl rollout status deployment/go-on
   ```

3. **更新後驗證**：
   - 驗證所有服務健康
   - 檢查錯誤率和性能
   - 確認用戶功能正常

## 成本優化

### 資源優化
```yaml
# Kubernetes 資源請求和限制
resources:
  requests:
    memory: "1Gi"
    cpu: "500m"
  limits:
    memory: "2Gi"
    cpu: "1000m"
```

### 自動擴展策略
- 在非高峰時段縮減規模
- 對非關鍵工作負載使用競價實例
- 實施成本感知調度

### 監控成本
- 跟蹤數據庫存儲增長
- 監控模型供應商的 API 調用成本
- 對意外成本激增發出警報

## 支持和社區

### 企業支持
- **SLA**：99.9% 正常運行時間保證
- **支持渠道**：電子郵件、電話、專用 Slack
- **響應時間**：關鍵：1 小時，高：4 小時，正常：24 小時

### 社區資源
- [GitHub 討論](https://github.com/your-org/go-on/discussions)
- [文檔](https://go-on.example.com/docs)
- [Stack Overflow](https://stackoverflow.com/questions/tagged/go-on)

### 培訓和認證
- 管理員培訓課程
- 開發者認證計劃
- 企業團隊的定製培訓

## 下一步

設置多用戶服務器模式後，您可以：
1. 配置 [高可用性](./high-availability.md)
2. 設置 [災難恢復](./disaster-recovery.md)
3. 實施 [高級安全](./advanced-security.md)
4. 探索 [API 文檔](../api/overview.md)
5. 加入 [企業社區](https://github.com/your-org/go-on/discussions/categories/enterprise)