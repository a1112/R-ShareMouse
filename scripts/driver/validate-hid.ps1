param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Debug",
    [ValidateSet("x64")]
    [string]$Platform = "x64",
    [switch]$SkipBuild,
    [switch]$SkipInstall,
    [switch]$EnableTestSigning,
    [switch]$EnableInputClassFilters,
    [switch]$SkipManualHardwareCapture,
    [uint32]$HardwareCaptureTimeoutSeconds = 20
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")

function Assert-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "HID validation installs and controls test drivers; run from an elevated PowerShell."
    }
}

function Invoke-Step([string]$Name, [scriptblock]$Body) {
    Write-Host ""
    Write-Host "== $Name =="
    & $Body
}

function Get-ProbePath {
    $probe = Join-Path $root "target\driver-tools\rshare-driver-probe.exe"
    if (-not (Test-Path $probe)) {
        throw "Missing rshare-driver-probe.exe at $probe. Run scripts\driver\build.ps1 first."
    }
    return $probe
}

function Invoke-Probe([string[]]$Arguments) {
    $probe = Get-ProbePath
    $output = & $probe @Arguments
    if ($LASTEXITCODE -ne 0) {
        $output | ForEach-Object { Write-Host $_ }
        throw "rshare-driver-probe.exe $($Arguments -join ' ') failed with exit code $LASTEXITCODE."
    }
    $output | ForEach-Object { Write-Host $_ }
    return $output
}

Assert-Admin
$requiresRestartBeforeHardwareCapture = $EnableInputClassFilters -and -not $SkipInstall

Invoke-Step "Check WDK environment" {
    & (Join-Path $PSScriptRoot "check-wdk.ps1")
}

if (-not $SkipBuild) {
    Invoke-Step "Build HID driver packages and probe" {
        & (Join-Path $PSScriptRoot "build.ps1") -Configuration $Configuration -Platform $Platform
    }
}

if (-not $SkipInstall) {
    Invoke-Step "Install HID test driver packages" {
        $installArgs = @{
            Configuration = $Configuration
            Platform = $Platform
            IncludeFilter = $true
            HidOnly = $true
        }
        if ($EnableTestSigning) {
            $installArgs.EnableTestSigning = $true
        }
        if ($EnableInputClassFilters) {
            $installArgs.EnableInputClassFilters = $true
        }
        & (Join-Path $PSScriptRoot "install-test-driver.ps1") @installArgs
    }
}

if ($requiresRestartBeforeHardwareCapture) {
    Write-Warning "Keyboard/mouse class filters were enabled or refreshed. Restart or reboot Windows, then re-run this script with -SkipBuild -SkipInstall to validate real hardware capture."
} elseif (-not $EnableInputClassFilters -and -not $SkipInstall) {
    Write-Warning "Real keyboard/mouse class capture requires -EnableInputClassFilters once, followed by a Windows restart or reboot."
}

Invoke-Step "Probe filter driver status" {
    Invoke-Probe @("filter", "status") | Out-Null
}

Invoke-Step "Probe virtual HID driver status" {
    Invoke-Probe @("vhid", "status") | Out-Null
}

Invoke-Step "Run virtual HID injection smoke test" {
    Invoke-Probe @("vhid", "inject-smoke") | Out-Null
}

Invoke-Step "Run filter synthetic event test" {
    Invoke-Probe @("filter", "test") | Out-Null
}

if (-not $SkipManualHardwareCapture -and $requiresRestartBeforeHardwareCapture) {
    Write-Warning "Skipping live hardware capture watch until after the required Windows restart or reboot."
} elseif (-not $SkipManualHardwareCapture) {
    Invoke-Step "Watch real keyboard/mouse capture" {
        Write-Host "Press and release a keyboard key. Move the mouse or click a mouse button."
        Write-Host "Waiting up to $HardwareCaptureTimeoutSeconds seconds for a hardware event from the filter driver..."
        Invoke-Probe @("filter", "watch", "$HardwareCaptureTimeoutSeconds") | Out-Null
    }
}

Write-Host ""
Write-Host "HID validation flow completed."
