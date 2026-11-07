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
$obj = Join-Path $OutDir "amf_enc.o"

Write-Host "[amf] compiling rdc-amf.dll"
& $Gxx -std=c++17 -O2 $inc -c $src -o $obj
if ($LASTEXITCODE -ne 0) { throw "compile amf_enc.cpp failed" }
& $Gxx -shared -o (Join-Path $OutDir "rdc-amf.dll") $obj "-Wl,--export-all-symbols" @libs
if ($LASTEXITCODE -ne 0) { throw "link rdc-amf.dll failed" }

Write-Host "[amf] compiling amf_spike.exe"
& $Gxx -std=c++17 -O2 $inc $src $spike -o (Join-Path $OutDir "amf_spike.exe") @libs
if ($LASTEXITCODE -ne 0) { throw "compile amf_spike.exe failed" }

Get-ChildItem $OutDir -Include *.dll,*.exe -File | Select-Object Name, Length | Format-Table
