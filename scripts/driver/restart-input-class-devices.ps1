param(
    [switch]$ConfirmRestart
)

$ErrorActionPreference = "Stop"

function Assert-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "Restarting keyboard and mouse device stacks requires an elevated PowerShell."
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

$KeyboardClassGuid = "{4D36E96B-E325-11CE-BFC1-08002BE10318}"
$MouseClassGuid = "{4D36E96F-E325-11CE-BFC1-08002BE10318}"

Assert-Admin

if (-not $ConfirmRestart) {
    throw "This script restarts keyboard and mouse device stacks. Re-run with -ConfirmRestart to proceed."
}

$pnpUtil = Find-SystemTool "pnputil.exe"
$devices = Get-CimInstance Win32_PnPEntity |
    Where-Object { $_.PNPDeviceID -and ($_.ClassGuid -ieq $KeyboardClassGuid -or $_.ClassGuid -ieq $MouseClassGuid) } |
    Sort-Object ClassGuid, PNPDeviceID

if (-not $devices) {
    throw "No keyboard or mouse device stacks were found to restart."
}

Write-Warning "Restarting keyboard and mouse device stacks can briefly interrupt local input."
foreach ($device in $devices) {
    Write-Host "Restarting $($device.ClassGuid) $($device.PNPDeviceID)"
    $output = & $pnpUtil /restart-device "$($device.PNPDeviceID)" 2>&1
    $exitCode = $LASTEXITCODE
    $output | ForEach-Object { Write-Host $_ }
    if ($exitCode -ne 0 -and $exitCode -ne 3010) {
        throw "pnputil.exe /restart-device failed for $($device.PNPDeviceID) with exit code $exitCode."
    }
}

Write-Host "Keyboard and mouse device stacks restarted."
