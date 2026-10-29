; meow-meow Windows installer (NSIS 3.x).
;
; Build (from the repo root, STAGE must be an absolute path to the packaged folder):
;   makensis -DSTAGE=/abs/path/lan_mesh-vX-windows-x86_64 -DVERSION=0.61.0 \
;            -DOUTFILE=/abs/path/meow-meow-vX-setup.exe scripts/windows/installer.nsi
;
; VERSION must be numeric x.y.z (NSIS version resources need it).
;
; Layout: installs to %ProgramFiles%\meow-meow. The existing run-*.bat launchers
; cd into their own folder and keep mesh.toml there, so the Users group is granted
; modify rights on the install folder. That lets `init`/`add-peer` work without
; elevation while the GUI and `run` keep their UAC behaviour.

!ifndef STAGE
  !error "Define STAGE=<absolute path to the packaged folder>"
!endif
!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef OUTFILE
  !define OUTFILE "meow-meow-setup.exe"
!endif

!define APPNAME    "meow-meow"
!define PUBLISHER  "faeker55555"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\meow-meow"
!define APP_KEY    "Software\meow-meow"

Unicode true
Name "${APPNAME} ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$PROGRAMFILES64\${APPNAME}"
InstallDirRegKey HKLM "${APP_KEY}" "InstallDir"
RequestExecutionLevel admin
SetCompressor /SOLID lzma
ShowInstDetails show
ShowUninstDetails show
BrandingText "${APPNAME} ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${APPNAME}"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "FileDescription" "${APPNAME} installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "${PUBLISHER}"

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"

; ---------- Welcome / first-run pages ----------
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "Welcome to meow-meow ${VERSION}"
!define MUI_WELCOMEPAGE_TEXT "meow-meow is a peer-to-peer virtual LAN for games and LAN-style apps. There is no central VPN server.$\r$\n$\r$\nThe package includes the Wintun network driver. Installing needs Administrator rights; the app asks for them when it starts the network adapter.$\r$\n$\r$\nClick Next to continue."

!define MUI_FINISHPAGE_TITLE "meow-meow is installed"
!define MUI_FINISHPAGE_TEXT "Next steps:$\r$\n  1. Run first-time setup to create your mesh.toml (identity keys and settings).$\r$\n  2. Share your peer card with friends with: meow-meow export$\r$\n  3. Start meow-meow. It asks for Administrator rights to create the adapter."

!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "Start meow-meow now"
!define MUI_FINISHPAGE_RUN_FUNCTION LaunchGui

!define MUI_FINISHPAGE_SHOWREADME
!define MUI_FINISHPAGE_SHOWREADME_TEXT "Run first-time setup (meow-meow init)"
!define MUI_FINISHPAGE_SHOWREADME_FUNCTION RunFirstTimeSetup
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

; ---------- Helpers ----------
; Firewall rule names match scripts/windows/allow-firewall.bat so either path
; (installer or the .bat helper) creates/removes the same rules.
!macro FwAdd NAME PROG
  nsExec::Exec `netsh advfirewall firewall delete rule name="${NAME}"`
  nsExec::Exec `netsh advfirewall firewall add rule name="${NAME}" dir=in action=allow program="${PROG}" enable=yes profile=any`
  nsExec::Exec `netsh advfirewall firewall add rule name="${NAME} (outbound)" dir=out action=allow program="${PROG}" enable=yes profile=any`
!macroend

!macro FwDel NAME
  nsExec::Exec `netsh advfirewall firewall delete rule name="${NAME}"`
  nsExec::Exec `netsh advfirewall firewall delete rule name="${NAME} (outbound)"`
!macroend

!macro StopApps
  ; Silently stop running copies so their files can be replaced or removed.
  nsExec::Exec `taskkill /F /T /IM meow-meow_gui.exe`
  nsExec::Exec `taskkill /F /T /IM meow-meow_browser.exe`
  nsExec::Exec `taskkill /F /T /IM meow-meow.exe`
!macroend

Function .onInit
  SetRegView 64
FunctionEnd

Function LaunchGui
  SetOutPath "$INSTDIR"
  ExecShell "open" "$INSTDIR\run-gui.bat"
FunctionEnd

Function RunFirstTimeSetup
  SetOutPath "$INSTDIR"
  ExecShell "open" "$SYSDIR\cmd.exe" `/k ""$INSTDIR\run-cli.bat" init`
FunctionEnd

; ---------- Sections ----------
Section "meow-meow (required)" SecCore
  SectionIn RO

  !insertmacro StopApps

  SetOutPath "$INSTDIR"
  File /r "${STAGE}\*.*"

  ; Let standard users write mesh.toml and run init/add-peer without elevation.
  ; S-1-5-32-545 is the built-in Users group (locale independent).
  nsExec::Exec `icacls "$INSTDIR" /grant *S-1-5-32-545:(OI)(CI)M /T /C`

  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr HKLM "${APP_KEY}" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayName" "${APPNAME}"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINST_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKLM "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\meow-meow_gui.exe"
  WriteRegStr HKLM "${UNINST_KEY}" "UninstallString" `"$INSTDIR\uninstall.exe"`
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "Start Menu shortcuts" SecStartMenu
  SetShellVarContext all
  CreateDirectory "$SMPROGRAMS\${APPNAME}"
  CreateShortCut "$SMPROGRAMS\${APPNAME}\meow-meow.lnk" "$INSTDIR\run-gui.bat" "" "$INSTDIR\meow-meow_gui.exe" 0 SW_SHOWNORMAL "" "Start the meow-meow GUI"
  CreateShortCut "$SMPROGRAMS\${APPNAME}\meow-meow CLI.lnk" "$SYSDIR\cmd.exe" `/k ""$INSTDIR\run-cli.bat""` "$INSTDIR\meow-meow.exe" 0 SW_SHOWNORMAL "" "meow-meow command line"
  CreateShortCut "$SMPROGRAMS\${APPNAME}\meow-meow browser.lnk" "$INSTDIR\run-browser.bat" "" "$INSTDIR\meow-meow_browser.exe" 0 SW_SHOWNORMAL "" "Open the meow-meow in-app browser"
  CreateShortCut "$SMPROGRAMS\${APPNAME}\Uninstall meow-meow.lnk" "$INSTDIR\uninstall.exe"
SectionEnd

Section "Desktop shortcut" SecDesktop
  SetShellVarContext all
  CreateShortCut "$DESKTOP\meow-meow.lnk" "$INSTDIR\run-gui.bat" "" "$INSTDIR\meow-meow_gui.exe" 0 SW_SHOWNORMAL "" "Start the meow-meow GUI"
SectionEnd

Section "Windows Firewall rules" SecFirewall
  ; Same rules as scripts/windows/allow-firewall.bat. TCP-based apps need them.
  !insertmacro FwAdd "meow-meow (CLI)" "$INSTDIR\meow-meow.exe"
  !insertmacro FwAdd "meow-meow (GUI)" "$INSTDIR\meow-meow_gui.exe"
  !insertmacro FwAdd "meow-meow (browser)" "$INSTDIR\meow-meow_browser.exe"
SectionEnd

LangString DESC_SecCore ${LANG_ENGLISH} "meow-meow program files, wintun driver and launchers (required)."
LangString DESC_SecStartMenu ${LANG_ENGLISH} "Start Menu entries for the GUI, CLI, browser and uninstaller."
LangString DESC_SecDesktop ${LANG_ENGLISH} "Put a meow-meow shortcut on the desktop of all users."
LangString DESC_SecFirewall ${LANG_ENGLISH} "Allow meow-meow through Windows Firewall so TCP-based apps can connect. Recommended."

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecCore} $(DESC_SecCore)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecStartMenu} $(DESC_SecStartMenu)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} $(DESC_SecDesktop)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecFirewall} $(DESC_SecFirewall)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; ---------- Uninstaller ----------
Function un.onInit
  SetRegView 64
FunctionEnd

Section "Uninstall"
  !insertmacro StopApps

  !insertmacro FwDel "meow-meow (CLI)"
  !insertmacro FwDel "meow-meow (GUI)"
  !insertmacro FwDel "meow-meow (browser)"

  SetShellVarContext all
  Delete "$DESKTOP\meow-meow.lnk"
  RMDir /r "$SMPROGRAMS\${APPNAME}"

  Delete "$INSTDIR\meow-meow.exe"
  Delete "$INSTDIR\meow-meow_gui.exe"
  Delete "$INSTDIR\meow-meow_browser.exe"
  Delete "$INSTDIR\wintun.dll"
  Delete "$INSTDIR\WINTUN_LICENSE.txt"
  Delete "$INSTDIR\run-gui.bat"
  Delete "$INSTDIR\run-cli.bat"
  Delete "$INSTDIR\run-browser.bat"
  Delete "$INSTDIR\allow-firewall.bat"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\uninstall.exe"
  RMDir /r "$INSTDIR\extras"

  ; mesh.toml holds this machine's identity keys and peer list. Keep it unless
  ; the user explicitly asks for removal, so a reinstall keeps the same identity.
  ; directory.toml holds the GitHub token / MantleDB write key, so it goes with mesh.toml.
  ${If} ${FileExists} "$INSTDIR\mesh.toml"
    MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "Also delete your meow-meow settings (mesh.toml: identity keys and peer list, and directory.toml: directory service credentials)?$\r$\n$\r$\nChoose No to keep them for a future reinstall." IDNO +3
    Delete "$INSTDIR\mesh.toml"
    Delete "$INSTDIR\directory.toml"
  ${EndIf}

  ; Removes the folder only if it is now empty (keeps mesh.toml when kept).
  RMDir "$INSTDIR"

  DeleteRegKey HKLM "${UNINST_KEY}"
  DeleteRegKey HKLM "${APP_KEY}"
SectionEnd
