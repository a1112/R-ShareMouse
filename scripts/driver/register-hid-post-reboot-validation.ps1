param(
    [string]$TaskName = "RShareMouse-HidPostRebootValidation",
    [uint32]$HardwareCaptureTimeoutSeconds = 20,
    [switch]$SkipManualHardwareCapture
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")

function Assert-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "Registering the post-reboot HID validation task requires an elevated PowerShell."
    }
}

Assert-Admin

$runner = Join-Path $PSScriptRoot "run-hid-post-reboot-validation.ps1"
if (-not (Test-Path $runner)) {
    throw "Missing post-reboot validation runner: $runner"
}

$arguments = @(
    "-NoProfile",
    "-ExecutionPolicy", "Bypass",
    "-File", "`"$runner`"",
    "-TaskName", "`"$TaskName`"",
    "-HardwareCaptureTimeoutSeconds", "$HardwareCaptureTimeoutSeconds"
)
if ($SkipManualHardwareCapture) {
    $arguments += "-SkipManualHardwareCapture"
}

$action = New-ScheduledTaskAction -Execute "powershell.exe" -Argument ($arguments -join " ")
$trigger = New-ScheduledTaskTrigger -AtLogOn
$principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -Compatibility Win8 -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 10)

Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null

Write-Host "Registered one-shot HID post-reboot validation task: $TaskName"
Write-Host "Task action root: $root"
Write-Host "Reboot Windows, log in as $env:USERDOMAIN\$env:USERNAME, then inspect target\driver-validation for the transcript."
