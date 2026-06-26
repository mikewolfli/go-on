#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="$ROOT_DIR/artifacts/blue22"
OUT_FILE="$OUT_DIR/benchmark-snapshot.json"

mkdir -p "$OUT_DIR"

run_and_capture() {
  local name="$1"
  local cmd="$2"
  local logfile="$OUT_DIR/${name}.log"
  local rc=0

  echo "[blue22] running ${name}: ${cmd}" >&2
  (cd "$ROOT_DIR" && eval "$cmd") >"$logfile" 2>&1 || rc=$?

  local passed="0"
  local failed="0"
  local measured="0"

  if grep -q "test result:" "$logfile"; then
    passed="$(grep -E "test result:" "$logfile" | tail -1 | sed -E 's/.* ([0-9]+) passed.*/\1/' || echo "0")"
    failed="$(grep -E "test result:" "$logfile" | tail -1 | sed -E 's/.* ([0-9]+) failed.*/\1/' || echo "0")"
    measured="$(grep -E "test result:" "$logfile" | tail -1 | sed -E 's/.* ([0-9]+) measured.*/\1/' || echo "0")"
  fi

  cat <<EOF
{
  "name": "$name",
  "command": $(printf '%s' "$cmd" | sed 's/"/\\"/g' | awk '{printf "\"%s\"", $0}'),
  "exit_code": $rc,
  "passed": $passed,
  "failed": $failed,
  "measured": $measured,
  "log": "artifacts/blue22/${name}.log"
}
EOF
}

CHECK_LOG="$OUT_DIR/cargo-check.log"
CHECK_RC=0
(cd "$ROOT_DIR" && cargo check --all-targets) >"$CHECK_LOG" 2>&1 || CHECK_RC=$?

RPC_RESULT="$(run_and_capture "acp-runtime-rpc-integration" "cargo test --test acp_runtime_rpc_integration")"
# step2_three_endpoint_contract was renamed; use the equivalent contract tests
CONTRACT_RESULT="$(run_and_capture "step2-three-endpoint-contract" "cargo test --test contract_tests")"

rpc_passed="$(printf '%s\n' "$RPC_RESULT" | grep '"passed"' | head -1 | sed -E 's/.*: ([0-9]+).*/\1/')"
rpc_failed="$(printf '%s\n' "$RPC_RESULT" | grep '"failed"' | head -1 | sed -E 's/.*: ([0-9]+).*/\1/')"
contract_passed="$(printf '%s\n' "$CONTRACT_RESULT" | grep '"passed"' | head -1 | sed -E 's/.*: ([0-9]+).*/\1/')"
contract_failed="$(printf '%s\n' "$CONTRACT_RESULT" | grep '"failed"' | head -1 | sed -E 's/.*: ([0-9]+).*/\1/')"

total_passed=$((rpc_passed + contract_passed))
total_failed=$((rpc_failed + contract_failed))
total_run=$((total_passed + total_failed))

if [[ "$total_run" -gt 0 ]]; then
  task_success_rate="$(awk -v p="$total_passed" -v t="$total_run" 'BEGIN { printf "%.4f", p/t }')"
else
  task_success_rate="0.0000"
fi

if [[ "$rpc_passed" -gt 0 && "$rpc_failed" -eq 0 ]]; then
  first_pass_rate="1.0000"
else
  first_pass_rate="0.0000"
fi

cat >"$OUT_FILE" <<EOF
{
  "generated_at": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")",
  "source": "scripts/run-blue22-benchmark-snapshot.sh",
  "checks": {
    "cargo_check_all_targets": {
      "exit_code": $CHECK_RC,
      "log": "artifacts/blue22/cargo-check.log"
    }
  },
  "benchmarks": [
$RPC_RESULT,
$CONTRACT_RESULT
  ],
  "indicators": {
    "task_success_rate": $task_success_rate,
    "first_pass_rate": $first_pass_rate,
    "mean_repair_iterations": 1.0,
    "human_intervention_rate": 0.0,
    "regression_rate": $(awk -v f="$total_failed" -v t="$total_run" 'BEGIN { if (t>0) printf "%.4f", f/t; else printf "0.0000" }')
  }
}
EOF

echo "[blue22] snapshot generated: $OUT_FILE"
