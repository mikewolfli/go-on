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

echo "=== BLUE15 Stage C release readiness gate ==="
echo "=== 1) Release readiness scenario replay ==="
cat "$ROOT_DIR/requests/release-readiness-drill.ndjson" | \
  "$BINARY" --config "$CONFIG" --protocol-mode acp_stdio

echo "=== 2) Integration assertions ==="
cargo test run_scenario_file_executes_release_readiness_drill_requests -- --nocapture
cargo test rpc_shutdown_waits_for_inflight_chat_completion -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture

echo "=== 3) BLUE22 benchmark snapshot ==="
"$ROOT_DIR/scripts/run-blue22-benchmark-snapshot.sh"

echo "✅ BLUE15 Stage C release readiness gate completed"
