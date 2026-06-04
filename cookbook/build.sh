#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"

echo "=== Building Documentation (English / 中文 / 繁體中文) ==="
mdbook build "$ROOT"

echo ""
echo "=== Done ==="
du -sh "$ROOT/book"/*/
