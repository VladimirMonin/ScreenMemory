# S01-T04a: process-only, fixed offline verification steps. PowerShell 5.1 compatible.
$ErrorActionPreference = 'Stop'
$script:Utf8 = New-Object System.Text.UTF8Encoding($false, $true)
function Write-VerificationJson([string]$Path, $Value) {
    $text = (ConvertTo-Json -InputObject $Value -Depth 12 -Compress) -replace "`r`n|`r", "`n"
    [System.IO.File]::WriteAllText($Path, $text + "`n", (New-Object System.Text.UTF8Encoding($false)))
}
function Assert-Under([string]$Child, [string]$Parent) {
    $full = [System.IO.Path]::GetFullPath($Child)
    $base = [System.IO.Path]::GetFullPath($Parent).TrimEnd([char[]]@('\','/')) + [System.IO.Path]::DirectorySeparatorChar
    if (-not $full.StartsWith($base, [StringComparison]::OrdinalIgnoreCase)) { throw 'Output must be under ignored .pi root' }
    return $full
}
function Quote-WindowsArgument([string]$Argument) {
    if ($Argument -notmatch '[\s"]' -and $Argument.Length -gt 0) { return $Argument }
    $result = '"'; $slashes = 0
    foreach ($c in $Argument.ToCharArray()) {
        if ($c -eq '\') { $slashes++; continue }
        if ($c -eq '"') { $result += ('\' * ($slashes * 2 + 1)) + '"'; $slashes = 0; continue }
        $result += ('\' * $slashes) + $c; $slashes = 0
    }
    return $result + ('\' * ($slashes * 2)) + '"'
}
function Invoke-VerificationProcess([string]$Executable, [string[]]$Arguments, [string]$WorkingDirectory,
    [string]$LogPath, [int]$TimeoutSeconds, [hashtable]$EnvironmentMap) {
    if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
        return @{ exit = $null; reason = 'MISSING_EXECUTABLE'; duration_ms = 0 }
    }
    $p = New-Object System.Diagnostics.Process
    $p.StartInfo.FileName = $Executable
    $p.StartInfo.Arguments = (($Arguments | ForEach-Object { Quote-WindowsArgument $_ }) -join ' ')
    $p.StartInfo.WorkingDirectory = $WorkingDirectory
    $p.StartInfo.UseShellExecute = $false
    $p.StartInfo.CreateNoWindow = $true
    $p.StartInfo.RedirectStandardOutput = $true
    $p.StartInfo.RedirectStandardError = $true
    $p.StartInfo.StandardOutputEncoding = [System.Text.Encoding]::UTF8
    $p.StartInfo.StandardErrorEncoding = [System.Text.Encoding]::UTF8
    foreach ($key in $EnvironmentMap.Keys) { $p.StartInfo.EnvironmentVariables[$key] = [string]$EnvironmentMap[$key] }
    $stdout = [System.IO.File]::Create($LogPath)
    $stderr = [System.IO.File]::Create($LogPath + '.stderr')
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        try { [void]$p.Start() } catch { return @{ exit = $null; reason = 'SPAWN_ERROR'; duration_ms = $watch.ElapsedMilliseconds } }
        $outTask = $p.StandardOutput.BaseStream.CopyToAsync($stdout)
        $errTask = $p.StandardError.BaseStream.CopyToAsync($stderr)
        if (-not $p.WaitForExit($TimeoutSeconds * 1000)) {
            # taskkill /T closes grandchildren of cmd/compiler too; never target another owner.
            $killer = Join-Path $env:SystemRoot 'System32\taskkill.exe'
            if (Test-Path $killer) { & $killer /PID $p.Id /T /F 2>&1 | Out-Null }
            try { if (-not $p.HasExited) { $p.Kill() } } catch { }
            if (-not $p.WaitForExit(5000)) { throw 'Child did not terminate after timeout' }
            return @{ exit = [int]$p.ExitCode; reason = 'TIMEOUT'; duration_ms = $watch.ElapsedMilliseconds }
        }
        [void][System.Threading.Tasks.Task]::WaitAll(@($outTask,$errTask), 5000)
        return @{ exit = [int]$p.ExitCode; reason = $(if ($p.ExitCode -eq 0) { 'OK' } else { 'NATIVE_EXIT' }); duration_ms = $watch.ElapsedMilliseconds }
    } finally {
        $watch.Stop(); $p.Dispose(); $stdout.Dispose(); $stderr.Dispose()
    }
}
function New-VerificationState([string]$Root, [string]$Output, [string]$Suite) {
    $privateRoot = Join-Path $Root '.pi'
    if ((Get-Item -LiteralPath $privateRoot -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint) { throw 'Private output root must not be a link' }
    $private = Assert-Under $Output $privateRoot
    if (Test-Path -LiteralPath $private) { throw 'Output directory must not exist (no overwrite)' }
    $parent = Split-Path $private -Parent
    while ($parent.Length -gt $privateRoot.Length) {
        if ((Test-Path -LiteralPath $parent) -and ((Get-Item -LiteralPath $parent -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint)) { throw 'Output ancestor must not be a link' }
        $parent = Split-Path $parent -Parent
    }
    if (-not (Test-Path -LiteralPath $privateRoot -PathType Container)) { throw 'Private output root missing' }
    [void](New-Item -ItemType Directory -Path $private)
    return @{ root = $Root; output = $private; suite = $Suite; steps = (New-Object System.Collections.ArrayList); versions = @{}; failed = $false }
}
function Get-SafeVerificationToken($State, [string]$Value) {
    if ($Value -eq $null) { return '' }
    $root = [System.IO.Path]::GetFullPath($State.root).TrimEnd([char[]]@('\','/'))
    if ($Value -match '(?i)[\\/]node_modules[\\/]pnpm[\\/]bin[\\/]pnpm\.cjs$') { return '@PNPM_CLI' }
    if ($Value -match '^[A-Za-z]:[\\/]' -or $Value.StartsWith('\\')) {
        $full = [System.IO.Path]::GetFullPath($Value)
        $prefix = $root + [System.IO.Path]::DirectorySeparatorChar
        if ($full.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
            $relative = $full.Substring($prefix.Length).Replace('\','/')
            if ($relative -eq '.pi' -or $relative.StartsWith('.pi/', [StringComparison]::OrdinalIgnoreCase)) { return '@PRIVATE' }
            return $relative
        }
        return '@PRIVATE'
    }
    return $Value
}
function Get-VerificationCommand($State, [string]$Executable) {
    $name = [System.IO.Path]::GetFileName($Executable).ToLowerInvariant()
    switch ($name) {
        'cargo.exe' { return 'cargo' }
        'rustc.exe' { return 'rustc' }
        'node.exe' { return 'node' }
        'python.exe' { return 'python' }
        'powershell.exe' { return 'powershell.exe' }
        default { return (Get-SafeVerificationToken $State $Executable) }
    }
}
function Add-VerificationStep($State, [string]$Id, [string]$Executable, [string[]]$Arguments,
    [hashtable]$EnvironmentMap, [int]$TimeoutSeconds = 600, [string]$Prerequisite = '') {
    if ($State.failed) { return }
    $now = [DateTime]::UtcNow
    $index = $State.steps.Count + 1
    $logName = ('native-{0:d2}.log' -f $index)
    $result = $null
    if ($Prerequisite) { $result = @{ exit = $null; reason = 'MISSING_PREREQUISITE'; duration_ms = 0 } }
    else { $result = Invoke-VerificationProcess $Executable $Arguments $State.root (Join-Path $State.output $logName) $TimeoutSeconds $EnvironmentMap }
    $end = [DateTime]::UtcNow
    $ok = $result.reason -eq 'OK'
    $artifact = $null
    $log = Join-Path $State.output $logName
    if (Test-Path $log) { $artifact = @(@($log,($log + '.stderr')) | ForEach-Object { @{ id = [System.IO.Path]::GetFileName($_); sha256 = (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToLowerInvariant() } }) }
    [void]$State.steps.Add([ordered]@{ check_id = $Id; command = (Get-VerificationCommand $State $Executable);
        args = @($Arguments | ForEach-Object { Get-SafeVerificationToken $State $_ }); cwd = 'REPO';
        started_at_utc = $now.ToString('o'); ended_at_utc = $end.ToString('o'); duration_ms = $result.duration_ms;
        versions = $State.versions; native_exit = $result.exit; outcome = $(if ($ok) { 'PASS' } else { 'FAIL' });
        reason = $result.reason; artifact = $artifact })
    if (-not $ok) { $State.failed = $true }
}
function Add-VerificationLocalStep($State, [string]$Id, [string]$Reason, [DateTime]$Start, [long]$DurationMs) {
    $now = [DateTime]::UtcNow.ToString('o')
    $ok = $Reason -eq 'OK'
    [void]$State.steps.Add([ordered]@{ check_id = $Id; command = 'source-text'; args = @('tracked+untracked'); cwd = 'REPO';
        started_at_utc = $Start.ToString('o'); ended_at_utc = $now; duration_ms = $DurationMs; versions = $State.versions;
        native_exit = $null; outcome = $(if ($ok) { 'PASS' } else { 'FAIL' }); reason = $Reason; artifact = $null })
    if (-not $ok) { $State.failed = $true }
}
function Add-VerificationGuardStep($State, [string]$Id, [string]$Reason) {
    $ok = $Reason -eq 'OK'
    [void]$State.steps.Add([ordered]@{ check_id = $Id; command = 'source-integrity'; args = @('tracked+candidate-untracked'); cwd = 'REPO';
        started_at_utc = [DateTime]::UtcNow.ToString('o'); ended_at_utc = [DateTime]::UtcNow.ToString('o'); duration_ms = 0;
        versions = $State.versions; native_exit = $null; outcome = $(if ($ok) { 'PASS' } else { 'FAIL' }); reason = $Reason; artifact = $null })
    if (-not $ok) { $State.failed = $true }
}
function Complete-Verification($State, [string[]]$NotImplemented) {
    $report = [ordered]@{ schema_version = 1; suite = $State.suite; stage = 'S01'; stage_result = 'IN_PROGRESS';
        scope = 'implemented-contract-subset'; outcome = $(if ($State.failed) { 'FAIL' } else { 'PASS' });
        steps = @($State.steps.ToArray()); catalog = @(@{ check_id = 'FOUNDATION-OFFLINE-FULL'; outcome = 'NOT_RUN'; reason = 'implementation incomplete; subset is not stage acceptance' }) + @($NotImplemented | ForEach-Object { @{ check_id = $_; outcome = 'NOT_IMPLEMENTED'; reason = 'future owner; not part of implemented contract subset' } });
        raw_logs = 'private .pi output only; do not upload automatically' }
    Write-VerificationJson (Join-Path $State.output 'result.json') $report
    if ($State.failed) { [Console]::Out.WriteLine('Verification FAIL; sanitized result.json in requested .pi output'); return 1 }
    [Console]::Out.WriteLine('Implemented contract subset PASS; S01 remains IN_PROGRESS'); return 0
}
function Get-VerificationEnvironment([string]$Root, [string]$LogRoot) {
    $rustBin = Join-Path $Root '.pi\toolchains\rust-1.98.1-x86_64-pc-windows-msvc\bin'
    $nodeBin = Join-Path $Root '.pi\toolchains\node-v24.13.0-win-x64'
    $vc = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
    foreach ($path in @((Join-Path $rustBin 'cargo.exe'), (Join-Path $rustBin 'rustc.exe'), (Join-Path $nodeBin 'node.exe'), $vc)) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw 'Missing pinned Rust/Node/MSVC prerequisite' }
    }
    $pnpm = Get-Command pnpm.cmd -ErrorAction SilentlyContinue
    if (-not $pnpm) { throw 'Missing pnpm prerequisite' }
    $pnpmJs = Join-Path (Split-Path $pnpm.Source -Parent) 'node_modules\pnpm\bin\pnpm.cjs'
    if (-not (Test-Path $pnpmJs)) { throw 'Missing pnpm package prerequisite' }
    if (-not (Test-Path (Join-Path $Root 'packages\protocol\node_modules\.bin\tsc.CMD'))) { throw 'Missing installed protocol package prerequisite; bootstrap explicitly' }
    # The current catalog covers exactly one package. New packages demand an explicit suite update.
    $packageDirs = @(Get-ChildItem (Join-Path $Root 'packages') -Directory | ForEach-Object { $_.Name })
    if ($packageDirs.Count -ne 1 -or $packageDirs[0] -ne 'protocol') { throw 'WORKSPACE_SUITE_UPDATE_REQUIRED' }
    $rootPackage = ConvertFrom-Json ([System.IO.File]::ReadAllText((Join-Path $Root 'package.json')))
    $protocolPackage = ConvertFrom-Json ([System.IO.File]::ReadAllText((Join-Path $Root 'packages\protocol\package.json')))
    foreach ($name in @('typecheck','lint','test','contracts:check')) {
        if (-not $rootPackage.scripts.$name -or -not $protocolPackage.scripts.$name -or
            $rootPackage.scripts.$name -match '--if-present|continue-on-error' -or
            $protocolPackage.scripts.$name -match '--if-present|continue-on-error') { throw 'Missing or optional mandatory pnpm script' }
    }
    $workspace = [System.IO.File]::ReadAllText((Join-Path $Root 'pnpm-workspace.yaml'))
    if ($workspace -notmatch "(?m)^\s+-\s+'packages/protocol'\s*$" -or ([regex]::Matches($workspace, '(?m)^\s+-\s+')).Count -ne 1) { throw 'WORKSPACE_SUITE_UPDATE_REQUIRED' }
    if ($rootPackage.packageManager -ne 'pnpm@10.32.1' -or $rootPackage.engines.node -ne '24.13.0') { throw 'Package pin mismatch' }
    $cmd = Join-Path $env:SystemRoot 'System32\cmd.exe'
    # vcvars writes only into this child; parse its environment, never mutate caller/registry.
    $vsLog = Join-Path $LogRoot 'vs-environment.log'
    $vsCommand = 'call "' + $vc + '" >nul && set'
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    # cmd /s strips this outer quote pair, leaving the quoted vcvars path intact.
    $psi.FileName = $cmd; $psi.Arguments = '/d /s /c "' + $vsCommand + '"'
    $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $vproc = [System.Diagnostics.Process]::Start($psi)
    try {
        $outTask = $vproc.StandardOutput.ReadToEndAsync()
        $errTask = $vproc.StandardError.ReadToEndAsync()
        if (-not $vproc.WaitForExit(60000)) {
            $killer = Join-Path $env:SystemRoot 'System32\taskkill.exe'
            if (Test-Path $killer) { & $killer /PID $vproc.Id /T /F 2>&1 | Out-Null }
            try { if (-not $vproc.HasExited) { $vproc.Kill() } } catch { }
            throw 'MSVC developer environment timeout'
        }
        $envText = $outTask.Result; $errText = $errTask.Result
        [System.IO.File]::WriteAllText($vsLog, $errText, (New-Object System.Text.UTF8Encoding($false)))
        if ($vproc.ExitCode -ne 0 -or $envText -notmatch '(?im)^VSCMD_ARG_TGT_ARCH=x64\s*$') { throw 'MSVC developer environment failed' }
    } finally { $vproc.Dispose() }
    $map = @{}
    foreach ($line in ($envText -split "`r?`n")) {
        $pos = $line.IndexOf('='); if ($pos -gt 0) { $map[$line.Substring(0,$pos)] = $line.Substring($pos+1) }
    }
    $map['PATH'] = $rustBin + ';' + $nodeBin + ';' + (Join-Path $Root 'packages\protocol\node_modules\.bin') + ';' + $map['PATH']
    $map['CARGO_HOME'] = Join-Path $Root '.pi\donor-build\.cargo-home'
    $map['CARGO_TARGET_DIR'] = Join-Path $Root '.pi\verify-contract-target'
    $map['NODE_OPTIONS'] = ''
    $map['COREPACK_ENABLE_AUTO_PIN'] = '0'
    return @{ rust = (Join-Path $rustBin 'cargo.exe'); rustc = (Join-Path $rustBin 'rustc.exe'); node = (Join-Path $nodeBin 'node.exe'); pnpm = $pnpmJs; python = $(try { (Get-Command python.exe -ErrorAction Stop).Source } catch { '' }); env = $map }
}
function Assert-VerificationVersions($State, $Tools) {
    $versionChecks = @(
        @('rustc',$Tools.rustc,@('--version'),'rustc 1.98.1'),
        @('cargo',$Tools.rust,@('--version'),'cargo 1.98.1'),
        @('node',$Tools.node,@('--version'),'v24.13.0'),
        @('pnpm',$Tools.node,@($Tools.pnpm,'--version'),'10.32.1')
    )
    foreach ($item in $versionChecks) {
        if ($State.failed) { break }
        $id,$exe,$args,$expected = $item
        Add-VerificationStep $State ('PREREQ-' + $id) $exe $args $Tools.env 30
        if (-not $State.failed) {
            $last = $State.steps[$State.steps.Count-1]
            $log = [System.IO.File]::ReadAllText((Join-Path $State.output $last.artifact[0].id))
            if (-not $log.Contains($expected)) { $last.outcome = 'FAIL'; $last.reason = 'VERSION_MISMATCH'; $State.failed = $true }
        }
    }
    Add-VerificationStep $State 'PREREQ-jsonschema' $Tools.python @('-c','import importlib.metadata,sys; sys.exit(0 if importlib.metadata.version("jsonschema")=="4.26.0" else 1)') $Tools.env 30
}
# Fixed original plan corpus: never derive expected hashes from mutable worktree files.
$script:PlanHashes = [ordered]@{
    'ScreenMemory_Full_Development_Plan_v3.md' = 'c238bcf79f8ceb907060599649b823c72cffa1d3b39f6f14f66664b7537398a6'
    'ScreenMemory_S01_Foundation_Architecture.md' = '5497a8f9fdd2e63cab0f06e31b92655471543a049379adb55209b3c1170c978c'
    'ScreenMemory_S02_Capture_Library.md' = '23c66146f4ed761963d79c4a4efe9c4740b8c05c7bbef4bdf76910aefcb4c4ef'
    'ScreenMemory_S03_Import_Images_Export.md' = '4d5a35030693df674f6c70a9a00ca7ef6bc8711ede5258894b4d46c32a12845b'
    'ScreenMemory_S04_Processing_LMStudio.md' = 'c463c1336a7c8276aa695d3310822a3c51afecd8e2adbc781ff901a34f118c25'
    'ScreenMemory_S05_Description_Editing.md' = '603ae93d0c7317cf5585a54eae44edb713b25c1f65daa3b347914c31b4b223bd'
    'ScreenMemory_S06_Index_Search.md' = 'e7c8015aa1b1b1bf128495ff8620185ebddf6d4b6b996dc92fe28cb9b070d2e1'
    'ScreenMemory_S07_GUI_CLI_MCP.md' = '50711350f855130ca03c33c2dae66811ea474ab56ea618b7f1fa830e9dfbe210'
    'ScreenMemory_S08_Release_Operations.md' = '926b628ae3484dca2d72b91ba65c02c006b31791457774bba5724b7802d0a3bf'
}
function Assert-PlanHashes([string]$Root) {
    foreach ($name in $script:PlanHashes.Keys) {
        $file = Join-Path $Root (Join-Path 'docs' $name)
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw 'PLAN_HASH_MISMATCH' }
        if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant() -cne $script:PlanHashes[$name]) { throw 'PLAN_HASH_MISMATCH' }
    }
}
function Get-SourceCandidates([string]$Root) {
    $git = (Get-Command git.exe -ErrorAction Stop).Source
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $git; $psi.Arguments = '-C ' + (Quote-WindowsArgument $Root) + ' ls-files --cached --others --exclude-standard -z'
    $psi.UseShellExecute = $false; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $proc = [System.Diagnostics.Process]::Start($psi)
    try {
        $buffer = New-Object System.IO.MemoryStream
        $errorTask = $proc.StandardError.ReadToEndAsync()
        $proc.StandardOutput.BaseStream.CopyTo($buffer)
        $proc.WaitForExit()
        $null = $errorTask.Result
        if ($proc.ExitCode -ne 0) { throw 'SOURCE_ENUMERATION_ERROR' }
        return @($script:Utf8.GetString($buffer.ToArray()) -split "`0" | Where-Object { $_ })
    } finally { $proc.Dispose() }
}
function Get-SourceSnapshot([string]$Root, [string[]]$Candidates = $null) {
    if ($null -eq $Candidates) { $Candidates = @(Get-SourceCandidates $Root) }
    $snapshot = @{}
    foreach ($relative in $Candidates) {
        if ($relative -match '^(?i)\.pi/') { throw 'Private output is a tracked source path' }
        # Git's standard ignore rules exclude declared build/cache roots; never omit tracked source.
        # The real candidate list comes from Git's tracked + nonignored untracked inventory.
        # No filtered source extension: additions/deletions of schemas, locks, and other sources count.
        $file = Join-Path $Root ($relative.Replace('/', '\'))
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw 'SOURCE_INVENTORY_ERROR' }
        $snapshot[$relative] = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    return $snapshot
}
function Test-SourceSnapshot($Before, $After) {
    if ($Before.Count -ne $After.Count) { return $false }
    foreach ($name in $Before.Keys) {
        if (-not $After.ContainsKey($name) -or $Before[$name] -cne $After[$name]) { return $false }
    }
    return $true
}
function Assert-SourceText([string]$Root) {
    $git = (Get-Command git.exe -ErrorAction Stop).Source
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $git; $psi.Arguments = '-C ' + (Quote-WindowsArgument $Root) + ' ls-files --cached --others --exclude-standard -z'
    $psi.UseShellExecute = $false; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $proc = [System.Diagnostics.Process]::Start($psi)
    $listing = $proc.StandardOutput.ReadToEnd(); $null = $proc.StandardError.ReadToEnd(); $proc.WaitForExit()
    if ($proc.ExitCode -ne 0) { throw 'Source candidate enumeration failed' }
    $checked = 0
    foreach ($relative in ($listing -split "`0")) {
        if (-not $relative) { continue }
        # Only the nine canonical repo-relative paths in the SHA guard bypass formatting.
        $normalized = $relative.Replace('\','/')
        if ($normalized.StartsWith('docs/', [StringComparison]::Ordinal) -and
            @($script:PlanHashes.Keys) -ccontains $normalized.Substring(5)) { continue }
        if ($relative -notmatch '(?i)(\.((ps1|psm1|rs|ts|tsx|mjs|js|json|md|toml|yaml|yml|py|txt|schema|gitignore|npmrc))$|^(AGENTS\.md|Cargo\.lock|\.npmrc|\.gitignore)$)') { continue }
        $file = Join-Path $Root ($relative.Replace('/', '\'))
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { continue }
        $bytes = [System.IO.File]::ReadAllBytes($file)
        if ($bytes.Length -ge 3 -and $bytes[0] -eq 239 -and $bytes[1] -eq 187 -and $bytes[2] -eq 191) { throw 'SOURCE_BOM' }
        $text = $script:Utf8.GetString($bytes)
        if ($text.Contains("`r")) { throw 'SOURCE_CRLF' }
        if ($text -match '(?m)[\t ]+$') { throw 'SOURCE_TRAILING_WHITESPACE' }
        $checked++
    }
    return $checked
}
