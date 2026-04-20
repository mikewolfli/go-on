@echo off
setlocal EnableExtensions

cd /d "%~dp0"

set "ACTION=%~1"
if "%ACTION%"=="" set "ACTION=start"

set "PID_FILE=go-on.pid"
set "LOG_FILE=go-on.log"
set "EXE=go-on.exe"
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

tasklist /FI "PID eq %PID%" | findstr /I /C:"%EXE%" >nul
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

set "PROTO_MODE="
for /f "tokens=2 delims==" %%M in ('findstr /b /c:"mode" config.toml 2^>nul') do set "PROTO_MODE=%%M"
if defined PROTO_MODE echo [info] protocol mode:%PROTO_MODE%

start "" /b "%EXE%" > "%LOG_FILE%" 2>&1
timeout /t 1 >nul

set "PID="
for /f "tokens=2 delims=," %%P in ('tasklist /FI "IMAGENAME eq %EXE%" /FO CSV /NH ^| findstr /V /I "INFO:"') do (
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