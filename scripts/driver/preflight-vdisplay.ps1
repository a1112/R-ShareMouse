param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Debug",
    [ValidateSet("x64")]
    [string]$Platform = "x64",
    [switch]$Strict
)

$ErrorActionPreference = "Continue"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$blockingCount = 0

function Test-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
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

    return $null
}

function Write-Check([string]$Name, [string]$Status, [string]$Detail, [bool]$Blocking = $false) {
    if ($Blocking) {
        $script:blockingCount += 1
    }
    Write-Host "[$Status] $Name - $Detail"
}

function Test-Wdk {
    $output = & (Join-Path $PSScriptRoot "check-wdk.ps1") -Platform $Platform -Quiet 2>&1
    if ($LASTEXITCODE -eq 0) {
        Write-Check "WDK/IddCx" "PASS" "driver build prerequisites are present"
    } else {
        Write-Check "WDK/IddCx" "BLOCKED" (($output | Select-Object -First 1) -join " ") $true
    }
}

function Test-TestSigning {
    $bcdEdit = Find-SystemTool "bcdedit.exe"
    if (-not $bcdEdit) {
        Write-Check "testsigning" "BLOCKED" "bcdedit.exe was not found" $true
        return
    }

    $output = & $bcdEdit /enum 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Check "testsigning" "BLOCKED" "could not read BCD state; run from an elevated PowerShell" $true
        return
    }

    if ($output | Select-String -Pattern "testsigning\s+Yes") {
        Write-Check "testsigning" "PASS" "Windows test signing is enabled"
    } else {
        Write-Check "testsigning" "BLOCKED" "Windows test signing is disabled; run start-vdisplay-validation.ps1 -EnableTestSigning after Secure Boot is off" $true
    }
}

function Test-SecureBoot {
    try {
        $enabled = [bool](Confirm-SecureBootUEFI -ErrorAction Stop)
        if ($enabled) {
            Write-Check "Secure Boot" "BLOCKED" "Secure Boot is enabled and blocks bcdedit /set testsigning on" $true
        } else {
            Write-Check "Secure Boot" "PASS" "Secure Boot is disabled"
        }
    } catch {
        $status = if (Test-Admin) { "WARN" } else { "WARN" }
        Write-Check "Secure Boot" $status "could not query Secure Boot state: $($_.Exception.Message)"
    }
}

function Test-DriverPackage {
    $inf = Join-Path $root "drivers\windows\rshare-vdisplay\$Platform\$Configuration\rshare-vdisplay\rshare-vdisplay.inf"
    $dll = Join-Path $root "drivers\windows\rshare-vdisplay\$Platform\$Configuration\rshare-vdisplay.dll"
    if ((Test-Path $inf) -and (Test-Path $dll)) {
        Write-Check "driver package" "PASS" "rshare-vdisplay package exists for $Platform $Configuration"
    } else {
        Write-Check "driver package" "BLOCKED" "build the driver first with scripts\driver\build.ps1 -Configuration $Configuration -Platform $Platform" $true
    }
}

function Test-DriverInterface {
    $probe = Join-Path $root "target\driver-tools\rshare-driver-probe.exe"
    if (-not (Test-Path $probe)) {
        Write-Check "vdisplay status" "BLOCKED" "rshare-driver-probe.exe is missing; run scripts\driver\build.ps1 first" $true
        return
    }

    $output = & $probe vdisplay status 2>&1
    if ($LASTEXITCODE -eq 0) {
        Write-Check "vdisplay status" "PASS" (($output | Select-Object -First 1) -join " ")
    } else {
        Write-Check "vdisplay status" "BLOCKED" "RShare virtual display driver interface is not available yet" $true
    }
}

Write-Host "R-ShareMouse virtual display preflight"
Write-Host "root: $root"
Write-Host ""

Write-Check "elevated shell" "$(if (Test-Admin) { 'PASS' } else { 'WARN' })" "$(if (Test-Admin) { 'running as administrator' } else { 'not elevated; install and exact boot checks need administrator' })"
Test-Wdk
Test-DriverPackage
Test-SecureBoot
Test-TestSigning
Test-DriverInterface

Write-Host ""
if ($blockingCount -eq 0) {
    Write-Host "Virtual display preflight passed. Run scripts\driver\validate-vdisplay.ps1 -VerifyDaemonDisplayTopology -WaitForManualModeChange from an elevated PowerShell."
    exit 0
}

Write-Host "Virtual display preflight found $blockingCount blocking item(s)."
if ($Strict) {
    exit 1
}
exit 0
