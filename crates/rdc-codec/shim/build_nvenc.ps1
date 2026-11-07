# Build NVENC shim (rdc-nvenc.dll).
# Requires MSYS2 MinGW64 g++ only: Phase 1 不依赖 NVENC SDK 头文件。
# 系统驱动 DLL（nvEncodeAPI64.dll / nvcuda.dll）由运行期 LoadLibrary 加载，禁止随包分发。
param(
    [string]$Gxx = "C:\msys64\mingw64\bin\g++.exe",
    [string]$OutDir = "$PSScriptRoot\..\..\..\target\nvenc"
)

$ErrorActionPreference = "Stop"
if (!(Test-Path $Gxx)) { throw "g++ not found: $Gxx" }

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$env:PATH = (Split-Path $Gxx -Parent) + ";" + $env:PATH
$src = Join-Path $PSScriptRoot "nvenc_enc.cpp"
$obj = "$OutDir\nvenc_enc.o"
$dll = "$OutDir\rdc-nvenc.dll"

Write-Host "[nvenc] outdir=[$OutDir] obj=[$obj]"
Write-Host "[nvenc] compiling rdc-nvenc.dll"
& $Gxx -std=c++17 -O2 -c $src -o $obj
if ($LASTEXITCODE -ne 0) { throw "compile nvenc_enc.cpp failed" }
# 静态链接 MinGW 运行时，避免宿主缺 libstdc++-6.dll / libgcc_s_seh-1.dll 时加载失败。
& $Gxx -shared -o $dll $obj "-static" "-Wl,--export-all-symbols"
if ($LASTEXITCODE -ne 0) { throw "link rdc-nvenc.dll failed" }

Get-ChildItem $OutDir -Include *.dll -File | Select-Object Name, Length | Format-Table
