# ============================================================
# 脚本用途：生成并签名自更新清单（update-manifest.json + .sig）
#
# 层级：packaging（发布流水线与本地演练共用）
# 调用方：.github/workflows/release-windows.yml；本地自更新演练脚本
#
# 安全口径：
# - 清单覆盖安装包的 SHA256，客户端下载后会交叉校验 `.sha256` 资产；
# - 签名私钥只从环境变量 RDC_RELEASE_SEED_B64 读取，禁止写入仓库；
# - 签名前核对“签名公钥 = 客户端内置公钥”，避免发出去没人能验签的包。
# ============================================================
param(
    [Parameter(Mandatory = $true)][string]$InstallerPath,
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Url = "",
    [string]$OutputDir = "",
    [string]$Notes = "",
    [string]$MinSupportedVersion = "",
    [string]$UpdaterPath = ""
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $RepoRoot "dist"
}
if (-not (Test-Path $InstallerPath)) {
    throw "找不到安装包：$InstallerPath"
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

# =====** 收集安装包元数据 =====**
$installer = Get-Item $InstallerPath
$sha256 = (Get-FileHash $installer.FullName -Algorithm SHA256).Hash.ToLower()
Write-Host "[manifest] 安装包 $($installer.Name) $($installer.Length) B sha256=$sha256"

# =====** 写清单 =====**
# 字段顺序固定，且只写一次：签名覆盖的就是这份字节，之后不得再格式化。
$manifest = [ordered]@{
    product               = "RDC"
    version               = $Version
    installer             = [ordered]@{
        name   = $installer.Name
        sha256 = $sha256
        size   = [int64]$installer.Length
        url    = $Url
    }
    notes                 = $Notes
    min_supported_version = $MinSupportedVersion
}
$manifestPath = Join-Path $OutputDir "update-manifest.json"
$json = $manifest | ConvertTo-Json -Depth 5
# 用 UTF8 无 BOM 写入：BOM 会让客户端解析失败，也会让签名与文件字节不一致。
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($manifestPath, $json + "`n", $utf8)
Write-Host "[manifest] 已写入 $manifestPath"

# =====** 签名（复用 rdc-updater sign） =====**
if ([string]::IsNullOrWhiteSpace($UpdaterPath)) {
    $targetDir = if ([string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
        Join-Path $RepoRoot "target"
    } else {
        $env:CARGO_TARGET_DIR
    }
    $UpdaterPath = Join-Path $targetDir "release\rdc-updater.exe"
}
if (-not (Test-Path $UpdaterPath)) {
    throw "找不到 rdc-updater：$UpdaterPath（请先执行 cargo build --release -p rdc-update）"
}
if ([string]::IsNullOrWhiteSpace($env:RDC_RELEASE_SEED_B64)) {
    throw "缺少环境变量 RDC_RELEASE_SEED_B64：无法签名发布清单（私钥只能放在 CI Secret）"
}

$signOutput = & $UpdaterPath sign --file $manifestPath 2>&1 | Out-String
Write-Host "[manifest] $signOutput"
if ($LASTEXITCODE -ne 0) {
    throw "清单签名失败（退出码 $LASTEXITCODE）"
}
$signResult = $signOutput | ConvertFrom-Json

# =====** 公钥一致性断言 =====**
# 客户端只信任内置公钥；签名公钥不一致说明用错了密钥，发出去也装不上。
$libPath = Join-Path $RepoRoot "crates\rdc-update\src\lib.rs"
$libSource = Get-Content $libPath -Raw
$keyMatch = [regex]::Match($libSource, 'RELEASE_PUBLIC_KEY_B64[^=]*=\s*"([^"]+)"')
if (-not $keyMatch.Success) {
    throw "无法从 $libPath 读取 RELEASE_PUBLIC_KEY_B64"
}
if ($keyMatch.Groups[1].Value -ne $signResult.public_key_b64) {
    throw "签名公钥与客户端内置公钥不一致：签名=$($signResult.public_key_b64) 内置=$($keyMatch.Groups[1].Value)"
}

# 清单自身的 sha256，便于人工核对 Release 资产未被替换。
$manifestHash = (Get-FileHash $manifestPath -Algorithm SHA256).Hash.ToLower()
"$manifestHash  update-manifest.json" | Set-Content -Path "$manifestPath.sha256" -Encoding ASCII

Write-Host "[manifest] 完成：$manifestPath"
Write-Host "[manifest] 签名：$($signResult.signature_path)"
