@echo off
REM go-on 进程管理脚本 for Windows
cd /d %~dp0
setlocal enabledelayedexpansion
set ACTION=%1
if "%ACTION%"=="" set ACTION=start
set PID_FILE=go-on.pid
set LOG_FILE=go-on.log
set EXE=go-on.exe

:status
if exist %PID_FILE% (
	set /p PID=<%PID_FILE%
	tasklist /FI "PID eq !PID!" | findstr /i !EXE! >nul
	if !errorlevel! == 0 (
		echo go-on 正在运行 (PID: !PID!)
		exit /b 0
	) else (
		echo go-on 进程不存在 (PID: !PID!)，清理 pid 文件
		del /f /q %PID_FILE%
		exit /b 1
	)
) else (
	echo go-on 未运行
	exit /b 1
)

:stop
if "%ACTION%"=="stop" (
	if exist %PID_FILE% (
		set /p PID=<%PID_FILE%
		tasklist /FI "PID eq !PID!" | findstr /i !EXE! >nul
		if !errorlevel! == 0 (
			taskkill /PID !PID! /F
			echo go-on 已停止 (PID: !PID!)
			del /f /q %PID_FILE%
			exit /b 0
		) else (
			echo go-on 进程不存在 (PID: !PID!)，清理 pid 文件
			del /f /q %PID_FILE%
			exit /b 1
		)
	) else (
		echo go-on 未运行，无需停止
		exit /b 1
	)
)

:start
if "%ACTION%"=="start" (
	call :status >nul 2>&1
	if !errorlevel! == 0 (
		echo go-on 已在运行，无需重复启动
		exit /b 0
	)
	if not exist %EXE% (
		echo 错误: %EXE% 不存在，请先编译 go-on 可执行文件。
		exit /b 1
	)
	REM 输出当前协议模式
	for /f "tokens=2 delims==" %%M in ('findstr /b /c:"mode" config.toml') do set PROTO_MODE=%%M
	if not "%PROTO_MODE%"=="" echo [info] 当前协议模式: %PROTO_MODE%
	start /b %EXE% > %LOG_FILE% 2>&1
	REM 等待进程启动
	timeout /t 1 >nul
	for /f "tokens=2 delims==" %%I in ('wmic process where "CommandLine like '%%%EXE%%%" get ProcessId /value') do set PID=%%I
	echo !PID! > %PID_FILE%
	echo go-on 已启动，日志写入 %LOG_FILE%，PID: !PID!
	exit /b 0
)

:restart
if "%ACTION%"=="restart" (
	call %0 stop
	timeout /t 1 >nul
	call %0 start
	exit /b 0
)

:status_cmd
if "%ACTION%"=="status" (
	call :status
	exit /b 0
)

echo 用法: start-go-on.bat [start|stop|restart|status]
exit /b 1