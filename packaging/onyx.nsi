; Onyx setup (.exe). Built by build.ps1, which passes VERSION, PUBLISHER, STAGE (the
; folder holding the files to install), ICON and OUTFILE.
;
; Installs for the current user only (no admin prompt) into
; %LOCALAPPDATA%\Programs\Onyx. Uninstalling keeps settings and downloaded
; models in %APPDATA%\Islet and %LOCALAPPDATA%\Islet.
; Silent install: Onyx-Setup.exe /S

Unicode true
SetCompressor /SOLID lzma
!include "MUI2.nsh"

!define APP "Onyx"
!define UNINST "Software\Microsoft\Windows\CurrentVersion\Uninstall\Onyx"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"

Name "${APP}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\Onyx"
InstallDirRegKey HKCU "${UNINST}" "InstallLocation"
RequestExecutionLevel user
BrandingText "Onyx ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "Onyx"
VIAddVersionKey "FileDescription" "Onyx Setup"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "LegalCopyright" "(c) 2026 ${PUBLISHER}"

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "Install Onyx"
!define MUI_WELCOMEPAGE_TEXT "Onyx puts a small black island at the top of your screen: your music, voice typing and system stats, out of the way until you need them.$\r$\n$\r$\nClick Next to continue."
!define MUI_FINISHPAGE_RUN "$INSTDIR\islet.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start Onyx"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; A running copy holds its files open; close it first.
!macro StopOnyx
  nsExec::Exec 'taskkill /F /IM islet.exe'
  nsExec::Exec 'taskkill /F /IM islet-settings.exe'
  nsExec::Exec 'taskkill /F /IM islet-asr-worker.exe'
  Sleep 600
!macroend

Section "Onyx"
  !insertmacro StopOnyx
  SetOutPath "$INSTDIR"
  File "${STAGE}\islet.exe"
  File "${STAGE}\islet-settings.exe"
  File "${STAGE}\islet-asr-worker.exe"
  File "${STAGE}\vcruntime140.dll"
  File "${STAGE}\vcruntime140_1.dll"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  CreateShortcut "$SMPROGRAMS\Onyx.lnk" "$INSTDIR\islet.exe" "" "$INSTDIR\islet.exe" 0

  WriteRegStr HKCU "${UNINST}" "DisplayName" "Onyx"
  WriteRegStr HKCU "${UNINST}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINST}" "DisplayIcon" "$INSTDIR\islet.exe"
  WriteRegStr HKCU "${UNINST}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINST}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  !insertmacro StopOnyx
  Delete "$INSTDIR\islet.exe"
  Delete "$INSTDIR\islet-settings.exe"
  Delete "$INSTDIR\islet-asr-worker.exe"
  Delete "$INSTDIR\vcruntime140.dll"
  Delete "$INSTDIR\vcruntime140_1.dll"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Onyx.lnk"
  DeleteRegValue HKCU "${RUN_KEY}" "Onyx"
  DeleteRegValue HKCU "${RUN_KEY}" "Islet"
  DeleteRegKey HKCU "${UNINST}"
SectionEnd
