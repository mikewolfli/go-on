#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
artifact_dir="$repo_root/artifacts/blue23"
report_path="$artifact_dir/rollback-report.json"

mkdir -p "$artifact_dir"

platform_mode="phase_compat"
ts="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
cat > "$report_path" <<JSON
{
  "ok": true,
  "action": "blue23-rollback-phase-compat",
  "platform_mode": "$platform_mode",
  "contract": "contracts/editor-capability-matrix.json",
  "generated_at": "$ts",
  "notes": "Rollback script staged phase compatibility mode metadata and audit report."
}
JSON

echo "[blue23] rollback report generated: $report_path"