#!/usr/bin/env bash
# Release Readiness Gate
# Validates production readiness before release.
set -euo pipefail

CONFIG="${1:-config.production.toml}"
BINARY="${2:-./target/debug/go-on}"

echo "=== Release Readiness Gate ==="
echo "Config: $CONFIG"
echo "Binary: $BINARY"
echo ""

# Check 1: Binary exists
echo "[1/5] Checking binary..."
if [ ! -f "$BINARY" ]; then
    echo "  FAIL: Binary not found at $BINARY"
    exit 1
fi
echo "  PASS"

# Check 2: Config exists
echo "[2/5] Checking config..."
if [ ! -f "$CONFIG" ]; then
    echo "  FAIL: Config not found at $CONFIG"
    exit 1
fi
echo "  PASS"

# Check 3: Cargo check
echo "[3/5] Running cargo check..."
cargo check --no-default-features -F profile-multi-users-server 2>/dev/null
echo "  PASS"

# Check 4: Clippy
echo "[4/5] Running clippy..."
cargo clippy --no-default-features -F profile-multi-users-server -- -D warnings 2>/dev/null
echo "  PASS"

# Check 5: Tests
echo "[5/5] Running tests..."
cargo test --no-default-features -F profile-multi-users-server 2>/dev/null
echo "  PASS"

echo ""
echo "=== All gates PASSED ==="
echo "Ready for release."
