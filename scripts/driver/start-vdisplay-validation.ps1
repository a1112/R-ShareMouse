param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Debug",
    [ValidateSet("x64")]
    [string]$Platform = "x64",
    [uint32]$Width = 1920,
    [uint32]$Height = 1080,
    [uint32]$RefreshRateMillihz = 60000,
    [switch]$EnableTestSigning,
    [switch]$KeepDisplay
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$logDir = Join-Path $root "target\driver-validation"
New-Item -ItemType Directory -Force -Path $logDir | Out-Null

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$logPath = Join-Path $logDir "vdisplay-validation-$timestamp.log"
$validateScript = Join-Path $PSScriptRoot "validate-vdisplay.ps1"

$validationArgs = @(
    "-NoExit",
    "-ExecutionPolicy", "Bypass",
    "-Command",
    @"
`$ErrorActionPreference = 'Stop'
Set-Location '$root'
Start-Transcript -Path '$logPath' -Force
try {
    & '$validateScript' -Configuration '$Configuration' -Platform '$Platform' -Width $Width -Height $Height -RefreshRateMillihz $RefreshRateMillihz -VerifyDaemonDisplayTopology -WaitForManualModeChange $(if ($EnableTestSigning) { '-EnableTestSigning' } else { '' }) $(if ($KeepDisplay) { '-KeepDisplay' } else { '' })
} finally {
    Stop-Transcript
    Write-Host ''
    Write-Host 'Validation transcript: $logPath'
}
"@
)

Write-Host "Starting elevated virtual display validation."
Write-Host "A UAC prompt is expected. Transcript will be written to: $logPath"
Start-Process -FilePath "powershell.exe" -ArgumentList $validationArgs -Verb RunAs
