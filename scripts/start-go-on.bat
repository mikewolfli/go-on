@echo off
setlocal EnableExtensions

cd /d "%~dp0"

set "ACTION=%~1"
if "%ACTION%"=="" set "ACTION=start"

set "PID_FILE=go-on.pid"
set "LOG_FILE=go-on.log"
rem Binary/config resolve relative to the repo root (up one dir from scripts/),
rem matching scripts/start-go-on.sh (which uses ROOT/target/... + ROOT/config/...).
set "EXE=%~dp0..\target\release\go-on.exe"
set "CONFIG_FILE=%~dp0..\config\config.toml"
rem tasklist IMAGENAME matches the process image name only (not a path).
set "IMAGE_NAME=go-on.exe"
set "PID="

if /I "%ACTION%"=="start" goto do_start
if /I "%ACTION%"=="stop" goto do_stop
if /I "%ACTION%"=="restart" goto do_restart
if /I "%ACTION%"=="status" goto do_status

echo Usage: start-go-on.bat [start^|stop^|restart^|status]
exit /b 1

:is_running
set "PID="
if not exist "%PID_FILE%" exit /b 1

set /p PID=<"%PID_FILE%"
if "%PID%"=="" (
    del /f /q "%PID_FILE%" >nul 2>&1
    exit /b 1
)

tasklist /FI "PID eq %PID%" | findstr /I /C:"%IMAGE_NAME%" >nul
if errorlevel 1 (
    echo Stale PID %PID% found; cleaning pid file.
    del /f /q "%PID_FILE%" >nul 2>&1
    exit /b 1
)

exit /b 0

:do_status
call :is_running
if errorlevel 1 (
    echo go-on is not running.
    exit /b 1
)
echo go-on is running ^(PID: %PID%^).
exit /b 0

:do_stop
call :is_running
if errorlevel 1 (
    echo go-on is not running, nothing to stop.
    exit /b 1
)

taskkill /PID %PID% /F >nul 2>&1
if errorlevel 1 (
    echo Failed to stop go-on ^(PID: %PID%^).
    exit /b 1
)

del /f /q "%PID_FILE%" >nul 2>&1
echo go-on stopped ^(PID: %PID%^).
exit /b 0

:do_start
call :is_running
if not errorlevel 1 (
    echo go-on is already running ^(PID: %PID%^).
    exit /b 0
)

if not exist "%EXE%" (
    echo Error: %EXE% not found. Build release binary first.
    exit /b 1
)

rem Extract protocol mode from the [protocol] section only. A plain
rem "mode" prefix match would pick up the top-level model_selection_mode
rem (same issue fixed in start-go-on.sh).
set "PROTO_MODE="
set "IN_PROTO="
for /f "usebackq delims=" %%L in ("%CONFIG_FILE%") do (
    echo(%%L|findstr /b /c:"[" >nul && set "IN_PROTO="
    echo(%%L|findstr /b /c:"[protocol]" >nul && set "IN_PROTO=1"
    if defined IN_PROTO (
        echo(%%L|findstr /b /c:"mode" >nul && for /f "tokens=2 delims==" %%M in ("%%L") do set "PROTO_MODE=%%M"
    )
)
if defined PROTO_MODE echo [info] protocol mode:%PROTO_MODE%

start "" /b "%EXE%" --config "%CONFIG_FILE%" > "%LOG_FILE%" 2>&1
timeout /t 1 >nul

set "PID="
for /f "tokens=2 delims=," %%P in ('tasklist /FI "IMAGENAME eq %IMAGE_NAME%" /FO CSV /NH ^| findstr /V /I "INFO:"') do (
    set "PID=%%~P"
)

if "%PID%"=="" (
    echo Warning: started but PID capture failed. Check %LOG_FILE%.
    exit /b 1
)

echo %PID%>"%PID_FILE%"
echo go-on started, log: %LOG_FILE%, PID: %PID%
exit /b 0

:do_restart
call "%~f0" stop >nul 2>&1
timeout /t 1 >nul
call "%~f0" start
exit /b %errorlevel%