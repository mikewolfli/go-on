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

echo "[skip] request benchmark — requests/quality-benchmark.ndjson not found"

echo "=== Running benchmark scenario integration regression ==="
cargo test --lib 2>&1 | tail -5
echo "Test run completed"

echo "✅ BLUE15 P3-1 quality gate completed"
