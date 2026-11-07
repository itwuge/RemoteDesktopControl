# Build AMF shim (rdc-amf.dll) and spike (amf_spike.exe).
# Requires MSYS2 MinGW64 g++ and AMF SDK headers (default C:\dev\amf-include).
# AMF runtime (amfrt64.dll) is provided by the installed driver; the shim only LoadLibrary's it.
param(
    [string]$AmfInclude = "C:\dev\amf-include",
    [string]$Gxx = "C:\msys64\mingw64\bin\g++.exe",
    [string]$OutDir = "$PSScriptRoot\..\..\..\target\amf"
)

$ErrorActionPreference = "Stop"
if (!(Test-Path $Gxx)) { throw "g++ not found: $Gxx" }
if (!(Test-Path (Join-Path $AmfInclude "core\Factory.h"))) { throw "AMF headers not found: $AmfInclude" }

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$env:PATH = (Split-Path $Gxx -Parent) + ";" + $env:PATH
$src = Join-Path $PSScriptRoot "amf_enc.cpp"
$spike = Join-Path $PSScriptRoot "amf_spike.cpp"
$inc = "-I" + $AmfInclude
$libs = @("-ld3d11", "-ldxgi", "-lole32", "-luuid")
# 静态链接 MinGW 运行时：发布包里不带 libstdc++-6.dll / libgcc_s_seh-1.dll，
# 也避免宿主缺少这两个 DLL 时 LoadLibrary 直接失败（S7-3 实测踩过）。
# 注意：`-static-libgcc -static-libstdc++` 对 `-shared` 不生效（仍留下导入表），
# 这里直接用 `-static`，shared 目标仍可正常产出。
$obj = "$OutDir\amf_enc.o"
$dll = "$OutDir\rdc-amf.dll"
$exe = "$OutDir\amf_spike.exe"

Write-Host "[amf] outdir=[$OutDir] obj=[$obj]"
Write-Host "[amf] compiling rdc-amf.dll"
& $Gxx -std=c++17 -O2 $inc -c $src -o $obj
if ($LASTEXITCODE -ne 0) { throw "compile amf_enc.cpp failed" }
& $Gxx -shared -o $dll $obj "-static" "-Wl,--export-all-symbols" @libs
if ($LASTEXITCODE -ne 0) { throw "link rdc-amf.dll failed" }

Write-Host "[amf] compiling amf_spike.exe"
& $Gxx -std=c++17 -O2 $inc $src $spike -o $exe "-static" @libs
if ($LASTEXITCODE -ne 0) { throw "compile amf_spike.exe failed" }

Get-ChildItem $OutDir -Include *.dll,*.exe -File | Select-Object Name, Length | Format-Table
