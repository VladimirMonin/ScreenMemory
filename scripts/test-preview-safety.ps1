# S01-T05d review fix: executable safety suite for the owned-preview cleanup guards.
# Real files, real directories, real processes and real synthetic Test credentials on private
# synthetic UUID paths only: no personal data, no model, no network and no live credential.
# Every negative case proves that cleanup refused, that the target directory and its sentinel
# files survived and that a canary file which a shell injection would have created never
# appeared. Runs standalone and is called by scripts/test-verify.ps1, so it is part of the
# mandatory verification.
[CmdletBinding()]
param([string]$OutputRoot)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'preview-common.ps1')
$utf8 = New-Object System.Text.UTF8Encoding($false)
$repo = Get-PreviewRepositoryRoot $PSScriptRoot
if (-not $OutputRoot) { $OutputRoot = New-PreviewOutputRoot $repo 'test-preview-safety' }
if (Test-Path -LiteralPath $OutputRoot) { throw 'Output root must not exist (immutable evidence)' }
[void](New-Item -ItemType Directory -Path $OutputRoot)
$work = Join-Path $OutputRoot 'work'
[void](New-Item -ItemType Directory -Path $work)
$steps = New-Object System.Collections.ArrayList
$notes = New-Object System.Collections.ArrayList
$created = New-Object System.Collections.ArrayList
$junctionLinks = New-Object System.Collections.ArrayList
$credentialTargets = New-Object System.Collections.ArrayList
$ownedChild = $null
$failed = $null

function Add-SafetyStep([string]$Id, [bool]$Passed, [string]$Detail) {
    [void]$steps.Add([ordered]@{
            check_id = $Id
            outcome = $(if ($Passed) { 'PASS' } else { 'FAIL' })
            detail = $Detail
        })
    if (-not $Passed) { throw ('safety check failed: ' + $Id + ' - ' + $Detail) }
}

function Write-SyntheticConfig([string]$Directory, [string]$LibraryId) {
    $config = [ordered]@{
        schema_version = 1
        environment = 'test'
        root = (Join-Path $Directory 'root')
        library_id = $LibraryId
        host_executable = (Join-Path $work 'shotd-stub.exe')
    }
    $text = (ConvertTo-Json -InputObject $config -Depth 4) -replace "`r`n", "`n"
    $path = Join-Path $Directory 'preview.json'
    [System.IO.File]::WriteAllText($path, $text + "`n", $utf8)
    return $path
}

function New-SyntheticCase([string]$Parent, [string]$Name, [string]$LibraryId, [switch]$NoMarker, [string]$MarkerContent) {
    $directory = Join-Path $Parent $Name
    $data = Join-Path $directory 'root'
    [void](New-Item -ItemType Directory -Path $data -Force)
    foreach ($child in @('data', 'config', 'cache', 'webview')) { [void](New-Item -ItemType Directory -Path (Join-Path $data $child) -Force) }
    $sentinel = Join-Path $directory 'sentinel.keep'
    [System.IO.File]::WriteAllText($sentinel, "synthetic sentinel`n", $utf8)
    $config = Write-SyntheticConfig $directory $LibraryId
    if (-not $NoMarker) {
        $content = $(if ($MarkerContent) { $MarkerContent } else { $Name })
        [System.IO.File]::WriteAllText((Join-Path $directory (Get-PreviewOwnedMarkerName)), $content + "`n", $utf8)
    }
    [void]$created.Add($directory)
    return @{ dir = $directory; data = $data; config = $config; sentinel = $sentinel; name = $Name }
}

function Invoke-PreviewScriptCleanup([string]$PreviewDir) {
    # The real script path (not only the helper): a refusal must be a non-zero process exit.
    return (Invoke-PreviewProcess (Join-Path $PSHOME 'powershell.exe') @('-NoProfile', '-NonInteractive', '-File', (Join-Path $PSScriptRoot 'preview.ps1'), '-Action', 'cleanup', '-PreviewRoot', $PreviewDir, '-SkipBuild') $repo 120)
}

function Remove-OwnedSyntheticDirectory([string]$Path) {
    # Only paths this suite created under the owned base or its own private work directory.
    $full = [System.IO.Path]::GetFullPath($Path)
    $ownedBase = [System.IO.Path]::GetFullPath((Get-PreviewOwnedRootBase $repo))
    $privateWork = [System.IO.Path]::GetFullPath($work)
    if (-not ($full.StartsWith($ownedBase, [StringComparison]::OrdinalIgnoreCase) -or $full.StartsWith($privateWork, [StringComparison]::OrdinalIgnoreCase))) {
        throw 'Refusing cleanup outside this suite own synthetic paths'
    }
    if (Test-Path -LiteralPath $full) { Remove-Item -LiteralPath $full -Recurse -Force }
}

function Add-SyntheticTestCredential([string]$Target) {
    # Exactly one fresh synthetic Test-namespace target of this suite, written through the
    # direct cmdkey.exe argument list (never a shell). Every target is tracked so the finally
    # block can confirm a real absence with the native readback.
    [void]$credentialTargets.Add($Target)
    $cmdkey = Join-Path $env:SystemRoot 'System32\cmdkey.exe'
    $added = Invoke-PreviewProcess $cmdkey @(('/generic:' + $Target), '/user:synthetic-preview-owner', '/pass:synthetic-preview-value') $env:SystemDrive 60
    if ($added.exit -ne 0) { throw ('synthetic credential add failed with raw exit ' + $added.exit) }
}

function Assert-SyntheticTestCredentialAbsent([string]$Target, [string]$StepId) {
    # The native readback is the only accepted proof that the exact target is gone.
    $presence = Get-OwnedCredentialPresence $Target
    Add-SafetyStep $StepId (($presence.status -eq 'absent') -and ($presence.code -eq 1168)) ('readback=' + $presence.status + ' code=' + $presence.code)
}

try {
    Write-Output ('safety output root: ' + $OutputRoot)

    # 1) Positive control: a created root is accepted and carries the ownership marker.
    $fresh = New-OwnedPreviewRoot $repo
    [void]$created.Add($fresh.preview)
    [void](Write-SyntheticConfig $fresh.preview ([guid]::NewGuid().ToString('D')))
    $resolved = Resolve-OwnedPreviewRoot $repo $fresh.preview
    Add-SafetyStep 'OWNED-ROOT-ACCEPTED' ([bool]$resolved.ok) ('stage=' + $resolved.stage)
    Add-SafetyStep 'OWNED-ROOT-STRICT-UUID' ((Split-Path $fresh.preview -Leaf) -ceq $fresh.uuid -and (Test-PreviewOwnedRootName $fresh.uuid)) ('uuid=' + $fresh.uuid)
    $marker = Join-Path $fresh.preview (Get-PreviewOwnedMarkerName)
    Add-SafetyStep 'OWNED-MARKER-MATCHES' ((Test-Path -LiteralPath $marker -PathType Leaf) -and (([System.IO.File]::ReadAllText($marker)).Trim() -ceq $fresh.uuid)) 'marker content equals the directory uuid'
    Add-SafetyStep 'OWNED-ROOT-REMOVABLE' ((Remove-OwnedPreviewRoot $fresh) -and -not (Test-Path -LiteralPath $fresh.preview)) 'marker-checked remover removes an owned root'

    # 2) A foreign directory outside <repo>/.pi/preview is refused (ownership, not shape).
    $validId = [guid]::NewGuid().ToString('D')
    $foreign = New-SyntheticCase $work ([guid]::NewGuid().ToString('D')) $validId
    $refusal = Invoke-OwnedPreviewCleanup $repo $foreign.dir (Join-Path $work 'shotd-does-not-exist.exe')
    Add-SafetyStep 'FOREIGN-PARENT-REFUSED' ((-not $refusal.ok) -and $refusal.stage -eq 'parent') ('stage=' + $refusal.stage)
    Add-SafetyStep 'FOREIGN-PARENT-PRESERVED' ((Test-Path -LiteralPath $foreign.sentinel -PathType Leaf) -and (Test-Path -LiteralPath $foreign.config -PathType Leaf)) 'sentinel and configuration survive'
    $scriptRun = Invoke-PreviewScriptCleanup $foreign.dir
    Add-SafetyStep 'FOREIGN-PARENT-NONZERO-EXIT' ($null -ne $scriptRun.exit -and $scriptRun.exit -ne 0) ('raw exit=' + $scriptRun.exit + ' reason=' + $scriptRun.reason)
    Add-SafetyStep 'FOREIGN-PARENT-SCRIPT-PRESERVED' (Test-Path -LiteralPath $foreign.sentinel -PathType Leaf) 'script refusal deleted nothing'

    # 3) A similar name without an ownership marker under the correct parent is refused.
    $noMarker = New-SyntheticCase (Get-PreviewOwnedRootBase $repo) ([guid]::NewGuid().ToString('D')) $validId -NoMarker
    $refusal = Invoke-OwnedPreviewCleanup $repo $noMarker.dir (Join-Path $work 'shotd-does-not-exist.exe')
    Add-SafetyStep 'MISSING-MARKER-REFUSED' ((-not $refusal.ok) -and $refusal.stage -eq 'marker') ('stage=' + $refusal.stage)
    Add-SafetyStep 'MISSING-MARKER-PRESERVED' (Test-Path -LiteralPath $noMarker.sentinel -PathType Leaf) 'sentinel survives'

    # 4) A marker that names another uuid is refused.
    $mismatch = New-SyntheticCase (Get-PreviewOwnedRootBase $repo) ([guid]::NewGuid().ToString('D')) $validId -MarkerContent ([guid]::NewGuid().ToString('D'))
    $refusal = Invoke-OwnedPreviewCleanup $repo $mismatch.dir (Join-Path $work 'shotd-does-not-exist.exe')
    Add-SafetyStep 'MARKER-MISMATCH-REFUSED' ((-not $refusal.ok) -and $refusal.stage -eq 'marker') ('stage=' + $refusal.stage)
    Add-SafetyStep 'MARKER-MISMATCH-PRESERVED' (Test-Path -LiteralPath $mismatch.sentinel -PathType Leaf) 'sentinel survives'

    # 5) Shell metacharacters in library_id: no cmdkey launch, no injected canary file.
    $injectCanary = Join-Path $work 'canary-injection.txt'
    $injectId = $validId + '&echo pwned>' + $injectCanary + '&rem'
    $inject = New-SyntheticCase (Get-PreviewOwnedRootBase $repo) ([guid]::NewGuid().ToString('D')) $injectId
    $injectionCredential = Remove-OwnedTestCredential $injectId
    Add-SafetyStep 'INJECTION-CREDENTIAL-REFUSED' ((-not $injectionCredential.ok) -and $injectionCredential.stage -eq 'identity' -and $null -eq $injectionCredential.target) ('stage=' + $injectionCredential.stage)
    Add-SafetyStep 'INJECTION-NO-CANARY' (-not (Test-Path -LiteralPath $injectCanary)) 'a shell-injected canary file never appeared'
    $refusal = Invoke-OwnedPreviewCleanup $repo $inject.dir (Join-Path $work 'shotd-does-not-exist.exe')
    Add-SafetyStep 'INJECTION-CLEANUP-REFUSED' ((-not $refusal.ok) -and $refusal.stage -eq 'identity') ('stage=' + $refusal.stage)
    Add-SafetyStep 'INJECTION-PRESERVED' ((Test-Path -LiteralPath $inject.sentinel -PathType Leaf) -and (Test-Path -LiteralPath $inject.config -PathType Leaf)) 'sentinel and configuration survive'
    Add-SafetyStep 'INJECTION-NO-CANARY-AFTER-CLEANUP' (-not (Test-Path -LiteralPath $injectCanary)) 'no canary after the cleanup refusal'

    # 6) The dot and space variants of the same id are refused the same way.
    foreach ($case in @(@{ id = ($validId + '.'); label = 'DOT' }, @{ id = ($validId + ' '); label = 'SPACE' })) {
        $variant = New-SyntheticCase (Get-PreviewOwnedRootBase $repo) ([guid]::NewGuid().ToString('D')) $case.id
        $refusal = Invoke-OwnedPreviewCleanup $repo $variant.dir (Join-Path $work 'shotd-does-not-exist.exe')
        Add-SafetyStep ('INJECTION-' + $case.label + '-REFUSED') ((-not $refusal.ok) -and $refusal.stage -eq 'identity') ('stage=' + $refusal.stage)
        Add-SafetyStep ('INJECTION-' + $case.label + '-PRESERVED') (Test-Path -LiteralPath $variant.sentinel -PathType Leaf) 'sentinel survives'
    }

    # 7) A live owned child process blocks cleanup even before the stop step is reached.
    $live = New-OwnedPreviewRoot $repo
    [void]$created.Add($live.preview)
    $liveLibrary = [guid]::NewGuid().ToString('D')
    [void](Write-SyntheticConfig $live.preview $liveLibrary)
    $stub = Join-Path $work 'shotd.exe'
    [System.IO.File]::Copy((Join-Path $PSHOME 'powershell.exe'), $stub)
    $sleeper = Join-Path $work 'owned-child.ps1'
    [System.IO.File]::WriteAllText($sleeper, "param([string]`$OwnedRoot)`nStart-Sleep -Seconds 120`n", $utf8)
    $childInfo = New-Object System.Diagnostics.ProcessStartInfo
    $childInfo.FileName = $stub
    $childInfo.Arguments = '-NoProfile -NonInteractive -File "' + $sleeper + '" -OwnedRoot "' + $live.root + '"'
    $childInfo.UseShellExecute = $false
    $childInfo.CreateNoWindow = $true
    $childInfo.RedirectStandardOutput = $true
    $childInfo.RedirectStandardError = $true
    $ownedChild = [System.Diagnostics.Process]::Start($childInfo)
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline -and (Get-OwnedShotdProcess $live.root).Count -eq 0) { Start-Sleep -Milliseconds 200 }
    Add-SafetyStep 'OWNED-CHILD-OBSERVED' ((Get-OwnedShotdProcess $live.root).Count -ge 1) 'own synthetic shotd.exe is visible through its command line'
    Add-SafetyStep 'OWNED-CHILD-NOT-QUIESCENT' (-not (Test-OwnedPreviewQuiescent $live.root)) 'quiescence predicate reports the live owned child'
    Add-SafetyStep 'OWNED-CHILD-WAIT-BOUNDED' (-not (Wait-OwnedShotdGone $live.root 2)) 'bounded wait never claims a live owned child is gone'
    $refusal = Invoke-OwnedPreviewCleanup $repo $live.preview (Join-Path $work 'shotd-does-not-exist.exe')
    Add-SafetyStep 'LIVE-CHILD-CLEANUP-REFUSED' ((-not $refusal.ok) -and $refusal.stage -eq 'binary') ('stage=' + $refusal.stage)
    Add-SafetyStep 'LIVE-CHILD-PRESERVED' ((Test-Path -LiteralPath $live.preview -PathType Container) -and (Test-Path -LiteralPath (Join-Path $live.preview (Get-PreviewOwnedMarkerName)) -PathType Leaf)) 'owned root is kept while its child lives'
    # A stop image that exists but cannot succeed is refused as well (no unverified delete).
    $failingStop = Join-Path $env:SystemRoot 'System32\where.exe'
    $refusal = Invoke-OwnedPreviewCleanup $repo $live.preview $failingStop 30
    Add-SafetyStep 'FAILED-STOP-REFUSED' ((-not $refusal.ok) -and $refusal.stage -eq 'stop' -and $null -ne $refusal.stop_exit -and $refusal.stop_exit -ne 0) ('stage=' + $refusal.stage + ' stop_exit=' + $refusal.stop_exit)
    Add-SafetyStep 'FAILED-STOP-PRESERVED' ((Test-Path -LiteralPath $live.preview -PathType Container) -and (Test-Path -LiteralPath (Join-Path $live.preview 'preview.json') -PathType Leaf)) 'owned root is kept after an unverified stop'
    $realShot = (Get-PreviewBinaries (Get-PreviewToolchain $repo)).shot
    if (Test-Path -LiteralPath $realShot -PathType Leaf) {
        $realStop = Invoke-PreviewProcess $realShot @('--config', (Join-Path $live.preview 'preview.json'), 'host', 'stop', '--json') $repo 120
        Add-SafetyStep 'REAL-CLI-STOP-UNREACHABLE' ($null -ne $realStop.exit -and $realStop.exit -ne 0) ('raw exit=' + $realStop.exit)
        $refusal = Invoke-OwnedPreviewCleanup $repo $live.preview $realShot
        Add-SafetyStep 'REAL-CLI-STOP-REFUSED' ((-not $refusal.ok) -and $refusal.stage -eq 'stop') ('stage=' + $refusal.stage + ' stop_exit=' + $refusal.stop_exit)
        Add-SafetyStep 'REAL-CLI-STOP-PRESERVED' (Test-Path -LiteralPath $live.preview -PathType Container) 'owned root is kept'
    } else {
        [void]$notes.Add('The real preview CLI image is not present; the failing-stop refusal was proven with a system executable instead.')
    }
    try { $ownedChild.Kill(); [void]$ownedChild.WaitForExit(10000) } catch { [void]$notes.Add('Own synthetic child termination reported: ' + $_.Exception.Message) }
    Add-SafetyStep 'OWNED-CHILD-TERMINATED' (Wait-OwnedShotdGone $live.root 10) 'own synthetic child was terminated by its owner and is gone'

    # 8) A credential result that does not confirm absence keeps the owned root.
    $credentialCase = New-OwnedPreviewRoot $repo
    [void]$created.Add($credentialCase.preview)
    $unusedId = [guid]::NewGuid().ToString('D')
    [void](Write-SyntheticConfig $credentialCase.preview $unusedId)
    $credentialCanary = Join-Path $work 'canary-credential.txt'
    $refusal = Invoke-OwnedPreviewCleanup $repo $credentialCase.preview (Join-Path $work 'shotd-does-not-exist.exe')
    Add-SafetyStep 'CREDENTIAL-STAGE-CONFIRMED' ((-not $refusal.ok) -and $refusal.stage -like 'credential-*') ('stage=' + $refusal.stage)
    Add-SafetyStep 'CREDENTIAL-TARGET-EXACT' ($refusal.target -ceq ('ScreenMemory.Test.V1/library/' + $unusedId + '/owner')) ('target=' + $refusal.target)
    Add-SafetyStep 'CREDENTIAL-FAILURE-KEEPS-ROOT' ((Test-Path -LiteralPath $credentialCase.preview -PathType Container) -and (Test-Path -LiteralPath (Join-Path $credentialCase.preview (Get-PreviewOwnedMarkerName)) -PathType Leaf) -and -not (Test-Path -LiteralPath $credentialCanary)) 'root and marker survive an unconfirmed credential outcome'
    Add-SafetyStep 'CREDENTIAL-ROOT-REMOVABLE' ((Remove-OwnedPreviewRoot $credentialCase) -and -not (Test-Path -LiteralPath $credentialCase.preview)) 'owned root is removable only through the marker-checked remover'

    # 9) A root whose marker disappears after creation cannot be removed.
    $tampered = New-OwnedPreviewRoot $repo
    [void]$created.Add($tampered.preview)
    Remove-Item -LiteralPath (Join-Path $tampered.preview (Get-PreviewOwnedMarkerName)) -Force
    Add-SafetyStep 'TAMPERED-MARKER-REMOVAL-REFUSED' ((-not (Remove-OwnedPreviewRoot $tampered)) -and (Test-Path -LiteralPath $tampered.preview -PathType Container)) 'removal refuses a root without its marker'

    # 10) A reparse point inside an owned root blocks cleanup instead of being followed.
    $junctionCase = New-OwnedPreviewRoot $repo
    [void]$created.Add($junctionCase.preview)
    [void](Write-SyntheticConfig $junctionCase.preview $validId)
    $junctionTarget = Join-Path $work 'junction-target'
    [void](New-Item -ItemType Directory -Path $junctionTarget -Force)
    $junction = Join-Path (Join-Path $junctionCase.root 'data') 'junction-entry'
    [void](New-Item -ItemType Junction -Path $junction -Target $junctionTarget)
    [void]$junctionLinks.Add($junction)
    Add-SafetyStep 'JUNCTION-CREATED' (((Get-Item -LiteralPath $junction -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) 'synthetic junction exists'
    $refusal = Invoke-OwnedPreviewCleanup $repo $junctionCase.preview (Join-Path $work 'shotd-does-not-exist.exe')
    Add-SafetyStep 'JUNCTION-REFUSED' ((-not $refusal.ok) -and $refusal.stage -eq 'reparse') ('stage=' + $refusal.stage)
    Add-SafetyStep 'JUNCTION-PRESERVED' ((Test-Path -LiteralPath $junctionCase.preview -PathType Container) -and (Test-Path -LiteralPath $junctionTarget -PathType Container)) 'owned root and junction target survive'

    # 10b) The whole tree is inspected now, so a reparse point deeper than the previously
    # bounded walk (root/data/a/b/link) is seen and refuses the cleanup with nothing deleted.
    $deepCase = New-SyntheticCase (Get-PreviewOwnedRootBase $repo) ([guid]::NewGuid().ToString('D')) $validId
    $deepTarget = Join-Path $work 'deep-junction-target'
    [void](New-Item -ItemType Directory -Path $deepTarget -Force)
    $deepSentinel = Join-Path $deepTarget 'deep-sentinel.keep'
    [System.IO.File]::WriteAllText($deepSentinel, "synthetic deep sentinel`n", $utf8)
    $deepLinkParent = Join-Path (Join-Path $deepCase.data 'a') 'b'
    [void](New-Item -ItemType Directory -Path $deepLinkParent -Force)
    $deepLink = Join-Path $deepLinkParent 'link'
    [void](New-Item -ItemType Junction -Path $deepLink -Target $deepTarget)
    [void]$junctionLinks.Add($deepLink)
    Add-SafetyStep 'DEEP-JUNCTION-CREATED' (((Get-Item -LiteralPath $deepLink -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) 'synthetic junction exists at root/data/a/b/link below the old depth bound'
    $refusal = Invoke-OwnedPreviewCleanup $repo $deepCase.dir (Join-Path $work 'shotd-does-not-exist.exe')
    Add-SafetyStep 'DEEP-JUNCTION-REFUSED' ((-not $refusal.ok) -and $refusal.stage -eq 'reparse') ('stage=' + $refusal.stage)
    Add-SafetyStep 'DEEP-JUNCTION-PRESERVED' ((Test-Path -LiteralPath $deepCase.sentinel -PathType Leaf) -and (Test-Path -LiteralPath $deepCase.config -PathType Leaf) -and (Test-Path -LiteralPath (Join-Path $deepCase.dir (Get-PreviewOwnedMarkerName)) -PathType Leaf)) 'owned root, marker, configuration and sentinel survive'
    Add-SafetyStep 'DEEP-JUNCTION-TARGET-PRESERVED' ((Test-Path -LiteralPath $deepSentinel -PathType Leaf) -and (([System.IO.File]::ReadAllText($deepSentinel)).Trim() -ceq 'synthetic deep sentinel')) 'the external sentinel behind the deep junction survives'

    # 10c) The whole-tree inspection is bounded by an explicit element limit and fails closed.
    $limitCase = New-SyntheticCase (Get-PreviewOwnedRootBase $repo) ([guid]::NewGuid().ToString('D')) $validId
    $defaultInspection = Test-OwnedPreviewTreeSafe $limitCase.dir
    Add-SafetyStep 'TREE-INSPECTION-DEFAULT-ACCEPTS' (($defaultInspection.safe) -and $defaultInspection.reason -eq '') ('counted=' + $defaultInspection.counted + ' limit=' + (Get-PreviewTreeElementLimit))
    $limitedInspection = Test-OwnedPreviewTreeSafe $limitCase.dir 2
    Add-SafetyStep 'TREE-ELEMENT-LIMIT-REFUSED' ((-not $limitedInspection.safe) -and $limitedInspection.reason -eq 'limit') ('reason=' + $limitedInspection.reason + ' counted=' + $limitedInspection.counted)
    Add-SafetyStep 'TREE-ELEMENT-LIMIT-PRESERVED' (Test-Path -LiteralPath $limitCase.sentinel -PathType Leaf) 'a refused bounded inspection deletes nothing'

    # 11) Credential absence must be proven by the native CredReadW readback, never by a
    # non-zero cmdkey exit. The safety seam substitutes only the readback verdict.
    $missingTarget = 'ScreenMemory.Test.V1/library/' + [guid]::NewGuid().ToString('D') + '/owner'
    $missingPresence = Get-OwnedCredentialPresence $missingTarget
    Add-SafetyStep 'CREDENTIAL-NATIVE-READBACK-ABSENT' (($missingPresence.status -eq 'absent') -and ($missingPresence.code -eq 1168)) ('readback=' + $missingPresence.status + ' code=' + $missingPresence.code)

    # 11a) The real readback reports an existing credential, and a seam that reports it too
    # keeps the owned root (refusal without deletion).
    $existsCase = New-OwnedPreviewRoot $repo
    [void]$created.Add($existsCase.preview)
    $existsId = [guid]::NewGuid().ToString('D')
    [void](Write-SyntheticConfig $existsCase.preview $existsId)
    $existsTarget = 'ScreenMemory.Test.V1/library/' + $existsId + '/owner'
    Add-SyntheticTestCredential $existsTarget
    $realPresence = Get-OwnedCredentialPresence $existsTarget
    Add-SafetyStep 'CREDENTIAL-NATIVE-READBACK-EXISTS' (($realPresence.status -eq 'exists') -and ($realPresence.code -eq 0)) ('readback=' + $realPresence.status + ' code=' + $realPresence.code)
    $script:PreviewCredentialReadbackSeam = @{ status = 'exists'; code = 0 }
    $refusal = Invoke-OwnedPreviewCleanup $repo $existsCase.preview (Join-Path $work 'shotd-does-not-exist.exe')
    $script:PreviewCredentialReadbackSeam = $null
    Add-SafetyStep 'CREDENTIAL-READBACK-EXISTS-REFUSED' ((-not $refusal.ok) -and $refusal.stage -eq 'credential-absence') ('stage=' + $refusal.stage + ' readback=' + $refusal.credential_readback + ' code=' + $refusal.credential_readback_code)
    Add-SafetyStep 'CREDENTIAL-READBACK-EXISTS-PRESERVED' ((Test-Path -LiteralPath $existsCase.preview -PathType Container) -and (Test-Path -LiteralPath (Join-Path $existsCase.preview (Get-PreviewOwnedMarkerName)) -PathType Leaf) -and (Test-Path -LiteralPath $existsCase.config -PathType Leaf)) 'owned root, marker and configuration are kept for reconciliation'
    Assert-SyntheticTestCredentialAbsent $existsTarget 'CREDENTIAL-READBACK-EXISTS-TARGET-ABSENT'

    # 11b) ERROR_NOT_FOUND from the readback is the only accepted absence: the owned root is removed.
    $absentCase = New-OwnedPreviewRoot $repo
    [void]$created.Add($absentCase.preview)
    $absentId = [guid]::NewGuid().ToString('D')
    [void](Write-SyntheticConfig $absentCase.preview $absentId)
    $absentTarget = 'ScreenMemory.Test.V1/library/' + $absentId + '/owner'
    Add-SyntheticTestCredential $absentTarget
    $script:PreviewCredentialReadbackSeam = @{ status = 'absent'; code = 1168 }
    $accepted = Invoke-OwnedPreviewCleanup $repo $absentCase.preview (Join-Path $work 'shotd-does-not-exist.exe')
    $script:PreviewCredentialReadbackSeam = $null
    Add-SafetyStep 'CREDENTIAL-READBACK-ABSENT-ACCEPTED' (($accepted.ok) -and $accepted.stage -eq 'removed' -and $accepted.credential_stage -eq 'verified') ('stage=' + $accepted.stage + ' credential_stage=' + $accepted.credential_stage)
    Add-SafetyStep 'CREDENTIAL-READBACK-ACCEPTED-ROOT-GONE' (-not (Test-Path -LiteralPath $absentCase.preview)) 'an accepted cleanup removes only the owned root'
    Add-SafetyStep 'CREDENTIAL-SEAM-RESTORED' ($null -eq $script:PreviewCredentialReadbackSeam) 'the seam is unset again after every substituted readback'
    Assert-SyntheticTestCredentialAbsent $absentTarget 'CREDENTIAL-READBACK-ABSENT-TARGET-ABSENT'

    # 11c) Any other readback code is an unresolved state: refusal without deletion.
    foreach ($code in @(2, 5)) {
        $errorCase = New-OwnedPreviewRoot $repo
        [void]$created.Add($errorCase.preview)
        $errorId = [guid]::NewGuid().ToString('D')
        [void](Write-SyntheticConfig $errorCase.preview $errorId)
        $errorTarget = 'ScreenMemory.Test.V1/library/' + $errorId + '/owner'
        Add-SyntheticTestCredential $errorTarget
        $script:PreviewCredentialReadbackSeam = @{ status = 'error'; code = $code }
        $refusal = Invoke-OwnedPreviewCleanup $repo $errorCase.preview (Join-Path $work 'shotd-does-not-exist.exe')
        $script:PreviewCredentialReadbackSeam = $null
        Add-SafetyStep ('CREDENTIAL-READBACK-ERROR-' + $code + '-REFUSED') ((-not $refusal.ok) -and $refusal.stage -eq 'credential-absence-error' -and $refusal.credential_readback_code -eq $code) ('stage=' + $refusal.stage + ' readback=' + $refusal.credential_readback + ' code=' + $refusal.credential_readback_code)
        Add-SafetyStep ('CREDENTIAL-READBACK-ERROR-' + $code + '-PRESERVED') ((Test-Path -LiteralPath $errorCase.preview -PathType Container) -and (Test-Path -LiteralPath (Join-Path $errorCase.preview (Get-PreviewOwnedMarkerName)) -PathType Leaf)) 'owned root and marker survive an unresolved readback'
        Assert-SyntheticTestCredentialAbsent $errorTarget ('CREDENTIAL-READBACK-ERROR-' + $code + '-TARGET-ABSENT')
    }
} catch {
    $failed = $_.Exception.Message
    [void]$notes.Add('SAFETY FAILED: ' + $failed)
} finally {
    if ($ownedChild) {
        try { if (-not $ownedChild.HasExited) { $ownedChild.Kill(); [void]$ownedChild.WaitForExit(10000) } } catch { }
        $ownedChild.Dispose()
    }
    foreach ($link in @($junctionLinks)) {
        # Every junction link is dropped first with the .NET call, which removes the link only
        # and never prompts or follows it, so a recursive delete can never touch a target.
        if ($link -and (Test-Path -LiteralPath $link)) {
            try { [System.IO.Directory]::Delete($link, $false) } catch { [void]$notes.Add('Junction link cleanup reported: ' + $_.Exception.Message) }
        }
    }
    foreach ($credentialTarget in @($credentialTargets)) {
        # No synthetic credential of this suite may survive it. Absence is proven with the
        # native readback; anything else is reported as an unexplained cleanup failure.
        $presence = $null
        try { $presence = Get-OwnedCredentialPresence $credentialTarget } catch { $presence = @{ status = 'error'; code = $null } }
        if ($presence.status -ne 'absent') {
            try { [void](Invoke-PreviewProcess (Join-Path $env:SystemRoot 'System32\cmdkey.exe') @('/delete:' + $credentialTarget) $env:SystemDrive 60) } catch { }
            try { $presence = Get-OwnedCredentialPresence $credentialTarget } catch { $presence = @{ status = 'error'; code = $null } }
        }
        if ($presence.status -ne 'absent') {
            [void]$notes.Add('Synthetic credential absence was not confirmed for ' + $credentialTarget + '; reconcile this exact target manually')
            if (-not $failed) { $failed = 'synthetic credential absence was not confirmed; reconcile the exact target manually' }
        }
    }
    foreach ($path in $created) {
        try { Remove-OwnedSyntheticDirectory $path } catch { [void]$notes.Add('Synthetic path cleanup reported: ' + $_.Exception.Message) }
    }
}

$result = [ordered]@{
    schema_version = 1
    suite = 'owned-preview-cleanup-safety'
    stage = 'S01'
    stage_result = 'IN_PROGRESS'
    scope = 'synthetic-negative-cleanup-guards-and-native-credential-readback'
    outcome = $(if ($failed) { 'FAIL' } else { 'PASS' })
    failure = $failed
    steps = @($steps.ToArray())
    notes = @($notes.ToArray())
    raw_logs = 'private .pi output only; do not upload automatically'
}
$text = (ConvertTo-Json -InputObject $result -Depth 8) -replace "`r`n", "`n"
[System.IO.File]::WriteAllText((Join-Path $OutputRoot 'result.json'), $text + "`n", $utf8)
if ($failed) {
    Write-Output ('SAFETY FAIL: ' + $failed)
    Write-Output ('sanitized evidence: ' + (Join-Path $OutputRoot 'result.json'))
    exit 1
}
Write-Output ('SAFETY PASS: ' + $steps.Count + ' checks, foreign/unowned roots, injected library ids, missing/mismatched markers, a live owned child, an unverified stop, a shallow and a deep reparse point, an over-limit tree and every non-1168 credential readback all refused with nothing deleted.')
Write-Output ('sanitized evidence: ' + (Join-Path $OutputRoot 'result.json'))
exit 0
