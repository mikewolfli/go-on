#!/usr/bin/env bash
# Clippy check — shows errors from clippy
set -euo pipefail
cd "$(dirname "$0")/.."
cargo clippy --all-targets -- -D warnings 2>&1 | grep -E "^error" | head -80 || true
echo "---CLIPPY_DONE---"
