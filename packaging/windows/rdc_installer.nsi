; ============================================================
; 脚本用途：RDC Windows NSIS 安装脚本
;
; 层级：packaging/windows（由 build_installer.ps1 调用，CI 与本地共用）
; 参数：/DVERSION 版本号 /DSRC Flutter Release 目录 /DOUTPUT 输出 exe
; 安装位置：`C:\Program Files\RDC`（per-machine，需要管理员权限）
; 配置位置：`<安装目录>\config`，安装时只对该子目录授予 Users 修改权限，
;          程序本体保持只读，运行时保存设置不会触发 UAC。
; ============================================================

!include "MUI2.nsh"
; LogicLib 提供 ${If}/${Else}/${EndIf}；卸载段判断 rename 成败要用到。
!include "LogicLib.nsh"
; FileFunc 提供 ${GetParent}：自定义安装目录时也要把 config 挪到同卷父目录。
!include "FileFunc.nsh"
Unicode true

!ifndef VERSION
  !define VERSION "0.1.1"
!endif
!ifndef SRC
  !error "必须通过 /DSRC 指定 Flutter Release 目录"
!endif
!ifndef OUTPUT
  !define OUTPUT "RDC-Setup.exe"
!endif
; 提权级别可被 /DREQUEST_LEVEL=user 覆盖：仅用于无管理员的本地回归测试，
; 正式发布必须保持默认 admin（per-machine 安装到 Program Files）。
!ifndef REQUEST_LEVEL
  !define REQUEST_LEVEL "admin"
!endif

; 产品标识唯一真源：窗口标题、快捷方式、卸载项、安装包界面共用同一组常量。
!define PRODUCT_NAME "RDC 远程桌面"
!define COMPANY_NAME "RDC Team"
!define APP_ICON "..\..\app\windows\runner\resources\app_icon.ico"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\RDC"

Name "${PRODUCT_NAME}"
OutFile "${OUTPUT}"
; per-machine 安装：固定写 Program Files，卸载信息写 HKLM。
InstallDir "$PROGRAMFILES64\RDC"
InstallDirRegKey HKLM "Software\RDC" "InstallDir"
RequestExecutionLevel ${REQUEST_LEVEL}
SetCompressor /SOLID lzma
; 64 位视图：避免 32 位重定向把注册表写到 WOW6432Node。
; SetRegView 只能在 Section/Function 内调用，这里放到安装段与卸载段开头。

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
  SetRegView 64
  SetOutPath "$INSTDIR"
  ; /r 递归复制包括 data、dll、rdc-cli.exe、rdc-updater.exe 与 BUILD_INFO.txt。
  ; 升级安装时 config 不在 SRC 中，因此用户配置不会被覆盖。
  File /r "${SRC}\*.*"

  ; config 目录单独创建：它必须可被普通用户写入（保存设置不弹 UAC）。
  CreateDirectory "$INSTDIR\config"

  ; 只对 config 授予 Users 修改权限；程序本体仍是只读（Program Files 默认 ACL）。
  ; 用 SID 而不是 “Users” 名称，避免不同语言/区域系统上名称不匹配。
  nsExec::ExecToLog 'icacls "$INSTDIR\config" /grant *S-1-5-32-545:(OI)(CI)M /T /C'
  Pop $0
  ${If} $0 != 0
    MessageBox MB_ICONEXCLAMATION|MB_OK "配置目录授权失败（icacls 退出码 $0）。$\r$\n程序仍可安装，但普通用户保存设置可能失败；请以管理员身份运行：$\r$\nicacls $\"$INSTDIR\config$\" /grant *S-1-5-32-545:(OI)(CI)M /T"
  ${EndIf}

  WriteUninstaller "$INSTDIR\Uninstall-RDC.exe"
  CreateShortcut "$SMPROGRAMS\${PRODUCT_NAME}.lnk" "$INSTDIR\rdc.exe"
  CreateShortcut "$DESKTOP\${PRODUCT_NAME}.lnk" "$INSTDIR\rdc.exe"

  ; 注册卸载项（HKLM：per-machine 安装）。
  WriteRegStr HKLM "Software\RDC" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayName" "${PRODUCT_NAME}"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINST_KEY}" "Publisher" "${COMPANY_NAME}"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\rdc.exe"
  WriteRegStr HKLM "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINST_KEY}" "UninstallString" "$INSTDIR\Uninstall-RDC.exe"
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair" 1
SectionEnd

; =====** 卸载段 =====**
Section "Uninstall"
  SetShellVarContext all
  SetRegView 64
  Delete "$INSTDIR\Uninstall-RDC.exe"
  Delete "$SMPROGRAMS\${PRODUCT_NAME}.lnk"
  Delete "$DESKTOP\${PRODUCT_NAME}.lnk"
  DeleteRegKey HKLM "${UNINST_KEY}"
  DeleteRegKey HKLM "Software\RDC"

  ; 配置文件由用户决定去留：默认保留（NO），避免卸载即丢密钥与设备列表。
  MessageBox MB_YESNO|MB_ICONQUESTION \
    "是否同时删除配置文件？$\r$\n$\r$\n路径：$INSTDIR\config$\r$\n$\r$\n选择“否”将保留配置（推荐，重新安装后可继续使用本机身份）。" \
    IDYES rdc_delete_config IDNO rdc_keep_config

  rdc_delete_config:
    RMDir /r "$INSTDIR"
    Goto rdc_uninstall_done

  rdc_keep_config:
    ; 先把 config 挪到安装目录的同卷父目录（同卷 rename 不复制数据），
    ; 再删除安装目录，最后放回原位；自定义安装目录同样适用。
    ${GetParent} "$INSTDIR" $R1
    StrCpy $R0 "$R1\RDC-config-keep"
    RMDir /r "$R0"
    ClearErrors
    Rename "$INSTDIR\config" "$R0"
    ${If} ${Errors}
      ; rename 失败时退而求其次：只删已知安装内容，保留 config 原样。
      RMDir /r "$INSTDIR\data"
      Delete "$INSTDIR\*.dll"
      Delete "$INSTDIR\*.exe"
      Delete "$INSTDIR\*.txt"
      Delete "$INSTDIR\*.md"
    ${Else}
      RMDir /r "$INSTDIR"
      CreateDirectory "$INSTDIR\config"
      Rename "$R0" "$INSTDIR\config"
    ${EndIf}
    Goto rdc_uninstall_done

  rdc_uninstall_done:
SectionEnd
