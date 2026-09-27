# S01-T02a: read-only, public-safe Windows build preflight. No process launch of donor.
[CmdletBinding()]
param([string]$OutputPath)
$ErrorActionPreference = 'Stop'
function VersionOf([string]$Name, [string[]]$Arguments) {
    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    if (-not $cmd) { return @{ status = 'NOT_FOUND'; version = $null; exit_code = $null } }
    try {
        $lines = @(& $cmd.Source @Arguments 2>&1 | ForEach-Object { $_.ToString() })
        $code = $LASTEXITCODE
        $first = @($lines | Where-Object { $_ -match '\S' } | Select-Object -First 1)
        return @{ status = $(if ($code -eq 0) { 'FOUND' } else { 'FAILED' }); version = $(if ($first.Count) { $first[0] } else { $null }); exit_code = $code }
    } catch { return @{ status = 'FAILED'; version = $null; exit_code = $null } }
}
$os = Get-CimInstance Win32_OperatingSystem
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vs = @()
if (Test-Path $vswhere) {
    $instances = & $vswhere -all -products '*' -format json | ConvertFrom-Json
    foreach ($instance in $instances) {
        if (-not $instance) { continue }
        $toolsetFile = Join-Path $instance.installationPath 'VC\Auxiliary\Build\Microsoft.VCToolsVersion.default.txt'
        $cmakeExe = Join-Path $instance.installationPath 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe'
        $vs += @{ version = $instance.installationVersion; product = $instance.productId; complete = [bool]$instance.isComplete;
            msvc_developer_environment = (Test-Path (Join-Path $instance.installationPath 'VC\Auxiliary\Build\vcvars64.bat'));
            msvc_toolset = $(if (Test-Path $toolsetFile) { (Get-Content $toolsetFile -First 1).Trim() } else { $null });
            bundled_cmake = $(if (Test-Path $cmakeExe) { VersionOf $cmakeExe @('--version') } else { $null }) }
    }
}
$sdkKey = 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots'
$sdk = @()
if (Test-Path $sdkKey) {
    $kitsRoot = (Get-ItemProperty $sdkKey).KitsRoot10
    if ($kitsRoot -and (Test-Path (Join-Path $kitsRoot 'Lib'))) {
        $sdk = @(Get-ChildItem (Join-Path $kitsRoot 'Lib') -Directory | Where-Object { $_.Name -match '^10\.\d+\.\d+\.\d+$' } | ForEach-Object { $_.Name } | Sort-Object)
    }
}
$webview = @()
$webviewId = '{F1E7E228-C908-4EC1-8F5F-CA740551B903}'
foreach ($hive in @('HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients', 'HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients', 'HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients')) {
    $key = Join-Path $hive $webviewId
    if (Test-Path $key) { $webview += @{ scope = $hive.Split(':')[0]; version = (Get-ItemProperty $key).pv } }
}
$portableRust = Join-Path (Split-Path -Parent $PSScriptRoot) '.pi\toolchains\rust-1.98.1-x86_64-pc-windows-msvc\bin\rustc.exe'
$data = [ordered]@{
    schema_version = 1; stage = 'S01'; task = 'S01-T02a'; captured_at_utc = (Get-Date).ToUniversalTime().ToString('o')
    os = @{ version = $os.Version; build = $os.BuildNumber; architecture = $os.OSArchitecture }
    shell = @{ name = 'Windows PowerShell'; version = $PSVersionTable.PSVersion.ToString() }
    tools = @{ git = (VersionOf 'git' @('--version')); rustc = (VersionOf 'rustc' @('-Vv')); cargo = (VersionOf 'cargo' @('--version')); rustup = (VersionOf 'rustup' @('show','active-toolchain')); node = (VersionOf 'node' @('--version')); pnpm = (VersionOf 'pnpm' @('--version')); cmake_path = (VersionOf 'cmake' @('--version')) }
    visual_studio = $vs; windows_sdk_versions = $sdk; webview2_evergreen_registry = $webview
    portable_rustc = (VersionOf $portableRust @('-Vv'))
    webview2_status = $(if ($webview.Count) { 'REGISTRY_FOUND_NOT_RUNTIME_TESTED' } else { 'NOT_FOUND_IN_PROBED_REGISTRY_NOT_RUNTIME_TESTED' })
    locale = [System.Globalization.CultureInfo]::CurrentCulture.Name
    gpu_driver = 'NOT_PROBED: build-only slice'; display_topology = 'NOT_PROBED: no desktop interaction'; runtime = 'NOT_RUN'
}
$json = ConvertTo-Json -InputObject $data -Depth 8
if ($OutputPath) {
    # ConvertTo-Json uses Windows newlines in Windows PowerShell 5.1; write a portable LF artifact.
    $lfJson = ($json -replace "`r`n|`r|`n", "`n").TrimEnd([char[]]@("`r", "`n")) + "`n"
    [System.IO.File]::WriteAllText($OutputPath, $lfJson, (New-Object System.Text.UTF8Encoding($false)))
}
else { Write-Output $json }
