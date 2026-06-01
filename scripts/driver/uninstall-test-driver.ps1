param(
    [switch]$Force
)

$ErrorActionPreference = "Stop"

function Assert-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "Driver uninstall requires an elevated PowerShell."
    }
}

Assert-Admin

function Remove-RShareClassUpperFilter([string]$ClassGuid) {
    $classPath = "HKLM:\SYSTEM\CurrentControlSet\Control\Class\$ClassGuid"
    if (-not (Test-Path $classPath)) {
        return
    }

    $existingValue = (Get-ItemProperty -Path $classPath -Name UpperFilters -ErrorAction SilentlyContinue).UpperFilters
    if (-not $existingValue) {
        return
    }

    $updated = @($existingValue) | Where-Object { $_ -and $_ -ne "rshare-filter" }
    if ($updated.Count -eq 0) {
        Remove-ItemProperty -Path $classPath -Name UpperFilters -ErrorAction SilentlyContinue
    } else {
        New-ItemProperty -Path $classPath -Name UpperFilters -PropertyType MultiString -Value $updated -Force | Out-Null
    }
}

Remove-RShareClassUpperFilter "{4D36E96B-E325-11CE-BFC1-08002BE10318}"
Remove-RShareClassUpperFilter "{4D36E96F-E325-11CE-BFC1-08002BE10318}"

$drivers = pnputil /enum-drivers
$targets = $drivers | Select-String -Pattern "Published Name|Original Name|Provider Name|Class Name" -Context 0,3 |
    Where-Object { $_.Context.PostContext -match "rshare-filter.inf|rshare-vhid.inf|rshare-vdisplay.inf|R-ShareMouse" }

if (-not $targets -and -not $Force) {
    Write-Host "No RShare driver packages found."
    return
}

$publishedNames = foreach ($line in $drivers) {
    if ($line -match "Published Name\s*:\s*(oem\d+\.inf)") {
        $current = $Matches[1]
    }
    if ($line -match "Original Name\s*:\s*(rshare-filter\.inf|rshare-vhid\.inf|rshare-vdisplay\.inf)") {
        $current
    }
}

foreach ($name in ($publishedNames | Sort-Object -Unique)) {
    Write-Host "Removing $name"
    pnputil /delete-driver $name /uninstall /force
}

Write-Host "RShare test drivers removed. Reboot to guarantee keyboard/mouse stack cleanup."
