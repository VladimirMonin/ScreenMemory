# S01-T06d developer preview of the diagnostic window: one owned test root, the real
# shotd/shot/screenmemory-gui binaries, no personal data, no model, no network.
# This is an explicit development preview, NOT a production first-run installer and NOT part
# of the offline verification suite: it is the only place where the window itself is started.
# `-Action selfcheck` (default) drives the same window client headlessly; each binary
# emits one JSON value with CLI exit codes. The wrapper saves evidence. `window` opens the
# owned root, closes it like a user would, and proves the same host process outlives it.
[CmdletBinding()]
param(
    [ValidateSet('selfcheck', 'window')][string]$Action = 'selfcheck',
    [string]$PreviewRoot,
    [switch]$SkipBuild,
    [ValidateRange(0,600)][int]$WindowHoldSeconds = 2,
    [switch]$Cleanup,
    [string]$OutputRoot,
    [ValidateSet('','after-connect','after-window-open','mutate-config','mutate-binding')][string]$FixtureCase = ''
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'preview-common.ps1')
$root = Get-PreviewRepositoryRoot $PSScriptRoot
. (Join-Path $PSScriptRoot 'verification-common.ps1')
if (-not $OutputRoot) { $OutputRoot = Join-Path $root ('.pi/temporary/' + [guid]::NewGuid().ToString('D')) }
$evidence = New-VerificationState $root $OutputRoot 'gui-preview'

function Add-GuiPreviewStep([string]$Id, [bool]$Passed, $RawExit = $null) {
    [void]$evidence.steps.Add([ordered]@{ check_id = $Id; native_exit = $RawExit; outcome = $(if ($Passed) { 'PASS' } else { 'FAIL' }) })
    if (-not $Passed) { $evidence.failed = $true }
}

function Get-PreviewGuiFrontendToolchain([string]$RepositoryRoot) {
    # Pinned project-local Node plus the explicitly installed local package tools; no global
    # install, no download and no network access happens here.
    $node = Join-Path $RepositoryRoot '.pi\toolchains\node-v24.13.0-win-x64\node.exe'
    if (-not (Test-Path -LiteralPath $node -PathType Leaf)) { throw 'Missing pinned Node.js prerequisite; prepare prerequisites first' }
    $pnpm = Get-Command pnpm.cmd -ErrorAction SilentlyContinue
    if (-not $pnpm) { throw 'Missing pnpm prerequisite' }
    $pnpmJs = Join-Path (Split-Path $pnpm.Source -Parent) 'node_modules\pnpm\bin\pnpm.cjs'
    if (-not (Test-Path -LiteralPath $pnpmJs)) { throw 'Missing pnpm package prerequisite' }
    foreach ($tool in @('tsc.CMD', 'vite.CMD')) {
        if (-not (Test-Path (Join-Path $RepositoryRoot ('apps\gui\node_modules\.bin\' + $tool)))) {
            throw 'Missing installed gui package prerequisite; bootstrap explicitly'
        }
    }
    $map = @{ PATH = ((Split-Path $node -Parent) + ';' + $env:PATH); NODE_OPTIONS = ''; COREPACK_ENABLE_AUTO_PIN = '0' }
    foreach ($pin in @(@(@('--version'),'v24.13.0'), @(@($pnpmJs,'--version'),'10.32.1'))) {
        $version = Invoke-PreviewProcess $node $pin[0] $RepositoryRoot 30 -EnvironmentMap $map
        if ($version.exit -ne 0 -or $version.stdout.Trim() -cne $pin[1]) { throw 'Pinned frontend version mismatch' }
    }
    return @{ node = $node; pnpm = $pnpmJs; env = $map }
}

function Build-PreviewGuiFrontend([string]$RepositoryRoot, [hashtable]$FrontendToolchain) {
    # Own new UUID log root: a frontend build never overwrites another run's evidence.
    $log = Join-Path (New-PreviewBuildLogRoot $RepositoryRoot) 'frontend.log'
    $build = Invoke-PreviewProcess $FrontendToolchain.node @($FrontendToolchain.pnpm, '--filter', '@screenmemory/gui', 'build') $RepositoryRoot 900 -EnvironmentMap $FrontendToolchain.env
    [System.IO.File]::WriteAllText($log, ("exit=$($build.exit) reason=$($build.reason)`n" + [string]$build.stdout + [string]$build.stderr), (New-Object System.Text.UTF8Encoding($false)))
    if ($build.exit -ne 0 -or -not (Test-Path -LiteralPath (Join-Path $RepositoryRoot 'apps\gui\dist\index.html') -PathType Leaf)) {
        throw ('Gui frontend build failed; raw exit ' + $build.exit + '; private build log ' + $log)
    }
    return $log
}

function Assert-GuiSelfCheckReport($Report, [string]$LibraryId, [string]$Label) {
    if ([string]$Report.mode -cne 'gui-self-check') { throw "$Label reported an unexpected mode" }
    if ([int]$Report.exit_code -ne 0) { throw "$Label reported a non-zero JSON exit code" }
    if ($null -ne $Report.error) { throw "$Label reported an error object on success" }
    $diagnostics = $Report.diagnostics
    if ($null -eq $diagnostics) { throw "$Label returned no diagnostics" }
    if ([string]$diagnostics.state -cnotin @('ready', 'runtime_unavailable')) { throw "$Label did not reach a connected foundation state" }
    if ($null -eq $diagnostics.host) { throw "$Label reported no verified host identity" }
    if ([string]$diagnostics.host.library_id -cne $LibraryId) { throw "$Label reported another library identity" }
    if (-not (Test-PreviewCanonicalUuid ([string]$diagnostics.host.host_instance_id))) { throw "$Label reported a non-canonical host instance id" }
    if ([int]$diagnostics.host.protocol.major -ne 1) { throw "$Label reported an unexpected protocol major" }
    if ($diagnostics.runtime_available -ne $false) { throw "$Label claims a usable AI runtime while AI is disabled" }
    if ($diagnostics.report_available -ne $true) { throw "$Label reported an unavailable diagnostic report directory" }
    if ($null -ne $diagnostics.safe_code) { throw "$Label reported a safe code on success" }
    # S02 capture and gallery are implemented; neither may be reported unfinished.
    foreach ($implemented in @('capture', 'gallery')) {
        if ($diagnostics.not_implemented -ccontains $implemented) { throw "$Label still reports the implemented $implemented capability as unfinished" }
    }
    foreach ($capability in @('search', 'knowledge_pipeline')) {
        if ($diagnostics.not_implemented -cnotcontains $capability) { throw "$Label does not report the unfinished capability $capability" }
    }
    if ($null -eq $diagnostics.doctor) { throw "$Label did not run the foundation doctor" }
    $checks = @($diagnostics.doctor.checks)
    if (@($checks | Where-Object { $_.outcome -eq 'FAILED' }).Count -ne 0) { throw "$Label reported a failed mandatory foundation check" }
    if (@($checks | Where-Object { $_.check_id -eq 'foundation.storage_schema' -and $_.outcome -eq 'PASSED' }).Count -ne 1) {
        throw "$Label did not pass the storage schema check"
    }
    if (@($checks | Where-Object { $_.check_id -eq 'runtime.lmstudio' -and $_.outcome -eq 'NOT_RUN' }).Count -ne 1) {
        throw "$Label did not keep the AI runtime check NOT_RUN"
    }
}

function Invoke-GuiSelfCheck([hashtable]$Binaries, [string]$Config, [string]$LibraryId, [string]$Label) {
    $result = Invoke-PreviewProcess $Binaries.gui @('--self-check', '--config', $Config) $root 180
    $index = $evidence.steps.Count + 1
    [System.IO.File]::WriteAllText((Join-Path $evidence.output ("native-$index.log")), [string]$result.stdout, $script:Utf8)
    [System.IO.File]::WriteAllText((Join-Path $evidence.output ("native-$index.log.stderr")), [string]$result.stderr, $script:Utf8)
    Add-GuiPreviewStep $Label ($result.exit -eq 0 -and $result.reason -eq 'OK') $result.exit
    $lines = @($result.stdout -split "`r?`n" | Where-Object { $_ -ne '' })
    # Progress goes to the host, so the only value this function returns is its report.
    Write-Host ("[$Label] exit=$($result.exit) reason=$($result.reason) stdout_lines=$($lines.Count)")
    if ($result.exit -ne 0) { throw ("$Label returned raw exit $($result.exit); expected 0") }
    if ($lines.Count -ne 1) { throw "$Label must print exactly one JSON value on stdout" }
    $report = ConvertFrom-Json $lines[0]
    Assert-GuiSelfCheckReport $report $LibraryId $Label
    Write-Host ("[$Label] library_id=" + [string]$report.diagnostics.host.library_id +
        ' host_instance_id=' + [string]$report.diagnostics.host.host_instance_id +
        ' state=' + [string]$report.diagnostics.state)
    return $report
}

function Stop-OwnedPreviewHost([hashtable]$Binaries, [hashtable]$Prepared, [string]$RepositoryRoot) {
    # Revalidate frozen authority before issuing any stop through a mutable config.
    $current = Resolve-ValidatedPreview $RepositoryRoot $Prepared.preview $Binaries.shotd $Prepared
    if (-not $current.ok) { throw 'Owned stop refused: validated inputs changed' }
    $result = Invoke-PreviewProcess $Binaries.shot @('--config', $Prepared.config, 'host', 'stop', '--json') $RepositoryRoot 120
    Add-GuiPreviewStep 'HOST-STOP' ($result.exit -eq 0) $result.exit
    Write-Output ('[stop] exit=' + $result.exit + ' reason=' + $result.reason)
    if ($result.exit -ne 0) { throw ('Owned host stop returned raw exit ' + $result.exit) }
    if (-not (Wait-OwnedShotdGone $Prepared.root 10)) { throw 'Owned host process did not terminate after the requested stop' }
    Write-Output '[stop] owned host process and endpoint are gone'
}

$binaries = $null
$prepared = $null
$cleanupAuthority = $null
$window = $null
$failure = $null
try {
$toolchain = Get-PreviewToolchain $root
if ($SkipBuild) {
    $base = Get-PreviewBinaries $toolchain
    $binaries = @{ shotd = $base.shotd; shot = $base.shot; gui = (Get-PreviewGuiBinary $toolchain) }
    foreach ($path in @($binaries.shotd, $binaries.shot, $binaries.gui)) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw 'Gui preview binaries are missing; run without -SkipBuild' }
    }
} else {
    $frontendToolchain = Get-PreviewGuiFrontendToolchain $root
    $frontendLog = Build-PreviewGuiFrontend $root $frontendToolchain
    Write-Output ('private frontend build log: ' + $frontendLog)
    $binaries = Build-PreviewGuiBinaries $root $toolchain
}
Write-Output ('built host:   ' + $binaries.shotd)
Write-Output ('built cli:    ' + $binaries.shot)
Write-Output ('built window: ' + $binaries.gui)
if ($binaries.ContainsKey('build_log')) { Write-Output ('private build log: ' + $binaries.build_log) }

# Ownership is proven before any process or credential action: an unowned, tampered or
# similarly named directory is refused with a safe message and nothing is deleted.
$prepared = $null
if ($PreviewRoot) {
    $prepared = Resolve-OwnedPreviewRoot $root $PreviewRoot
    if (-not $prepared.ok) { throw ('Refusing an unowned preview root (' + $prepared.stage + '): ' + $prepared.message) }
    $config = ConvertFrom-Json ([System.IO.File]::ReadAllText($prepared.config))
} else {
    $prepared = New-OwnedPreviewRoot $root
    $report = Initialize-OwnedPreviewLibrary $root $binaries.shotd $prepared.root $prepared
    [void](Write-TrustedPreviewConfig $prepared ([string]$report.library_id) $binaries.shotd)
    $config = ConvertFrom-Json ([System.IO.File]::ReadAllText($prepared.config))
    Write-Output ('owned preview root: ' + $prepared.preview)
}
$validated = Resolve-ValidatedPreview $root $prepared.preview $binaries.shotd
if (-not $validated.ok) { throw ('Preview preflight refused at ' + $validated.stage) }
# Only a fully bound root may arm finalization. Freeze before the first connect;
# authentication of a different already-running library is not root ownership.
$cleanupAuthority = $validated
$prepared = $validated
$config = $validated.config_value
$libraryId = $validated.library_id
Write-Output ('library_id: ' + $libraryId)
Write-Output ''

if ($Action -eq 'selfcheck') {
    $first = Invoke-GuiSelfCheck $binaries $prepared.config $libraryId 'self-check'
    if ($FixtureCase -eq 'after-connect') { throw 'Synthetic failure after connect' }
    if ($FixtureCase -in @('mutate-config','mutate-binding')) {
        $mutable = $(if ($FixtureCase -eq 'mutate-config') { $prepared.config } else { Join-Path $prepared.preview (Get-PreviewLibraryBindingName) })
        # Semantically unchanged, deliberately different bytes: test frozen hashes,
        # not only tuple equality. Own synthetic fixture input, never another root.
        [System.IO.File]::AppendAllText($mutable,"`n",$script:Utf8)
        throw 'Synthetic input mutation after connect'
    }
    # A second window/check attaches to the same running instance instead of starting another.
    $second = Invoke-GuiSelfCheck $binaries $prepared.config $libraryId 'self-check-reopen'
    if ([string]$first.diagnostics.host.host_instance_id -cne [string]$second.diagnostics.host.host_instance_id) {
        throw 'A repeated window check did not attach to the same host instance'
    }
    Write-Output '[reopen] the repeated check reached the same host instance without another start'
    Stop-OwnedPreviewHost $binaries $prepared $root
    Write-Host ''
    Write-Host 'GUI SELF-CHECK PASS: the window client connected, read real status/doctor and released the host.'
    if (-not $Cleanup) {
        Write-Host ('ONE command to see the window: powershell -NoProfile -File scripts/preview-gui.ps1 -Action window -PreviewRoot "' + $prepared.preview + '"')
        Write-Host ('Cleanup: powershell -NoProfile -File scripts/preview.ps1 -Action cleanup -PreviewRoot "' + $prepared.preview + '"')
    }
} else {

# The window really starts and shows the typed client state of the same owned host. The owned
# host of this preview is already running (started by the headless check below), so opening and
# closing the window can be proven to leave exactly that process alive and reusable.
$before = Invoke-GuiSelfCheck $binaries $prepared.config $libraryId 'self-check-before-window'
$hostBefore = Get-OwnedShotdProcess $prepared.root
if ($hostBefore.Count -ne 1) { throw 'The owned host of this preview is not exactly one process before the window opens' }
$hostPid = $hostBefore[0].ProcessId
$instance = [string]$before.diagnostics.host.host_instance_id
Write-Output ('[window] owned host pid=' + $hostPid + ' instance=' + $instance)

$window = Start-Process -FilePath $binaries.gui -ArgumentList @('--config', ('"' + $prepared.config + '"')) -PassThru
if ($FixtureCase -eq 'after-window-open') { throw 'Synthetic failure after spawning the owned window' }
$deadline = (Get-Date).AddSeconds(30)
$windowProcess = $null
while ((Get-Date) -lt $deadline) {
    if ($window.HasExited) { throw ('The window process exited with raw code ' + $window.ExitCode + ' before showing a window') }
    $windowProcess = Get-Process -Id $window.Id -ErrorAction SilentlyContinue
    if ($windowProcess -and $windowProcess.MainWindowTitle) { break }
    Start-Sleep -Milliseconds 200
}
if (-not $windowProcess -or -not $windowProcess.MainWindowTitle) { throw 'The diagnostic window did not appear within the bounded wait' }
Write-Output ('[window] pid=' + $window.Id + ' title="' + $windowProcess.MainWindowTitle + '" responding=' + $windowProcess.Responding)
$hostDuring = Get-OwnedShotdProcess $prepared.root
if ($hostDuring.Count -ne 1 -or $hostDuring[0].ProcessId -ne $hostPid) { throw 'The window did not attach to the one owned host process' }

if ($WindowHoldSeconds -gt 0) { Start-Sleep -Seconds $WindowHoldSeconds }
$capture = Invoke-PreviewProcess (Join-Path $PSHOME 'powershell.exe') @('-NoProfile','-NonInteractive','-File',(Join-Path $PSScriptRoot 'capture-preview-window.ps1'),'-OwnedProcessId',[string]$window.Id,'-PreviewRoot',$prepared.preview,'-OutputRoot',(Join-Path $evidence.output 'window')) $root 60
Add-GuiPreviewStep 'OWNED-HWND-PHYSICAL-EVIDENCE' ($capture.exit -eq 0) $capture.exit
if ($capture.exit -ne 0) { throw 'Owned window physical evidence failed' }
# Agent-driven bounded close, never asks the owner to click.
# A user-closed window is already gone; only request close if it is still running.
if (-not $window.HasExited -and -not $window.CloseMainWindow()) { throw 'The diagnostic window refused the close request' }
if (-not $window.WaitForExit(30000)) { throw 'The diagnostic window did not exit within the bounded close wait' }
Write-Output ('[window] closed with raw code ' + $window.ExitCode)
Add-GuiPreviewStep 'GUI-WINDOW-CLOSE' ($window.ExitCode -eq 0) $window.ExitCode
if ($window.ExitCode -ne 0) { throw 'The closed window returned a non-zero exit code' }
$hostAfter = Get-OwnedShotdProcess $prepared.root
if ($hostAfter.Count -ne 1 -or $hostAfter[0].ProcessId -ne $hostPid) {
    throw 'Closing the window did not leave the same owned host process running'
}
Write-Output ('[window] the same owned host pid=' + $hostPid + ' outlived the window')
# A reopened client reaches that already running instance instead of starting a second host.
$after = Invoke-GuiSelfCheck $binaries $prepared.config $libraryId 'self-check-after-window'
$hostFinal = Get-OwnedShotdProcess $prepared.root
if ($hostFinal.Count -ne 1 -or $hostFinal[0].ProcessId -ne $hostPid) {
    throw 'The reopened window check did not reuse the same owned host process'
}
if ([string]$after.diagnostics.host.host_instance_id -cne $instance) {
    throw 'The reopened window check reached another host instance'
}
Write-Output ('[window] the reopened check attached to the same host_instance_id=' + $instance)
Stop-OwnedPreviewHost $binaries $prepared $root
Write-Output ''
Write-Output 'GUI WINDOW PASS: the window opened on the owned root, the host outlived its close and the reopened client reused it.'
if (-not $Cleanup) {
    Write-Output ('ONE command to repeat: powershell -NoProfile -File scripts/preview-gui.ps1 -Action window -PreviewRoot "' + $prepared.preview + '"')
    Write-Output ('Cleanup: powershell -NoProfile -File scripts/preview.ps1 -Action cleanup -PreviewRoot "' + $prepared.preview + '"')
}
}
} catch {
    $evidence.failed = $true
    $failure = $(if ($FixtureCase) { 'SYNTHETIC_FAILURE' } else { 'GUI_PREVIEW_FAILED' })
    [Console]::Error.WriteLine('GUI preview failed; private evidence retained, no exception payload disclosed.')
} finally {
    # Closing a client never authorizes leaving an owned window/host orphan on error.
    if ($window) {
        try {
            if (-not $window.HasExited) {
                [void]$window.CloseMainWindow()
                if (-not $window.WaitForExit(10000)) { $window.Kill(); [void]$window.WaitForExit(5000) }
            }
            Add-GuiPreviewStep 'GUI-PROCESS-GONE' $window.HasExited
        } catch { Add-GuiPreviewStep 'GUI-PROCESS-GONE' $false }
        finally { $window.Dispose() }
    }
    if ($cleanupAuthority -and $binaries) {
        $current = Resolve-ValidatedPreview $root $cleanupAuthority.preview $binaries.shotd $cleanupAuthority
        if (-not $current.ok) {
            Add-GuiPreviewStep 'OWNED-FINALIZATION-REFUSED' $false
            $evidence.steps[-1].cleanup_stage = $current.stage
            $evidence.steps[-1].local_host = 'BLOCKED: preserved because config/binding changed'
        } else {
            try {
                if ($Cleanup) {
                    $clean = Invoke-OwnedPreviewCleanup $root $cleanupAuthority.preview $binaries.shot 120 $cleanupAuthority
                    Add-GuiPreviewStep 'OWNED-ROOT-CREDENTIAL-CLEANUP' ([bool]$clean.ok) $clean.credential_exit
                    $evidence.steps[-1].readback_code = $clean.credential_readback_code
                    $evidence.steps[-1].cleanup_stage = $clean.stage
                    Add-GuiPreviewStep 'HOST-PROCESS-GONE' ([bool]$clean.ok -and (Test-OwnedPreviewQuiescent $prepared.root))
                } else {
                    if (-not (Test-OwnedPreviewQuiescent $prepared.root)) { Stop-OwnedPreviewHost $binaries $prepared $root }
                    Add-GuiPreviewStep 'HOST-PROCESS-GONE' (Wait-OwnedShotdGone $prepared.root 10)
                }
            } catch { Add-GuiPreviewStep 'OWNED-FINALIZATION' $false }
        }
    }
    $result = [ordered]@{ schema_version = 1; suite = 'gui-preview'; stage_result = 'IN_PROGRESS';
        library_id = $libraryId; host_instance_id = $(if ($first) { $first.diagnostics.host.host_instance_id } elseif ($before) { $before.diagnostics.host.host_instance_id } else { $null });
        outcome = $(if ($evidence.failed) { 'FAIL' } else { 'PASS' }); failure = $failure;
        action = $Action; preview = $(if ($prepared) { Get-SafeVerificationToken $evidence $prepared.preview } else { $null });
        steps = @($evidence.steps.ToArray()); capture = 'NOT_RUN'; ai_runtime = 'NOT_RUN';
        raw_logs = 'private .pi output only; do not upload automatically' }
    Write-VerificationJson (Join-Path $evidence.output 'result.json') $result
}
if ($evidence.failed) { exit 1 }
exit 0
