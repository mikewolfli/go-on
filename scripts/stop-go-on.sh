#!/bin/sh
# 停止 go-on 进程
DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$DIR"
if [ -f go-on.pid ]; then
  PID=$(cat go-on.pid)
  if kill -0 $PID 2>/dev/null; then
    kill $PID
    echo "go-on 已停止 (PID: $PID)"
    rm -f go-on.pid
    exit 0
  else
    echo "go-on 进程不存在 (PID: $PID)，清理 pid 文件"
    rm -f go-on.pid
    exit 1
  fi
else
  echo "go-on 未运行，无需停止"
  exit 1
fi
