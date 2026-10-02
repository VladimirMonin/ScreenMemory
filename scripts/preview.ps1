# S01 developer preview: one owned test root, the real shotd/shot binaries, no personal data.
# This is NOT a production first-run installer: it prepares an explicit owned root under
# .pi\preview, initializes its library, enrolls the synthetic Test-namespace owner credential
# and then drives the real CLI. Nothing outside the owned root and the Test credential
# namespace is touched, and no model or AI call happens.
# `-Action cleanup` acts only on a directory this script created: it must be a direct child of
# .pi\preview with a canonical UUIDv4 name and a matching ownership marker, its library id
# must be a canonical UUID, its owned host must be provably stopped and its exact synthetic
# Test credential must be provably absent before anything is removed. Otherwise it refuses
# with a safe message and a non-zero exit and deletes nothing.
[CmdletBinding()]
param(
    [ValidateSet('all', 'prepare', 'status', 'doctor', 'stop', 'cleanup')][string]$Action = 'all',
    [string]$PreviewRoot,
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'preview-common.ps1')
$root = Get-PreviewRepositoryRoot $PSScriptRoot

function Get-PreviewPreparedBinaries([string]$RepositoryRoot, [hashtable]$Toolchain, [bool]$Reuse) {
    if (-not $Reuse) { return (Build-PreviewBinaries $RepositoryRoot $Toolchain) }
    $binaries = Get-PreviewBinaries $Toolchain
    foreach ($path in @($binaries.shotd, $binaries.shot)) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw 'Preview binaries are missing; run without -SkipBuild' }
    }
    return $binaries
}

function Invoke-PreviewCommand([string]$Label, [string[]]$Arguments, [int]$ExpectedExit) {
    $result = Invoke-PreviewProcess $binaries.shot $Arguments $root 120
    Write-Output ("[$Label] exit=$($result.exit) reason=$($result.reason)")
    if ($result.stdout) { Write-Output $result.stdout.TrimEnd() }
    if ($result.stderr) { Write-Output ('stderr: ' + $result.stderr.TrimEnd()) }
    if ($result.exit -ne $ExpectedExit) { throw "$Label returned raw exit $($result.exit); expected $ExpectedExit" }
}

$owned = $null
if ($Action -eq 'cleanup') {
    if (-not $PreviewRoot) { throw 'cleanup requires -PreviewRoot <owned preview directory>' }
    # Ownership is proven before any build, process or credential action. A foreign path,
    # a similar-looking directory or a missing marker is refused here with no side effect.
    $owned = Resolve-OwnedPreviewRoot $root $PreviewRoot
    if (-not $owned.ok) {
        Write-Output ('[cleanup] refused at ' + $owned.stage + ': ' + $owned.message)
        throw ('Refusing cleanup of an unowned preview root (' + $owned.stage + '): ' + $owned.message)
    }
}

$toolchain = Get-PreviewToolchain $root
$binaries = Get-PreviewPreparedBinaries $root $toolchain ([bool]$SkipBuild)
Write-Output ('built host: ' + $binaries.shotd)
Write-Output ('built cli:  ' + $binaries.shot)
if ($binaries.ContainsKey('build_log')) { Write-Output ('private build log: ' + $binaries.build_log) }

if ($Action -eq 'cleanup') {
    $cleanup = Invoke-OwnedPreviewCleanup $root $owned.preview $binaries.shot
    Write-Output ('[cleanup] stage=' + $cleanup.stage + ' stop_exit=' + $cleanup.stop_exit +
        ' credential_stage=' + $cleanup.credential_stage + ' target=' + $cleanup.target)
    if (-not $cleanup.ok) { throw ('Refusing/aborting cleanup at stage ' + $cleanup.stage + ': ' + $cleanup.message) }
    Write-Output '[cleanup] owned preview root removed'
    exit 0
}

$prepared = $null
if ($PreviewRoot) {
    $prepared = Resolve-OwnedPreviewRoot $root $PreviewRoot
    if (-not $prepared.ok) {
        Write-Output ('[preview] refused at ' + $prepared.stage + ': ' + $prepared.message)
        throw ('Refusing an unowned preview root (' + $prepared.stage + '): ' + $prepared.message)
    }
} else {
    $prepared = New-OwnedPreviewRoot $root
}
$config = $null
if ($PreviewRoot) {
    $config = ConvertFrom-Json ([System.IO.File]::ReadAllText($prepared.config))
} else {
    $report = Initialize-OwnedPreviewLibrary $root $binaries.shotd $prepared.root $prepared
    [void](Write-TrustedPreviewConfig $prepared ([string]$report.library_id) $binaries.shotd)
    $config = ConvertFrom-Json ([System.IO.File]::ReadAllText($prepared.config))
    Write-Output ('owned preview root: ' + $prepared.preview)
    Write-Output ('library_id: ' + [string]$config.library_id)
}
$validated = Resolve-ValidatedPreview $root $prepared.preview $binaries.shotd
if (-not $validated.ok) { throw ('Preview configuration refused at ' + $validated.stage) }
$config = $validated.config_value
$libraryId = $validated.library_id

$oneCommand = Get-PreviewCommandLine $binaries.shot $prepared.config
Write-Output ''
Write-Output 'ONE command to check this developer preview:'
Write-Output ('  ' + $oneCommand)
Write-Output ''

switch ($Action) {
    'prepare' { exit 0 }
    'status' { Invoke-PreviewCommand 'status' @('--config', $prepared.config, 'status', '--json') 0; exit 0 }
    'doctor' { Invoke-PreviewCommand 'doctor' @('--config', $prepared.config, 'doctor', '--json') 0; exit 0 }
    'stop' { Invoke-PreviewCommand 'stop' @('--config', $prepared.config, 'host', 'stop', '--json') 0; exit 0 }
    'all' {
        Invoke-PreviewCommand 'status' @('--config', $prepared.config, 'status', '--json') 0
        Invoke-PreviewCommand 'doctor' @('--config', $prepared.config, 'doctor', '--json') 0
        Invoke-PreviewCommand 'stop' @('--config', $prepared.config, 'host', 'stop', '--json') 0
        if (-not (Wait-OwnedShotdGone $prepared.root 5)) { throw 'Owned host process did not terminate after stop' }
        Write-Output '[stop] owned host process and endpoint are gone'
        Write-Output ''
        Write-Output 'DEVELOPER PREVIEW PASS: real host served status/doctor and stopped on request.'
        Write-Output 'Next commands:'
        Write-Output ('  ' + $oneCommand)
        Write-Output ('  powershell -NoProfile -File scripts/preview.ps1 -Action cleanup -PreviewRoot "' + $prepared.preview + '"')
        Write-Output 'This is an explicit development/test root, not a production first-run installation.'
        exit 0
    }
}
