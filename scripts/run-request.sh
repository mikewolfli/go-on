#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "usage: $0 <config-path> <template-ndjson> [binary]" >&2
  exit 1
fi

CONFIG="$1"
TEMPLATE="$2"
BINARY="${3:-./target/debug/go-on}"

[[ -f "$BINARY" ]] || { echo "binary not found: $BINARY" >&2; exit 1; }
[[ -f "$CONFIG" ]] || { echo "config not found: $CONFIG" >&2; exit 1; }
[[ -f "$TEMPLATE" ]] || { echo "template not found: $TEMPLATE" >&2; exit 1; }

cat "$TEMPLATE" | "$BINARY" --config "$CONFIG"

