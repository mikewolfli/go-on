#!/usr/bin/env bash
set -euo pipefail

# go-on Multi-Users Server — Deploy Script

INSTALL_DIR="${INSTALL_DIR:-/opt/go-on}"
BUILD_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
BINARY="${BUILD_DIR}/target/release/go-on"
SERVICE_FILE="${BUILD_DIR}/deploy/multi-users-server/go-on-multi.service"
CONFIG_TEMPLATE="${BUILD_DIR}/config/config.multi-users-server.toml"

echo "=== go-on Multi-Users Server Deploy ==="
echo "Install dir: ${INSTALL_DIR}/backend"

# 1. Create directories
sudo mkdir -p "${INSTALL_DIR}/backend"
# Ensure go-on user exists (matches systemd service User=go-on)
sudo id -u go-on &>/dev/null || sudo useradd -r -s /sbin/nologin go-on
sudo chown "go-on:go-on" "${INSTALL_DIR}" -R

# 2. Build
echo "Building..."
cd "$BUILD_DIR"
cargo build --release --no-default-features -F profile-multi-users-server 2>&1 && \
    echo "Build OK" || { echo "BUILD FAILED"; exit 1; }

# 3. Copy binary
cp "$BINARY" "${INSTALL_DIR}/backend/"

# 4. Deploy config
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
  echo "  DB_HOST=localhost"
  echo "  DB_PORT=5432"
  echo "  DB_USER=goon"
  echo "  DB_PASS=<your-password>"
  echo "  DB_NAME=goon"
  echo "  GO_ON_ENTRY_API_KEY=<your-api-key>"
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
echo "  3. Verify: sudo -u go-on ${INSTALL_DIR}/backend/go-on -c ${INSTALL_DIR}/backend/config.toml --validate-config"
echo "  4. Start: sudo systemctl start go-on-multi"
echo "  5. Check: curl -H 'Authorization: Bearer \${GO_ON_ENTRY_API_KEY}' http://127.0.0.1:8090/health"
