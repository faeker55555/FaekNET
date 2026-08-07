@echo off
REM One-time helper: adds a Windows Defender Firewall rule allowing all
REM traffic to/from lan_mesh.exe and lan_mesh_gui.exe. Needed if TCP-based
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

echo Adding firewall rules for lan_mesh...
netsh advfirewall firewall add rule name="lan_mesh (CLI)" dir=in action=allow program="%~dp0lan_mesh.exe" enable=yes
netsh advfirewall firewall add rule name="lan_mesh (CLI, outbound)" dir=out action=allow program="%~dp0lan_mesh.exe" enable=yes
netsh advfirewall firewall add rule name="lan_mesh (GUI)" dir=in action=allow program="%~dp0lan_mesh_gui.exe" enable=yes
netsh advfirewall firewall add rule name="lan_mesh (GUI, outbound)" dir=out action=allow program="%~dp0lan_mesh_gui.exe" enable=yes

echo Done. Rules added for both lan_mesh.exe and lan_mesh_gui.exe.
pause
