#!/usr/bin/env bash
set -euo pipefail

COPY_MODE="0"
if [[ "${1:-}" == "--copy" ]]; then
  COPY_MODE="1"
  shift
fi

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 [--copy] <path-to-go-on-gui-app-or-go-on-backend-binary>"
  exit 1
fi

TARGET="$1"
if [[ ! -e "$TARGET" ]]; then
  echo "Target not found: $TARGET"
  exit 1
fi

WORK_TARGET="$TARGET"
if [[ "$COPY_MODE" == "1" ]]; then
  if [[ -d "$TARGET" ]]; then
    WORK_TARGET="${TARGET%.app}-local-signed.app"
    rm -rf "$WORK_TARGET"
    cp -R "$TARGET" "$WORK_TARGET"
  else
    WORK_TARGET="$TARGET.local-signed"
    cp "$TARGET" "$WORK_TARGET"
    chmod +x "$WORK_TARGET" || true
  fi
fi

# Remove Gatekeeper quarantine flag from downloaded artifacts.
xattr -dr com.apple.quarantine "$WORK_TARGET" || true

# Apply ad-hoc signature so macOS can validate code structure locally.
if command -v codesign >/dev/null 2>&1; then
  codesign --force --deep --sign - "$WORK_TARGET" || true
fi

echo "Unblock complete: $WORK_TARGET"
echo "You can run it now."
