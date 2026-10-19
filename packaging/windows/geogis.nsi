; setup.exe installer (NSIS 3), built by package.sh. A per-user install that needs no admin
; rights, in %LOCALAPPDATA%\Programs\GeoGIS: a Start menu shortcut, rustgis-cli on the user's
; PATH, .rgis/.rgisx projects opened by GeoGIS and GeoGIS offered under Open with for .geojson.
; Its registry keys and file type (GeoGIS.Project) are its own, so it never touches an installed
; RustGIS. PUBLISHER may be passed with -D; it defaults to GeoFlux.
; Defines passed with -D: VERSION, VERSION4 (four numbers), STAGE (the folder holding the exes,
; LICENSE, NOTICE and path.ps1), ICON, SIDEBAR (a 164x314 BMP) and OUTFILE.

Unicode true
SetCompressor /SOLID lzma
RequestExecutionLevel user
ManifestDPIAware true

!define APP "GeoGIS"
!ifndef PUBLISHER
  !define PUBLISHER "GeoFlux"
!endif
!define PROGID "GeoGIS.Project"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}"

Name "${APP}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\${APP}"
InstallDirRegKey HKCU "Software\${APP}" "InstallDir"

VIProductVersion "${VERSION4}"
VIAddVersionKey "ProductName" "${APP}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "${APP} ${VERSION} setup"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "LegalCopyright" "Copyright ${PUBLISHER} and the RustGIS contributors. Apache-2.0."

!include "MUI2.nsh"
!include "WinMessages.nsh"

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_WELCOMEFINISHPAGE_BITMAP "${SIDEBAR}"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "${SIDEBAR}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\geogis.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start ${APP}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${STAGE}\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

; Asks Explorer to pick up changed file associations.
!macro RefreshShell
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend

; Tells running programs (a new terminal) that the user's PATH changed.
!macro RefreshEnvironment
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
!macroend

Section "Install"
  SetOutPath "$INSTDIR"
  File "${STAGE}\geogis.exe"
  File "${STAGE}\rustgis-cli.exe"
  File /oname=LICENSE.txt "${STAGE}\LICENSE"
  File /oname=NOTICE.txt "${STAGE}\NOTICE"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortcut "$SMPROGRAMS\${APP}.lnk" "$INSTDIR\geogis.exe"

  ; rustgis-cli from any new terminal.
  InitPluginsDir
  File /oname=$PLUGINSDIR\path.ps1 "${STAGE}\path.ps1"
  nsExec::ExecToLog 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\path.ps1" add "$INSTDIR"'
  Pop $0
  StrCmp $0 0 +2
    MessageBox MB_OK|MB_ICONEXCLAMATION "${APP} is installed, but rustgis-cli could not be added to your PATH ($0). Run it from $INSTDIR instead." /SD IDOK
  !insertmacro RefreshEnvironment

  ; Projects open in GeoGIS; GeoJSON lists it under Open with.
  WriteRegStr HKCU "Software\Classes\${PROGID}" "" "GeoGIS project"
  WriteRegStr HKCU "Software\Classes\${PROGID}\DefaultIcon" "" '"$INSTDIR\geogis.exe",0'
  WriteRegStr HKCU "Software\Classes\${PROGID}\shell\open\command" "" '"$INSTDIR\geogis.exe" "%1"'
  WriteRegStr HKCU "Software\Classes\.rgis" "" "${PROGID}"
  WriteRegStr HKCU "Software\Classes\.rgisx" "" "${PROGID}"
  WriteRegStr HKCU "Software\Classes\.geojson\OpenWithProgids" "${PROGID}" ""
  !insertmacro RefreshShell

  ; Apps & features.
  WriteRegStr HKCU "Software\${APP}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" '"$INSTDIR\geogis.exe",0'
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout" "https://github.com/Utkdby090/GeoGIS"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  SectionGetSize 0 $0
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" $0
SectionEnd

Section "Uninstall"
  Delete "$SMPROGRAMS\${APP}.lnk"

  InitPluginsDir
  File /oname=$PLUGINSDIR\path.ps1 "${STAGE}\path.ps1"
  nsExec::ExecToLog 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\path.ps1" remove "$INSTDIR"'
  Pop $0
  StrCmp $0 0 +2
    MessageBox MB_OK|MB_ICONEXCLAMATION "$INSTDIR could not be removed from your PATH ($0). Remove it in Settings › System › About › Advanced system settings › Environment Variables." /SD IDOK
  !insertmacro RefreshEnvironment

  ; Only the associations this installer made: another program may own .geojson.
  DeleteRegValue HKCU "Software\Classes\.geojson\OpenWithProgids" "${PROGID}"
  ReadRegStr $0 HKCU "Software\Classes\.rgis" ""
  StrCmp $0 "${PROGID}" 0 +2
    DeleteRegKey HKCU "Software\Classes\.rgis"
  ReadRegStr $0 HKCU "Software\Classes\.rgisx" ""
  StrCmp $0 "${PROGID}" 0 +2
    DeleteRegKey HKCU "Software\Classes\.rgisx"
  DeleteRegKey HKCU "Software\Classes\${PROGID}"
  !insertmacro RefreshShell

  ; The DJI pack's files, if it was unzipped here. DJI's own libraries in dji-sdk were added by
  ; the user and are left in place, which keeps those folders.
  Delete "$INSTDIR\geoflux-dirp.exe"
  Delete "$INSTDIR\README-DJI-pack.txt"
  Delete "$INSTDIR\dji-sdk\README.txt"
  RMDir "$INSTDIR\dji-sdk"
  Delete "$INSTDIR\geogis.exe"
  Delete "$INSTDIR\rustgis-cli.exe"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\NOTICE.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegValue HKCU "Software\${APP}" "InstallDir"
  DeleteRegKey /ifempty HKCU "Software\${APP}"
SectionEnd
