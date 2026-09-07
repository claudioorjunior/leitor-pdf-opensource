Unicode true
; Tsuro — instalador CurrentUser (sem admin).
; Flags: /S silencioso. Destino: %LOCALAPPDATA%\Programs\Tsuro
; Pdfium vai ao lado do exe (o motor procura pdfium.dll no mesmo diretório).
; Compilar: makensis /DVERSION=0.1.0 /DSRC=dist\windows /DOUT=dist\Tsuro-0.1.0-x86_64-pc-windows-msvc-setup.exe scripts\windows\tsuro.nsi

!ifndef VERSION
  !define VERSION "0.1.0"
!endif
!ifndef SRC
  !define SRC "..\..\dist\windows"
!endif
!ifndef OUT
  !define OUT "..\..\dist\Tsuro-${VERSION}-x86_64-pc-windows-msvc-setup.exe"
!endif

Name "Tsuro ${VERSION}"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\Tsuro"
RequestExecutionLevel user
SetCompressor /SOLID lzma
AllowRootDirInstall false

!include "MUI2.nsh"
!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "PortugueseBR"

Section "Tsuro" SecApp
  SetOutPath "$INSTDIR"
  File "${SRC}\tsuro.exe"
  File "${SRC}\pdfium.dll"
  File /oname=LICENSE.txt "${SRC}\LICENSE"
  CreateDirectory "$SMPROGRAMS"
  CreateShortCut "$SMPROGRAMS\Tsuro.lnk" "$INSTDIR\tsuro.exe"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tsuro" "DisplayName" "Tsuro"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tsuro" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tsuro" "Publisher" "Tsuro"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tsuro" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tsuro" "UninstallString" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\tsuro.exe"
  Delete "$INSTDIR\pdfium.dll"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\Uninstall.exe"
  Delete "$SMPROGRAMS\Tsuro.lnk"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tsuro"
SectionEnd
