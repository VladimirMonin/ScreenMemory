param([Parameter(Mandatory=$true)][int]$OwnedPid,[Parameter(Mandatory=$true)][int]$Port)
$ErrorActionPreference='Stop'
if($OwnedPid -le 0 -or $Port -lt 1 -or $Port -gt 65535){throw 'Invalid recorded owner endpoint'}
# Exact chosen port only. No foreign scan, alternate endpoint or payload read.
try {
  $s=@(Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction Stop)
} catch {
  # This exact structured CIM no-match alone is retryable, never stderr text.
  if($_.FullyQualifiedErrorId -cne 'CmdletizationQuery_NotFound,Get-NetTCPConnection'){throw}
  [Console]::Error.WriteLine(($_ | Out-String))
  @{outcome='not_ready';fully_qualified_error_id=$_.FullyQualifiedErrorId} | ConvertTo-Json -Compress
  exit 3
}
if($s.Count -lt 1){throw 'Malformed empty exact-port query result'}
foreach($listener in $s){
  $ancestor=$listener.OwningProcess
  $owned=$false
  for($n=0;$n -lt 8;$n++){
    if($ancestor -eq $OwnedPid){$owned=$true;break}
    $row=Get-CimInstance Win32_Process -Filter ('ProcessId='+$ancestor)
    if(-not $row){break}
    $ancestor=$row.ParentProcessId
  }
  if(-not $owned){throw 'Chosen endpoint provenance mismatch'}
}
@{outcome='owned'} | ConvertTo-Json -Compress
