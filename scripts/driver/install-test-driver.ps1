param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Debug",
    [ValidateSet("x64")]
    [string]$Platform = "x64",
    [switch]$IncludeFilter,
    [switch]$FilterOnly,
    [switch]$HidOnly,
    [switch]$EnableInputClassFilters,
    [switch]$SkipSign,
    [switch]$EnableTestSigning
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

function Get-SecureBootEnabled {
    try {
        return [bool](Confirm-SecureBootUEFI -ErrorAction Stop)
    } catch {
        Write-Verbose "Secure Boot state could not be queried: $($_.Exception.Message)"
        return $false
    }
}

function Test-DevicePresent([string]$PnpUtil, [string]$HardwareId) {
    if (-not $HardwareId) {
        return $false
    }

    $output = & $PnpUtil /enum-devices /deviceid $HardwareId
    return [bool]($output | Select-String -Pattern "Instance ID:")
}

$KeyboardClassGuid = "{4D36E96B-E325-11CE-BFC1-08002BE10318}"
$MouseClassGuid = "{4D36E96F-E325-11CE-BFC1-08002BE10318}"

function Add-RShareClassUpperFilter([string]$ClassGuid) {
    $classPath = "HKLM:\SYSTEM\CurrentControlSet\Control\Class\$ClassGuid"
    if (-not (Test-Path $classPath)) {
        throw "Device class registry key not found: $classPath"
    }

    $existingValue = (Get-ItemProperty -Path $classPath -Name UpperFilters -ErrorAction SilentlyContinue).UpperFilters
    $existing = @()
    if ($existingValue) {
        $existing = @($existingValue) | Where-Object { $_ -and $_ -ne "rshare-filter" }
    }

    $updated = @($existing + "rshare-filter")
    New-ItemProperty -Path $classPath -Name UpperFilters -PropertyType MultiString -Value $updated -Force | Out-Null
}

function Ensure-RShareClassFilterService([string]$DriverPath) {
    if (-not (Test-Path $DriverPath)) {
        throw "Missing class filter driver binary: $DriverPath. Run scripts\driver\build.ps1 first."
    }

    $targetPath = Join-Path $env:SystemRoot "System32\drivers\rshare-filter.sys"
    Copy-Item -LiteralPath $DriverPath -Destination $targetPath -Force

    $sc = Find-SystemTool "sc.exe"
    & $sc query rshare-filter *> $null
    if ($LASTEXITCODE -eq 0) {
        & $sc config rshare-filter type= kernel start= demand error= normal binPath= "\SystemRoot\System32\drivers\rshare-filter.sys"
    } else {
        & $sc create rshare-filter type= kernel start= demand error= normal binPath= "\SystemRoot\System32\drivers\rshare-filter.sys" DisplayName= "R-ShareMouse input filter driver"
    }
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to create or update the rshare-filter kernel service."
    }
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

    if (-not $FilterOnly -and -not $HidOnly) {
        $packages += [pscustomobject]@{
            Name = "rshare-vdisplay"
            Inf = Join-Path $root "drivers\windows\rshare-vdisplay\$Platform\$Configuration\rshare-vdisplay\rshare-vdisplay.inf"
            HardwareId = "ROOT\RShareVDisplay"
            UseDevCon = $true
        }
    }

    if ($IncludeFilter -or $FilterOnly -or $EnableInputClassFilters) {
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
    if (Get-SecureBootEnabled) {
        throw "Windows test signing is not enabled and Secure Boot is enabled. Disable Secure Boot in firmware settings, boot Windows again, then re-run this script with -EnableTestSigning."
    }

    if ($EnableTestSigning) {
        Write-Host "Enabling Windows test signing with bcdedit."
        & $bcdEdit /set testsigning on
        if ($LASTEXITCODE -ne 0) {
            throw "Failed to enable Windows test signing with bcdedit."
        }
        throw "Windows test signing has been enabled. Reboot Windows, then re-run this script."
    }

    throw "Windows test signing is not enabled. Re-run with -EnableTestSigning from an elevated PowerShell, reboot Windows, then re-run this script."
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
    if ($EnableInputClassFilters) {
        $signArgs.IncludeFilter = $true
    }
    if ($FilterOnly) {
        $signArgs.FilterOnly = $true
    }
    if ($HidOnly) {
        $signArgs.HidOnly = $true
    }
    & (Join-Path $PSScriptRoot "sign-test-driver.ps1") @signArgs
    if ($LASTEXITCODE -ne 0) {
        throw "Driver package signing failed."
    }
}

$pnpUtil = Find-SystemTool "pnputil.exe"
$devcon = Find-DevCon (Get-WindowsKitRoot) $Platform
if (-not $devcon -and ($packages | Where-Object { $_.UseDevCon })) {
    throw "devcon.exe is required to install root-enumerated driver packages such as rshare-vhid and rshare-vdisplay. Install the WDK tools, then re-run this script from an elevated shell."
}

foreach ($package in $packages) {
    Write-Host "Installing $($package.Name)"
    if ($package.UseDevCon) {
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

if ($EnableInputClassFilters) {
    Ensure-RShareClassFilterService (Join-Path $root "drivers\windows\rshare-filter\$Platform\$Configuration\rshare-filter.sys")
    Add-RShareClassUpperFilter $KeyboardClassGuid
    Add-RShareClassUpperFilter $MouseClassGuid
    Write-Warning "RShare keyboard/mouse class filters were enabled. Restart or reboot Windows before validating real filter capture."
}

Write-Host "RShare test drivers installed. Reboot if Windows asks for it."
