# S01-ENV-LF-01: test emitted bytes and native process exit codes on Windows PowerShell 5.1.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -ne 5) { throw 'Requires Windows PowerShell 5.1' }
$collector = Join-Path $PSScriptRoot 'collect-environment.ps1'
$powerShell = Join-Path $PSHOME 'powershell.exe'
$root = Join-Path (Split-Path -Parent $PSScriptRoot) ('.pi\test-collect-environment-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root -ErrorAction Stop | Out-Null
try {
    $output = Join-Path $root 'environment.json'
    & $powerShell -NoProfile -NonInteractive -File $collector -OutputPath $output 2>&1 | Out-Null
    $successExit = $LASTEXITCODE
    if ($successExit -ne 0) { throw "Collector success exit: $successExit (expected 0)" }
    $bytes = [System.IO.File]::ReadAllBytes($output)
    if ($bytes.Length -lt 2) { throw 'Output is empty' }
    if ($bytes.Length -ge 3 -and $bytes[0] -eq 239 -and $bytes[1] -eq 187 -and $bytes[2] -eq 191) { throw 'UTF-8 BOM present' }
    if ($bytes -contains [byte]13) { throw 'CR byte present' }
    if ($bytes[$bytes.Length - 1] -ne 10 -or $bytes[$bytes.Length - 2] -eq 10) { throw 'Expected one final LF' }
    $utf8 = New-Object System.Text.UTF8Encoding($false, $true)
    $text = $utf8.GetString($bytes)
    $parsed = ConvertFrom-Json -InputObject $text -ErrorAction Stop
    if ($parsed.task -ne 'S01-T02a') { throw 'Unexpected JSON task' }

    # A failed write must reach the calling process as a nonzero exit, not a false PASS.
    $badOutput = Join-Path $root 'missing-parent\environment.json'
    # PowerShell 5.1 promotes child stderr to NativeCommandError under Stop.
    $ErrorActionPreference = 'Continue'
    & $powerShell -NoProfile -NonInteractive -File $collector -OutputPath $badOutput 2>&1 | Out-Null
    $failureExit = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($failureExit -eq 0 -or (Test-Path $badOutput)) { throw "Failed write exit: $failureExit (expected nonzero and no file)" }
    Write-Output "S01-ENV-LF-01 PASS: success_exit=$successExit failure_exit=$failureExit UTF8_NO_BOM LF_ONLY JSON_OK FINAL_LF"
} finally {
    Remove-Item -LiteralPath $root -Recurse -Force
}
