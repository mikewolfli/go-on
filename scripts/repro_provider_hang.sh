#!/bin/sh
# Manual repro for provider_matrix hang: spawn go-on, run provider.status +
# provider.test_connection for every registry provider, then shutdown, then
# inspect /proc wchan of any threads that remain stuck after 10s.
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

"$BIN" --config "$CFG" < "$FIFO" > "$DIR/out.log" 2> "$DIR/err.log" &
PID=$!
echo "spawned pid=$PID"

# keep the FIFO write end open for the whole session
exec 9> "$FIFO"
send() { printf '%s\n' "$1" >&9; }

sleep 2
send '{"jsonrpc":"2.0","id":1,"method":"initialize"}'
send '{"jsonrpc":"2.0","id":2,"method":"provider.status"}'
sleep 3

# extract provider names from the provider.status response
PROVIDERS=$(grep -o '"agent":"[^"]*"' "$DIR/out.log" | sed 's/"agent":"//;s/"//' | sort -u)
echo "providers: $PROVIDERS"

ID=100
if [ "${SKIP_TEST_CONNECTION:-0}" = "1" ]; then
  echo "skipping test_connection calls"
else
  for p in $PROVIDERS; do
    send "{\"jsonrpc\":\"2.0\",\"id\":$ID,\"method\":\"provider.test_connection\",\"params\":{\"provider\":\"$p\"}}"
    ID=$((ID+1))
  done
fi

sleep 3
send '{"jsonrpc":"2.0","id":9999,"method":"shutdown","params":{"user_id":"test-admin","roles":["admin"]}}'

# mimic the test harness: close stdin write end shortly after shutdown RPC
if [ "${KEEP_STDIN_OPEN:-0}" != "1" ]; then
  sleep 1
  exec 9>&-
  rm -f "$FIFO"
fi

sleep 12
if kill -0 "$PID" 2>/dev/null; then
  echo "=== PROCESS STILL ALIVE after 12s — inspecting threads ==="
  for t in /proc/$PID/task/*; do
    tid=${t##*/}
    state=$(awk '{print $3}' "$t/stat" 2>/dev/null)
    wchan=$(cat "$t/wchan" 2>/dev/null)
    name=$(awk '{print $2}' "$t/stat" 2>/dev/null | tr -d '()')
    printf 'tid=%s name=%-22s state=%s wchan=%s\n' "$tid" "$name" "$state" "$wchan"
    if [ -r "$t/syscall" ]; then
      printf '    syscall: %s\n' "$(cat "$t/syscall")"
    else
      printf '    syscall: unreadable\n'
    fi
    printf '    fd info:\n'
    for fd in /proc/$PID/fd/*; do
      printf '      %s -> %s\n' "${fd##*/}" "$(readlink "$fd" 2>/dev/null)"
    done
  done
  echo "=== stderr tail ==="
  tail -30 "$DIR/err.log"
  kill -9 "$PID"
else
  echo "=== process exited cleanly ==="
  tail -10 "$DIR/err.log"
fi
exec 9>&-
echo "log dir: $DIR"
