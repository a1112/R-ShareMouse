param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Debug",
    [ValidateSet("x64")]
    [string]$Platform = "x64"
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$logDir = Join-Path $root "target\driver-validation"
New-Item -ItemType Directory -Force -Path $logDir | Out-Null

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$logPath = Join-Path $logDir "vdisplay-preflight-$timestamp.log"
$preflightScript = Join-Path $PSScriptRoot "preflight-vdisplay.ps1"

$preflightArgs = @(
    "-NoExit",
    "-ExecutionPolicy", "Bypass",
    "-Command",
    @"
`$ErrorActionPreference = 'Stop'
Set-Location '$root'
Start-Transcript -Path '$logPath' -Force
try {
    & '$preflightScript' -Configuration '$Configuration' -Platform '$Platform' -Strict
} finally {
    Stop-Transcript
    Write-Host ''
    Write-Host 'Preflight transcript: $logPath'
}
"@
)

Write-Host "Starting elevated virtual display preflight."
Write-Host "A UAC prompt is expected. Transcript will be written to: $logPath"
Start-Process -FilePath "powershell.exe" -ArgumentList $preflightArgs -Verb RunAs
