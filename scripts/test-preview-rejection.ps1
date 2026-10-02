# Two-root preflight isolation regression. Own synthetic roots/credentials only;
# no desktop capture, GUI window, keyboard/clipboard or AI/model operations.
[CmdletBinding()]
param(
    [string]$OutputRoot,
    [ValidateSet('invalid-image','cross-library','cross-root','invalid-schema','invalid-environment','cli-cross-library','missing-binding','malformed-binding','binding-directory','binding-library','mutate-config','mutate-binding')]
    [string[]]$Cases = @('invalid-image','cross-library','cross-root','invalid-schema','invalid-environment','cli-cross-library','missing-binding','malformed-binding','binding-directory','binding-library','mutate-config','mutate-binding')
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'verification-common.ps1')
. (Join-Path $PSScriptRoot 'preview-common.ps1')
$repo = Get-PreviewRepositoryRoot $PSScriptRoot
if (-not $OutputRoot) { $OutputRoot = Join-Path $repo ('.pi/temporary/' + [guid]::NewGuid().ToString('D')) }
$state = New-VerificationState $repo $OutputRoot 'preview-rejection-isolation'
$tools = Get-PreviewToolchain $repo
$binaries = Get-PreviewBinaries $tools
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class PreviewRejectionCredentialDelete {
 [DllImport("advapi32.dll", EntryPoint="CredDeleteW", CharSet=CharSet.Unicode, SetLastError=true)]
 public static extern bool Delete(string target, uint type, uint flags);
}
'@
function Get-RejectionRootSnapshot([hashtable]$Owned) {
    if (-not (Test-Path -LiteralPath $Owned.preview -PathType Container)) { return 'missing' }
    $files = @(Get-ChildItem -LiteralPath $Owned.preview -Recurse -File | Sort-Object FullName | ForEach-Object {
        $_.FullName.Substring($Owned.preview.Length) + ':' + (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
    })
    return ($files -join '|')
}
try {
    foreach ($case in $Cases) {
        $a = $null; $b = $null; $idA = $null; $idB = $null
        $hostProcess = $null; $hostAProcess = $null; $savedA = $null; $savedBindingA = $null
        $mutationCase = $case -in @('mutate-config','mutate-binding')
        $cleanup = [ordered]@{ host_gone=$false; credentials_absent=$false; roots_removed=$false }
        try {
            $a = New-OwnedPreviewRoot $repo
            $initA = Initialize-OwnedPreviewLibrary $repo $binaries.shotd $a.root $a
            $idA = [string]$initA.library_id
            [void](Write-TrustedPreviewConfig $a $idA $binaries.shotd)
            $b = New-OwnedPreviewRoot $repo
            $initB = Initialize-OwnedPreviewLibrary $repo $binaries.shotd $b.root $b
            $idB = [string]$initB.library_id
            [void](Write-TrustedPreviewConfig $b $idB $binaries.shotd)
            if (-not (Test-PreviewCanonicalUuid $idA) -or -not (Test-PreviewCanonicalUuid $idB) -or $idA -ceq $idB) { throw 'Own library identity invalid' }
            $targetA = 'ScreenMemory.Test.V1/library/' + $idA + '/owner'
            $targetB = 'ScreenMemory.Test.V1/library/' + $idB + '/owner'
            if ((Get-OwnedCredentialPresence $targetA).status -ne 'exists' -or (Get-OwnedCredentialPresence $targetB).status -ne 'exists') { throw 'Own init did not create the exact credentials' }
            # A real B host makes a valid-image/wrong-library config meaningful: a late
            # flag after a successful selfcheck must not adopt another root's authority.
            $status = Invoke-PreviewProcess $binaries.shot @('--config',$b.config,'status','--json') $repo 120
            if ($status.exit -ne 0 -or $status.reason -ne 'OK') { throw 'Own B host start failed' }
            $ownedB = Get-OwnedShotdProcess $b.root
            if ($ownedB.Count -ne 1) { throw 'Own B host count invalid' }
            $hostProcess = [Diagnostics.Process]::GetProcessById([int]$ownedB[0].ProcessId)
            $null = $hostProcess.Handle # retain this exact own process, not a reused PID
            $savedA = [System.IO.File]::ReadAllText($a.config)
            $savedBindingA = [System.IO.File]::ReadAllText((Join-Path $a.preview (Get-PreviewLibraryBindingName)))
            $config = ConvertFrom-Json $savedA
            switch ($case) {
                'invalid-image' { $config.library_id=$idB; $config.host_executable=Join-Path $state.output 'not-a-host.exe' }
                'cross-library' { $config.library_id=$idB }
                'cross-root' { $config.root=$b.root; $config.library_id=$idB }
                'invalid-schema' { $config.schema_version=2 }
                'invalid-environment' { $config.environment='production' }
                'cli-cross-library' { $config.library_id=$idB }
                'missing-binding' { Remove-Item -LiteralPath (Join-Path $a.preview (Get-PreviewLibraryBindingName)) }
                'malformed-binding' { [System.IO.File]::WriteAllText((Join-Path $a.preview (Get-PreviewLibraryBindingName)),'{',$script:Utf8) }
                'binding-directory' {
                    $binding = [ordered]@{ directory_uuid=$b.uuid; library_id=$idA }
                    [System.IO.File]::WriteAllText((Join-Path $a.preview (Get-PreviewLibraryBindingName)),($binding | ConvertTo-Json),$script:Utf8)
                }
                'binding-library' {
                    $binding = [ordered]@{ directory_uuid=$a.uuid; library_id=$idB }
                    [System.IO.File]::WriteAllText((Join-Path $a.preview (Get-PreviewLibraryBindingName)),($binding | ConvertTo-Json),$script:Utf8)
                }
            }
            [System.IO.File]::WriteAllText($a.config, (($config | ConvertTo-Json -Depth 4) -replace "`r`n","`n") + "`n", $script:Utf8)
            $before = Get-RejectionRootSnapshot $a
            $childOutput = Join-Path $state.output $case
            $log = Join-Path $state.output ($case + '.log')
            $argv = @('-NoProfile','-NonInteractive','-File',(Join-Path $PSScriptRoot 'preview-gui.ps1'),'-Action','selfcheck','-SkipBuild','-Cleanup','-PreviewRoot',$a.preview,'-OutputRoot',$childOutput)
            if ($case -eq 'cli-cross-library') {
                $argv = @('-NoProfile','-NonInteractive','-File',(Join-Path $PSScriptRoot 'preview.ps1'),'-Action','cleanup','-SkipBuild','-PreviewRoot',$a.preview)
            }
            if ($mutationCase) { $argv += @('-FixtureCase',$case) }
            $raw = Invoke-VerificationProcess (Join-Path $PSHOME 'powershell.exe') $argv $repo $log 180 @{}
            $hostsA = Get-OwnedShotdProcess $a.root
            if ($hostsA.Count -eq 1) {
                $hostAProcess = [Diagnostics.Process]::GetProcessById([int]$hostsA[0].ProcessId)
                $null = $hostAProcess.Handle
            }
            $presentA = Get-OwnedCredentialPresence $targetA
            $presentB = Get-OwnedCredentialPresence $targetB
            $hostProcess.Refresh()
            $checks = [ordered]@{
                rejected=($raw.exit -ne 0 -and $raw.reason -in @('OK','NATIVE_EXIT'))
                a_unchanged=$(if($mutationCase){$null}else{(Get-RejectionRootSnapshot $a) -ceq $before})
                a_snapshot_scope=$(if($mutationCase){'NOT_RUN: live host files; input intentionally mutated'}else{'inactive root full-file hash'})
                b_exists=(Test-Path -LiteralPath $b.preview -PathType Container)
                a_credential_present=($presentA.status -eq 'exists')
                b_credential_present=($presentB.status -eq 'exists')
                b_host_alive=(-not $hostProcess.HasExited)
                a_host_count=(Get-OwnedShotdProcess $a.root).Count
            }
            $ok = $checks.rejected -and $checks.b_exists -and $checks.a_credential_present -and $checks.b_credential_present -and $checks.b_host_alive
            if ($mutationCase) {
                $child = ConvertFrom-Json ([System.IO.File]::ReadAllText((Join-Path $childOutput 'result.json')))
                $refusal = @($child.steps | Where-Object { $_.check_id -eq 'OWNED-FINALIZATION-REFUSED' -and $_.cleanup_stage -eq 'mutation' -and $_.local_host -like 'BLOCKED:*' })
                $checks.mutation_refused = $refusal.Count -eq 1
                $checks.a_host_preserved = $hostAProcess -and -not $hostAProcess.HasExited
                $ok = $ok -and $checks.mutation_refused -and $checks.a_host_preserved -and (Test-Path -LiteralPath $a.preview -PathType Container)
            } else { $ok = $ok -and $checks.a_unchanged -and $checks.a_host_count -eq 0 }
            [void]$state.steps.Add([ordered]@{ check_id=$case; outcome=$(if($ok){'PASS'}else{'FAIL'}); native_exit=$raw.exit; reason=$raw.reason; checks=$checks; evidence=$(if($case -eq 'cli-cross-library'){$case+'.log'}else{$case+'/result.json'}) })
            if (-not $ok) { $state.failed=$true }
        } finally {
            # Harness owns both freshly created roots and retained B process. Restore
            # only its own poisoned A config; no prefix credential/root sweep.
            if ($a -and $savedA -and (Test-Path -LiteralPath $a.preview -PathType Container)) { [System.IO.File]::WriteAllText($a.config,$savedA,$script:Utf8) }
            if ($a -and $savedBindingA -and (Test-Path -LiteralPath $a.preview -PathType Container)) {
                [System.IO.File]::WriteAllText((Join-Path $a.preview (Get-PreviewLibraryBindingName)),$savedBindingA,$script:Utf8)
            }
            $allHostsGone=$true
            foreach ($entry in @(@{process=$hostAProcess;owned=$a;id=$idA},@{process=$hostProcess;owned=$b;id=$idB})) {
                $process=$entry.process; $owned=$entry.owned; $id=$entry.id
                if (-not $process) { continue }
                try {
                    if (-not $process.HasExited -and $owned -and (Get-OwnedCredentialPresence ('ScreenMemory.Test.V1/library/'+$id+'/owner')).status -eq 'exists') {
                        $stop = Invoke-PreviewProcess $binaries.shot @('--config',$owned.config,'host','stop','--json') $repo 60
                        if ($stop.exit -eq 0) { [void]$process.WaitForExit(10000) }
                    }
                    if (-not $process.HasExited) { $process.Kill(); [void]$process.WaitForExit(5000) }
                    if (-not $process.HasExited) { $allHostsGone=$false }
                } finally { $process.Dispose() }
            }
            $cleanup.host_gone=$allHostsGone
            $allAbsent=$true
            foreach ($id in @($idA,$idB)) {
                if (-not $id) { continue }
                if (-not (Test-PreviewCanonicalUuid $id)) { $allAbsent=$false; continue }
                $target='ScreenMemory.Test.V1/library/'+$id+'/owner'
                $presence=Get-OwnedCredentialPresence $target
                if ($presence.status -eq 'exists') {
                    if (-not [PreviewRejectionCredentialDelete]::Delete($target,1,0)) { $allAbsent=$false }
                } elseif ($presence.status -ne 'absent') { $allAbsent=$false }
                if ((Get-OwnedCredentialPresence $target).code -ne 1168) { $allAbsent=$false }
            }
            $cleanup.credentials_absent=$allAbsent
            $allRemoved=$true
            if ($cleanup.host_gone -and $allAbsent) {
                foreach ($owned in @($a,$b)) {
                    if ($owned -and (Test-Path -LiteralPath $owned.preview -PathType Container)) {
                        if (-not (Remove-OwnedPreviewRoot $owned)) { $allRemoved=$false }
                    }
                }
            } else { $allRemoved=$false }
            $cleanup.roots_removed=$allRemoved
            Write-VerificationJson (Join-Path $state.output ($case+'-harness-cleanup.json')) $cleanup
            if (-not ($cleanup.host_gone -and $cleanup.credentials_absent -and $cleanup.roots_removed)) { $state.failed=$true }
        }
        if ($state.failed) { break }
    }
} catch { Add-VerificationGuardStep $state 'REJECTION-HARNESS' 'SYNTHETIC_PROBE_FAILED' }
exit (Complete-Verification $state @() @(@{check_id='CAPTURE-CLIPBOARD-MODELS';outcome='NOT_RUN';reason='preflight isolation probe never captures screen, writes clipboard or calls models'}))
