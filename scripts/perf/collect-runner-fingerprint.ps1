[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$OutputPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Invoke-CapturedCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,

        [Parameter(Mandatory = $true)]
        [string[]]$Arguments,

        [Parameter(Mandatory = $true)]
        [string]$WorkingDirectory
    )

    Push-Location $WorkingDirectory
    try {
        $output = @(& $FilePath @Arguments 2>&1)
        if ($LASTEXITCODE -ne 0) {
            throw "$FilePath $($Arguments -join ' ') failed with exit code $LASTEXITCODE`n$($output -join [Environment]::NewLine)"
        }
        return (($output | ForEach-Object { $_.ToString() }) -join [Environment]::NewLine).Trim()
    }
    finally {
        Pop-Location
    }
}

function Get-Sha256File {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-Sha256Text {
    param([Parameter(Mandatory = $true)][string]$Text)

    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [Text.Encoding]::UTF8.GetBytes($Text)
        return ([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant()
    }
    finally {
        $sha.Dispose()
    }
}

function Get-PhysicalMemoryBytes {
    if ($null -eq ('RSharePerf.NativeMemory' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

namespace RSharePerf
{
    [StructLayout(LayoutKind.Sequential)]
    public sealed class MemoryStatusEx
    {
        public UInt32 Length = (UInt32)Marshal.SizeOf(typeof(MemoryStatusEx));
        public UInt32 MemoryLoad;
        public UInt64 TotalPhysical;
        public UInt64 AvailablePhysical;
        public UInt64 TotalPageFile;
        public UInt64 AvailablePageFile;
        public UInt64 TotalVirtual;
        public UInt64 AvailableVirtual;
        public UInt64 AvailableExtendedVirtual;
    }

    public static class NativeMemory
    {
        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool GlobalMemoryStatusEx([In, Out] MemoryStatusEx status);
    }
}
'@
    }

    $status = New-Object RSharePerf.MemoryStatusEx
    if (-not [RSharePerf.NativeMemory]::GlobalMemoryStatusEx($status)) {
        throw "GlobalMemoryStatusEx failed with Win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
    }
    return [UInt64]$status.TotalPhysical
}

function Write-JsonAtomically {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)]$Value
    )

    $parent = Split-Path -Parent $Path
    if ([string]::IsNullOrWhiteSpace($parent)) {
        $parent = (Get-Location).Path
    }
    [IO.Directory]::CreateDirectory($parent) | Out-Null
    $temporary = Join-Path $parent (".{0}.{1}.tmp" -f ([IO.Path]::GetFileName($Path)), [Guid]::NewGuid().ToString('N'))
    try {
        $json = $Value | ConvertTo-Json -Depth 16
        [IO.File]::WriteAllText($temporary, $json + [Environment]::NewLine, (New-Object Text.UTF8Encoding($false)))
        Move-Item -LiteralPath $temporary -Destination $Path -Force
    }
    finally {
        if (Test-Path -LiteralPath $temporary) {
            Remove-Item -LiteralPath $temporary -Force
        }
    }
}

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$resolvedOutputPath = [IO.Path]::GetFullPath($OutputPath)

$commit = Invoke-CapturedCommand -FilePath 'git.exe' -Arguments @('rev-parse', 'HEAD') -WorkingDirectory $repositoryRoot
$porcelain = Invoke-CapturedCommand -FilePath 'git.exe' -Arguments @('status', '--porcelain') -WorkingDirectory $repositoryRoot
$rustc = Invoke-CapturedCommand -FilePath 'rustc.exe' -Arguments @('--version') -WorkingDirectory $repositoryRoot
$cargo = Invoke-CapturedCommand -FilePath 'cargo.exe' -Arguments @('--version') -WorkingDirectory $repositoryRoot
$rustcVerbose = Invoke-CapturedCommand -FilePath 'rustc.exe' -Arguments @('-vV') -WorkingDirectory $repositoryRoot
$target = @($rustcVerbose -split "`r?`n" | Where-Object { $_ -like 'host: *' } | Select-Object -First 1)
if ($target.Count -ne 1) {
    throw 'rustc -vV did not report exactly one host target'
}
$target = $target[0].Substring('host: '.Length)

$runnerId = $env:COMPUTERNAME
if ([string]::IsNullOrWhiteSpace($runnerId)) {
    $runnerId = $env:HOSTNAME
}
if ([string]::IsNullOrWhiteSpace($runnerId)) {
    throw 'COMPUTERNAME/HOSTNAME is unavailable; cannot create a stable runner identity'
}

$processorIdentifier = $env:PROCESSOR_IDENTIFIER
if ([string]::IsNullOrWhiteSpace($processorIdentifier)) {
    $processorIdentifier = 'unknown'
}

$toolchain = [ordered]@{
    rustc = $rustc
    cargo = $cargo
    target = $target
}
$hardware = [ordered]@{
    os = 'windows'
    cpu = $processorIdentifier
    logical_cores = [UInt32][Environment]::ProcessorCount
    memory_bytes = Get-PhysicalMemoryBytes
}

# Keep this JSON tuple byte-for-byte compatible with tools/rshare-perf's
# serde_json::to_vec((&runner_id, &toolchain, &hardware)) fingerprint input.
$powerPlan = Invoke-CapturedCommand -FilePath 'powercfg.exe' -Arguments @('/getactivescheme') -WorkingDirectory $repositoryRoot
if ($powerPlan -notmatch '([0-9A-Fa-f-]{36})') {
    throw "powercfg did not return an active power-plan GUID: $powerPlan"
}
$powerPlanGuid = $Matches[1].ToLowerInvariant()
$affinityMask = [UInt64][Diagnostics.Process]::GetCurrentProcess().ProcessorAffinity.ToInt64()
if ($affinityMask -eq 0) {
    throw 'the performance process has an empty CPU affinity mask'
}
$runnerSettings = [ordered]@{
    power_plan_guid = $powerPlanGuid
    process_affinity_mask = $affinityMask.ToString('x')
}
$runnerTuple = @($runnerId, $toolchain, $hardware, $runnerSettings)
$runnerFingerprintInput = $runnerTuple | ConvertTo-Json -Depth 8 -Compress
$runnerFingerprint = Get-Sha256Text -Text $runnerFingerprintInput

$metadata = Invoke-CapturedCommand -FilePath 'cargo.exe' -Arguments @('metadata', '--no-deps', '--format-version', '1') -WorkingDirectory $repositoryRoot | ConvertFrom-Json
$targetDirectory = [IO.Path]::GetFullPath([string]$metadata.target_directory)
$binaryHashes = [ordered]@{}
$knownBinaries = [ordered]@{
    'rshare-perf' = Join-Path $targetDirectory 'release\rshare-perf.exe'
    'rshare-daemon' = Join-Path $targetDirectory 'release\rshare-daemon.exe'
    'rshare-desktop' = Join-Path $targetDirectory 'release\rshare-desktop.exe'
}
foreach ($role in $knownBinaries.Keys) {
    $path = $knownBinaries[$role]
    if (Test-Path -LiteralPath $path -PathType Leaf) {
        $binaryHashes[$role] = Get-Sha256File -Path $path
    }
}
if (-not $binaryHashes.Contains('rshare-perf')) {
    throw "required rshare-perf binary is missing under Cargo target directory $targetDirectory"
}
$frontendDist = Join-Path $repositoryRoot 'apps\rshare-desktop-frontend\dist'
if (-not (Test-Path -LiteralPath $frontendDist -PathType Container)) {
    throw "frontend build output is missing: $frontendDist"
}
$frontendEntries = @(Get-ChildItem -LiteralPath $frontendDist -Recurse -File | Sort-Object FullName | ForEach-Object {
    $relative = $_.FullName.Substring($frontendDist.TrimEnd('\', '/').Length + 1).Replace('\', '/')
    "$relative`n$(Get-Sha256File -Path $_.FullName)"
})
if ($frontendEntries.Count -eq 0) {
    throw 'frontend build output contains no files'
}
$binaryHashes['rshare-desktop-frontend'] = Get-Sha256Text -Text (($frontendEntries -join "`n") + "`n")

$cargoLockPath = Join-Path $repositoryRoot 'Cargo.lock'
if (-not (Test-Path -LiteralPath $cargoLockPath -PathType Leaf)) {
    throw "Cargo.lock is missing at $cargoLockPath"
}

$fingerprint = [ordered]@{
    schema_version = 1
    captured_at_utc = [DateTime]::UtcNow.ToString('o')
    commit = $commit
    dirty = -not [string]::IsNullOrWhiteSpace($porcelain)
    binary_sha256 = $binaryHashes
    cargo_lock_sha256 = Get-Sha256File -Path $cargoLockPath
    build_profile = 'release'
    cargo_features = @()
    rustflags = [string]$env:RUSTFLAGS
    runner_id = $runnerId
    runner_fingerprint = $runnerFingerprint
    runner_fingerprint_input = $runnerTuple
    toolchain = $toolchain
    hardware = $hardware
    process_affinity_mask = $runnerSettings.process_affinity_mask
    power_plan_guid = $powerPlanGuid
    power_plan = $powerPlan
    cargo_target_directory = $targetDirectory
}

Write-JsonAtomically -Path $resolvedOutputPath -Value $fingerprint
Write-Output $resolvedOutputPath
