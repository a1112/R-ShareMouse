param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Debug",
    [ValidateSet("x64")]
    [string]$Platform = "x64",
    [switch]$IncludeFilter,
    [switch]$FilterOnly,
    [switch]$SkipSign
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")

function Assert-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "Driver install requires an elevated PowerShell."
    }
}

function Find-SystemTool([string]$Name) {
    $candidates = @(
        (Join-Path $env:SystemRoot "Sysnative\$Name"),
        (Join-Path $env:SystemRoot "System32\$Name")
    )
    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path $candidate)) {
            return $candidate
        }
    }

    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    throw "$Name was not found."
}

function Get-WindowsKitRoot {
    $kitRoot = (Get-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots' -ErrorAction SilentlyContinue).KitsRoot10
    if (-not $kitRoot) {
        $kitRoot = (Get-ItemProperty -Path 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots' -ErrorAction SilentlyContinue).KitsRoot10
    }
    if (-not $kitRoot -or -not (Test-Path $kitRoot)) {
        throw "Windows Kits root was not found. Install Windows SDK + WDK."
    }
    return (Resolve-Path $kitRoot).Path
}

function Find-DevCon([string]$KitRoot, [string]$TargetPlatform) {
    $devcon = Get-ChildItem -Path (Join-Path $KitRoot "Tools") -Recurse -Filter devcon.exe -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like "*\$TargetPlatform\devcon.exe" } |
        Sort-Object FullName -Descending |
        Select-Object -First 1
    if ($devcon) {
        return $devcon.FullName
    }
    return $null
}

function Get-TestSigningEnabled([string]$BcdEdit) {
    $output = & $BcdEdit /enum
    return [bool]($output | Select-String -Pattern "testsigning\s+Yes")
}

function Test-DevicePresent([string]$PnpUtil, [string]$HardwareId) {
    if (-not $HardwareId) {
        return $false
    }

    $output = & $PnpUtil /enum-devices /deviceid $HardwareId
    return [bool]($output | Select-String -Pattern "Instance ID:")
}

function Get-DriverPackages {
    $packages = @()

    if (-not $FilterOnly) {
        $packages += [pscustomobject]@{
            Name = "rshare-vhid"
            Inf = Join-Path $root "drivers\windows\rshare-vhid\$Platform\$Configuration\rshare-vhid\rshare-vhid.inf"
            HardwareId = "ROOT\RSHAREVHID"
            UseDevCon = $true
        }
    }

    if ($IncludeFilter -or $FilterOnly) {
        $packages += [pscustomobject]@{
            Name = "rshare-filter"
            Inf = Join-Path $root "drivers\windows\rshare-filter\$Platform\$Configuration\rshare-filter\rshare-filter.inf"
            HardwareId = $null
            UseDevCon = $false
        }
    } else {
        Write-Warning "Keyboard/mouse filter driver is skipped by default. Re-run with -FilterOnly only after confirming test signing and uninstall recovery."
    }

    return $packages
}

Assert-Admin

$bcdEdit = Find-SystemTool "bcdedit.exe"
if (-not (Get-TestSigningEnabled $bcdEdit)) {
    throw "Windows test signing is not enabled. Run elevated: bcdedit /set testsigning on, reboot, then re-run this script."
}

$packages = Get-DriverPackages
foreach ($package in $packages) {
    if (-not (Test-Path $package.Inf)) {
        throw "Missing driver package INF: $($package.Inf). Run scripts\driver\build.ps1 first."
    }
}

if (-not $SkipSign) {
    $signArgs = @{
        Configuration = $Configuration
        Platform = $Platform
    }
    if ($IncludeFilter) {
        $signArgs.IncludeFilter = $true
    }
    if ($FilterOnly) {
        $signArgs.FilterOnly = $true
    }
    & (Join-Path $PSScriptRoot "sign-test-driver.ps1") @signArgs
    if ($LASTEXITCODE -ne 0) {
        throw "Driver package signing failed."
    }
}

$pnpUtil = Find-SystemTool "pnputil.exe"
$devcon = Find-DevCon (Get-WindowsKitRoot) $Platform

foreach ($package in $packages) {
    Write-Host "Installing $($package.Name)"
    if ($package.UseDevCon -and $devcon) {
        $verb = if (Test-DevicePresent $pnpUtil $package.HardwareId) { "update" } else { "install" }
        & $devcon $verb $package.Inf $package.HardwareId
        if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne 3010) {
            throw "devcon failed for $($package.Inf)"
        }
        if ($LASTEXITCODE -eq 3010) {
            Write-Warning "$($package.Name) installed; reboot is required to complete device setup."
        }
    } else {
        & $pnpUtil /add-driver $package.Inf /install
        if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne 3010) {
            throw "pnputil failed for $($package.Inf)"
        }
        if ($LASTEXITCODE -eq 3010) {
            Write-Warning "$($package.Name) installed; reboot is required to complete device setup."
        }
    }
}

Write-Host "RShare test drivers installed. Reboot if Windows asks for it."
