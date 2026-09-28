# S01-T06d developer preview of the diagnostic window: one owned test root, the real
# shotd/shot/screenmemory-gui binaries, no personal data, no model, no network.
# This is an explicit development preview, NOT a production first-run installer and NOT part
# of the offline verification suite: it is the only place where the window itself is started.
# `-Action selfcheck` (default) drives the same window client headlessly and prints exactly one
# JSON value with the CLI exit codes; `-Action window` really opens the Tauri window on the
# owned root, closes it like a user would, and proves the same host process outlives it.
[CmdletBinding()]
param(
    [ValidateSet('selfcheck', 'window')][string]$Action = 'selfcheck',
    [string]$PreviewRoot,
    [switch]$SkipBuild,
    [int]$WindowHoldSeconds = 0
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'preview-common.ps1')
$root = Get-PreviewRepositoryRoot $PSScriptRoot

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
    return @{ node = $node; pnpm = $pnpmJs }
}

function Build-PreviewGuiFrontend([string]$RepositoryRoot, [hashtable]$FrontendToolchain) {
    # Own new UUID log root: a frontend build never overwrites another run's evidence.
    $log = Join-Path (New-PreviewBuildLogRoot $RepositoryRoot) 'frontend.log'
    $build = Invoke-PreviewProcess $FrontendToolchain.node @($FrontendToolchain.pnpm, '--filter', '@screenmemory/gui', 'build') $RepositoryRoot 900
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
    foreach ($capability in @('capture', 'library_counts', 'gallery', 'search', 'drop_zone', 'model_settings', 'bridge_runtime')) {
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
    # Only the owned root is named here, and the stop is verified against that root's process.
    $result = Invoke-PreviewProcess $Binaries.shot @('--config', $Prepared.config, 'host', 'stop', '--json') $RepositoryRoot 120
    Write-Output ('[stop] exit=' + $result.exit + ' reason=' + $result.reason)
    if ($result.exit -ne 0) { throw ('Owned host stop returned raw exit ' + $result.exit) }
    if (-not (Wait-OwnedShotdGone $Prepared.root 10)) { throw 'Owned host process did not terminate after the requested stop' }
    Write-Output '[stop] owned host process and endpoint are gone'
}

$toolchain = Get-PreviewToolchain $root
$binaries = $null
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
    $report = Initialize-OwnedPreviewLibrary $root $binaries.shotd $prepared.root
    [void](Write-TrustedPreviewConfig $prepared ([string]$report.library_id) $binaries.shotd)
    $config = ConvertFrom-Json ([System.IO.File]::ReadAllText($prepared.config))
    Write-Output ('owned preview root: ' + $prepared.preview)
}
if (-not (Test-PreviewCanonicalUuid ([string]$config.library_id))) { throw 'Preview configuration library id is not a canonical UUID' }
if ([string]$config.environment -cne 'test') { throw 'The diagnostic window preview requires the explicit test environment' }
if ([string]$config.host_executable -cne $binaries.shotd) { throw 'The preview configuration does not name the built owned host image' }
$libraryId = [string]$config.library_id
Write-Output ('library_id: ' + $libraryId)
Write-Output ''

if ($Action -eq 'selfcheck') {
    $first = Invoke-GuiSelfCheck $binaries $prepared.config $libraryId 'self-check'
    # A second window/check attaches to the same running instance instead of starting another.
    $second = Invoke-GuiSelfCheck $binaries $prepared.config $libraryId 'self-check-reopen'
    if ([string]$first.diagnostics.host.host_instance_id -cne [string]$second.diagnostics.host.host_instance_id) {
        throw 'A repeated window check did not attach to the same host instance'
    }
    Write-Output '[reopen] the repeated check reached the same host instance without another start'
    Stop-OwnedPreviewHost $binaries $prepared $root
    Write-Host ''
    Write-Host 'GUI SELF-CHECK PASS: the window client connected, read real status/doctor and released the host.'
    Write-Host ('ONE command to see the window: powershell -NoProfile -File scripts/preview-gui.ps1 -Action window -PreviewRoot "' + $prepared.preview + '"')
    Write-Host ('Cleanup: powershell -NoProfile -File scripts/preview.ps1 -Action cleanup -PreviewRoot "' + $prepared.preview + '"')
    exit 0
}

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
else { Write-Output '[window] press the window close button to continue (bounded wait 600s)'; $null = $window.WaitForExit(600000) }
# A real close request, exactly like the user's window close button.
if (-not $window.CloseMainWindow()) { throw 'The diagnostic window refused the close request' }
if (-not $window.WaitForExit(30000)) { throw 'The diagnostic window did not exit within the bounded close wait' }
Write-Output ('[window] closed with raw code ' + $window.ExitCode)
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
Write-Output ('ONE command to repeat: powershell -NoProfile -File scripts/preview-gui.ps1 -Action window -PreviewRoot "' + $prepared.preview + '"')
Write-Output ('Cleanup: powershell -NoProfile -File scripts/preview.ps1 -Action cleanup -PreviewRoot "' + $prepared.preview + '"')
exit 0
