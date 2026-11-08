; ============================================================
; 脚本用途：RDC Windows NSIS 安装脚本
;
; 层级：packaging/windows（由 build_installer.ps1 调用，CI 与本地共用）
; 参数：/DVERSION 版本号 /DSRC Flutter Release 目录 /DOUTPUT 输出 exe
; 安装位置：C:\Program Files\RDC（固定安装目录；程序目录只读，仅 config 子目录可写）
; ============================================================

!include "MUI2.nsh"
!include "LogicLib.nsh"
Unicode true

!ifndef VERSION
  !define VERSION "0.2.0"
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
; 固定安装到 Program Files：程序文件只读，配置独立在 $INSTDIR\config。
InstallDir "$PROGRAMFILES64\RDC"
InstallDirRegKey HKLM "Software\RDC" "InstallDir"
RequestExecutionLevel admin
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
  SetShellVarContext all
  SetOutPath "$INSTDIR"
  ; /r 递归复制包括 data、dll、rdc-cli.exe 与 BUILD_INFO.txt。
  File /r "${SRC}\*.*"

  ; 配置目录与程序目录分离：Program Files 默认只读，仅对 config 授予普通用户写权限，
  ; 保证应用保存设置时不触发第二次 UAC（SID S-1-5-32-545 = Users）。
  CreateDirectory "$INSTDIR\config"
  nsExec::ExecToLog 'icacls "$INSTDIR\config" /grant *S-1-5-32-545:(OI)(CI)M /T'
  Pop $0
  ${If} $0 != "0"
    MessageBox MB_ICONEXCLAMATION|MB_OK "无法为配置目录授予写权限（icacls 退出码 $0）。保存设置时可能需要管理员权限。"
  ${EndIf}

  WriteUninstaller "$INSTDIR\Uninstall-RDC.exe"
  CreateShortcut "$SMPROGRAMS\${PRODUCT_NAME}.lnk" "$INSTDIR\rdc.exe"
  CreateShortcut "$DESKTOP\${PRODUCT_NAME}.lnk" "$INSTDIR\rdc.exe"

  ; 注册卸载项（HKLM，按机器安装；卸载时需要一次管理员权限）。
  WriteRegStr HKLM "Software\RDC" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC" "DisplayName" "${PRODUCT_NAME}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC" "Publisher" "${COMPANY_NAME}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC" "DisplayIcon" "$INSTDIR\rdc.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC" "UninstallString" "$INSTDIR\Uninstall-RDC.exe"
SectionEnd

; =====** 卸载段 =====**
Section "Uninstall"
  SetShellVarContext all
  ; 默认保留配置与日志（静默卸载同样保留）；用户明确选择「否」才随程序删除。
  StrCpy $R0 "keep"
  IfSilent +2
  MessageBox MB_YESNO|MB_ICONQUESTION "是否保留配置与日志？$\r$\n$\r$\n是：保留在 $INSTDIR\config$\r$\n否：随程序一并删除" IDYES +1
  StrCpy $R0 "remove"

  ${If} $R0 == "keep"
    ${If} ${FileExists} "$INSTDIR\config\*.*"
      ; 先暂存到临时目录，删完程序后再放回，避免 RMDir /r 误删配置。
      RMDir /r "$TEMP\RDC-config-uninstall"
      ClearErrors
      Rename "$INSTDIR\config" "$TEMP\RDC-config-uninstall"
      ${If} ${Errors}
        MessageBox MB_ICONSTOP|MB_OK "无法暂存配置目录，已取消卸载以保护配置。"
        Abort
      ${EndIf}
    ${EndIf}
  ${EndIf}

  Delete "$INSTDIR\Uninstall-RDC.exe"
  RMDir /r "$INSTDIR"
  Delete "$SMPROGRAMS\${PRODUCT_NAME}.lnk"
  Delete "$DESKTOP\${PRODUCT_NAME}.lnk"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC"
  DeleteRegKey HKLM "Software\RDC"

  ${If} $R0 == "keep"
    CreateDirectory "$INSTDIR"
    Rename "$TEMP\RDC-config-uninstall" "$INSTDIR\config"
  ${EndIf}
SectionEnd
