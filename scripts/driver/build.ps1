param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Debug",
    [ValidateSet("x64")]
    [string]$Platform = "x64"
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$projects = @(
    (Join-Path $root "drivers\windows\rshare-filter\rshare-filter.vcxproj"),
    (Join-Path $root "drivers\windows\rshare-vhid\rshare-vhid.vcxproj")
)

function Find-VisualStudioInstall {
    if ($env:VSINSTALLDIR -and (Test-Path $env:VSINSTALLDIR)) {
        return (Resolve-Path $env:VSINSTALLDIR).Path
    }

    $vswhereCandidates = @(
        (Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"),
        (Join-Path $env:ProgramFiles "Microsoft Visual Studio\Installer\vswhere.exe")
    ) | Where-Object { $_ -and (Test-Path $_) }
    foreach ($candidate in $vswhereCandidates) {
        $install = & $candidate -latest -products * -requires Microsoft.Component.MSBuild -property installationPath
        if ($install -and (Test-Path $install)) {
            return (Resolve-Path $install).Path
        }
    }

    $knownRoots = @(
        "E:\Visual Studio\2022\Community",
        "E:\Visual Studio\2022\Professional",
        "E:\Visual Studio\2022\Enterprise",
        "E:\Visual Studio\2022\BuildTools",
        "${env:ProgramFiles}\Microsoft Visual Studio\2022\Community",
        "${env:ProgramFiles}\Microsoft Visual Studio\2022\Professional",
        "${env:ProgramFiles}\Microsoft Visual Studio\2022\Enterprise",
        "${env:ProgramFiles}\Microsoft Visual Studio\2022\BuildTools"
    )
    foreach ($rootCandidate in $knownRoots) {
        if ($rootCandidate -and (Test-Path $rootCandidate)) {
            return (Resolve-Path $rootCandidate).Path
        }
    }

    return $null
}

function Find-VcToolsInstall([string]$VsInstall) {
    if ($env:VCToolsInstallDir -and (Test-Path $env:VCToolsInstallDir)) {
        return (Resolve-Path $env:VCToolsInstallDir).Path
    }
    if (-not $VsInstall) {
        return $null
    }

    $versionFile = Join-Path $VsInstall "VC\Auxiliary\Build\Microsoft.VCToolsVersion.default.txt"
    if (Test-Path $versionFile) {
        $version = (Get-Content $versionFile | Select-Object -First 1).Trim()
        $vcTools = Join-Path $VsInstall "VC\Tools\MSVC\$version"
        if (Test-Path $vcTools) {
            return (Resolve-Path $vcTools).Path
        }
    }

    $toolsRoot = Join-Path $VsInstall "VC\Tools\MSVC"
    if (Test-Path $toolsRoot) {
        $latest = Get-ChildItem -Path $toolsRoot -Directory | Sort-Object Name -Descending | Select-Object -First 1
        if ($latest) {
            return $latest.FullName
        }
    }

    return $null
}

$vsInstall = Find-VisualStudioInstall
$msbuildCommand = Get-Command msbuild.exe -ErrorAction SilentlyContinue
$msbuildPath = if ($msbuildCommand) { $msbuildCommand.Source } else { $null }
if (-not $msbuildPath -and $vsInstall) {
    $msbuildCandidates = @(
        (Join-Path $vsInstall "MSBuild\Current\Bin\amd64\MSBuild.exe"),
        (Join-Path $vsInstall "MSBuild\Current\Bin\MSBuild.exe")
    )
    $msbuildPath = $msbuildCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $msbuildPath) {
    throw "msbuild.exe was not found. Install Visual Studio Build Tools/Community with MSBuild and WDK integration."
}

$kitRoot = (Get-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots' -ErrorAction SilentlyContinue).KitsRoot10
if (-not $kitRoot) {
    $kitRoot = (Get-ItemProperty -Path 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots' -ErrorAction SilentlyContinue).KitsRoot10
}
if (-not $kitRoot -or -not (Test-Path $kitRoot)) {
    throw "Windows Kits root was not found. Install the Windows SDK and WDK, then reopen Developer PowerShell."
}

$ntddk = Get-ChildItem -Path (Join-Path $kitRoot "Include") -Recurse -Filter ntddk.h -ErrorAction SilentlyContinue | Select-Object -First 1
$wdfLib = Get-ChildItem -Path (Join-Path $kitRoot "Lib") -Recurse -Filter WdfDriverEntry.lib -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $ntddk -or -not $wdfLib) {
    throw "WDK headers/libs are incomplete under $kitRoot. Missing ntddk.h or WdfDriverEntry.lib. Install the Windows Driver Kit driver headers/libraries for x64."
}

foreach ($project in $projects) {
    Write-Host "Building $project"
    & $msbuildPath $project /p:Configuration=$Configuration /p:Platform=$Platform /p:SpectreMitigation=false /p:Driver_SpectreMitigation=false /p:SignMode=Off /m
    if ($LASTEXITCODE -ne 0) {
        throw "Driver build failed: $project"
    }
}

$cl = Get-Command cl.exe -ErrorAction SilentlyContinue
if ($cl -or $vsInstall) {
    $clPath = $cl.Source
    $outDir = Join-Path $root "target\driver-tools"
    New-Item -ItemType Directory -Force -Path $outDir | Out-Null
    $probe = Join-Path $root "drivers\windows\tools\rshare-driver-probe.c"
    $includeDir = Join-Path $root "drivers\windows\rshare-common"
    $probeOut = Join-Path $outDir "rshare-driver-probe.exe"
    $probeObj = Join-Path $outDir "rshare-driver-probe.obj"

    $vcTools = Find-VcToolsInstall $vsInstall
    if ($vcTools) {
        $platformCl = Join-Path $vcTools "bin\HostX64\$Platform\cl.exe"
        if (Test-Path $platformCl) {
            $clPath = $platformCl
        }
    }
    if (-not $clPath) {
        Write-Warning "cl.exe was not found; skipped rshare-driver-probe build."
        return
    }

    $sdkIncludeVersion = Get-ChildItem -Path (Join-Path $kitRoot "Include") -Directory |
        Where-Object { Test-Path (Join-Path $_.FullName "um\windows.h") } |
        Sort-Object Name -Descending |
        Select-Object -First 1
    $sdkLibVersion = Get-ChildItem -Path (Join-Path $kitRoot "Lib") -Directory |
        Where-Object { Test-Path (Join-Path $_.FullName "um\$Platform\kernel32.lib") } |
        Sort-Object Name -Descending |
        Select-Object -First 1
    if (-not $sdkIncludeVersion -or -not $sdkLibVersion) {
        throw "Windows SDK user-mode headers/libs are incomplete under $kitRoot. Missing windows.h or kernel32.lib."
    }

    $vcInclude = if ($vcTools) { Join-Path $vcTools "include" } else { $null }
    $vcLib = if ($vcTools) { Join-Path $vcTools "lib\$Platform" } else { $null }
    $sdkSharedInclude = Join-Path $sdkIncludeVersion.FullName "shared"
    $sdkUmInclude = Join-Path $sdkIncludeVersion.FullName "um"
    $sdkUcrtInclude = Join-Path $sdkIncludeVersion.FullName "ucrt"
    $sdkUmLib = Join-Path $sdkLibVersion.FullName "um\$Platform"
    $sdkUcrtLib = Join-Path $sdkLibVersion.FullName "ucrt\$Platform"

    $includeArgs = @("/I$includeDir", "/I$sdkSharedInclude", "/I$sdkUmInclude", "/I$sdkUcrtInclude")
    if ($vcInclude -and (Test-Path $vcInclude)) {
        $includeArgs = @("/I$vcInclude") + $includeArgs
    }
    $linkArgs = @("/link", "/LIBPATH:$sdkUmLib", "/LIBPATH:$sdkUcrtLib")
    if ($vcLib -and (Test-Path $vcLib)) {
        $linkArgs = @("/link", "/LIBPATH:$vcLib", "/LIBPATH:$sdkUmLib", "/LIBPATH:$sdkUcrtLib")
    }

    & $clPath /nologo /W4 /WX $includeArgs $probe "/Fo$probeObj" "/Fe$probeOut" $linkArgs
    if ($LASTEXITCODE -ne 0) {
        throw "Driver probe build failed"
    }
} else {
    Write-Warning "cl.exe was not found; skipped rshare-driver-probe build."
}
