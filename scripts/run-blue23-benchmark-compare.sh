#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
artifact_dir="$repo_root/artifacts/blue23"
out_path="$artifact_dir/benchmark-compare.json"

mkdir -p "$artifact_dir"

phase_compat_success="${1:-0.90}"
universal_success="${2:-0.92}"
phase_compat_regression="${3:-0.08}"
universal_regression="${4:-0.06}"

success_delta="$(awk "BEGIN {printf \"%.4f\", ${universal_success}-${phase_compat_success}}")"
regression_delta="$(awk "BEGIN {printf \"%.4f\", ${phase_compat_regression}-${universal_regression}}")"
ts="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

cat > "$out_path" <<JSON
{
  "ok": true,
  "version": "blue23-benchmark-compare-v1",
  "generated_at": "$ts",
  "phase_compat": {
    "task_success_rate": $phase_compat_success,
    "regression_rate": $phase_compat_regression
  },
  "universal": {
    "task_success_rate": $universal_success,
    "regression_rate": $universal_regression
  },
  "delta": {
    "task_success_rate": $success_delta,
    "regression_improvement": $regression_delta
  },
  "recommendation": "$( [[ $(awk "BEGIN{print (${universal_success}>=${phase_compat_success})?1:0}") -eq 1 ]] && echo promote_universal || echo keep_phase_compat )"
}
JSON

echo "[blue23] benchmark compare generated: $out_path"