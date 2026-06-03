#!/bin/bash
# Start go-on service on port 8090
set -eu

DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$DIR"

# go-on process manager
ACTION=${1:-start}
GOON_BIN="./go-on"
PID_FILE="go-on.pid"
LOG_FILE="go-on.log"

status() {
	if [ -f "$PID_FILE" ]; then
		PID=$(cat "$PID_FILE")
		if kill -0 $PID 2>/dev/null; then
			echo "go-on 正在运行 (PID: $PID)"
			return 0
		else
			echo "go-on 进程不存在 (PID: $PID)，清理 pid 文件"
			rm -f "$PID_FILE"
			return 1
		fi
	else
		echo "go-on 未运行"
		return 1
	fi
}

stop() {
	if [ -f "$PID_FILE" ]; then
		PID=$(cat "$PID_FILE")
		if kill -0 $PID 2>/dev/null; then
			kill $PID
			echo "go-on 已停止 (PID: $PID)"
			rm -f "$PID_FILE"
			return 0
		else
			echo "go-on 进程不存在 (PID: $PID)，清理 pid 文件"
			rm -f "$PID_FILE"
			return 1
		fi
	else
		echo "go-on 未运行，无需停止"
		return 1
	fi
}

start() {
	status >/dev/null 2>&1 && {
		echo "go-on 已在运行，无需重复启动"; return 0;
	}
	if [ ! -x "$GOON_BIN" ]; then
		echo "错误: $GOON_BIN 不存在或不可执行，请先编译 go-on 二进制文件。"
		exit 1
	fi

	# ── 启动后端 ────────────────────────────────────────────
	# API key 通过系统 keyring 管理（配置为 keyring://go-on/{provider}_api_key）
	# 用户在 GUI 中保存 API key 时会自动写入系统 keyring

	# 输出当前协议模式
	if grep -q "^mode" "$DIR/config.toml" 2>/dev/null; then
		PROTO_MODE=$(grep "^mode" "$DIR/config.toml" | head -n1 | cut -d'=' -f2 | tr -d ' "')
		echo "[info] 当前协议模式: $PROTO_MODE"
	fi

	nohup "$GOON_BIN" > "$LOG_FILE" 2>&1 &&
	PID=$!
	if [ -z "$PID" ] || ! kill -0 "$PID" 2>/dev/null; then
		echo "错误: go-on 启动失败，请检查日志: $LOG_FILE"
		exit 1
	fi
	echo "$PID" > "$PID_FILE"
	echo "go-on 已启动，日志写入 $LOG_FILE，PID: $(cat "$PID_FILE")"
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
		echo "用法: $0 {start|stop|restart|status}"
		exit 1
		;;
esac
