#!/bin/bash
# This script checks for dead code using cargo clippy with all targets
echo "=== Dead Code Scan ==="
echo "Running cargo clippy --all-targets to find dead_code warnings..."
cargo clippy --all-targets 2>&1 | grep "warning:.*dead_code" | sed 's/^/  /' || echo "  (no dead_code warnings found)"
echo ""
echo "=== Unused items ==="
echo "To check for unused dependencies: cargo +nightly udeps (requires nightly toolchain)"
