#!/usr/bin/env bash
# Migration Validation Script
# Validates ACP module compilation and tests
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

print_status() {
    if [ "$1" -eq 0 ]; then
        echo -e "${GREEN}✓${NC} $2"
    else
        echo -e "${RED}✗${NC} $2"
        return 1
    fi
}

START_TIME=$(date +%s)

echo "Phase 1: Basic Compilation Checks"
echo "---------------------------------"
cargo check 2>&1 && print_status 0 "Basic compilation" || print_status 1 "Basic compilation"
cargo test --lib --no-run 2>&1 | tail -5 && print_status 0 "Library tests compilation" || print_status 1 "Library tests compilation"

echo ""
echo "Phase 2: Module Existence Checks"
echo "---------------------------------"
for file in "src/acp/mod.rs" "src/acp/prelude/mod.rs" "src/acp/server.rs" "src/acp/background.rs" "src/acp/tests.rs"; do
    if [ -f "$file" ]; then
        print_status 0 "Module file exists: $file"
    else
        print_status 1 "Module file missing: $file"
    fi
done

echo ""
echo "Phase 3: Test Execution"
echo "-----------------------"
echo -e "${YELLOW}Running ACP tests...${NC}"
if cargo test acp -- --test-threads=1 2>&1 | tail -10; then
    print_status 0 "ACP tests passed"
else
    print_status 1 "ACP tests failed"
fi

echo ""
END_TIME=$(date +%s)
TOTAL_TIME=$((END_TIME - START_TIME))
echo "Migration Validation Complete: ${TOTAL_TIME}s"
echo -e "${GREEN}=========================================${NC}"
echo -e "${GREEN}✓ VALIDATION COMPLETE${NC}"
echo -e "${GREEN}=========================================${NC}"
