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

; 产品标识唯一真源：窗口标题、快捷方式、卸载项、安装包界面共用同一组常量。
!define PRODUCT_NAME "RDC 远程桌面"
!define COMPANY_NAME "RDC Team"
!define APP_ICON "..\..\app\windows\runner\resources\app_icon.ico"

Name "${PRODUCT_NAME}"
OutFile "${OUTPUT}"
InstallDir "$LOCALAPPDATA\RDC"
InstallDirRegKey HKCU "Software\RDC" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

; 安装包自身的版本资源，与卸载项 DisplayVersion 保持一致。
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${PRODUCT_NAME}"
VIAddVersionKey "CompanyName" "${COMPANY_NAME}"
VIAddVersionKey "FileDescription" "${PRODUCT_NAME} 安装程序"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "Copyright (C) 2026 RDC Team."

; =====** 安装界面 =====**
!define MUI_ABORTWARNING
!define MUI_ICON "${APP_ICON}"
!define MUI_UNICON "${APP_ICON}"
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
  CreateShortcut "$SMPROGRAMS\${PRODUCT_NAME}.lnk" "$INSTDIR\rdc.exe"
  CreateShortcut "$DESKTOP\${PRODUCT_NAME}.lnk" "$INSTDIR\rdc.exe"

  ; 注册卸载项（HKCU，无需管理员权限）。
  WriteRegStr HKCU "Software\RDC" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC" "DisplayName" "${PRODUCT_NAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC" "Publisher" "${COMPANY_NAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC" "DisplayIcon" "$INSTDIR\rdc.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC" "UninstallString" "$INSTDIR\Uninstall-RDC.exe"
SectionEnd

; =====** 卸载段 =====**
Section "Uninstall"
  Delete "$INSTDIR\Uninstall-RDC.exe"
  ; 只删安装目录，不触碰用户配置（%APPDATA%\RDC 保留）。
  RMDir /r "$INSTDIR"
  Delete "$SMPROGRAMS\${PRODUCT_NAME}.lnk"
  Delete "$DESKTOP\${PRODUCT_NAME}.lnk"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC"
  DeleteRegKey HKCU "Software\RDC"
SectionEnd
