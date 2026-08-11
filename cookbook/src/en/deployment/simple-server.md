# Simple Server Mode Deployment

## Overview

Simple Server mode (`simple-server`) is designed for single-server deployments that require better performance and reliability than local mode, while maintaining simplicity. It uses SQLite with required vector extensions and is suitable for small teams or production-like environments.

## Features

### Enhanced Capabilities
- **Single-server deployment**: Designed for dedicated server environments
- **Required vector extensions**: Uses `sqlite-vec` (no JSON fallback)
- **Improved performance**: Optimized for server workloads
- **Better reliability**: Enhanced error handling and recovery
- **Production readiness**: Suitable for small-scale production use
- **Full sub-bus architecture**: All 7 feature-gated sub-buses (including distributed-memory), per `Cargo.toml`
- **Conditionally compiled modules**: `DistributedMemoryBus` is gated with `#[cfg(feature = "sub-bus-distributed-memory")]` (enabled by the `simple-server` profile)

### Architecture
```
Simple Server Architecture:
├── Runtime: Dedicated server process
├── Storage: SQLite with vector extensions
├── Network: HTTP/HTTPS endpoints
└── Monitoring: Enhanced observability
```

## Configuration

### Server Configuration
Create `config/config.simple-server.toml`:

```toml
# config/config.simple-server.toml
default_phase = "coding"
model_selection_mode = "adaptive"

[protocol]
mode = "acp_http"  # HTTP mode for server deployment

[runtime]
acp_http_bind_addr = "0.0.0.0:8090"  # Bind to all interfaces
production_strict = true
entry_auth_enabled = true
entry_auth_api_key_env = "GO_ON_ENTRY_API_KEY"
entry_rate_limit_rpm = 1000
entry_rate_limit_burst = 200

[cache]
enabled = true
path = "/var/lib/go-on/cache.sqlite3"
default_ttl_seconds = 7200
max_entries = 20000

[vector]
enabled = true
auto_mode = false  # Require sqlite-vec
path = "/var/lib/go-on/vector.sqlite3"
dimensions = 384  # Higher dimensions for better accuracy
top_k = 5
min_similarity = 0.75

# OpenTelemetry settings live under [runtime]
[runtime]
otel_enabled = true
otel_exporter = "otlp"
otel_endpoint = "http://localhost:4317"
```

> Note: there is no separate `[observability]` or `[metrics]` section. OpenTelemetry
> settings belong to `[runtime]`, and Prometheus metrics are served at `GET /metrics`
> on the ACP HTTP port (8090).

### Feature Flags
Simple Server mode is a **profile** (`simple-server`) that already includes the
SQLite backend; the raw `backend-sqlite` feature alone does not select any
profile. See `Cargo.toml` for the exact profile composition.

## Installation

### Building for Server Deployment
```bash
# Build with simple-server profile (includes SQLite + sqlite-vec)
cargo build --no-default-features -F simple-server
```

### System Requirements
- **CPU**: 2+ cores recommended
- **Memory**: 4GB+ RAM
- **Storage**: 10GB+ free space (SSD recommended)
- **Network**: Stable internet connection for model providers

### Systemd Service Setup
Create `/etc/systemd/system/go-on.service`:

```ini
[Unit]
Description=go-on Simple Server
After=network.target

[Service]
Type=simple
User=go-on
Group=go-on
WorkingDirectory=/opt/go-on
Environment="GO_ON_ENTRY_API_KEY=your-api-key-here"
Environment="RUST_LOG=info"
ExecStart=/opt/go-on/go-on --config /opt/go-on/config/config.simple-server.toml
Restart=on-failure
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

## Setup

### Directory Structure
```bash
# Create directories
sudo mkdir -p /opt/go-on /var/lib/go-on /var/log/go-on
sudo chown -R go-on:go-on /opt/go-on /var/lib/go-on /var/log/go-on

# Copy configuration
sudo cp config/config.simple-server.toml /opt/go-on/config/
sudo cp scripts/start-go-on.sh /opt/go-on/
sudo chmod +x /opt/go-on/start-go-on.sh
```

### Database Initialization
```bash
# Initialize as go-on user
sudo -u go-on /opt/go-on/go-on --init --config /opt/go-on/config/config.simple-server.toml

# Check configuration
sudo -u go-on /opt/go-on/go-on --check --config /opt/go-on/config/config.simple-server.toml
```

### User and Permissions
```bash
# Create system user
sudo useradd -r -s /bin/false -m -d /opt/go-on go-on

# Set permissions
sudo chown -R go-on:go-on /opt/go-on
sudo chmod 750 /opt/go-on
```

## Running

### Starting the Server
```bash
# Using systemd
sudo systemctl daemon-reload
sudo systemctl enable go-on
sudo systemctl start go-on

# Check status
sudo systemctl status go-on

# View logs
sudo journalctl -u go-on -f
```

### Manual Start
```bash
# As go-on user
sudo -u go-on /opt/go-on/go-on --config /opt/go-on/config/config.simple-server.toml

# With environment variables
GO_ON_ENTRY_API_KEY="your-key" sudo -u go-on /opt/go-on/go-on --config /opt/go-on/config/config.simple-server.toml
```

### Health and Monitoring
```bash
# Health endpoint
curl http://localhost:8090/health

# Prometheus metrics (text format, served on the ACP HTTP port)
curl http://localhost:8090/metrics
```

## Network Configuration

### Firewall Rules
```bash
# Allow HTTP port
sudo ufw allow 8090/tcp

# Enable firewall
sudo ufw enable
```

### Reverse Proxy (Nginx)
Create `/etc/nginx/sites-available/go-on`:

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
    
    # Health check endpoint
    location /health {
        proxy_pass http://127.0.0.1:8090/health;
        access_log off;
    }
}
```

### SSL/TLS Configuration
```bash
# Using Let's Encrypt with Certbot
sudo certbot --nginx -d go-on.example.com

# Or manual SSL configuration
ssl_certificate /etc/ssl/certs/go-on.crt;
ssl_certificate_key /etc/ssl/private/go-on.key;
```

## Storage Management

### Database Maintenance
```bash
# Regular vacuum (weekly)
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3 "VACUUM;"
sudo -u go-on sqlite3 /var/lib/go-on/vector.sqlite3 "VACUUM;"

# Analyze for query optimization
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3 "ANALYZE;"
sudo -u go-on sqlite3 /var/lib/go-on/vector.sqlite3 "ANALYZE;"
```

### Backup Strategy
```bash
# Daily backup script
#!/bin/bash
BACKUP_DIR="/backup/go-on"
DATE=$(date +%Y%m%d)

# Backup databases
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3 ".backup $BACKUP_DIR/cache-$DATE.sqlite3"
sudo -u go-on sqlite3 /var/lib/go-on/vector.sqlite3 ".backup $BACKUP_DIR/vector-$DATE.sqlite3"

# Backup configuration
cp /opt/go-on/config/config.simple-server.toml $BACKUP_DIR/config-$DATE.toml

# Rotate old backups (keep 30 days)
find $BACKUP_DIR -name "*.sqlite3" -mtime +30 -delete
find $BACKUP_DIR -name "*.toml" -mtime +30 -delete
```

### Disk Space Monitoring
```bash
# Check database sizes
du -h /var/lib/go-on/*.sqlite3

# Monitor growth
df -h /var/lib/go-on
```

## Performance Tuning

### Memory and concurrency
Concurrency limits are configured per phase via `[phases.<name>.options]`
(`phase_max_inflight` / `global_max_inflight`) and entry rate limits via
`[runtime]` (`entry_rate_limit_rpm` / `entry_rate_limit_burst`). There are no
`[concurrency]` or `[timeouts]` top-level sections.

## Security

### API Key Management
```bash
# Set API key in environment
export GO_ON_ENTRY_API_KEY="secure-random-key-here"

# Or use keyring
keyring set go-on server-api-key
```

### Rate Limiting
Entry-layer rate limits are configured in `[runtime]`:

```toml
[runtime]
entry_rate_limit_rpm = 1000
entry_rate_limit_burst = 200
```

### Access Control
CORS origins and entry auth are configured in `[runtime]`:

```toml
[runtime]
entry_auth_enabled = true
entry_auth_api_key_env = "GO_ON_ENTRY_API_KEY"
cors_allowed_origins = ["https://your-domain.com"]
```

> There is no `[security]` or `[access]` section. IP allow/block lists and an
> HTTPS enforcement toggle are not supported; use a firewall / reverse proxy
> for those controls.

## Monitoring and Logging

### Logging
Set the log level via the `RUST_LOG` environment variable; there
is no `[logging]` section and no `--verbose` flag. Logs go to stderr and can be redirected by systemd or
a log manager.

### Metrics Collection
Prometheus-format metrics are served at `GET /metrics` on the ACP HTTP port (8090)
— no separate metrics port is needed.

### Alerting
```bash
# Example alert rule for Prometheus
groups:
- name: go-on-alerts
  rules:
  - alert: GoOnHighErrorRate
    expr: rate(go_on_errors_total[5m]) > 0.1
    for: 2m
    labels:
      severity: warning
    annotations:
      summary: "High error rate detected"
      description: "Error rate is {{ $value }} per second"
```

## Scaling Considerations

### When to Scale Up
- CPU usage consistently above 70%
- Memory usage consistently above 80%
- Response times increasing significantly
- Concurrent users exceeding 50

### Scaling Options
1. **Vertical scaling**: Upgrade server resources
2. **Horizontal scaling**: Migrate to multi-users-server mode
3. **Load balancing**: Add multiple simple-server instances

## Migration

### From Local Mode
The SQLite data files (`cache.sqlite3`, `vector.sqlite3`) can be copied directly
from the local machine to the server (go-on has no `--export`/`--import` CLI):

```bash
# Stop both instances, then copy the data files
scp ./sqlite3/acp_cache.sqlite3 go-on@server:/var/lib/go-on/cache.sqlite3
scp ./sqlite3/acp_vector.sqlite3 go-on@server:/var/lib/go-on/vector.sqlite3
```

### Backup and Restore
```bash
# Full backup
tar czf go-on-backup-$(date +%Y%m%d).tar.gz /opt/go-on /var/lib/go-on

# Restore
tar xzf go-on-backup-20240101.tar.gz -C /
```

## Troubleshooting

### Common Issues

#### Service Won't Start
```bash
# Check logs
sudo journalctl -u go-on --no-pager -n 50

# Check permissions
sudo ls -la /opt/go-on/
sudo ls -la /var/lib/go-on/

# Test manually
sudo -u go-on /opt/go-on/go-on --config /opt/go-on/config/config.simple-server.toml --validate-config
```

#### Database Issues
```bash
# Check SQLite integrity
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3 "PRAGMA integrity_check;"
sudo -u go-on sqlite3 /var/lib/go-on/vector.sqlite3 "PRAGMA integrity_check;"

# Repair if needed
sudo -u go-on cp /var/lib/go-on/cache.sqlite3 /var/lib/go-on/cache.sqlite3.backup
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3.backup ".recover" | sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3
```

#### Performance Issues
```bash
# Monitor resource usage
top -u go-on
iotop -u go-on

# Check database performance
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3 "EXPLAIN QUERY PLAN SELECT * FROM cache WHERE key = 'test';"
```

## Next Steps

After setting up simple server mode, you can:
1. Configure [Monitoring and Alerting](./monitoring.md)
2. Set up [Backup Strategy](./backup.md)
3. Explore [API Documentation](../api/overview.md)
4. Consider [Multi-Users Server Mode](./multi-users-server.md) for larger deployments