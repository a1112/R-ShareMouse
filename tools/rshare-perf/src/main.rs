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
    scenario_config_sha256, validate_json_schema, PerfReport, ScenarioContract, VerdictStatus,
    REQUIRED_COUNTERS,
};
use serde::Deserialize;
use std::{fs, path::PathBuf};

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
    #[arg(long, default_value = "perf/baselines/manifest.toml")]
    manifest: PathBuf,
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

    let parameters = serde_json::to_value(&scenario)?;
    let mut report = control::not_run_without_framed_ipc();
    report.scenario = "quic-control-v3".into();
    report.scenario_parameters = match parameters.clone() {
        serde_json::Value::Object(values) => values.into_iter().collect(),
        _ => Default::default(),
    };
    report.scenario_config_sha256 = scenario_config_sha256(&parameters)?;
    report.availability = report::Availability::NotRun {
        reason: "scenario is declared, but no fixed-runner QUIC measurement was executed".into(),
    };
    report.errors =
        vec!["no synthetic daemon IPC or hard-coded localhost daemon port was substituted".into()];
    report.verdict = VerdictStatus::NotRun;

    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&args.output, serde_json::to_vec_pretty(&report)?)?;
    bail!(
        "wrote a fail-closed not_run artifact to {}; fixed-runner execution is required",
        args.output.display()
    )
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

    let manifest: BaselineManifest = toml::from_str(
        &fs::read_to_string(&args.manifest)
            .with_context(|| format!("read manifest {}", args.manifest.display()))?,
    )?;
    let entry = manifest
        .baseline
        .iter()
        .find(|entry| entry.id == args.baseline_id)
        .context("baseline id is absent from the reviewed manifest")?;
    let approval = verify_github_approval(entry, &args.manifest).map_err(anyhow::Error::msg)?;
    compare::resolve_reviewed_entry(&manifest, &args.baseline_id, Some(&approval))
        .map_err(anyhow::Error::msg)?;
    let baseline_report = load_reviewed_baseline(&manifest, &args.baseline_id, &approval)
        .map_err(anyhow::Error::msg)?;
    let candidate_bytes = fs::read(&args.candidate)?;
    let candidate_value: serde_json::Value = serde_json::from_slice(&candidate_bytes)?;
    let schema: serde_json::Value =
        serde_json::from_slice(&fs::read("perf/baselines/schema.json")?)?;
    validate_json_schema(&candidate_value, &schema)?;
    let candidate_report: PerfReport = serde_json::from_value(candidate_value)?;
    let contract = ScenarioContract {
        metrics: baseline_report
            .runs
            .first()
            .map(|run| run.metrics.keys().cloned().collect())
            .unwrap_or_default(),
        counters: REQUIRED_COUNTERS
            .iter()
            .map(|counter| counter.to_string())
            .collect(),
    };
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
}
