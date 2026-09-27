# S01-T04a: schema/TS generation is confined to new private dirs, never overwrite-first.
[CmdletBinding()]
param([string]$OutputRoot)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'verification-common.ps1')
$root = Split-Path $PSScriptRoot -Parent
$state = $null
$before = $null
try {
    if (-not $OutputRoot) { $OutputRoot = Join-Path $root ('.pi\contracts-' + [guid]::NewGuid().ToString('N')) }
    $state = New-VerificationState $root $OutputRoot 'contracts'
    try { Assert-PlanHashes $root; Add-VerificationGuardStep $state 'PLAN-HASH-PRE' 'OK' }
    catch { Add-VerificationGuardStep $state 'PLAN-HASH-PRE' 'PLAN_HASH_MISMATCH'; throw }
    $before = Get-SourceSnapshot $root
    Add-VerificationGuardStep $state 'SOURCE-INTEGRITY-PRE' 'OK'
    $tools = Get-VerificationEnvironment $root $state.output
    $state.versions = @{ rust = '1.98.1'; node = '24.13.0'; pnpm = '10.32.1'; python_jsonschema = '4.26.0' }
    $envMap = $tools.env
    Assert-VerificationVersions $state $tools
    $rustOut = Join-Path $state.output 'rust-generated'
    $tsOut = Join-Path $state.output 'ts-generated'
    Add-VerificationStep $state 'SCHEMA-GENERATE' $tools.rust @('run','--locked','-p','sm-protocol','--example','generate_schemas','--','--output',$rustOut) $envMap
    if (-not $state.failed) {
        foreach ($name in @('application-v1.schema.json','application-methods-v1.schema.json','bridge-v1.schema.json','foundation-errors.schema.json')) {
            if ((Get-FileHash -LiteralPath (Join-Path $rustOut $name) -Algorithm SHA256).Hash -ne (Get-FileHash -LiteralPath (Join-Path $root "schemas\$name") -Algorithm SHA256).Hash) { throw 'Generated Rust schema drift' }
        }
    }
    Add-VerificationStep $state 'SCHEMA-CHECK' $tools.rust @('run','--locked','-p','sm-protocol','--example','generate_schemas','--','--check','schemas') $envMap
    Add-VerificationStep $state 'TS-GENERATE' $tools.node @((Join-Path $root 'packages\protocol\scripts\generate.mjs'),'--output',$tsOut) $envMap
    if (-not $state.failed) {
        foreach ($name in @('application-v1.ts','application-methods-v1.ts','bridge-v1.ts','foundation-errors.ts')) {
            if ((Get-FileHash -LiteralPath (Join-Path $tsOut $name) -Algorithm SHA256).Hash -ne (Get-FileHash -LiteralPath (Join-Path $root "packages\protocol\src\generated\$name") -Algorithm SHA256).Hash) { throw 'Generated TS contract drift' }
        }
    }
    Add-VerificationStep $state 'PNPM-CONTRACTS' $tools.node @($tools.pnpm,'contracts:check') $envMap
    Add-VerificationStep $state 'BOUNDARIES' $tools.python @((Join-Path $root 'tests\contracts\check_boundaries.py')) $envMap
    Add-VerificationStep $state 'SCHEMA-FIXTURES' $tools.python @((Join-Path $root 'tests\contracts\check_schemas.py')) $envMap
} catch {
    if ($state) { Add-VerificationGuardStep $state 'CONTRACTS-PREFLIGHT' 'PREREQUISITE_OR_DRIFT' }
    [Console]::Error.WriteLine('Contract verification failed; no exception payload disclosed.')
} finally {
    if ($state -and $before) {
        try { Assert-PlanHashes $root; Add-VerificationGuardStep $state 'PLAN-HASH-POST' 'OK' }
        catch { Add-VerificationGuardStep $state 'PLAN-HASH-POST' 'PLAN_HASH_MISMATCH' }
        try {
            $after = Get-SourceSnapshot $root
            Add-VerificationGuardStep $state 'SOURCE-INTEGRITY-POST' $(if (Test-SourceSnapshot $before $after) { 'OK' } else { 'SOURCE_MUTATION' })
        } catch { Add-VerificationGuardStep $state 'SOURCE-INTEGRITY-POST' 'SOURCE_INVENTORY_ERROR' }
    }
}
if ($state) { exit (Complete-Verification $state @('FRONTEND-CONTRACTS','BRIDGE-TRANSPORT','HOST-SECURITY')) }
exit 1
