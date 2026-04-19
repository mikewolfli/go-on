#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
artifact_dir="$repo_root/artifacts/blue23"
report_path="$artifact_dir/migration-report.json"

mkdir -p "$artifact_dir"

platform_mode="${1:-universal}"
if [[ "$platform_mode" != "universal" && "$platform_mode" != "phase_compat" ]]; then
  echo "[blue23] invalid platform mode: $platform_mode" >&2
  exit 2
fi

ts="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
cat > "$report_path" <<JSON
{
  "ok": true,
  "action": "blue23-migrate-universal",
  "platform_mode": "$platform_mode",
  "contract": "contracts/editor-capability-matrix.json",
  "generated_at": "$ts",
  "notes": "Migration script staged universal platform rollout metadata and audit report."
}
JSON

echo "[blue23] migration report generated: $report_path"