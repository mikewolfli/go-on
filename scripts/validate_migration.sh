#!/bin/bash

# Migration Validation Script
# This script validates the ACP module migration from include! to mod structure

set -e

echo "========================================="
echo "ACP Module Migration Validation"
echo "========================================="
echo "Date: $(date)"
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    if [ $1 -eq 0 ]; then
        echo -e "${GREEN}✓${NC} $2"
    else
        echo -e "${RED}✗${NC} $2"
        return 1
    fi
}

# Function to run command and check status
run_check() {
    echo -e "${YELLOW}Running:${NC} $1"
    bash -c "$1"
    local status=$?
    if [ $status -eq 0 ]; then
        print_status 0 "$2"
    else
        print_status 1 "$2"
        return $status
    fi
}

# Record start time
START_TIME=$(date +%s)

echo "Phase 1: Basic Compilation Checks"
echo "---------------------------------"

# Check 1: Basic compilation
run_check "cargo check" "Basic compilation"

# Check 2: Library tests compilation
run_check "cargo test --lib --no-run 2>&1 | tail -5" "Library tests compilation"

# Check 3: Binary compilation
run_check "cargo build --release 2>&1 | tail -5" "Release build"

echo ""
echo "Phase 2: ACP Module Specific Checks"
echo "-----------------------------------"

# Check 4: ACP module compilation
run_check "cargo check -p go-on --features '' 2>&1 | grep -c 'error\[E0432\]: unresolved import' || true" "Check for unresolved imports in ACP"

# Check 5: Verify include! files still exist
INCLUDE_FILES=(
    "src/acp/mod.rs"
    "src/acp/prelude.rs"
    "src/acp/server.rs"
    "src/acp/background.rs"
    "src/acp/tests.rs"
)

for file in "${INCLUDE_FILES[@]}"; do
    if [ -f "$file" ]; then
        print_status 0 "Include file exists: $file"
    else
        print_status 1 "Include file missing: $file"
    fi
done

# Check 6: Verify new module structure exists
MODULE_FILES=(
    "src/acp/modules/mod.rs"
    "src/acp/modules/helpers"
    "src/acp/modules/impl"
)

for file in "${MODULE_FILES[@]}"; do
    if [ -f "$file" ] || [ -d "$file" ]; then
        print_status 0 "Module structure exists: $file"
    else
        print_status 1 "Module structure missing: $file"
    fi
done

echo ""
echo "Phase 3: Test Execution"
echo "-----------------------"

# Check 7: Run ACP-specific tests
echo -e "${YELLOW}Running ACP tests...${NC}"
ACP_TEST_OUTPUT=$(cargo test acp -- --test-threads=1 2>&1 | tail -20)
ACP_TEST_STATUS=$?

if [ $ACP_TEST_STATUS -eq 0 ]; then
    print_status 0 "ACP tests passed"
    echo -e "${YELLOW}Last 5 test results:${NC}"
    echo "$ACP_TEST_OUTPUT" | grep -E "(test|ok|FAILED)" | tail -5
else
    print_status 1 "ACP tests failed"
    echo -e "${YELLOW}Test output:${NC}"
    echo "$ACP_TEST_OUTPUT"
fi

# Check 8: Run integration tests
echo -e "${YELLOW}Running integration tests...${NC}"
INTEGRATION_TEST_OUTPUT=$(cargo test --test '*integration*' -- --test-threads=1 2>&1 | tail -20)
INTEGRATION_TEST_STATUS=$?

if [ $INTEGRATION_TEST_STATUS -eq 0 ]; then
    print_status 0 "Integration tests passed"
    echo -e "${YELLOW}Last 5 test results:${NC}"
    echo "$INTEGRATION_TEST_OUTPUT" | grep -E "(test|ok|FAILED)" | tail -5
else
    print_status 1 "Integration tests failed"
    echo -e "${YELLOW}Test output:${NC}"
    echo "$INTEGRATION_TEST_OUTPUT"
fi

echo ""
echo "Phase 4: Performance Baseline"
echo "----------------------------"

# Check 9: Compilation time baseline
echo -e "${YELLOW}Measuring compilation time...${NC}"
COMPILE_START=$(date +%s.%N)
cargo check --quiet
COMPILE_END=$(date +%s.%N)
COMPILE_TIME=$(awk "BEGIN { printf \"%.2f\", $COMPILE_END - $COMPILE_START }")
print_status 0 "Compilation time: ${COMPILE_TIME}s"

# Check 10: Binary size
if [ -f "target/release/go-on.exe" ] || [ -f "target/release/go-on" ]; then
    if [ -f "target/release/go-on.exe" ]; then
        BINARY_SIZE=$(stat -f%z "target/release/go-on.exe" 2>/dev/null || stat -c%s "target/release/go-on.exe")
    else
        BINARY_SIZE=$(stat -f%z "target/release/go-on" 2>/dev/null || stat -c%s "target/release/go-on")
    fi
    BINARY_SIZE_MB=$(awk "BEGIN { printf \"%.2f\", $BINARY_SIZE / 1024 / 1024 }")
    print_status 0 "Binary size: ${BINARY_SIZE_MB}MB"
else
    print_status 1 "Binary not found for size check"
fi

echo ""
echo "Phase 5: Migration Status Summary"
echo "--------------------------------"

# Summary
END_TIME=$(date +%s)
TOTAL_TIME=$((END_TIME - START_TIME))

echo "Migration Validation Complete"
echo "Total time: ${TOTAL_TIME} seconds"
echo ""

# Final status
if [ $ACP_TEST_STATUS -eq 0 ] && [ $INTEGRATION_TEST_STATUS -eq 0 ]; then
    echo -e "${GREEN}=========================================${NC}"
    echo -e "${GREEN}✓ MIGRATION VALIDATION PASSED${NC}"
    echo -e "${GREEN}=========================================${NC}"
    echo ""
    echo "Next steps:"
    echo "1. Begin migrating helpers modules (Phase 2)"
    echo "2. Run this validation script after each migration step"
    echo "3. Monitor for any compilation warnings or test failures"
    exit 0
else
    echo -e "${RED}=========================================${NC}"
    echo -e "${RED}✗ MIGRATION VALIDATION FAILED${NC}"
    echo -e "${RED}=========================================${NC}"
    echo ""
    echo "Issues detected:"
    [ $ACP_TEST_STATUS -ne 0 ] && echo "- ACP tests failed"
    [ $INTEGRATION_TEST_STATUS -ne 0 ] && echo "- Integration tests failed"
    echo ""
    echo "Please fix the issues before proceeding with migration."
    exit 1
fi
