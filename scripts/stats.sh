#!/usr/bin/env bash
set -euo pipefail

# Generate the "Codebase Statistics" numbers for README.md.
#
# Usage:
#   scripts/stats.sh             # print a markdown table of current numbers
#   scripts/stats.sh --check     # exit non-zero if README.md is stale
#   scripts/stats.sh --no-tests  # skip the (slow) `cargo test --lib` run

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT_DIR"

CHECK=0
RUN_TESTS=1
for arg in "$@"; do
  case "$arg" in
    --check) CHECK=1 ;;
    --no-tests) RUN_TESTS=0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

# ── Raw measurements ────────────────────────────────────────────────────────
src_loc=$(find src -name '*.rs' -exec cat {} + | wc -l | tr -d ' ')
src_modules=$(find src -name '*.rs' | wc -l | tr -d ' ')
gui_loc=$(find gui/src -name '*.rs' -exec cat {} + | wc -l | tr -d ' ')
vscode_loc=$(find vscode-addon/src -name '*.ts' -exec cat {} + | wc -l | tr -d ' ')
# SDK counts source only: exclude node_modules and the generated dist/ output.
sdk_loc=$(find sdk \( -name '*.rs' -o -name '*.py' -o -name '*.ts' \) \
  | grep -v node_modules | grep -v '/dist/' \
  | xargs cat 2>/dev/null | wc -l | tr -d ' ')
providers=$(grep -c 'ProviderSpec {' src/core/providers.rs | tr -d ' ')
skills=$(ls skills | wc -l | tr -d ' ')
# Registered tool count: every `register_with_profile(...)` call registers one
# tool (feature-gated tools are included; the default profile enables fewer).
tools=$(grep -c 'register_with_profile(' src/orchestration/tool/mod.rs | tr -d ' ')

if [ "$RUN_TESTS" = 1 ]; then
  # Executed lib-test count: sum the "passed" figure across all test binaries.
  tests=$(cargo test --lib 2>/dev/null | grep -E '^test result: ok' \
    | awk '{s += $4} END {print s+0}')
  test_note="executed via \`cargo test --lib\`"
else
  tests="?"
  test_note="(test count skipped)"
fi

# ── Round a LOC count to the nearest thousand, e.g. 6859 → 7. ────────────────
round_k() {
  awk -v n="$1" 'BEGIN { printf "%.0f", n / 1000 }'
}

printf '| Metric | Value |\n|:-------|:------|\n'
printf '| Rust backend LOC | ~%sK (%s modules) |\n' "$(round_k "$src_loc")" "$src_modules"
printf '| GUI (EGUI) LOC | ~%sK |\n' "$(round_k "$gui_loc")"
printf '| VS Code addon (TypeScript) LOC | ~%sK |\n' "$(round_k "$vscode_loc")"
printf '| SDK (Rust + Python + TypeScript) LOC | ~%sK |\n' "$(round_k "$sdk_loc")"
printf '| Built-in tools | %s registered in ToolRegistry (+feature-gated) |\n' "$tools"
printf '| AI providers | %s |\n' "$providers"
printf '| Skills in marketplace | %s |\n' "$skills"
printf '| Unit tests | %s (%s) |\n' "$tests" "$test_note"

# ── --check gate ────────────────────────────────────────────────────────────
if [ "$CHECK" = 1 ]; then
  fail=0
  expect() {
    local needle="$1"
    if ! grep -qF "$needle" README.md; then
      echo "stale README: expected to find: $needle" >&2
      fail=1
    fi
  }
  expect "~$(round_k "$src_loc")K ($src_modules modules)"
  expect "AI providers | $providers |"
  expect "Skills in marketplace | $skills |"
  if [ "$RUN_TESTS" = 1 ]; then
    expect "Unit tests | $tests "
  fi
  exit "$fail"
fi
