; Screencapture — Windows installer (NSIS / Unicode, MUI2).
;
; Builds the desktop install: the exe (with its baked-in icon), the
; bundled FFmpeg engine next to it (integrity-recorded, so the shipped
; engine.sha256 proves the binaries verbatim), shortcuts, and uninstall
; entries. Build with, from the repo root after `cargo build --release`:
;
;   makensis -DROOT=. installer/nsis/screencapture.nsi
;
; NSIS 3.x with MUI2 is required; nothing else (pure script, no plugins).

Unicode true
ManifestDPIAware true

; ROOT may be passed on the makensis command line (/DROOT=<abs path>);
; default is the repository root (invoke makensis from the repo root).
!ifndef ROOT
!define ROOT "."
!endif
!define APPNAME "Screencapture"
!define COMPANY "Eric Lautanen"
!define EXENAME "screencapture.exe"
!define PRODUCTNAME "screencapture"
!define VERSION "0.1.6"
!define VERSIONNUM "0.1.6.0"
!define DESC "Professional 4K screen recorder and still-image capture."

!include "MUI2.nsh"
!include "FileFunc.nsh"

Name "${APPNAME} ${VERSION}"
OutFile "${ROOT}\installer\screencapture-setup.exe"
InstallDir "$PROGRAMFILES64\${APPNAME}"
InstallDirRegKey HKCU "Software\${APPNAME}" "InstallDir"
RequestExecutionLevel admin

Icon "${ROOT}\assets\icon.ico"
UninstallIcon "${ROOT}\assets\icon.ico"

VIProductVersion "${VERSIONNUM}"
VIAddVersionKey /LANG=1033 "CompanyName" "${COMPANY}"
VIAddVersionKey /LANG=1033 "ProductName" "${PRODUCTNAME}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=1033 "FileDescription" "${APPNAME} setup"
VIAddVersionKey /LANG=1033 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Copyright 2026 the screencapture authors"

; ── Pages ────────────────────────────────────────────────────────────────
!define MUI_ABORTWARNING
!define MUI_ICON "${ROOT}\assets\icon.ico"
!define MUI_UNICON "${ROOT}\assets\icon.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXENAME}"
!define MUI_FINISHPAGE_RUN_TEXT "Launch ${APPNAME} now"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; ── Install ──────────────────────────────────────────────────────────────
Section "Install"
    SetShellVarContext all
    SetOutPath "$INSTDIR"

    File "${ROOT}\target\release\${EXENAME}"
    File "${ROOT}\README.md"
    File "${ROOT}\LICENSE"

    ; The engine's license/source note ships under docs\ to match the
    ; README and the uninstaller's paths.
    SetOutPath "$INSTDIR\docs"
    File "${ROOT}\docs\FFMPEG-LICENSE.md"
    SetOutPath "$INSTDIR"

    ; The bundled engine travels verbatim with its integrity record.
    SetOutPath "$INSTDIR\ffmpeg"
    File "${ROOT}\vendor\ffmpeg\ffmpeg.exe"
    File "${ROOT}\vendor\ffmpeg\ffprobe.exe"
    File "${ROOT}\vendor\ffmpeg\ffmpeg.LICENSE.txt"
    File "${ROOT}\vendor\ffmpeg\engine.sha256"

    ; Store the engine where the app looks first: %APPDIR% (the install
    ; directory) is the top of its lookup order after an explicit config.
    WriteRegStr HKCU "Software\${APPNAME}" "InstallDir" "$INSTDIR"

    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" \
        "DisplayName" "${APPNAME}"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" \
        "DisplayVersion" "${VERSION}"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" \
        "Publisher" "${COMPANY}"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" \
        "DisplayIcon" "$\"$INSTDIR\${EXENAME}$\""
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" \
        "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
    WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" \
        "NoModify" 1
    WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" \
        "NoRepair" 1
    ; Estimated installed size, for the "Programs and Features" column.
    ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
    IntFmt $0 "0x%08X" $0
    WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}" \
        "EstimatedSize" $0

    WriteUninstaller "$INSTDIR\uninstall.exe"

    CreateDirectory "$SMPROGRAMS\${APPNAME}"
    CreateShortcut "$SMPROGRAMS\${APPNAME}\${APPNAME}.lnk" "$INSTDIR\${EXENAME}" \
        "" "$INSTDIR\${EXENAME}"
    CreateShortcut "$SMPROGRAMS\${APPNAME}\Uninstall ${APPNAME}.lnk" \
        "$INSTDIR\uninstall.exe"
SectionEnd

; ── Uninstall ────────────────────────────────────────────────────────────
Section "Uninstall"
    SetShellVarContext all
    Delete "$INSTDIR\${EXENAME}"
    Delete "$INSTDIR\README.md"
    Delete "$INSTDIR\LICENSE"
    Delete "$INSTDIR\docs\FFMPEG-LICENSE.md"
    ; Clean up the pre-docs layout left by earlier local builds.
    Delete "$INSTDIR\FFMPEG-LICENSE.md"
    Delete "$INSTDIR\ffmpeg\ffmpeg.exe"
    Delete "$INSTDIR\ffmpeg\ffprobe.exe"
    Delete "$INSTDIR\ffmpeg\ffmpeg.LICENSE.txt"
    Delete "$INSTDIR\ffmpeg\engine.sha256"
    RMDir "$INSTDIR\ffmpeg"
    RMDir "$INSTDIR\docs"
    Delete "$INSTDIR\uninstall.exe"
    RMDir "$INSTDIR"

    RMDir /r "$SMPROGRAMS\${APPNAME}"
    Delete "$DESKTOP\${APPNAME}.lnk"

    DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}"
    ; User settings and recordings are intentionally kept.
SectionEnd
