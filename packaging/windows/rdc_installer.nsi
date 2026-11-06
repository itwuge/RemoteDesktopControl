; ============================================================
; 脚本用途：RDC Windows NSIS 安装脚本
;
; 层级：packaging/windows（由 build_installer.ps1 调用，CI 与本地共用）
; 参数：/DVERSION 版本号 /DSRC Flutter Release 目录 /DOUTPUT 输出 exe
; 安装位置：%LOCALAPPDATA%\RDC（免管理员权限，符合 docs/08 §5）
; ============================================================

!include "MUI2.nsh"
Unicode true

!ifndef VERSION
  !define VERSION "0.1.0"
!endif
!ifndef SRC
  !error "必须通过 /DSRC 指定 Flutter Release 目录"
!endif
!ifndef OUTPUT
  !define OUTPUT "RDC-Setup.exe"
!endif

Name "RDC"
OutFile "${OUTPUT}"
InstallDir "$LOCALAPPDATA\RDC"
InstallDirRegKey HKCU "Software\RDC" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

; =====** 安装界面 =====**
!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

; =====** 安装段 =====**
Section "RDC" SecMain
  SetOutPath "$INSTDIR"
  ; /r 递归复制包括 data、dll、rdc-cli.exe 与 BUILD_INFO.txt。
  File /r "${SRC}\*.*"

  WriteUninstaller "$INSTDIR\Uninstall-RDC.exe"
  CreateShortcut "$SMPROGRAMS\RDC.lnk" "$INSTDIR\rdc.exe"
  CreateShortcut "$DESKTOP\RDC.lnk" "$INSTDIR\rdc.exe"

  ; 注册卸载项（HKCU，无需管理员权限）。
  WriteRegStr HKCU "Software\RDC" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC" "DisplayName" "RDC"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC" "Publisher" "RDC Team"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC" "UninstallString" "$INSTDIR\Uninstall-RDC.exe"
SectionEnd

; =====** 卸载段 =====**
Section "Uninstall"
  Delete "$INSTDIR\Uninstall-RDC.exe"
  ; 只删安装目录，不触碰用户配置（%APPDATA%\RDC 保留）。
  RMDir /r "$INSTDIR"
  Delete "$SMPROGRAMS\RDC.lnk"
  Delete "$DESKTOP\RDC.lnk"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC"
  DeleteRegKey HKCU "Software\RDC"
SectionEnd
