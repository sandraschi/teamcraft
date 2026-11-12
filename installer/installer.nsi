; TeamCraft per-user NSIS installer (no admin rights, no UAC prompt).
; Installs to %LOCALAPPDATA%\Programs\TeamCraft. Built by scripts/release.ps1:
;   makensis /DPRODUCT_VERSION=x.y.z /DOUT_FILE=dist\teamcraft-x.y.z-setup.exe installer\installer.nsi
; The database is NOT in the install folder (%LOCALAPPDATA%\teamcraft\teamcraft.db),
; so uninstalling never deletes the time records.

!ifndef PRODUCT_VERSION
  !error "pass /DPRODUCT_VERSION=x.y.z"
!endif
!ifndef OUT_FILE
  !define OUT_FILE "..\dist\teamcraft-${PRODUCT_VERSION}-setup.exe"
!endif

!define PRODUCT_NAME "TeamCraft"
!define PRODUCT_PUBLISHER "sandraschi"
!define PRODUCT_WEB_SITE "https://github.com/sandraschi/teamcraft"
!define PRODUCT_DIR_REGKEY "Software\Microsoft\Windows\CurrentVersion\App Paths\teamcraft-app.exe"
!define PRODUCT_UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}"

Unicode true
RequestExecutionLevel user
SetCompressor /SOLID lzma

Name "${PRODUCT_NAME} ${PRODUCT_VERSION}"
OutFile "${OUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\TeamCraft"
InstallDirRegKey HKCU "${PRODUCT_DIR_REGKEY}" ""
ShowInstDetails show
ShowUnInstDetails show

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "TeamCraft" SEC01
  SetOutPath "$INSTDIR"
  SetOverwrite on
  ; Missing binaries are an error: a half-empty installer is worse than none.
  File "..\target\release\teamcraft-app.exe"
  File "..\target\release\teamcraft.exe"
  File "..\README.md"
  File "..\HELP.md"
  File "..\CHANGELOG.md"

  CreateDirectory "$SMPROGRAMS\TeamCraft"
  CreateShortCut "$SMPROGRAMS\TeamCraft\TeamCraft.lnk" "$INSTDIR\teamcraft-app.exe"
  CreateShortCut "$SMPROGRAMS\TeamCraft\TeamCraft Help.lnk" "$INSTDIR\HELP.md"
  CreateShortCut "$SMPROGRAMS\TeamCraft\Uninstall TeamCraft.lnk" "$INSTDIR\uninst.exe"

  WriteUninstaller "$INSTDIR\uninst.exe"
  WriteRegStr HKCU "${PRODUCT_DIR_REGKEY}" "" "$INSTDIR\teamcraft-app.exe"
  WriteRegStr HKCU "${PRODUCT_UNINST_KEY}" "DisplayName" "$(^Name)"
  WriteRegStr HKCU "${PRODUCT_UNINST_KEY}" "UninstallString" "$INSTDIR\uninst.exe"
  WriteRegStr HKCU "${PRODUCT_UNINST_KEY}" "DisplayIcon" "$INSTDIR\teamcraft-app.exe"
  WriteRegStr HKCU "${PRODUCT_UNINST_KEY}" "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegStr HKCU "${PRODUCT_UNINST_KEY}" "URLInfoAbout" "${PRODUCT_WEB_SITE}"
  WriteRegStr HKCU "${PRODUCT_UNINST_KEY}" "Publisher" "${PRODUCT_PUBLISHER}"
  WriteRegDWORD HKCU "${PRODUCT_UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${PRODUCT_UNINST_KEY}" "NoRepair" 1
SectionEnd

Section Uninstall
  Delete "$SMPROGRAMS\TeamCraft\TeamCraft.lnk"
  Delete "$SMPROGRAMS\TeamCraft\TeamCraft Help.lnk"
  Delete "$SMPROGRAMS\TeamCraft\Uninstall TeamCraft.lnk"
  RMDir "$SMPROGRAMS\TeamCraft"
  Delete "$INSTDIR\teamcraft-app.exe"
  Delete "$INSTDIR\teamcraft.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\HELP.md"
  Delete "$INSTDIR\CHANGELOG.md"
  Delete "$INSTDIR\uninst.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "${PRODUCT_UNINST_KEY}"
  DeleteRegKey HKCU "${PRODUCT_DIR_REGKEY}"
  SetAutoClose true
SectionEnd
