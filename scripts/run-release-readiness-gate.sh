#!/usr/bin/env bash
# Release Readiness Gate
# Validates production readiness before release across all 3 profiles.
set -euo pipefail

BINARY="${2:-./target/debug/go-on}"

echo "=== Release Readiness Gate ==="
echo ""

# ── Profiles to validate ─────────────────────────────────────────────────────
# Each entry: "<cargo-features>:<config-file>"
# Use --no-default-features --features to ensure exactly one profile is selected.
PROFILES=(
  "local:config.toml"
  "multi-users-server:config.multi-users-server.toml"
  "simple-server:config.simple-server.toml"
  "full:config.toml"
)

OVERALL_EXIT=0

for ENTRY in "${PROFILES[@]}"; do
    PROFILE="${ENTRY%%:*}"
    CONFIG="${ENTRY#*:}"
    CONFIG_PATH="config/$CONFIG"

    echo "========================================="
    echo "  Profile: $PROFILE"
    echo "  Config:  $CONFIG_PATH"
    echo "========================================="
    echo ""

    # Check 1: Binary exists (first profile only)
    if [ "$PROFILE" = "local" ]; then
        echo "[1] Checking binary..."
        if [ ! -f "$BINARY" ]; then
            echo "  FAIL: Binary not found at $BINARY"
            exit 1
        fi
        echo "  PASS"
    else
        echo "[1] SKIP (binary check already passed)"
    fi

    # Check 2: Config exists
    echo "[2] Checking config..."
    if [ ! -f "$CONFIG_PATH" ]; then
        echo "  FAIL: Config not found at $CONFIG_PATH"
        exit 1
    fi
    echo "  PASS"

    # Check 3: Cargo check
    echo "[3] Running cargo check..."
    cargo check --no-default-features --features "$PROFILE" || { echo "  FAIL"; OVERALL_EXIT=1; }
    echo "  PASS"

    # Check 4: Clippy
    echo "[4] Running clippy..."
    cargo clippy --no-default-features --features "$PROFILE" -- -D warnings || { echo "  FAIL"; OVERALL_EXIT=1; }
    echo "  PASS"

    # Check 5: Tests
    echo "[5] Running tests..."
    cargo test --lib --no-default-features --features "$PROFILE" || { echo "  FAIL"; OVERALL_EXIT=1; }
    echo "  PASS"

    echo ""
done

echo "========================================="
if [ $OVERALL_EXIT -eq 0 ]; then
    echo "=== All gates PASSED ==="
    echo "Ready for release."
else
    echo "=== Some gates FAILED ==="
    exit $OVERALL_EXIT
fi
