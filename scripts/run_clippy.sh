#!/usr/bin/env bash
# Run clippy and capture errors
#
# Consolidated script: replaces the former run_clippy.sh, clippy_check.sh,
# and run_check.sh (all identical with minor variation).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo clippy --all-targets -- -D warnings 2>&1 | grep -E "^error" | head -120 || true
echo "---DONE---"
