# Multi-Users Server Mode Deployment

## Overview

Multi-Users Server mode (`multi-users-server`) is the enterprise-grade build profile for go-on, designed for production environments with multiple concurrent users. It uses PostgreSQL with pgvector for scalable storage and provides advanced features for security, monitoring, and high availability.

## Features

### Enterprise Capabilities
- **Multi-user support**: Designed for concurrent access by multiple users
- **PostgreSQL storage**: Scalable database with pgvector extension
- **CORS support**: Configurable allowed origins, preflight (OPTIONS) handling, CORS headers on all HTTP/SSE responses
- **Entry auth (gateway)**: Shared API key validated via `Authorization: Bearer`, `X-Api-Key`, or `X-Go-On-Key` headers
- **User auth (HMAC tokens)**: Per-user JWT-like tokens with HMAC-SHA256 signing, auto-provisioning, configurable TTL
- **RBAC authorization**: Role-based access control (admin/user/viewer/monitor) with per-endpoint permission checks across both ACP+HTTP and MCP+HTTP paths
- **Tenant budget enforcement**: Per-tenant daily token/concurrent task/API call quotas with auto-provisioning
- **Conversation isolation**: Namespaced conversation IDs with tenant prefix to prevent cross-user data leakage
- **Shutdown drain**: Configurable drain period for in-flight connections during graceful shutdown
- **Signal handling**: SIGINT (Ctrl+C) and SIGTERM on all platforms for clean shutdown
- **Thread-safe secret management**: `SECRET_OVERRIDE_MAP` replaces `std::env::set_var()` (documented UB in multi-threaded contexts); `KEYRING_CACHE` with 30s TTL avoids blocking keyring I/O in async hot paths
- **Hot-reload config**: Runtime configuration reload via `config.reload` RPC
- **High availability**: Built-in redundancy and failover support
- **Enterprise monitoring**: Comprehensive observability stack
- **Scalability**: Horizontal scaling capabilities
- **Full Phase 4 architecture**: All 7 feature-gated sub-buses and 21 F-GAP modules
- **Distributed memory bus**: Cross-node memory sharing via DistributedMemoryBus
- **Fault tolerance engine**: Cross-node fault isolation and auto-recovery
- **Multi-channel transport**: 6-channel, QoS-enabled message transport

### Architecture
```
Multi-Users Server Architecture:
├── Application Layer: go-on runtime instances
│   ├── ACP HTTP Server (port 8090): CORS → Entry Auth → User Auth → RBAC → Routing
│   └── MCP HTTP Server (port 8090): CORS → Entry Auth → User Auth → RBAC → Dispatch
├── Database Layer: PostgreSQL with pgvector
├── Load Balancer: Traffic distribution
├── Monitoring: Prometheus, Grafana, ELK
└── Backup: Automated backup system

Request Flow (ACP+HTTP):
Client → CORS headers → Entry Auth (API key) → User Session (HMAC token)
       → RBAC Authorization → Tenant Budget Check → Namespaced Conversation
       → AI Processing → Response with CORS headers

Request Flow (MCP+HTTP):
Client → CORS headers → Entry Auth (API key) → User Session (HMAC token)
       → RBAC Authorization → MCP Method Dispatch → JSON-RPC Response with CORS
```

## Configuration

### Server Configuration
Create `config/config.multi-users-server.toml`:

```toml
# config/config.multi-users-server.toml
default_phase = "think"
model_selection_mode = "adaptive"

[protocol]
mode = "acp_http"  # ACP over HTTP for multi-user access (use mcp_http for MCP)

[runtime]
acp_http_bind_addr = "0.0.0.0:8090"
production_strict = true

# ── Gateway Entry Auth (shared API key for all ingress) ────
entry_auth_enabled = true
entry_auth_api_key_env = "GO_ON_ENTRY_API_KEY"
entry_rate_limit_rpm = 5000
entry_rate_limit_burst = 1000

# ── User Authentication (per-user HMAC tokens) ─────────────
user_auth_enabled = true
user_auth_token_secret_env = "GO_ON_USER_AUTH_TOKEN_SECRET"
user_auth_token_ttl_seconds = 86400

# ── Cross-Origin Resource Sharing (CORS) ───────────────────
cors_allowed_origins = ["https://my-frontend.example.com"]
# Use "*" for development only:
# cors_allowed_origins = ["*"]

# ── Tenant Resource Quotas ─────────────────────────────────
tenant_default_daily_token_limit = 1000000
tenant_default_concurrent_tasks = 10
tenant_default_daily_api_calls = 10000

# ── OpenTelemetry (inside [runtime]) ───────────────────────
otel_enabled = true
otel_exporter = "otlp"
otel_endpoint = "http://localhost:4317"

otel_sample_ratio = 0.5

[cache]
enabled = true
# PostgreSQL connection (multi-users-server): set a postgres:// URL.
# Example: "postgres://user:pass@host:5432/goon_cache?sslmode=verify-full"
# connection_string = "postgres://..."
# read_replica_connection_string = "postgres://..."

[vector]
enabled = true
# pgvector connection (multi-users-server): set a postgres:// URL.
# Example: "postgres://user:pass@host:5432/goon_vector?sslmode=verify-full"
# connection_string = "postgres://..."
dimensions = 768
top_k = 10
min_similarity = 0.7
```

> There is no `[database]` or `[observability]` section. PostgreSQL connectivity
> is configured via `cache.connection_string` / `vector.connection_string`, and
> OpenTelemetry settings live inside `[runtime]`. Prometheus metrics are served
> at `GET /metrics` on the ACP HTTP port (8090).

### Feature Flags
Multi-Users Server mode is a **profile** (`multi-users-server`) that already
includes the PostgreSQL backend; enabling the raw `backend-postgres` feature
alone does not select any profile and fails the compile-time gate. See
`Cargo.toml` for the exact profile composition.

### Runtime Config Fields

The following runtime configuration fields are specific to multi-user mode:

| Field | Description | Default |
|-------|-------------|---------|
| `entry_auth_enabled` | Enable gateway API key auth | `false` |
| `entry_auth_api_key_env` | Env var name for entry API key | `GO_ON_ENTRY_API_KEY` |
| `entry_rate_limit_rpm` | Rate limit per source IP | `240` |
| `entry_rate_limit_burst` | Token bucket burst capacity | `60` |
| `user_auth_enabled` | Enable per-user HMAC token auth | `false` |
| `user_auth_token_secret` | HMAC signing secret (config file) | `go-on-multi-user-secret` |
| `user_auth_token_secret_env` | Env var override for HMAC secret | `GO_ON_USER_AUTH_TOKEN_SECRET` |
| `user_auth_token_ttl_seconds` | Token TTL in seconds | `86400` (24h) |
| `cors_allowed_origins` | Allowed CORS origins (empty = disabled) | `[]` |
| `tenant_default_daily_token_limit` | Daily token limit per tenant | `1000000` |
| `tenant_default_concurrent_tasks` | Max concurrent tasks per tenant | `10` |
| `tenant_default_daily_api_calls` | Daily API call limit per tenant | `10000` |
| `shutdown_drain_seconds` | Graceful shutdown drain period | `30` |

## Installation

### Building for Enterprise Deployment
```bash
# Build with multi-users-server profile (includes PostgreSQL + pgvector)
cargo build --no-default-features -F multi-users-server
```

### System Requirements
- **CPU**: 4+ cores recommended
- **Memory**: 8GB+ RAM
- **Storage**: 50GB+ free space (SSD required)
- **Network**: High-speed, reliable connection
- **Database**: PostgreSQL 14+ with pgvector extension

### Docker Compose Setup
Create `docker-compose.yml`:

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

  go-on:
    build:
      context: .
      dockerfile: Dockerfile
    environment:
      GO_ON_ENTRY_API_KEY: ${ENTRY_API_KEY}
      RUST_LOG: info
    ports:
      - "8090:8090"
    depends_on:
      postgres:
        condition: service_healthy
    volumes:
      # The mounted config must set cache.connection_string / vector.connection_string
      # to the postgres URL (e.g. postgres://go_on:${DB_PASSWORD}@postgres:5432/go_on_production).
      - ./config/config.multi-users-server.toml:/app/config.toml
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
  prometheus_data:
  grafana_data:
```

## Database Setup

### PostgreSQL Configuration
Create `init.sql` for database initialization:

```sql
-- Create database and extensions
CREATE DATABASE go_on_production;
\c go_on_production;

-- Enable required extensions
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;
CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- Create tables
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

-- Create indexes
CREATE INDEX idx_cache_key ON cache(key);
CREATE INDEX idx_cache_expires_at ON cache(expires_at);
CREATE INDEX idx_vector_embedding ON vector_store USING ivfflat (embedding vector_cosine_ops);
CREATE INDEX idx_users_username ON users(username);
CREATE INDEX idx_sessions_token ON sessions(session_token);
CREATE INDEX idx_audit_logs_created_at ON audit_logs(created_at);

-- Create roles and permissions
CREATE ROLE go_on_app WITH LOGIN PASSWORD '${APP_DB_PASSWORD}';
GRANT CONNECT ON DATABASE go_on_production TO go_on_app;
GRANT USAGE ON SCHEMA public TO go_on_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO go_on_app;
GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO go_on_app;
```

### Database Performance Tuning
Update `postgresql.conf`:

```ini
# Connection settings
max_connections = 200
shared_buffers = 2GB
effective_cache_size = 6GB

# Performance settings
maintenance_work_mem = 512MB
checkpoint_completion_target = 0.9
wal_buffers = 16MB
default_statistics_target = 100

# Query optimization
random_page_cost = 1.1
effective_io_concurrency = 200
work_mem = 32MB
```

## Deployment

### Kubernetes Deployment
Create `kubernetes/deployment.yaml`:

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
        env:
        - name: GO_ON_ENTRY_API_KEY
          valueFrom:
            secretKeyRef:
              name: go-on-secrets
              key: GO_ON_ENTRY_API_KEY
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
  type: LoadBalancer
```

### Load Balancer Configuration
Create `nginx/load-balancer.conf`:

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
    
    # SSL configuration
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
        
        # Timeouts
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

## Security

### Authentication and Authorization

Multi-Users Server provides a **two-layer authentication** system:

**Layer 1: Gateway Entry Auth (shared API key)**
- Validates every HTTP request against a pre-shared API key
- Key comes from the environment variable specified in `entry_auth_api_key_env` (default: `GO_ON_ENTRY_API_KEY`)
- Accepted headers: `Authorization: Bearer <key>`, `X-Api-Key`, `X-Go-On-Key`
- Returns 401 `ENTRY_AUTH_REQUIRED` if missing/invalid
- Returns 503 `ENTRY_AUTH_MISCONFIGURED` if env var is not set

**Layer 2: User Auth (HMAC-SHA256 tokens)**
- Issues per-user tokens signed with HMAC-SHA256
- Token format: `user_id:base64_hmac:expires_at_ms`
- Token secret resolved from env var (`user_auth_token_secret_env`) > config file > default
- Auto-provisioning: valid tokens get sessions without pre-registration
- Graceful fallback: when `user_auth_enabled=false`, all requests treated as admin

**RBAC Authorization**
- Built-in roles: `admin` (full access), `user` (R/W/X), `viewer` (read-only), `monitor`
- Permission mapping: `GET` → Read, `POST /chat` → Execute, `POST /rpc` → Execute
- Applied consistently across both ACP+HTTP and MCP+HTTP paths
- Returns 401 `AUTH_REQUIRED` (no session), 403 `ACCESS_DENIED` (no permission), 403 `PRIVILEGE_ESCALATION_REQUIRED` (insufficient role)

**Exempt endpoints:** `/` and `/health` bypass all authentication.

### Issuing User Tokens

User tokens can be issued programmatically via the internal `SessionManager` API. The token format is `user_id:base64_hmac_sig:expires_at_ms`:

```rust
// Rust: issue a token programmatically
use crate::acp::r#impl::session::{AuthConfig, SessionManager};

let auth_cfg = AuthConfig::from(&runtime_config);
let mgr = SessionManager::with_auth_config(auth_cfg);
let token = mgr.issue_token("alice", &["admin"], None, 86400).unwrap();
// Returns: "alice:<base64_hmac>:<expires_at_ms>"
```

### CORS Configuration

Allowed origins default to `[]` (empty = CORS disabled). To enable cross-origin requests:

```toml
[runtime]
cors_allowed_origins = ["https://my-frontend.example.com"]
# Multiple origins:
# cors_allowed_origins = ["https://app1.example.com", "https://app2.example.com"]
# Development only:
# cors_allowed_origins = ["*"]
```

All HTTP responses (JSON, SSE, errors) automatically include CORS headers when configured.

### Tenant Budget Enforcement

When user auth is enabled, the system auto-provisions a default tenant quota:

```toml
[runtime]
tenant_default_daily_token_limit = 1000000
tenant_default_concurrent_tasks = 10
tenant_default_daily_api_calls = 10000
```

Conversation IDs are namespaced with the tenant ID prefix to prevent cross-user data leakage:
```
Internal conversation_id = "tenant_id:user_provided_conversation_id"
```

### Conversation Isolation

All conversation state is isolated per-tenant:
- Conversation IDs are prefixed with `tenant_id:` when `user_auth_enabled`
- Budget enforcement uses `tenant_id` from user session (not raw `conversation_id`)
- Cross-user conversation history leakage is prevented

### Thread-Safe Secret Management

The project replaces all `std::env::set_var()` calls (documented as **undefined behavior** in multi-threaded contexts) with an in-memory `HashMap`:

```rust
// Thread-safe: set a secret override
crate::shared::secret_override::set_secret_override("GITHUB_TOKEN", "ghp_xxx");

// Thread-safe: read (checks override map first, then env var)
let value = crate::shared::secret_override::get_secret("GITHUB_TOKEN");
```

Keyring lookups are cached with a 30-second TTL to avoid blocking I/O in async contexts:

```rust
// Uses cache with 30s TTL
let value = crate::shared::secret_override::get_keyring_cached("go-on", "copilot_api_key");
```

**Security compliance:**
- ✅ 0 `std::env::set_var()` calls in production code
- ✅ 0 `.expect("lock poisoned")` panics — all lock poison recovered via `unwrap_or_else(|e| e.into_inner())`
- ✅ 0 `unsafe` blocks in production code
- ✅ All HTTP responses (JSON, SSE, error) include CORS headers
- ✅ MCP HTTP server shares the same auth pipeline as ACP HTTP server

### Network Security
```bash
# Firewall rules
sudo ufw allow 443/tcp  # HTTPS
sudo ufw allow 5432/tcp  # PostgreSQL
sudo ufw allow 8090/tcp  # go-on ACP/MCP HTTP

# Enable firewall
sudo ufw --force enable
```

### Secrets Management
```bash
# Set required environment variables
# (used by GO_ON_ENTRY_API_KEY / GO_ON_USER_AUTH_TOKEN_SECRET env fields)
export GO_ON_ENTRY_API_KEY=$(openssl rand -base64 48)
export GO_ON_USER_AUTH_TOKEN_SECRET=$(openssl rand -base64 64)

# PostgreSQL credentials are embedded in cache.connection_string /
# vector.connection_string in the config file (e.g.
# postgres://USER:PASS@HOST:5432/goon_cache?sslmode=verify-full).
```

## Monitoring and Observability

### Prometheus Configuration
Create `prometheus.yml`:

```yaml
global:
  scrape_interval: 15s
  evaluation_interval: 15s

scrape_configs:
  - job_name: 'go-on'
    static_configs:
      - targets: ['go-on:8090']
    metrics_path: '/metrics'
    
  - job_name: 'postgres'
    static_configs:
      - targets: ['postgres-exporter:9187']

alerting:
  alertmanagers:
    - static_configs:
        - targets: ['alertmanager:9093']

rule_files:
  - 'alerts.yml'
```

### Grafana Dashboards
Create `grafana/dashboards/go-on.json` with key metrics:
- Request rate and latency
- Error rates and types
- Database performance
- Cache hit rates
- User activity
- Resource utilization

### Log Aggregation
Collect stderr/stdout (systemd, Docker, or a log shipper). The log level is set
via the `RUST_LOG` environment variable; there is no `[logging]` config section.

## Backup and Disaster Recovery

### Database Backup
```bash
#!/bin/bash
# backup.sh
BACKUP_DIR="/backup/go-on"
DATE=$(date +%Y%m%d_%H%M%S)

# Backup PostgreSQL
pg_dump -h localhost -U go_on -d go_on_production \
  --format=custom \
  --file="$BACKUP_DIR/go-on-$DATE.dump"

# Backup configuration
cp /opt/go-on/config/config.multi-users-server.toml "$BACKUP_DIR/config-$DATE.toml"

# Upload to cloud storage
aws s3 cp "$BACKUP_DIR/go-on-$DATE.dump" s3://go-on-backups/
aws s3 cp "$BACKUP_DIR/config-$DATE.toml" s3://go-on-backups/

# Cleanup old backups (keep 30 days)
find $BACKUP_DIR -name "*.dump" -mtime +30 -delete
find $BACKUP_DIR -name "*.toml" -mtime +30 -delete
```

### Disaster Recovery Plan
1. **Recovery Time Objective (RTO)**: 1 hour
2. **Recovery Point Objective (RPO)**: 15 minutes
3. **Backup frequency**: Hourly incremental, daily full
4. **Testing**: Monthly recovery drills

## Scaling

### Horizontal Scaling
```yaml
# Kubernetes Horizontal Pod Autoscaler
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

### Database Scaling
```sql
-- Read replicas for scaling reads
CREATE PUBLICATION go_on_publication FOR ALL TABLES;
CREATE SUBSCRIPTION go_on_subscription
  CONNECTION 'host=replica-server port=5432 dbname=go_on_production user=replication_user password=secret'
  PUBLICATION go_on_publication;

-- Partitioning for large tables
CREATE TABLE cache_partitioned (
    LIKE cache INCLUDING ALL
) PARTITION BY RANGE (created_at);

CREATE TABLE cache_2024_q1 PARTITION OF cache_partitioned
    FOR VALUES FROM ('2024-01-01') TO ('2024-04-01');
CREATE TABLE cache_2024_q2 PARTITION OF cache_partitioned
    FOR VALUES FROM ('2024-04-01') TO ('2024-07-01');
```

## Migration

### From Simple Server Mode
Simple-server uses SQLite (`cache.sqlite3`, `vector.sqlite3`); multi-users-server uses
PostgreSQL. There is no `--export`/`--import` CLI — migrate the configuration and let
the cache re-populate:

```bash
# 1. Copy the configuration (agents, phases, runtime settings)
cp config/config.simple-server.toml config/config.multi-users-server.toml

# 2. Update the database connection strings in the new config
#    cache.connection_string = "postgres://USER:PASS@HOST:5432/goon_cache?sslmode=verify-full"
#    vector.connection_string = "postgres://USER:PASS@HOST:5432/goon_vector?sslmode=verify-full"

# 3. Optional: carry over the SQLite cache files for a warm start
cp /var/lib/go-on/cache.sqlite3  /var/lib/go-on/cache.sqlite3.simple-server.bak
cp /var/lib/go-on/vector.sqlite3 /var/lib/go-on/vector.sqlite3.simple-server.bak
```

Cached responses and vector indexes are best-effort caches; they will be
rebuilt lazily against PostgreSQL after the first requests.

### Zero-Downtime Migration
1. **Phase 1**: Set up new infrastructure alongside old
2. **Phase 2**: Start dual-write to both systems
3. **Phase 3**: Migrate read traffic to new system
4. **Phase 4**: Migrate write traffic to new system
5. **Phase 5**: Decommission old system

## Troubleshooting

### Performance Issues

#### Database Bottlenecks
```sql
-- Identify slow queries
SELECT query, calls, total_time, mean_time
FROM pg_stat_statements
ORDER BY mean_time DESC
LIMIT 10;

-- Check table sizes
SELECT schemaname, tablename, pg_size_pretty(pg_total_relation_size(schemaname||'.'||tablename))
FROM pg_tables
WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
ORDER BY pg_total_relation_size(schemaname||'.'||tablename) DESC;

-- Check index usage
SELECT schemaname, tablename, indexname, idx_scan, idx_tup_read, idx_tup_fetch
FROM pg_stat_user_indexes
ORDER BY idx_scan;
```

#### Application Issues
```bash
# Check application logs
kubectl logs -l app=go-on --tail=100
kubectl logs -l app=go-on --since=1h

# Check resource usage
kubectl top pods -l app=go-on
kubectl describe pods -l app=go-on

# Check network connectivity
kubectl exec go-on-pod -- curl http://postgres:5432
```

### Security Issues

#### Audit Log Review
```sql
-- Review recent security events
SELECT *
FROM audit_logs
WHERE action IN ('login_failed', 'access_denied', 'privilege_escalation')
ORDER BY created_at DESC
LIMIT 100;

-- Check for suspicious activity
SELECT user_id, COUNT(*) as failed_attempts, MIN(created_at) as first_attempt, MAX(created_at) as last_attempt
FROM audit_logs
WHERE action = 'login_failed'
  AND created_at > NOW() - INTERVAL '1 hour'
GROUP BY user_id
HAVING COUNT(*) > 5;
```

#### Security Scanning
```bash
# Run vulnerability scan
trivy image your-registry/go-on:multi-users-latest

# Check for secrets in code
gitleaks detect --source . --verbose

# Network security scan
nmap -sV -p 80,443,5432,8090 go-on.example.com
```

## Maintenance

### Regular Maintenance Tasks
```bash
#!/bin/bash
# maintenance.sh

# 1. Database maintenance
psql -h localhost -U go_on -d go_on_production -c "VACUUM ANALYZE;"
psql -h localhost -U go_on -d go_on_production -c "REINDEX DATABASE go_on_production;"

# 2. Log rotation
find /var/log/go-on -name "*.log" -mtime +7 -delete

# 3. Verify data integrity (cache/vector SQLite or PostgreSQL)
# 4. Backup verification
# (Verify latest backup can be restored)
```

### Update Procedure
1. **Pre-update checklist**:
   - Verify backups are current
   - Notify users of maintenance window
   - Prepare rollback plan

2. **Update process**:
   ```bash
   # 1. Deploy new version to staging
   kubectl apply -f kubernetes/staging/
   
   # 2. Run smoke tests
   ./scripts/smoke-tests.sh
   
   # 3. Deploy to production (canary)
   kubectl set image deployment/go-on go-on=your-registry/go-on:new-version
   
   # 4. Monitor metrics
   watch kubectl get pods -l app=go-on
   
   # 5. Full rollout
   kubectl rollout status deployment/go-on
   ```

3. **Post-update verification**:
   - Verify all services are healthy
   - Check error rates and performance
   - Confirm user functionality

## Cost Optimization

### Resource Optimization
```yaml
# Kubernetes resource requests and limits
resources:
  requests:
    memory: "1Gi"
    cpu: "500m"
  limits:
    memory: "2Gi"
    cpu: "1000m"
```

### Auto-scaling Policies
- Scale down during off-peak hours
- Use spot instances for non-critical workloads
- Implement cost-aware scheduling

### Monitoring Costs
- Track database storage growth
- Monitor API call costs to model providers
- Alert on unexpected cost spikes

## Support and Community

### Enterprise Support
- **SLAs**: 99.9% uptime guarantee
- **Support channels**: Email, phone, dedicated Slack
- **Response times**: Critical: 1 hour, High: 4 hours, Normal: 24 hours

### Community Resources
- [GitHub Discussions](https://github.com/your-org/go-on/discussions)
- [Documentation](https://go-on.example.com/docs)
- [Stack Overflow](https://stackoverflow.com/questions/tagged/go-on)

### Training and Certification
- Administrator training courses
- Developer certification program
- Custom training for enterprise teams

## Next Steps

After setting up multi-users server mode, you can:
1. Configure [High Availability](./high-availability.md)
2. Set up [Disaster Recovery](./disaster-recovery.md)
3. Implement [Advanced Security](./advanced-security.md)
4. Explore [API Documentation](../api/overview.md)
5. Join the [Enterprise Community](https://github.com/your-org/go-on/discussions/categories/enterprise)
