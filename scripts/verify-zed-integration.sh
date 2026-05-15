#!/usr/bin/env bash
# Verify local Zed integration readiness for go-on (Linux/macOS)
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SETTINGS_FILE="$ROOT_DIR/.zed/settings.json"
DOC_EN="$ROOT_DIR/DOC/en/src/zed.md"
DOC_ZH="$ROOT_DIR/DOC/zh-CN/src/zed.md"
LOCAL_BASE_URL="${1:-http://127.0.0.1:8080}"

pass() { echo "[PASS] $1"; }
fail() { echo "[FAIL] $1"; exit 1; }
step() { echo "== $1 =="; }

step "Zed integration file checks"
[[ -f "$SETTINGS_FILE" ]] || fail "missing .zed/settings.json"
[[ -f "$DOC_EN" ]] || fail "missing DOC/en/src/zed.md"
[[ -f "$DOC_ZH" ]] || fail "missing DOC/zh-CN/src/zed.md"
pass "required Zed files exist"

step "Workspace settings schema checks"
rg -q '"agent_servers"' "$SETTINGS_FILE" || fail "agent_servers missing"
rg -q '"language_models"' "$SETTINGS_FILE" || fail "language_models missing"
rg -q '"openai_compatible"' "$SETTINGS_FILE" || fail "openai_compatible provider missing"
rg -q '"available_models"' "$SETTINGS_FILE" || fail "available_models missing"
rg -q '"gpt-5\.5"' "$SETTINGS_FILE" || fail "gpt-5.5 model entry missing"
pass "workspace settings structure is valid"

step "Docs consistency checks"
rg -q 'openai_compatible' "$DOC_EN" || fail "EN doc does not mention openai_compatible"
rg -q 'openai_compatible' "$DOC_ZH" || fail "ZH doc does not mention openai_compatible"
rg -q -e 'type:\s*custom|"type"\s*:\s*"custom"' "$DOC_EN" || fail "EN doc does not mention custom provider type"
rg -q -e 'type:\s*custom|"type"\s*:\s*"custom"' "$DOC_ZH" || fail "ZH doc does not mention custom provider type"
pass "docs include current provider guidance"

step "Local endpoint smoke checks (optional)"
if command -v curl >/dev/null 2>&1; then
  if curl -fsS "$LOCAL_BASE_URL/v1/models" >/dev/null 2>&1; then
    pass "endpoint reachable: $LOCAL_BASE_URL/v1/models"
  else
    echo "[WARN] endpoint not reachable at $LOCAL_BASE_URL/v1/models (start server to enable runtime verification)"
  fi
else
  echo "[WARN] curl not found, skipped runtime endpoint check"
fi

step "Result"
pass "Zed integration baseline verification completed"
