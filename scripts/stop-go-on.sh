#!/bin/sh
# Stop go-on process
DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$DIR"
if [ -f go-on.pid ]; then
  PID=$(cat go-on.pid)
  if kill -0 $PID 2>/dev/null; then
    kill $PID
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
