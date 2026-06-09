param(
    [string]$TaskName = "RShareMouse-HidPostRebootValidation",
    [uint32]$HardwareCaptureTimeoutSeconds = 20,
    [switch]$SkipManualHardwareCapture
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$logDir = Join-Path $root "target\driver-validation"
New-Item -ItemType Directory -Force -Path $logDir | Out-Null

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$logPath = Join-Path $logDir "hid-validation-post-reboot-$timestamp.log"
$validateScript = Join-Path $PSScriptRoot "validate-hid.ps1"

Start-Transcript -Path $logPath -Force
try {
    Set-Location $root
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    $validateArgs = @{
        SkipBuild = $true
        SkipInstall = $true
        HardwareCaptureTimeoutSeconds = $HardwareCaptureTimeoutSeconds
    }
    if ($SkipManualHardwareCapture) {
        $validateArgs.SkipManualHardwareCapture = $true
    }
    & $validateScript @validateArgs
} finally {
    try {
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    } catch {
        Write-Warning "Failed to unregister scheduled task $TaskName`: $($_.Exception.Message)"
    }
    Stop-Transcript
}
