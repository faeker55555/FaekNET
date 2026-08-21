@echo off
REM One-time helper: adds a Windows Defender Firewall rule allowing all
REM traffic to/from meow-meow.exe and meow-meow_gui.exe. Needed if TCP-based
REM apps (e.g. Minecraft, a hosted website) don't connect even though the
REM mesh is running and UDP-based games work fine -- see the README's "If
REM TCP-based things don't connect" section for why.
REM
REM Needs Administrator privileges (re-launches itself elevated if needed).
setlocal

net session >nul 2>&1
if %errorlevel% neq 0 (
    echo Requesting Administrator privileges...
    powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
    exit /b 0
)

cd /d "%~dp0"

echo Adding firewall rules for meow-meow...
netsh advfirewall firewall add rule name="meow-meow (CLI)" dir=in action=allow program="%~dp0meow-meow.exe" enable=yes
netsh advfirewall firewall add rule name="meow-meow (CLI, outbound)" dir=out action=allow program="%~dp0meow-meow.exe" enable=yes
netsh advfirewall firewall add rule name="meow-meow (GUI)" dir=in action=allow program="%~dp0meow-meow_gui.exe" enable=yes
netsh advfirewall firewall add rule name="meow-meow (GUI, outbound)" dir=out action=allow program="%~dp0meow-meow_gui.exe" enable=yes

echo Done. Rules added for both meow-meow.exe and meow-meow_gui.exe.
pause
