param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Debug",
    [ValidateSet("x64")]
    [string]$Platform = "x64",
    [uint32]$Width = 1920,
    [uint32]$Height = 1080,
    [uint32]$RefreshRateMillihz = 60000,
    [switch]$SkipBuild,
    [switch]$SkipInstall,
    [switch]$VerifyDaemonDisplayTopology,
    [switch]$WaitForManualModeChange,
    [switch]$KeepDisplay
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")

function Assert-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "Virtual display validation installs and controls a test driver; run from an elevated PowerShell."
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
        throw "rshare-driver-probe.exe $($Arguments -join ' ') failed with exit code $LASTEXITCODE."
    }
    $output | ForEach-Object { Write-Host $_ }
    return $output
}

function Read-VDisplayState {
    $output = Invoke-Probe @("vdisplay", "status")
    $line = $output | Where-Object { $_ -like "vdisplay state abi=*" } | Select-Object -Last 1
    if (-not $line) {
        throw "Probe output did not contain a vdisplay state abi= line."
    }

    if ($line -notmatch "active=(\d+)\s+(\d+)x(\d+)@(\d+)\s+connector=(\d+)") {
        throw "Probe output state line has an unexpected format: $line"
    }

    return [pscustomobject]@{
        Raw = $line
        Active = [uint32]$Matches[1]
        Width = [uint32]$Matches[2]
        Height = [uint32]$Matches[3]
        RefreshRateMillihz = [uint32]$Matches[4]
        ConnectorIndex = [uint32]$Matches[5]
    }
}

function EnsureDaemonForTopologyVerification {
    # cargo build -p rshare-daemon -p rshare-cli
    $buildArgs = @("build", "-p", "rshare-daemon", "-p", "rshare-cli")
    $buildOutput = & cargo @buildArgs
    if ($LASTEXITCODE -ne 0) {
        $buildOutput | ForEach-Object { Write-Host $_ }
        throw "Failed to build rshare-daemon and rshare-cli for topology verification."
    }
    $buildOutput | ForEach-Object { Write-Host $_ }

    # cargo run -p rshare-cli -- start --daemon
    $startArgs = @("run", "-p", "rshare-cli", "--", "start", "--daemon")
    $startOutput = & cargo @startArgs
    if ($LASTEXITCODE -ne 0) {
        $startOutput | ForEach-Object { Write-Host $_ }
        throw "Failed to start rshare-daemon for topology verification."
    }
    $startOutput | ForEach-Object { Write-Host $_ }
}

function Invoke-DaemonDisplayTopologyVerification {
    # cargo run -p rshare-cli -- display virtual verify
    $args = @(
        "run",
        "-p",
        "rshare-cli",
        "--",
        "display",
        "virtual",
        "verify",
        "--width",
        "$Width",
        "--height",
        "$Height",
        "--refresh-rate-millihz",
        "$RefreshRateMillihz"
    )
    $output = & cargo @args
    if ($LASTEXITCODE -ne 0) {
        $output | ForEach-Object { Write-Host $_ }
        throw "Daemon display topology verification failed. Ensure rshare-daemon is running, then re-run this script."
    }
    $output | ForEach-Object { Write-Host $_ }
}

Assert-Admin

Invoke-Step "Check WDK and IddCx environment" {
    & (Join-Path $PSScriptRoot "check-wdk.ps1")
}

if (-not $SkipBuild) {
    Invoke-Step "Build driver packages and probe" {
        & (Join-Path $PSScriptRoot "build.ps1") -Configuration $Configuration -Platform $Platform
    }
}

if (-not $SkipInstall) {
    Invoke-Step "Install test driver packages" {
        & (Join-Path $PSScriptRoot "install-test-driver.ps1") -Configuration $Configuration -Platform $Platform
    }
}

Invoke-Step "Probe installed virtual display driver" {
    Invoke-Probe @("vdisplay", "status") | Out-Null
}

Invoke-Step "Create virtual display" {
    Invoke-Probe @("vdisplay", "create", "$Width", "$Height", "$RefreshRateMillihz") | Out-Null
}

Invoke-Step "Confirm driver state after create" {
    $state = Read-VDisplayState
    if ($state.Active -ne 1 -or $state.Width -ne $Width -or $state.Height -ne $Height -or $state.RefreshRateMillihz -ne $RefreshRateMillihz) {
        throw "Unexpected virtual display state after create: $($state.Raw)"
    }
}

if ($VerifyDaemonDisplayTopology) {
    Invoke-Step "Ensure daemon for topology verification" {
        EnsureDaemonForTopologyVerification
    }

    Invoke-Step "Verify daemon display topology" {
        Invoke-DaemonDisplayTopologyVerification
    }
}

Invoke-Step "Open Windows display settings" {
    Start-Process "ms-settings:display"
    Write-Host "Windows Settings > System > Display should now show the R-ShareMouse virtual display."
}

if ($WaitForManualModeChange) {
    Invoke-Step "Wait for manual mode change from Windows Settings" {
        Write-Host "Change the virtual display resolution or refresh rate in Windows Settings."
        Write-Host "Waiting until rshare-driver-probe.exe reports a different mode..."
        $deadline = (Get-Date).AddMinutes(5)
        while ((Get-Date) -lt $deadline) {
            Start-Sleep -Seconds 2
            $state = Read-VDisplayState
            $sameMode = $state.Width -eq $Width -and $state.Height -eq $Height -and $state.RefreshRateMillihz -eq $RefreshRateMillihz
            if ($state.Active -eq 1 -and -not $sameMode) {
                Write-Host "Manual mode change observed through CommitModes: $($state.Raw)"
                return
            }
        }
        throw "Timed out waiting for Windows Settings to commit a different virtual display mode."
    }
}

if (-not $KeepDisplay) {
    Invoke-Step "Remove virtual display" {
        Invoke-Probe @("vdisplay", "remove") | Out-Null
    }
}

Write-Host ""
Write-Host "Virtual display validation flow completed."
