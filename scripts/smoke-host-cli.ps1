# S01 explicit synthetic executable smoke for the real shotd/shot binaries.
# Deliberately NOT part of scripts/verify.ps1: it starts the product host on an owned
# Test-namespace root with fixed built images. No model, no capture, no personal data,
# no network, no foreign process is started or terminated.
# Evidence is written into a new immutable .pi output root; the exact synthetic Test
# credential of the owned library is removed and its absence verified at the end.
[CmdletBinding()]
param([string]$OutputRoot)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'preview-common.ps1')
$utf8 = New-Object System.Text.UTF8Encoding($false)
$repo = Get-PreviewRepositoryRoot $PSScriptRoot
. (Join-Path $PSScriptRoot 'verification-common.ps1')
if (-not $OutputRoot) { $OutputRoot = Join-Path $repo ('.pi/temporary/' + [guid]::NewGuid().ToString('D')) }
$outputState = New-VerificationState $repo $OutputRoot 'host-cli-smoke'
$OutputRoot = $outputState.output
$toolchain = Get-PreviewToolchain $repo
$binaries = Build-PreviewBinaries $repo $toolchain

$steps = New-Object System.Collections.ArrayList
$notes = New-Object System.Collections.ArrayList
$failed = $null
$preview = $null
$absent = $null
$libraryId = $null
$credentialTarget = $null

function Get-SafeArguments([string[]]$Arguments) {
    return @($Arguments | ForEach-Object { $_.Replace($repo, '@REPO').Replace($OutputRoot, '@OUT') })
}

function Invoke-SmokeStep([string]$Id, [string]$Executable, [string[]]$Arguments, [int]$TimeoutSeconds, [int]$ExpectedExit) {
    $result = Invoke-PreviewProcess $Executable $Arguments $repo $TimeoutSeconds
    $index = $steps.Count + 1
    $outLog = Join-Path $OutputRoot ('native-{0:d2}-stdout.log' -f $index)
    $errLog = Join-Path $OutputRoot ('native-{0:d2}-stderr.log' -f $index)
    [System.IO.File]::WriteAllText($outLog, [string]$result.stdout, $utf8)
    [System.IO.File]::WriteAllText($errLog, [string]$result.stderr, $utf8)
    $ok = ($result.reason -eq 'OK' -and $result.exit -eq $ExpectedExit)
    [void]$steps.Add([ordered]@{
            check_id = $Id
            command = (Split-Path $Executable -Leaf)
            args = Get-SafeArguments $Arguments
            reason = $result.reason
            native_exit = $result.exit
            expected_exit = $ExpectedExit
            duration_ms = $result.duration_ms
            outcome = $(if ($ok) { 'PASS' } else { 'FAIL' })
            stdout_log = (Split-Path $outLog -Leaf)
            stderr_log = (Split-Path $errLog -Leaf)
        })
    if (-not $ok) { throw "$Id returned raw exit $($result.exit); expected $ExpectedExit" }
    return $result
}

function Assert-SmokeTrue([bool]$Condition, [string]$Message, [string]$Id) {
    [void]$steps.Add([ordered]@{
            check_id = $Id
            command = 'assertion'
            args = @()
            reason = $Message
            native_exit = $null
            expected_exit = $null
            duration_ms = 0
            outcome = $(if ($Condition) { 'PASS' } else { 'FAIL' })
            stdout_log = $null
            stderr_log = $null
        })
    if (-not $Condition) { throw $Message }
}

try {
    Write-Output ('smoke output root: ' + $OutputRoot)
    Write-Output ('host image: ' + $binaries.shotd)
    Write-Output ('cli image:  ' + $binaries.shot)
    if ($binaries.ContainsKey('build_log')) { Write-Output ('private build log: ' + $binaries.build_log) }

    $preview = New-OwnedPreviewRoot $repo
    $init = Invoke-SmokeStep 'PREVIEW-INIT' $binaries.shotd @('--init', '--root', $preview.root) 120 0
    $report = ConvertFrom-Json $init.stdout
    $libraryId = [string]$report.library_id
    Assert-SmokeTrue (Test-PreviewCanonicalUuid $libraryId) 'first-run library identity is not a canonical UUID' 'LIBRARY-IDENTITY'
    $preview.initialized_library_id = $libraryId # this own root's successful --init above
    [void](Write-TrustedPreviewConfig $preview $libraryId $binaries.shotd)
    # Current foundation plus S02 image/recovery/list migrations; config schema stays 1.
    Assert-SmokeTrue ([int]$report.schema_version -eq 4) 'first-run storage schema version is not 4' 'SCHEMA-VERSION'
    Assert-SmokeTrue ([int]$report.format_major -eq 1) 'first-run format major is not 1' 'FORMAT-MAJOR'
    Assert-SmokeTrue (Test-Path -LiteralPath (Join-Path $preview.root 'data\library.sqlite3') -PathType Leaf) 'library database was not created in the owned root' 'DATABASE-CREATED'

    # 1) CLI status with the fixed trusted host image: real autostart, real handshake.
    $status1 = Invoke-SmokeStep 'CLI-STATUS-AUTOSTART' $binaries.shot @('--config', $preview.config, 'status', '--json') 120 0
    $first = ConvertFrom-Json $status1.stdout
    Assert-SmokeTrue ([string]$first.library_id -eq $libraryId) 'CLI status library id differs from the first-run identity' 'STATUS-LIBRARY-ID'
    Assert-SmokeTrue ([string]$first.state -eq 'ready') 'CLI status did not report a ready host' 'STATUS-READY'
    Assert-SmokeTrue ($first.ai_enabled -eq $false) 'AI must be disabled in the foundation preview' 'STATUS-AI-OFF'
    Assert-SmokeTrue ([string]$first.bridge_state -eq 'stopped') 'bridge must be stopped in the foundation preview' 'STATUS-BRIDGE-STOPPED'
    $hostInstance = [string]$first.host_instance_id
    Assert-SmokeTrue ($hostInstance.Length -gt 0) 'CLI status reported no host instance id' 'STATUS-HOST-ID'
    Assert-SmokeTrue ((Get-OwnedShotdProcess $preview.root).Count -eq 1) 'no owned host process is running after autostart' 'HOST-PROCESS-RUNNING'

    # 2) A second status command must reach the same single owner and library identity.
    $status2 = Invoke-SmokeStep 'CLI-STATUS-SECOND' $binaries.shot @('--config', $preview.config, 'status', '--json') 120 0
    $second = ConvertFrom-Json $status2.stdout
    Assert-SmokeTrue ([string]$second.host_instance_id -eq $hostInstance) 'a second CLI call reached a different host instance' 'SINGLE-OWNER'
    Assert-SmokeTrue ([string]$second.library_id -eq $libraryId) 'a second CLI call reported a different library' 'SINGLE-LIBRARY'

    # 3) Doctor must report the same identity with real mandatory checks and honest gaps.
    $doctor = Invoke-SmokeStep 'CLI-DOCTOR' $binaries.shot @('--config', $preview.config, 'doctor', '--json') 240 0
    $doctorJson = ConvertFrom-Json $doctor.stdout
    Assert-SmokeTrue ([string]$doctorJson.host.host_instance_id -eq $hostInstance) 'doctor host instance differs from status' 'DOCTOR-HOST-ID'
    Assert-SmokeTrue ([string]$doctorJson.host.library_id -eq $libraryId) 'doctor library differs from status' 'DOCTOR-LIBRARY-ID'
    $checks = @($doctorJson.checks)
    foreach ($id in @('foundation.root_identity', 'foundation.host_lease', 'foundation.credential_store',
            'foundation.storage_schema', 'foundation.sqlite_runtime', 'foundation.log_sink')) {
        $check = @($checks | Where-Object { $_.check_id -eq $id })
        Assert-SmokeTrue ($check.Count -eq 1 -and [string]$check[0].outcome -eq 'PASSED') "mandatory doctor check $id is not PASSED" 'DOCTOR-MANDATORY'
    }
    Assert-SmokeTrue (@($checks | Where-Object { [string]$_.outcome -eq 'FAILED' }).Count -eq 0) 'doctor reported a failed mandatory check' 'DOCTOR-NO-FAILURE'
    $runtime = @($checks | Where-Object { $_.check_id -eq 'runtime.lmstudio' })
    Assert-SmokeTrue ($runtime.Count -eq 1 -and [string]$runtime[0].outcome -eq 'NOT_RUN') 'runtime inventory must be honestly NOT_RUN' 'DOCTOR-RUNTIME-NOT-RUN'
    $bridge = @($checks | Where-Object { $_.check_id -eq 'bridge.supervisor' })
    Assert-SmokeTrue ($bridge.Count -eq 1 -and [string]$bridge[0].outcome -eq 'UNSUPPORTED') 'bridge must be honestly unsupported' 'DOCTOR-BRIDGE-UNSUPPORTED'

    # 4) A duplicated start of the same root must be refused; the first owner stays alive.
    $duplicate = Invoke-SmokeStep 'DUPLICATE-START-REFUSED' $binaries.shotd @('--root', $preview.root) 60 4
    Assert-SmokeTrue ([string]$duplicate.stderr -match 'host_lock_conflict') 'duplicate start did not report a safe lock conflict' 'DUPLICATE-SAFE-DIAGNOSTIC'
    $status3 = Invoke-SmokeStep 'CLI-STATUS-AFTER-DUPLICATE' $binaries.shot @('--config', $preview.config, 'status', '--json') 120 0
    $third = ConvertFrom-Json $status3.stdout
    Assert-SmokeTrue ([string]$third.host_instance_id -eq $hostInstance) 'duplicate start replaced the owner' 'DUPLICATE-NO-SECOND-WRITER'

    # 5) --no-start on a root without a host: one error object, exit 4, no side effect.
    $absent = New-OwnedPreviewRoot $repo
    $absentInit = Initialize-OwnedPreviewLibrary $repo $binaries.shotd $absent.root $absent
    $absentLibrary = [string]$absentInit.library_id
    # Persist only known own identity (never Blob/UserName), before any destructive
    # path, so a failed future harness can reconcile its exact target without a sweep.
    [System.IO.File]::WriteAllText((Join-Path $OutputRoot 'own-init-identities.json'),
        (([ordered]@{ primary_library_id=$libraryId; absent_library_id=$absentLibrary } | ConvertTo-Json) + "`n"),$utf8)
    [void](Write-TrustedPreviewConfig $absent $absentLibrary $binaries.shotd)
    $absentDatabase = Join-Path $absent.root 'data\library.sqlite3'
    $absentDatabaseHash = (Get-FileHash -LiteralPath $absentDatabase -Algorithm SHA256).Hash
    $absentLock = Join-Path $absent.root 'data\host.lock'
    $absentLockHash = (Get-FileHash -LiteralPath $absentLock -Algorithm SHA256).Hash
    $noStart = Invoke-SmokeStep 'CLI-NO-START-ABSENT' $binaries.shot @('--config', $absent.config, 'status', '--no-start', '--json') 60 4
    $noStartJson = ConvertFrom-Json $noStart.stdout
    Assert-SmokeTrue ([string]$noStartJson.code -eq 'RUNTIME_UNAVAILABLE') '--no-start did not return the safe error object' 'NO-START-ERROR-OBJECT'
    Assert-SmokeTrue ((Get-FileHash -LiteralPath $absentLock -Algorithm SHA256).Hash -ceq $absentLockHash) '--no-start changed the init-created lock file' 'NO-START-NO-LOCK-MUTATION'
    Assert-SmokeTrue ((Get-FileHash -LiteralPath $absentDatabase -Algorithm SHA256).Hash -ceq $absentDatabaseHash) '--no-start changed the prepared library database' 'NO-START-NO-DATABASE-MUTATION'
    Assert-SmokeTrue ((Get-OwnedShotdProcess $absent.root).Count -eq 0) '--no-start started a host process' 'NO-START-NO-PROCESS'

    # 6) Owner stop: receipt is Stopping, not a false handle-closure claim.
    $stop = Invoke-SmokeStep 'CLI-HOST-STOP' $binaries.shot @('--config', $preview.config, 'host', 'stop', '--json') 120 0
    $receipt = ConvertFrom-Json $stop.stdout
    Assert-SmokeTrue ([string]$receipt.state -eq 'stopping') 'stop receipt did not report the stopping state' 'STOP-RECEIPT'
    Assert-SmokeTrue ([string]$receipt.host_instance_id -eq $hostInstance) 'stop receipt reported a different host instance' 'STOP-RECEIPT-HOST-ID'
    Assert-SmokeTrue (Wait-OwnedShotdGone $preview.root 5) 'owned host process did not terminate within the bounded window' 'STOP-PROCESS-GONE'
    $afterStop = Invoke-SmokeStep 'CLI-NO-START-AFTER-STOP' $binaries.shot @('--config', $preview.config, 'status', '--no-start', '--json') 60 4
    Assert-SmokeTrue ([string](ConvertFrom-Json $afterStop.stdout).code -eq 'RUNTIME_UNAVAILABLE') 'the endpoint survived the stop' 'STOP-ENDPOINT-GONE'

    # 7) Reopen the same library: same durable identity, a new host instance.
    $restart = Invoke-SmokeStep 'CLI-STATUS-RESTART' $binaries.shot @('--config', $preview.config, 'status', '--json') 120 0
    $restarted = ConvertFrom-Json $restart.stdout
    Assert-SmokeTrue ([string]$restarted.library_id -eq $libraryId) 'restart changed the durable library identity' 'RESTART-LIBRARY-ID'
    Assert-SmokeTrue ([string]$restarted.host_instance_id -ne $hostInstance) 'restart reused the previous host instance id' 'RESTART-NEW-HOST-ID'

    # 8) No AI or runtime work: no child process of the owned host, AI off, bridge stopped.
    $owned = Get-OwnedShotdProcess $preview.root
    Assert-SmokeTrue ($owned.Count -eq 1) 'no owned host process is running after restart' 'RESTART-PROCESS-RUNNING'
    $children = @()
    foreach ($process in $owned) {
        $children += @(Get-CimInstance Win32_Process -Filter ("ParentProcessId = " + [int]$process.ProcessId))
    }
    Assert-SmokeTrue ($children.Count -eq 0) 'the owned host started a child process (no bridge/model child is expected)' 'NO-CHILD-PROCESS'
    Assert-SmokeTrue ($restarted.ai_enabled -eq $false -and [string]$restarted.bridge_state -eq 'stopped') 'AI or bridge state is not off/stopped' 'NO-AI-STATE'
    [void]$notes.Add('The workspace dependency boundary suite proves no model, SDK, HTTP client or network crate is linked.')

    # 9) Stop again and clean up only this owned root and its exact Test credential.
    $finalStop = Invoke-SmokeStep 'CLI-HOST-STOP-FINAL' $binaries.shot @('--config', $preview.config, 'host', 'stop', '--json') 120 0
    Assert-SmokeTrue ([string](ConvertFrom-Json $finalStop.stdout).state -eq 'stopping') 'final stop receipt is not stopping' 'STOP-RECEIPT-FINAL'
    Assert-SmokeTrue (Wait-OwnedShotdGone $preview.root 5) 'owned host process survived the final stop' 'STOP-PROCESS-GONE-FINAL'
    $credential = Remove-OwnedTestCredential $libraryId
    $credentialTarget = $credential.target
    Assert-SmokeTrue ([bool]$credential.ok) ("exact synthetic credential cleanup failed at stage " + $credential.stage + " with raw exit " + $credential.exit) 'CREDENTIAL-CLEANUP'
    Assert-SmokeTrue (Remove-OwnedPreviewRoot $preview) 'owned preview root removal failed' 'PREVIEW-ROOT-REMOVED'
    $absentCleanup = Invoke-OwnedPreviewCleanup $repo $absent.preview $binaries.shot 60
    Assert-SmokeTrue ([bool]$absentCleanup.ok -and $absentCleanup.credential_readback_code -eq 1168) 'owned no-start ordered cleanup failed' 'ABSENT-CREDENTIAL-CLEANUP'
    $absentPresence = Get-OwnedCredentialPresence ('ScreenMemory.Test.V1/library/' + $absentLibrary + '/owner')
    Assert-SmokeTrue ($absentPresence.status -eq 'absent' -and $absentPresence.code -eq 1168) 'owned no-start credential survived cleanup' 'ABSENT-CREDENTIAL-ABSENT'
    Assert-SmokeTrue (-not (Test-Path -LiteralPath $absent.preview)) 'owned no-start root survived cleanup' 'ABSENT-ROOT-REMOVED'
    [void]$notes.Add('Cleaned exact synthetic Test credential target only: ' + $credentialTarget)
} catch {
    $failed = $_.Exception.Message
    [void]$notes.Add('SMOKE FAILED: ' + $failed)
    foreach ($ownedRoot in @($preview, $absent)) {
        if (-not $ownedRoot -or -not (Test-Path -LiteralPath $ownedRoot.config)) { continue }
        try {
            # Never delete a credential or root after an unverified stop. Reuse the
            # same ordered guard as the real preview, preserving evidence on refusal.
            $cleanup = Invoke-OwnedPreviewCleanup $repo $ownedRoot.preview $binaries.shot 60
            if (-not $cleanup.ok) { [void]$notes.Add('Guarded cleanup refused at ' + $cleanup.stage + '; owned root retained.') }
        } catch { [void]$notes.Add('Guarded cleanup failed; owned root retained.') }
    }
}

$result = [ordered]@{
    schema_version = 1
    suite = 'host-cli-executable-smoke'
    stage = 'S01'
    stage_result = 'IN_PROGRESS'
    scope = 'synthetic-explicit-executable-smoke'
    outcome = $(if ($failed) { 'FAIL' } else { 'PASS' })
    failure = $failed
    images = @{ host = (Split-Path $binaries.shotd -Leaf); cli = (Split-Path $binaries.shot -Leaf) }
    library_id = $libraryId
    absent_library_id = $absentLibrary
    credential_target = $credentialTarget
    steps = @($steps.ToArray())
    notes = @($notes.ToArray())
    raw_logs = 'private .pi output only; do not upload automatically'
}
$text = (ConvertTo-Json -InputObject $result -Depth 8) -replace "`r`n", "`n"
[System.IO.File]::WriteAllText((Join-Path $OutputRoot 'result.json'), $text + "`n", $utf8)
if ($failed) {
    Write-Output ('SMOKE FAIL: ' + $failed)
    Write-Output ('sanitized evidence: ' + (Join-Path $OutputRoot 'result.json'))
    exit 1
}
Write-Output 'SMOKE PASS: real host+CLI served status/doctor, refused a duplicate owner, stopped, restarted with the same library id and cleaned its exact Test credential.'
Write-Output ('sanitized evidence: ' + (Join-Path $OutputRoot 'result.json'))
exit 0
