#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <config-path> [binary]" >&2
  exit 1
fi

CONFIG="$1"
BINARY="${2:-./target/debug/go-on}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=== BLUE15 P3-1 quality gate: prompt validation + regression checks ==="
echo "=== Validating prompt templates ==="
"$SCRIPT_DIR/validate-prompts.sh"

# The request benchmark needs the generated `requests/quality-benchmark.ndjson`
# scenario file plus the Windows run-request harness; when the data file is
# absent we run the lib test suite as the regression gate instead.
if [[ -f "$ROOT_DIR/requests/quality-benchmark.ndjson" ]]; then
  echo "=== Running request benchmark scenario ==="
  cargo test --lib run_scenario 2>&1 | tail -5
else
  echo "[note] requests/quality-benchmark.ndjson not found — running lib test suite as regression gate"
  echo "=== Running lib test suite (regression) ==="
  cargo test --lib 2>&1 | tail -5
fi
echo "Test run completed"

echo "✅ BLUE15 P3-1 quality gate completed"
