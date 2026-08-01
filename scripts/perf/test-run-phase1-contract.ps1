[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$runnerPath = Join-Path $PSScriptRoot 'run-phase1.ps1'
$source = Get-Content -LiteralPath $runnerPath -Raw

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

Write-Output 'Phase 1 runner contract PASS'
