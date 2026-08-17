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

# i18n drift guard: backend / GUI / vscode catalogs must stay key-consistent
# within each end (three-language sets), and vscode MessageKeys must cover the
# locale keys it mirrors.
echo "=== Validating i18n key sets ==="
python3 "$SCRIPT_DIR/validate-i18n.py"

# Generated SDK types drift guard (M2.3): sdk/typescript/src/types.ts must be
# byte-identical to what scripts/gen-sdk-types.py emits from the canonical
# ACP stream event contract (contracts/acp-stream-events.json).
echo "=== Verifying generated SDK types (M2.3) ==="
python3 "$SCRIPT_DIR/gen-sdk-types.py" --check

# Regression gate: the generated `requests/quality-benchmark.ndjson` scenario
# is not part of the repo, so run the lib test suite as the regression gate.
echo "=== Running lib test suite (regression) ==="
cargo test --lib 2>&1 | tail -5
echo "Test run completed"

echo "✅ BLUE15 P3-1 quality gate completed"
