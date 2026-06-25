#!/bin/bash
cd "$(dirname "$0")/.."
cargo clippy --all-targets -- -D warnings 2>&1 | grep -E "^error" | head -80
echo "---CLIPPY_DONE---"
