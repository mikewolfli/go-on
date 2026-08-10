#!/usr/bin/env bash
set -euo pipefail

# go-on Simple Server — Deploy Script

INSTALL_DIR="${INSTALL_DIR:-/opt/go-on}"
BUILD_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
BINARY="${BUILD_DIR}/target/release/go-on"
SERVICE_NAME="go-on"
SERVICE_FILE="${BUILD_DIR}/deploy/simple-server/go-on.service"
CONFIG_TEMPLATE="${BUILD_DIR}/config/config.simple-server.toml"

# ── Pre-flight checks ────────────────────────────────────────────────

# Check if go-on binary exists
if [ ! -f "$BINARY" ]; then
    echo "WARNING: Binary not found at $BINARY — will build first."
fi

# Check if service is already running (idempotency)
if command -v systemctl &>/dev/null; then
    if systemctl is-active --quiet "$SERVICE_NAME"; then
        echo "WARNING: go-on service is already running. Stopping before deploy..."
        sudo systemctl stop "$SERVICE_NAME"
    fi
fi

echo "=== go-on Simple Server Deploy ==="
echo "Install dir: ${INSTALL_DIR}/backend"

# 1. Create directories
sudo mkdir -p "${INSTALL_DIR}/backend"
# Data directory used by the config template (cache/vector/memory stores).
sudo mkdir -p /var/lib/go-on
sudo chown "go-on:go-on" /var/lib/go-on
# Ensure go-on user exists (matches systemd service User=go-on)
sudo id -u go-on &>/dev/null || sudo useradd -r -s /sbin/nologin go-on
sudo chown "go-on:go-on" "${INSTALL_DIR}" -R

# 2. Build (skip if binary already exists and is newer than source)
echo "Building..."
cd "$BUILD_DIR"
cargo build --release --no-default-features --features simple-server 2>&1 && \
    echo "Build OK" || { echo "BUILD FAILED"; exit 1; }

# 3. Copy binary (verify it exists after build)
if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found at $BINARY after build!"
    exit 1
fi
cp -f "$BINARY" "${INSTALL_DIR}/backend/"
echo "Binary copied."

# 4. Deploy config (preserve existing)
if [ ! -f "${INSTALL_DIR}/backend/config.toml" ]; then
    cp "$CONFIG_TEMPLATE" "${INSTALL_DIR}/backend/config.toml"
    echo "Config deployed. Edit ${INSTALL_DIR}/backend/config.toml to set API keys."
else
    echo "Config exists at ${INSTALL_DIR}/backend/config.toml — keeping existing."
fi

# 4b. Environment file — must be pre-created by admin
if [ ! -f "${INSTALL_DIR}/backend/environment" ]; then
  echo "ERROR: Environment file not found at ${INSTALL_DIR}/backend/environment"
  echo "Create it manually with the required variables:"
  echo "  GO_ON_SERVER_API_KEY=<your-api-key>"
  exit 1
else
  echo "Environment file exists — keeping existing."
fi

# 5. Install systemd service
if command -v systemctl &>/dev/null; then
    sudo cp "$SERVICE_FILE" /etc/systemd/system/go-on.service
    sudo systemctl daemon-reload
    echo "systemd service installed."
    echo "Run: sudo systemctl enable go-on && sudo systemctl start go-on"
fi

echo ""
echo "=== Deploy complete ==="
echo "Binary: ${INSTALL_DIR}/backend/go-on"
echo "Config: ${INSTALL_DIR}/backend/config.toml"
echo ""
echo "Next steps:"
echo "  1. Edit config: vim ${INSTALL_DIR}/backend/config.toml"
echo "  2. Verify: ${INSTALL_DIR}/backend/go-on -c ${INSTALL_DIR}/backend/config.toml --doctor"
echo "  3. Start: sudo systemctl start go-on"
echo "  4. Check: curl http://127.0.0.1:8090/health"
