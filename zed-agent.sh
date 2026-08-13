#!/bin/sh
# Zed agent-server launcher for go-on.
# Loads API keys from the GUI-generated .env (DEEPSEEK_API_KEY etc.) into the
# environment so the agent reads keys from env vars instead of the macOS
# keychain — no per-binary keychain authorization prompts, no 5s keyring
# timeouts, no "secret not found" flakes.
DIR="$(cd "$(dirname "$0")" && pwd)"
if [ -f "$DIR/backend/.env" ]; then
  set -a
  . "$DIR/backend/.env"
  set +a
fi
exec "$DIR/backend/go-on" "$@"
