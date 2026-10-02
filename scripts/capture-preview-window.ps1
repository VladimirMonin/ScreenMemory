# Agent evidence only: PrintWindow of one proven owned synthetic GUI HWND, never desktop.
[CmdletBinding()]
param([Parameter(Mandatory=$true)][int]$OwnedProcessId,
    [Parameter(Mandatory=$true)][string]$PreviewRoot,
    [Parameter(Mandatory=$true)][string]$OutputRoot)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'preview-common.ps1')
. (Join-Path $PSScriptRoot 'verification-common.ps1')
$repo = Get-PreviewRepositoryRoot $PSScriptRoot
$owned = Resolve-OwnedPreviewRoot $repo $PreviewRoot
if (-not $owned.ok) { throw 'Refusing an unowned preview root' }
$process = Get-CimInstance Win32_Process -Filter ('ProcessId = ' + $OwnedProcessId)
$image = Get-PreviewGuiBinary (Get-PreviewToolchain $repo)
if (-not $process -or $process.Name -cne 'screenmemory-gui.exe' -or
    $process.ExecutablePath -cne $image -or
    $process.CommandLine -notmatch ('--config\s+(?:"' + [regex]::Escape($owned.config) + '"|' + [regex]::Escape($owned.config) + ')(?:\s|$)')) { throw 'Refusing a non-owned GUI process' }
$state = New-VerificationState $repo $OutputRoot 'owned-window-evidence'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public struct PreviewRect { public int Left, Top, Right, Bottom; }
public static class PreviewWindowEvidence {
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out PreviewRect rect);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr dc, uint flags);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
}
'@
if (-not [PreviewWindowEvidence]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))) { throw 'Physical DPI awareness denied' }
$gui = Get-Process -Id $OwnedProcessId -ErrorAction Stop
$hwnd = $gui.MainWindowHandle
[uint32]$actualOwner = 0
[void][PreviewWindowEvidence]::GetWindowThreadProcessId($hwnd, [ref]$actualOwner)
if ($hwnd -eq [IntPtr]::Zero -or $actualOwner -ne $OwnedProcessId) { throw 'Owned HWND unavailable' }
$rect = New-Object PreviewRect
if (-not [PreviewWindowEvidence]::GetWindowRect($hwnd, [ref]$rect)) { throw 'Owned window bounds unavailable' }
$width = $rect.Right - $rect.Left; $height = $rect.Bottom - $rect.Top
if ($width -lt 100 -or $height -lt 100 -or $width -gt 4096 -or $height -gt 4096) { throw 'Owned window dimensions out of bounds' }
Add-Type -AssemblyName System.Drawing
$bitmap = New-Object System.Drawing.Bitmap($width, $height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
try {
    $dc = $graphics.GetHdc()
    try { $ok = [PreviewWindowEvidence]::PrintWindow($hwnd, $dc, 2) }
    finally { $graphics.ReleaseHdc($dc) }
    if (-not $ok) { throw 'PrintWindow refused owned GUI' }
    $path = Join-Path $state.output 'owned-window.png'
    $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $report = [ordered]@{ schema_version = 1; outcome = 'PASS'; method = 'PrintWindow-owned-HWND';
        width = $width; height = $height; dpi = [PreviewWindowEvidence]::GetDpiForWindow($hwnd);
        image = 'owned-window.png'; sha256 = (Get-FileHash $path -Algorithm SHA256).Hash.ToLowerInvariant(); desktop_capture = $false }
    Write-VerificationJson (Join-Path $state.output 'result.json') $report
    Write-Output (ConvertTo-Json $report -Compress)
} finally { $graphics.Dispose(); $bitmap.Dispose(); $gui.Dispose() }
