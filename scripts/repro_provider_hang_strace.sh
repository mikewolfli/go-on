#!/bin/sh
# strace-based repro: spawn go-on under strace -f, drive test_connection + shutdown,
# then report the last syscalls of each surviving thread.
set -u

BIN=/home/mikeli/workspace/go-on/target/debug/go-on
DIR=$(mktemp -d)
CFG="$DIR/config.toml"
FIFO="$DIR/stdin.fifo"
mkfifo "$FIFO"

cat > "$CFG" <<'EOF'
default_phase = "coding"

[flow]
name = "Test Flow"
phases = ["coding"]

[runtime]
maintenance_interval_seconds = 60
health_interval_seconds = 120
shutdown_drain_seconds = 5

[agents.copilot]
type = "copilot"
url = "http://127.0.0.1:8080"

[phases.coding]
description = "Coding"
agents = ["copilot"]
fallback = true
EOF

export GO_ON_ENABLE_LOCAL_TEST_AGENTS=1
export GO_ON_SKIP_MEMORY_CHECK=true

strace -f -o "$DIR/trace.log" "$BIN" --config "$CFG" < "$FIFO" > "$DIR/out.log" 2> "$DIR/err.log" &
PID=$!
echo "spawned pid=$PID (strace)"

exec 9> "$FIFO"
send() { printf '%s\n' "$1" >&9; }

sleep 2
send '{"jsonrpc":"2.0","id":1,"method":"initialize"}'
send '{"jsonrpc":"2.0","id":2,"method":"provider.status"}'
sleep 3

PROVIDERS=$(grep -o '"agent":"[^"]*"' "$DIR/out.log" | sed 's/"agent":"//;s/"//' | sort -u)
echo "providers: $PROVIDERS"

ID=100
for p in $PROVIDERS; do
  send "{\"jsonrpc\":\"2.0\",\"id\":$ID,\"method\":\"provider.test_connection\",\"params\":{\"provider\":\"$p\"}}"
  ID=$((ID+1))
done

sleep 3
send '{"jsonrpc":"2.0","id":9999,"method":"shutdown","params":{"user_id":"test-admin","roles":["admin"]}}'
sleep 1
exec 9>&-
rm -f "$FIFO"

sleep 8
if kill -0 "$PID" 2>/dev/null; then
  echo "=== STILL ALIVE — last syscalls per thread ==="
  # The go-on child is a direct child of strace; find it
  CHILD=$(pgrep -P "$PID" | head -1)
  echo "child pid=$CHILD"
  if [ -n "$CHILD" ]; then
    for t in /proc/$CHILD/task/*; do
      tid=${t##*/}
      name=$(awk '{print $2}' "$t/stat" 2>/dev/null | tr -d '()')
      wchan=$(cat "$t/wchan" 2>/dev/null)
      printf 'tid=%s name=%s wchan=%s\n' "$tid" "$name" "$wchan"
      echo "  --- last syscalls for tid $tid ---"
      grep " <$tid> " "$DIR/trace.log" | tail -12
    done
  fi
  kill -9 "$PID" 2>/dev/null
else
  echo "=== exited cleanly ==="
fi
echo "trace: $DIR"
