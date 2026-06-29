#!/usr/bin/env bash
# Run clippy and capture errors
set -euo pipefail
cd "$(dirname "$0")/.."
cargo clippy --all-targets -- -D warnings 2>&1 | grep -E "^error" | head -120 || true
echo "---DONE---"
