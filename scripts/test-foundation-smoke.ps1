# Synthetic composed foundation regression, separate from offline verification.
# No capture command, personal data, clipboard, model or foreign process/window.
[CmdletBinding()]
param([string]$OutputRoot)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'verification-common.ps1')
$repo = Split-Path $PSScriptRoot -Parent
if (-not $OutputRoot) { $OutputRoot = Join-Path $repo ('.pi/temporary/' + [guid]::NewGuid().ToString('D')) }
$state = New-VerificationState $repo $OutputRoot 'foundation-smoke-regression'
try {
    $cases = @(
        @{ id = 'HOST-CLI'; script = 'smoke-host-cli.ps1'; args = @(); expected = 0 },
        @{ id = 'GUI-SELFCHECK'; script = 'preview-gui.ps1'; args = @('-Action','selfcheck','-Cleanup'); expected = 0 },
        @{ id = 'GUI-WINDOW'; script = 'preview-gui.ps1'; args = @('-Action','window','-SkipBuild','-Cleanup'); expected = 0 },
        @{ id = 'GUI-CONNECT-FAILURE-CLEANUP'; script = 'preview-gui.ps1'; args = @('-Action','selfcheck','-SkipBuild','-Cleanup','-FixtureCase','after-connect'); expected = 1 },
        @{ id = 'GUI-WINDOW-FAILURE-CLEANUP'; script = 'preview-gui.ps1'; args = @('-Action','window','-SkipBuild','-Cleanup','-FixtureCase','after-window-open'); expected = 1 },
        @{ id = 'GUI-PREFLIGHT-AND-MUTATION-ISOLATION'; script = 'test-preview-rejection.ps1'; args = @(); expected = 0 }
    )
    foreach ($case in $cases) {
        $childOutput = Join-Path $state.output $case.id
        $args = @('-NoProfile','-NonInteractive','-File',(Join-Path $PSScriptRoot $case.script),'-OutputRoot',$childOutput) + $case.args
        $log = Join-Path $state.output ($case.id + '.log')
        $raw = Invoke-VerificationProcess (Join-Path $PSHOME 'powershell.exe') $args $repo $log 900 @{}
        $ok = $raw.exit -eq $case.expected -and $raw.reason -in @('OK','NATIVE_EXIT')
        if ($ok) {
            $report = ConvertFrom-Json ([System.IO.File]::ReadAllText((Join-Path $childOutput 'result.json')))
            if ($case.script -eq 'preview-gui.ps1') {
                $ok = @($report.steps | Where-Object { $_.check_id -eq 'HOST-PROCESS-GONE' -and $_.outcome -eq 'PASS' }).Count -eq 1 -and
                    @($report.steps | Where-Object { $_.check_id -eq 'OWNED-ROOT-CREDENTIAL-CLEANUP' -and $_.outcome -eq 'PASS' }).Count -eq 1
                if ($case.id -like '*WINDOW*') {
                    $ok = $ok -and @($report.steps | Where-Object { $_.check_id -eq 'GUI-PROCESS-GONE' -and $_.outcome -eq 'PASS' }).Count -eq 1
                }
                if ($case.expected -eq 1) { $ok = $ok -and $report.outcome -eq 'FAIL' -and $report.failure -eq 'SYNTHETIC_FAILURE' }
                else { $ok = $ok -and $report.outcome -eq 'PASS' }
            } else { $ok = $report.outcome -eq 'PASS' }
        }
        [void]$state.steps.Add([ordered]@{ check_id = $case.id; command = 'powershell.exe';
            args = @($args | ForEach-Object { Get-SafeVerificationToken $state $_ }); native_exit = $raw.exit;
            expected_exit = $case.expected; outcome = $(if ($ok) { 'PASS' } else { 'FAIL' });
            artifact = @(@{ id = (Split-Path $log -Leaf); sha256 = (Get-FileHash $log).Hash.ToLowerInvariant() });
            evidence = ($case.id + '/result.json') })
        if (-not $ok) { $state.failed = $true; break }
    }
} catch { Add-VerificationGuardStep $state 'SMOKE-REGRESSION' 'RUNNER_OR_COMPOSED_FAILURE' }
exit (Complete-Verification $state @() @(@{ check_id = 'LIVE-CAPTURE'; outcome = 'NOT_RUN'; reason = 'GUI evidence uses only PrintWindow on the proven owned synthetic HWND; no desktop capture' }))
