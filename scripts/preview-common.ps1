# Shared helpers for the S01 host/CLI developer preview and the explicit executable smoke.
# Owned .pi roots, pinned local toolchain, no network, no personal data, no global setup.
$ErrorActionPreference = 'Stop'

function Get-PreviewRepositoryRoot([string]$ScriptRoot) {
    return (Split-Path $ScriptRoot -Parent)
}

function Quote-PreviewArgument([string]$Argument) {
    if ($Argument -notmatch '[\s"]' -and $Argument.Length -gt 0) { return $Argument }
    $result = '"'; $slashes = 0
    foreach ($c in $Argument.ToCharArray()) {
        if ($c -eq '\') { $slashes++; continue }
        if ($c -eq '"') { $result += ('\' * ($slashes * 2 + 1)) + '"'; $slashes = 0; continue }
        $result += ('\' * $slashes) + $c; $slashes = 0
    }
    return $result + ('\' * ($slashes * 2)) + '"'
}

# `-RawArguments` passes a verbatim command line for cmd.exe, whose own parser does not
# follow the C-runtime argv quoting used for ordinary executables.
function Invoke-PreviewProcess([string]$Executable, [string[]]$Arguments, [string]$WorkingDirectory, [int]$TimeoutSeconds, [string]$RawArguments = '') {
    if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
        return @{ exit = $null; reason = 'MISSING_EXECUTABLE'; stdout = ''; stderr = ''; duration_ms = 0 }
    }
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Executable
    if ($RawArguments) { $psi.Arguments = $RawArguments }
    else { $psi.Arguments = (($Arguments | ForEach-Object { Quote-PreviewArgument $_ }) -join ' ') }
    $psi.WorkingDirectory = $WorkingDirectory
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = (New-Object System.Text.UTF8Encoding($false))
    $psi.StandardErrorEncoding = (New-Object System.Text.UTF8Encoding($false))
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $proc = [System.Diagnostics.Process]::Start($psi)
    try {
        $outTask = $proc.StandardOutput.ReadToEndAsync()
        $errTask = $proc.StandardError.ReadToEndAsync()
        if (-not $proc.WaitForExit($TimeoutSeconds * 1000)) {
            # Only this direct owned child handle is terminated; never an image name.
            try { $proc.Kill() } catch { }
            $null = $proc.WaitForExit(5000)
            return @{ exit = $null; reason = 'TIMEOUT'; stdout = ''; stderr = ''; duration_ms = $watch.ElapsedMilliseconds }
        }
        # Bounded drain: a leaked pipe holder in some other process must never hang the
        # caller, and a not-yet-closed stream is reported as empty instead of blocking.
        $null = [System.Threading.Tasks.Task]::WaitAll(@($outTask, $errTask), 5000)
        $stdout = $(if ($outTask.IsCompleted) { $outTask.Result } else { '' })
        $stderr = $(if ($errTask.IsCompleted) { $errTask.Result } else { '' })
        return @{ exit = [int]$proc.ExitCode; reason = 'OK'; stdout = $stdout; stderr = $stderr; duration_ms = $watch.ElapsedMilliseconds }
    } finally {
        $watch.Stop()
        $proc.Dispose()
    }
}

function Get-PreviewToolchain([string]$Root) {
    $rustBin = Join-Path $Root '.pi\toolchains\rust-1.98.1-x86_64-pc-windows-msvc\bin'
    $vc = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
    foreach ($path in @((Join-Path $rustBin 'cargo.exe'), (Join-Path $rustBin 'rustc.exe'), $vc)) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw 'Missing pinned Rust/MSVC prerequisite; prepare prerequisites first (scripts/verify.ps1 documents them)' }
    }
    return @{ cargo = (Join-Path $rustBin 'cargo.exe'); rust_bin = $rustBin; vcvars = $vc; target = (Join-Path $Root '.pi\preview-target') }
}

function Invoke-PinnedCargo([string]$Root, [hashtable]$Toolchain, [string[]]$CargoArguments, [int]$TimeoutSeconds = 1200) {
    $helper = Join-Path $Root ('.pi\preview-build-' + [guid]::NewGuid().ToString('N') + '.cmd')
    $lines = New-Object System.Collections.ArrayList
    [void]$lines.Add('@echo off')
    [void]$lines.Add('call "' + $Toolchain.vcvars + '" >nul')
    [void]$lines.Add('set "PATH=' + $Toolchain.rust_bin + ';%PATH%"')
    [void]$lines.Add('set "CARGO_HOME=' + (Join-Path $Root '.pi\donor-build\.cargo-home') + '"')
    [void]$lines.Add('set "CARGO_TARGET_DIR=' + $Toolchain.target + '"')
    [void]$lines.Add('"' + $Toolchain.cargo + '" ' + ($CargoArguments -join ' '))
    [System.IO.File]::WriteAllLines($helper, @($lines.ToArray()), (New-Object System.Text.UTF8Encoding($false)))
    try {
        # `cmd /d /s /c ""<path>""` is the /s-safe form: cmd strips only the outermost
        # pair, so a checkout path containing spaces still resolves. The raw command line
        # bypasses the C-runtime quoter, which cmd does not use.
        $raw = '/d /s /c ""' + $helper + '""'
        return Invoke-PreviewProcess -Executable (Join-Path $env:SystemRoot 'System32\cmd.exe') -Arguments @() -WorkingDirectory $Root -TimeoutSeconds $TimeoutSeconds -RawArguments $raw
    } finally {
        Remove-Item -LiteralPath $helper -Force -ErrorAction SilentlyContinue
    }
}

function Get-PreviewBinaries([hashtable]$Toolchain) {
    return @{ shotd = (Join-Path $Toolchain.target 'debug\shotd.exe'); shot = (Join-Path $Toolchain.target 'debug\shot.exe') }
}

# Every build gets its own new UUID directory: a preview or smoke run must never overwrite
# another run's evidence, and the shared .pi/temporary/preview-build.log is not written again.
function New-PreviewBuildLogRoot([string]$Root) {
    $logRoot = Join-Path (Join-Path $Root '.pi\preview-build-logs') ([guid]::NewGuid().ToString('D'))
    if (Test-Path -LiteralPath $logRoot) { throw 'Preview build log collision; refusing overwrite' }
    [void](New-Item -ItemType Directory -Path $logRoot -Force)
    return $logRoot
}

function Build-PreviewBinaries([string]$Root, [hashtable]$Toolchain) {
    # The private log path is allocated before the build and refuses an existing path.
    $log = Join-Path (New-PreviewBuildLogRoot $Root) 'build.log'
    $build = Invoke-PinnedCargo $Root $Toolchain @('build', '--locked', '--offline', '--bin', 'shotd', '--bin', 'shot')
    [System.IO.File]::WriteAllText($log, ("exit=$($build.exit) reason=$($build.reason)`n" + [string]$build.stdout + [string]$build.stderr), (New-Object System.Text.UTF8Encoding($false)))
    $binaries = Get-PreviewBinaries $Toolchain
    if ($build.exit -ne 0 -or -not (Test-Path -LiteralPath $binaries.shotd -PathType Leaf) -or -not (Test-Path -LiteralPath $binaries.shot -PathType Leaf)) {
        throw ('Preview build failed; raw exit ' + $build.exit + '; private build log ' + $log)
    }
    return @{ shotd = $binaries.shotd; shot = $binaries.shot; build_log = $log }
}

function Get-PreviewOwnedRootBase([string]$Root) {
    return (Join-Path (Join-Path $Root '.pi') 'preview')
}

function Get-PreviewOwnedMarkerName() {
    # The ownership marker created only by New-OwnedPreviewRoot; its content is exactly the
    # canonical UUID of its own directory name.
    return 'owned-preview.id'
}

function Test-PreviewCanonicalUuid([string]$Value) {
    if ([string]::IsNullOrEmpty($Value)) { return $false }
    return ($Value -cmatch '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$')
}

function Test-PreviewOwnedRootName([string]$Name) {
    return ($Name -cmatch '^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$')
}

function New-PreviewRefusal([string]$Stage, [string]$Message) {
    # One refusal shape for every guard: ok=false, a stage, a safe message and the observed
    # stop/credential facts (raw exits only, never secret bytes).
    return [ordered]@{
        ok = $false; stage = $Stage; message = $Message; target = $null
        stop_exit = $null; stop_reason = ''; credential_stage = ''
        credential_readback = ''; credential_readback_code = $null
    }
}

function New-OwnedPreviewRoot([string]$Root) {
    $base = Get-PreviewOwnedRootBase $Root
    if (Test-Path -LiteralPath $base -PathType Container) {
        $baseAttributes = (Get-Item -LiteralPath $base -Force).Attributes
        if (($baseAttributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw 'Owned preview base is a reparse point; refusing to create a root inside it'
        }
    }
    $uuid = [guid]::NewGuid().ToString('D')
    $preview = Join-Path $base $uuid
    $data = Join-Path $preview 'root'
    if (-not (Test-PreviewOwnedRootName $uuid)) { throw 'Generated preview identity is not a canonical UUIDv4' }
    if (Test-Path -LiteralPath $preview) { throw 'Owned preview root collision; refusing to reuse' }
    [void](New-Item -ItemType Directory -Path $preview)
    [void](New-Item -ItemType Directory -Path $data)
    foreach ($child in @('data', 'config', 'cache', 'webview')) { [void](New-Item -ItemType Directory -Path (Join-Path $data $child)) }
    [System.IO.File]::WriteAllText((Join-Path $preview (Get-PreviewOwnedMarkerName)), $uuid + "`n", (New-Object System.Text.UTF8Encoding($false)))
    return @{ repo = $Root; preview = $preview; root = $data; config = (Join-Path $preview 'preview.json'); uuid = $uuid }
}

function Get-PreviewTreeElementLimit() {
    # One explicit inspection limit for every owned-tree walk; a tree above it is refused
    # before anything is deleted instead of being walked with an unbounded cost.
    return 10000
}

function Test-OwnedPreviewTreeSafe([string]$Preview, [int]$Limit = 0) {
    # Explicit stack-based walk of the WHOLE tree, with no depth bound and no following of
    # links: a recursive removal must never be allowed to walk out of the owned tree. Each
    # directory is listed non-recursively (Get-ChildItem without -Recurse lists a reparse
    # point itself instead of descending through it, also on Windows PowerShell 5.1) and
    # every entry's attributes are tested BEFORE the entry is ever used as a path. Any
    # reparse point anywhere in the tree, an enumeration error or more elements than the
    # limit is reported unsafe, so the caller refuses with nothing deleted.
    if ($Limit -le 0) { $Limit = Get-PreviewTreeElementLimit }
    $stack = New-Object System.Collections.Stack
    $stack.Push($Preview)
    $counted = 0
    while ($stack.Count -gt 0) {
        $directory = [string]$stack.Pop()
        $entries = $null
        try { $entries = @(Get-ChildItem -LiteralPath $directory -Force -ErrorAction Stop) }
        catch { return @{ safe = $false; reason = 'read-error'; counted = $counted } }
        foreach ($entry in $entries) {
            $counted++
            if ($counted -gt $Limit) { return @{ safe = $false; reason = 'limit'; counted = $counted } }
            if (($entry.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                return @{ safe = $false; reason = 'reparse'; counted = $counted }
            }
            if ($entry.PSIsContainer) { $stack.Push($entry.FullName) }
        }
    }
    return @{ safe = $true; reason = ''; counted = $counted }
}

function Resolve-OwnedPreviewRoot([string]$Root, [string]$PreviewRoot) {
    # A cleanup may only act on a directory this preview created: the strict parent
    # <repo>/.pi/preview, a canonical UUIDv4 directory name and an ownership marker whose
    # content is exactly that UUID. Anything else is refused before any side effect.
    if ([string]::IsNullOrWhiteSpace($PreviewRoot)) { return (New-PreviewRefusal 'arguments' 'no preview root was given') }
    $base = [System.IO.Path]::GetFullPath((Get-PreviewOwnedRootBase $Root)).TrimEnd('\')
    $candidate = $null
    try { $candidate = [System.IO.Path]::GetFullPath($PreviewRoot).TrimEnd('\') } catch { return (New-PreviewRefusal 'path' 'preview root is not a usable local path') }
    if ((Split-Path -Path $candidate -Parent) -ine $base) {
        return (New-PreviewRefusal 'parent' ('refusing a path that is not a direct child of ' + $base))
    }
    $name = Split-Path -Path $candidate -Leaf
    if (-not (Test-PreviewOwnedRootName $name)) { return (New-PreviewRefusal 'name' 'preview directory name is not a canonical UUIDv4') }
    if (-not (Test-Path -LiteralPath $candidate -PathType Container)) { return (New-PreviewRefusal 'missing' 'owned preview directory does not exist') }
    if (-not (Test-Path -LiteralPath $base -PathType Container)) { return (New-PreviewRefusal 'missing' 'owned preview base directory does not exist') }
    foreach ($path in @($base, $candidate)) {
        if ((Get-Item -LiteralPath $path -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint) {
            return (New-PreviewRefusal 'reparse' 'owned preview path is a reparse point')
        }
    }
    $marker = Join-Path $candidate (Get-PreviewOwnedMarkerName)
    if (-not (Test-Path -LiteralPath $marker -PathType Leaf)) {
        return (New-PreviewRefusal 'marker' 'ownership marker is missing; only a root created by this preview may be cleaned')
    }
    if ((Get-Item -LiteralPath $marker -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint) {
        return (New-PreviewRefusal 'marker' 'ownership marker is a reparse point')
    }
    if (([System.IO.File]::ReadAllText($marker)).Trim() -cne $name) {
        return (New-PreviewRefusal 'marker' 'ownership marker does not match this directory')
    }
    $config = Join-Path $candidate 'preview.json'
    if (-not (Test-Path -LiteralPath $config -PathType Leaf)) { return (New-PreviewRefusal 'config' 'preview configuration is missing') }
    $tree = Test-OwnedPreviewTreeSafe $candidate
    if (-not $tree.safe) {
        if ($tree.reason -eq 'limit') {
            return (New-PreviewRefusal 'size' ('owned preview tree exceeds the ' + (Get-PreviewTreeElementLimit) + '-element inspection limit; nothing was deleted'))
        }
        if ($tree.reason -eq 'read-error') {
            return (New-PreviewRefusal 'tree-read' 'owned preview tree could not be enumerated safely; nothing was deleted')
        }
        return (New-PreviewRefusal 'reparse' 'owned preview tree contains a reparse point')
    }
    return @{
        ok = $true; stage = 'owned'; message = 'owned preview root'
        repo = [System.IO.Path]::GetFullPath($Root); preview = $candidate; root = (Join-Path $candidate 'root')
        config = $config; uuid = $name
        target = $null; stop_exit = $null; stop_reason = ''; credential_stage = ''
    }
}

function Initialize-OwnedPreviewLibrary([string]$Root, [string]$Shotd, [string]$DataRoot) {
    $result = Invoke-PreviewProcess $Shotd @('--init', '--root', $DataRoot) $Root 120
    if ($result.exit -ne 0) { throw ('Library initialization failed; raw exit ' + $result.exit) }
    $report = ConvertFrom-Json $result.stdout
    if (-not $report.library_id) { throw 'Library initialization returned no identity' }
    return $report
}

function Write-TrustedPreviewConfig([hashtable]$Preview, [string]$LibraryId, [string]$Shotd) {
    $config = [ordered]@{
        schema_version = 1
        environment = 'test'
        root = $Preview.root
        library_id = $LibraryId
        host_executable = $Shotd
    }
    if (-not (Test-PreviewCanonicalUuid ([string]$LibraryId))) { throw 'First-run library identity is not a canonical UUID; refusing to write a trusted config' }
    $text = (ConvertTo-Json -InputObject $config -Depth 4) -replace "`r`n", "`n"
    [System.IO.File]::WriteAllText($Preview.config, $text + "`n", (New-Object System.Text.UTF8Encoding($false)))
    return $Preview.config
}

function Get-OwnedShotdProcess([string]$DataRoot) {
    # Only processes whose command line names this exact owned root; never a foreign or
    # system-wide image inventory. Read-only inspection, no termination here. The leading
    # comma keeps the result an array so a single match still reports .Count = 1.
    return ,@(Get-CimInstance Win32_Process -Filter "Name = 'shotd.exe'" |
        Where-Object { $_.CommandLine -and $_.CommandLine.Contains($DataRoot) })
}

function Wait-OwnedShotdGone([string]$DataRoot, [int]$TimeoutSeconds = 5) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if ((Get-OwnedShotdProcess $DataRoot).Count -eq 0) { return $true }
        Start-Sleep -Milliseconds 100
    }
    return ((Get-OwnedShotdProcess $DataRoot).Count -eq 0)
}

function Test-OwnedPreviewQuiescent([string]$DataRoot) {
    # Own root only: no image-name inventory and no foreign PID is ever inspected or touched.
    return ((Get-OwnedShotdProcess $DataRoot).Count -eq 0)
}

# Test-only seam: scripts/test-preview-safety.ps1 assigns this script-scope table (with a
# `status` and a `code`) to substitute the native credential readback result. Production
# never sets it, and an unset seam always uses the real CredReadW readback below. The seam
# only replaces the readback verdict; the exact cmdkey delete still really happens.
$script:PreviewCredentialReadbackSeam = $null
$script:PreviewCredentialReadbackReady = $false

function Initialize-OwnedCredentialReadback() {
    # Minimal advapi32 P/Invoke for one unambiguous native readback. Nothing but the raw
    # Win32 code leaves the helper: the credential blob and the user name are never read and
    # never printed. Compiled once per session and needed only when a credential is deleted.
    if ($script:PreviewCredentialReadbackReady) { return }
    $source = @'
using System;
using System.Runtime.InteropServices;
public static class ScreenMemoryPreviewCredentialReadback {
    [DllImport("advapi32.dll", EntryPoint = "CredReadW", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CredReadW(string target, int type, int flags, out IntPtr credential);
    [DllImport("advapi32.dll", EntryPoint = "CredFree")]
    private static extern void CredFree(IntPtr buffer);
    public static int Read(string target) {
        IntPtr handle;
        if (CredReadW(target, 1, 0, out handle)) {
            if (handle != IntPtr.Zero) { CredFree(handle); }
            return 0;
        }
        return Marshal.GetLastWin32Error();
    }
}
'@
    try { Add-Type -TypeDefinition $source -Language CSharp | Out-Null }
    catch {
        # A repeated Add-Type in one session reports a duplicate type; only a genuinely
        # missing type is a real failure.
        if (-not ([System.Management.Automation.PSTypeName]'ScreenMemoryPreviewCredentialReadback').Type) { throw }
    }
    $script:PreviewCredentialReadbackReady = $true
}

function Get-OwnedCredentialPresence([string]$Target) {
    # The only accepted proof of absence. CredReadW success means the exact target still
    # exists, GetLastError 1168 (ERROR_NOT_FOUND) is the confirmed absence, and every other
    # code is an unresolved state that must be refused. A non-zero cmdkey exit is never
    # treated as evidence of absence.
    if ($script:PreviewCredentialReadbackSeam) {
        return @{ status = [string]$script:PreviewCredentialReadbackSeam.status; code = [int]$script:PreviewCredentialReadbackSeam.code }
    }
    Initialize-OwnedCredentialReadback
    $code = [ScreenMemoryPreviewCredentialReadback]::Read($Target)
    if ($code -eq 0) { return @{ status = 'exists'; code = 0 } }
    if ($code -eq 1168) { return @{ status = 'absent'; code = 1168 } }
    return @{ status = 'error'; code = $code }
}

function Remove-OwnedTestCredential([string]$LibraryId) {
    # Exact synthetic Test-namespace target of this owned library only: no enumeration,
    # no other target, and no shell. The target is only built from a strict canonical UUID,
    # so no character that a command interpreter could reuse can ever reach cmdkey.exe; the
    # executable is invoked directly with an argument list. The exact delete must report
    # success, and absence is then proven ONLY by the native CredReadW readback: success
    # (the credential is still there) and every code other than 1168 refuse the cleanup.
    if (-not (Test-PreviewCanonicalUuid $LibraryId)) {
        return @{ ok = $false; target = $null; stage = 'identity'; exit = $null; readback = 'not-run'; readback_code = $null }
    }
    $target = 'ScreenMemory.Test.V1/library/' + $LibraryId + '/owner'
    $cmdkey = Join-Path $env:SystemRoot 'System32\cmdkey.exe'
    if (-not (Test-Path -LiteralPath $cmdkey -PathType Leaf)) {
        return @{ ok = $false; target = $target; stage = 'missing-cmdkey'; exit = $null; readback = 'not-run'; readback_code = $null }
    }
    $delete = Invoke-PreviewProcess $cmdkey @('/delete:' + $target) $env:SystemDrive 60
    if ($delete.exit -ne 0) {
        return @{ ok = $false; target = $target; stage = 'delete'; exit = $delete.exit; readback = 'not-run'; readback_code = $null }
    }
    $presence = $null
    try { $presence = Get-OwnedCredentialPresence $target }
    catch {
        return @{ ok = $false; target = $target; stage = 'absence-unavailable'; exit = $delete.exit; readback = 'failed'; readback_code = $null }
    }
    if ($presence.status -eq 'exists') {
        return @{ ok = $false; target = $target; stage = 'absence'; exit = $delete.exit; readback = $presence.status; readback_code = $presence.code }
    }
    if ($presence.status -ne 'absent') {
        return @{ ok = $false; target = $target; stage = 'absence-error'; exit = $delete.exit; readback = $presence.status; readback_code = $presence.code }
    }
    return @{ ok = $true; target = $target; stage = 'verified'; exit = $delete.exit; readback = $presence.status; readback_code = $presence.code }
}

function Remove-OwnedPreviewRoot([hashtable]$Preview) {
    # Ownership is re-proven here (strict parent, UUIDv4 name and marker) so no caller can
    # turn this helper into a recursive delete of an arbitrary directory.
    if (-not $Preview -or -not $Preview.repo -or -not $Preview.preview) { return $false }
    $owned = Resolve-OwnedPreviewRoot $Preview.repo $Preview.preview
    if (-not $owned.ok) { return $false }
    if (Test-Path -LiteralPath $owned.preview) { Remove-Item -LiteralPath $owned.preview -Recurse -Force }
    return -not (Test-Path -LiteralPath $owned.preview)
}

function Invoke-OwnedPreviewCleanup([string]$Root, [string]$PreviewRoot, [string]$Shot = '', [int]$StopTimeoutSeconds = 120) {
    # Order: prove ownership (nothing else is ever touched) -> canonical library id ->
    # stop and prove the owned host gone -> exact synthetic credential delete plus absence
    # -> re-prove quiescence and ownership -> remove the owned root. Any unsuccessful or
    # uncertain step refuses with a non-zero exit and deletes nothing.
    $owned = Resolve-OwnedPreviewRoot $Root $PreviewRoot
    if (-not $owned.ok) { return $owned }
    $config = $null
    try { $config = ConvertFrom-Json ([System.IO.File]::ReadAllText($owned.config)) }
    catch { return (New-PreviewRefusal 'config' 'preview configuration is not readable JSON') }
    $libraryId = [string]$config.library_id
    if (-not (Test-PreviewCanonicalUuid $libraryId)) {
        return (New-PreviewRefusal 'identity' 'preview configuration library id is not a canonical UUID; refusing to build a credential target')
    }
    $result = New-PreviewRefusal 'pending' 'no step has completed yet'
    if (-not (Test-OwnedPreviewQuiescent $owned.root)) {
        if (-not $Shot -or -not (Test-Path -LiteralPath $Shot -PathType Leaf)) {
            return (New-PreviewRefusal 'binary' 'an owned shotd.exe is still running for this root but the owning CLI image is unavailable; nothing was deleted')
        }
        $stop = Invoke-PreviewProcess $Shot @('--config', $owned.config, 'host', 'stop', '--json') $Root $StopTimeoutSeconds
        $result.stop_exit = $stop.exit
        $result.stop_reason = [string]$stop.reason
        if ($stop.reason -ne 'OK' -or $stop.exit -ne 0) {
            $result.stage = 'stop'
            $result.message = 'owned host stop did not succeed (reason ' + $stop.reason + ', raw exit ' + $stop.exit + '); the root and the exact credential are kept for reconciliation'
            return $result
        }
        if (-not (Wait-OwnedShotdGone $owned.root 5)) {
            $result.stage = 'process'
            $result.message = 'own shotd.exe is still alive after the bounded wait; nothing was deleted'
            return $result
        }
    }
    $credential = Remove-OwnedTestCredential $libraryId
    $result.target = $credential.target
    $result.credential_stage = $credential.stage
    $result.credential_readback = [string]$credential.readback
    $result.credential_readback_code = $credential.readback_code
    if (-not $credential.ok) {
        $result.stage = 'credential-' + $credential.stage
        $result.message = 'exact synthetic credential cleanup did not confirm absence at stage ' + $credential.stage +
            ' (delete raw exit ' + $credential.exit + ', native readback ' + $credential.readback + ' code ' + $credential.readback_code + '); the owned root is kept for reconciliation'
        return $result
    }
    if (-not (Test-OwnedPreviewQuiescent $owned.root)) {
        $result.stage = 'process'
        $result.message = 'own shotd.exe appeared while cleaning; the owned root is kept for reconciliation'
        return $result
    }
    if (-not (Remove-OwnedPreviewRoot $owned)) {
        $result.stage = 'remove'
        $result.message = 'owned preview root removal failed; reconcile manually'
        return $result
    }
    $result.ok = $true
    $result.stage = 'removed'
    $result.message = 'owned preview root removed'
    return $result
}

function New-PreviewOutputRoot([string]$Root, [string]$Name) {
    $private = Join-Path $Root '.pi'
    $output = Join-Path $private ($Name + '-' + [guid]::NewGuid().ToString('N'))
    if (Test-Path -LiteralPath $output) { throw 'Output directory collision; refusing overwrite' }
    return $output
}

function Get-PreviewCommandLine([string]$Shot, [string]$Config) {
    return ('"' + $Shot + '" --config "' + $Config + '" status --json')
}
