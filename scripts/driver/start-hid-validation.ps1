param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Debug",
    [ValidateSet("x64")]
    [string]$Platform = "x64",
    [switch]$EnableTestSigning,
    [switch]$EnableInputClassFilters,
    [switch]$SkipManualHardwareCapture,
    [uint32]$HardwareCaptureTimeoutSeconds = 20
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$logDir = Join-Path $root "target\driver-validation"
New-Item -ItemType Directory -Force -Path $logDir | Out-Null

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$logPath = Join-Path $logDir "hid-validation-$timestamp.log"
$validateScript = Join-Path $PSScriptRoot "validate-hid.ps1"

$validationArgs = @(
    "-NoExit",
    "-ExecutionPolicy", "Bypass",
    "-Command",
    @"
`$ErrorActionPreference = 'Stop'
Set-Location '$root'
Start-Transcript -Path '$logPath' -Force
try {
    & '$validateScript' -Configuration '$Configuration' -Platform '$Platform' -HardwareCaptureTimeoutSeconds $HardwareCaptureTimeoutSeconds $(if ($EnableTestSigning) { '-EnableTestSigning' } else { '' }) $(if ($EnableInputClassFilters) { '-EnableInputClassFilters' } else { '' }) $(if ($SkipManualHardwareCapture) { '-SkipManualHardwareCapture' } else { '' })
} finally {
    Stop-Transcript
    Write-Host ''
    Write-Host 'Validation transcript: $logPath'
}
"@
)

Write-Host "Starting elevated HID driver validation."
Write-Host "A UAC prompt is expected. Transcript will be written to: $logPath"
Start-Process -FilePath "powershell.exe" -ArgumentList $validationArgs -Verb RunAs
