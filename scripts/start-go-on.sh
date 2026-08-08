#!/bin/bash
# Start go-on service on port 8090
set -eu

DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$DIR"

# go-on process manager
ACTION=${1:-start}
GOON_BIN="../target/debug/go-on"
CONFIG_ARG="--config ../config/config.toml"
PID_FILE="go-on.pid"
LOG_FILE="go-on.log"

status() {
    if [ -f "$PID_FILE" ]; then
        PID=$(cat "$PID_FILE")
        if kill -0 $PID 2>/dev/null; then
            echo "go-on is running (PID: $PID)"
            return 0
        else
            echo "go-on process not found (PID: $PID), cleaning up pid file"
            rm -f "$PID_FILE"
            return 1
        fi
    else
        echo "go-on is not running"
        return 1
    fi
}

stop() {
    if [ -f "$PID_FILE" ]; then
        PID=$(cat "$PID_FILE")
        if kill -0 $PID 2>/dev/null; then
            kill $PID
            echo "go-on stopped (PID: $PID)"
            rm -f "$PID_FILE"
            return 0
        else
            echo "go-on process not found (PID: $PID), cleaning up pid file"
            rm -f "$PID_FILE"
            return 1
        fi
    else
        echo "go-on is not running, nothing to stop"
        return 1
    fi
}

start() {
    status >/dev/null 2>&1 && {
        echo "go-on is already running"; return 0;
    }
    if [ ! -x "$GOON_BIN" ]; then
        echo "error: $GOON_BIN not found or not executable. Build go-on first."
        exit 1
    fi

    # Start the backend
    # API keys are managed via system keyring (configured as keyring://go-on/{provider}_api_key)

    # Print current protocol mode from config file ([protocol].mode — the
    # canonical key; the legacy [runtime].protocol_mode key was removed).
    CONFIG_FILE="../config/config.toml"
    if grep -q "^mode" "$DIR/$CONFIG_FILE" 2>/dev/null; then
        PROTO_MODE=$(grep "^mode" "$DIR/$CONFIG_FILE" | head -n1 | cut -d'=' -f2 | tr -d ' "')
        echo "[info] current protocol mode: $PROTO_MODE"
    fi

    nohup "$GOON_BIN" $CONFIG_ARG > "$LOG_FILE" 2>&1 &
    PID=$!
    if [ -z "$PID" ] || ! kill -0 "$PID" 2>/dev/null; then
        echo "error: go-on failed to start, check log: $LOG_FILE"
        exit 1
    fi
    echo "$PID" > "$PID_FILE"
    echo "go-on started, log: $LOG_FILE, PID: $(cat "$PID_FILE")"
}

restart() {
    stop
    sleep 1
    start
}

case "$ACTION" in
    start)
        start
        ;;
    stop)
        stop
        ;;
    restart)
        restart
        ;;
    status)
        status
        ;;
    *)
        echo "usage: $0 {start|stop|restart|status}"
        exit 1
        ;;
esac
