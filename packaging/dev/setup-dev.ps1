# ============================================================
# 脚本用途：新机器一键搭好 RDC 的开发生成环境（幂等，可重复执行）
#
# 层级：packaging/dev（开发环境线）
# 调用方：MIGRATION.md 第二节；迁移后第一件事
#
# 口径：
#   - 版本写死：Rust 1.99.1、Flutter 3.47.9、Python 3.12，与 CI（release-windows.yml）一致；
#   - 每步先检测「已安装且版本匹配」则跳过，否则用 winget 或官方归档安装；
#   - 不改系统级 PATH，只写用户级环境变量；
#   - -WhatIf 只打印计划，不下载不安装（用于自检脚本逻辑）。
# ============================================================
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$RustVersion = "1.99.1",
    [string]$FlutterVersion = "3.47.9",
    [string]$PythonVersion = "3.12",
    [string]$DevRoot = "C:\dev",
    [switch]$SkipFlutter,
    [switch]$SkipBuildTools
)

$ErrorActionPreference = "Stop"

# =====** 输出与通用函数 =====**
$script:Report = New-Object System.Collections.Generic.List[object]

function Add-Report {
    param([string]$Name, [string]$Version, [string]$Status)
    $script:Report.Add([pscustomobject]@{ 组件 = $Name; 版本 = $Version; 状态 = $Status })
    Write-Host ("[{0}] {1} {2}" -f $Status, $Name, $Version)
}

function Get-CommandVersion {
    param([string]$Command, [string[]]$Arguments)
    try {
        $output = & $Command @Arguments 2>&1 | Out-String
    } catch {
        return $null
    }
    if ($LASTEXITCODE -ne 0) { return $null }
    # 取第一行里第一个形如 x.y.z 的版本号。
    $match = [regex]::Match($output, "(\d+\.\d+(?:\.\d+)?)")
    if ($match.Success) { return $match.Groups[1].Value }
    return $null
}

function Test-VersionAtLeast {
    param([string]$Actual, [string]$Expected)
    if ([string]::IsNullOrWhiteSpace($Actual)) { return $false }
    try {
        return ([version]$Actual -ge [version]$Expected)
    } catch {
        return $false
    }
}

function Get-Winget {
    $command = Get-Command winget -ErrorAction SilentlyContinue
    if (-not $command) {
        throw "找不到 winget：请先在「应用安装程序」中更新 App Installer，再运行本脚本。"
    }
    return $command.Source
}

function Install-WingetPackage {
    param([string]$Id, [string]$DisplayName, [string[]]$ExtraArguments = @())
    if (-not $PSCmdlet.ShouldProcess($DisplayName, "winget install $Id")) { return }
    $winget = Get-Winget
    $arguments = @(
        "install", "--id", $Id, "--exact", "--silent",
        "--accept-source-agreements", "--accept-package-agreements"
    ) + $ExtraArguments
    Write-Host "[install] winget $($arguments -join ' ')"
    & $winget @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "winget 安装 $Id 失败（退出码 $LASTEXITCODE）"
    }
}

function Add-UserPath {
    param([string]$Directory)
    if (-not (Test-Path $Directory)) { return }
    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    if (($current -split ";") -contains $Directory) { return }
    if (-not $PSCmdlet.ShouldProcess("用户 PATH", "追加 $Directory")) { return }
    $updated = if ([string]::IsNullOrWhiteSpace($current)) { $Directory } else { "$current;$Directory" }
    [Environment]::SetEnvironmentVariable("Path", $updated, "User")
    # 同时作用于当前进程，脚本后续步骤可直接用到。
    $env:Path = "$env:Path;$Directory"
    Write-Host "[env] 用户 PATH += $Directory"
}

# =====** 1. Git =====**
$gitVersion = Get-CommandVersion -Command "git" -Arguments @("--version")
if (Test-VersionAtLeast $gitVersion "2.30") {
    Add-Report "Git" $gitVersion "已安装"
} else {
    Install-WingetPackage -Id "Git.Git" -DisplayName "Git for Windows"
    $gitVersion = Get-CommandVersion -Command "git" -Arguments @("--version")
    Add-Report "Git" $gitVersion "新装"
}

# =====** 2. Rust（固定版本，不用 stable 浮动） =====**
$rustcVersion = Get-CommandVersion -Command "rustc" -Arguments @("--version")
if ($rustcVersion -eq $RustVersion) {
    Add-Report "Rust" $rustcVersion "已安装"
} else {
    if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
        Install-WingetPackage -Id "Rustlang.Rustup" -DisplayName "rustup"
        $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
        Add-UserPath $cargoBin
    }
    if ($PSCmdlet.ShouldProcess("Rust $RustVersion", "rustup toolchain install")) {
        & rustup toolchain install $RustVersion --profile minimal --component rustfmt --component clippy
        if ($LASTEXITCODE -ne 0) { throw "安装 Rust $RustVersion 失败（退出码 $LASTEXITCODE）" }
        & rustup default $RustVersion
        if ($LASTEXITCODE -ne 0) { throw "切换默认 Rust 工具链失败（退出码 $LASTEXITCODE）" }
    }
    $rustcVersion = Get-CommandVersion -Command "rustc" -Arguments @("--version")
    Add-Report "Rust" $rustcVersion "新装"
}

# =====** 3. VS 2022 Build Tools + MSVC =====**
# 只装 Build Tools（无 IDE），组件按 flutter build windows 与 cargo 链接所需最小集。
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vcInstalled = $false
if (Test-Path $vswhere) {
    $vcPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    $vcInstalled = -not [string]::IsNullOrWhiteSpace($vcPath)
}
if ($SkipBuildTools) {
    Add-Report "VS 2022 Build Tools" "跳过（-SkipBuildTools）" "跳过"
} elseif ($vcInstalled) {
    $vsVersion = (& $vswhere -latest -products * -property installationVersion | Out-String).Trim()
    Add-Report "VS 2022 Build Tools" $vsVersion "已安装"
} else {
    # 官方文档口径：winget 装 BuildTools 必须用 --override 传 vs_installer 参数。
    $override = "--quiet --wait --norestart --nocache " +
        "--add Microsoft.VisualStudio.Workload.VCTools " +
        "--includeRecommended " +
        "--add Microsoft.VisualStudio.Component.Windows11SDK.22621"
    Install-WingetPackage -Id "Microsoft.VisualStudio.2022.BuildTools" -DisplayName "VS 2022 Build Tools" `
        -ExtraArguments @("--override", $override)
    Add-Report "VS 2022 Build Tools" "见 vswhere" "新装"
}

# =====** 4. Flutter（固定 3.47.9） =====**
if ($SkipFlutter) {
    Add-Report "Flutter" "跳过（-SkipFlutter）" "跳过"
} else {
    $flutterExe = (Get-Command flutter -ErrorAction SilentlyContinue).Source
    if (-not $flutterExe) {
        $candidate = Join-Path $DevRoot "flutter\bin\flutter.bat"
        if (Test-Path $candidate) { $flutterExe = $candidate }
    }
    $flutterVersion = if ($flutterExe) {
        (Get-CommandVersion -Command $flutterExe -Arguments @("--version"))
    } else { $null }
    if ($flutterVersion -eq $FlutterVersion) {
        Add-Report "Flutter" $flutterVersion "已安装"
        Add-UserPath (Split-Path $flutterExe -Parent)
    } else {
        # 官方归档直链（releases_windows.json 里 stable 通道的固定命名规则）。
        $zipUrl = "https://storage.googleapis.com/flutter_infra_release/releases/stable/windows/flutter_windows_$FlutterVersion-stable.zip"
        $zipPath = Join-Path $env:TEMP "flutter_windows_$FlutterVersion-stable.zip"
        $flutterRoot = Join-Path $DevRoot "flutter"
        if ($PSCmdlet.ShouldProcess($flutterRoot, "下载并解压 Flutter $FlutterVersion（约 1.9 GB）")) {
            New-Item -ItemType Directory -Force -Path $DevRoot | Out-Null
            if (-not (Test-Path $zipPath)) {
                Write-Host "[download] $zipUrl"
                Invoke-WebRequest -Uri $zipUrl -OutFile $zipPath -UseBasicParsing
            }
            if (Test-Path $flutterRoot) {
                throw "$flutterRoot 已存在但版本不是 $FlutterVersion；请先手动移除或改名，避免覆盖已有 SDK"
            }
            Expand-Archive -Path $zipPath -DestinationPath $DevRoot -Force
            Remove-Item $zipPath -Force
        }
        $flutterExe = Join-Path $flutterRoot "bin\flutter.bat"
        Add-UserPath (Join-Path $flutterRoot "bin")
        # 首次调用会下载 Dart SDK 并展开缓存（不是无操作，需要网络）。
        if ($PSCmdlet.ShouldProcess("flutter", "首次初始化（下载 Dart SDK）")) {
            & $flutterExe --version
            if ($LASTEXITCODE -ne 0) { throw "Flutter 首次初始化失败（退出码 $LASTEXITCODE）" }
            & $flutterExe doctor -v | Out-String | Write-Host
        }
        Add-Report "Flutter" $FlutterVersion "新装"
    }
}

# =====** 5. NSIS =====**
$makensis = (Get-Command makensis -ErrorAction SilentlyContinue).Source
if (-not $makensis) {
    $fallback = "C:\Program Files (x86)\NSIS\makensis.exe"
    if (Test-Path $fallback) { $makensis = $fallback }
}
if ($makensis) {
    $nsisVersion = Get-CommandVersion -Command $makensis -Arguments @("/VERSION")
    Add-Report "NSIS" $nsisVersion "已安装"
} else {
    Install-WingetPackage -Id "NSIS.NSIS" -DisplayName "NSIS"
    Add-Report "NSIS" "见安装日志" "新装"
}

# =====** 6. Python 3.12 =====**
$pythonVersion = Get-CommandVersion -Command "python" -Arguments @("--version")
if ($pythonVersion -and $pythonVersion.StartsWith($PythonVersion)) {
    Add-Report "Python" $pythonVersion "已安装"
} else {
    Install-WingetPackage -Id "Python.Python.3.12" -DisplayName "Python 3.12"
    $pythonVersion = Get-CommandVersion -Command "python" -Arguments @("--version")
    Add-Report "Python" $pythonVersion "新装"
}

# =====** 7. 构建目录不落 C 盘 =====**
# 本项目 cargo 全量构建可达数 GB；默认指到非系统盘，避免 C 盘塞满。
if ([string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR) -and (Test-Path "E:\")) {
    if ($PSCmdlet.ShouldProcess("用户环境变量 CARGO_TARGET_DIR", "设为 E:\rdc-target")) {
        [Environment]::SetEnvironmentVariable("CARGO_TARGET_DIR", "E:\rdc-target", "User")
        $env:CARGO_TARGET_DIR = "E:\rdc-target"
        Write-Host "[env] CARGO_TARGET_DIR=E:\rdc-target"
    }
} elseif ($env:CARGO_TARGET_DIR) {
    Write-Host "[env] CARGO_TARGET_DIR 已设置：$($env:CARGO_TARGET_DIR)"
}

# =====** 8. 版本表 =====**
Write-Host ""
Write-Host "===== 实际版本 ====="
$script:Report | Format-Table -AutoSize | Out-String | Write-Host

Write-Host @"
===== 必须手动安装（脚本不代劳） =====
1) 显卡驱动（NVIDIA/AMD/Intel 官方安装器）：
   原因：AMF 编码依赖显卡驱动自带的 amfrt64.dll；屏幕采集需要驱动提供的 DXGI/DDA 能力。
   用 winget 装的通用驱动不带 AMF 运行库，自更新与安装器都替不了这一步。
2) ToDesk（或等价远程控制）：
   原因：本机验证锁屏/无物理显示器场景要靠 ToDesk 虚拟显示器提供可采集的显示输出；
   虚拟显示器驱动是 ToDesk 安装时附带的可选组件，未安装会出现「白屏 / 无帧」。
3) 代码签名证书（EV/OV）：
   原因：签名私钥不在迁移包里（安全约束），Release 与安装包的 Authenticode 签名需在新机器重新配置。
4) FFmpeg LGPL 共享构建（可选）：
   默认期望 C:\dev\ffmpeg-out；缺失时 build_installer.ps1 会显式警告并退回「仅 MF 解码」。
"@
