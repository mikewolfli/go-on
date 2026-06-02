#!/bin/sh
# Stop go-on process with graceful shutdown
set -eu

DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$DIR"
if [ -f go-on.pid ]; then
  PID=$(cat go-on.pid)
  if kill -0 $PID 2>/dev/null; then
    echo "Sending SIGTERM to go-on (PID: $PID)..."
    kill $PID
    # Wait up to 10 seconds for graceful shutdown
    WAIT=0
    while kill -0 $PID 2>/dev/null && [ $WAIT -lt 10 ]; do
      sleep 1
      WAIT=$((WAIT + 1))
    done
    # Force kill if still running
    if kill -0 $PID 2>/dev/null; then
      echo "Process did not stop after 10s, sending SIGKILL..."
      kill -9 $PID 2>/dev/null
      sleep 1
    fi
    echo "go-on stopped (PID: $PID)"
    rm -f go-on.pid
    exit 0
  else
    echo "go-on process not found (PID: $PID), cleaning up pid file"
    rm -f go-on.pid
    exit 1
  fi
else
  echo "go-on is not running, no need to stop"
  exit 1
fi
