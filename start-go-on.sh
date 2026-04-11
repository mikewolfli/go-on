#!/bin/sh
# 启动 go-on，监听 8090 端口，日志输出到 go-on.log
DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$DIR"

# 可执行文件名假定为 go-on
./go-on --port 8090 > go-on.log 2>&1 &
echo $! > go-on.pid
echo "go-on 已启动，监听端口 8090，日志写入 go-on.log，PID: $(cat go-on.pid)"