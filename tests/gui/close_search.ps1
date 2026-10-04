param([int]$OwnedPid,[long]$CreatedTicks,[string]$Config,[string]$ConfigSha256)
$ErrorActionPreference='Stop'
$repo=(Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$proc=Get-CimInstance Win32_Process -Filter ('ProcessId = '+$OwnedPid)
$expected=Join-Path $repo '.pi\staging-target-t09b\debug\screenmemory-gui.exe'
if(-not $proc -or $proc.ExecutablePath -cne $expected -or $proc.CreationDate.ToUniversalTime().Ticks -ne $CreatedTicks){throw 'Recorded GUI identity refused'}
if((Get-FileHash -LiteralPath $Config -Algorithm SHA256).Hash.ToLowerInvariant() -cne $ConfigSha256){throw 'Pinned config mutation'}
$gui=Get-Process -Id $OwnedPid
if($gui.MainWindowHandle -eq [IntPtr]::Zero){throw 'Owned GUI HWND absent'}
Add-Type -TypeDefinition @'
using System;using System.Runtime.InteropServices;
public static class OwnClose{
[DllImport("user32.dll")]public static extern uint GetWindowThreadProcessId(IntPtr h,out uint p);
[DllImport("user32.dll")]public static extern bool PostMessageW(IntPtr h,uint m,IntPtr w,IntPtr l);
}
'@
[uint32]$owner=0;[void][OwnClose]::GetWindowThreadProcessId($gui.MainWindowHandle,[ref]$owner)
if($owner -ne $OwnedPid){throw 'Owned HWND mismatch'}
if(-not [OwnClose]::PostMessageW($gui.MainWindowHandle,0x10,[IntPtr]::Zero,[IntPtr]::Zero)){throw 'Own WM_CLOSE refused'}
Write-Output 'Own WM_CLOSE posted; NOT genuine input'
