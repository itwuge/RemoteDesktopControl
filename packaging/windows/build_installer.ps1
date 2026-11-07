# ============================================================
# 脚本用途：Windows 打包主流程（cargo 产物 + Flutter 产物 → NSIS 安装包）
#
# 层级：packaging（CI 与本地共用）
# 调用方：.github/workflows/ci.yml 的 windows job；本地可直接运行
#
# 降级策略：未安装 makensis 时不静默失败，而是产出便携 zip 并在日志写明原因。
# ============================================================
param(
    [string]$Version = "",
    [switch]$SkipFlutter
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$DistDir = Join-Path $RepoRoot "dist"
$ReleaseDir = Join-Path $RepoRoot "app\build\windows\x64\runner\Release"

# =====** 外部命令包装 =====**
# 为什么需要：Windows PowerShell 5 会把原生命令的 stderr 当成 ErrorRecord，
# 在 $ErrorActionPreference=Stop 下直接中断（cargo 的进度条走 stderr）。
# 这里临时放宽为 Continue，仅依据退出码判定成败。
function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)][string[]]$Arguments
    )
    $previous = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $Command @Arguments 2>&1 | ForEach-Object { Write-Host $_ }
    } finally {
        $code = $LASTEXITCODE
        $ErrorActionPreference = $previous
    }
    if ($code -ne 0) {
        throw "$Command 执行失败（退出码 $code）：$($Arguments -join ' ')"
    }
}

# =====** 版本号 =====**
if ([string]::IsNullOrWhiteSpace($Version)) {
    # 版本唯一真源：根 Cargo.toml 的 workspace.package.version。
    $manifest = Get-Content (Join-Path $RepoRoot "Cargo.toml") -Raw
    $match = [regex]::Match($manifest, "version\s*=\s*""([^""]+)""")
    $Version = if ($match.Success) { $match.Groups[1].Value } else { "0.1.0" }
}
Write-Host "[packaging/windows] 版本 $Version"

# =====** 构建 Rust 产物 =====**
Push-Location $RepoRoot
try {
    # rdc-cli 随安装包分发，供用户自助诊断（docs/08 §5 产物附加要求）。
    Invoke-Native -Command "cargo" -Arguments @("build", "--release", "-p", "rdc-bridge", "-p", "rdc-cli")
} finally {
    Pop-Location
}

# =====** 构建 Flutter =====**
if (-not $SkipFlutter) {
    Push-Location (Join-Path $RepoRoot "app")
    try {
        Invoke-Native -Command "flutter" -Arguments @("build", "windows", "--release")
    } finally {
        Pop-Location
    }
}

if (-not (Test-Path $ReleaseDir)) {
    throw "Flutter 产物目录不存在：$ReleaseDir（请确认 flutter build windows 成功）"
}

# =====** 补齐动态库与 CLI =====**
# CMake 钩子通常已复制 dll；这里再复制一次，保证 -SkipFlutter 场景也能打包。
Copy-Item (Join-Path $RepoRoot "target\release\rdc_bridge.dll") $ReleaseDir -Force
Copy-Item (Join-Path $RepoRoot "target\release\rdc-cli.exe") $ReleaseDir -Force

# =====** VC++ 运行库（app-local 部署） =====**
# 依据 dumpbin /dependents：rdc.exe 需要 MSVCP140 / VCRUNTIME140 / VCRUNTIME140_1，
# rdc_bridge.dll 需要 VCRUNTIME140；flutter_windows.dll 静态链接 CRT，不需要额外 DLL。
# 优先从 MSVC 的 Redist 目录取（版本与编译工具链一致），缺失时回退系统目录。
$crtNames = @("msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll")
$vcRedist = Get-ChildItem "C:\Program Files*\Microsoft Visual Studio\*\BuildTools\VC\Redist\MSVC\*\x64\Microsoft.VC143.CRT" -Directory -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending | Select-Object -First 1
foreach ($name in $crtNames) {
    $source = $null
    if ($vcRedist) {
        $candidate = Join-Path $vcRedist.FullName $name
        if (Test-Path $candidate) { $source = $candidate }
    }
    if (-not $source) {
        $candidate = Join-Path $env:SystemRoot "System32\$name"
        if (Test-Path $candidate) { $source = $candidate }
    }
    if (-not $source) { throw "缺少 VC++ 运行库 $name，无法完成 app-local 部署" }
    Copy-Item $source $ReleaseDir -Force
    Write-Host "[packaging/windows] VC++ 运行库：$name ← $source"
}

# =====** 许可证与第三方声明 =====**
foreach ($doc in @("LICENSE", "THIRD_PARTY_NOTICES.md", "README.md")) {
    $candidate = Join-Path $RepoRoot $doc
    if (Test-Path $candidate) { Copy-Item $candidate $ReleaseDir -Force }
}

# FFmpeg 解码后端默认关闭（rdc-decode 的 ffmpeg feature 未启用），产物不得混入 FFmpeg DLL。
$ffmpegDlls = Get-ChildItem $ReleaseDir -Filter "av*.dll" -ErrorAction SilentlyContinue
if ($ffmpegDlls) {
    throw "Release 目录出现 FFmpeg DLL：$($ffmpegDlls.Name -join ', ')；默认构建不应包含 FFmpeg"
}

# 构建信息：产物必须可追溯到 commit 与版本（docs/08 §5）。
$commit = (git -C $RepoRoot rev-parse --short HEAD 2>$null)
if (-not $commit) { $commit = "unknown" }
Set-Content -Path (Join-Path $ReleaseDir "BUILD_INFO.txt") -Value "version=$Version`ncommit=$commit"

# =====** NSIS 安装包（缺失时降级为 zip） =====**
New-Item -ItemType Directory -Force -Path $DistDir | Out-Null
$nsi = Join-Path $PSScriptRoot "rdc_installer.nsi"
$outExe = Join-Path $DistDir "RDC-$Version-windows-x64-setup.exe"
$makensisPath = (Get-Command makensis -ErrorAction SilentlyContinue).Source
if (-not $makensisPath) {
    $fallback = "C:\Program Files (x86)\NSIS\makensis.exe"
    if (Test-Path $fallback) { $makensisPath = $fallback }
}

# =====** 便携 zip（解压即用，目录内容平铺到 zip 根） =====**
$zip = Join-Path $DistDir "RDC-$Version-windows-portable.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path (Join-Path $ReleaseDir "*") -DestinationPath $zip

$artifacts = @($zip)
if ($makensisPath) {
    Push-Location $PSScriptRoot
    try {
        Invoke-Native -Command $makensisPath -Arguments @(
            "/DVERSION=$Version", "/DSRC=$ReleaseDir", "/DOUTPUT=$outExe", $nsi
        )
    } finally {
        Pop-Location
    }
    Write-Host "[packaging/windows] 安装包：$outExe"
    $artifacts = @($outExe, $zip)
} else {
    # 明确降级：CI 中会先 choco install nsis，本地缺工具时不应让流水线"悄悄成功"。
    Write-Warning "未找到 makensis，仅产出便携 zip（无安装包）。安装 NSIS 后可生成安装包。"
}

# =====** 校验和 =====**
foreach ($artifact in $artifacts) {
    $hash = (Get-FileHash $artifact -Algorithm SHA256).Hash.ToLower()
    "$hash  $(Split-Path $artifact -Leaf)" | Set-Content -Path "$artifact.sha256"
    Write-Host "[packaging/windows] 完成：$artifact"
}
