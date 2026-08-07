@echo off
REM Runs the lan_mesh CLI. Passes through all arguments, e.g.:
REM   run-cli.bat init
REM   run-cli.bat export
REM   run-cli.bat run
REM
REM "run" (and anything that brings the mesh up) needs Administrator
REM privileges to create the virtual network adapter, so it's
REM automatically re-launched elevated when needed. Other commands (init,
REM add-peer, export, import, list-peers, myaddr, ping, genkey) run
REM directly without a UAC prompt.
setlocal
cd /d "%~dp0"

if /I "%~1"=="run" (
    net session >nul 2>&1
    if %errorlevel% neq 0 (
        echo lan_mesh run needs Administrator privileges to create its virtual network adapter.
        echo Requesting elevation...
        powershell -NoProfile -Command "Start-Process -FilePath '%~dp0lan_mesh.exe' -ArgumentList 'run' -Verb RunAs -Wait"
        exit /b 0
    )
)

"%~dp0lan_mesh.exe" %*
