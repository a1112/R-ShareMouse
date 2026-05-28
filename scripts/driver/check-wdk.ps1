param(
    [ValidateSet("x64")]
    [string]$Platform = "x64",
    [switch]$Install,
    [switch]$Quiet
)

$ErrorActionPreference = "Stop"

$recommendedWdkPackage = "Microsoft.WindowsWDK.10.0.26100"

function Get-WindowsKitRoot {
    $kitRoot = (Get-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots' -ErrorAction SilentlyContinue).KitsRoot10
    if (-not $kitRoot) {
        $kitRoot = (Get-ItemProperty -Path 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots' -ErrorAction SilentlyContinue).KitsRoot10
    }
    if ($kitRoot -and (Test-Path $kitRoot)) {
        return (Resolve-Path $kitRoot).Path
    }
    return $null
}

function Find-VisualStudioInstall {
    if ($env:VSINSTALLDIR -and (Test-Path $env:VSINSTALLDIR)) {
        return (Resolve-Path $env:VSINSTALLDIR).Path
    }

    $vswhereCandidates = @(
        (Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"),
        (Join-Path $env:ProgramFiles "Microsoft Visual Studio\Installer\vswhere.exe")
    ) | Where-Object { $_ -and (Test-Path $_) }

    foreach ($candidate in $vswhereCandidates) {
        $install = & $candidate -latest -products * -requires Microsoft.Component.MSBuild -property installationPath
        if ($install -and (Test-Path $install)) {
            return (Resolve-Path $install).Path
        }
    }

    return $null
}

function Find-FirstFile([string]$Root, [string]$Filter) {
    if (-not $Root -or -not (Test-Path $Root)) {
        return $null
    }
    return Get-ChildItem -Path $Root -Recurse -Filter $Filter -ErrorAction SilentlyContinue |
        Select-Object -First 1
}

function Find-PlatformFile([string]$Root, [string]$Filter, [string]$TargetPlatform) {
    if (-not $Root -or -not (Test-Path $Root)) {
        return $null
    }
    return Get-ChildItem -Path $Root -Recurse -Filter $Filter -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like "*\$TargetPlatform\*" } |
        Sort-Object FullName -Descending |
        Select-Object -First 1
}

function Get-WdkInstallCommand {
    "winget install --id $recommendedWdkPackage --exact --source winget"
}

if ($Install) {
    $winget = Get-Command winget.exe -ErrorAction SilentlyContinue
    if (-not $winget) {
        throw "winget.exe was not found. Install the Windows Driver Kit manually, then re-run this script."
    }

    & $winget.Source install --id $recommendedWdkPackage --exact --source winget --accept-package-agreements --accept-source-agreements
    if ($LASTEXITCODE -ne 0) {
        throw "winget install failed for $recommendedWdkPackage"
    }
}

$kitRoot = Get-WindowsKitRoot
$vsInstall = Find-VisualStudioInstall
$missing = New-Object System.Collections.Generic.List[string]

if (-not $kitRoot) {
    $missing.Add("Windows Kits 10 root")
}

$includeRoot = $null
$libRoot = $null
if ($kitRoot) {
    $includeRoot = Join-Path $kitRoot "Include"
    $libRoot = Join-Path $kitRoot "Lib"
}

$ntddk = Find-FirstFile $includeRoot "ntddk.h"
$iddcxHeader = Find-FirstFile $includeRoot "iddcx.h"
$wdfLib = Find-PlatformFile $libRoot "WdfDriverEntry.lib" $Platform
$iddcxLib = Find-PlatformFile $libRoot "IddCxStub.lib" $Platform

if (-not $ntddk) { $missing.Add("ntddk.h") }
if (-not $iddcxHeader) { $missing.Add("iddcx.h") }
if (-not $wdfLib) { $missing.Add("WdfDriverEntry.lib") }
if (-not $iddcxLib) { $missing.Add("IddCxStub.lib") }

$driverToolset = $null
if ($vsInstall) {
    $driverToolset = Join-Path $vsInstall "MSBuild\Microsoft\VC\v170\Platforms\$Platform\PlatformToolsets\WindowsUserModeDriver10.0"
}
if (-not $driverToolset -or -not (Test-Path $driverToolset)) {
    $missing.Add("WindowsUserModeDriver10.0 MSBuild platform toolset")
}

if ($missing.Count -gt 0) {
    $kitRootLabel = if ($kitRoot) { $kitRoot } else { "<not found>" }
    $vsInstallLabel = if ($vsInstall) { $vsInstall } else { "<not found>" }
    $details = @(
        "WDK/IddCx environment is incomplete.",
        "Missing: $($missing -join ', ')",
        "Windows Kits root: $kitRootLabel",
        "Visual Studio install: $vsInstallLabel",
        "Recommended install command: $(Get-WdkInstallCommand)",
        "After installation, reopen Developer PowerShell or the Codex shell and re-run scripts\driver\build.ps1."
    )
    throw ($details -join [Environment]::NewLine)
}

if (-not $Quiet) {
    Write-Host "WDK/IddCx environment OK"
    Write-Host "Windows Kits root: $kitRoot"
    Write-Host "ntddk.h: $($ntddk.FullName)"
    Write-Host "iddcx.h: $($iddcxHeader.FullName)"
    Write-Host "WdfDriverEntry.lib: $($wdfLib.FullName)"
    Write-Host "IddCxStub.lib: $($iddcxLib.FullName)"
    Write-Host "WindowsUserModeDriver10.0: $driverToolset"
}
