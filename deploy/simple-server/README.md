# go-on Simple Server — Deployment Guide

Single-server deployment using SQLite + sqlite-vec as backend storage.
Suitable for small to medium teams (≤50 concurrent users).

## Prerequisites

| Component | Version | Note |
|-----------|---------|------|
| Linux / macOS | — | Windows OK for dev, Linux recommended for production |
| Rust | ≥ 1.80 | `rustup update stable` |
| libsqlite3-dev | ≥ 3.40 | `apt install libsqlite3-dev` (Ubuntu/Debian) |
| systemd / supervisor | — | Process management |

## 1. Build

```bash
git clone <repo-url> /opt/go-on
cd /opt/go-on

cargo build --release --no-default-features -F simple-server

# Optional: Build the GUI
cargo build --release --manifest-path gui/Cargo.toml
```

Artifacts:
```
target/release/go-on                          # Backend CLI (with --chat mode)
gui/target/release/go-on-gui-egui             # Desktop GUI (optional)
```

## 2. Directory Layout

```
/opt/go-on/
├── backend/
│   ├── go-on                        # Backend binary
│   ├── go-on-gui-egui              # GUI (optional)
│   ├── config.toml                 # Main config
│   ├── backend.log                 # Runtime log (auto-created)
│   ├── acp_cache.sqlite3           # Cache DB (auto-created)
│   └── acp_vector.sqlite3          # Vector DB (auto-created)
├── deploy/
│   └── simple-server/
│       ├── deploy.sh               # One-click deploy script
│       ├── go-on.service           # systemd unit
│       └── README.md               # This file
└── config/
    └── config.simple-server.toml   # Template config
```

## 3. Deployment Steps

### 3.1 Create directories

```bash
sudo mkdir -p /opt/go-on/backend
sudo chown $USER:$USER /opt/go-on -R
```

### 3.2 Copy the binary

```bash
cp target/release/go-on /opt/go-on/backend/
```

### 3.3 Deploy configuration

```bash
cp config/config.simple-server.toml /opt/go-on/backend/config.toml
vim /opt/go-on/backend/config.toml
```

Configure your AI provider under `[agents]`, for example:

```toml
[agents.deepseek]
type = "deepseek"
api_key_env = "DEEPSEEK_API_KEY"    # or keyring://go-on/deepseek_api_key
model = "deepseek-v4-flash"
supports_system = true
```

### 3.4 Configure API Key

**Option A: Environment variable (recommended)**

```bash
export DEEPSEEK_API_KEY="sk-xxxxx"
```

Add the `export` line to `/opt/go-on/backend/start.sh`.

**Option B: System keyring**

```bash
/opt/go-on/backend/go-on --secret set --secret-name deepseek --secret-value "sk-xxxxx"
```

### 3.5 Install systemd service

```bash
sudo cp deploy/simple-server/go-on.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable go-on
sudo systemctl start go-on
sudo systemctl status go-on
sudo journalctl -u go-on -f
```

### 3.6 Verify

```bash
# Health check
curl http://127.0.0.1:8090/health

# Validate config
/opt/go-on/backend/go-on -c /opt/go-on/backend/config.toml --validate-config

# Runtime status
/opt/go-on/backend/go-on -c /opt/go-on/backend/config.toml --status
```

## 4. Docker Deployment

```bash
# Build image
docker build -f deploy/simple-server/Dockerfile -t go-on:simple .

# Run with docker compose
docker compose -f deploy/simple-server/docker-compose.yml up -d

# With API keys
DEEPSEEK_API_KEY=sk-xxx \
  docker compose -f deploy/simple-server/docker-compose.yml up -d
```

## 5. Operations

### Start / Stop / Restart

```bash
sudo systemctl start go-on
sudo systemctl stop go-on
sudo systemctl restart go-on
```

### Logs

```bash
sudo journalctl -u go-on -n 100 -f
tail -f /opt/go-on/backend/backend.log
```

### Configuration update

Edit `/opt/go-on/backend/config.toml` then restart:

```bash
sudo systemctl restart go-on
```

### Data backup

```bash
cp /opt/go-on/backend/acp_cache.sqlite3 /backup/go-on/cache-$(date +%Y%m%d).sqlite3
cp /opt/go-on/backend/acp_vector.sqlite3 /backup/go-on/vector-$(date +%Y%m%d).sqlite3
```

### Terminal chat mode

```bash
cd /opt/go-on/backend && ./go-on -a
```

## 6. Config Reference

Key fields from `config/config.simple-server.toml`:

| Field | Default | Description |
|-------|---------|-------------|
| `runtime.acp_http_bind_addr` | `127.0.0.1:8090` | Listen address |
| `runtime.entry_auth_enabled` | `true` | Entry authentication |
| `runtime.entry_auth_api_key_env` | `GO_ON_SERVER_API_KEY` | API key env var name |
| `runtime.entry_rate_limit_rpm` | `1000` | Requests per minute limit |
| `runtime.entry_rate_limit_burst` | `200` | Burst limit |
| `runtime.production_strict` | `true` | Strict mode |
| `cache.max_entries` | `20000` | Max cache entries |
| `vector.max_entries` | `50000` | Max vector entries |

## 7. Monitoring

| Metric | Threshold | Description |
|--------|-----------|-------------|
| Process alive | — | systemd auto-restart |
| Health check | Alert after 3 failures | `curl http://127.0.0.1:8090/health` |
| Disk usage | > 80% | SQLite DB growth |
| p95 latency | > 10s | AI provider response time |
| Error rate | > 5% | RPC error ratio |

## 8. Upgrade

```bash
cd /opt/go-on
git pull
cargo build --release --no-default-features -F simple-server

cp backend/acp_cache.sqlite3 /backup/go-on/
cp backend/acp_vector.sqlite3 /backup/go-on/

sudo systemctl stop go-on
cp target/release/go-on backend/
sudo systemctl start go-on

curl http://127.0.0.1:8090/health
```
