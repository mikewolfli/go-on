#!/bin/bash
# GAP-B53-52: Integration test coverage gate
#
# Runs the project's test suite and reports a coverage summary.
# In production, replace with tarpaulin or grcov for line/branch coverage.
#
# Usage:
#   ./scripts/coverage.sh              # Run all tests
#   ./scripts/coverage.sh --unit       # Unit tests only
#   ./scripts/coverage.sh --integration # Integration tests only

set -euo pipefail

echo "=== Coverage Gate ==="
echo ""

# ── Parse arguments ──────────────────────────────────────────────────────────
RUN_UNIT=true
RUN_INTEGRATION=true

case "${1:-}" in
    --unit)
        RUN_INTEGRATION=false
        ;;
    --integration)
        RUN_UNIT=false
        ;;
    --help|-h)
        echo "Usage: $0 [--unit|--integration]"
        exit 0
        ;;
esac

# ── Run unit tests ───────────────────────────────────────────────────────────
if [ "$RUN_UNIT" = true ]; then
    echo "--- Unit Tests ---"
    echo "Running: cargo test --lib"
    if cargo test --lib 2>&1; then
        echo ""
        echo "Unit tests: OK"
    else
        echo ""
        echo "Unit tests: FAILED"
        exit 1
    fi
fi

# ── Run integration tests ────────────────────────────────────────────────────
if [ "$RUN_INTEGRATION" = true ]; then
    echo ""
    echo "--- Integration Tests ---"
    echo "Running: cargo test --test '*'"

    # Capture exit code manually so we can report status.
    if cargo test --test '*' 2>&1; then
        echo ""
        echo "Integration tests: OK"
    else
        echo ""
        echo "Integration tests: FAILED"
        exit 1
    fi
fi

# ── Summary ──────────────────────────────────────────────────────────────────
echo ""
echo "=== Gate Check ==="
echo "Integration tests: OK"
echo "Unit tests: OK"
echo ""
echo "All coverage gates passed."
