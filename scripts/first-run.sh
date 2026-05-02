#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")" && pwd)"
OS_NAME="$(uname -s || echo unknown)"

if [[ "$OS_NAME" != "Darwin" ]]; then
  echo "No macOS trust bootstrap needed on $OS_NAME."
  exit 0
fi

UNBLOCK_SCRIPT="$ROOT_DIR/macos-gui-unblock.sh"
if [[ ! -x "$UNBLOCK_SCRIPT" ]]; then
  if [[ -f "$UNBLOCK_SCRIPT" ]]; then
    chmod +x "$UNBLOCK_SCRIPT" || true
  else
    echo "macOS unblock helper not found: $UNBLOCK_SCRIPT"
    exit 1
  fi
fi

# Support both package layouts:
# 1) root contains backend/ and optional *.app
# 2) script is inside backend/ folder
BACKEND_BIN=""
if [[ -x "$ROOT_DIR/backend/go-on" ]]; then
  BACKEND_BIN="$ROOT_DIR/backend/go-on"
elif [[ -x "$ROOT_DIR/go-on" ]]; then
  BACKEND_BIN="$ROOT_DIR/go-on"
fi

if [[ -n "$BACKEND_BIN" ]]; then
  "$UNBLOCK_SCRIPT" --copy "$BACKEND_BIN"
fi

APP_BUNDLE="$(find "$ROOT_DIR" -maxdepth 2 -type d -name "*.app" | head -1 || true)"
if [[ -n "$APP_BUNDLE" ]]; then
  "$UNBLOCK_SCRIPT" --copy "$APP_BUNDLE"
fi

echo "macOS first-run trust bootstrap finished."
