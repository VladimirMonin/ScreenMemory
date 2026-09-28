# S01-VER-01/02/03: real wrapper child, fail-fast, mutation and untracked source byte regression.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'verification-common.ps1')
$root = Split-Path $PSScriptRoot -Parent
$temp = Join-Path $root ('.pi\test-verify-' + [guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $temp)
$ps = Join-Path $PSHOME 'powershell.exe'
$beforeEnv = @{ PATH = $env:PATH; CARGO_HOME = $env:CARGO_HOME; NODE_OPTIONS = $env:NODE_OPTIONS }
$null = Get-VerificationEnvironment $root $temp
foreach ($key in $beforeEnv.Keys) {
    if ([string][Environment]::GetEnvironmentVariable($key, 'Process') -cne [string]$beforeEnv[$key]) { throw 'Caller process environment changed' }
}
$fixture = ConvertFrom-Json ([System.IO.File]::ReadAllText((Join-Path $root 'tests\fixtures\s01\verification-manifest.json')))
if ($fixture.cases.Count -ne 1 -or $fixture.cases[0].id -ne 'S01-VER-01' -or
    (Get-FileHash (Join-Path $root 'tests\fixtures\s01\native-child.ps1') -Algorithm SHA256).Hash.ToLowerInvariant() -ne $fixture.cases[0].sha256) { throw 'Synthetic fixture manifest/checksum mismatch' }
$baseline = @{}
$files = @(Get-ChildItem (Join-Path $root 'schemas') -File) + @(Get-ChildItem (Join-Path $root 'packages\protocol\src\generated') -File) + @(
    (Get-Item (Join-Path $root 'Cargo.lock')), (Get-Item (Join-Path $root 'pnpm-lock.yaml'))) +
    @(Get-ChildItem (Join-Path $root 'docs') -File -Filter 'ScreenMemory_*.md')
foreach ($file in $files) { $baseline[$file.FullName] = (Get-FileHash $file.FullName -Algorithm SHA256).Hash }
function Assert-Case([string]$Runner, [string]$Case, [int]$Exit, [string]$Reason, [string]$Output) {
    & $ps -NoProfile -NonInteractive -File $Runner -FixtureCase $Case -OutputRoot $Output | Out-Null
    $raw = $LASTEXITCODE
    if ($raw -ne $Exit) { throw "case $Case unexpected wrapper exit $raw" }
    $reportFile = Join-Path $Output 'result.json'
    $bytes = [System.IO.File]::ReadAllBytes($reportFile)
    if ($bytes -contains [byte]13 -or ($bytes.Length -ge 3 -and $bytes[0] -eq 239 -and $bytes[1] -eq 187 -and $bytes[2] -eq 191)) { throw 'Public report not UTF8/LF/no-BOM' }
    $report = ConvertFrom-Json ([System.Text.Encoding]::UTF8.GetString($bytes))
    if ($report.steps[0].reason -ne $Reason) { throw "case $Case wrong reason" }
    if ($Case -eq 'exit7' -and $report.steps[0].native_exit -ne 7) { throw 'S01-VER-01 lost raw exit7' }
    if ($Case -in @('exit7','success')) {
        $argsExpected = @('-NoProfile','-NonInteractive','-File','@PRIVATE','-Mode',$Case,'-Label','two words')
        if ($report.steps[0].command -ne 'powershell.exe' -or
            (ConvertTo-Json -Compress $report.steps[0].args) -cne (ConvertTo-Json -Compress $argsExpected)) { throw 'Synthetic fixed executable/argv evidence drift' }
    }
    if ($Case -eq 'timeout' -and $null -eq $report.steps[0].native_exit) { throw 'Started timeout child lost raw termination code' }
    if ($Case -in @('missing','spawn','prerequisite') -and $null -ne $report.steps[0].native_exit) { throw 'Not-started child has native exit' }
    if ($Exit -ne 0 -and ($report.steps.Count -ne 1 -or $report.outcome -ne 'FAIL')) { throw 'Fail-fast or FAIL evidence violated' }
    if ($Exit -eq 0 -and ($report.steps.Count -ne 2 -or $report.steps[1].native_exit -ne 0 -or $report.outcome -ne 'PASS')) { throw 'Success continuation did not run' }
    if ($Case -eq 'stderr' -and ([System.Text.Encoding]::UTF8.GetString($bytes)).Contains('synthetic-secret-canary')) { throw 'Leaked stderr' }
}
$untracked = Join-Path $root 'tests\fixtures\s01\untracked-format-negative.json'
if (Test-Path $untracked) { throw 'Negative fixture collision; refusing overwrite' }
try {
    # Run both real positive suites in the regression and assert their exact public argv.
    $expected = @{
        'RUST-FMT' = @('cargo',@('fmt','--all','--','--check'))
        'RUST-CLIPPY' = @('cargo',@('clippy','--workspace','--all-targets','--locked','--','-D','warnings'))
        'RUST-TEST' = @('cargo',@('test','--workspace','--locked'))
        'HOST-LEASE' = @('cargo',@('test','--locked','-p','sm-isolation','--test','host_lease'))
        'FRAMING' = @('cargo',@('test','--locked','-p','sm-framing'))
        'PIPE-NATIVE' = @('cargo',@('test','--locked','-p','sm-pipe'))
        'PNPM-TYPECHECK' = @('node',@('@PNPM_CLI','typecheck'))
        'PNPM-LINT' = @('node',@('@PNPM_CLI','lint'))
        'PNPM-TEST' = @('node',@('@PNPM_CLI','test'))
        'CONTRACTS' = @('powershell.exe',@('-NoProfile','-NonInteractive','-File','scripts/verify-contracts.ps1','-OutputRoot','@PRIVATE'))
        'SCHEMA-GENERATE' = @('cargo',@('run','--locked','-p','sm-protocol','--example','generate_schemas','--','--output','@PRIVATE'))
        'SCHEMA-CHECK' = @('cargo',@('run','--locked','-p','sm-protocol','--example','generate_schemas','--','--check','schemas'))
        'TS-GENERATE' = @('node',@('packages/protocol/scripts/generate.mjs','--output','@PRIVATE'))
        'PNPM-CONTRACTS' = @('node',@('@PNPM_CLI','contracts:check'))
        'BOUNDARIES' = @('python',@('tests/contracts/check_boundaries.py'))
        'SCHEMA-FIXTURES' = @('python',@('tests/contracts/check_schemas.py'))
        'PREREQ-pnpm' = @('node',@('@PNPM_CLI','--version'))
    }
    foreach ($suite in @('verify','verify-contracts')) {
        $out = Join-Path $temp ('positive-' + $suite)
        & $ps -NoProfile -NonInteractive -File (Join-Path $PSScriptRoot ($suite + '.ps1')) -OutputRoot $out | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "Real $suite regression failed" }
        $positive = ConvertFrom-Json ([System.IO.File]::ReadAllText((Join-Path $out 'result.json')))
        if ($positive.outcome -ne 'PASS') { throw "Real $suite report failed" }
        foreach ($id in @('PLAN-HASH-PRE','SOURCE-INTEGRITY-PRE','PLAN-HASH-POST','SOURCE-INTEGRITY-POST')) {
            if (@($positive.steps | Where-Object { $_.check_id -eq $id -and $_.outcome -eq 'PASS' }).Count -ne 1) { throw "Missing positive integrity gate $id" }
        }
        foreach ($step in $positive.steps) {
            if (-not $expected.ContainsKey($step.check_id)) { continue }
            $want = $expected[$step.check_id]
            if ($step.command -cne $want[0] -or
                (ConvertTo-Json -InputObject @($step.args) -Compress) -cne (ConvertTo-Json -InputObject @($want[1]) -Compress)) { throw "Fixed command evidence incorrect: $($step.check_id)" }
        }
        $ids = @($positive.steps | ForEach-Object { $_.check_id })
        foreach ($id in $expected.Keys) {
            if ($id -in @('PREREQ-pnpm') -or ($suite -eq 'verify' -and $id -in @('RUST-FMT','RUST-CLIPPY','RUST-TEST','HOST-LEASE','FRAMING','PIPE-NATIVE','PNPM-TYPECHECK','PNPM-LINT','PNPM-TEST','CONTRACTS')) -or
                ($suite -eq 'verify-contracts' -and $id -in @('SCHEMA-GENERATE','SCHEMA-CHECK','TS-GENERATE','PNPM-CONTRACTS','BOUNDARIES','SCHEMA-FIXTURES'))) {
                if ($id -notin $ids) { throw "Missing fixed check evidence: $id" }
            }
        }
    }
    foreach ($case in @('success','exit7','missing','spawn','timeout','stderr','prerequisite')) {
        $expect = @{ success = @(0,'OK'); exit7 = @(1,'NATIVE_EXIT'); missing = @(1,'MISSING_EXECUTABLE');
            spawn = @(1,'SPAWN_ERROR'); timeout = @(1,'TIMEOUT'); stderr = @(0,'OK'); prerequisite = @(1,'MISSING_PREREQUISITE') }[$case]
        Assert-Case (Join-Path $PSScriptRoot 'verify.ps1') $case $expect[0] $expect[1] (Join-Path $temp $case)
    }
    # Exercise the same pinned-plan and source snapshot guards via real wrapper in a copied host.
    foreach ($case in @('plan-mismatch','plan-missing','source-modify','source-create','source-delete','source-fail')) {
        # Strict synthetic host form; never modify source outside ignored .pi.
        $hostRoot = Join-Path $temp 'host'
        if (Test-Path $hostRoot) { Remove-Item -LiteralPath $hostRoot -Recurse -Force }
        [void](New-Item -ItemType Directory -Path (Join-Path $hostRoot 'scripts') -Force)
        [void](New-Item -ItemType Directory -Path (Join-Path $hostRoot 'docs') -Force)
        [void](New-Item -ItemType Directory -Path (Join-Path $hostRoot 'tests\fixtures\s01') -Force)
        [void](New-Item -ItemType Directory -Path (Join-Path $hostRoot '.pi') -Force)
        foreach ($script in @('verify.ps1','verification-common.ps1')) { [System.IO.File]::Copy((Join-Path $PSScriptRoot $script),(Join-Path $hostRoot "scripts\$script")) }
        [System.IO.File]::Copy((Join-Path $root 'tests\fixtures\s01\integrity-child.ps1'),(Join-Path $hostRoot 'tests\fixtures\s01\integrity-child.ps1'))
        foreach ($name in $script:PlanHashes.Keys) { [System.IO.File]::Copy((Join-Path $root "docs\$name"),(Join-Path $hostRoot "docs\$name")) }
        [System.IO.File]::WriteAllText((Join-Path $hostRoot 'scripts\guard-source.txt'),'original')
        if ($case -eq 'plan-mismatch') { [System.IO.File]::AppendAllText((Join-Path $hostRoot 'docs\ScreenMemory_S01_Foundation_Architecture.md'),'synthetic-change') }
        if ($case -eq 'plan-missing') { Remove-Item -LiteralPath (Join-Path $hostRoot 'docs\ScreenMemory_S01_Foundation_Architecture.md') }
        & $ps -NoProfile -NonInteractive -File (Join-Path $hostRoot 'scripts\verify.ps1') -FixtureCase $case -OutputRoot (Join-Path $hostRoot ".pi\$case") | Out-Null
        if ($LASTEXITCODE -ne 1) { throw "integrity $case expected wrapper RED exit1" }
        $red = ConvertFrom-Json ([System.IO.File]::ReadAllText((Join-Path $hostRoot ".pi\$case\result.json")))
        $expectedId = $(if ($case -like 'plan-*') { 'PLAN-HASH-PRE' } else { 'SOURCE-INTEGRITY-POST' })
        $failed = @($red.steps | Where-Object { $_.check_id -eq $expectedId -and $_.outcome -eq 'FAIL' })
        if ($failed.Count -ne 1 -or $red.outcome -ne 'FAIL') { throw "integrity $case not caught by production guard" }
        if ($case -like 'source-*') {
            $childExit = $(if ($case -eq 'source-fail') { 7 } else { 0 })
            if (@($red.steps | Where-Object { $_.check_id -eq 'SOURCE-MUTATION-CHILD' -and $_.native_exit -eq $childExit }).Count -ne 1) { throw 'Mutation child exit evidence incorrect' }
            if ($case -eq 'source-fail' -and $red.steps[-1].check_id -ne 'SOURCE-INTEGRITY-POST') { throw 'Failed child skipped post guard' }
        }
    }
    # Mutate only an isolated copied runner, never the production source. Its ordinary exit7 regression MUST fail.
    $mutantRoot = Join-Path $temp 'mutant'
    [void](New-Item -ItemType Directory -Path (Join-Path $mutantRoot 'scripts') -Force)
    [void](New-Item -ItemType Directory -Path (Join-Path $mutantRoot 'tests\fixtures\s01') -Force)
    [void](New-Item -ItemType Directory -Path (Join-Path $mutantRoot '.pi') -Force)
    [System.IO.File]::Copy((Join-Path $PSScriptRoot 'verify.ps1'), (Join-Path $mutantRoot 'scripts\verify.ps1'))
    [System.IO.File]::Copy((Join-Path $root 'tests\fixtures\s01\native-child.ps1'), (Join-Path $mutantRoot 'tests\fixtures\s01\native-child.ps1'))
    $common = [System.IO.File]::ReadAllText((Join-Path $PSScriptRoot 'verification-common.ps1'))
    $needle = '$ok = $result.reason -eq ''OK'''
    if (([regex]::Matches($common,[regex]::Escape($needle))).Count -ne 1) { throw 'Mutation target changed' }
    [System.IO.File]::WriteAllText((Join-Path $mutantRoot 'scripts\verification-common.ps1'), $common.Replace($needle,'$ok = $true'))
    $caught = $false
    try { Assert-Case (Join-Path $mutantRoot 'scripts\verify.ps1') 'exit7' 1 'NATIVE_EXIT' (Join-Path $mutantRoot '.pi\mutation') }
    catch { $caught = $true }
    if (-not $caught) { throw 'Ordinary S01-VER-01 failed to detect disabled exit guard' }
    # Explicitly untracked candidate, not git diff --check. Restore by deleting only this owned fixture.
    [System.IO.File]::WriteAllBytes($untracked, [System.Text.Encoding]::UTF8.GetBytes("{`r`n  `"x`": 1  `r`n}`r`n"))
    $negativeCaught = $false
    try { $null = Assert-SourceText $root } catch { $negativeCaught = $_.Exception.Message -in @('SOURCE_CRLF','SOURCE_TRAILING_WHITESPACE') }
    if (-not $negativeCaught) { throw 'Untracked JSON CRLF regression not detected' }
    [System.IO.File]::WriteAllBytes($untracked, [System.Text.Encoding]::UTF8.GetBytes("{`n  `"x`": 1  `n}`n"))
    $trailingCaught = $false
    try { $null = Assert-SourceText $root } catch { $trailingCaught = $_.Exception.Message -eq 'SOURCE_TRAILING_WHITESPACE' }
    if (-not $trailingCaught) { throw 'Untracked JSON trailing-space regression not detected' }
    Remove-Item -LiteralPath $untracked -Force
    $null = Assert-SourceText $root
    # A separate disposable Git root makes the same-prefix file genuinely untracked
    # without writing malformed bytes into the real docs/ or checked-in fixtures.
    $fixtureRoot = Join-Path $temp 'exact-plan-path-host'
    if (Test-Path -LiteralPath $fixtureRoot) { throw 'Synthetic Git host collision' }
    [void](New-Item -ItemType Directory -Path (Join-Path $fixtureRoot 'docs') -Force)
    if (-not (Assert-Under $fixtureRoot $temp) -or
        ((Get-Item -LiteralPath $fixtureRoot -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint) -or
        (Test-Path -LiteralPath (Join-Path $fixtureRoot '.git'))) { throw 'Unsafe synthetic Git host' }
    $git = (Get-Command git.exe -ErrorAction Stop).Source
    function Invoke-SyntheticGit([string[]]$GitArgs) {
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $git
        $psi.Arguments = (($GitArgs | ForEach-Object { Quote-WindowsArgument $_ }) -join ' ')
        $psi.WorkingDirectory = $fixtureRoot
        $psi.UseShellExecute = $false
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        foreach ($key in @('GIT_DIR','GIT_WORK_TREE','GIT_INDEX_FILE','GIT_COMMON_DIR')) { $psi.EnvironmentVariables.Remove($key) }
        $proc = [System.Diagnostics.Process]::Start($psi)
        try {
            $stdout = $proc.StandardOutput.ReadToEnd()
            $null = $proc.StandardError.ReadToEnd()
            if (-not $proc.WaitForExit(30000)) { throw 'Synthetic Git command timeout' }
            return @{ exit = $proc.ExitCode; stdout = $stdout }
        } finally { $proc.Dispose() }
    }
    $init = Invoke-SyntheticGit @('-C',$fixtureRoot,'init','--quiet')
    if ($init.exit -ne 0) { throw 'Synthetic Git init failed' }
    $top = Invoke-SyntheticGit @('-C',$fixtureRoot,'rev-parse','--show-toplevel')
    if ($top.exit -ne 0 -or [System.IO.Path]::GetFullPath($top.stdout.Trim()) -cne [System.IO.Path]::GetFullPath($fixtureRoot)) { throw 'Synthetic Git host escaped fixture root' }
    $exactNeedle = @'
        if ($normalized.StartsWith('docs/', [StringComparison]::Ordinal) -and
            @($script:PlanHashes.Keys) -ccontains $normalized.Substring(5)) { continue }
'@
    if ($common.IndexOf($exactNeedle, [StringComparison]::Ordinal) -lt 0) { throw 'Exact-plan exclusion target changed' }
    $oldHelper = Join-Path $temp 'old-prefix-verification-common.ps1'
    [System.IO.File]::WriteAllText($oldHelper, $common.Replace($exactNeedle, "        if (`$relative -match '^docs/ScreenMemory_(Full_Development_Plan_v3|S0[1-8]_)') { continue }"), $script:Utf8)
    foreach ($name in @('ScreenMemory_S01_synthetic-extra.md',
            'ScreenMemory_S01_Foundation_Architecture.md.extra.md',
            'ScreenMemory_Full_Development_Plan_v3-extra.md')) {
        $samePrefix = Join-Path $fixtureRoot (Join-Path 'docs' $name)
        [System.IO.File]::WriteAllBytes($samePrefix, [System.Text.Encoding]::UTF8.GetBytes("synthetic  `r`n"))
        if (@(Get-SourceCandidates $fixtureRoot) -cnotcontains "docs/$name") { throw 'Synthetic same-prefix path is not Git untracked source' }
        $oldCount = & { param($helper,$hostRoot) . $helper; Assert-SourceText $hostRoot } $oldHelper $fixtureRoot
        if ($oldCount -ne 0) { throw 'Old prefix exclusion did not reproduce format bypass' }
        $samePrefixCaught = $false
        try { $null = Assert-SourceText $fixtureRoot } catch { $samePrefixCaught = $_.Exception.Message -eq 'SOURCE_CRLF' }
        if (-not $samePrefixCaught) { throw 'Same-prefix untracked CRLF bypass persists' }
        [System.IO.File]::WriteAllBytes($samePrefix, [System.Text.Encoding]::UTF8.GetBytes("synthetic  `n"))
        $samePrefixCaught = $false
        try { $null = Assert-SourceText $fixtureRoot } catch { $samePrefixCaught = $_.Exception.Message -eq 'SOURCE_TRAILING_WHITESPACE' }
        if (-not $samePrefixCaught) { throw 'Same-prefix untracked trailing whitespace bypass persists' }
        [System.IO.File]::WriteAllBytes($samePrefix, [System.Text.Encoding]::UTF8.GetBytes("synthetic`n"))
        if ((Assert-SourceText $fixtureRoot) -ne 1) { throw 'Clean same-prefix source must pass format scan' }
        Remove-Item -LiteralPath $samePrefix -Force
    }
    Assert-PlanHashes $root # originals remain under their separate immutable SHA gate
    foreach ($file in $baseline.Keys) {
        if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $baseline[$file]) { throw 'Tracked source hash changed' }
    }
    [Console]::Out.WriteLine('S01-VER-01/02/03 PASS: real fixed argv/integrity, exit7 raw, plan hash RED, source modify/create/delete/exit7 RED, fail-fast, mutation caught, timeout/spawn/untracked format, exact-plan same-prefix RED/PASS')
} finally {
    if (Test-Path $untracked) { Remove-Item -LiteralPath $untracked -Force }
    # Keep only ignored synthetic evidence for review; no cleanup of unrelated data.
}
