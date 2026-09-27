# Synthetic native child for the bounded verifier regression. No application/user data.
param([ValidateSet('success','exit7','stderr','timeout')][string]$Mode, [string]$Label)
if ($Label -ne 'two words') { exit 9 }
switch ($Mode) {
    'exit7' { exit 7 }
    'stderr' { [Console]::Error.WriteLine('synthetic-secret-canary'); exit 0 }
    'timeout' { Start-Sleep -Seconds 10; exit 0 }
    default { exit 0 }
}
