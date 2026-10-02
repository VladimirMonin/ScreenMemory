# Compile-only regression: default Cargo must refuse the desktop-reading live target.
# Never enables the opt-in feature, starts capture, or dispatches a live test.
[CmdletBinding()]
param([string]$OutputRoot)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'verification-common.ps1')
$root = Split-Path $PSScriptRoot -Parent
if (-not $OutputRoot) { $OutputRoot = Join-Path $root ('.pi/temporary/' + [guid]::NewGuid().ToString('D')) }
$state = New-VerificationState $root $OutputRoot 'offline-safety'
try {
    $tools = Get-VerificationEnvironment $root $state.output
    if ($tools.env.CARGO_NET_OFFLINE -cne 'true') { throw 'Cargo offline policy missing' }
    Add-VerificationStep $state 'CAPTURE-TARGET-METADATA' $tools.rust @('metadata','--offline','--locked','--no-deps','--format-version','1') $tools.env
    if ($state.failed) { throw 'Cargo metadata failed' }
    $meta = ConvertFrom-Json ([System.IO.File]::ReadAllText((Join-Path $state.output $state.steps[-1].artifact[0].id)))
    $capture = @($meta.packages | Where-Object { $_.name -ceq 'sm-capture' })
    if ($capture.Count -ne 1) { throw 'Capture package missing' }
    $live = @($capture[0].targets | Where-Object { $_.name -ceq 'live_capture' })
    if ($live.Count -ne 1 -or @($live[0].'required-features').Count -ne 1 -or
        $live[0].'required-features'[0] -cne 'isolated-desktop-tests' -or
        @($capture[0].features.default).Count -ne 0) { throw 'Live capture must require an explicit non-default feature' }
    # Actual default workspace inventory is list-only: no test body is dispatched.
    # This also catches accidental feature activation through another workspace owner.
    Add-VerificationStep $state 'DEFAULT-WORKSPACE-TEST-INVENTORY' $tools.rust @('test','--offline','--locked','--workspace','--','--list') $tools.env
    if (-not $state.failed) {
        $inventoryLog = Join-Path $state.output $state.steps[-1].artifact[0].id
        $inventory = [System.IO.File]::ReadAllText($inventoryLog) + [System.IO.File]::ReadAllText($inventoryLog + '.stderr')
        if ($inventory -match 's02_nat_[0-9]|Running[^\r\n]*live_capture') { throw 'Desktop-reading tests appear in default workspace inventory' }
    }
    # --no-run ensures even a broken guard cannot dispatch desktop capture. The old
    # manifest returns 0 here, while the corrected one refuses with Cargo raw exit 101.
    $probe = Invoke-VerificationProcess $tools.rust @('test','--offline','--locked','-p','sm-capture','--test','live_capture','--no-run') $root (Join-Path $state.output 'live-denial.log') 120 $tools.env
    $diagnostic = [System.IO.File]::ReadAllText((Join-Path $state.output 'live-denial.log.stderr'))
    if ($probe.exit -ne 101 -or $diagnostic -notmatch 'requires the features' -or $diagnostic -notmatch 'isolated-desktop-tests') {
        Add-VerificationGuardStep $state 'LIVE-CAPTURE-DEFAULT-DENIAL' 'LIVE_TARGET_NOT_GUARDED'
    } else {
        [void]$state.steps.Add([ordered]@{ check_id = 'LIVE-CAPTURE-DEFAULT-DENIAL'; command = 'cargo';
            args = @('test','--offline','--locked','-p','sm-capture','--test','live_capture','--no-run');
            native_exit = $probe.exit; expected_exit = 101; outcome = 'PASS'; reason = 'REQUIRED_FEATURE_DENIAL';
            artifact = @(@{ id = 'live-denial.log.stderr'; sha256 = (Get-FileHash (Join-Path $state.output 'live-denial.log.stderr')).Hash.ToLowerInvariant() }) })
    }
} catch {
    Add-VerificationGuardStep $state 'OFFLINE-SAFETY' 'UNSAFE_TEST_TARGET_OR_PREREQUISITE'
}
exit (Complete-Verification $state @() @(@{ check_id = 'CAPTURE-LIVE-DESKTOP'; outcome = 'NOT_RUN'; reason = 'requires explicit opt-in and an owned isolated Windows desktop/session; never personal desktop' }))
