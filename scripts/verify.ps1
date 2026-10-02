# S01-T04a implemented contract subset only; -FixtureCase is an explicit synthetic runner regression, not a production bypass.
[CmdletBinding()]
param([string]$OutputRoot, [ValidateSet('','success','exit7','missing','spawn','timeout','stderr','prerequisite','plan-mismatch','plan-missing','source-modify','source-create','source-delete','source-fail')][string]$FixtureCase = '')
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'verification-common.ps1')
$root = Split-Path $PSScriptRoot -Parent
$state = $null
$before = $null
try {
    if (-not $OutputRoot) { $OutputRoot = Join-Path $root ('.pi\verify-' + [guid]::NewGuid().ToString('N')) }
    $state = New-VerificationState $root $OutputRoot 'verify'
    if ($FixtureCase) {
        $integrityFixture = $FixtureCase -match '^(plan-|source-)'
        if ($integrityFixture) {
            # Only an isolated copied host may execute intentional source mutation.
            if ($root -notmatch '[\\/]\.pi[\\/]test-verify-[0-9a-f]{32}[\\/]host$') { throw 'Integrity fixtures require isolated copied host' }
            try { Assert-PlanHashes $root; Add-VerificationGuardStep $state 'PLAN-HASH-PRE' 'OK' }
            catch { Add-VerificationGuardStep $state 'PLAN-HASH-PRE' 'PLAN_HASH_MISMATCH' }
            if (-not $state.failed) {
                $candidates = @('scripts/guard-source.txt')
                $before = Get-SourceSnapshot $root $candidates
                Add-VerificationGuardStep $state 'SOURCE-INTEGRITY-PRE' 'OK'
                if ($FixtureCase -match '^source-') {
                    $ps = Join-Path $PSHOME 'powershell.exe'
                    Add-VerificationStep $state 'SOURCE-MUTATION-CHILD' $ps @('-NoProfile','-NonInteractive','-File',(Join-Path $root 'tests\fixtures\s01\integrity-child.ps1'),'-Mode',$FixtureCase,'-HostRoot',$root) @{} 20
                    if ($FixtureCase -eq 'source-create') { $candidates += 'scripts/guard-created.txt' }
                }
                try { Assert-PlanHashes $root; Add-VerificationGuardStep $state 'PLAN-HASH-POST' 'OK' }
                catch { Add-VerificationGuardStep $state 'PLAN-HASH-POST' 'PLAN_HASH_MISMATCH' }
                try {
                    $after = Get-SourceSnapshot $root $candidates
                    Add-VerificationGuardStep $state 'SOURCE-INTEGRITY-POST' $(if (Test-SourceSnapshot $before $after) { 'OK' } else { 'SOURCE_MUTATION' })
                } catch { Add-VerificationGuardStep $state 'SOURCE-INTEGRITY-POST' 'SOURCE_INVENTORY_ERROR' }
            }
        } else {
        $ps = Join-Path $PSHOME 'powershell.exe'
        $mode = $FixtureCase
        if ($mode -notin @('exit7','stderr','timeout')) { $mode = 'success' }
        $fixtureDir = Join-Path $state.output 'with spaces'
        [void](New-Item -ItemType Directory -Path $fixtureDir)
        $fixtureScript = Join-Path $fixtureDir 'native child.ps1'
        [System.IO.File]::Copy((Join-Path $root 'tests\fixtures\s01\native-child.ps1'), $fixtureScript)
        $arg = @('-NoProfile','-NonInteractive','-File',$fixtureScript,'-Mode',$mode,'-Label','two words')
        if ($FixtureCase -eq 'timeout') { $timeout = 1 } else { $timeout = 20 }
        if ($FixtureCase -eq 'missing') { $ps = Join-Path $state.output 'does-not-exist.exe' }
        if ($FixtureCase -eq 'spawn') { $ps = Join-Path $state.output 'not-executable.exe'; [System.IO.File]::WriteAllText($ps, 'synthetic') }
        Add-VerificationStep $state 'S01-VER-01' $ps $arg @{} $timeout $(if ($FixtureCase -eq 'prerequisite') { 'synthetic prerequisite missing' } else { '' })
        # A following mandatory step must never mask exit7 or run after failure.
        Add-VerificationStep $state 'S01-VER-FAILFAST' (Join-Path $PSHOME 'powershell.exe') @('-NoProfile','-NonInteractive','-Command','exit 0') @{} 20
        }
    } else {
        try { Assert-PlanHashes $root; Add-VerificationGuardStep $state 'PLAN-HASH-PRE' 'OK' }
        catch { Add-VerificationGuardStep $state 'PLAN-HASH-PRE' 'PLAN_HASH_MISMATCH'; throw }
        $before = Get-SourceSnapshot $root
        Add-VerificationGuardStep $state 'SOURCE-INTEGRITY-PRE' 'OK'
        $tools = Get-VerificationEnvironment $root $state.output
        $envMap = $tools.env
        $state.versions = @{ rust = '1.98.1'; node = '24.13.0'; pnpm = '10.32.1'; python_jsonschema = '4.26.0' }
        Assert-VerificationVersions $state $tools
        Add-VerificationStep $state 'OFFLINE-SAFETY' (Join-Path $PSHOME 'powershell.exe') @('-NoProfile','-NonInteractive','-File',(Join-Path $PSScriptRoot 'test-offline-safety.ps1'),'-OutputRoot',(Join-Path $state.output 'offline-safety')) $envMap
        $checks = @(
            @('GUI-FRONTEND-BUILD',$tools.node,@($tools.pnpm,'--filter','@screenmemory/gui','build')),
            @('RUST-FMT',$tools.rust,@('fmt','--all','--','--check')),
            @('RUST-CLIPPY',$tools.rust,@('clippy','--workspace','--all-targets','--locked','--','-D','warnings')),
            @('RUST-TEST',$tools.rust,@('test','--workspace','--locked')),
            @('HOST-LEASE',$tools.rust,@('test','--locked','-p','sm-isolation','--test','host_lease')),
            @('STORAGE-FOUNDATION',$tools.rust,@('test','--locked','-p','sm-storage')),
            @('TESTS-CODEC',$tools.rust,@('test','--locked','-p','sm-codec')),
            @('TESTS-STORAGE-IMAGES',$tools.rust,@('test','--locked','-p','sm-storage','image_tests')),
            @('CAPTURE-NATIVE',$tools.rust,@('test','--locked','-p','sm-capture','--lib')),
            @('CLI-CAPTURE',$tools.rust,@('test','--locked','-p','sm-cli','--test','capture_surface')),
            @('CREDENTIALS-POLICY',$tools.rust,@('test','--locked','-p','sm-application','--test','credentials')),
            @('CREDENTIALS-NATIVE',$tools.rust,@('test','--locked','-p','sm-credentials')),
            @('FRAMING',$tools.rust,@('test','--locked','-p','sm-framing')),
            @('PIPE-NATIVE',$tools.rust,@('test','--locked','-p','sm-pipe')),
            @('SESSION-NATIVE',$tools.rust,@('test','--locked','-p','sm-session')),
            @('HOST-COMPOSITION',$tools.rust,@('test','--locked','-p','sm-host')),
            @('CLI-SURFACE',$tools.rust,@('test','--locked','-p','sm-cli')),
            @('GUI-RUST-TEST',$tools.rust,@('test','--locked','-p','sm-gui')),
            @('GUI-FRONTEND-TYPECHECK',$tools.node,@($tools.pnpm,'--filter','@screenmemory/gui','typecheck')),
            @('GUI-FRONTEND-LINT',$tools.node,@($tools.pnpm,'--filter','@screenmemory/gui','lint')),
            @('GUI-FRONTEND-TEST',$tools.node,@($tools.pnpm,'--filter','@screenmemory/gui','test')),
            @('PNPM-TYPECHECK',$tools.node,@($tools.pnpm,'typecheck')),
            @('PNPM-LINT',$tools.node,@($tools.pnpm,'lint')),
            @('PNPM-TEST',$tools.node,@($tools.pnpm,'test'))
        )
        foreach ($check in $checks) { $id,$exe,$args = $check; Add-VerificationStep $state $id $exe $args $envMap }
        if (-not $state.failed) {
            $child = Join-Path $state.output 'contracts'
            $ps = Join-Path $PSHOME 'powershell.exe'
            Add-VerificationStep $state 'CONTRACTS' $ps @('-NoProfile','-NonInteractive','-File',(Join-Path $PSScriptRoot 'verify-contracts.ps1'),'-OutputRoot',$child) $envMap
        }
        if (-not $state.failed) {
            $textStart = [DateTime]::UtcNow
            $textWatch = [System.Diagnostics.Stopwatch]::StartNew()
            try { $null = Assert-SourceText $root; $textWatch.Stop(); Add-VerificationLocalStep $state 'SOURCE-TEXT' 'OK' $textStart $textWatch.ElapsedMilliseconds }
            catch { $textWatch.Stop(); Add-VerificationLocalStep $state 'SOURCE-TEXT' 'SOURCE_FORMAT_INVALID' $textStart $textWatch.ElapsedMilliseconds }
        }
    }
} catch {
    if ($state) { Add-VerificationGuardStep $state 'PREFLIGHT' 'PREREQUISITE_OR_RUNNER_ERROR' }
    [Console]::Error.WriteLine('Verification preflight failed; no exception payload disclosed.')
} finally {
    if ($state -and $before -and -not $FixtureCase) {
        try { Assert-PlanHashes $root; Add-VerificationGuardStep $state 'PLAN-HASH-POST' 'OK' }
        catch { Add-VerificationGuardStep $state 'PLAN-HASH-POST' 'PLAN_HASH_MISMATCH' }
        try {
            $after = Get-SourceSnapshot $root
            Add-VerificationGuardStep $state 'SOURCE-INTEGRITY-POST' $(if (Test-SourceSnapshot $before $after) { 'OK' } else { 'SOURCE_MUTATION' })
        } catch { Add-VerificationGuardStep $state 'SOURCE-INTEGRITY-POST' 'SOURCE_INVENTORY_ERROR' }
    }
}
if ($state) { exit (Complete-Verification $state @('BRIDGE','LIVE-RUNTIME','WINDOWS-PACKAGE') @(@{ check_id = 'GUI-WINDOW-RUNTIME'; outcome = 'NOT_RUN'; reason = 'the diagnostic window is deliberately not started by the offline suite; exercise it with scripts/preview-gui.ps1 -Action window' }, @{ check_id = 'CAPTURE-LIVE-DESKTOP'; outcome = 'NOT_RUN'; reason = 'desktop-reading live_capture requires explicit isolated-desktop-tests opt-in on an owned isolated Windows desktop/session' }, @{ check_id = 'FTS5-PRODUCT'; outcome = 'NOT_RUN'; reason = 'S06 lexical corpus/index proof; not foundation acceptance' }, @{ check_id = 'SDK-PACKAGED'; outcome = 'NOT_RUN'; reason = 'S04/S08 SDK/runtime bundling; no model calls in foundation' }, @{ check_id = 'T02-ISOLATION-MENU'; outcome = 'NOT_RUN'; reason = 'donor menu matrix outside offline subset' })) }
exit 1
