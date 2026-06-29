#!/bin/bash
# This script checks for dead code using cargo check warnings
echo "=== Dead Code Scan ==="
echo "Running cargo check to find dead_code warnings..."
cargo check 2>&1 | grep "warning:.*dead_code" | sed 's/^/  /'
echo ""
echo "=== Unused items ==="
echo "To check for unused dependencies: cargo +nightly udeps (requires nightly toolchain)"
