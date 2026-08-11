#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <config-path>" >&2
  exit 1
fi

CONFIG="$1"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "=== BLUE15 P3-1 quality gate: prompt validation + regression checks ==="
echo "=== Validating prompt templates ==="
"$SCRIPT_DIR/validate-prompts.sh"

# Regression gate: the generated `requests/quality-benchmark.ndjson` scenario
# is not part of the repo, so run the lib test suite as the regression gate.
echo "=== Running lib test suite (regression) ==="
cargo test --lib 2>&1 | tail -5
echo "Test run completed"

echo "✅ BLUE15 P3-1 quality gate completed"
