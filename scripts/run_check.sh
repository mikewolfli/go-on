#!/bin/bash
set -e
echo "step1" > /tmp/clippy_output.txt
cd "$(dirname "$0")/.."
echo "step2" >> /tmp/clippy_output.txt
cargo clippy --all-targets -- -D warnings > /tmp/clippy_full.txt 2>&1
echo "step3" >> /tmp/clippy_output.txt
grep -E "^error" /tmp/clippy_full.txt > /tmp/clippy_errors.txt 2>&1 || true
echo "step4" >> /tmp/clippy_output.txt
cat /tmp/clippy_errors.txt
echo "---CLIPPY_DONE---"
