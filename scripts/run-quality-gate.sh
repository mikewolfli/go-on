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

echo "=== BLUE15 P3-1 quality gate: request benchmark + regression checks ==="
echo "=== Validating prompt templates ==="
"$SCRIPT_DIR/validate-prompts.sh"

"$SCRIPT_DIR/run-request.sh" "$CONFIG" "$ROOT_DIR/requests/quality-benchmark.ndjson" "$BINARY"

echo "=== Running benchmark scenario integration regression ==="
cargo test run_scenario_file_executes_quality_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture

if cargo --list | grep -q "^    tarpaulin$"; then
  echo "=== Optional coverage gate (tarpaulin) ==="
  cargo tarpaulin --out Stdout --fail-under 70
else
  echo "cargo-tarpaulin not installed, skipping optional coverage gate"
fi

echo "✅ BLUE15 P3-1 quality gate completed"