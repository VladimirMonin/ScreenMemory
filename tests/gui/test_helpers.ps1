$ErrorActionPreference='Stop'
foreach($name in @('check_endpoint.ps1','capture_search.ps1','close_search.ps1')){
  $tokens=$null;$errors=$null
  [void][System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot $name),[ref]$tokens,[ref]$errors)
  if($errors.Count){throw 'Tracked owned helper syntax refused'}
}
Write-Output 'TRACKED_HELPER_PS_SYNTAX_PASS'
