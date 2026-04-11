@echo off
REM 启动 go-on，监听 8090 端口，日志输出到 go-on.log
cd /d %~dp0
start /b go-on.exe --port 8090 > go-on.log 2>&1
REM 获取 go-on.exe 的 PID
for /f "tokens=2 delims==;" %%I in ('wmic process where "CommandLine like '%%go-on.exe%%' and CommandLine like '%%--port 8090%%'" get ProcessId /value') do set PID=%%I
echo %PID% > go-on.pid
echo go-on 已启动，监听端口 8090，日志写入 go-on.log，PID: %PID%