#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"

echo "=== Building English ==="
mdbook build "$ROOT/en"

echo "=== Building Simplified Chinese ==="
mdbook build "$ROOT/zh-CN"

echo "=== Building Traditional Chinese ==="
mdbook build "$ROOT/zh-TW"

echo ""
echo "=== Done ==="
du -sh "$ROOT/book"/*/
