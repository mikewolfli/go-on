#!/usr/bin/env bash
# Release Readiness Gate
# Validates production readiness before release across all 3 profiles.
set -euo pipefail

BINARY="${2:-./target/debug/go-on}"

echo "=== Release Readiness Gate ==="
echo ""

# ── Profiles to validate ─────────────────────────────────────────────────────
PROFILES=(
  "profile-local,backend-sqlite:config.local.toml"
  "profile-multi-users-server,backend-postgres:config.multi-users-server.toml"
  "profile-simple-server,backend-sqlite:config.simple-server.toml"
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

    # Check 1: Binary exists (first profile only — same binary, different features)
    if [ "$PROFILE" = "profile-local,backend-sqlite" ]; then
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
    cargo check --no-default-features -F "$PROFILE" || { echo "  FAIL"; OVERALL_EXIT=1; }
    echo "  PASS"

    # Check 4: Clippy
    echo "[4] Running clippy..."
    cargo clippy --no-default-features -F "$PROFILE" -- -D warnings || { echo "  FAIL"; OVERALL_EXIT=1; }
    echo "  PASS"

    # Check 5: Tests
    echo "[5] Running tests..."
    cargo test --no-default-features -F "$PROFILE" || { echo "  FAIL"; OVERALL_EXIT=1; }
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
