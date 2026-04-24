param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Debug",
    [ValidateSet("x64")]
    [string]$Platform = "x64",
    [switch]$IncludeFilter,
    [switch]$FilterOnly
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$certSubject = "CN=R-ShareMouse Test Driver Signing"

function Assert-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "Driver package signing requires an elevated PowerShell."
    }
}

function Get-WindowsKitRoot {
    $kitRoot = (Get-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots' -ErrorAction SilentlyContinue).KitsRoot10
    if (-not $kitRoot) {
        $kitRoot = (Get-ItemProperty -Path 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots' -ErrorAction SilentlyContinue).KitsRoot10
    }
    if (-not $kitRoot -or -not (Test-Path $kitRoot)) {
        throw "Windows Kits root was not found. Install Windows SDK + WDK."
    }
    return (Resolve-Path $kitRoot).Path
}

function Find-SignTool([string]$KitRoot, [string]$TargetPlatform) {
    $signTool = Get-ChildItem -Path (Join-Path $KitRoot "bin") -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like "*\$TargetPlatform\signtool.exe" } |
        Sort-Object FullName -Descending |
        Select-Object -First 1
    if (-not $signTool) {
        throw "signtool.exe was not found under $KitRoot."
    }
    return $signTool.FullName
}

function Get-DriverPackages {
    $packages = @()

    if (-not $FilterOnly) {
        $packages += [pscustomobject]@{
            Name = "rshare-vhid"
            Directory = Join-Path $root "drivers\windows\rshare-vhid\$Platform\$Configuration\rshare-vhid"
            Catalog = "rshare-vhid.cat"
        }
    }

    if ($IncludeFilter -or $FilterOnly) {
        $packages += [pscustomobject]@{
            Name = "rshare-filter"
            Directory = Join-Path $root "drivers\windows\rshare-filter\$Platform\$Configuration\rshare-filter"
            Catalog = "rshare-filter.cat"
        }
    }

    return $packages
}

function Ensure-TestCertificate {
    $cert = Get-ChildItem Cert:\LocalMachine\My |
        Where-Object { $_.Subject -eq $certSubject -and $_.NotAfter -gt (Get-Date).AddDays(30) } |
        Sort-Object NotAfter -Descending |
        Select-Object -First 1

    if (-not $cert) {
        $cert = New-SelfSignedCertificate `
            -Type CodeSigningCert `
            -Subject $certSubject `
            -CertStoreLocation Cert:\LocalMachine\My `
            -KeyAlgorithm RSA `
            -KeyLength 2048 `
            -HashAlgorithm SHA256 `
            -KeyUsage DigitalSignature `
            -NotAfter (Get-Date).AddYears(3)
    }

    $certOutDir = Join-Path $root "target\driver-test-cert"
    New-Item -ItemType Directory -Force -Path $certOutDir | Out-Null
    $certPath = Join-Path $certOutDir "rshare-test-driver.cer"
    Export-Certificate -Cert $cert -FilePath $certPath -Force | Out-Null

    foreach ($store in @("Cert:\LocalMachine\Root", "Cert:\LocalMachine\TrustedPublisher")) {
        $trusted = Get-ChildItem $store | Where-Object { $_.Thumbprint -eq $cert.Thumbprint } | Select-Object -First 1
        if (-not $trusted) {
            Import-Certificate -FilePath $certPath -CertStoreLocation $store | Out-Null
        }
    }

    return $cert
}

Assert-Admin
$kitRoot = Get-WindowsKitRoot
$signTool = Find-SignTool $kitRoot $Platform
$cert = Ensure-TestCertificate

$packages = Get-DriverPackages
if (-not $packages) {
    throw "No driver packages selected for signing."
}

foreach ($package in $packages) {
    if (-not (Test-Path $package.Directory)) {
        throw "Missing driver package directory: $($package.Directory). Run scripts\driver\build.ps1 first."
    }

    $catPath = Join-Path $package.Directory $package.Catalog
    if (-not (Test-Path $catPath)) {
        throw "Missing driver catalog: $catPath. Run scripts\driver\build.ps1 first."
    }

    Write-Host "Signing $catPath"
    & $signTool sign /v /fd SHA256 /sm /s My /sha1 $cert.Thumbprint $catPath
    if ($LASTEXITCODE -ne 0) {
        throw "signtool sign failed for $catPath"
    }

    & $signTool verify /v /pa $catPath
    if ($LASTEXITCODE -ne 0) {
        throw "signtool verify failed for $catPath"
    }
}

Write-Host "RShare test driver packages signed with certificate $($cert.Thumbprint)."
