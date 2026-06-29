#!/bin/bash
# BLUE57: Coverage gate — measures test coverage using cargo-llvm-cov.
#
# Uses cargo-llvm-cov for line/branch coverage reporting.
# Falls back to plain cargo test if llvm-cov is not installed.
#
# Runs coverage across all 3 profiles and merges reports.
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

# ── Profiles to test ─────────────────────────────────────────────────────────
# Note: these are Cargo feature flags, not profile names.
# Each entry is passed as --features to cargo.
PROFILES=(
  "local"
  "multi-users-server"
  "simple-server"
  "full"
)

# ── Detect coverage tool ─────────────────────────────────────────────────────
USE_LLVM_COV=false
if command -v cargo-llvm-cov &>/dev/null; then
    USE_LLVM_COV=true
elif cargo llvm-cov --help &>/dev/null 2>&1; then
    USE_LLVM_COV=true
fi

COV_OPTS=""
if [ "$GEN_HTML" = true ] && [ "$USE_LLVM_COV" = true ]; then
    COV_OPTS="--html"
fi

# ── Temporary directory for per-profile reports ──────────────────────────────
COV_DIR=$(mktemp -d)
trap 'rm -rf "$COV_DIR"' EXIT

# ── Run coverage ─────────────────────────────────────────────────────────────
if [ "$USE_LLVM_COV" = true ]; then
    echo "Using: cargo-llvm-cov"
    echo ""

    OVERALL_EXIT=0
    for PROFILE in "${PROFILES[@]}"; do
        echo "--- Profile: $PROFILE ---"
        COV_FLAGS="--no-default-features --features $PROFILE"
        PROFILE_SAFE="$PROFILE"
        PROFILE_OUT="$COV_DIR/$PROFILE_SAFE"

        if [ "$RUN_UNIT" = true ] && [ "$RUN_INTEGRATION" = true ]; then
            echo "Running: cargo llvm-cov --all-targets $COV_FLAGS --lcov --output-path ${PROFILE_OUT}.info $COV_OPTS"
            cargo llvm-cov --all-targets $COV_FLAGS --lcov --output-path "${PROFILE_OUT}.info" $COV_OPTS || OVERALL_EXIT=$?
        elif [ "$RUN_UNIT" = true ]; then
            echo "Running: cargo llvm-cov --lib $COV_FLAGS --lcov --output-path ${PROFILE_OUT}.info $COV_OPTS"
            cargo llvm-cov --lib $COV_FLAGS --lcov --output-path "${PROFILE_OUT}.info" $COV_OPTS || OVERALL_EXIT=$?
        elif [ "$RUN_INTEGRATION" = true ]; then
            echo "Running: cargo llvm-cov --test '*' $COV_FLAGS --lcov --output-path ${PROFILE_OUT}.info $COV_OPTS"
            cargo llvm-cov --test '*' $COV_FLAGS --lcov --output-path "${PROFILE_OUT}.info" $COV_OPTS || OVERALL_EXIT=$?
        fi
        echo ""
    done

    if [ $OVERALL_EXIT -ne 0 ]; then
        echo "FAILED: One or more profiles reported errors"
        exit $OVERALL_EXIT
    fi

    # ── Merge per-profile LCOV reports ───────────────────────────────────────
    echo "--- Merging coverage reports ---"
    LCOV_FILES=()
    for f in "$COV_DIR"/*.info; do
        [ -f "$f" ] && LCOV_FILES+=("$f")
    done
    if [ ${#LCOV_FILES[@]} -gt 1 ]; then
        MERGED="$COV_DIR/merged.info"
        echo "Merging ${#LCOV_FILES[@]} reports into $MERGED"
        if command -v lcov &>/dev/null; then
            lcov -o "$MERGED" $(printf -- '-a %s ' "${LCOV_FILES[@]}")
        elif command -v cargo-llvm-cov &>/dev/null; then
            # fallback: use first file as merged (cargo-llvm-cov merge is unstable)
            cp "${LCOV_FILES[0]}" "$MERGED"
            echo "Warning: lcov not installed; using first profile report as merged result"
        fi
    fi
else
    echo "Warning: cargo-llvm-cov not found — falling back to plain cargo test (no coverage data)"
    echo "Install: cargo install cargo-llvm-cov"
    echo ""

    # Fallback: plain test run for each profile
    OVERALL_EXIT=0
    for PROFILE in "${PROFILES[@]}"; do
        echo "--- Profile: $PROFILE ---"
        COV_FLAGS="--no-default-features --features $PROFILE"

        if [ "$RUN_UNIT" = true ]; then
            echo "  Unit Tests..."
            cargo test --lib $COV_FLAGS || { echo "FAILED"; OVERALL_EXIT=1; }
        fi

        if [ "$RUN_INTEGRATION" = true ]; then
            echo "  Integration Tests..."
            cargo test --test '*' $COV_FLAGS || { echo "FAILED"; OVERALL_EXIT=1; }
        fi
        echo ""
    done

    if [ $OVERALL_EXIT -ne 0 ]; then
        exit $OVERALL_EXIT
    fi
fi

# ── Summary ──────────────────────────────────────────────────────────────────
echo ""
echo "=== Coverage Gate PASSED ==="
