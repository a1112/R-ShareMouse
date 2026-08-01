[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$OutputDirectory,

    [ValidateSet('Strict', 'Bootstrap')]
    [string]$Mode = 'Strict',

    [ValidateRange(0, 3600)]
    [int]$WarmupSeconds = 0
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RunCount = 5
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$outputRoot = [IO.Path]::GetFullPath($OutputDirectory)
$phase1BatchId = [Guid]::NewGuid().ToString('D')
$batchDirectory = Join-Path $outputRoot $phase1BatchId
$rawDirectory = Join-Path $batchDirectory 'raw'
$candidateDirectory = Join-Path $batchDirectory 'candidates'
$logDirectory = Join-Path $batchDirectory 'logs'
$summaryPath = Join-Path $batchDirectory 'summary.json'
$fingerprintPath = Join-Path $batchDirectory 'runner-fingerprint.json'
$finalFingerprintPath = Join-Path $batchDirectory 'runner-fingerprint-final.json'
$cargoTargetRoot = if ([string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
    Join-Path $repositoryRoot 'target'
} elseif ([IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
    [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
} else {
    [IO.Path]::GetFullPath((Join-Path $repositoryRoot $env:CARGO_TARGET_DIR))
}
$perfBinaryPath = Join-Path $cargoTargetRoot 'release\rshare-perf.exe'
$originalLocation = (Get-Location).Path
$trackedEnvironment = @(
    'RSHARE_PERF_BATCH_ID',
    'RSHARE_PERF_BATCH_ATTEMPT',
    'RSHARE_PERF_RUN_INDEX',
    'RSHARE_PERF_OUTPUT',
    'RSHARE_PERF_PLAYWRIGHT_OUTPUT',
    'RSHARE_PERF_WARMUP_MS',
    'RSHARE_PERF_AFFINITY_MASK',
    'RSHARE_PERF_POWER_PLAN_GUID'
)
$savedEnvironment = @{}
foreach ($name in $trackedEnvironment) {
    $item = Get-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
    $savedEnvironment[$name] = [ordered]@{
        existed = $null -ne $item
        value = if ($null -ne $item) { $item.Value } else { $null }
    }
}

$summary = [ordered]@{
    schema_version = 1
    phase1_batch_id = $phase1BatchId
    label = 'loopback'
    mode = $Mode.ToLowerInvariant()
    status = 'RUNNING'
    run_count = $RunCount
    started_at_utc = [DateTime]::UtcNow.ToString('o')
    completed_at_utc = $null
    output_directory = $batchDirectory
    warmup = [ordered]@{ seconds = $WarmupSeconds }
    reproducibility = $null
    correctness = [ordered]@{
        command = 'cargo test --workspace --locked'
        status = 'NOT_RUN'
        log = $null
    }
    quic = @()
    phase1_thresholds = [ordered]@{}
    ipc = [ordered]@{
        status = 'NOT_RUN'
        selected_attempt = $null
        candidate = $null
        batches = @()
    }
    ui = [ordered]@{
        status = 'NOT_RUN'
        selected_attempt = $null
        candidate = $null
        batches = @()
    }
    comparisons = @()
    artifacts = @()
    errors = @()
}
$failureMessage = $null
$pendingBaseline = $false

function Write-JsonAtomically {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)]$Value
    )

    $parent = Split-Path -Parent $Path
    [IO.Directory]::CreateDirectory($parent) | Out-Null
    $temporary = Join-Path $parent (".{0}.{1}.tmp" -f ([IO.Path]::GetFileName($Path)), [Guid]::NewGuid().ToString('N'))
    try {
        $json = ($Value | ConvertTo-Json -Depth 32).Replace("`r`n", "`n").Replace("`r", "`n")
        [IO.File]::WriteAllText($temporary, $json + "`n", (New-Object Text.UTF8Encoding($false)))
        Move-Item -LiteralPath $temporary -Destination $Path -Force
    }
    finally {
        if (Test-Path -LiteralPath $temporary) {
            Remove-Item -LiteralPath $temporary -Force
        }
    }
}

function Save-Summary {
    $summary.completed_at_utc = [DateTime]::UtcNow.ToString('o')
    Write-JsonAtomically -Path $summaryPath -Value $summary
}

function Invoke-LoggedCommand {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$LogPath
    )

    Write-Host ">> $FilePath $($Arguments -join ' ')"
    # PowerShell 5 surfaces native stderr as ErrorRecord objects. Cargo writes
    # normal progress to stderr, so keep native output non-terminating and
    # decide success exclusively from the process exit code.
    $previousErrorActionPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        & $FilePath @Arguments 2>&1 | Tee-Object -FilePath $LogPath
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    if ($exitCode -ne 0) {
        throw "$FilePath failed with exit code $exitCode; see $LogPath"
    }
}

function Get-Sha256File {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-CanonicalScenarioConfigHash {
    param(
        [Parameter(Mandatory = $true)][string]$Scenario,
        [Parameter(Mandatory = $true)]$Value
    )

    if (-not (Test-Path -LiteralPath $perfBinaryPath -PathType Leaf)) {
        throw "canonical configuration hasher is missing: $perfBinaryPath"
    }
    $configPath = Join-Path $candidateDirectory "$Scenario.config.json"
    Write-JsonAtomically -Path $configPath -Value $Value
    $hash = (& $perfBinaryPath config-hash --input $configPath | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $hash -notmatch '^[0-9a-f]{64}$') {
        throw "failed to compute canonical scenario hash for $Scenario`: $hash"
    }
    return $hash
}

function New-MetricSummary {
    param(
        [Parameter(Mandatory = $true)][double[]]$Values,
        [Parameter(Mandatory = $true)][string]$Unit
    )
    if ($Values.Count -ne $RunCount) {
        throw "metric summary requires exactly $RunCount values"
    }
    $sorted = @($Values | Sort-Object)
    $mean = ($sorted | Measure-Object -Average).Average
    $variance = 0.0
    foreach ($value in $sorted) {
        $variance += [Math]::Pow($value - $mean, 2)
    }
    $variance /= ($sorted.Count - 1)
    $percentile = {
        param([double]$Fraction)
        $index = [Math]::Ceiling(($sorted.Count - 1) * $Fraction)
        return [double]$sorted[[int]$index]
    }
    return [ordered]@{
        unit = $Unit
        samples = $sorted.Count
        median = & $percentile 0.50
        p95 = & $percentile 0.95
        p99 = & $percentile 0.99
        max = [double]$sorted[-1]
        coefficient_of_variation = if ($mean -eq 0.0) { 0.0 } else { [Math]::Sqrt($variance) / [Math]::Abs($mean) }
    }
}

function New-Phase1Candidate {
    param(
        [Parameter(Mandatory = $true)][string]$Scenario,
        [Parameter(Mandatory = $true)]$ScenarioParameters,
        [Parameter(Mandatory = $true)][string]$BatchId,
        [Parameter(Mandatory = $true)][object[]]$RunMetrics,
        [object[]]$RunCounters,
        [object[]]$RunRawSamples,
        [Parameter(Mandatory = $true)][string]$MetricUnit,
        [Parameter(Mandatory = $true)][string]$BinaryRole,
        [Parameter(Mandatory = $true)][object[]]$RawArtifacts,
        [Parameter(Mandatory = $true)][string]$OutputPath
    )
    if ($RunMetrics.Count -ne $RunCount -or $RawArtifacts.Count -ne $RunCount) {
        throw "$Scenario candidate requires exactly $RunCount complete runs and artifacts"
    }
    if ($null -ne $RunRawSamples -and $RunRawSamples.Count -ne $RunCount) {
        throw "$Scenario candidate requires exactly $RunCount raw-sample sets"
    }
    $configHash = Get-CanonicalScenarioConfigHash -Scenario $Scenario -Value $ScenarioParameters
    $binaryProperty = $fingerprint.binary_sha256.PSObject.Properties[$BinaryRole]
    if ($null -eq $binaryProperty -or [string]$binaryProperty.Value -notmatch '^[0-9a-f]{64}$') {
        throw "$Scenario is missing required binary hash role $BinaryRole"
    }
    $metricNames = @($RunMetrics[0].Keys)
    $metricSummaries = [ordered]@{}
    foreach ($metricName in $metricNames) {
        $values = [double[]]@($RunMetrics | ForEach-Object { [double]$_[$metricName] })
        $metricSummaries[$metricName] = New-MetricSummary -Values $values -Unit $MetricUnit
    }
    $runs = @()
    for ($index = 0; $index -lt $RunCount; $index++) {
        $counters = [ordered]@{
            overwrite = 0
            gap = 0
            duplicate = 0
            out_of_order = 0
            reliable_overflow = 0
        }
        if ($null -ne $RunCounters) {
            foreach ($counterName in $RunCounters[$index].Keys) {
                $counters[$counterName] = [UInt64]$RunCounters[$index][$counterName]
            }
        }
        $runs += [ordered]@{
            run_id = "$BatchId-$($index + 1)"
            batch_id = $BatchId
            process_exit_success = $true
            schema_valid = $true
            scenario_config_sha256 = $configHash
            metrics = $RunMetrics[$index]
            counters = $counters
            raw_samples = if ($null -eq $RunRawSamples) {
                [ordered]@{}
            } else {
                $RunRawSamples[$index]
            }
            errors = @()
        }
    }
    $provenance = [ordered]@{}
    foreach ($metricName in $metricNames) {
        $provenance[$metricName] = [ordered]@{
            method = if ($Scenario -eq 'desktop-ui-state') { 'event_timestamp_to_react_passive_effect_after_committed_paint' } else { 'real_framed_loopback_ipc_round_trip' }
            uncertainty_us = if ($Scenario -eq 'desktop-ui-state') { 1000 } else { 1 }
            evidence_path = $null
            evidence_sha256 = $null
            estimate_only = $false
        }
    }
    $candidate = [ordered]@{
        schema_version = 1
        scenario = $Scenario
        scenario_parameters = $ScenarioParameters
        scenario_config_sha256 = $configHash
        random_seed = 0
        commit = [string]$fingerprint.commit
        dirty = [bool]$fingerprint.dirty
        binary_sha256 = [ordered]@{ $BinaryRole = [string]$binaryProperty.Value }
        cargo_lock_sha256 = [string]$fingerprint.cargo_lock_sha256
        build_profile = 'release'
        cargo_features = @()
        rustflags = [string]$fingerprint.rustflags
        runner_id = [string]$fingerprint.runner_id
        runner_fingerprint = [string]$fingerprint.runner_fingerprint
        availability = [ordered]@{ status = 'available' }
        toolchain = $fingerprint.toolchain
        hardware = $fingerprint.hardware
        warmup = [ordered]@{ millis = $WarmupSeconds * 1000 }
        batch_artifacts = @($RawArtifacts | ForEach-Object {
            [ordered]@{
                batch_id = $BatchId
                path = [string]$_.path
                sha256 = [string]$_.sha256
                verdict = 'pass'
            }
        })
        runs = $runs
        metrics = $metricSummaries
        queues = [ordered]@{}
        errors = @()
        rss = $null
        measurement_provenance = $provenance
        verdict = 'pass'
    }
    Write-JsonAtomically -Path $OutputPath -Value $candidate
    return $candidate
}

function Add-Artifact {
    param(
        [Parameter(Mandatory = $true)][string]$Kind,
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][int]$RunIndex
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "expected $Kind artifact is missing: $Path"
    }
    $summary.artifacts += [ordered]@{
        phase1_batch_id = $phase1BatchId
        kind = $Kind
        run_index = $RunIndex
        path = $Path
        sha256 = Get-Sha256File -Path $Path
    }
}

function Assert-QuicReport {
    param(
        [Parameter(Mandatory = $true)]$Report,
        [Parameter(Mandatory = $true)][string]$ScenarioName
    )

    if (@($Report.runs).Count -ne $RunCount) {
        throw "$ScenarioName produced $(@($Report.runs).Count) runs; exactly $RunCount are required"
    }
    $batchIds = @($Report.runs | ForEach-Object { [string]$_.batch_id } | Sort-Object -Unique)
    if ($batchIds.Count -ne 1 -or [string]::IsNullOrWhiteSpace($batchIds[0])) {
        throw "$ScenarioName is not one immutable exactly-five-run batch"
    }
    foreach ($run in $Report.runs) {
        if (-not [bool]$run.process_exit_success -or -not [bool]$run.schema_valid) {
            throw "$ScenarioName contains an incomplete run"
        }
        if (@($run.errors).Count -ne 0) {
            throw "$ScenarioName run $($run.run_id) reported errors"
        }
        if (@($run.raw_samples.latency_us).Count -eq 0) {
            throw "$ScenarioName run $($run.run_id) omitted raw latency samples"
        }
        if ([UInt64]$run.counters.reliable_overflow -ne 0 -or
            [UInt64]$run.counters.duplicate -ne 0 -or
            [UInt64]$run.counters.out_of_order -ne 0) {
            throw "$ScenarioName lost reliable correctness"
        }
        $expectedReliable = 100 * [UInt64]$run.counters.reliable_peers
        if ([UInt64]$run.counters.reliable_expected -ne $expectedReliable -or
            [UInt64]$run.counters.reliable_delivered -ne $expectedReliable -or
            [UInt64]$run.counters.stuck_modifiers -ne 0 -or
            [UInt64]$run.counters.stuck_mouse_buttons -ne 0 -or
            [UInt64]$run.counters.stale_replay_attempted -ne 1 -or
            [UInt64]$run.counters.stale_replay_filtered -ne 1 -or
            [UInt64]$run.counters.stale_replay_delivered -ne 0 -or
            [UInt64]$run.counters.same_epoch_stale_replay -ne 0) {
            throw "$ScenarioName did not preserve the mixed reliable press/release sequence"
        }
    }
    $queueProperties = @($Report.queues.PSObject.Properties)
    $requiredQueues = @(
        'producer_realtime',
        'fast_qos_realtime_direct',
        'fast_qos_reliable',
        'fast_qos_emergency',
        'fast_qos_control',
        'fast_qos_bulk',
        'fast_qos_telemetry',
        'fast_qos_terminal_release',
        'fast_receiver_qos_realtime_direct',
        'fast_receiver_qos_reliable',
        'fast_receiver_qos_emergency',
        'fast_receiver_qos_control',
        'fast_receiver_qos_bulk',
        'fast_receiver_qos_telemetry',
        'fast_receiver_qos_terminal_release',
        'fast_inbound_realtime',
        'fast_inbound_reliable',
        'fast_inbound_control',
        'fast_inbound_control_events',
        'fast_inbound_telemetry',
        'fast_inbound_telemetry_events',
        'fast_inbound_bulk',
        'fast_inbound_bulk_events',
        'fast_inbound_protocol_errors',
        'fast_inbound_terminal_release',
        'fast_sender_inbound_realtime',
        'fast_sender_inbound_reliable',
        'fast_sender_inbound_control',
        'fast_sender_inbound_control_events',
        'fast_sender_inbound_telemetry',
        'fast_sender_inbound_telemetry_events',
        'fast_sender_inbound_bulk',
        'fast_sender_inbound_bulk_events',
        'fast_sender_inbound_protocol_errors',
        'fast_sender_inbound_terminal_release'
    )
    if ($ScenarioName -eq 'quic-slow-fast') {
        $requiredQueues += @(
            'slow_qos_realtime_direct',
            'slow_qos_reliable',
            'slow_qos_emergency',
            'slow_qos_control',
            'slow_qos_bulk',
            'slow_qos_telemetry',
            'slow_qos_terminal_release',
            'slow_receiver_qos_realtime_direct',
            'slow_receiver_qos_reliable',
            'slow_receiver_qos_emergency',
            'slow_receiver_qos_control',
            'slow_receiver_qos_bulk',
            'slow_receiver_qos_telemetry',
            'slow_receiver_qos_terminal_release',
            'slow_inbound_realtime',
            'slow_inbound_reliable',
            'slow_inbound_control',
            'slow_inbound_control_events',
            'slow_inbound_telemetry',
            'slow_inbound_telemetry_events',
            'slow_inbound_bulk',
            'slow_inbound_bulk_events',
            'slow_inbound_protocol_errors',
            'slow_inbound_terminal_release',
            'slow_sender_inbound_realtime',
            'slow_sender_inbound_reliable',
            'slow_sender_inbound_control',
            'slow_sender_inbound_control_events',
            'slow_sender_inbound_telemetry',
            'slow_sender_inbound_telemetry_events',
            'slow_sender_inbound_bulk',
            'slow_sender_inbound_bulk_events',
            'slow_sender_inbound_protocol_errors',
            'slow_sender_inbound_terminal_release'
        )
    }
    foreach ($requiredQueue in $requiredQueues) {
        if ($Report.queues.PSObject.Properties.Name -notcontains $requiredQueue) {
            throw "$ScenarioName is missing production queue evidence $requiredQueue"
        }
    }
    foreach ($property in $queueProperties) {
        $queue = $property.Value
        if ([UInt64]$queue.high_watermark -gt [UInt64]$queue.capacity) {
            throw "$ScenarioName queue $($property.Name) exceeded its declared capacity"
        }
        if ([UInt64]$queue.overflows -ne 0) {
            throw "$ScenarioName queue $($property.Name) overflowed"
        }
    }
    if ([string]$Report.verdict -ne 'pass') {
        throw "$ScenarioName verdict is $($Report.verdict), not pass"
    }
}

function Parse-BaselineManifest {
    param([Parameter(Mandatory = $true)][string]$Path)

    $entries = @()
    $current = $null
    foreach ($line in Get-Content -LiteralPath $Path) {
        $trimmed = $line.Trim()
        if ($trimmed -eq '[[baseline]]') {
            if ($null -ne $current) {
                $entries += [PSCustomObject]$current
            }
            $current = [ordered]@{}
            continue
        }
        if ($null -eq $current -or $trimmed.Length -eq 0 -or $trimmed.StartsWith('#')) {
            continue
        }
        if ($trimmed -match '^([A-Za-z0-9_]+)\s*=\s*"(.*)"\s*$') {
            $current[$Matches[1]] = $Matches[2]
        }
        elseif ($trimmed -match "^([A-Za-z0-9_]+)\s*=\s*'(.*)'\s*$") {
            $current[$Matches[1]] = $Matches[2]
        }
    }
    if ($null -ne $current) {
        $entries += [PSCustomObject]$current
    }
    return @($entries)
}

function Resolve-ReviewedBaseline {
    param(
        [Parameter(Mandatory = $true)]$Candidate,
        [Parameter(Mandatory = $true)][object[]]$ManifestEntries
    )

    $matches = @($ManifestEntries | Where-Object {
        [string]$_.scenario -eq [string]$Candidate.scenario -and
        [string]$_.scenario_config_sha256 -eq [string]$Candidate.scenario_config_sha256 -and
        [string]$_.runner_fingerprint -eq [string]$Candidate.runner_fingerprint
    })
    if ($matches.Count -ne 1) {
        throw "expected exactly one reviewed baseline for runner=$($Candidate.runner_fingerprint), scenario=$($Candidate.scenario), config=$($Candidate.scenario_config_sha256); found $($matches.Count)"
    }

    $entry = $matches[0]
    foreach ($field in @('id', 'artifact_path', 'artifact_sha256', 'source_commit', 'approval_ref')) {
        if ($entry.PSObject.Properties.Name -notcontains $field -or [string]::IsNullOrWhiteSpace([string]$entry.$field)) {
            throw "baseline entry is missing required field $field"
        }
    }
    if ([string]$entry.artifact_sha256 -notmatch '^[0-9a-fA-F]{64}$') {
        throw "baseline $($entry.id) has an invalid artifact SHA-256"
    }

    $artifactPath = [IO.Path]::GetFullPath((Join-Path $repositoryRoot ([string]$entry.artifact_path)))
    $rootPrefix = $repositoryRoot.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    if (-not $artifactPath.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "baseline $($entry.id) artifact escapes the repository root"
    }
    if (-not (Test-Path -LiteralPath $artifactPath -PathType Leaf)) {
        throw "baseline $($entry.id) artifact is missing: $artifactPath"
    }
    $actualHash = Get-Sha256File -Path $artifactPath
    if ($actualHash -ne ([string]$entry.artifact_sha256).ToLowerInvariant()) {
        throw "baseline $($entry.id) artifact hash mismatch"
    }
    return $entry
}

function Assert-IpcRawReport {
    param([Parameter(Mandatory = $true)]$Report)

    if (-not [bool]$Report.ephemeral_port) {
        throw 'IPC harness did not bind an ephemeral port'
    }
    $rows = @($Report.runs)
    if ($rows.Count -ne 2) {
        throw "IPC harness must report sequential and concurrency-8 rows; got $($rows.Count)"
    }
    $sequential = @($rows | Where-Object { [int]$_.concurrency -eq 1 })
    $concurrent = @($rows | Where-Object { [int]$_.concurrency -eq 8 })
    if ($sequential.Count -ne 1 -or $concurrent.Count -ne 1) {
        throw 'IPC harness did not report exactly concurrency 1 and 8'
    }
    if ([int]$sequential[0].completed_requests -ne 500 -or [int]$sequential[0].handler_dispatches -ne 500) {
        throw 'IPC sequential run was incomplete'
    }
    if ([int]$concurrent[0].completed_requests -ne 4000 -or [int]$concurrent[0].handler_dispatches -ne 4000) {
        throw 'IPC concurrency-8 run was incomplete'
    }
    if (@($sequential[0].latency_samples_us).Count -ne 500 -or
        @($concurrent[0].latency_samples_us).Count -ne 4000) {
        throw 'IPC raw latency samples are incomplete'
    }
    if ([UInt64]$sequential[0].p99_us -gt 100000) {
        throw "IPC sequential p99 exceeded the 100 ms catastrophe limit: $($sequential[0].p99_us) us"
    }
    if ([UInt64]$concurrent[0].p99_us -gt 200000) {
        throw "IPC concurrency-8 p99 exceeded the 200 ms catastrophe limit: $($concurrent[0].p99_us) us"
    }
}

function Get-UiMetric {
    param(
        [Parameter(Mandatory = $true)]$Report,
        [Parameter(Mandatory = $true)][string]$Name
    )

    if ($Report.PSObject.Properties.Name -contains $Name) {
        return [double]$Report.$Name
    }
    if ($Report.PSObject.Properties.Name -contains 'metrics' -and
        $Report.metrics.PSObject.Properties.Name -contains $Name) {
        return [double]$Report.metrics.$Name
    }
    throw "UI report is missing required metric $Name"
}

function Get-CoefficientOfVariation {
    param([Parameter(Mandatory = $true)][double[]]$Values)

    if ($Values.Count -ne $RunCount) {
        throw "coefficient of variation requires exactly $RunCount values"
    }
    $mean = ($Values | Measure-Object -Average).Average
    if ($mean -eq 0.0) {
        $nonZero = @($Values | Where-Object { $_ -ne 0.0 })
        if ($nonZero.Count -eq 0) {
            return 0.0
        }
        return [double]::PositiveInfinity
    }
    $sumSquares = 0.0
    foreach ($value in $Values) {
        $difference = $value - $mean
        $sumSquares += $difference * $difference
    }
    $sampleDeviation = [Math]::Sqrt($sumSquares / ($Values.Count - 1))
    return $sampleDeviation / [Math]::Abs($mean)
}

function Get-IpcBatchVariation {
    param([Parameter(Mandatory = $true)][object[]]$RunReports)

    $variation = [ordered]@{}
    foreach ($concurrency in @(1, 8)) {
        foreach ($metric in @('median_us', 'p95_us', 'p99_us', 'max_us')) {
            $values = @($RunReports | ForEach-Object {
                $row = @($_.runs | Where-Object { [int]$_.concurrency -eq $concurrency })
                if ($row.Count -ne 1) {
                    throw "IPC run is missing concurrency $concurrency"
                }
                [double]$row[0].$metric
            })
            $variation["concurrency_${concurrency}_$metric"] = Get-CoefficientOfVariation -Values $values
        }
    }
    return $variation
}

function Get-UiBatchVariation {
    param([Parameter(Mandatory = $true)][object[]]$RunReports)

    $variation = [ordered]@{}
    foreach ($metric in @(
        'paint_p50_ms',
        'paint_p95_ms',
        'paint_p99_ms',
        'paint_max_ms',
        'topology_status_p50_ms',
        'topology_status_p95_ms',
        'topology_status_p99_ms',
        'topology_status_max_ms'
    )) {
        $available = @($RunReports | Where-Object {
            $_.PSObject.Properties.Name -contains $metric -or
            ($_.PSObject.Properties.Name -contains 'metrics' -and $_.metrics.PSObject.Properties.Name -contains $metric)
        })
        if ($available.Count -eq $RunCount) {
            $values = @($RunReports | ForEach-Object { Get-UiMetric -Report $_ -Name $metric })
            $variation[$metric] = Get-CoefficientOfVariation -Values $values
        }
    }
    foreach ($required in @(
        'paint_p95_ms',
        'paint_p99_ms',
        'topology_status_p95_ms',
        'topology_status_p99_ms'
    )) {
        if (-not $variation.Contains($required)) {
            throw "UI batch cannot calculate CV for required metric $required"
        }
    }
    return $variation
}

function Test-VariationUnstable {
    param([Parameter(Mandatory = $true)]$Variation)

    foreach ($property in $Variation.GetEnumerator()) {
        if ([double]$property.Value -gt 0.10) {
            return $true
        }
    }
    return $false
}

function Assert-UiReport {
    param(
        [Parameter(Mandatory = $true)]$Report,
        [Parameter(Mandatory = $true)][int]$ExpectedRunIndex,
        [Parameter(Mandatory = $true)][int]$ExpectedAttempt
    )

    if ([string]$Report.batch_id -ne $phase1BatchId) {
        throw "UI run $ExpectedRunIndex did not retain phase batch id $phase1BatchId"
    }
    if ([int]$Report.run_index -ne $ExpectedRunIndex) {
        throw "UI report run index is $($Report.run_index), expected $ExpectedRunIndex"
    }
    if ([int]$Report.batch_attempt -ne $ExpectedAttempt) {
        throw "UI report batch attempt is $($Report.batch_attempt), expected $ExpectedAttempt"
    }
    if ([string]::IsNullOrWhiteSpace([string]$Report.environment.node_version) -or
        [string]::IsNullOrWhiteSpace([string]$Report.environment.playwright_version) -or
        [string]::IsNullOrWhiteSpace([string]$Report.environment.os_version) -or
        [string]::IsNullOrWhiteSpace([string]$Report.environment.os_release) -or
        [string]$Report.environment.browser_name -ne 'chromium' -or
        [string]::IsNullOrWhiteSpace([string]$Report.environment.browser_version) -or
        [string]::IsNullOrWhiteSpace([string]$Report.environment.graphics.vendor) -or
        [string]::IsNullOrWhiteSpace([string]$Report.environment.graphics.renderer) -or
        -not [bool]$Report.environment.headless -or
        [int]$Report.environment.viewport.width -ne 1440 -or
        [int]$Report.environment.viewport.height -ne 900) {
        throw "UI run $ExpectedRunIndex did not record the required browser execution environment"
    }
    if ([int]$Report.pointer_deltas_sent -ne 10000 -or
        [int]$Report.discrete_transitions_sent -ne 100 -or
        [int]$Report.discrete_transitions_applied -ne 100 -or
        [int]$Report.discrete_paint_sample_count -ne 100 -or
        [string]$Report.final_discrete_state -ne 'Released') {
        throw "UI run $ExpectedRunIndex did not apply the complete ordered input stream"
    }
    if ([int]$Report.paint_sample_count -lt 300) {
        throw "UI run $ExpectedRunIndex produced fewer than 300 applied-event paint samples"
    }
    if ([int]$Report.topology_status_sample_count -ne 100) {
        throw "UI run $ExpectedRunIndex did not paint all 100 topology/status transitions"
    }
    if (@($Report.paint_samples_ms).Count -ne [int]$Report.paint_sample_count -or
        @($Report.discrete_paint_samples_ms).Count -ne 100 -or
        @($Report.topology_status_samples_ms).Count -ne 100) {
        throw "UI run $ExpectedRunIndex did not preserve recomputable raw paint samples"
    }
    if ((Get-UiMetric -Report $Report -Name 'paint_p95_ms') -gt 16.7) {
        throw "UI run $ExpectedRunIndex paint p95 exceeded 16.7 ms"
    }
    if ((Get-UiMetric -Report $Report -Name 'paint_p99_ms') -gt 33.0) {
        throw "UI run $ExpectedRunIndex paint p99 exceeded 33 ms"
    }
    if ((Get-UiMetric -Report $Report -Name 'topology_status_p95_ms') -gt 50.0) {
        throw "UI run $ExpectedRunIndex topology/status p95 exceeded 50 ms"
    }
    if ((Get-UiMetric -Report $Report -Name 'topology_status_p99_ms') -gt 100.0) {
        throw "UI run $ExpectedRunIndex topology/status p99 exceeded 100 ms"
    }
    if ((Get-UiMetric -Report $Report -Name 'topology_commits_during_pointer_flood') -ne 0) {
        throw "UI run $ExpectedRunIndex committed topology during pointer flood"
    }
    if ((Get-UiMetric -Report $Report -Name 'long_tasks_over_50ms') -ne 0) {
        throw "UI run $ExpectedRunIndex observed a long task over 50 ms"
    }
    if ((Get-UiMetric -Report $Report -Name 'dashboard_or_endpoint_polls_while_healthy') -ne 0) {
        throw "UI run $ExpectedRunIndex polled dashboard/endpoint state while healthy"
    }
}

function Assert-ReproducibilityStable {
    param(
        [Parameter(Mandatory = $true)]$Initial,
        [Parameter(Mandatory = $true)]$Current
    )
    if ([bool]$Current.dirty) {
        throw 'Phase 1 changed the worktree; refusing to publish candidates from a dirty source state'
    }
    foreach ($field in @('commit', 'cargo_lock_sha256', 'runner_fingerprint', 'power_plan_guid', 'process_affinity_mask')) {
        if ([string]$Current.$field -ne [string]$Initial.$field) {
            throw "runner fingerprint field '$field' changed during Phase 1"
        }
    }
    if (($Initial.binary_sha256 | ConvertTo-Json -Compress) -ne
        ($Current.binary_sha256 | ConvertTo-Json -Compress)) {
        throw 'measured binary hashes changed during Phase 1'
    }
}

try {
    if ($env:OS -ne 'Windows_NT') {
        throw 'Phase 1 strict timing acceptance requires the fixed Windows runner'
    }

    [IO.Directory]::CreateDirectory($rawDirectory) | Out-Null
    [IO.Directory]::CreateDirectory($candidateDirectory) | Out-Null
    [IO.Directory]::CreateDirectory($logDirectory) | Out-Null
    Set-Location $repositoryRoot
    [Environment]::SetEnvironmentVariable('RSHARE_PERF_BATCH_ID', $phase1BatchId, 'Process')
    [Environment]::SetEnvironmentVariable('RSHARE_PERF_WARMUP_MS', [string]($WarmupSeconds * 1000), 'Process')
    $activePowerPlan = (& powercfg.exe /getactivescheme | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $activePowerPlan -notmatch '([0-9A-Fa-f-]{36})') {
        throw "unable to capture the active power-plan GUID: $activePowerPlan"
    }
    [Environment]::SetEnvironmentVariable('RSHARE_PERF_POWER_PLAN_GUID', $Matches[1].ToLowerInvariant(), 'Process')
    $activeAffinity = [UInt64][Diagnostics.Process]::GetCurrentProcess().ProcessorAffinity.ToInt64()
    if ($activeAffinity -eq 0) {
        throw 'performance runner has an empty CPU affinity mask'
    }
    [Environment]::SetEnvironmentVariable('RSHARE_PERF_AFFINITY_MASK', $activeAffinity.ToString('x'), 'Process')

    Invoke-LoggedCommand -FilePath 'cargo.exe' -Arguments @('build', '--release', '--locked', '-p', 'rshare-perf') -LogPath (Join-Path $logDirectory 'cargo-build-perf.log')
    Invoke-LoggedCommand -FilePath 'npm.cmd' -Arguments @('--prefix', 'apps/rshare-desktop-frontend', 'run', 'build') -LogPath (Join-Path $logDirectory 'frontend-build.log')

    $summary.correctness.status = 'RUNNING'
    $summary.correctness.log = Join-Path $logDirectory 'cargo-test-workspace.log'
    Save-Summary
    Invoke-LoggedCommand -FilePath 'cargo.exe' -Arguments @('test', '--workspace', '--locked') -LogPath $summary.correctness.log
    $summary.correctness.status = 'PASS'
    Save-Summary

    & (Join-Path $PSScriptRoot 'collect-runner-fingerprint.ps1') -OutputPath $fingerprintPath | Out-Host
    $summary.reproducibility = Get-Content -LiteralPath $fingerprintPath -Raw | ConvertFrom-Json
    if ([bool]$summary.reproducibility.dirty) {
        throw 'Phase 1 baseline and strict runs require a clean worktree so commit and binary hashes describe the same source'
    }
    $initialFingerprint = $summary.reproducibility
    Save-Summary

    if ($WarmupSeconds -gt 0) {
        Write-Host "Warming fixed runner for $WarmupSeconds seconds"
        Start-Sleep -Seconds $WarmupSeconds
    }

    $quicScenarios = @(
        [ordered]@{
            name = 'quic-1000hz'
            arguments = @('quic', '--rate-hz', '1000', '--duration-secs', '60')
        },
        [ordered]@{
            name = 'quic-1000hz-loaded'
            arguments = @('quic', '--rate-hz', '1000', '--duration-secs', '60', '--load', 'diagnostics,status,audio,bulk')
        },
        [ordered]@{
            name = 'quic-slow-fast'
            arguments = @('quic', '--slow-fast-isolation')
        },
        [ordered]@{
            name = 'quic-stall-100ms'
            arguments = @('quic', '--stall-ms', '100')
        }
    )

    foreach ($scenario in $quicScenarios) {
        $candidatePath = Join-Path $candidateDirectory "$($scenario.name).json"
        $logPath = Join-Path $logDirectory "$($scenario.name).log"
        $arguments = @('run', '--release', '--locked', '-p', 'rshare-perf', '--') + $scenario.arguments + @('--output', $candidatePath)
        Invoke-LoggedCommand -FilePath 'cargo.exe' -Arguments $arguments -LogPath $logPath
        $candidate = Get-Content -LiteralPath $candidatePath -Raw | ConvertFrom-Json
        Assert-QuicReport -Report $candidate -ScenarioName $scenario.name
        Add-Artifact -Kind $scenario.name -Path $candidatePath -RunIndex 0
        $summary.quic += [ordered]@{
            phase1_batch_id = $phase1BatchId
            name = $scenario.name
            status = 'PASS'
            candidate = $candidatePath
            harness_batch_id = [string]$candidate.runs[0].batch_id
            scenario = [string]$candidate.scenario
            scenario_config_sha256 = [string]$candidate.scenario_config_sha256
            runner_fingerprint = [string]$candidate.runner_fingerprint
        }
        Save-Summary
    }
    $unloaded = Get-Content -LiteralPath (($summary.quic | Where-Object { $_.name -eq 'quic-1000hz' }).candidate) -Raw | ConvertFrom-Json
    $loaded = Get-Content -LiteralPath (($summary.quic | Where-Object { $_.name -eq 'quic-1000hz-loaded' }).candidate) -Raw | ConvertFrom-Json
    $slowFast = Get-Content -LiteralPath (($summary.quic | Where-Object { $_.name -eq 'quic-slow-fast' }).candidate) -Raw | ConvertFrom-Json
    $stall = Get-Content -LiteralPath (($summary.quic | Where-Object { $_.name -eq 'quic-stall-100ms' }).candidate) -Raw | ConvertFrom-Json
    $unloadedP99 = [double]$unloaded.metrics.p99_us.max
    $loadedP99 = [double]$loaded.metrics.p99_us.max
    if (($loadedP99 - $unloadedP99) -gt 5000.0) {
        throw "loaded QUIC p99 regressed by $($loadedP99 - $unloadedP99) us; limit is 5000 us"
    }
    $slowFastP99 = [double]$slowFast.metrics.fast_peer_p99_us.max
    if (($slowFastP99 - $unloadedP99) -gt 2000.0) {
        throw "slow-peer isolation regressed fast-peer p99 by $($slowFastP99 - $unloadedP99) us; limit is 2000 us"
    }
    if ([double]$stall.metrics.stall_recovery_us.max -gt 20000.0) {
        throw "stall recovery exceeded 20000 us"
    }
    foreach ($run in $stall.runs) {
        if ($run.counters.PSObject.Properties.Name -notcontains 'stale_replay_delivered' -or
            [UInt64]$run.counters.stale_replay_delivered -ne 0 -or
            [UInt64]$run.counters.same_epoch_stale_replay -ne 0 -or
            [UInt64]$run.counters.stall_converged_sequence -lt
                [UInt64]$run.counters.stall_convergence_floor_sequence) {
            throw "stall recovery did not prove zero stale delivery and same-epoch convergence"
        }
    }
    $summary.phase1_thresholds = [ordered]@{
        loaded_p99_delta_us = $loadedP99 - $unloadedP99
        loaded_p99_delta_limit_us = 5000.0
        slow_peer_fast_path_p99_delta_us = $slowFastP99 - $unloadedP99
        slow_peer_fast_path_p99_delta_limit_us = 2000.0
        stall_recovery_us = [double]$stall.metrics.stall_recovery_us.max
        stall_recovery_limit_us = 20000.0
        stale_replay_delivered = 0
        status = 'PASS'
    }
    Save-Summary

    $summary.ipc.status = 'RUNNING'
    Save-Summary
    for ($attempt = 1; $attempt -le 2; $attempt++) {
        $ipcBatchRuns = @()
        $ipcReports = @()
        for ($runIndex = 1; $runIndex -le $RunCount; $runIndex++) {
            $toolOutputPath = Join-Path $rawDirectory "ipc-batch-$attempt-tool-run-$runIndex.json"
            $rawPath = Join-Path $rawDirectory "ipc-batch-$attempt-run-$runIndex.json"
            $logPath = Join-Path $logDirectory "ipc-batch-$attempt-run-$runIndex.log"
            Invoke-LoggedCommand -FilePath 'cargo.exe' -Arguments @(
                'run', '--release', '--locked', '-p', 'rshare-perf', '--',
                'ipc', '--requests', '500', '--concurrency', '1,8', '--output', $toolOutputPath
            ) -LogPath $logPath
            $ipcReport = Get-Content -LiteralPath $toolOutputPath -Raw | ConvertFrom-Json
            Assert-IpcRawReport -Report $ipcReport
            $toolOutputHash = Get-Sha256File -Path $toolOutputPath
            $raw = [ordered]@{
                phase1_batch_id = $phase1BatchId
                batch_attempt = $attempt
                run_index = $runIndex
                process_exit_success = $true
                tool_artifact_path = $toolOutputPath
                tool_artifact_sha256 = $toolOutputHash
                report = $ipcReport
            }
            Write-JsonAtomically -Path $rawPath -Value $raw
            Add-Artifact -Kind 'ipc-tool-raw' -Path $toolOutputPath -RunIndex $runIndex
            Add-Artifact -Kind 'ipc' -Path $rawPath -RunIndex $runIndex
            $ipcReports += $ipcReport
            $ipcBatchRuns += [ordered]@{
                phase1_batch_id = $phase1BatchId
                batch_attempt = $attempt
                run_index = $runIndex
                path = $rawPath
                sha256 = Get-Sha256File -Path $rawPath
                tool_path = $toolOutputPath
                tool_sha256 = $toolOutputHash
            }
            Save-Summary
        }
        if ($ipcBatchRuns.Count -ne $RunCount) {
            throw "IPC batch attempt $attempt is incomplete; expected $RunCount raw runs"
        }
        $ipcVariation = Get-IpcBatchVariation -RunReports $ipcReports
        $ipcUnstable = Test-VariationUnstable -Variation $ipcVariation
        $summary.ipc.batches += [ordered]@{
            phase1_batch_id = $phase1BatchId
            attempt = $attempt
            status = if ($ipcUnstable) { 'UNSTABLE' } else { 'PASS' }
            coefficient_of_variation = $ipcVariation
            runs = $ipcBatchRuns
        }
        Save-Summary
        if (-not $ipcUnstable) {
            $summary.ipc.selected_attempt = $attempt
            break
        }
        if ($attempt -eq 2) {
            throw 'IPC remained unstable after one complete exactly-five-run batch retry'
        }
    }
    $summary.ipc.status = 'PASS'

    $summary.ui.status = 'RUNNING'
    Save-Summary
    for ($attempt = 1; $attempt -le 2; $attempt++) {
        $uiBatchRuns = @()
        $uiReports = @()
        for ($runIndex = 1; $runIndex -le $RunCount; $runIndex++) {
            $rawPath = Join-Path $rawDirectory "ui-batch-$attempt-run-$runIndex.json"
            $logPath = Join-Path $logDirectory "ui-batch-$attempt-run-$runIndex.log"
            [Environment]::SetEnvironmentVariable('RSHARE_PERF_RUN_INDEX', [string]$runIndex, 'Process')
            [Environment]::SetEnvironmentVariable('RSHARE_PERF_OUTPUT', $rawPath, 'Process')
            [Environment]::SetEnvironmentVariable('RSHARE_PERF_BATCH_ATTEMPT', [string]$attempt, 'Process')
            [Environment]::SetEnvironmentVariable(
                'RSHARE_PERF_PLAYWRIGHT_OUTPUT',
                (Join-Path $batchDirectory "playwright\attempt-$attempt-run-$runIndex"),
                'Process'
            )
            Invoke-LoggedCommand -FilePath 'npm.cmd' -Arguments @(
                '--prefix', 'apps/rshare-desktop-frontend', 'run', 'test:perf', '--', '--grep', '@fixed-runner'
            ) -LogPath $logPath
            if (-not (Test-Path -LiteralPath $rawPath -PathType Leaf)) {
                throw "UI run $runIndex did not write RSHARE_PERF_OUTPUT $rawPath"
            }
            $uiReport = Get-Content -LiteralPath $rawPath -Raw | ConvertFrom-Json
            Assert-UiReport -Report $uiReport -ExpectedRunIndex $runIndex -ExpectedAttempt $attempt
            Add-Artifact -Kind 'ui' -Path $rawPath -RunIndex $runIndex
            $uiReports += $uiReport
            $uiBatchRuns += [ordered]@{
                phase1_batch_id = $phase1BatchId
                batch_attempt = $attempt
                run_index = $runIndex
                path = $rawPath
                sha256 = Get-Sha256File -Path $rawPath
            }
            Save-Summary
        }
        if ($uiBatchRuns.Count -ne $RunCount) {
            throw "UI batch attempt $attempt is incomplete; expected $RunCount raw runs"
        }
        $uiVariation = Get-UiBatchVariation -RunReports $uiReports
        $uiUnstable = Test-VariationUnstable -Variation $uiVariation
        $summary.ui.batches += [ordered]@{
            phase1_batch_id = $phase1BatchId
            attempt = $attempt
            status = if ($uiUnstable) { 'UNSTABLE' } else { 'PASS' }
            coefficient_of_variation = $uiVariation
            runs = $uiBatchRuns
        }
        Save-Summary
        if (-not $uiUnstable) {
            $summary.ui.selected_attempt = $attempt
            break
        }
        if ($attempt -eq 2) {
            throw 'UI remained unstable after one complete exactly-five-run batch retry'
        }
    }
    $summary.ui.status = 'PASS'

    & (Join-Path $PSScriptRoot 'collect-runner-fingerprint.ps1') -OutputPath $fingerprintPath | Out-Host
    $fingerprint = Get-Content -LiteralPath $fingerprintPath -Raw | ConvertFrom-Json
    Assert-ReproducibilityStable -Initial $initialFingerprint -Current $fingerprint
    $summary.reproducibility = $fingerprint
    Add-Artifact -Kind 'runner-fingerprint' -Path $fingerprintPath -RunIndex 0

    $ipcRunMetrics = @($ipcReports | ForEach-Object {
        $sequential = @($_.runs | Where-Object { [int]$_.concurrency -eq 1 })[0]
        $concurrent = @($_.runs | Where-Object { [int]$_.concurrency -eq 8 })[0]
        [ordered]@{
            concurrent8_median_us = [double]$concurrent.median_us
            concurrent8_p95_us = [double]$concurrent.p95_us
            concurrent8_p99_us = [double]$concurrent.p99_us
            sequential_median_us = [double]$sequential.median_us
            sequential_p95_us = [double]$sequential.p95_us
            sequential_p99_us = [double]$sequential.p99_us
        }
    })
    $ipcRunRawSamples = @($ipcReports | ForEach-Object {
        $sequential = @($_.runs | Where-Object { [int]$_.concurrency -eq 1 })[0]
        $concurrent = @($_.runs | Where-Object { [int]$_.concurrency -eq 8 })[0]
        [ordered]@{
            sequential_latency_us = @($sequential.latency_samples_us | ForEach-Object { [double]$_ })
            concurrent8_latency_us = @($concurrent.latency_samples_us | ForEach-Object { [double]$_ })
        }
    })
    $ipcCandidatePath = Join-Path $candidateDirectory 'daemon-framed-ipc.json'
    $ipcCandidate = New-Phase1Candidate `
        -Scenario 'daemon-framed-ipc' `
        -ScenarioParameters ([ordered]@{ concurrency = @(1, 8); requests = 500 }) `
        -BatchId "$phase1BatchId-ipc-attempt-$($summary.ipc.selected_attempt)" `
        -RunMetrics $ipcRunMetrics `
        -RunRawSamples $ipcRunRawSamples `
        -MetricUnit 'us' `
        -BinaryRole 'rshare-perf' `
        -RawArtifacts $ipcBatchRuns `
        -OutputPath $ipcCandidatePath
    $summary.ipc.candidate = $ipcCandidatePath
    Invoke-LoggedCommand -FilePath 'cargo.exe' -Arguments @(
        'run', '--release', '--locked', '-p', 'rshare-perf', '--',
        'validate', '--candidate', $ipcCandidatePath
    ) -LogPath (Join-Path $logDirectory 'validate-daemon-framed-ipc.log')
    Add-Artifact -Kind 'ipc-candidate' -Path $ipcCandidatePath -RunIndex 0

    $uiMetricNames = @(
        'paint_max_ms',
        'paint_p50_ms',
        'paint_p95_ms',
        'paint_p99_ms',
        'topology_status_max_ms',
        'topology_status_p50_ms',
        'topology_status_p95_ms',
        'topology_status_p99_ms'
    )
    $uiRunMetrics = @($uiReports | ForEach-Object {
        $values = [ordered]@{}
        foreach ($metricName in $uiMetricNames) {
            $values[$metricName] = Get-UiMetric -Report $_ -Name $metricName
        }
        $values
    })
    $uiRunCounters = @($uiReports | ForEach-Object {
        [ordered]@{
            discrete_transitions_applied = [UInt64]$_.discrete_transitions_applied
            healthy_fallback_polls = [UInt64]$_.dashboard_or_endpoint_polls_while_healthy
            input_commits = [UInt64]$_.input_commits_during_pointer_flood
            long_tasks_over_50ms = [UInt64]$_.long_tasks_over_50ms
            paint_samples = [UInt64]$_.paint_sample_count
            react_commits = [UInt64]$_.react_commits_during_pointer_flood
            topology_commits = [UInt64]$_.topology_commits_during_pointer_flood
        }
    })
    $uiRunRawSamples = @($uiReports | ForEach-Object {
        [ordered]@{
            paint_ms = @($_.paint_samples_ms | ForEach-Object { [double]$_ })
            discrete_paint_ms = @($_.discrete_paint_samples_ms | ForEach-Object { [double]$_ })
            topology_status_ms = @($_.topology_status_samples_ms | ForEach-Object { [double]$_ })
        }
    })
    $uiCandidatePath = Join-Path $candidateDirectory 'desktop-ui-state.json'
    $uiEnvironment = $uiReports[0].environment
    $uiEnvironmentJson = $uiEnvironment | ConvertTo-Json -Depth 8 -Compress
    foreach ($report in $uiReports) {
        if (($report.environment | ConvertTo-Json -Depth 8 -Compress) -ne $uiEnvironmentJson) {
            throw 'UI browser execution environment changed inside the selected five-run batch'
        }
    }
    $uiScenarioParameters = [ordered]@{
        discrete_transitions = 100
        duration_ms = 10000
        pointer_hz = 1000
        topology_status_transitions = 100
        environment = $uiEnvironment
        package_lock_sha256 = Get-Sha256File -Path (Join-Path $repositoryRoot 'apps\rshare-desktop-frontend\package-lock.json')
        playwright_config_sha256 = Get-Sha256File -Path (Join-Path $repositoryRoot 'apps\rshare-desktop-frontend\playwright.perf.config.mjs')
        ui_scenario_sha256 = Get-Sha256File -Path (Join-Path $repositoryRoot 'apps\rshare-desktop-frontend\tests\performance\ui-state.spec.mjs')
    }
    $uiCandidate = New-Phase1Candidate `
        -Scenario 'desktop-ui-state' `
        -ScenarioParameters $uiScenarioParameters `
        -BatchId "$phase1BatchId-ui-attempt-$($summary.ui.selected_attempt)" `
        -RunMetrics $uiRunMetrics `
        -RunCounters $uiRunCounters `
        -RunRawSamples $uiRunRawSamples `
        -MetricUnit 'ms' `
        -BinaryRole 'rshare-desktop-frontend' `
        -RawArtifacts $uiBatchRuns `
        -OutputPath $uiCandidatePath
    $summary.ui.candidate = $uiCandidatePath
    Invoke-LoggedCommand -FilePath 'cargo.exe' -Arguments @(
        'run', '--release', '--locked', '-p', 'rshare-perf', '--',
        'validate', '--candidate', $uiCandidatePath
    ) -LogPath (Join-Path $logDirectory 'validate-desktop-ui-state.log')
    Add-Artifact -Kind 'ui-candidate' -Path $uiCandidatePath -RunIndex 0
    Save-Summary

    $phase1Candidates = @($summary.quic | ForEach-Object {
        [ordered]@{ name = $_.name; path = $_.candidate }
    })
    $phase1Candidates += [ordered]@{ name = 'daemon-framed-ipc'; path = $summary.ipc.candidate }
    $phase1Candidates += [ordered]@{ name = 'desktop-ui-state'; path = $summary.ui.candidate }

    if ($Mode -eq 'Bootstrap') {
        $bootstrapCandidates = @()
        foreach ($scenario in $phase1Candidates) {
            $candidate = Get-Content -LiteralPath $scenario.path -Raw | ConvertFrom-Json
            $suggestedId = "windows-$($scenario.name)-$($candidate.runner_fingerprint.Substring(0, 12))"
            $packageDirectory = Join-Path $batchDirectory "baseline-package\$suggestedId"
            $repositoryPackageDirectory =
                Join-Path $packageDirectory "perf\baselines\candidates\$suggestedId"
            $packageRawDirectory = Join-Path $repositoryPackageDirectory 'raw'
            [IO.Directory]::CreateDirectory($packageRawDirectory) | Out-Null
            $pathMap = @{}
            $hashMap = @{}
            $artifactIndex = 0
            foreach ($reference in @($candidate.batch_artifacts)) {
                $sourcePath = [IO.Path]::GetFullPath([string]$reference.path)
                if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) {
                    throw "baseline package sidecar is missing: $sourcePath"
                }
                $artifactIndex++
                $packagedName = '{0:D2}-{1}' -f $artifactIndex, [IO.Path]::GetFileName($sourcePath)
                $packagedPath = Join-Path $packageRawDirectory $packagedName
                $sidecarJson = [IO.File]::ReadAllText($sourcePath)
                if ([string]::IsNullOrWhiteSpace($sidecarJson)) {
                    throw "baseline package sidecar is empty: $sourcePath"
                }
                $sourceHash = Get-Sha256File -Path $sourcePath
                if ($sourceHash -ne [string]$reference.sha256) {
                    throw "baseline package source sidecar hash drifted: $sourcePath"
                }
                $sidecarPayload = $sidecarJson | ConvertFrom-Json
                $sidecarEnvelope = [ordered]@{
                    schema_version = 1
                    scenario = [string]$candidate.scenario
                    scenario_config_sha256 = [string]$candidate.scenario_config_sha256
                    batch_id = [string]$reference.batch_id
                    source_sha256 = $sourceHash
                    payload = $sidecarPayload
                }
                Write-JsonAtomically -Path $packagedPath -Value $sidecarEnvelope
                $repositorySidecarPath = "perf/baselines/candidates/$suggestedId/raw/$packagedName"
                $pathMap[$sourcePath] = $repositorySidecarPath
                $hashMap[$sourcePath] = Get-Sha256File -Path $packagedPath
                $reference.path = $repositorySidecarPath
                $reference.sha256 = $hashMap[$sourcePath]
            }
            if ($artifactIndex -eq 0) {
                throw "$($scenario.name) cannot be promoted without at least one raw batch sidecar"
            }
            foreach ($property in @($candidate.measurement_provenance.PSObject.Properties)) {
                $evidencePath = [string]$property.Value.evidence_path
                if (-not [string]::IsNullOrWhiteSpace($evidencePath)) {
                    $absoluteEvidencePath = [IO.Path]::GetFullPath($evidencePath)
                    if (-not $pathMap.ContainsKey($absoluteEvidencePath)) {
                        throw "measurement provenance references an unpackaged file: $evidencePath"
                    }
                    $property.Value.evidence_path = $pathMap[$absoluteEvidencePath]
                    $property.Value.evidence_sha256 = $hashMap[$absoluteEvidencePath]
                }
            }
            $packagedCandidatePath = Join-Path $repositoryPackageDirectory 'report.json'
            Write-JsonAtomically -Path $packagedCandidatePath -Value $candidate
            $candidateHash = Get-Sha256File -Path $packagedCandidatePath
            $repositoryArtifactPath = "perf/baselines/candidates/$suggestedId/report.json"
            $manifestTemplatePath = Join-Path $packageDirectory 'manifest-entry.toml.template'
            $manifestTemplate = @"
[[baseline]]
id = "$suggestedId"
scenario = "$($candidate.scenario)"
scenario_config_sha256 = "$($candidate.scenario_config_sha256)"
runner_fingerprint = "$($candidate.runner_fingerprint)"
artifact_path = "$repositoryArtifactPath"
artifact_sha256 = "$candidateHash"
source_commit = "$($fingerprint.commit)"
approval_ref = "github-pr:OWNER/REPO#NUMBER"
"@.Replace("`r`n", "`n").Replace("`r", "`n")
            [IO.File]::WriteAllText(
                $manifestTemplatePath,
                $manifestTemplate + "`n",
                (New-Object Text.UTF8Encoding($false))
            )
            Add-Artifact -Kind "baseline-package-$($scenario.name)" -Path $packagedCandidatePath -RunIndex 0
            Add-Artifact -Kind "baseline-manifest-template-$($scenario.name)" -Path $manifestTemplatePath -RunIndex 0
            $bootstrapCandidates += [ordered]@{
                phase1_batch_id = $phase1BatchId
                status = 'PENDING_BASELINE'
                suggested_id = $suggestedId
                scenario = $candidate.scenario
                scenario_config_sha256 = $candidate.scenario_config_sha256
                runner_fingerprint = $candidate.runner_fingerprint
                artifact_path = $repositoryArtifactPath
                artifact_sha256 = $candidateHash
                source_commit = $fingerprint.commit
                approval_ref_template = 'github-pr:OWNER/REPO#NUMBER'
                package_directory = $packageDirectory
                manifest_template = $manifestTemplatePath
            }
        }
        $bootstrapPath = Join-Path $batchDirectory 'bootstrap-baseline-candidates.json'
        Write-JsonAtomically -Path $bootstrapPath -Value ([ordered]@{
            phase1_batch_id = $phase1BatchId
            status = 'PENDING_BASELINE'
            candidates = $bootstrapCandidates
        })
        Add-Artifact -Kind 'bootstrap-baseline-candidates' -Path $bootstrapPath -RunIndex 0
        $terminalStatus = 'PENDING_BASELINE'
        $pendingBaseline = $true
    }
    else {
        $manifestPath = Join-Path $repositoryRoot 'perf\baselines\manifest.toml'
        $manifestEntries = Parse-BaselineManifest -Path $manifestPath
        foreach ($scenario in $phase1Candidates) {
            $candidate = Get-Content -LiteralPath $scenario.path -Raw | ConvertFrom-Json
            $baseline = Resolve-ReviewedBaseline -Candidate $candidate -ManifestEntries $manifestEntries
            $compareLog = Join-Path $logDirectory "compare-$($scenario.name).log"
            $approvalEvidencePath = Join-Path $candidateDirectory "approval-$($scenario.name).json"
            Invoke-LoggedCommand -FilePath 'cargo.exe' -Arguments @(
                'run', '--release', '--locked', '-p', 'rshare-perf', '--',
                'compare', '--baseline-id', [string]$baseline.id,
                '--candidate', $scenario.path,
                '--budget', 'perf\budgets\windows-fixed.toml',
                '--evidence-output', $approvalEvidencePath
            ) -LogPath $compareLog
            Add-Artifact -Kind "approval-$($scenario.name)" -Path $approvalEvidencePath -RunIndex 0
            $summary.comparisons += [ordered]@{
                phase1_batch_id = $phase1BatchId
                scenario = $scenario.name
                status = 'PASS'
                baseline_id = [string]$baseline.id
                candidate = $scenario.path
                log = $compareLog
                approval_evidence = $approvalEvidencePath
            }
            Save-Summary
        }
        if (@($summary.comparisons).Count -ne @($phase1Candidates).Count) {
            throw 'not every QUIC, IPC, and UI scenario was compared to a matching reviewed baseline'
        }
        $terminalStatus = 'PASS'
    }
    & (Join-Path $PSScriptRoot 'collect-runner-fingerprint.ps1') -OutputPath $finalFingerprintPath | Out-Host
    $finalFingerprint = Get-Content -LiteralPath $finalFingerprintPath -Raw | ConvertFrom-Json
    Assert-ReproducibilityStable -Initial $initialFingerprint -Current $finalFingerprint
    $summary.reproducibility = $finalFingerprint
    Add-Artifact -Kind 'runner-fingerprint-final' -Path $finalFingerprintPath -RunIndex 0
    $summary.status = $terminalStatus
}
catch {
    $failureMessage = $_.Exception.Message
    $summary.status = 'FAIL'
    $summary.errors += $failureMessage
}
finally {
    foreach ($name in $trackedEnvironment) {
        $saved = $savedEnvironment[$name]
        if ([bool]$saved.existed) {
            [Environment]::SetEnvironmentVariable($name, [string]$saved.value, 'Process')
        }
        else {
            [Environment]::SetEnvironmentVariable($name, $null, 'Process')
        }
    }
    Set-Location $originalLocation
    try {
        if (Test-Path -LiteralPath $batchDirectory -PathType Container) {
            Save-Summary
        }
    }
    catch {
        if ($null -eq $failureMessage) {
            $failureMessage = "failed to write final summary: $($_.Exception.Message)"
        }
    }
}

Write-Host "Phase 1 result: $($summary.status)"
Write-Host "Summary: $summaryPath"
if ($null -ne $failureMessage) {
    Write-Error $failureMessage
    exit 1
}
if ($pendingBaseline) {
    Write-Warning 'PENDING_BASELINE: candidate artifacts require reviewed manifest entries; bootstrap cannot pass the gate.'
    exit 2
}
exit 0
