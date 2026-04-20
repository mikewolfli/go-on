# Multi-Users Server Mode Deployment

## Overview

Multi-Users Server mode (`profile-multi-users-server`) is the enterprise-grade deployment profile for go-on, designed for production environments with multiple concurrent users. It uses PostgreSQL with pgvector for scalable storage and provides advanced features for security, monitoring, and high availability.

## Features

### Enterprise Capabilities
- **Multi-user support**: Designed for concurrent access by multiple users
- **PostgreSQL storage**: Scalable database with pgvector extension
- **High availability**: Built-in redundancy and failover support
- **Advanced security**: Role-based access control, audit logging
- **Enterprise monitoring**: Comprehensive observability stack
- **Scalability**: Horizontal scaling capabilities

### Architecture
```
Multi-Users Server Architecture:
├── Application Layer: go-on runtime instances
├── Database Layer: PostgreSQL with pgvector
├── Cache Layer: Redis (optional)
├── Load Balancer: Traffic distribution
├── Monitoring: Prometheus, Grafana, ELK
└── Backup: Automated backup system
```

## Configuration

### Server Configuration
Create `config/multi-users-server.toml`:

```toml
# config/multi-users-server.toml
default_phase = "coding"
model_selection_mode = "adaptive"

[protocol]
mode = "mcp_http"  # MCP over HTTP for multi-user access

[runtime]
acp_http_bind_addr = "0.0.0.0:8090"
production_strict = true
entry_auth_enabled = true
entry_auth_api_key_env = "GO_ON_ENTRY_API_KEY"
entry_rate_limit_rpm = 5000
entry_rate_limit_burst = 1000
session_timeout_minutes = 60
max_sessions_per_user = 10

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
type = "database"  # Uses PostgreSQL for cache
# Optional Redis cache
# type = "redis"
# redis_url = "redis://localhost:6379"

[vector]
enabled = true
type = "pgvector"
dimensions = 768  # Higher dimensions for enterprise use
top_k = 10
min_similarity = 0.7

[security]
rbac_enabled = true
audit_logging_enabled = true
data_encryption_enabled = true
mfa_enabled = false  # Optional: enable for higher security

[observability]
otel_enabled = true
otel_exporter = "otlp"
otel_endpoint = "http://localhost:4317"
metrics_port = 9090
tracing_sampling_rate = 0.1
```

### Feature Flags
Multi-Users Server mode requires:
- `backend-postgres`: PostgreSQL database support
- `postgres`: PostgreSQL client library
- `pgvector`: Vector extension for PostgreSQL

## Installation

### Building for Enterprise Deployment
```bash
# Build with multi-users-server profile
cargo build --no-default-features -F profile-multi-users-server

# Or explicitly enable features
cargo build --features "backend-postgres postgres pgvector"
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
```toml
[security.authentication]
type = "jwt"
jwt_secret_env = "GO_ON_JWT_SECRET"
token_expiry_hours = 24
refresh_token_expiry_days = 7

[security.authorization]
rbac_enabled = true
roles = ["admin", "user", "viewer"]
default_role = "user"

[security.audit]
enabled = true
retention_days = 90
sensitive_fields = ["password", "api_key", "token"]
```

### Network Security
```bash
# Firewall rules
sudo ufw allow 443/tcp  # HTTPS
sudo ufw allow 5432/tcp  # PostgreSQL
sudo ufw allow 6379/tcp  # Redis
sudo ufw allow 9090/tcp  # Metrics
sudo ufw allow 4317/tcp  # OpenTelemetry

# Enable firewall
sudo ufw --force enable
```

### Secrets Management
```bash
# Using HashiCorp Vault
vault kv put secret/go-on \
  db_username=go_on \
  db_password=$(openssl rand -base64 32) \
  entry_api_key=$(openssl rand -base64 48) \
  jwt_secret=$(openssl rand -base64 64)

# Environment variables
export GO_ON_DB_USERNAME=$(vault kv get -field=db_username secret/go-on)
export GO_ON_DB_PASSWORD=$(vault kv get -field=db_password secret/go-on)
export GO_ON_ENTRY_API_KEY=$(vault kv get -field=entry_api_key secret/go-on)
export GO_ON_JWT_SECRET=$(vault kv get -field=jwt_secret secret/go-on)
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

### Grafana Dashboards
Create `grafana/dashboards/go-on.json` with key metrics:
- Request rate and latency
- Error rates and types
- Database performance
- Cache hit rates
- User activity
- Resource utilization

### Log Aggregation
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
cp /opt/go-on/config/multi-users-server.toml "$BACKUP_DIR/config-$DATE.toml"

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
```bash
# Export data from simple server
pg_dump -h simple-server -U go_on -d go_on_simple \
  --format=custom \
  --file=simple-server-export.dump

# Import to multi-users server
pg_restore -h multi-users-server -U go_on -d go_on_production \
  --clean \
  --if-exists \
  simple-server-export.dump

# Update configuration
cp config/simple-server.toml config/multi-users-server.toml
# Update database settings in the new config
```

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
kubectl exec go-on-pod -- curl http://redis:6379
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
nmap -sV -p 80,443,5432,6379,8090,9090 go-on.example.com
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

# 3. Cache cleanup
redis-cli FLUSHDB

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
