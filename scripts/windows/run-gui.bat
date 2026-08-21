@echo off
REM Runs the meow-meow GUI. Creating the virtual network adapter needs
REM Administrator privileges, so this script re-launches itself elevated
REM automatically (Windows' UAC prompt will appear once) -- no manual
REM "Run as Administrator" step needed.
setlocal

net session >nul 2>&1
if %errorlevel% == 0 (
    cd /d "%~dp0"
    start "" "%~dp0meow-meow_gui.exe"
    exit /b 0
) else (
    echo Requesting Administrator privileges...
    powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
    exit /b 0
)
