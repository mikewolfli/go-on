#!/bin/bash
# BLUE57: Coverage gate — measures test coverage using cargo-llvm-cov.
#
# Uses cargo-llvm-cov for line/branch coverage reporting.
# Falls back to plain cargo test if llvm-cov is not installed.
#
# Usage:
#   ./scripts/coverage.sh              # Full coverage (unit + integration)
#   ./scripts/coverage.sh --unit       # Unit tests only
#   ./scripts/coverage.sh --integration # Integration tests only
#   ./scripts/coverage.sh --html       # Generate HTML report

set -euo pipefail

echo "=== Coverage Gate ==="
echo ""

# ── Parse arguments ──────────────────────────────────────────────────────────
RUN_UNIT=true
RUN_INTEGRATION=true
GEN_HTML=false

case "${1:-}" in
    --unit)
        RUN_INTEGRATION=false
        ;;
    --integration)
        RUN_UNIT=false
        ;;
    --html)
        GEN_HTML=true
        ;;
    --help|-h)
        echo "Usage: $0 [--unit|--integration|--html]"
        exit 0
        ;;
esac

# ── Detect coverage tool ─────────────────────────────────────────────────────
USE_LLVM_COV=false
if command -v cargo-llvm-cov &>/dev/null; then
    USE_LLVM_COV=true
elif cargo llvm-cov --help &>/dev/null 2>&1; then
    USE_LLVM_COV=true
fi

# ── Common flags ─────────────────────────────────────────────────────────────
COV_FLAGS="--features profile-local,backend-sqlite"
COV_OPTS=""

if [ "$GEN_HTML" = true ] && [ "$USE_LLVM_COV" = true ]; then
    COV_OPTS="--html"
fi

# ── Run coverage ─────────────────────────────────────────────────────────────
if [ "$USE_LLVM_COV" = true ]; then
    echo "Using: cargo-llvm-cov"

    if [ "$RUN_UNIT" = true ] && [ "$RUN_INTEGRATION" = true ]; then
        echo "Running: cargo llvm-cov --all-targets $COV_FLAGS $COV_OPTS"
        cargo llvm-cov --all-targets $COV_FLAGS $COV_OPTS
    elif [ "$RUN_UNIT" = true ]; then
        echo "Running: cargo llvm-cov --lib $COV_FLAGS $COV_OPTS"
        cargo llvm-cov --lib $COV_FLAGS $COV_OPTS
    elif [ "$RUN_INTEGRATION" = true ]; then
        echo "Running: cargo llvm-cov --test '*' $COV_FLAGS $COV_OPTS"
        cargo llvm-cov --test '*' $COV_FLAGS $COV_OPTS
    fi
else
    echo "Warning: cargo-llvm-cov not found — falling back to plain cargo test (no coverage data)"
    echo "Install: cargo install cargo-llvm-cov"
    echo ""

    # Fallback: plain test run
    if [ "$RUN_UNIT" = true ]; then
        echo "--- Unit Tests ---"
        cargo test --lib $COV_FLAGS || { echo "FAILED"; exit 1; }
        echo "Unit tests: OK"
    fi

    if [ "$RUN_INTEGRATION" = true ]; then
        echo ""
        echo "--- Integration Tests ---"
        cargo test --test '*' $COV_FLAGS || { echo "FAILED"; exit 1; }
        echo "Integration tests: OK"
    fi
fi

# ── Summary ──────────────────────────────────────────────────────────────────
echo ""
echo "=== Coverage Gate PASSED ==="
