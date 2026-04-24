param(
    [string]$OutputDir = (Join-Path (Get-Location) "target\driver-logs")
)

$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

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

$wevtutil = Find-SystemTool "wevtutil.exe"
$pnpUtil = Find-SystemTool "pnputil.exe"
$driverQuery = Find-SystemTool "driverquery.exe"

& $wevtutil qe System /q:"*[System[Provider[@Name='Microsoft-Windows-Kernel-PnP']]]" /f:text /c:100 > (Join-Path $OutputDir "kernel-pnp.txt")
& $wevtutil qe System /q:"*[System[Provider[@Name='Service Control Manager']]]" /f:text /c:100 > (Join-Path $OutputDir "service-control-manager.txt")
& $pnpUtil /enum-drivers > (Join-Path $OutputDir "pnputil-enum-drivers.txt")
& $driverQuery /v > (Join-Path $OutputDir "driverquery.txt")

Write-Host "Driver logs collected at $OutputDir"
