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
    cargo build --release -p rdc-bridge -p rdc-cli
} finally {
    Pop-Location
}

# =====** 构建 Flutter =====**
if (-not $SkipFlutter) {
    Push-Location (Join-Path $RepoRoot "app")
    try {
        flutter build windows --release
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
# 构建信息：产物必须可追溯到 commit 与版本（docs/08 §5）。
$commit = (git -C $RepoRoot rev-parse --short HEAD 2>$null)
if (-not $commit) { $commit = "unknown" }
Set-Content -Path (Join-Path $ReleaseDir "BUILD_INFO.txt") -Value "version=$Version`ncommit=$commit"

# =====** NSIS 安装包（缺失时降级为 zip） =====**
New-Item -ItemType Directory -Force -Path $DistDir | Out-Null
$nsi = Join-Path $PSScriptRoot "rdc_installer.nsi"
$outExe = Join-Path $DistDir "RDC-Setup-$Version.exe"
$makensis = Get-Command makensis -ErrorAction SilentlyContinue

if ($makensis) {
    Push-Location $PSScriptRoot
    try {
        & $makensis.Source "/DVERSION=$Version" "/DSRC=$ReleaseDir" "/DOUTPUT=$outExe" $nsi
        if ($LASTEXITCODE -ne 0) { throw "makensis 退出码 $LASTEXITCODE" }
    } finally {
        Pop-Location
    }
    Write-Host "[packaging/windows] 安装包：$outExe"
} else {
    # 明确降级：CI 中会先 choco install nsis，本地缺工具时不应让流水线"悄悄成功"。
    Write-Warning "未找到 makensis，降级产出便携 zip。安装 NSIS 后可生成安装包。"
    $zip = Join-Path $DistDir "RDC-$Version-windows-portable.zip"
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Compress-Archive -Path $ReleaseDir -DestinationPath $zip
    $outExe = $zip
}

# =====** 校验和 =====**
$hash = (Get-FileHash $outExe -Algorithm SHA256).Hash.ToLower()
"$hash  $(Split-Path $outExe -Leaf)" | Set-Content -Path "$outExe.sha256"
Write-Host "[packaging/windows] 完成：$outExe"
