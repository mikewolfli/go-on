#!/usr/bin/env bash
# Check cargo availability
set -euo pipefail
cargo --version 2>&1
echo "---CHECK_DONE---"
