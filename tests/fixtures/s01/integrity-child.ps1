[CmdletBinding()]
param([ValidateSet('source-modify','source-create','source-delete','source-fail')][string]$Mode, [string]$HostRoot)
$ErrorActionPreference = 'Stop'
# The wrapper accepts this child only from an isolated copied test host under .pi.
if ($HostRoot -notmatch '[\\/]\.pi[\\/]test-verify-[0-9a-f]{32}[\\/]host$') { throw 'Synthetic host required' }
$source = Join-Path $HostRoot 'scripts\guard-source.txt'
switch ($Mode) {
    'source-modify' { [System.IO.File]::WriteAllText($source, 'modified') }
    'source-create' { [System.IO.File]::WriteAllText((Join-Path $HostRoot 'scripts\guard-created.txt'), 'created') }
    'source-delete' { Remove-Item -LiteralPath $source }
    'source-fail' { [System.IO.File]::WriteAllText($source, 'failed-child-modification'); exit 7 }
}
