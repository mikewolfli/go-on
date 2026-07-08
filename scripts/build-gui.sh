#!/bin/sh
cd "$(dirname "$0")/../gui" || exit 1
cargo build --release > /tmp/build-gui.log 2>&1
echo "Exit code: $?" >> /tmp/build-gui.log
