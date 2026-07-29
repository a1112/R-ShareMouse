$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$source = Join-Path $root "drivers\windows\rshare-filter\event_queue.c"
$tests = Join-Path $root "drivers\windows\rshare-filter\tests\event_queue_tests.c"

foreach ($required in @($source, $tests)) {
    if (-not (Test-Path $required)) {
        throw "Missing semantic queue source: $required"
    }
}

function Find-VisualStudioInstall {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $install = & $vswhere -latest -products * `
            -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
            -property installationPath
        if ($install -and (Test-Path $install)) {
            return (Resolve-Path $install).Path
        }
    }
    return $null
}

$visualStudio = Find-VisualStudioInstall
if (-not $visualStudio) {
    throw "Visual Studio C++ build tools were not found."
}
$developerShell = Join-Path $visualStudio "Common7\Tools\VsDevCmd.bat"
if (-not (Test-Path $developerShell)) {
    throw "Visual Studio developer shell was not found at $developerShell."
}

$temporaryRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$outputRoot = Join-Path $temporaryRoot (
    "rshare-filter-queue-tests-{0}-{1}" -f $PID, [guid]::NewGuid().ToString("N")
)
New-Item -ItemType Directory -Path $outputRoot | Out-Null
$outputRoot = [System.IO.Path]::GetFullPath($outputRoot)
$executable = Join-Path $outputRoot "event_queue_tests.exe"

try {
    $compile = "`"$developerShell`" -arch=x64 -host_arch=x64 >nul && " +
        "cl.exe /nologo /std:c11 /W4 /WX /DRSHARE_EVENT_QUEUE_PORTABLE_TEST " +
        "/DRSHARE_EVENT_QUEUE_CAPACITY=4 " +
        "`"$source`" `"$tests`" /Fo`"$outputRoot\\`" /Fe`"$executable`""
    & $env:ComSpec /d /s /c $compile
    if ($LASTEXITCODE -ne 0) {
        throw "Semantic queue C compilation failed with exit code $LASTEXITCODE."
    }

    & $executable
    if ($LASTEXITCODE -ne 0) {
        throw "Semantic queue tests failed with exit code $LASTEXITCODE."
    }
} finally {
    $ownedPrefix = Join-Path $temporaryRoot "rshare-filter-queue-tests-"
    $isOwnedDirectory = $outputRoot.StartsWith(
        $ownedPrefix,
        [System.StringComparison]::OrdinalIgnoreCase
    )
    if ($isOwnedDirectory -and (Test-Path -LiteralPath $outputRoot)) {
        Remove-Item -LiteralPath $outputRoot -Recurse -Force
    }
}
