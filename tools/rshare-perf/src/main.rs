mod compare;
mod control;
mod dual;
mod quic;
mod report;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use compare::{
    compare, load_reviewed_baseline, verify_github_approval, BaselineManifest, ComparisonPolicy,
    ReportBatch,
};
use quic::{LoadKind, QuicScenario};
use report::{
    parse_and_validate_report, percentile, scenario_config_sha256, Availability,
    BatchArtifactReference, DurationSpec, HardwareFingerprint, MeasurementProvenance,
    MetricSummary, PerfReport, QueueSummary, ScenarioContract, ToolchainFingerprint, VerdictStatus,
    PERF_SCHEMA_VERSION,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

#[derive(Debug, Parser)]
#[command(
    name = "rshare-perf",
    about = "Reproducible R-ShareMouse performance artifacts"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Quic(QuicArgs),
    Compare(CompareArgs),
    Dual(DualArgs),
}

#[derive(Debug, Args)]
struct QuicArgs {
    #[arg(long)]
    rate_hz: Option<u32>,
    #[arg(long)]
    duration_secs: Option<u64>,
    #[arg(long, value_delimiter = ',')]
    load: Vec<LoadKind>,
    #[arg(long)]
    slow_fast_isolation: bool,
    #[arg(long)]
    stall_ms: Option<u64>,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct CompareArgs {
    #[arg(long)]
    baseline_id: String,
    #[arg(long)]
    candidate: PathBuf,
    #[arg(long)]
    budget: PathBuf,
}

#[derive(Debug, Args)]
struct DualArgs {
    #[arg(long)]
    physical_runner_configured: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Budget {
    runs: usize,
    median_regression_percent: f64,
    p95_p99_regression_percent: f64,
    coefficient_of_variation_percent: f64,
    require_reviewed_baseline: bool,
    unstable_complete_batch_retries: u8,
    preserve_all_batches: bool,
    runner_class: String,
    enforced: bool,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Quic(args) => run_quic(args),
        Command::Compare(args) => run_compare(args),
        Command::Dual(args) => run_dual(args),
    }
}

fn run_dual(args: DualArgs) -> Result<()> {
    let status = dual::require_physical_runner(args.physical_runner_configured);
    println!("{}", serde_json::to_string_pretty(&status)?);
    bail!("dual-machine measurements never pass without a separately executed physical run")
}

fn run_quic(args: QuicArgs) -> Result<()> {
    run_quic_with_duration(args, None)
}

fn run_quic_with_duration(args: QuicArgs, effective_duration: Option<Duration>) -> Result<()> {
    let scenario = if args.slow_fast_isolation {
        QuicScenario::SlowFastPeerIsolation
    } else if let Some(stall_ms) = args.stall_ms {
        QuicScenario::StallRecovery { stall_ms }
    } else {
        QuicScenario::Rate {
            rate_hz: args.rate_hz.context("--rate-hz is required")?,
            duration_secs: args.duration_secs.context("--duration-secs is required")?,
            load: args.load,
        }
    };
    scenario.validate()?;
    if !quic::scenario_matrix().contains(&scenario) {
        bail!("scenario is not part of the predeclared QUIC matrix");
    }

    let mut parameters = serde_json::to_value(&scenario)?;
    if let Some(duration) = effective_duration {
        let duration_ms = u64::try_from(duration.as_millis())
            .context("effective QUIC measurement duration exceeds u64 milliseconds")?;
        parameters
            .as_object_mut()
            .context("serialized QUIC scenario is not an object")?
            .insert("measurement_duration_ms".into(), duration_ms.into());
    }
    let config_hash = scenario_config_sha256(&parameters)?;
    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)?;
    }

    let queue_samples = Arc::new(Mutex::new(
        Vec::<(String, BTreeMap<String, QueueSummary>)>::new(),
    ));
    let runtime = tokio::runtime::Runtime::new()?;
    let mut orchestration = runtime.block_on(quic::orchestrate_five_run_batches(
        &config_hash,
        |batch_id, run_index| {
            let scenario = scenario.clone();
            let queue_samples = Arc::clone(&queue_samples);
            let batch_id = batch_id.to_string();
            let mut options = quic::LoopbackRunOptions::measured(batch_id.clone(), run_index);
            options.effective_duration = effective_duration;
            async move {
                let measurement = quic::run_loopback_once(&scenario, options).await?;
                queue_samples
                    .lock()
                    .expect("queue sample mutex poisoned")
                    .push((batch_id, measurement.queues));
                Ok(measurement.run)
            }
        },
    ))?;

    let fingerprints = collect_fingerprints()?;
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../perf/baselines/schema.json"))?;
    let stem = args
        .output
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("quic");
    let output_directory = args.output.parent().unwrap_or_else(|| Path::new("."));
    let mut sidecar_hashes = Vec::with_capacity(orchestration.batches.len());
    let mut batch_reports = Vec::with_capacity(orchestration.batches.len());
    for (index, batch) in orchestration.batches.iter_mut().enumerate() {
        let sidecar_path = output_directory.join(format!("{stem}.batch-{}.json", index + 1));
        batch.artifact_path = sidecar_path.display().to_string();
        let batch_queue_samples: Vec<_> = queue_samples
            .lock()
            .expect("queue sample mutex poisoned")
            .iter()
            .filter(|(batch_id, _)| batch_id == &batch.batch_id)
            .map(|(_, queues)| queues.clone())
            .collect();
        let errors = (batch.verdict == VerdictStatus::Unstable)
            .then(|| vec!["complete batch exceeded the coefficient-of-variation limit".into()])
            .unwrap_or_default();
        let report = build_quic_report(
            &parameters,
            &config_hash,
            batch.runs.clone(),
            summarize_queues(&batch_queue_samples),
            batch.verdict,
            errors,
            &fingerprints,
            None,
        )?;
        let (report, bytes) = validate_complete_report(report, &schema)?;
        atomic_write(&sidecar_path, &bytes)?;
        sidecar_hashes.push((
            batch.artifact_path.clone(),
            format!("{:x}", Sha256::digest(&bytes)),
        ));
        batch_reports.push(report);
    }

    let selected_index = orchestration
        .selected_batch
        .unwrap_or_else(|| orchestration.batches.len().saturating_sub(1));
    let mut report = batch_reports
        .get(selected_index)
        .context("QUIC orchestrator did not produce a batch")?
        .clone();
    let evidence = sidecar_hashes.get(selected_index).cloned();
    for provenance in report.measurement_provenance.values_mut() {
        provenance.evidence_path = evidence.as_ref().map(|(path, _)| path.clone());
        provenance.evidence_sha256 = evidence.as_ref().map(|(_, hash)| hash.clone());
    }
    if let Some(reason) = &orchestration.infrastructure_failure {
        report.errors.push(reason.clone());
    }
    report.batch_artifacts = orchestration
        .batches
        .iter()
        .zip(&sidecar_hashes)
        .map(|(batch, (path, sha256))| BatchArtifactReference {
            batch_id: batch.batch_id.clone(),
            path: path.clone(),
            sha256: sha256.clone(),
            verdict: batch.verdict,
        })
        .collect();

    write_primary_report(&args.output, report, &schema)?;
    if let Some(reason) = orchestration.infrastructure_failure {
        bail!(
            "wrote an available but unstable artifact to {}: {reason}",
            args.output.display()
        );
    }
    Ok(())
}

fn build_quic_report(
    parameters: &serde_json::Value,
    config_hash: &str,
    mut runs: Vec<report::PerfRun>,
    queues: BTreeMap<String, QueueSummary>,
    verdict: VerdictStatus,
    errors: Vec<String>,
    fingerprints: &Fingerprints,
    evidence: Option<&(String, String)>,
) -> Result<PerfReport> {
    for run in &mut runs {
        run.schema_valid = true;
    }
    let metrics = summarize_metrics(&runs);
    let measurement_provenance = metrics
        .keys()
        .map(|name| {
            (
                name.clone(),
                MeasurementProvenance {
                    method: "real_quic_loopback_send_to_receive_monotonic".into(),
                    uncertainty_us: Some(1),
                    evidence_path: evidence.map(|(path, _)| path.clone()),
                    evidence_sha256: evidence.map(|(_, hash)| hash.clone()),
                    estimate_only: false,
                },
            )
        })
        .collect();
    Ok(PerfReport {
        schema_version: PERF_SCHEMA_VERSION,
        scenario: "quic-control-v3".into(),
        scenario_parameters: serde_json::from_value(parameters.clone())?,
        scenario_config_sha256: config_hash.into(),
        random_seed: 0,
        commit: fingerprints.commit.clone(),
        dirty: fingerprints.dirty,
        binary_sha256: fingerprints.binary_sha256.clone(),
        cargo_lock_sha256: fingerprints.cargo_lock_sha256.clone(),
        build_profile: if cfg!(debug_assertions) {
            "debug".into()
        } else {
            "release".into()
        },
        cargo_features: enabled_cargo_features(),
        rustflags: std::env::var("RUSTFLAGS").unwrap_or_default(),
        runner_id: fingerprints.runner_id.clone(),
        runner_fingerprint: fingerprints.runner_fingerprint.clone(),
        availability: Availability::Available,
        toolchain: fingerprints.toolchain.clone(),
        hardware: fingerprints.hardware.clone(),
        warmup: DurationSpec { millis: 0 },
        batch_artifacts: vec![],
        runs,
        metrics,
        queues,
        errors,
        rss: None,
        measurement_provenance,
        verdict,
        local_schema_validated: false,
    })
}

fn validate_complete_report(
    report: PerfReport,
    schema: &serde_json::Value,
) -> Result<(PerfReport, Vec<u8>)> {
    let bytes = serde_json::to_vec_pretty(&report)?;
    let report = parse_and_validate_report(&bytes, &schema)?;
    let contract = ScenarioContract::for_report(&report)?;
    report.validate_complete(&contract)?;
    let bytes = serde_json::to_vec_pretty(&report)?;
    Ok((report, bytes))
}

fn write_primary_report(
    path: &Path,
    report: PerfReport,
    schema: &serde_json::Value,
) -> Result<PerfReport> {
    let (report, bytes) = validate_complete_report(report, schema)?;
    atomic_write(path, &bytes)?;
    Ok(report)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_with_pre_rename(path, bytes, || Ok(()))
}

fn atomic_write_with_pre_rename<F>(path: &Path, bytes: &[u8], before_rename: F) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("artifact");
    let temp_path = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        before_rename()?;
        replace_file(&temp_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source: Vec<u16> = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let succeeded = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    fs::rename(source, destination)?;
    if let Some(parent) = destination.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[derive(Clone)]
struct Fingerprints {
    commit: String,
    dirty: bool,
    binary_sha256: BTreeMap<String, String>,
    cargo_lock_sha256: String,
    runner_id: String,
    runner_fingerprint: String,
    toolchain: ToolchainFingerprint,
    hardware: HardwareFingerprint,
}

fn collect_fingerprints() -> Result<Fingerprints> {
    let repository_root = PathBuf::from(git_output(["rev-parse", "--show-toplevel"])?);
    let commit = git_output(["rev-parse", "HEAD"])?;
    let dirty = repository_is_dirty(&repository_root)?;
    let binary = std::env::current_exe()?;
    let binary_sha256 = BTreeMap::from([("rshare-perf".into(), digest_file(&binary)?)]);
    let cargo_lock_sha256 = digest_file(&repository_root.join("Cargo.lock"))?;
    let rustc = process_output("rustc", &["--version"])?;
    let cargo = process_output("cargo", &["--version"])?;
    let verbose_rustc = process_output("rustc", &["-vV"])?;
    let target = verbose_rustc
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .unwrap_or(std::env::consts::ARCH)
        .to_string();
    let toolchain = ToolchainFingerprint {
        rustc,
        cargo,
        target,
    };
    let hardware = HardwareFingerprint {
        os: std::env::consts::OS.into(),
        cpu: std::env::var("PROCESSOR_IDENTIFIER")
            .or_else(|_| std::env::var("HOSTTYPE"))
            .unwrap_or_else(|_| "unknown".into()),
        logical_cores: std::thread::available_parallelism()
            .map(|value| value.get() as u32)
            .unwrap_or(0),
        memory_bytes: physical_memory_bytes()?,
    };
    let runner_id = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH));
    let runner_fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(&runner_id, &toolchain, &hardware))?)
    );
    Ok(Fingerprints {
        commit,
        dirty,
        binary_sha256,
        cargo_lock_sha256,
        runner_id,
        runner_fingerprint,
        toolchain,
        hardware,
    })
}

fn repository_is_dirty(repository_root: &Path) -> Result<bool> {
    let output = ProcessCommand::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repository_root)
        .output()?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(!output.stdout.is_empty())
}

fn enabled_cargo_features() -> Vec<String> {
    // rshare-perf currently declares no opt-in Cargo features. Keep this function
    // adjacent to report construction so any future feature declaration must also
    // be reflected in the reproducibility fingerprint.
    Vec::new()
}

#[cfg(windows)]
fn physical_memory_bytes() -> Result<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = u32::try_from(std::mem::size_of::<MEMORYSTATUSEX>())
        .context("MEMORYSTATUSEX size does not fit in DWORD")?;
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(status.ullTotalPhys)
}

#[cfg(target_os = "linux")]
fn physical_memory_bytes() -> Result<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo")?;
    let kib = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .and_then(|value| value.split_whitespace().next())
        .context("/proc/meminfo does not contain MemTotal")?
        .parse::<u64>()?;
    kib.checked_mul(1024)
        .context("physical memory size overflowed u64")
}

#[cfg(target_os = "macos")]
fn physical_memory_bytes() -> Result<u64> {
    process_output("sysctl", &["-n", "hw.memsize"])?
        .parse::<u64>()
        .context("sysctl hw.memsize did not return a byte count")
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn physical_memory_bytes() -> Result<u64> {
    bail!("physical memory fingerprint is unsupported on this operating system")
}

fn process_output(program: &str, args: &[&str]) -> Result<String> {
    let output = ProcessCommand::new(program).args(args).output()?;
    if !output.status.success() {
        bail!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn digest_file(path: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}

fn summarize_metrics(runs: &[report::PerfRun]) -> BTreeMap<String, MetricSummary> {
    let names = runs
        .iter()
        .flat_map(|run| run.metrics.keys().cloned())
        .collect::<BTreeSet<_>>();
    names
        .into_iter()
        .map(|name| {
            let mut values: Vec<_> = runs
                .iter()
                .filter_map(|run| run.metrics.get(&name).copied())
                .collect();
            values.sort_by(f64::total_cmp);
            let mean = values.iter().sum::<f64>() / values.len() as f64;
            let variance = if values.len() > 1 {
                values
                    .iter()
                    .map(|value| (value - mean).powi(2))
                    .sum::<f64>()
                    / (values.len() - 1) as f64
            } else {
                0.0
            };
            let summary = MetricSummary {
                unit: "us".into(),
                samples: values.len() as u64,
                median: percentile(&values, 0.50),
                p95: percentile(&values, 0.95),
                p99: percentile(&values, 0.99),
                max: values.last().copied().unwrap_or(0.0),
                coefficient_of_variation: if mean == 0.0 {
                    0.0
                } else {
                    variance.sqrt() / mean.abs()
                },
            };
            (name, summary)
        })
        .collect()
}

fn summarize_queues(samples: &[BTreeMap<String, QueueSummary>]) -> BTreeMap<String, QueueSummary> {
    let mut result = BTreeMap::new();
    for queues in samples {
        for (name, queue) in queues {
            let aggregate = result.entry(name.clone()).or_insert(QueueSummary {
                capacity: queue.capacity,
                high_watermark: 0,
                overwrites: 0,
                overflows: 0,
            });
            aggregate.capacity = aggregate.capacity.max(queue.capacity);
            aggregate.high_watermark = aggregate.high_watermark.max(queue.high_watermark);
            aggregate.overwrites += queue.overwrites;
            aggregate.overflows += queue.overflows;
        }
    }
    result
}

fn run_compare(args: CompareArgs) -> Result<()> {
    let budget: Budget = toml::from_str(
        &fs::read_to_string(&args.budget)
            .with_context(|| format!("read budget {}", args.budget.display()))?,
    )?;
    let policy = ComparisonPolicy {
        runs: budget.runs,
        median_regression_limit: budget.median_regression_percent / 100.0,
        tail_regression_limit: budget.p95_p99_regression_percent / 100.0,
        cv_limit: budget.coefficient_of_variation_percent / 100.0,
        require_reviewed_baseline: budget.require_reviewed_baseline,
    };
    if policy != ComparisonPolicy::strict() {
        bail!("strict comparison budget must retain the reviewed 5/10/15/10 policy");
    }
    if budget.runner_class.trim().is_empty() || !budget.enforced {
        bail!("strict comparison requires an enforced named runner class");
    }
    if budget.unstable_complete_batch_retries != 1 || !budget.preserve_all_batches {
        bail!("unstable policy must retry one whole batch and preserve every batch");
    }

    let repository_root = git_output(["rev-parse", "--show-toplevel"])?;
    let repository_root = PathBuf::from(repository_root);
    let manifest_path = repository_root.join("perf/baselines/manifest.toml");
    let manifest: BaselineManifest = toml::from_str(
        &fs::read_to_string(&manifest_path)
            .with_context(|| format!("read protected manifest {}", manifest_path.display()))?,
    )?;
    let expected_repository =
        canonical_github_repository(&git_output(["remote", "get-url", "origin"])?)
            .context("origin is not a canonical GitHub repository")?;
    let entry = manifest
        .baseline
        .iter()
        .find(|entry| entry.id == args.baseline_id)
        .context("baseline id is absent from the reviewed manifest")?;
    let approval = verify_github_approval(entry, &manifest_path, &expected_repository)
        .map_err(anyhow::Error::msg)?;
    compare::resolve_reviewed_entry(&manifest, &args.baseline_id, Some(&approval))
        .map_err(anyhow::Error::msg)?;
    let baseline_report =
        load_reviewed_baseline(&manifest, &args.baseline_id, &approval, &repository_root)
            .map_err(anyhow::Error::msg)?;
    let candidate_bytes = fs::read(&args.candidate)?;
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../perf/baselines/schema.json"))?;
    let candidate_report = parse_and_validate_report(&candidate_bytes, &schema)?;
    let contract = ScenarioContract::for_report(&baseline_report)?;
    baseline_report.validate_complete(&contract)?;
    candidate_report.validate_complete(&contract)?;

    let mut baseline = batch_from_artifact(baseline_report);
    baseline.reviewed_manifest_id = Some(args.baseline_id);
    let candidate = batch_from_artifact(candidate_report);
    let verdict = compare(&baseline, &candidate, policy).map_err(anyhow::Error::msg)?;
    println!("{}", serde_json::to_string_pretty(&verdict)?);
    match verdict.status {
        VerdictStatus::Pass => Ok(()),
        other => bail!("performance gate did not pass: {other:?}"),
    }
}

fn git_output<const N: usize>(args: [&str; N]) -> Result<String> {
    let output = ProcessCommand::new("git").args(args).output()?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn canonical_github_repository(remote: &str) -> Option<String> {
    let trimmed = remote.trim().trim_end_matches(".git");
    let path = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("ssh://git@github.com/"))
        .or_else(|| trimmed.strip_prefix("git@github.com:"))?;
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let repository = parts.next()?;
    (parts.next().is_none() && !owner.is_empty() && !repository.is_empty())
        .then(|| format!("{owner}/{repository}"))
}

fn batch_from_artifact(report: PerfReport) -> ReportBatch {
    let reports = report
        .runs
        .iter()
        .cloned()
        .map(|run| {
            let mut single = report.clone();
            single.runs = vec![run];
            single
        })
        .collect();
    ReportBatch::from_reports(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn quic_command_accepts_the_documented_control_load() {
        let cli = Cli::try_parse_from([
            "rshare-perf",
            "quic",
            "--rate-hz",
            "1000",
            "--duration-secs",
            "60",
            "--load",
            "diagnostics,status,audio,bulk",
            "--output",
            "quic.json",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::Quic(_)));
    }

    #[test]
    fn compare_command_requires_manifest_baseline_id_not_direct_path() {
        assert!(Cli::try_parse_from([
            "rshare-perf",
            "compare",
            "--baseline",
            "baseline.json",
            "--candidate",
            "candidate.json",
            "--budget",
            "budget.toml",
        ])
        .is_err());
    }

    #[test]
    fn quic_cli_executes_five_real_runs_and_writes_batch_sidecars() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("rshare-perf-cli-{}-{nonce}", std::process::id()));
        let output = directory.join("quic.json");
        let args = QuicArgs {
            rate_hz: Some(125),
            duration_secs: Some(10),
            load: vec![],
            slow_fast_isolation: false,
            stall_ms: None,
            output: output.clone(),
        };

        run_quic_with_duration(args, Some(Duration::from_secs(1)))
            .expect("CLI must complete its five-run QUIC measurement");

        let bytes = fs::read(&output).expect("CLI must write its primary artifact");
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../perf/baselines/schema.json")).unwrap();
        let report = parse_and_validate_report(&bytes, &schema).unwrap();
        assert!(matches!(report.availability, Availability::Available));
        assert_eq!(report.runs.len(), 5);
        assert!(report.runs.iter().all(|run| run.process_exit_success));
        assert_eq!(
            report.scenario_parameters["measurement_duration_ms"],
            serde_json::json!(1_000)
        );
        assert_eq!(
            report.scenario_config_sha256,
            scenario_config_sha256(&serde_json::to_value(&report.scenario_parameters).unwrap())
                .unwrap()
        );

        let sidecars: Vec<_> = fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("quic.batch-"))
            })
            .collect();
        assert!(!sidecars.is_empty());
        assert_eq!(report.batch_artifacts.len(), sidecars.len());
        for reference in &report.batch_artifacts {
            let bytes = fs::read(&reference.path).unwrap();
            assert_eq!(format!("{:x}", Sha256::digest(&bytes)), reference.sha256);
        }
        for sidecar in sidecars {
            let bytes = fs::read(sidecar).unwrap();
            let batch = parse_and_validate_report(&bytes, &schema).unwrap();
            let contract = ScenarioContract::for_report(&batch).unwrap();
            batch.validate_complete(&contract).unwrap();
            assert_eq!(batch.runs.len(), 5);
            let unique_run_ids: std::collections::HashSet<_> =
                batch.runs.iter().map(|run| &run.run_id).collect();
            assert_eq!(unique_run_ids.len(), 5);
            assert!(batch.runs.iter().all(|run| run.schema_valid));
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn hardware_fingerprint_reports_real_physical_memory() {
        assert!(physical_memory_bytes().unwrap() > 0);
    }

    #[test]
    fn dirty_fingerprint_includes_untracked_files() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("rshare-perf-dirty-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        assert!(ProcessCommand::new("git")
            .args(["init", "--quiet"])
            .current_dir(&directory)
            .status()
            .unwrap()
            .success());
        fs::write(directory.join("untracked.txt"), b"reproducibility input").unwrap();

        assert!(repository_is_dirty(&directory).unwrap());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn primary_artifact_gate_rejects_missing_metric_or_binary_role_without_writing() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../perf/baselines/schema.json")).unwrap();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("rshare-perf-gate-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();

        let mut missing_metric = complete_report_fixture();
        missing_metric.runs[2].metrics.remove("p95_us");
        let metric_path = directory.join("missing-metric.json");
        fs::write(&metric_path, b"old-metric-artifact").unwrap();
        assert!(write_primary_report(&metric_path, missing_metric, &schema).is_err());
        assert_eq!(fs::read(&metric_path).unwrap(), b"old-metric-artifact");

        let mut missing_role = complete_report_fixture();
        missing_role.binary_sha256.clear();
        missing_role
            .binary_sha256
            .insert("unrelated-role".into(), format!("{:064x}", 7));
        let role_path = directory.join("missing-role.json");
        fs::write(&role_path, b"old-role-artifact").unwrap();
        assert!(write_primary_report(&role_path, missing_role, &schema).is_err());
        assert_eq!(fs::read(&role_path).unwrap(), b"old-role-artifact");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn atomic_writer_failure_before_rename_preserves_existing_artifact() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("rshare-perf-atomic-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("artifact.json");
        fs::write(&path, b"reviewed-old-artifact").unwrap();

        let result = atomic_write_with_pre_rename(&path, b"partial-new-artifact", || {
            anyhow::bail!("simulated interruption before rename")
        });

        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"reviewed-old-artifact");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    fn complete_report_fixture() -> PerfReport {
        let runs = (0..5)
            .map(|index| report::PerfRun {
                run_id: format!("run-{index}"),
                batch_id: "batch-a".into(),
                process_exit_success: true,
                schema_valid: true,
                scenario_config_sha256: "config".into(),
                metrics: BTreeMap::from([
                    ("median_us".into(), 10.0),
                    ("p95_us".into(), 12.0),
                    ("p99_us".into(), 14.0),
                ]),
                counters: report::REQUIRED_COUNTERS
                    .into_iter()
                    .map(|counter| (counter.into(), 0))
                    .collect(),
                errors: vec![],
            })
            .collect();
        PerfReport::test_fixture("batch-a", "runner-a", runs)
    }
}
