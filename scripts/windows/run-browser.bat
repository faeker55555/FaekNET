@echo off
REM Runs the lan_mesh in-app browser standalone. Unlike run-gui.bat/
REM run-cli.bat, this does NOT need Administrator -- the browser never
REM touches the virtual network adapter, it just connects to whatever
REM mesh IPs/names are already reachable. No UAC prompt.
REM
REM Usage: run-browser.bat [address]
setlocal
cd /d "%~dp0"
start "" "%~dp0lan_mesh_browser.exe" %*
