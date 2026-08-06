# 多用户服务器模式部署

## 概述

多用户服务器模式（`multi-users-server`）是 go-on 的企业级部署配置，专为具有多个并发用户的生产环境设计。它使用 PostgreSQL 和 pgvector 进行可扩展存储，并提供安全、监控和高可用性的高级功能。

## 特性

### 企业能力
- **多用户支持**：专为多个用户并发访问设计
- **PostgreSQL 存储**：带 pgvector 扩展的可扩展数据库
- **CORS 支持**：可配置的允许来源、预检（OPTIONS）处理、所有 HTTP/SSE 响应上的 CORS 标头
- **入口认证（网关）**：通过 `Authorization: Bearer`、`X-Api-Key` 或 `X-Go-On-Key` 标头验证的共享 API 密钥
- **用户认证（HMAC 令牌）**：基于 HMAC-SHA256 签名的每用户 JWT 类令牌，自动配置，可配置 TTL
- **RBAC 授权**：基于角色的访问控制（admin/user/viewer/monitor），在 ACP+HTTP 和 MCP+HTTP 路径上均进行按端点权限检查
- **租户预算控制**：每个租户的每日令牌/并发任务/API 调用配额，自动配置
- **对话隔离**：带租户前缀的命名空间对话 ID，防止跨用户数据泄露
- **关闭排空**：优雅关闭期间处理进行中连接的可配置排空期
- **信号处理**：所有平台上的 SIGINT（Ctrl+C）和 SIGTERM，实现干净关闭
- **线程安全密钥管理**：`SECRET_OVERRIDE_MAP` 替代 `std::env::set_var()`（在多线程上下文中已记录为未定义行为）；`KEYRING_CACHE` 带 30 秒 TTL，避免在异步热路径中阻塞密钥环 I/O
- **热重载配置**：通过 `config.reload` RPC 进行运行时配置重载
- **高可用性**：内置冗余和故障转移支持
- **企业监控**：全面的可观测性堆栈
- **可扩展性**：水平扩展能力
- **完整 Phase 4 架构**：全部 7 个特性门控子总线和 21 个 F-GAP 模块
- **分布式内存总线**：通过 DistributedMemoryBus 跨节点记忆共享
- **跨节点容错引擎**：跨节点故障隔离和自动恢复
- **多渠道消息传输**：6 通道、QoS 启用的消息传输

### 架构
```
多用户服务器架构：
├── 应用层：go-on 运行时实例
│   ├── ACP HTTP 服务器（端口 8090）：CORS → 入口认证 → 用户认证 → RBAC → 路由
│   └── MCP HTTP 服务器（端口 8090）：CORS → 入口认证 → 用户认证 → RBAC → 分发
├── 数据库层：带 pgvector 的 PostgreSQL
├── 缓存层：Redis（可选）
├── 负载均衡器：流量分发
├── 监控：Prometheus、Grafana、ELK
└── 备份：自动化备份系统

请求流程（ACP+HTTP）：
客户端 → CORS 标头 → 入口认证（API 密钥）→ 用户会话（HMAC 令牌）
       → RBAC 授权 → 租户预算检查 → 命名空间对话
       → AI 处理 → 带 CORS 标头的响应

请求流程（MCP+HTTP）：
客户端 → CORS 标头 → 入口认证（API 密钥）→ 用户会话（HMAC 令牌）
       → RBAC 授权 → MCP 方法分发 → 带 CORS 的 JSON-RPC 响应
```

## 配置

### 服务器配置
创建 `config/multi-users-server.toml`：

```toml
# config/multi-users-server.toml
default_phase = "coding"
model_selection_mode = "adaptive"

[protocol]
mode = "acp_http"  # 用于多用户访问的 ACP over HTTP（使用 mcp_http 用于 MCP）

[runtime]
acp_http_bind_addr = "0.0.0.0:8090"
production_strict = true

# ── 网关入口认证（所有入站流量的共享 API 密钥）────
entry_auth_enabled = true
entry_auth_api_key_env = "GO_ON_ENTRY_API_KEY"
entry_rate_limit_rpm = 5000
entry_rate_limit_burst = 1000

# ── 用户认证（每用户 HMAC 令牌）─────────────
user_auth_enabled = true
user_auth_token_secret_env = "GO_ON_USER_AUTH_TOKEN_SECRET"
user_auth_token_ttl_seconds = 86400

# ── 跨域资源共享（CORS）───────────────────
cors_allowed_origins = ["https://my-frontend.example.com"]
# 仅用于开发使用 "*"：
# cors_allowed_origins = ["*"]

# ── 租户资源配额 ────────────────────────────
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
type = "database"  # 使用 PostgreSQL 作为缓存

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

### 特性标志
多用户服务器模式需要：
- `backend-postgres`：PostgreSQL 数据库支持
- `postgres`：PostgreSQL 客户端库
- `pgvector`：PostgreSQL 的向量扩展

### 运行时配置字段

以下是多用户模式特有的运行时配置字段：

| 字段 | 描述 | 默认值 |
|-------|-------------|---------|
| `entry_auth_enabled` | 启用网关 API 密钥认证 | `false` |
| `entry_auth_api_key_env` | 入口 API 密钥的环境变量名 | `GO_ON_ENTRY_API_KEY` |
| `entry_rate_limit_rpm` | 每源 IP 的速率限制 | `240` |
| `entry_rate_limit_burst` | 令牌桶突发容量 | `60` |
| `user_auth_enabled` | 启用每用户 HMAC 令牌认证 | `false` |
| `user_auth_token_secret` | HMAC 签名密钥（配置文件） | `go-on-multi-user-secret` |
| `user_auth_token_secret_env` | HMAC 密钥的环境变量覆盖 | `GO_ON_USER_AUTH_TOKEN_SECRET` |
| `user_auth_token_ttl_seconds` | 令牌 TTL（秒） | `86400`（24小时） |
| `cors_allowed_origins` | 允许的 CORS 来源（空 = 禁用） | `[]` |
| `tenant_default_daily_token_limit` | 每租户每日令牌限制 | `1000000` |
| `tenant_default_concurrent_tasks` | 每租户最大并发任务数 | `10` |
| `tenant_default_daily_api_calls` | 每租户每日 API 调用限制 | `10000` |
| `shutdown_drain_seconds` | 优雅关闭排空期 | `30` |

## 安装

### 为企业部署构建
```bash
# 使用 multi-users-server 配置文件构建
cargo build --no-default-features -F multi-users-server

# 或显式启用特性
cargo build --features "backend-postgres postgres pgvector"
```

### 系统要求
- **CPU**：推荐 4+ 核心
- **内存**：8GB+ RAM
- **存储**：50GB+ 可用空间（需要 SSD）
- **网络**：高速、可靠的连接
- **数据库**：PostgreSQL 14+ 带 pgvector 扩展

### Docker Compose 设置
创建 `docker-compose.yml`：

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

## 数据库设置

### PostgreSQL 配置
创建 `init.sql` 用于数据库初始化：

```sql
-- 创建数据库和扩展
CREATE DATABASE go_on_production;
\c go_on_production;

-- 启用所需扩展
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;
CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- 创建表
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

-- 创建索引
CREATE INDEX idx_cache_key ON cache(key);
CREATE INDEX idx_cache_expires_at ON cache(expires_at);
CREATE INDEX idx_vector_embedding ON vector_store USING ivfflat (embedding vector_cosine_ops);
CREATE INDEX idx_users_username ON users(username);
CREATE INDEX idx_sessions_token ON sessions(session_token);
CREATE INDEX idx_audit_logs_created_at ON audit_logs(created_at);

-- 创建角色和权限
CREATE ROLE go_on_app WITH LOGIN PASSWORD '${APP_DB_PASSWORD}';
GRANT CONNECT ON DATABASE go_on_production TO go_on_app;
GRANT USAGE ON SCHEMA public TO go_on_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO go_on_app;
GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO go_on_app;
```

### 数据库性能调优
更新 `postgresql.conf`：

```ini
# 连接设置
max_connections = 200
shared_buffers = 2GB
effective_cache_size = 6GB

# 性能设置
maintenance_work_mem = 512MB
checkpoint_completion_target = 0.9
wal_buffers = 16MB
default_statistics_target = 100

# 查询优化
random_page_cost = 1.1
effective_io_concurrency = 200
work_mem = 32MB
```

## 部署

### Kubernetes 部署
创建 `kubernetes/deployment.yaml`：

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

### 负载均衡器配置
创建 `nginx/load-balancer.conf`：

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
        
        # 超时设置
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

### 认证和授权

多用户服务器提供**双层认证**系统：

**第一层：网关入口认证（共享 API 密钥）**
- 对每个 HTTP 请求验证预先共享的 API 密钥
- 密钥来自 `entry_auth_api_key_env` 指定的环境变量（默认：`GO_ON_ENTRY_API_KEY`）
- 接受的标头：`Authorization: Bearer <key>`、`X-Api-Key`、`X-Go-On-Key`
- 如果缺失/无效，返回 401 `ENTRY_AUTH_REQUIRED`
- 如果环境变量未设置，返回 503 `ENTRY_AUTH_MISCONFIGURED`

**第二层：用户认证（HMAC-SHA256 令牌）**
- 发行使用 HMAC-SHA256 签名的每用户令牌
- 令牌格式：`user_id:base64_hmac:expires_at_ms`
- 令牌密钥解析顺序：环境变量（`user_auth_token_secret_env`）> 配置文件 > 默认值
- 自动配置：有效令牌无需预注册即可获得会话
- 降级回退：当 `user_auth_enabled=false` 时，所有请求视为管理员

**RBAC 授权**
- 内置角色：`admin`（完全访问）、`user`（R/W/X）、`viewer`（只读）、`monitor`
- 权限映射：`GET` → 读取，`POST /chat` → 执行，`POST /rpc` → 执行
- 在 ACP+HTTP 和 MCP+HTTP 路径上一致应用
- 返回 401 `AUTH_REQUIRED`（无会话）、403 `ACCESS_DENIED`（无权限）、403 `PRIVILEGE_ESCALATION_REQUIRED`（角色不足）

**免认证端点：** `/` 和 `/health` 绕过所有认证。

### 发行用户令牌

用户令牌可以通过内部 `SessionManager` API 编程方式发行。令牌格式为 `user_id:base64_hmac_sig:expires_at_ms`：

```rust
// Rust：编程方式发行令牌
use crate::acp::r#impl::session::{AuthConfig, SessionManager};

let auth_cfg = AuthConfig::from(&runtime_config);
let mgr = SessionManager::with_auth_config(auth_cfg);
let token = mgr.issue_token("alice", &["admin"], None, 86400).unwrap();
// 返回："alice:<base64_hmac>:<expires_at_ms>"
```

### CORS 配置

允许的来源默认为 `[]`（空 = 禁用 CORS）。要启用跨域请求：

```toml
[runtime]
cors_allowed_origins = ["https://my-frontend.example.com"]
# 多个来源：
# cors_allowed_origins = ["https://app1.example.com", "https://app2.example.com"]
# 仅用于开发：
# cors_allowed_origins = ["*"]
```

配置后，所有 HTTP 响应（JSON、SSE、错误）都会自动包含 CORS 标头。

### 租户预算控制

启用用户认证后，系统会自动配置默认租户配额：

```toml
[runtime]
tenant_default_daily_token_limit = 1000000
tenant_default_concurrent_tasks = 10
tenant_default_daily_api_calls = 10000
```

对话 ID 使用租户 ID 前缀进行命名空间隔离，以防止跨用户数据泄露：
```
内部 conversation_id = "tenant_id:user_provided_conversation_id"
```

### 对话隔离

所有对话状态按租户隔离：
- 当 `user_auth_enabled` 时，对话 ID 以 `tenant_id:` 为前缀
- 预算控制使用用户会话中的 `tenant_id`（而非原始的 `conversation_id`）
- 防止跨用户对话历史泄露

### 线程安全密钥管理

项目将所有 `std::env::set_var()` 调用（在多线程上下文中已记录为**未定义行为**）替换为内存中的 `HashMap`：

```rust
// 线程安全：设置密钥覆盖
crate::shared::secret_override::set_secret_override("GITHUB_TOKEN", "ghp_xxx");

// 线程安全：读取（先检查覆盖映射，再检查环境变量）
let value = crate::shared::secret_override::get_secret("GITHUB_TOKEN");
```

密钥环查找缓存 30 秒 TTL，以避免在异步上下文中阻塞 I/O：

```rust
// 使用 30 秒 TTL 的缓存
let value = crate::shared::secret_override::get_keyring_cached("go-on", "copilot_api_key");
```

**安全合规：**
- ✅ 生产代码中 0 个 `std::env::set_var()` 调用
- ✅ 0 个 `.expect("lock poisoned")` panic——所有锁中毒通过 `unwrap_or_else(|e| e.into_inner())` 恢复
- ✅ 生产代码中 0 个 `unsafe` 块
- ✅ 所有 HTTP 响应（JSON、SSE、错误）包含 CORS 标头
- ✅ MCP HTTP 服务器与 ACP HTTP 服务器共享相同的认证管道

### 网络安全
```bash
# 防火墙规则
sudo ufw allow 443/tcp  # HTTPS
sudo ufw allow 5432/tcp  # PostgreSQL
sudo ufw allow 6379/tcp  # Redis
sudo ufw allow 8090/tcp  # go-on ACP/MCP HTTP

# 启用防火墙
sudo ufw --force enable
```

### 密钥管理
```bash
# 设置所需的环境变量
export GO_ON_ENTRY_API_KEY=$(openssl rand -base64 48)
export GO_ON_USER_AUTH_TOKEN_SECRET=$(openssl rand -base64 64)

# 可选：PostgreSQL 凭据
export GO_ON_DB_USERNAME="go_on"
export GO_ON_DB_PASSWORD=$(openssl rand -base64 32)
```

## 监控和可观测性

### Prometheus 配置
创建 `prometheus.yml`：

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

### Grafana 仪表板
创建 `grafana/dashboards/go-on.json` 包含关键指标：
- 请求率和延迟
- 错误率和类型
- 数据库性能
- 缓存命中率
- 用户活动
- 资源利用率

### 日志聚合
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

## 备份和灾难恢复

### 数据库备份
```bash
#!/bin/bash
# backup.sh
BACKUP_DIR="/backup/go-on"
DATE=$(date +%Y%m%d_%H%M%S)

# 备份 PostgreSQL
pg_dump -h localhost -U go_on -d go_on_production \
  --format=custom \
  --file="$BACKUP_DIR/go-on-$DATE.dump"

# 备份配置
cp /opt/go-on/config/multi-users-server.toml "$BACKUP_DIR/config-$DATE.toml"

# 上传到云存储
aws s3 cp "$BACKUP_DIR/go-on-$DATE.dump" s3://go-on-backups/
aws s3 cp "$BACKUP_DIR/config-$DATE.toml" s3://go-on-backups/

# 清理旧备份（保留 30 天）
find $BACKUP_DIR -name "*.dump" -mtime +30 -delete
find $BACKUP_DIR -name "*.toml" -mtime +30 -delete
```

### 灾难恢复计划
1. **恢复时间目标 (RTO)**：1 小时
2. **恢复点目标 (RPO)**：15 分钟
3. **备份频率**：每小时增量，每日完整
4. **测试**：每月恢复演练

## 扩展

### 水平扩展
```yaml
# Kubernetes 水平 Pod 自动扩展器
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

### 数据库扩展
```sql
-- 用于扩展读取的只读副本
CREATE PUBLICATION go_on_publication FOR ALL TABLES;
CREATE SUBSCRIPTION go_on_subscription
  CONNECTION 'host=replica-server port=5432 dbname=go_on_production user=replication_user password=secret'
  PUBLICATION go_on_publication;

-- 大表分区
CREATE TABLE cache_partitioned (
    LIKE cache INCLUDING ALL
) PARTITION BY RANGE (created_at);

CREATE TABLE cache_2024_q1 PARTITION OF cache_partitioned
    FOR VALUES FROM ('2024-01-01') TO ('2024-04-01');
CREATE TABLE cache_2024_q2 PARTITION OF cache_partitioned
    FOR VALUES FROM ('2024-04-01') TO ('2024-07-01');
```

## 迁移

### 从简单服务器模式迁移
```bash
# 从简单服务器导出数据
pg_dump -h simple-server -U go_on -d go_on_simple \
  --format=custom \
  --file=simple-server-export.dump

# 导入到多用户服务器
pg_restore -h multi-users-server -U go_on -d go_on_production \
  --clean \
  --if-exists \
  simple-server-export.dump

# 更新配置
cp config/simple-server.toml config/multi-users-server.toml
# 在新配置中更新数据库设置
```

### 零停机迁移
1. **阶段 1**：在旧系统旁边设置新基础设施
2. **阶段 2**：开始向两个系统双重写入
3. **阶段 3**：将读取流量迁移到新系统
4. **阶段 4**：将写入流量迁移到新系统
5. **阶段 5**：停用旧系统

## 故障排除

### 性能问题

#### 数据库瓶颈
```sql
-- 识别慢查询
SELECT query, calls, total_time, mean_time
FROM pg_stat_statements
ORDER BY mean_time DESC
LIMIT 10;

-- 检查表大小
SELECT schemaname, tablename, pg_size_pretty(pg_total_relation_size(schemaname||'.'||tablename))
FROM pg_tables
WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
ORDER BY pg_total_relation_size(schemaname||'.'||tablename) DESC;

-- 检查索引使用情况
SELECT schemaname, tablename, indexname, idx_scan, idx_tup_read, idx_tup_fetch
FROM pg_stat_user_indexes
ORDER BY idx_scan;
```

#### 应用问题
```bash
# 检查应用日志
kubectl logs -l app=go-on --tail=100
kubectl logs -l app=go-on --since=1h

# 检查资源使用情况
kubectl top pods -l app=go-on
kubectl describe pods -l app=go-on

# 检查网络连接
kubectl exec go-on-pod -- curl http://postgres:5432
kubectl exec go-on-pod -- curl http://redis:6379
```

### 安全问题

#### 审计日志审查
```sql
-- 审查最近的安全事件
SELECT *
FROM audit_logs
WHERE action IN ('login_failed', 'access_denied', 'privilege_escalation')
ORDER BY created_at DESC
LIMIT 100;

-- 检查可疑活动
SELECT user_id, COUNT(*) as failed_attempts, MIN(created_at) as first_attempt, MAX(created_at) as last_attempt
FROM audit_logs
WHERE action = 'login_failed'
  AND created_at > NOW() - INTERVAL '1 hour'
GROUP BY user_id
HAVING COUNT(*) > 5;
```

#### 安全扫描
```bash
# 运行漏洞扫描
trivy image your-registry/go-on:multi-users-latest

# 检查代码中的密钥
gitleaks detect --source . --verbose

# 网络安全扫描
nmap -sV -p 80,443,5432,6379,8090,9090 go-on.example.com
```

## 维护

### 定期维护任务
```bash
#!/bin/bash
# maintenance.sh

# 1. 数据库维护
psql -h localhost -U go_on -d go_on_production -c "VACUUM ANALYZE;"
psql -h localhost -U go_on -d go_on_production -c "REINDEX DATABASE go_on_production;"

# 2. 日志轮转
find /var/log/go-on -name "*.log" -mtime +7 -delete

# 3. 缓存清理
redis-cli FLUSHDB

# 4. 备份验证
# （验证最新备份可以恢复）
```

### 更新流程
1. **更新前检查清单**：
   - 验证备份是最新的
   - 通知用户维护窗口
   - 准备回滚计划

2. **更新过程**：
   ```bash
   # 1. 部署新版本到暂存环境
   kubectl apply -f kubernetes/staging/
   
   # 2. 运行冒烟测试
   ./scripts/smoke-tests.sh
   
   # 3. 部署到生产环境（金丝雀）
   kubectl set image deployment/go-on go-on=your-registry/go-on:new-version
   
   # 4. 监控指标
   watch kubectl get pods -l app=go-on
   
   # 5. 完全推出
   kubectl rollout status deployment/go-on
   ```

3. **更新后验证**：
   - 验证所有服务健康
   - 检查错误率和性能
   - 确认用户功能正常

## 成本优化

### 资源优化
```yaml
# Kubernetes 资源请求和限制
resources:
  requests:
    memory: "1Gi"
    cpu: "500m"
  limits:
    memory: "2Gi"
    cpu: "1000m"
```

### 自动扩展策略
- 在非高峰时段缩减规模
- 对非关键工作负载使用竞价实例
- 实施成本感知调度

### 监控成本
- 跟踪数据库存储增长
- 监控模型供应商的 API 调用成本
- 对意外成本激增发出警报

## 支持和社区

### 企业支持
- **SLA**：99.9% 正常运行时间保证
- **支持渠道**：电子邮件、电话、专用 Slack
- **响应时间**：关键：1 小时，高：4 小时，正常：24 小时

### 社区资源
- [GitHub 讨论](https://github.com/your-org/go-on/discussions)
- [文档](https://go-on.example.com/docs)
- [Stack Overflow](https://stackoverflow.com/questions/tagged/go-on)

### 培训和认证
- 管理员培训课程
- 开发者认证计划
- 企业团队的定制培训

## 下一步

设置多用户服务器模式后，您可以：
1. 配置 [高可用性](./high-availability.md)
2. 设置 [灾难恢复](./disaster-recovery.md)
3. 实施 [高级安全](./advanced-security.md)
4. 探索 [API 文档](../api/overview.md)
5. 加入 [企业社区](https://github.com/your-org/go-on/discussions/categories/enterprise)