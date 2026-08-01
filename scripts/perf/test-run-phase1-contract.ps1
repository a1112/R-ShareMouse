[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$runnerPath = Join-Path $PSScriptRoot 'run-phase1.ps1'
$source = Get-Content -LiteralPath $runnerPath -Raw
$uiScenarioPath = Join-Path $PSScriptRoot '..\..\apps\rshare-desktop-frontend\tests\performance\ui-state.spec.mjs'
$uiScenarioSource = Get-Content -LiteralPath $uiScenarioPath -Raw

function Assert-ContainsLiteral {
    param(
        [Parameter(Mandatory = $true)][string]$Needle,
        [Parameter(Mandatory = $true)][string]$FailureMessage
    )

    if (-not $source.Contains($Needle)) {
        throw $FailureMessage
    }
}

Assert-ContainsLiteral `
    -Needle '$IpcRequestsPerConnection = 5000' `
    -FailureMessage 'Phase 1 IPC must measure 5000 requests per connection so p99 has enough tail samples.'
Assert-ContainsLiteral `
    -Needle "'ipc', '--requests', `$IpcRequestsPerConnection, '--concurrency', '1,8'" `
    -FailureMessage 'The IPC command must use the shared request-count contract.'
Assert-ContainsLiteral `
    -Needle 'requests = $IpcRequestsPerConnection' `
    -FailureMessage 'The candidate scenario parameters must record the measured IPC request count.'

$variationStart = $source.IndexOf('function Get-IpcBatchVariation')
$variationEnd = $source.IndexOf('function Get-UiBatchVariation')
if ($variationStart -lt 0 -or $variationEnd -le $variationStart) {
    throw 'Could not locate the IPC variation function.'
}
$variationSource = $source.Substring($variationStart, $variationEnd - $variationStart)
if (-not $source.Contains("`$IpcComparativeMetrics = @('median_us', 'p95_us', 'p99_us')") -or
    -not $variationSource.Contains('foreach ($metric in $IpcComparativeMetrics)')) {
    throw 'IPC CV must cover the three baseline-comparative latency metrics.'
}
if ($variationSource.Contains("'max_us'")) {
    throw 'IPC max latency is catastrophe evidence, not a baseline-comparative CV metric.'
}

Assert-ContainsLiteral `
    -Needle "`$UiComparativeMetrics = @(" `
    -FailureMessage 'Phase 1 must declare UI comparative metrics separately from raw evidence.'
Assert-ContainsLiteral `
    -Needle 'foreach ($metric in $UiComparativeMetrics)' `
    -FailureMessage 'UI CV must use only the declared baseline-comparative metrics.'
if (-not $uiScenarioSource.Contains('warmupDurationMs: 2_000')) {
    throw 'The fixed-runner UI scenario must warm each fresh browser before measurement.'
}
if (-not $uiScenarioSource.Contains('durationMs: 30_000') -or
    -not $uiScenarioSource.Contains('discreteTransitions: 300') -or
    -not $uiScenarioSource.Contains('topologyStatusTransitions: 300')) {
    throw 'The fixed-runner UI scenario must provide a 30-second, 300-transition tail sample.'
}

Write-Output 'Phase 1 runner contract PASS'
