#!/usr/bin/env bash
# Verify local Zed integration readiness for go-on (Linux/macOS)
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SETTINGS_FILE="$ROOT_DIR/.zed/settings.json"
# Note: DOC directory was reorganized. Docs are now in docs/guides/.
LOCAL_BASE_URL="${1:-http://127.0.0.1:8090}"

pass() { echo "[PASS] $1"; }
fail() { echo "[FAIL] $1"; exit 1; }
step() { echo "== $1 =="; }

step "Zed integration file checks"
[[ -f "$SETTINGS_FILE" ]] || fail "missing .zed/settings.json"
pass "required Zed files exist"

step "Workspace settings schema checks"
rg -q '"agent_servers"' "$SETTINGS_FILE" || fail "agent_servers missing"
rg -q '"go-on"' "$SETTINGS_FILE" || fail "go-on agent server entry missing"
pass "workspace settings define the go-on agent server"

step "Docs consistency checks"
if rg -q '"auto_approve_tools"' "$SETTINGS_FILE"; then
  pass "settings include auto_approve_tools"
fi

step "Local endpoint smoke checks (optional)"
if command -v curl >/dev/null 2>&1; then
  if curl -fsS "$LOCAL_BASE_URL/health" >/dev/null 2>&1; then
    pass "endpoint reachable: $LOCAL_BASE_URL/health"
  else
    echo "[WARN] endpoint not reachable at $LOCAL_BASE_URL/health (start server to enable runtime verification)"
  fi
else
  echo "[WARN] curl not found, skipped runtime endpoint check"
fi

step "Result"
pass "Zed integration baseline verification completed"
