# ============================================================
# Purpose : Portable engineering gate (R-C / R-N / R-L / R-T / R-D)
# Layer   : repository / CI
# Callers : local pre-commit, CI gate job, any new project copied from tools/gate-template/
#
# Design:
#   1) The script contains no project paths, crate names, module names or whitelists.
#      Everything project-specific comes from an external gate.config.json.
#   2) Without a config file it runs with neutral defaults and prints its assumptions.
#   3) Rule IDs are configurable via ruleIds. The source of truth is the cross-project
#      "iron rules" document; remap IDs here without touching logic.
#   4) Static checks only. -Fix currently only adds UTF-8 BOM.
#
# Exit codes:
#   0 = no violation
#   1 = violation found
#   2 = script error / self-test failure
# ============================================================

[CmdletBinding()]
param(
    [string]$Root = "",
    # Alias keeps the public -Config switch while avoiding a name collision with script:Config.
    [Alias("Config")]
    [string]$ConfigFile = "",
    [string[]]$Paths = @(),
    [switch]$Changed,
    [string]$BaseRef = "master",
    [switch]$Fix,
    [switch]$SelfTest,
    [switch]$SkipGlobal,
    [switch]$SkipVersionBump,
    # 全量运行时写出基线快照（只允许持平或下降，不能借更新基线掩盖新增违规）。
    [switch]$UpdateBaseline,
    # 跳过“基线只许降不许升”检查（仅用于本地统计，不用于 CI 主门禁）。
    [switch]$SkipBaseline,
    # 覆盖配置里的 baseline.file。
    [string]$BaselineFile = "",
    [switch]$Build,
    [int]$MinFreeGB = -1,
    [string]$TargetDir = "",
    [string]$Rules = "",
    [switch]$Quiet
)

# =====** global state =====**

$script:Root = ""
$script:RootFull = ""
$script:ConfigPath = ""
$script:Config = $null
$script:Violations = @()
$script:Exemptions = 0
$script:FixedCount = 0
$script:SelfTestMode = $false
$script:SelfTestTargetDir = ""
$script:SelfTestBaseVersion = ""
$script:SelfTestChangedFiles = @()
$script:SelfTestBuild = $false
$script:FileCache = @{}
$script:GlobCache = @{}
$script:AllFilesCache = $null
$script:ChangedSet = $null
$script:SelectedRuleIds = @()
$script:ParseErrors = @()

# =====** default config =====**

function New-DefaultConfig {
    # Neutral defaults: no project-specific information.
    return @{
        configVersion = 1
        project = @{ name = "project" }
        # semantic key -> rule ID. IDs must follow the cross-project iron rules document.
        ruleIds = @{
            swallowed_error       = "R-C-2"
            error_context         = "R-C-3"
            unbounded_queue       = "R-C-5"
            version_consistency   = "R-C-14"
            build_location        = "R-C-15"
            source_encoding       = "R-C-1"
            note_header           = "R-N-1"
            note_doc              = "R-N-2"
            note_section          = "R-N-4"
            note_section_control  = "R-N-5"
            note_section_sync     = "R-N-7"
            log_print_bypass      = "R-L-1"
            log_integration       = "R-L-4"
            log_only              = "R-L-9"
            team_conflict         = "R-T-2"
            team_change_record    = "R-T-4"
            team_shared_file      = "R-T-8"
            command_version_bump  = "R-D-9"
            command_smoke_record  = "R-D-9"
            command_milestone     = "R-T-5"
            # 基线增长不是五域内的独立规则，但必须可被门禁输出为一条可检索的“违规”。
            baseline_growth       = "GATE-B"
        }
        rules = @{
            swallowed_error      = @{ enabled = $true }
            error_context        = @{ enabled = $true }
            unbounded_queue      = @{ enabled = $true }
            version_consistency  = @{ enabled = $true }
            build_location       = @{ enabled = $true }
            source_encoding      = @{ enabled = $true }
            note_header          = @{ enabled = $true }
            note_doc             = @{ enabled = $true }
            note_section         = @{ enabled = $true }
            note_section_control = @{ enabled = $true }
            note_section_sync    = @{ enabled = $true; manual = $true }
            log_print_bypass     = @{ enabled = $true }
            log_integration      = @{ enabled = $true }
            log_only             = @{ enabled = $true }
            team_conflict        = @{ enabled = $true }
            team_change_record   = @{ enabled = $true }
            team_shared_file     = @{ enabled = $true; manual = $true }
            command_version_bump = @{ enabled = $true }
            command_smoke_record = @{ enabled = $true }
            command_milestone    = @{ enabled = $true; manual = $true }
        }
        scan = @{
            include = @("**/*.rs", "**/*.dart", "**/*.cpp", "**/*.h", "**/*.hpp", "**/*.cc", "**/*.c")
            exclude = @(
                "**/target/**", "**/build/**", "**/.dart_tool/**", "**/node_modules/**",
                "**/dist/**", "**/.git/**", "**/vendor/**", "**/third_party/**",
                "**/generated/**", "**/*.g.rs", "**/frb_generated.rs"
            )
            maxFileBytes = 4194304
        }
        rust = @{
            cratesDir = "crates"
            sourceGlobs = @("crates/*/src/**/*.rs")
            streamingGlobs = @("crates/*/src/**/*.rs")
            pathSensitiveGlobs = @("**/platform/**/*.rs", "**/ffi/**/*.rs", "**/bridge/**/*.rs")
            pureCrates = @()
            requiredLogDeps = @("tracing")
        }
        encoding = @{
            # Non-ASCII sources in these extensions must carry a UTF-8 BOM.
            extensions = @(".cpp", ".h", ".hpp", ".cc", ".c")
            roots = @(".")
            exclude = @(
                "**/target/**", "**/build/**", "**/.git/**", "**/dist/**",
                "**/node_modules/**", "**/.dart_tool/**", "**/third_party/**",
                "**/vendor/**", "**/generated/**"
            )
        }
        note = @{
            # Header must appear in the first N lines. Keep this regex neutral.
            headerRegex = "^(//|///|//!|/*)\s*(=====|工作流用途|文件用途|模块用途|用途|File|Module)"
            headerScanLines = 25
            sourceGlobs = @("**/*.rs", "**/*.dart", "**/*.cpp", "**/*.h", "**/*.hpp", "**/*.c", "**/*.cc")
            requireDocForPublic = $true
            requireParamSections = $false
            paramSectionRegex = "#\s*(Arguments|Parameters|Args|参数)"
            componentMarkerRegex = "^\s*(//|#)\s*=====+\*\*"
            controlMarkerRegex = "^\s*(//|#)\s*========+\*\*"
            uiGlobs = @("**/view/**", "**/views/**", "**/widget/**", "**/widgets/**", "**/page/**", "**/pages/**", "**/component/**", "**/components/**")
            controlGlobs = @("**/controller/**", "**/controllers/**", "**/model/**", "**/models/**")
            minLinesForSection = 200
            minLinesForControlMarker = 400
            placeholderNames = @("组件名", "Component", "TODO")
        }
        team = @{
            conflictMarkerRegex = "^(<<<<<<< |=======$|>>>>>>> )"
            sourceGlobs = @("crates/*/src/**/*.rs", "**/*.dart", "**/*.cpp", "**/*.h")
            # Empty means "not configured"; the rule then reports not-applicable instead of failing.
            changeRecordGlobs = @()
        }
        command = @{
            # Smoke records are checked only in -Build mode (packaging/pre-release).
            smokeRecordGlobs = @()
            requireSmokeOnBuild = $true
        }
        baseline = @{
            # -1 = 基线策略未配置：新项目默认不启用，只报告违规总数。
            allowedCount = -1
            file = "logs/gate-baseline.json"
        }
        version = @{
            cargoManifest = "Cargo.toml"
            pubspec = ""
            nsiGlobs = @("**/*.nsi")
            buildInfoGlobs = @("**/BUILD_INFO.txt")
            distDir = "dist"
            artifactExtensions = @(".exe", ".msi", ".zip", ".apk", ".aab", ".dmg", ".deb", ".rpm")
            versionBumpExcludeGlobs = @("docs/**", "**/*.md", "scripts/**", "tools/**", ".github/**")
        }
        build = @{
            targetDirEnv = "CARGO_TARGET_DIR"
            defaultTargetDir = "target"
            minFreeGB = 10
            checkSystemDrive = $true
        }
        logging = @{
            allowedPrintGlobs = @("**/examples/**", "**/benches/**", "**/tests/**", "**/build.rs")
        }
        whitelist = @()
    }
}

# =====** helpers =====**

function Write-Info {
    param([string]$Message)
    if (-not $Quiet) { Write-Host ("[gate] " + $Message) -ForegroundColor DarkGray }
}

function Write-WarnLine {
    param([string]$Message)
    Write-Host ("[gate] " + $Message) -ForegroundColor Yellow
}

function Fail-Script {
    param([string]$Message)
    Write-Host ("[gate] ERROR: " + $Message) -ForegroundColor Red
    exit 2
}

function ConvertTo-Hashtable {
    param($InputObject)
    if ($null -eq $InputObject) { return $null }
    if ($InputObject -is [System.Collections.IEnumerable] -and -not ($InputObject -is [string]) -and -not ($InputObject -is [hashtable])) {
        $list = @()
        foreach ($item in $InputObject) { $list += ,(ConvertTo-Hashtable $item) }
        return $list
    }
    if ($InputObject -is [System.Management.Automation.PSCustomObject]) {
        $hash = @{}
        foreach ($prop in $InputObject.PSObject.Properties) {
            $hash[$prop.Name] = ConvertTo-Hashtable $prop.Value
        }
        return $hash
    }
    return $InputObject
}

function Merge-Config {
    param($Base, $Override)
    if ($null -eq $Override) { return $Base }
    if (-not ($Base -is [hashtable]) -or -not ($Override -is [hashtable])) { return $Override }
    foreach ($key in $Override.Keys) {
        if ($Base.ContainsKey($key)) {
            $Base[$key] = Merge-Config $Base[$key] $Override[$key]
        } else {
            $Base[$key] = $Override[$key]
        }
    }
    return $Base
}

function Get-RuleId {
    param([string]$SemanticKey)
    if ($script:Config.ruleIds.ContainsKey($SemanticKey)) {
        return [string]$script:Config.ruleIds[$SemanticKey]
    }
    return $SemanticKey
}

function Test-RuleEnabled {
    param([string]$SemanticKey)
    $ruleId = Get-RuleId $SemanticKey
    if ($script:SelectedRuleIds.Count -gt 0) {
        if (-not ($script:SelectedRuleIds -contains $ruleId)) { return $false }
    }
    if ($script:Config.rules.ContainsKey($SemanticKey)) {
        $entry = $script:Config.rules[$SemanticKey]
        if ($entry["enabled"] -ne $true) { return $false }
        if ($entry.ContainsKey("manual") -and $entry["manual"] -eq $true) { return $false }
    }
    return $true
}

function Test-RuleManual {
    param([string]$SemanticKey)
    if ($script:Config.rules.ContainsKey($SemanticKey)) {
        $entry = $script:Config.rules[$SemanticKey]
        if ($entry.ContainsKey("manual") -and $entry["manual"] -eq $true) { return $true }
    }
    return $false
}

function ConvertTo-GlobRegex {
    param([string]$Glob)
    if ($script:GlobCache.ContainsKey($Glob)) { return $script:GlobCache[$Glob] }
    $g = $Glob.Replace("\", "/")
    $out = ""
    $i = 0
    while ($i -lt $g.Length) {
        $ch = $g[$i]
        if ($ch -eq "*") {
            if (($i + 1) -lt $g.Length -and $g[$i + 1] -eq "*") {
                if (($i + 2) -lt $g.Length -and $g[$i + 2] -eq "/") {
                    $out += "(?:.*/)?"
                    $i += 3
                    continue
                }
                $out += ".*"
                $i += 2
                continue
            }
            $out += "[^/]*"
            $i++
            continue
        }
        if ($ch -eq "?") {
            $out += "[^/]"
            $i++
            continue
        }
        if ("\^$.|+()[]{}".IndexOf($ch) -ge 0) {
            $out += "\" + $ch
        } else {
            $out += $ch
        }
        $i++
    }
    $regex = "(?i)^" + $out + "$"
    $script:GlobCache[$Glob] = $regex
    return $regex
}

function Test-Glob {
    param([string]$RelativePath, [string]$Glob)
    return ($RelativePath -match (ConvertTo-GlobRegex $Glob))
}

function Test-AnyGlob {
    param([string]$RelativePath, [string[]]$Globs)
    foreach ($glob in $Globs) {
        if (Test-Glob $RelativePath $glob) { return $true }
    }
    return $false
}

function Get-RelativePath {
    param([string]$FullPath)
    $full = [System.IO.Path]::GetFullPath($FullPath)
    $root = [System.IO.Path]::GetFullPath($script:RootFull)
    if ($full.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase)) {
        $rel = $full.Substring($root.Length)
        $rel = $rel.TrimStart('\', '/')
        return $rel.Replace("\", "/")
    }
    return $full.Replace("\", "/")
}

function Read-SourceFile {
    param([string]$FullPath)
    $key = [System.IO.Path]::GetFullPath($FullPath)
    if ($script:FileCache.ContainsKey($key)) { return $script:FileCache[$key] }
    $bytes = [System.IO.File]::ReadAllBytes($key)
    $hasBom = $false
    if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
        $hasBom = $true
    }
    $text = [System.Text.Encoding]::UTF8.GetString($bytes)
    if ($hasBom -and $text.Length -gt 0) { $text = $text.Substring(1) }
    $lines = @($text -split "`r?`n")
    $entry = @{ Bytes = $bytes; HasBom = $hasBom; Text = $text; Lines = $lines }
    $script:FileCache[$key] = $entry
    return $entry
}

function Get-AllFiles {
    # 全量枚举只做一次：规则数量多，重复递归会让全量扫描变成分钟级。
    if ($null -ne $script:AllFilesCache) { return $script:AllFilesCache }
    $files = @()
    $maxBytes = [int]$script:Config.scan.maxFileBytes
    foreach ($file in (Get-ChildItem -Path $script:RootFull -Recurse -File -ErrorAction SilentlyContinue)) {
        if ($file.Length -gt $maxBytes) { continue }
        $rel = Get-RelativePath $file.FullName
        if (Test-AnyGlob $rel $script:Config.scan.exclude) { continue }
        $files += $file.FullName
    }
    $script:AllFilesCache = $files
    return $files
}

function Test-ScanInclude {
    param([string]$RelativePath)
    return (Test-AnyGlob $RelativePath $script:Config.scan.include)
}

function Get-SourceFilesByGlob {
    param([string[]]$Globs)
    $result = @()
    foreach ($full in (Get-AllFiles)) {
        $rel = Get-RelativePath $full
        if (-not (Test-ScanInclude $rel)) { continue }
        if ($script:ChangedSet -ne $null -and -not $script:ChangedSet.ContainsKey($rel)) { continue }
        if (Test-AnyGlob $rel $Globs) { $result += $full }
    }
    return $result
}

function Test-InlineAllow {
    param([string]$RuleId, [string]$LineText)
    if ($LineText -match ("(?i)gate-allow\s+" + [regex]::Escape($RuleId) + "\s*:\s*(.+)")) {
        return $Matches[1].Trim()
    }
    return $null
}

function Test-Whitelisted {
    param([string]$RuleId, [string]$RelativePath, [string]$LineText)
    $inline = Test-InlineAllow $RuleId $LineText
    if ($null -ne $inline) {
        $script:Exemptions++
        return $inline
    }
    foreach ($entry in $script:Config.whitelist) {
        $entryRule = [string]$entry["rule"]
        if ($entryRule -ne $RuleId) { continue }
        $entryGlob = [string]$entry["glob"]
        if ($entryGlob -ne "" -and -not (Test-Glob $RelativePath $entryGlob)) { continue }
        $entryPattern = [string]$entry["pattern"]
        if ($entryPattern -ne "" -and $LineText -notmatch $entryPattern) { continue }
        $script:Exemptions++
        return [string]$entry["reason"]
    }
    return $null
}

function Add-Violation {
    param([string]$RuleId, [string]$RelativePath, [int]$LineNumber, [string]$Message)
    # Messages may start with the rule ID (older call sites); avoid printing it twice.
    $prefix = "^" + [regex]::Escape($RuleId) + "\s+"
    $Message = [regex]::Replace($Message, $prefix, "")
    $script:Violations += [pscustomobject]@{ Rule = $RuleId; File = $RelativePath; Line = $LineNumber; Message = $Message }
}

# =====** scope =====**

function Get-ChangedSet {
    $set = @{}
    $git = Get-Command git -ErrorAction SilentlyContinue
    if ($null -eq $git) { Fail-Script "-Changed requires git, but git was not found in PATH." }
    $outputs = @()
    # core.quotepath=false：否则 git 会把非 ASCII 路径写成 "\345\217..." 八进制转义，glob 匹配不到。
    $outputs += (& git -C $script:RootFull -c core.quotepath=false diff --name-only ("{0}...HEAD" -f $BaseRef) 2>$null)
    $outputs += (& git -C $script:RootFull -c core.quotepath=false diff --name-only 2>$null)
    $outputs += (& git -C $script:RootFull -c core.quotepath=false diff --cached --name-only 2>$null)
    $outputs += (& git -C $script:RootFull -c core.quotepath=false ls-files --others --exclude-standard 2>$null)
    foreach ($line in $outputs) {
        if ($null -eq $line) { continue }
        $line = ([string]$line).Trim()
        if ($line -eq "") { continue }
        $set[($line.Replace("\", "/"))] = $true
    }
    return $set
}

function Initialize-Scope {
    if ($Changed) { $script:ChangedSet = Get-ChangedSet }
    if ($Paths.Count -gt 0) {
        $resolved = @()
        foreach ($p in $Paths) {
            $candidate = $p
            if (-not [System.IO.Path]::IsPathRooted($candidate)) { $candidate = Join-Path $script:RootFull $candidate }
            if (-not (Test-Path -LiteralPath $candidate)) { Write-WarnLine ("path not found: " + $p); continue }
            $item = Get-Item -LiteralPath $candidate
            if ($item.PSIsContainer) {
                $resolved += (Get-ChildItem -LiteralPath $candidate -Recurse -File -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName })
            } else {
                $resolved += $item.FullName
            }
        }
        $script:ChangedSet = @{}
        foreach ($full in $resolved) { $script:ChangedSet[(Get-RelativePath $full)] = $true }
    }
}

# =====** R-C-2 swallowed error =====**

function Invoke-Rule-SwallowedError {
    if (-not (Test-RuleEnabled "swallowed_error")) { return }
    $ruleId = Get-RuleId "swallowed_error"
    foreach ($full in (Get-SourceFilesByGlob $script:Config.rust.sourceGlobs)) {
        $rel = Get-RelativePath $full
        $entry = Read-SourceFile $full
        $isStreaming = Test-AnyGlob $rel $script:Config.rust.streamingGlobs
        $lineNo = 0
        foreach ($line in $entry.Lines) {
            $lineNo++
            $trim = $line.Trim()
            if ($trim.StartsWith("//") -or $trim.StartsWith("/*")) { continue }
            $reason = Test-Whitelisted $ruleId $rel $line
            if ($null -ne $reason) { continue }
            if ($trim -match '^\s*let\s+_\s*=') {
                Add-Violation $ruleId $rel $lineNo "R-C-2 discarded result with 'let _ ='; log or propagate the error."
                continue
            }
            if ($trim -match '\.ok\(\)\s*;') {
                Add-Violation $ruleId $rel $lineNo "R-C-2 '.ok();' drops the Err branch; use match/propagate."
                continue
            }
            if ($isStreaming -and $trim -match '\.unwrap_or_default\(\)') {
                Add-Violation $ruleId $rel $lineNo "R-C-2 unwrap_or_default() in IO/codec/network path hides failure."
                continue
            }
        }
    }
}

# =====** R-C-3 error context =====**

function Invoke-Rule-ErrorContext {
    if (-not (Test-RuleEnabled "error_context")) { return }
    $ruleId = Get-RuleId "error_context"
    foreach ($full in (Get-SourceFilesByGlob $script:Config.rust.sourceGlobs)) {
        $rel = Get-RelativePath $full
        $entry = Read-SourceFile $full
        $isPathSensitive = Test-AnyGlob $rel $script:Config.rust.pathSensitiveGlobs
        $lineNo = 0
        foreach ($line in $entry.Lines) {
            $lineNo++
            $trim = $line.Trim()
            if ($trim.StartsWith("//") -or $trim.StartsWith("/*")) { continue }
            $reason = Test-Whitelisted $ruleId $rel $line
            if ($null -ne $reason) { continue }
            if ($line -match '\banyhow!\s*\(') {
                Add-Violation $ruleId $rel $lineNo "R-C-3 anyhow!() without source erases the error chain."
                continue
            }
            if ($line -match 'map_err\(\s*\|[^|]*_\s*\|') {
                Add-Violation $ruleId $rel $lineNo "R-C-3 map_err(|_| ...) discards the original error."
                continue
            }
            if ($isPathSensitive) {
                if ($line -match 'format!\(\s*"\{\}"\s*,\s*[A-Za-z_]') {
                    Add-Violation $ruleId $rel $lineNo "R-C-3 platform error formatted as text; keep numeric code and system message."
                    continue
                }
                if ($line -match 'format!\(\s*"\{[A-Za-z_][A-Za-z0-9_]*\}"\s*\)') {
                    Add-Violation $ruleId $rel $lineNo "R-C-3 platform error formatted as text; keep numeric code and system message."
                    continue
                }
                if ($line -match '\b(err|error|e)\s*\.to_string\(\)') {
                    Add-Violation $ruleId $rel $lineNo "R-C-3 error.to_string() loses platform code/source."
                    continue
                }
            }
        }
    }
}

# =====** R-C-5 unbounded queue =====**

function Invoke-Rule-UnboundedQueue {
    if (-not (Test-RuleEnabled "unbounded_queue")) { return }
    $ruleId = Get-RuleId "unbounded_queue"
    foreach ($full in (Get-SourceFilesByGlob $script:Config.rust.sourceGlobs)) {
        $rel = Get-RelativePath $full
        $entry = Read-SourceFile $full
        $lineNo = 0
        foreach ($line in $entry.Lines) {
            $lineNo++
            $trim = $line.Trim()
            if ($trim.StartsWith("//") -or $trim.StartsWith("/*")) { continue }
            $reason = Test-Whitelisted $ruleId $rel $line
            if ($null -ne $reason) { continue }
            if ($line -match '\bunbounded(_channel)?\b') {
                Add-Violation $ruleId $rel $lineNo "R-C-5 unbounded channel/queue has no capacity limit."
                continue
            }
            if ($line -match '(?i)(queue|pending|backlog|reasm|jitter|history|ring)[A-Za-z0-9_]*\s*:\s*VecDeque::new\(\)') {
                Add-Violation $ruleId $rel $lineNo "R-C-5 streaming VecDeque field without capacity limit."
                continue
            }
            if ($line -match '(?i)(queue|pending|backlog|reasm|jitter|history|ring)[A-Za-z0-9_]*\s*:\s*VecDeque\s*<') {
                # 允许的显式上限声明：同行写 `// capacity: 32` 或使用 `with_capacity(N)`。
                if ($line -match '(?i)(capacity|容量)\s*[:(=]?\s*\d+') { continue }
                Add-Violation $ruleId $rel $lineNo "R-C-5 streaming VecDeque field has no declared capacity."
                continue
            }
            if ($line -match '(?i)let\s+(queue|pending|backlog|reasm|jitter|history|ring)[A-Za-z0-9_]*\s*=\s*VecDeque::new\(\)') {
                Add-Violation $ruleId $rel $lineNo "R-C-5 streaming VecDeque local without capacity limit."
                continue
            }
            if ($line -match '(?i)(queue|pending|backlog|buffer)[A-Za-z0-9_]*\s*:\s*Vec::new\(\)') {
                Add-Violation $ruleId $rel $lineNo "R-C-5 streaming Vec field without capacity limit."
                continue
            }
        }
    }
}

# =====** R-C-14 version consistency =====**

function Get-CargoVersion {
    param([string]$ManifestPath)
    if (-not (Test-Path -LiteralPath $ManifestPath)) { return $null }
    $text = [System.IO.File]::ReadAllText($ManifestPath, [System.Text.Encoding]::UTF8)
    $m = [regex]::Match($text, '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"')
    if ($m.Success) { return $m.Groups[1].Value }
    $m = [regex]::Match($text, '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"')
    if ($m.Success) { return $m.Groups[1].Value }
    return $null
}

function Normalize-SemVer {
    param([string]$Version)
    if ($null -eq $Version) { return $null }
    $m = [regex]::Match($Version, '(\d+\.\d+\.\d+)')
    if ($m.Success) { return $m.Groups[1].Value }
    return $Version.Trim()
}

function Add-VersionMismatch {
    param([string]$RuleId, [string]$File, [int]$Line, [string]$Found, [string]$Expected, [string]$Source)
    Add-Violation $RuleId $File $Line ("R-C-14 " + $Source + " version " + $Found + " != reference " + $Expected)
}

function Invoke-Rule-VersionConsistency {
    if (-not (Test-RuleEnabled "version_consistency")) { return }
    # 跨文件一致性是仓库级规则；-Changed/-SkipGlobal 下交给全量报告任务。
    if ($SkipGlobal) { return }
    $ruleId = Get-RuleId "version_consistency"
    $cargoPath = Join-Path $script:RootFull $script:Config.version.cargoManifest
    $reference = Get-CargoVersion $cargoPath

    if ($script:Config.version.pubspec -ne "") {
        $pubspecPath = Join-Path $script:RootFull $script:Config.version.pubspec
        if (Test-Path -LiteralPath $pubspecPath) {
            $text = [System.IO.File]::ReadAllText($pubspecPath, [System.Text.Encoding]::UTF8)
            $m = [regex]::Match($text, '(?m)^version:\s*(\d+\.\d+\.\d+)')
            if ($m.Success) {
                $found = Normalize-SemVer $m.Groups[1].Value
                if ($null -ne $reference -and $found -ne (Normalize-SemVer $reference)) {
                    Add-VersionMismatch $ruleId $script:Config.version.pubspec 0 $found $reference "pubspec.yaml"
                } elseif ($null -eq $reference) {
                    $reference = $found
                }
            }
        }
    }

    foreach ($full in (Get-AllFiles)) {
        $rel = Get-RelativePath $full
        if (-not (Test-AnyGlob $rel $script:Config.version.nsiGlobs)) { continue }
        $entry = Read-SourceFile $full
        $lineNo = 0
        foreach ($line in $entry.Lines) {
            $lineNo++
            $m = [regex]::Match($line, '!define\s+VERSION\s+"([^"]+)"')
            if ($m.Success) {
                $found = Normalize-SemVer $m.Groups[1].Value
                if ($null -eq $reference) { $reference = $found }
                elseif ($found -ne (Normalize-SemVer $reference)) { Add-VersionMismatch $ruleId $rel $lineNo $found $reference "NSIS fallback" }
            }
        }
    }

    foreach ($full in (Get-AllFiles)) {
        $rel = Get-RelativePath $full
        if (-not (Test-AnyGlob $rel $script:Config.version.buildInfoGlobs)) { continue }
        $entry = Read-SourceFile $full
        $lineNo = 0
        foreach ($line in $entry.Lines) {
            $lineNo++
            $m = [regex]::Match($line, '(?i)version\s*[=:]\s*(\d+\.\d+\.\d+)')
            if ($m.Success) {
                $found = Normalize-SemVer $m.Groups[1].Value
                if ($null -eq $reference) { $reference = $found }
                elseif ($found -ne (Normalize-SemVer $reference)) { Add-VersionMismatch $ruleId $rel $lineNo $found $reference "BUILD_INFO" }
            }
        }
    }

    # 安装包文件名一致性只在出包阶段有意义：dist/ 是本机未跟踪的构建产物，
    # 普通扫描/CI 报告任务看不到它，把历史残留产物计入基线只会制造噪声。
    if (($Build -or $script:SelfTestMode) -and $script:Config.version.distDir -ne "") {
        $distPath = Join-Path $script:RootFull $script:Config.version.distDir
        if (Test-Path -LiteralPath $distPath) {
            foreach ($file in (Get-ChildItem -LiteralPath $distPath -File -ErrorAction SilentlyContinue)) {
                $ext = [System.IO.Path]::GetExtension($file.Name).ToLowerInvariant()
                if (-not ($script:Config.version.artifactExtensions -contains $ext)) { continue }
                $m = [regex]::Match($file.Name, '(\d+\.\d+\.\d+)')
                if (-not $m.Success) { continue }
                $found = Normalize-SemVer $m.Groups[1].Value
                if ($null -eq $reference) { $reference = $found }
                elseif ($found -ne (Normalize-SemVer $reference)) {
                    Add-VersionMismatch $ruleId (Get-RelativePath $file.FullName) 0 $found $reference "artifact filename"
                }
            }
        }
    }

    if ($null -eq $reference) { Write-Info "R-C-14: no version source found; skipped." }
}

# =====** R-C-15 build location =====**

function Invoke-Rule-BuildLocation {
    if (-not (Test-RuleEnabled "build_location")) { return }
    # 构建落盘反映本机环境而非本次改动；-Changed/-SkipGlobal 下交给全量报告任务。
    if ($SkipGlobal -and -not $Build) { return }
    $ruleId = Get-RuleId "build_location"
    $target = $TargetDir
    if ($script:SelfTestMode -and $script:SelfTestTargetDir -ne "") { $target = $script:SelfTestTargetDir }
    if ($target -eq "") {
        $envName = [string]$script:Config.build.targetDirEnv
        if ($envName -ne "" -and (Test-Path Env:$envName)) { $target = [string](Get-Item Env:$envName).Value }
    }
    if ($target -eq "") { $target = Join-Path $script:RootFull ([string]$script:Config.build.defaultTargetDir) }
    if (-not [System.IO.Path]::IsPathRooted($target)) { $target = Join-Path $script:RootFull $target }
    $targetFull = [System.IO.Path]::GetFullPath($target)
    $targetRoot = [System.IO.Path]::GetPathRoot($targetFull)
    $systemRoot = [System.IO.Path]::GetPathRoot($script:RootFull)
    if ($null -ne $env:SystemDrive -and $env:SystemDrive -ne "") { $systemRoot = $env:SystemDrive + "\" }

    if ($script:Config.build.checkSystemDrive -ne $false -and $targetRoot -ne "" -and $targetRoot.ToLowerInvariant() -eq $systemRoot.ToLowerInvariant()) {
        Add-Violation $ruleId ($targetFull.Replace("\", "/")) 0 ("R-C-15 build target is on system drive " + $systemRoot + "; set " + $script:Config.build.targetDirEnv + " to another drive.")
    }

    if ($Build -or $script:SelfTestBuild) {
        $minGb = $MinFreeGB
        if ($minGb -lt 0) { $minGb = [int]$script:Config.build.minFreeGB }
        $driveName = $targetRoot.TrimEnd('\', '/')
        if ($driveName -ne "" -and $driveName.Length -eq 2 -and $driveName.EndsWith(":")) {
            $drive = Get-PSDrive -Name $driveName.Substring(0, 1) -ErrorAction SilentlyContinue
            if ($null -ne $drive) {
                $freeGb = [math]::Round($drive.Free / 1073741824, 2)
                if ($freeGb -lt $minGb) {
                    Add-Violation $ruleId ($targetFull.Replace("\", "/")) 0 ("R-C-15 free space " + $freeGb + " GB is below threshold " + $minGb + " GB.")
                } else {
                    Write-Info ("R-C-15 free space on " + $driveName + ": " + $freeGb + " GB (threshold " + $minGb + " GB).")
                }
            }
        }
    }
}

# =====** R-C-1 source encoding BOM =====**

function Fix-BomFile {
    param([string]$FullPath)
    $entry = Read-SourceFile $FullPath
    if ($entry.HasBom) { return $false }
    $utf8Bom = New-Object System.Text.UTF8Encoding($true)
    [System.IO.File]::WriteAllBytes($FullPath, $utf8Bom.GetPreamble() + $entry.Bytes)
    $key = [System.IO.Path]::GetFullPath($FullPath)
    if ($script:FileCache.ContainsKey($key)) { $script:FileCache.Remove($key) }
    return $true
}

function Invoke-Rule-SourceEncoding {
    if (-not (Test-RuleEnabled "source_encoding")) { return }
    $ruleId = Get-RuleId "source_encoding"
    $extensions = $script:Config.encoding.extensions
    $roots = $script:Config.encoding.roots
    foreach ($rootEntry in $roots) {
        $rootPath = $rootEntry
        if (-not [System.IO.Path]::IsPathRooted($rootPath)) { $rootPath = Join-Path $script:RootFull $rootPath }
        if (-not (Test-Path -LiteralPath $rootPath)) { continue }
        $files = @()
        if ((Get-Item -LiteralPath $rootPath).PSIsContainer) {
            $files = @(Get-ChildItem -LiteralPath $rootPath -Recurse -File -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName })
        } else {
            $files = @($rootPath)
        }
        foreach ($full in $files) {
            $ext = [System.IO.Path]::GetExtension($full).ToLowerInvariant()
            if (-not ($extensions -contains $ext)) { continue }
            $rel = Get-RelativePath $full
            if (Test-AnyGlob $rel $script:Config.encoding.exclude) { continue }
            if ($script:ChangedSet -ne $null -and -not $script:ChangedSet.ContainsKey($rel)) { continue }
            $entry = Read-SourceFile $full
            if ($entry.HasBom) { continue }
            $nonAsciiLine = 0
            $lineNo = 0
            foreach ($line in $entry.Lines) {
                $lineNo++
                if ($line -match '[^\x00-\x7F]') { $nonAsciiLine = $lineNo; break }
            }
            if ($nonAsciiLine -eq 0) { continue }
            if ($Fix) {
                if (Fix-BomFile $full) {
                    $script:FixedCount++
                    Write-Info ("R-C-1 fixed UTF-8 BOM: " + $rel)
                    continue
                }
            } else {
                Add-Violation $ruleId $rel $nonAsciiLine "R-C-1 non-ASCII source without UTF-8 BOM; run with -Fix."
            }
        }
    }
}

# =====** R-N-1 file header =====**

function Invoke-Rule-NoteHeader {
    if (-not (Test-RuleEnabled "note_header")) { return }
    $ruleId = Get-RuleId "note_header"
    $regex = [string]$script:Config.note.headerRegex
    $scanLines = [int]$script:Config.note.headerScanLines
    foreach ($full in (Get-SourceFilesByGlob $script:Config.note.sourceGlobs)) {
        $rel = Get-RelativePath $full
        $entry = Read-SourceFile $full
        $limit = [Math]::Min($scanLines, $entry.Lines.Count)
        $found = $false
        for ($i = 0; $i -lt $limit; $i++) {
            if ($entry.Lines[$i] -match $regex) { $found = $true; break }
        }
        if (-not $found) {
            Add-Violation $ruleId $rel 1 "R-N-1 missing file header comment block (first $scanLines lines)."
        }
    }
}

# =====** R-N-2 public function docs =====**

function Get-DocBlock {
    # Return contiguous /// lines immediately above index i (skip attributes/blank lines).
    param([string[]]$Lines, [int]$Index)
    $j = $Index - 1
    while ($j -ge 0) {
        $trim = $Lines[$j].Trim()
        if ($trim -eq "" -or $trim.StartsWith("#[")) { $j--; continue }
        break
    }
    $docs = @()
    while ($j -ge 0 -and $Lines[$j].Trim().StartsWith("///")) {
        $docs = @($Lines[$j].Trim()) + $docs
        $j--
    }
    return $docs
}

function Invoke-Rule-NoteDoc {
    if (-not (Test-RuleEnabled "note_doc")) { return }
    $ruleId = Get-RuleId "note_doc"
    $requireParam = $script:Config.note.requireParamSections -eq $true
    $paramRegex = [string]$script:Config.note.paramSectionRegex
    foreach ($full in (Get-SourceFilesByGlob $script:Config.note.sourceGlobs)) {
        if (-not ($full.EndsWith(".rs"))) { continue }
        $rel = Get-RelativePath $full
        $entry = Read-SourceFile $full
        $lineNo = 0
        foreach ($line in $entry.Lines) {
            $lineNo++
            $index = $lineNo - 1
            if ($line -notmatch '^\s*pub\s+(async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)') { continue }
            $fnName = $Matches[2]
            $trim = $line.Trim()
            if ($trim.StartsWith("//")) { continue }
            $docs = Get-DocBlock $entry.Lines $index
            if ($docs.Count -eq 0) {
                Add-Violation $ruleId $rel $lineNo ("R-N-2 public fn " + $fnName + " has no doc comment.")
                continue
            }
            if ($requireParam -and $line -match '\(') {
                $argsText = $line
                if ($argsText -match '\(\s*\)') { continue }
                $joined = ($docs -join " ")
                if ($joined -notmatch $paramRegex) {
                    Add-Violation $ruleId $rel $lineNo ("R-N-2 public fn " + $fnName + " lacks Arguments/Parameters section.")
                }
            }
        }
    }
}

# =====** R-N-4 section markers =====**

function Invoke-Rule-NoteSection {
    $uiEnabled = Test-RuleEnabled "note_section"
    $controlEnabled = Test-RuleEnabled "note_section_control"
    if (-not ($uiEnabled -or $controlEnabled)) { return }
    $ruleId = Get-RuleId "note_section"
    $controlRuleId = Get-RuleId "note_section_control"
    $componentRegex = [string]$script:Config.note.componentMarkerRegex
    $controlRegex = [string]$script:Config.note.controlMarkerRegex
    $minSection = [int]$script:Config.note.minLinesForSection
    $minControl = [int]$script:Config.note.minLinesForControlMarker
    $placeholders = $script:Config.note.placeholderNames
    foreach ($full in (Get-SourceFilesByGlob $script:Config.note.sourceGlobs)) {
        $rel = Get-RelativePath $full
        $entry = Read-SourceFile $full
        $lineCount = $entry.Lines.Count
        $hasComponent = $false
        $hasControl = $false
        $lineNo = 0
        foreach ($line in $entry.Lines) {
            $lineNo++
            if ($line -match $componentRegex) {
                $hasComponent = $true
                if ($uiEnabled) {
                    foreach ($name in $placeholders) {
                        if ($line -match ([regex]::Escape([string]$name))) {
                            Add-Violation $ruleId $rel $lineNo ("R-N-4 section marker still uses placeholder name '" + $name + "'.")
                        }
                    }
                }
            }
            if ($line -match $controlRegex) { $hasControl = $true }
        }
        $uiInScope = (Test-AnyGlob $rel $script:Config.note.uiGlobs)
        $controlInScope = (Test-AnyGlob $rel $script:Config.note.controlGlobs)
        if ($uiEnabled -and $uiInScope -and $lineCount -ge $minSection -and -not $hasComponent) {
            Add-Violation $ruleId $rel 1 ("R-N-4 file has " + $lineCount + " lines but no UI component section marker.")
        }
        if ($controlEnabled -and $controlInScope -and $lineCount -ge $minControl -and -not $hasControl) {
            Add-Violation $controlRuleId $rel 1 ("R-N-5 file has " + $lineCount + " lines but no controller/model section marker.")
        }
    }
}

# =====** R-L-1 print bypass =====**

function Invoke-Rule-LogPrintBypass {
    if (-not (Test-RuleEnabled "log_print_bypass")) { return }
    $ruleId = Get-RuleId "log_print_bypass"
    $allowed = $script:Config.logging.allowedPrintGlobs
    foreach ($full in (Get-AllFiles)) {
        $rel = Get-RelativePath $full
        if (-not (Test-ScanInclude $rel)) { continue }
        if ($script:ChangedSet -ne $null -and -not $script:ChangedSet.ContainsKey($rel)) { continue }
        if (Test-AnyGlob $rel $allowed) { continue }
        $ext = [System.IO.Path]::GetExtension($full).ToLowerInvariant()
        if (-not ($ext -in @(".rs", ".dart", ".cpp", ".h", ".hpp", ".c", ".cc"))) { continue }
        $entry = Read-SourceFile $full
        $lineNo = 0
        foreach ($line in $entry.Lines) {
            $lineNo++
            $trim = $line.Trim()
            if ($trim.StartsWith("//") -or $trim.StartsWith("/*")) { continue }
            $reason = Test-Whitelisted $ruleId $rel $line
            if ($null -ne $reason) { continue }
            $detail = ""
            if ($ext -eq ".rs") {
                if ($line -match '(?<![\w.])println!\s*\(') { $detail = "println!" }
                elseif ($line -match '(?<![\w.])eprintln!\s*\(') { $detail = "eprintln!" }
                elseif ($line -match '(?<![\w.])print!\s*\(') { $detail = "print!" }
                elseif ($line -match '(?<![\w.])eprint!\s*\(') { $detail = "eprint!" }
            } elseif ($ext -eq ".dart") {
                if ($line -match '(?<![\w.])debugPrintStack\s*\(') { $detail = "debugPrintStack" }
                elseif ($line -match '(?<![\w.])debugPrint\s*\(') { $detail = "debugPrint" }
                elseif ($line -match '(?<![\w.])print\s*\(') { $detail = "print" }
            } else {
                if ($line -match 'std::cout') { $detail = "std::cout" }
                elseif ($line -match 'std::cerr') { $detail = "std::cerr" }
                elseif ($line -match '(?<![\w.])printf\s*\(') { $detail = "printf" }
                elseif ($line -match '(?<![\w.])fprintf\s*\(') { $detail = "fprintf" }
                elseif ($line -match 'OutputDebugString') { $detail = "OutputDebugString" }
            }
            if ($detail -ne "") {
                Add-Violation $ruleId $rel $lineNo ("R-L-1 " + $detail + " bypasses the unified logging layer.")
            }
        }
    }
}

# =====** R-L-4 unified logging integration =====**

function Invoke-Rule-LogIntegration {
    if (-not (Test-RuleEnabled "log_integration")) { return }
    $ruleId = Get-RuleId "log_integration"
    $cratesDir = Join-Path $script:RootFull ([string]$script:Config.rust.cratesDir)
    if (-not (Test-Path -LiteralPath $cratesDir)) {
        Write-Info ("R-L-4: crates dir not found (" + $script:Config.rust.cratesDir + "); skipped.")
        return
    }
    $pure = $script:Config.rust.pureCrates
    $cratesRel = ([string]$script:Config.rust.cratesDir).Replace("\", "/").TrimEnd("/")
    foreach ($crateDir in (Get-ChildItem -LiteralPath $cratesDir -Directory -ErrorAction SilentlyContinue)) {
        $manifest = Join-Path $crateDir.FullName "Cargo.toml"
        if (-not (Test-Path -LiteralPath $manifest)) { continue }
        if ($pure -contains $crateDir.Name) { continue }
        # 改动模式下只检查“被本次改动触及的 crate”，避免存量 crate 让主门禁永远红。
        if ($script:ChangedSet -ne $null) {
            $crateRel = $cratesRel + "/" + $crateDir.Name + "/"
            $touched = $false
            foreach ($rel in $script:ChangedSet.Keys) {
                if ($rel.StartsWith($crateRel, [System.StringComparison]::OrdinalIgnoreCase)) { $touched = $true; break }
            }
            if (-not $touched) { continue }
        }
        $entry = Read-SourceFile $manifest
        $inDeps = $false
        $hasLogDep = $false
        foreach ($line in $entry.Lines) {
            $trim = $line.Trim()
            if ($trim -match '^\[([^\]]+)\]$') {
                $section = $Matches[1]
                if ($section -eq "dependencies" -or $section -like "target.*dependencies" -or $section -eq "workspace.dependencies") {
                    $inDeps = $true
                } else {
                    $inDeps = $false
                }
                continue
            }
            if (-not $inDeps) { continue }
            foreach ($dep in $script:Config.rust.requiredLogDeps) {
                if ($line -match ('^\s*' + [regex]::Escape([string]$dep) + '\s*=')) { $hasLogDep = $true }
            }
        }
        if (-not $hasLogDep) {
            Add-Violation $ruleId (Get-RelativePath $manifest) 0 ("R-L-4 crate " + $crateDir.Name + " has no unified logging dependency.")
        }
    }
}

# =====** R-L-9 log only, no error return =====**

function Test-LogOnlyBlock {
    param([string[]]$Lines, [int]$StartIndex, [string]$FirstLine)
    $joined = ""
    $depth = 0
    $opened = $false
    $limit = [Math]::Min($Lines.Count, $StartIndex + 20)
    for ($k = $StartIndex; $k -lt $limit; $k++) {
        $line = $Lines[$k]
        $joined += $line + "`n"
        $openCount = ([regex]::Matches($line, '\{')).Count
        $closeCount = ([regex]::Matches($line, '\}')).Count
        $depth += $openCount - $closeCount
        if ($openCount -gt 0) { $opened = $true }
        if ($opened -and $depth -le 0) { break }
    }
    $rest = $joined
    if ($rest.Length -ge $FirstLine.Length) { $rest = $rest.Substring($FirstLine.Length) }
    $hasLog = ($joined -match '(warn!|error!|log::warn|log::error|tracing::warn|tracing::error)')
    $hasReturn = ($rest -match '(\?\s*;|return\s+Err|bail!|Err\()')
    if ($hasLog -and -not $hasReturn) { return $true }
    return $false
}

function Invoke-Rule-LogOnly {
    if (-not (Test-RuleEnabled "log_only")) { return }
    $ruleId = Get-RuleId "log_only"
    foreach ($full in (Get-SourceFilesByGlob $script:Config.rust.sourceGlobs)) {
        $rel = Get-RelativePath $full
        $entry = Read-SourceFile $full
        $lineNo = 0
        foreach ($line in $entry.Lines) {
            $lineNo++
            $index = $lineNo - 1
            if ($line -notmatch 'if\s+let\s+Err\s*\(') { continue }
            $reason = Test-Whitelisted $ruleId $rel $line
            if ($null -ne $reason) { continue }
            if (Test-LogOnlyBlock $entry.Lines $index $line) {
                Add-Violation $ruleId $rel $lineNo "R-L-9 error branch only logs; it must return or propagate the error."
            }
        }
    }
}

# =====** R-T-2 merge conflict markers =====**

function Invoke-Rule-TeamConflict {
    if (-not (Test-RuleEnabled "team_conflict")) { return }
    $ruleId = Get-RuleId "team_conflict"
    $markerRegex = [string]$script:Config.team.conflictMarkerRegex
    foreach ($full in (Get-AllFiles)) {
        $rel = Get-RelativePath $full
        if (-not (Test-ScanInclude $rel)) { continue }
        if ($script:ChangedSet -ne $null -and -not $script:ChangedSet.ContainsKey($rel)) { continue }
        $entry = Read-SourceFile $full
        $lineNo = 0
        foreach ($line in $entry.Lines) {
            $lineNo++
            if ($line -match $markerRegex) {
                Add-Violation $ruleId $rel $lineNo "R-T-2 merge conflict marker found."
            }
        }
    }
}

# =====** R-T-4 change record =====**

function Invoke-Rule-TeamChangeRecord {
    if (-not (Test-RuleEnabled "team_change_record")) { return }
    # 这不是全局规则：它只判定“本次改动是否附带记录”，-SkipGlobal 不得跳过。
    $ruleId = Get-RuleId "team_change_record"
    $recordGlobs = $script:Config.team.changeRecordGlobs
    if ($recordGlobs.Count -eq 0) {
        Write-Info "R-T-4: changeRecordGlobs is empty; rule not configured (manual-only in this project)."
        return
    }
    $changed = $script:ChangedSet
    if ($null -eq $changed) {
        $git = Get-Command git -ErrorAction SilentlyContinue
        if ($null -eq $git) { Write-Info "R-T-4: git not found; skipped."; return }
        $changed = Get-ChangedSet
    }
    $sourceChanged = @()
    foreach ($rel in $changed.Keys) {
        if (Test-AnyGlob $rel $script:Config.team.sourceGlobs) { $sourceChanged += $rel }
    }
    if ($sourceChanged.Count -eq 0) { return }
    $hasRecord = $false
    foreach ($rel in $changed.Keys) {
        if (Test-AnyGlob $rel $recordGlobs) { $hasRecord = $true; break }
    }
    if (-not $hasRecord) {
        Add-Violation $ruleId $sourceChanged[0] 0 "R-T-4 source changed but no change-record file was updated."
    }
}

# =====** R-D-9 version increment =====**

function Invoke-Rule-CommandVersionBump {
    if (-not (Test-RuleEnabled "command_version_bump")) { return }
    if ($SkipVersionBump) { return }
    $ruleId = Get-RuleId "command_version_bump"
    $current = Normalize-SemVer (Get-CargoVersion (Join-Path $script:RootFull $script:Config.version.cargoManifest))
    if ($null -eq $current) { return }

    if ($script:SelfTestMode) {
        if ($script:SelfTestBaseVersion -eq "") { return }
        $baseVersion = Normalize-SemVer $script:SelfTestBaseVersion
        $changed = @{}
        foreach ($rel in $script:SelfTestChangedFiles) { $changed[$rel] = $true }
    } else {
        $git = Get-Command git -ErrorAction SilentlyContinue
        if ($null -eq $git) { Write-Info "R-D-9: git not found; skipped."; return }
        $inside = (& git -C $script:RootFull rev-parse --is-inside-work-tree 2>$null)
        if ($LASTEXITCODE -ne 0 -or ([string]$inside).Trim() -ne "true") { Write-Info "R-D-9: not a git repo; skipped."; return }
        $baseText = (& git -C $script:RootFull show ("{0}:{1}" -f $BaseRef, $script:Config.version.cargoManifest) 2>$null)
        if ($LASTEXITCODE -ne 0 -or $null -eq $baseText) { Write-Info ("R-D-9: cannot read " + $BaseRef + ":" + $script:Config.version.cargoManifest + "; skipped."); return }
        $text = ([string]$baseText -join "`n")
        $m = [regex]::Match($text, '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"')
        if (-not $m.Success) { $m = [regex]::Match($text, '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"') }
        if (-not $m.Success) { Write-Info "R-D-9: baseline version not found; skipped."; return }
        $baseVersion = Normalize-SemVer $m.Groups[1].Value
        $changed = Get-ChangedSet
    }

    # 只有“源码改动”才要求版本递增；改配置/文档/脚本不应触发误报。
    $sourceChanged = $false
    foreach ($rel in $changed.Keys) {
        if (Test-AnyGlob $rel $script:Config.version.versionBumpExcludeGlobs) { continue }
        if (Test-AnyGlob $rel $script:Config.team.sourceGlobs) { $sourceChanged = $true; break }
    }
    if (-not $sourceChanged) { return }
    if ($current -eq $baseVersion) {
        Add-Violation $ruleId $script:Config.version.cargoManifest 0 ("R-D-9 source changed but version stayed " + $current + " (base " + $BaseRef + "); bump the version.")
    }
}

# =====** R-D-9 smoke record =====**

function Invoke-Rule-CommandSmokeRecord {
    if (-not (Test-RuleEnabled "command_smoke_record")) { return }
    if (-not ($Build -or $script:SelfTestBuild)) { return }
    if ($script:Config.command.requireSmokeOnBuild -eq $false) { return }
    $ruleId = Get-RuleId "command_smoke_record"
    $globs = $script:Config.command.smokeRecordGlobs
    if ($globs.Count -eq 0) {
        Write-Info "R-D-9: smokeRecordGlobs is empty; rule not configured."
        return
    }
    $found = $false
    foreach ($full in (Get-AllFiles)) {
        $rel = Get-RelativePath $full
        if (Test-AnyGlob $rel $globs) { $found = $true; break }
    }
    if (-not $found) {
        Add-Violation $ruleId ($globs -join ",") 0 "R-D-9 no smoke-test record found for build."
    }
}

# =====** baseline (只许降不许升) =====**

function Get-BaselinePath {
    $candidate = $BaselineFile
    if ($candidate -eq "") { $candidate = [string]$script:Config.baseline.file }
    if ($candidate -eq "") { $candidate = "logs/gate-baseline.json" }
    if ([System.IO.Path]::IsPathRooted($candidate)) { return $candidate }
    return (Join-Path $script:RootFull $candidate)
}

function Read-BaselineSnapshot {
    $path = Get-BaselinePath
    $snapshot = @{ Path = $path; Total = -1; ByRule = @{}; ByFile = @{}; Exists = $false }
    if (-not (Test-Path -LiteralPath $path)) { return $snapshot }
    try {
        $raw = [System.IO.File]::ReadAllText($path, [System.Text.Encoding]::UTF8)
        $obj = ConvertTo-Hashtable (ConvertFrom-Json -InputObject $raw)
        $snapshot.Exists = $true
        if ($obj.ContainsKey("total")) { $snapshot.Total = [int]$obj["total"] }
        if ($obj.ContainsKey("byRule") -and $obj["byRule"] -is [hashtable]) {
            foreach ($key in $obj["byRule"].Keys) { $snapshot.ByRule[$key] = [int]$obj["byRule"][$key] }
        }
        if ($obj.ContainsKey("entries") -and $obj["entries"] -is [System.Collections.IEnumerable]) {
            foreach ($e in $obj["entries"]) {
                if (-not ($e -is [hashtable])) { continue }
                $key = [string]$e["rule"] + "|" + [string]$e["file"]
                $snapshot.ByFile[$key] = 1 + $snapshot.ByFile[$key]
            }
        }
    } catch {
        Write-WarnLine ("baseline: cannot parse " + $path + ": " + $_.Exception.Message)
    }
    return $snapshot
}

function Write-BaselineSnapshot {
    param([int]$Total, $ByRule)
    $path = Get-BaselinePath
    $dir = Split-Path -Parent $path
    if ($dir -ne "" -and -not (Test-Path -LiteralPath $dir)) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }
    $byRuleOrdered = [ordered]@{}
    foreach ($key in ($ByRule.Keys | Sort-Object)) { $byRuleOrdered[$key] = [int]$ByRule[$key] }
    # 逐条入库：-Changed 模式据此区分“存量”与“本次改动新引入”。
    $entries = @()
    foreach ($v in ($script:Violations | Sort-Object Rule, File, Line)) {
        $entries += [ordered]@{ rule = $v.Rule; file = $v.File; line = $v.Line; message = $v.Message }
    }
    $obj = [ordered]@{
        generatedAt = (Get-Date).ToString("yyyy-MM-ddTHH:mm:ssK")
        project = [string]$script:Config.project.name
        total = $Total
        byRule = $byRuleOrdered
        entries = $entries
    }
    $json = ($obj | ConvertTo-Json -Depth 5) + "`n"
    [System.IO.File]::WriteAllText($path, $json, (New-Object System.Text.UTF8Encoding($false)))
    return $path
}

function Invoke-ChangedBaselineFilter {
    # 只对“改动范围”生效：文件里已有的存量不算本次引入，但新增一条就红。
    if ($script:SelfTestMode) { return }
    if ($script:ChangedSet -eq $null) { return }
    if ($script:Config.baseline.allowedCount -lt 0) { return }
    $snapshot = Read-BaselineSnapshot
    if (-not $snapshot.Exists) {
        Write-WarnLine "baseline: snapshot missing; -Changed mode cannot separate new violations from existing debt."
        return
    }
    $allowance = @{}
    foreach ($key in $snapshot.ByFile.Keys) { $allowance[$key] = [int]$snapshot.ByFile[$key] }
    $kept = @()
    $suppressed = 0
    foreach ($v in $script:Violations) {
        $key = [string]$v.Rule + "|" + [string]$v.File
        $left = 0
        if ($allowance.ContainsKey($key)) { $left = [int]$allowance[$key] }
        if ($left -gt 0) {
            $allowance[$key] = $left - 1
            $suppressed++
            continue
        }
        $kept += $v
    }
    $script:Violations = $kept
    Write-Info ("baseline: -Changed mode suppressed " + $suppressed + " pre-existing violation(s); " + $kept.Count + " new.")
}

function Invoke-BaselineCheck {
    # 基线只对“全量扫描”有意义；-Changed/-Paths 只判定改动引入的新违规。
    if ($SkipBaseline) { Write-Info "baseline: skipped by -SkipBaseline."; return $true }
    if (-not $script:SelfTestMode -and $script:ChangedSet -ne $null) {
        Write-Info "baseline: skipped (only full scans compare against the baseline)."
        return $true
    }
    $allowed = [int]$script:Config.baseline.allowedCount
    $snapshot = Read-BaselineSnapshot
    $byRule = @{}
    foreach ($v in $script:Violations) { $byRule[$v.Rule] = 1 + $byRule[$v.Rule] }
    $total = $script:Violations.Count
    $ruleId = Get-RuleId "baseline_growth"

    if ($UpdateBaseline) {
        if ($allowed -ge 0 -and $total -gt $allowed) {
            Write-Host ("[gate] baseline: REFUSED. current=" + $total + " allowedCount=" + $allowed + "; fix violations instead of raising the baseline.") -ForegroundColor Red
            return $false
        }
        if ($snapshot.Exists -and $snapshot.Total -ge 0 -and $total -gt $snapshot.Total) {
            Write-Host ("[gate] baseline: REFUSED. current=" + $total + " > last snapshot=" + $snapshot.Total + " (" + $snapshot.Path + ").") -ForegroundColor Red
            return $false
        }
        $written = Write-BaselineSnapshot $total $byRule
        Write-Host ("[gate] baseline: written " + $written + " total=" + $total) -ForegroundColor Green
        if ($allowed -ne $total) {
            Write-Host ("[gate] baseline: set gate.config.json baseline.allowedCount = " + $total + " (equal or lower only).") -ForegroundColor Yellow
        }
        return $true
    }

    if ($allowed -lt 0) {
        Write-Info ("baseline: not configured; current violations=" + $total + ".")
        return $true
    }
    $problems = @()
    if ($total -gt $allowed) { $problems += ("total " + $total + " > allowedCount " + $allowed) }
    if ($snapshot.Exists -and $snapshot.Total -ge 0 -and $total -gt $snapshot.Total) {
        $problems += ("total " + $total + " > last snapshot " + $snapshot.Total + " (" + $snapshot.Path + ")")
    }
    if ($problems.Count -gt 0) {
        Add-Violation $ruleId $snapshot.Path 0 ("violation count grew: " + ($problems -join "; ") + "; existing debt may only decrease.")
        return $false
    }
    Write-Info ("baseline: " + $total + "/" + $allowed + " (no growth)")
    return $true
}

# =====** run all =====**

function Invoke-AllRules {
    Invoke-Rule-SwallowedError
    Invoke-Rule-ErrorContext
    Invoke-Rule-UnboundedQueue
    Invoke-Rule-VersionConsistency
    Invoke-Rule-BuildLocation
    Invoke-Rule-SourceEncoding
    Invoke-Rule-NoteHeader
    Invoke-Rule-NoteDoc
    Invoke-Rule-NoteSection
    Invoke-Rule-LogPrintBypass
    Invoke-Rule-LogIntegration
    Invoke-Rule-LogOnly
    Invoke-Rule-TeamConflict
    Invoke-Rule-TeamChangeRecord
    Invoke-Rule-CommandVersionBump
    Invoke-Rule-CommandSmokeRecord
}

function Write-Violations {
    $sorted = $script:Violations | Sort-Object Rule, File, Line
    foreach ($v in $sorted) {
        $location = $v.File
        if ($v.Line -gt 0) { $location = $v.File + ":" + $v.Line }
        Write-Host ($location + ": " + $v.Rule + " " + $v.Message) -ForegroundColor Red
    }
    $counts = @{}
    foreach ($v in $script:Violations) { $counts[$v.Rule] = 1 + $counts[$v.Rule] }
    $parts = @()
    foreach ($key in ($counts.Keys | Sort-Object)) { $parts += ($key + "=" + $counts[$key]) }
    $summary = "gate: " + $script:Violations.Count + " violations"
    if ($parts.Count -gt 0) { $summary += " (" + ($parts -join " ") + ")" }
    $summary += ", exempted " + $script:Exemptions
    if ($script:FixedCount -gt 0) { $summary += ", fixed " + $script:FixedCount }
    if ($script:Violations.Count -eq 0) {
        Write-Host $summary -ForegroundColor Green
    } else {
        Write-Host $summary -ForegroundColor Red
    }
    foreach ($err in $script:ParseErrors) { Write-WarnLine $err }
}

# =====** self-test =====**

function New-SelfTestFile {
    param([string]$Path, [string]$Content)
    $dir = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
    $encoding = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $Content, $encoding)
}

function Get-SelfTestFixtureName {
    param([string]$Semantic)
    switch ($Semantic) {
        "swallowed_error" { return "lib.rs: let _ / .ok()" }
        "error_context" { return "platform/windows.rs: anyhow!/map_err|_|/format" }
        "unbounded_queue" { return "lib.rs: VecDeque::new field" }
        "version_consistency" { return "Cargo.toml 0.1.0 vs setup.nsi 0.9.9" }
        "build_location" { return "TargetDir C:\\..." }
        "source_encoding" { return "bad.cpp: non-ASCII, no BOM" }
        "note_header" { return "lib.rs: no header in first 25 lines" }
        "note_doc" { return "lib.rs: pub fn without ///" }
        "note_section" { return "view/long_widget.dart: >200 lines, no component marker" }
        "note_section_control" { return "controller/control.dart: >400 lines, no controller/model marker" }
        "note_section_sync" { return "manual-only" }
        "log_print_bypass" { return "lib.rs println! + main.dart print" }
        "log_integration" { return "fake crate without tracing" }
        "log_only" { return "lib.rs if let Err + warn without return" }
        "team_conflict" { return "conflict.rs with <<<<<<<" }
        "team_change_record" { return "changed source + no record file" }
        "team_shared_file" { return "manual-only" }
        "command_version_bump" { return "Cargo 0.1.0, base 0.1.0, source changed" }
        "command_smoke_record" { return "logs/smoke-*.log absent" }
        "command_milestone" { return "manual-only" }
        "baseline_growth" { return "full scan count > baseline.allowedCount" }
        default { return "" }
    }
}

function Invoke-SelfTest {
    Write-Host "gate-selftest: building fixtures..." -ForegroundColor Cyan
    $temp = Join-Path $env:TEMP ("gate-selftest-" + [System.Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $temp -Force | Out-Null
    try {
        New-SelfTestFile (Join-Path $temp "Cargo.toml") @"
[workspace]
members = ["crates/fake"]

[workspace.package]
version = "0.1.0"
"@
        New-SelfTestFile (Join-Path $temp "crates/fake/Cargo.toml") @"
[package]
name = "fake"
version = "0.1.0"
edition = "2021"

[dependencies]
thiserror = "1"
"@
        New-SelfTestFile (Join-Path $temp "crates/fake/src/lib.rs") @"
use std::collections::VecDeque;

pub struct Queue {
    queue: VecDeque<u8>,
}

pub fn bad() {
    let _ = std::fs::read("x");
    std::fs::read("y").ok();
    println!("debug output");
    if let Err(e) = std::fs::read("z") {
        tracing::warn!("failed: {}", e);
    }
}
"@
        New-SelfTestFile (Join-Path $temp "crates/fake/src/platform/windows.rs") @"
use anyhow::anyhow;

pub fn bad(e: std::io::Error) -> String {
    let _ = anyhow!("boom");
    let _ = std::fs::read("x").map_err(|_| "no context");
    format!("{e}")
}
"@
        New-SelfTestFile (Join-Path $temp "packaging/windows/setup.nsi") '!define VERSION "0.9.9"'
        New-SelfTestFile (Join-Path $temp "app/windows/bad.cpp") "// 中文注释，缺少 UTF-8 BOM`nint main() { return 0; }`n"
        New-SelfTestFile (Join-Path $temp "app/lib/main.dart") "void main() {`n  print(\"debug output\");`n}`n"
        New-SelfTestFile (Join-Path $temp "crates/fake/src/conflict.rs") @"
// =====** conflict sample =====**
<<<<<<< HEAD
let a = 1;
=======
let a = 2;
>>>>>>> other
"@
        # 长文件用于 R-N-4：没有分区标记。
        $longLines = @()
        for ($i = 0; $i -lt 240; $i++) { $longLines += ("// filler line " + $i) }
        New-SelfTestFile (Join-Path $temp "app/lib/view/long_widget.dart") ($longLines -join "`n")

        # 更长的文件用于 R-N-5：超过 400 行且没有控制分区标记。
        $controlLines = @()
        for ($i = 0; $i -lt 420; $i++) { $controlLines += ("// filler line " + $i) }
        New-SelfTestFile (Join-Path $temp "app/lib/controller/control.dart") ($controlLines -join "`n")

        $script:Root = $temp
        $script:RootFull = [System.IO.Path]::GetFullPath($temp)
        $script:Config = New-DefaultConfig
        $script:Violations = @()
        $script:Exemptions = 0
        $script:FixedCount = 0
        $script:FileCache = @{}
        $script:GlobCache = @{}
        $script:ChangedSet = $null
        $script:AllFilesCache = $null
        $script:SelfTestMode = $true
        $script:SelfTestTargetDir = "C:\\gate-selftest-target"
        $script:SelectedRuleIds = @()
        $script:SelfTestChangedFiles = @("crates/fake/src/lib.rs")
        $script:SelfTestBaseVersion = "0.1.0"
        $script:SelfTestBuild = $true
        $script:Config.command.smokeRecordGlobs = @("logs/smoke-*.log")

        Write-Host "gate-selftest: default assumptions (no config file):" -ForegroundColor DarkGray
        Write-Host ("  cratesDir=" + $script:Config.rust.cratesDir) -ForegroundColor DarkGray
        Write-Host ("  sourceGlobs=" + ($script:Config.rust.sourceGlobs -join ",")) -ForegroundColor DarkGray
        Write-Host ("  note.headerRegex=" + $script:Config.note.headerRegex) -ForegroundColor DarkGray
        Write-Host ("  requiredLogDeps=" + ($script:Config.rust.requiredLogDeps -join "|")) -ForegroundColor DarkGray

        Invoke-AllRules

        # R-T-4 需要“已改动源码”上下文；单独补测一次。
        $script:ChangedSet = @{ "crates/fake/src/lib.rs" = $true }
        $script:Config.team.changeRecordGlobs = @("CHANGELOG.md")
        Invoke-Rule-TeamChangeRecord
        $script:ChangedSet = $null

        # GATE-B 自测：允许数比当前少 1，必须产生一条“基线增长”。
        $beforeBaseline = $script:Violations.Count
        $script:Config.baseline.allowedCount = [Math]::Max(0, $beforeBaseline - 1)
        $null = Invoke-BaselineCheck
        $script:Config.baseline.allowedCount = -1

        $implemented = @(
            "swallowed_error", "error_context", "unbounded_queue", "version_consistency", "build_location", "source_encoding",
            "note_header", "note_doc", "note_section", "note_section_control",
            "log_print_bypass", "log_integration", "log_only",
            "team_conflict", "team_change_record",
            "command_version_bump", "command_smoke_record",
            "baseline_growth"
        )
        $manual = @("note_section_sync", "team_shared_file", "command_milestone")
        $table = @()
        $missing = @()
        foreach ($semantic in $implemented) {
            $ruleId = Get-RuleId $semantic
            $hits = @($script:Violations | Where-Object { $_.Rule -eq $ruleId })
            $status = "MISS"
            if ($hits.Count -gt 0) { $status = "HIT" } else { $missing += $ruleId }
            $table += [pscustomobject]@{
                Semantic = $semantic; Rule = $ruleId; Implemented = "yes"
                Fixture = (Get-SelfTestFixtureName $semantic); Hits = $hits.Count; Status = $status
            }
        }
        foreach ($semantic in $manual) {
            $ruleId = Get-RuleId $semantic
            $table += [pscustomobject]@{
                Semantic = $semantic; Rule = $ruleId; Implemented = "manual"
                Fixture = (Get-SelfTestFixtureName $semantic); Hits = 0; Status = "n/a"
            }
        }

        # R-C-1 fix check: BOM should make the encoding rule pass for bad.cpp.
        $badCpp = Join-Path $temp "app/windows/bad.cpp"
        $fixed = Fix-BomFile $badCpp
        $entry = Read-SourceFile $badCpp
        $bomOk = ($fixed -and $entry.HasBom)
        $table += [pscustomobject]@{
            Semantic = "source_encoding(-Fix)"; Rule = (Get-RuleId "source_encoding"); Implemented = "yes"
            Fixture = "bad.cpp (Chinese, no BOM)"; Hits = $(if ($bomOk) { 1 } else { 0 }); Status = $(if ($bomOk) { "HIT" } else { "MISS" })
        }
        if (-not $bomOk) { $missing += ((Get-RuleId "source_encoding") + "-Fix") }

        Write-Host ""
        Write-Host "rule | implemented | fixture | hits | status" -ForegroundColor Cyan
        foreach ($row in $table) {
            $color = "Red"
            if ($row.Status -eq "HIT") { $color = "Green" }
            if ($row.Status -eq "n/a") { $color = "DarkGray" }
            Write-Host ("{0} | {1} | {2} | {3} | {4}" -f $row.Rule, $row.Implemented, $row.Fixture, $row.Hits, $row.Status) -ForegroundColor $color
        }

        if ($missing.Count -gt 0) {
            Write-Host ("gate-selftest: FAIL (missing: " + ($missing -join ", ") + ")") -ForegroundColor Red
            return 2
        }
        Write-Host "gate-selftest: PASS (all implemented rules produced at least one violation)" -ForegroundColor Green
        return 0
    } catch {
        Write-Host ("gate-selftest: ERROR " + $_.Exception.Message) -ForegroundColor Red
        return 2
    } finally {
        if (Test-Path -LiteralPath $temp) { Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue }
    }
}

# =====** main =====**

if ($Root -eq "") {
    if ($PSScriptRoot -ne "") { $Root = Split-Path -Parent $PSScriptRoot } else { $Root = (Get-Location).Path }
}
if (-not (Test-Path -LiteralPath $Root)) { Fail-Script ("root not found: " + $Root) }
$script:Root = (Resolve-Path -LiteralPath $Root).Path
$script:RootFull = [System.IO.Path]::GetFullPath($script:Root)

if ($Rules -ne "") {
    $script:SelectedRuleIds = @()
    foreach ($part in ($Rules -split ",")) {
        $id = $part.Trim()
        if ($id -ne "") { $script:SelectedRuleIds += $id.ToUpperInvariant() }
    }
}

if ($SelfTest) {
    $script:Config = New-DefaultConfig
    exit (Invoke-SelfTest)
}

$configCandidates = @()
if ($ConfigFile -ne "") {
    $configCandidates += $ConfigFile
} else {
    $configCandidates += (Join-Path $script:RootFull "gate.config.json")
    $configCandidates += (Join-Path $script:RootFull "tools/gate.config.json")
}
$loaded = $false
foreach ($candidate in $configCandidates) {
    if (Test-Path -LiteralPath $candidate) {
        try {
            $raw = [System.IO.File]::ReadAllText($candidate, [System.Text.Encoding]::UTF8)
            $userConfig = ConvertTo-Hashtable (ConvertFrom-Json -InputObject $raw)
            $script:Config = Merge-Config (New-DefaultConfig) $userConfig
            $script:ConfigPath = (Resolve-Path -LiteralPath $candidate).Path
            $loaded = $true
            break
        } catch {
            Fail-Script ("failed to parse config " + $candidate + ": " + $_.Exception.Message)
        }
    }
}
if (-not $loaded) {
    if ($ConfigFile -ne "") { Fail-Script ("config not found: " + $ConfigFile) }
    $script:Config = New-DefaultConfig
    Write-WarnLine "no gate.config.json found; using built-in defaults:"
    Write-WarnLine ("  scan.include=" + ($script:Config.scan.include -join ","))
    Write-WarnLine ("  rust.cratesDir=" + $script:Config.rust.cratesDir + " sourceGlobs=" + ($script:Config.rust.sourceGlobs -join ","))
    Write-WarnLine ("  logging requiredDeps=" + ($script:Config.rust.requiredLogDeps -join "|") + " allowedPrintGlobs=" + ($script:Config.logging.allowedPrintGlobs -join ","))
    Write-WarnLine ("  build.targetDirEnv=" + $script:Config.build.targetDirEnv + " minFreeGB=" + $script:Config.build.minFreeGB)
    Write-WarnLine ("  version.cargoManifest=" + $script:Config.version.cargoManifest + " pubspec=" + $script:Config.version.pubspec)
} else {
    Write-Info ("config: " + $script:ConfigPath)
}

Initialize-Scope
Invoke-AllRules
Invoke-ChangedBaselineFilter
$baselineOk = Invoke-BaselineCheck
Write-Violations

if (-not $baselineOk) { exit 1 }
# -UpdateBaseline 成功写出快照即视为成功，避免把“存量仍存在”误报成更新失败。
if ($UpdateBaseline) { exit 0 }
if ($script:Violations.Count -gt 0) { exit 1 }
exit 0
