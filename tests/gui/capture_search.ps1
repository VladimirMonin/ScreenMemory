param([int]$OwnedPid,[string]$Config,[string]$Output)
$ErrorActionPreference='Stop'
$repo=(Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if(Test-Path -LiteralPath $Output){throw 'Absent output required'}
if(-not $Output.StartsWith((Join-Path $repo '.pi\temporary\'),[StringComparison]::OrdinalIgnoreCase)){throw 'Private owned output required'}
$proc=Get-CimInstance Win32_Process -Filter ('ProcessId = '+$OwnedPid)
$expected=Join-Path $repo '.pi\staging-target-t09b\debug\screenmemory-gui.exe'
if(-not $proc -or $proc.ExecutablePath -cne $expected -or $proc.CommandLine -notlike ('*'+$Config+'*')){throw 'Exact own process/config refused'}
$gui=Get-Process -Id $OwnedPid
if($gui.MainWindowHandle -eq [IntPtr]::Zero){throw 'Own visible HWND absent'}
Add-Type -TypeDefinition @'
using System;using System.Runtime.InteropServices;
public struct Rect{public int Left,Top,Right,Bottom;}
public static class OwnCapture{
[DllImport("user32.dll")]public static extern bool SetProcessDpiAwarenessContext(IntPtr c);
[DllImport("user32.dll")]public static extern bool GetWindowRect(IntPtr h,out Rect r);
[DllImport("user32.dll")]public static extern bool PrintWindow(IntPtr h,IntPtr dc,uint f);
[DllImport("user32.dll")]public static extern uint GetWindowThreadProcessId(IntPtr h,out uint p);
[DllImport("user32.dll")]public static extern uint GetDpiForWindow(IntPtr h);
}
'@
if(-not [OwnCapture]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))){throw 'Physical DPI awareness refused'}
[uint32]$owner=0;[void][OwnCapture]::GetWindowThreadProcessId($gui.MainWindowHandle,[ref]$owner)
if($owner -ne $OwnedPid){throw 'HWND owner mismatch'}
Add-Type -AssemblyName System.Drawing
$r=New-Object Rect
if(-not [OwnCapture]::GetWindowRect($gui.MainWindowHandle,[ref]$r)){throw 'Window rect refused'}
$w=$r.Right-$r.Left;$h=$r.Bottom-$r.Top
if($w -lt 100 -or $h -lt 100 -or $w -gt 4096 -or $h -gt 4096){throw 'Geometry refused'}
$bitmap=New-Object System.Drawing.Bitmap($w,$h);$g=[System.Drawing.Graphics]::FromImage($bitmap)
try{$dc=$g.GetHdc();try{$ok=[OwnCapture]::PrintWindow($gui.MainWindowHandle,$dc,2)}finally{$g.ReleaseHdc($dc)};if(-not $ok){throw 'PrintWindow refused'};$bitmap.Save($Output,[System.Drawing.Imaging.ImageFormat]::Png);Write-Output ('OWN_HWND='+$gui.MainWindowHandle+' DPI='+[OwnCapture]::GetDpiForWindow($gui.MainWindowHandle)+' PHYSICAL='+$w+'x'+$h+' INPUT=NOT_PROVEN')}finally{$g.Dispose();$bitmap.Dispose()}
