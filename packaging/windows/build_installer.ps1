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
    [string]$FfmpegDir = "",
    [string]$FfmpegSrcDir = "",
    [switch]$SkipFlutter
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$DistDir = Join-Path $RepoRoot "dist"
$ReleaseDir = Join-Path $RepoRoot "app\build\windows\x64\runner\Release"

# =====** FFmpeg LGPL 共享构建 =====**
# S7 口径：发布包默认 FFmpeg 解码（MF 回退），只带 avcodec/avutil 两个 DLL。
# 构建目录可用参数或环境变量 FFMPEG_DIR 覆盖；缺失时退回「仅 MF」构建并显式警告，
# 不允许悄悄打出没有 FFmpeg 的包而不告知。
if ([string]::IsNullOrWhiteSpace($FfmpegDir)) {
    $FfmpegDir = if ([string]::IsNullOrWhiteSpace($env:FFMPEG_DIR)) { "C:\dev\ffmpeg-out" } else { $env:FFMPEG_DIR }
}
if ([string]::IsNullOrWhiteSpace($FfmpegSrcDir)) {
    $FfmpegSrcDir = "C:\dev\ffmpeg-src"
}
$ffmpegInclude = Join-Path $FfmpegDir "include\libavcodec\avcodec.h"
$ffmpegBin = Join-Path $FfmpegDir "bin"
$withFfmpeg = (Test-Path $ffmpegInclude) -and (Test-Path $ffmpegBin)
if (-not $withFfmpeg) {
    Write-Warning "未找到 FFmpeg LGPL 共享构建（$FfmpegDir）；本次产物不含 FFmpeg，运行时只能走 MF 解码。"
}

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
# cargo 可能不在当前 shell 的 PATH（CI/受限环境常见），先解析绝对路径。
$cargoExe = (Get-Command cargo -ErrorAction SilentlyContinue).Source
if (-not $cargoExe) {
    $candidate = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
    if (Test-Path $candidate) { $cargoExe = $candidate }
}
if (-not $cargoExe) { throw "找不到 cargo；请安装 Rust 工具链或把它加入 PATH" }
Write-Host "[packaging/windows] cargo：$cargoExe"

Push-Location $RepoRoot
try {
    # rdc-cli 随安装包分发，供用户自助诊断（docs/08 §5 产物附加要求）。
    # 有 LGPL 构建时启用 ffmpeg feature：应用默认 FFmpeg 解码，MF 保留回退。
    if ($withFfmpeg) {
        $env:FFMPEG_DIR = $FfmpegDir
        Invoke-Native -Command $cargoExe -Arguments @(
            "build", "--release", "-p", "rdc-bridge", "-p", "rdc-cli",
            "--features", "rdc-bridge/ffmpeg,rdc-cli/ffmpeg"
        )
    } else {
        Invoke-Native -Command $cargoExe -Arguments @("build", "--release", "-p", "rdc-bridge", "-p", "rdc-cli")
    }
} finally {
    Pop-Location
}

# =====** 构建 Flutter =====**
# flutter 常常不在非交互 shell 的 PATH 里（CI/受限环境），先解析绝对路径并注入 PATH，
# 否则 Invoke-Native "flutter" 会抛 CommandNotFoundException。
$flutterExe = (Get-Command flutter -ErrorAction SilentlyContinue).Source
if (-not $flutterExe) {
    $flutterCandidates = @()
    if ($env:FLUTTER_ROOT) { $flutterCandidates += (Join-Path $env:FLUTTER_ROOT "bin\flutter.bat") }
    $flutterCandidates += "C:\dev\flutter\bin\flutter.bat"
    foreach ($candidate in $flutterCandidates) {
        if ($candidate -and (Test-Path $candidate)) { $flutterExe = $candidate; break }
    }
}
if ($flutterExe) {
    $flutterBin = Split-Path $flutterExe -Parent
    if (-not (($env:Path -split ";") -contains $flutterBin)) {
        $env:Path = "$flutterBin;$env:Path"
    }
    Write-Host "[packaging/windows] flutter：$flutterExe"
} elseif (-not $SkipFlutter) {
    throw "找不到 flutter；请安装 Flutter 并加入 PATH，或用 -SkipFlutter 复用已有产物"
}
if (-not $SkipFlutter) {
    Push-Location (Join-Path $RepoRoot "app")
    try {
        Invoke-Native -Command $flutterExe -Arguments @("build", "windows", "--release")
    } finally {
        Pop-Location
    }
}

if (-not (Test-Path $ReleaseDir)) {
    throw "Flutter 产物目录不存在：$ReleaseDir（请确认 flutter build windows 成功）"
}

# =====** 补齐动态库与 CLI =====**
# CMake 钩子通常已复制 dll；这里再复制一次，保证 -SkipFlutter 场景也能打包。
# cargo 产物目录跟随 CARGO_TARGET_DIR（外部 target 方案），未设置时退回仓库内 target。
$cargoTargetDir = if ([string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) { Join-Path $RepoRoot "target" } else { $env:CARGO_TARGET_DIR }
Write-Host "[packaging/windows] cargo 产物目录：$cargoTargetDir"
Copy-Item (Join-Path $cargoTargetDir "release\rdc_bridge.dll") $ReleaseDir -Force
Copy-Item (Join-Path $cargoTargetDir "release\rdc-cli.exe") $ReleaseDir -Force

# AMF 原生编码 shim：bridge 运行时用 LoadLibrary 动态加载，缺它只能回退 MFT。
# AMF 运行库（amfrt64.dll）由显卡驱动提供，不随包分发；rdc-amf.dll 是本仓库自建产物。
$amfShim = Join-Path $RepoRoot "target\amf\rdc-amf.dll"
if (Test-Path $amfShim) {
    Copy-Item $amfShim $ReleaseDir -Force
    Write-Host "[packaging/windows] AMF shim：rdc-amf.dll ($((Get-Item $amfShim).Length) B)"
} else {
    Write-Warning "缺少 target\amf\rdc-amf.dll：本次产物的编码器只能是 MFT。先运行 crates/rdc-codec/shim/build_amf.ps1"
}

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

# 诊断说明（RDR-001 错误码表 + 三步自助排查）：安装目录内可直接打开，
# 与保底窗口的「查看诊断说明」链接指向同一份文件。
$diagnosticsPage = Join-Path $PSScriptRoot "diagnostics.html"
if (Test-Path $diagnosticsPage) {
    Copy-Item $diagnosticsPage (Join-Path $ReleaseDir "诊断说明.html") -Force
} else {
    throw "缺少诊断资源：$diagnosticsPage（安装包必须附带「诊断说明.html」）"
}

# =====** FFmpeg 运行库与许可（S7：默认解码器，仅 avcodec/avutil） =====**
$ffmpegInfo = "ffmpeg=off"
if ($withFfmpeg) {
    $copiedDlls = @()
    foreach ($pattern in @("avcodec-*.dll", "avutil-*.dll", "libwinpthread-1.dll")) {
        $dll = Get-ChildItem $ffmpegBin -Filter $pattern | Sort-Object Name -Descending | Select-Object -First 1
        if (-not $dll) { throw "FFmpeg 构建缺少 $pattern（$ffmpegBin）" }
        Copy-Item $dll.FullName $ReleaseDir -Force
        $copiedDlls += $dll
        Write-Host "[packaging/windows] FFmpeg 运行库：$($dll.Name) ($($dll.Length) B)"
    }

    # 断言一：包内 FFmpeg 相关 DLL 只能有 avcodec/avutil/libwinpthread 三个，
    # 且无 ffmpeg.exe/avformat/avfilter/swscale。
    $strayAv = Get-ChildItem $ReleaseDir -Filter "av*.dll" |
        Where-Object { $copiedDlls.Name -notcontains $_.Name }
    if ($strayAv) {
        throw "Release 目录出现多余的 FFmpeg DLL：$($strayAv.Name -join ', ')"
    }
    $forbidden = Get-ChildItem $ReleaseDir -Recurse |
        Where-Object {
            # 本脚本自己产出的许可/构建说明不算可执行组件。
            $_.Name -notin @("FFMPEG-BUILD-INFO.txt", "FFmpeg-LICENSE.md") -and (
                $_.Name -match "^(avformat|avfilter|avdevice|swscale|ffmpeg|ffprobe)" -or
                $_.Name -match "x264|x265")
        }
    if ($forbidden) {
        throw "Release 目录出现禁止分发的组件（GPL/x264/x265/完整 FFmpeg）：$($forbidden.Name -join ', ')"
    }

    # 断言二：自建配置本身必须关闭 GPL 与 x264/x265，且只保留 H.264 解码。
    $configRules = @(
        @{ File = "config.h"; Key = "CONFIG_GPL"; Expect = "0" },
        @{ File = "config.h"; Key = "CONFIG_LIBX264"; Expect = "0" },
        @{ File = "config.h"; Key = "CONFIG_LIBX265"; Expect = "0" },
        @{ File = "config_components.h"; Key = "CONFIG_H264_DECODER"; Expect = "1" },
        @{ File = "config_components.h"; Key = "CONFIG_H264_D3D11VA2_HWACCEL"; Expect = "1" }
    )
    foreach ($rule in $configRules) {
        $configPath = Join-Path $FfmpegSrcDir $rule.File
        if (-not (Test-Path $configPath)) {
            throw "找不到 FFmpeg 构建配置 $configPath，无法完成许可证断言（可用 -FfmpegSrcDir 指定源码目录）"
        }
        $hit = Select-String -Path $configPath -Pattern "^#define $($rule.Key) $($rule.Expect)$"
        if (-not $hit) {
            throw "FFmpeg 配置断言失败：$($rule.Key) 不等于 $($rule.Expect)（$configPath）"
        }
    }

    # 许可文本 + 构建信息（LGPL-2.1-or-later 动态链接，允许用户替换 DLL）。
    $licenseDir = Join-Path $PSScriptRoot "..\licenses"
    Copy-Item (Join-Path $licenseDir "COPYING.LGPLv2.1") $ReleaseDir -Force
    Copy-Item (Join-Path $licenseDir "FFmpeg-LICENSE.md") $ReleaseDir -Force
    Copy-Item (Join-Path $licenseDir "libwinpthread-COPYING.txt") $ReleaseDir -Force
    $dllHashes = foreach ($dll in $copiedDlls) {
        "$((Get-FileHash $dll.FullName -Algorithm SHA256).Hash.ToLower())  $($dll.Name)  $($dll.Length) B"
    }
    $buildInfo = @(
        "ffmpeg_tag=n7.1",
        "configure=--disable-everything --enable-decoder=h264 --enable-parser=h264 --enable-protocol=file --disable-gpl --disable-libx264 --disable-libx265 --enable-d3d11va --enable-hwaccel=h264_d3d11va --enable-hwaccel=h264_d3d11va2 --enable-shared --disable-static --disable-programs --disable-doc --disable-network --disable-autodetect",
        "linking=dynamic (LGPL-2.1-or-later); DLL 未修改，用户可按 LGPL 2.1 第 6 节替换为兼容版本",
        "source=https://git.ffmpeg.org/ffmpeg.git (tag n7.1)",
        "dlls=",
        $dllHashes
    )
    Set-Content -Path (Join-Path $ReleaseDir "FFMPEG-BUILD-INFO.txt") -Value $buildInfo -Encoding UTF8
    $ffmpegInfo = "ffmpeg=on (dlls=$($copiedDlls.Name -join ','))"
    Write-Host "[packaging/windows] $ffmpegInfo"
} else {
    # 无 LGPL 构建时如实断言「不含 FFmpeg」，避免与构建开关不一致。
    $ffmpegDlls = Get-ChildItem $ReleaseDir -Filter "av*.dll" -ErrorAction SilentlyContinue
    if ($ffmpegDlls) {
        throw "Release 目录出现 FFmpeg DLL：$($ffmpegDlls.Name -join ', ')；本次未启用 ffmpeg feature"
    }
}

# 构建信息：产物必须可追溯到 commit 与版本（docs/08 §5）。
$commit = (git -C $RepoRoot rev-parse --short HEAD 2>$null)
if (-not $commit) { $commit = "unknown" }
Set-Content -Path (Join-Path $ReleaseDir "BUILD_INFO.txt") -Value "version=$Version`ncommit=$commit`n$ffmpegInfo"

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
