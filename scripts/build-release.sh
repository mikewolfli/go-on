#!/bin/sh
cd "$(dirname "$0")/.." || exit 1
cargo build --release > /tmp/build-release.log 2>&1
echo "Exit code: $?" >> /tmp/build-release.log
