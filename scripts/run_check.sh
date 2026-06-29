#!/usr/bin/env bash
# Run clippy check with step tracking (CI use)
set -euo pipefail
cd "$(dirname "$0")/.."
cargo clippy --all-targets -- -D warnings 2>&1 | grep -E "^error" | head -80 || true
echo "---CLIPPY_DONE---"
