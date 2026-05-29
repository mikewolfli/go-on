#!/usr/bin/env bash
set -euo pipefail

# go-on Simple Server — Deploy Script

INSTALL_DIR="${INSTALL_DIR:-/opt/go-on}"
BUILD_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
BINARY="${BUILD_DIR}/target/release/go-on"
SERVICE_FILE="${BUILD_DIR}/deploy/simple-server/go-on.service"
CONFIG_TEMPLATE="${BUILD_DIR}/config/config.simple-server.toml"

echo "=== go-on Simple Server Deploy ==="
echo "Install dir: ${INSTALL_DIR}/backend"

# 1. Create directories
sudo mkdir -p "${INSTALL_DIR}/backend"
# Use colon-separated user (no group) for compatibility with systems
# where the user's primary group name differs from the username.
sudo chown "$USER:" "${INSTALL_DIR}" -R

# 2. Build
echo "Building..."
cd "$BUILD_DIR"
cargo build --release --no-default-features -F profile-simple-server 2>&1 | tail -5

# 3. Copy binary
cp "$BINARY" "${INSTALL_DIR}/backend/"

# 4. Deploy config
if [ ! -f "${INSTALL_DIR}/backend/config.toml" ]; then
    cp "$CONFIG_TEMPLATE" "${INSTALL_DIR}/backend/config.toml"
    echo "Config deployed. Edit ${INSTALL_DIR}/backend/config.toml to set API keys."
else
    echo "Config exists at ${INSTALL_DIR}/backend/config.toml — keeping existing."
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
echo "  2. Verify: ${INSTALL_DIR}/backend/go-on -c ${INSTALL_DIR}/backend/config.toml --validate-config"
echo "  3. Start: sudo systemctl start go-on"
echo "  4. Check: curl http://127.0.0.1:8090/health"
