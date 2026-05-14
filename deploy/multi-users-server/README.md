# go-on Multi-Users Server — Deployment Guide

Production multi-user deployment using PostgreSQL + pgvector as backend storage.
Supports large-scale concurrent users (≥100), with entry authentication, rate limiting,
OTEL observability, and strict mode.

## Architecture

```
                        ┌──────────────┐
                        │   Nginx/HA   │  ← Reverse proxy / TLS termination
                        │  (443→8090)  │
                        └──────┬───────┘
                               │
                   ┌───────────┴───────────┐
                   │   go-on Server         │
                   │   (acp_http:8090)      │
                   └───────────┬───────────┘
                               │
          ┌────────────────────┼────────────────────┐
          ▼                    ▼                    ▼
   ┌──────────┐       ┌──────────────┐     ┌──────────────┐
   │PostgreSQL │       │  Redis/Mem   │     │  OTEL        │
   │+ pgvector │       │  (opt cache) │     │  Collector   │
   └──────────┘       └──────────────┘     └──────────────┘
```

## Prerequisites

| Component | Version | Note |
|-----------|---------|------|
| Linux (Ubuntu 22.04+ / Debian 12+) | — | Production recommended |
| Rust | ≥ 1.80 | `rustup update stable` |
| PostgreSQL | ≥ 15 | Primary database |
| pgvector | ≥ 0.6 | Vector extension |
| systemd | — | Process management |
| Nginx (optional) | — | Reverse proxy / TLS |

### PostgreSQL Setup

```bash
sudo apt install postgresql-15 postgresql-15-pgvector

sudo -u postgres psql -c "CREATE USER goon WITH PASSWORD 'strong-password-here';"
sudo -u postgres psql -c "CREATE DATABASE goon OWNER goon;"
sudo -u postgres psql -d goon -c "CREATE EXTENSION vector;"
sudo -u postgres psql -d goon -c "GRANT ALL ON SCHEMA public TO goon;"
```

## 1. Build

```bash
git clone <repo-url> /opt/go-on
cd /opt/go-on
cargo build --release --no-default-features -F profile-multi-users-server
```

Artifact:
```
target/release/go-on     # Backend CLI (with --chat mode)
```

## 2. Directory Layout

```
/opt/go-on/
├── backend/
│   ├── go-on                        # Backend binary
│   ├── config.toml                 # Main config
│   ├── backend.log                 # Runtime log (auto-created)
│   ├── environment                 # Env vars (chmod 600)
│   └── start.sh                    # Start script (optional)
├── deploy/
│   └── multi-users-server/
│       ├── deploy.sh               # Deploy script
│       ├── go-on-multi.service     # systemd unit
│       ├── nginx.conf              # Nginx reverse proxy
│       ├── Dockerfile              # Docker image
│       ├── docker-compose.yml      # Docker Compose
│       ├── otel-collector-config.yaml
│       └── README.md               # This file
└── config/
    └── config.multi-users-server.toml  # Template config
```

## 3. Deployment Steps

### 3.1 Create directories

```bash
sudo mkdir -p /opt/go-on/backend
sudo chown $USER:$USER /opt/go-on -R
```

### 3.2 Copy binary

```bash
cp target/release/go-on /opt/go-on/backend/
```

### 3.3 Configure environment

Create `/opt/go-on/backend/environment` (chmod 600):

```bash
DB_HOST=localhost
DB_PORT=5432
DB_USER=goon
DB_PASS=your-strong-password
DB_NAME=goon
GO_ON_ENTRY_API_KEY=generate-a-random-64-char-secret
DEEPSEEK_API_KEY=sk-xxxxx
```

### 3.4 Deploy configuration

```bash
cp config/config.multi-users-server.toml /opt/go-on/backend/config.toml
```

Database connection is configured via environment variables (see 3.3), never in config.toml.
The `[cache]` and `[vector]` sections use local SQLite files as caches.

### 3.5 systemd service

```bash
sudo cp deploy/multi-users-server/go-on-multi.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable go-on-multi
sudo systemctl start go-on-multi
sudo systemctl status go-on-multi
```

### 3.6 Nginx + TLS

```bash
sudo cp deploy/multi-users-server/nginx.conf /etc/nginx/sites-available/go-on-multi
sudo ln -s /etc/nginx/sites-available/go-on-multi /etc/nginx/sites-enabled/

sudo apt install certbot python3-certbot-nginx
sudo certbot --nginx -d your-domain.com

sudo nginx -t && sudo systemctl reload nginx
```

### 3.7 Docker Deployment

```bash
# Build
docker build -f deploy/multi-users-server/Dockerfile -t go-on:multi .

# Start PostgreSQL first, then go-on
docker compose -f deploy/multi-users-server/docker-compose.yml up -d

# With OTEL observability (optional)
docker compose -f deploy/multi-users-server/docker-compose.yml --profile observability up -d
```

### 3.8 Verify

```bash
# Via Nginx
curl https://your-domain.com/health

# Direct
curl http://127.0.0.1:8090/health

# Validate config
/opt/go-on/backend/go-on -c /opt/go-on/backend/config.toml --validate-config

# Runtime status
/opt/go-on/backend/go-on -c /opt/go-on/backend/config.toml --status
```

## 4. Database

### Connection

Configured via environment variables (never write credentials to config.toml):

| Variable | Description | Default |
|----------|-------------|---------|
| `DB_HOST` | Database host | `localhost` |
| `DB_PORT` | Database port | `5432` |
| `DB_USER` | Database user | — |
| `DB_PASS` | Database password | — |
| `DB_NAME` | Database name | — |

### Migration

Auto-migrated on first startup. To trigger manually:

```bash
/opt/go-on/backend/go-on -c /opt/go-on/backend/config.toml --validate-config
```

## 5. Entry Authentication

All API requests require `Authorization: Bearer <key>` header.
The API key is set via `GO_ON_ENTRY_API_KEY` environment variable.

```bash
curl -H "Authorization: Bearer your-api-key" https://your-domain.com/health
```

## 6. Operations

### Service management

```bash
sudo systemctl start go-on-multi
sudo systemctl stop go-on-multi
sudo systemctl restart go-on-multi
sudo journalctl -u go-on-multi -n 100 -f
```

### Database maintenance

```bash
sudo -u postgres psql -d goon -c "VACUUM ANALYZE;"
sudo -u postgres psql -d goon -c "SELECT pg_size_pretty(pg_database_size('goon'));"
```

### Backup

```bash
# /etc/cron.daily/goon-backup
BACKUP_DIR="/backup/goon"
mkdir -p "$BACKUP_DIR"
pg_dump goon -U goon -h localhost | gzip > "$BACKUP_DIR/goon-$(date +%Y%m%d-%H%M%S).sql.gz"
find "$BACKUP_DIR" -name "*.sql.gz" -mtime +30 -delete
```

### Terminal chat mode

```bash
cd /opt/go-on/backend && ./go-on -a
```

## 7. Config Reference

Key fields from `config/config.multi-users-server.toml`:

| Field | Default | Description |
|-------|---------|-------------|
| `runtime.acp_http_bind_addr` | `0.0.0.0:8090` | Listen on all interfaces |
| `runtime.entry_auth_enabled` | `true` | Entry authentication |
| `runtime.entry_auth_api_key_env` | `GO_ON_ENTRY_API_KEY` | API key env var |
| `runtime.entry_rate_limit_rpm` | `5000` | Requests per minute |
| `runtime.entry_rate_limit_burst` | `1000` | Burst limit |
| `runtime.production_strict` | `true` | Fail-fast on misconfig |
| `runtime.otel_enabled` | `true` | OTEL observability |
| `runtime.otel_sample_ratio` | `0.5` | Trace sampling (prod: 0.1-0.5) |
| `cache.max_entries` | `50000` | Max cache entries |
| `vector.max_entries` | `200000` | Max vector entries |
| `vector.dimensions` | `768` | Vector dimensions (depends on model) |

## 8. OTEL Observability

Docker Compose includes an OTEL Collector container (opt-in via `--profile observability`).
See `otel-collector-config.yaml` for configuration.

## 9. Scaling

### Vertical

| Resource | Recommendation | Note |
|----------|---------------|------|
| CPU | ≥ 8 cores | AI inference is CPU-intensive |
| RAM | ≥ 16 GB | Vector index + cache |
| Disk | ≥ 100 GB SSD | PostgreSQL + logs |
| Network | ≥ 1 Gbps | AI API calls |

### Horizontal

go-on multi-users server is single-process. For horizontal scaling:

```bash
# DNS round-robin + multiple go-on instances
# Each instance connects to the same PostgreSQL
# Note: cache and vector store are local SQLite, not shared
```

## 10. Security Hardening

| Measure | Description |
|---------|-------------|
| TLS | Nginx terminates TLS |
| Entry auth | `entry_auth_enabled = true` |
| Rate limiting | `entry_rate_limit_rpm` / `burst` |
| Strict mode | `production_strict = true` |
| NoNewPrivileges | systemd security |
| ProtectSystem | systemd filesystem protection |
| API key management | Use keyring, not env vars |
| DB encryption | PostgreSQL TDE or disk encryption |

## 11. Monitoring

| Metric | Threshold | Description |
|--------|-----------|-------------|
| Process alive | — | systemd auto-restart |
| p95 latency | > 5s | AI response time |
| Error rate | > 3% | RPC errors |
| QPS | > 80% capacity | Approaching rate limit |
| PG connections | > 80% max | Connection pool exhaustion |
| Disk space | > 80% | DB + logs |

## 12. Upgrade

```bash
cd /opt/go-on
git pull
cargo build --release --no-default-features -F profile-multi-users-server

pg_dump -U goon goon | gzip > /backup/goon/pre-upgrade-$(date +%Y%m%d).sql.gz

sudo systemctl stop go-on-multi
cp target/release/go-on backend/
sudo systemctl start go-on-multi

curl -H "Authorization: Bearer $GO_ON_ENTRY_API_KEY" https://your-domain.com/health
sudo journalctl -u go-on-multi -n 50 --no-pager
```
