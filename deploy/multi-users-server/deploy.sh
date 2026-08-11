#!/usr/bin/env bash
set -euo pipefail

# go-on Multi-Users Server — Deploy Script

INSTALL_DIR="${INSTALL_DIR:-/opt/go-on}"
BUILD_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
BINARY="${BUILD_DIR}/target/release/go-on"
SERVICE_NAME="go-on-multi"
SERVICE_FILE="${BUILD_DIR}/deploy/multi-users-server/go-on-multi.service"
CONFIG_TEMPLATE="${BUILD_DIR}/config/config.multi-users-server.toml"

# ── Pre-flight checks ────────────────────────────────────────────────

if [ ! -f "$BINARY" ]; then
    echo "WARNING: Binary not found at $BINARY — will build first."
fi

# Idempotency: stop running service before deploy
if command -v systemctl &>/dev/null; then
    if systemctl is-active --quiet "$SERVICE_NAME"; then
        echo "WARNING: $SERVICE_NAME is already running. Stopping before deploy..."
        sudo systemctl stop "$SERVICE_NAME"
    fi
fi

echo "=== go-on Multi-Users Server Deploy ==="
echo "Install dir: ${INSTALL_DIR}/backend"

# 1. Create directories
sudo mkdir -p "${INSTALL_DIR}/backend"
# Data directory used by the config template (cache/vector/memory stores).
sudo mkdir -p /var/lib/go-on
sudo chown "go-on:go-on" /var/lib/go-on
# Ensure go-on user exists (matches systemd service User=go-on)
sudo id -u go-on &>/dev/null || sudo useradd -r -s /sbin/nologin go-on
sudo chown "go-on:go-on" "${INSTALL_DIR}" -R

# 2. Build
echo "Building..."
cd "$BUILD_DIR"
cargo build --release --no-default-features --features multi-users-server 2>&1 && \
    echo "Build OK" || { echo "BUILD FAILED"; exit 1; }

# 3. Copy binary
if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found at $BINARY after build!"
    exit 1
fi
cp -f "$BINARY" "${INSTALL_DIR}/backend/"
echo "Binary copied."

# 4. Deploy config (preserve existing)
if [ ! -f "${INSTALL_DIR}/backend/config.toml" ]; then
    cp "$CONFIG_TEMPLATE" "${INSTALL_DIR}/backend/config.toml"
    echo "Config deployed."
else
    echo "Config exists — keeping existing."
fi

# 5. Environment file — must be pre-created by admin
if [ ! -f "${INSTALL_DIR}/backend/environment" ]; then
  echo "ERROR: Environment file not found at ${INSTALL_DIR}/backend/environment"
  echo "Create it manually with the required variables:"
  echo "  GO_ON_PG_CONNECTION_STRING=postgres://USER:PASS@HOST:5432/goon?sslmode=require"
  echo "  GO_ON_ENTRY_API_KEY=<your-api-key>"
  echo "  DEEPSEEK_API_KEY=<your-key>"
  echo ""
  echo "The legacy DB_HOST/DB_PORT/DB_USER/DB_PASS/DB_NAME variables are NOT read by"
  echo "the binary — the PostgreSQL DSN is resolved via GO_ON_PG_CONNECTION_STRING"
  echo "(fallbacks: DATABASE_URL, PG_DSN, GO_ON_DATABASE_URL)."
  exit 1
else
  echo "Environment file exists — keeping existing."
fi

# 6. Install systemd service
if command -v systemctl &>/dev/null; then
    sudo cp "$SERVICE_FILE" /etc/systemd/system/go-on-multi.service
    sudo systemctl daemon-reload
    echo "systemd service installed."
fi

echo ""
echo "=== Deploy complete ==="
echo "Binary: ${INSTALL_DIR}/backend/go-on"
echo "Config: ${INSTALL_DIR}/backend/config.toml"
echo "Environ: ${INSTALL_DIR}/backend/environment"
echo ""
echo "Next steps:"
echo "  1. Edit environment: vim ${INSTALL_DIR}/backend/environment"
echo "  2. Edit config: vim ${INSTALL_DIR}/backend/config.toml"
echo "  3. Verify: sudo -u go-on ${INSTALL_DIR}/backend/go-on -c ${INSTALL_DIR}/backend/config.toml --doctor"
echo "  4. Start: sudo systemctl start go-on-multi"
echo "  5. Check: curl -H 'Authorization: Bearer \${GO_ON_ENTRY_API_KEY}' http://127.0.0.1:8090/health"
