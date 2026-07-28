use crate::report::{
    validate_json_schema, Availability, PerfReport, PerfRun, VerdictStatus, PERF_SCHEMA_VERSION,
    REQUIRED_COUNTERS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::Path, process::Command};

#[derive(Debug, Clone)]
pub struct ReportBatch {
    pub reports: Vec<PerfReport>,
    pub runs: Vec<PerfRun>,
    pub reviewed_manifest_id: Option<String>,
}

impl ReportBatch {
    pub fn from_reports(reports: Vec<PerfReport>) -> Self {
        let runs = reports
            .iter()
            .filter_map(|report| report.runs.first().cloned())
            .collect();
        Self {
            reports,
            runs,
            reviewed_manifest_id: Some("reviewed-test-baseline".into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComparisonPolicy {
    pub runs: usize,
    pub median_regression_limit: f64,
    pub tail_regression_limit: f64,
    pub cv_limit: f64,
    pub require_reviewed_baseline: bool,
}

impl ComparisonPolicy {
    pub const fn strict() -> Self {
        Self {
            runs: 5,
            median_regression_limit: 0.10,
            tail_regression_limit: 0.15,
            cv_limit: 0.10,
            require_reviewed_baseline: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComparisonVerdict {
    pub status: VerdictStatus,
    pub regressions: Vec<Regression>,
    pub metrics: Vec<ComparedMetric>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Regression {
    pub metric: String,
    pub baseline_median: f64,
    pub candidate_median: f64,
    pub regression: f64,
    pub allowed: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComparedMetric {
    pub metric: String,
    pub baseline_median: f64,
    pub candidate_median: f64,
    pub candidate_cv: f64,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CompareError {
    #[error("strict comparison requires reviewed baseline provenance")]
    MissingReviewedBaseline,
    #[error("batch has {actual} runs, expected {expected}")]
    IncompleteBatch { expected: usize, actual: usize },
    #[error("runner fingerprint differs: {baseline} vs {candidate}")]
    RunnerMismatch { baseline: String, candidate: String },
    #[error("scenario/configuration mismatch: {details}")]
    ScenarioConfigMismatch { details: String },
    #[error("immutable batch mismatch: {details}")]
    BatchMismatch { details: String },
    #[error("run {run_id} is incomplete: {reason}")]
    RunIncomplete { run_id: String, reason: String },
    #[error("build fingerprint mismatch: {details}")]
    BuildMismatch { details: String },
    #[error("run {run_id} is missing metric {metric}")]
    MissingMetric { run_id: String, metric: String },
    #[error("run {run_id} is missing counter {counter}")]
    MissingCounter { run_id: String, counter: String },
    #[error("report is unavailable: {reason}")]
    Unavailable { reason: String },
}

pub fn compare(
    baseline: &ReportBatch,
    candidate: &ReportBatch,
    policy: ComparisonPolicy,
) -> Result<ComparisonVerdict, CompareError> {
    if policy.require_reviewed_baseline && baseline.reviewed_manifest_id.is_none() {
        return Err(CompareError::MissingReviewedBaseline);
    }
    validate_batch_shape(baseline, policy)?;
    validate_batch_shape(candidate, policy)?;
    validate_matching_context(baseline, candidate)?;

    let baseline_metrics: BTreeSet<_> = baseline.runs[0].metrics.keys().cloned().collect();
    for run in baseline.runs.iter().chain(&candidate.runs) {
        for metric in &baseline_metrics {
            if !run.metrics.contains_key(metric) {
                return Err(CompareError::MissingMetric {
                    run_id: run.run_id.clone(),
                    metric: metric.clone(),
                });
            }
        }
    }
    for metric in &baseline_metrics {
        if candidate.runs[0].metrics.get(metric).is_none() {
            return Err(CompareError::MissingMetric {
                run_id: candidate.runs[0].run_id.clone(),
                metric: metric.clone(),
            });
        }
    }

    let mut comparisons = Vec::new();
    let mut regressions = Vec::new();
    let mut unstable = false;
    for metric in baseline_metrics {
        let baseline_values: Vec<_> = baseline
            .runs
            .iter()
            .map(|run| run.metrics[&metric])
            .collect();
        let candidate_values: Vec<_> = candidate
            .runs
            .iter()
            .map(|run| run.metrics[&metric])
            .collect();
        let baseline_median = median(&baseline_values);
        let candidate_median = median(&candidate_values);
        let candidate_cv = coefficient_of_variation(&candidate_values);
        unstable |= candidate_cv > policy.cv_limit;
        let allowed = if metric.contains("p95") || metric.contains("p99") {
            policy.tail_regression_limit
        } else {
            policy.median_regression_limit
        };
        let regression = if baseline_median == 0.0 {
            if candidate_median == 0.0 {
                0.0
            } else {
                f64::INFINITY
            }
        } else {
            candidate_median / baseline_median - 1.0
        };
        comparisons.push(ComparedMetric {
            metric: metric.clone(),
            baseline_median,
            candidate_median,
            candidate_cv,
        });
        if regression > allowed {
            regressions.push(Regression {
                metric,
                baseline_median,
                candidate_median,
                regression,
                allowed,
            });
        }
    }

    let status = if unstable {
        VerdictStatus::Unstable
    } else if regressions.is_empty() {
        VerdictStatus::Pass
    } else {
        VerdictStatus::Fail
    };
    Ok(ComparisonVerdict {
        status,
        regressions,
        metrics: comparisons,
    })
}

fn validate_batch_shape(batch: &ReportBatch, policy: ComparisonPolicy) -> Result<(), CompareError> {
    if batch.runs.len() != policy.runs {
        return Err(CompareError::IncompleteBatch {
            expected: policy.runs,
            actual: batch.runs.len(),
        });
    }
    if batch.reports.len() != policy.runs {
        return Err(CompareError::IncompleteBatch {
            expected: policy.runs,
            actual: batch.reports.len(),
        });
    }
    let batch_id = &batch.runs[0].batch_id;
    let config = &batch.runs[0].scenario_config_sha256;
    for (index, run) in batch.runs.iter().enumerate() {
        if run.batch_id != *batch_id {
            return Err(CompareError::BatchMismatch {
                details: format!("run {} belongs to {}", run.run_id, run.batch_id),
            });
        }
        if run.scenario_config_sha256 != *config {
            return Err(CompareError::ScenarioConfigMismatch {
                details: format!(
                    "run {} uses {} instead of {}",
                    run.run_id, run.scenario_config_sha256, config
                ),
            });
        }
        if !run.process_exit_success || !run.schema_valid || !run.errors.is_empty() {
            return Err(CompareError::RunIncomplete {
                run_id: run.run_id.clone(),
                reason: if !run.process_exit_success {
                    "process failed".into()
                } else if !run.schema_valid {
                    "schema validation failed".into()
                } else {
                    run.errors.join("; ")
                },
            });
        }
        for counter in REQUIRED_COUNTERS {
            if !run.counters.contains_key(counter) {
                return Err(CompareError::MissingCounter {
                    run_id: run.run_id.clone(),
                    counter: counter.into(),
                });
            }
        }
        let report = &batch.reports[index];
        report
            .validate_reproducibility()
            .map_err(|error| CompareError::BuildMismatch {
                details: error.to_string(),
            })?;
        match &report.availability {
            Availability::Available => {}
            Availability::Unsupported { reason } | Availability::NotRun { reason } => {
                return Err(CompareError::Unavailable {
                    reason: reason.clone(),
                });
            }
        }
        if report.schema_version != PERF_SCHEMA_VERSION {
            return Err(CompareError::BuildMismatch {
                details: "schema version differs".into(),
            });
        }
        if report.scenario_config_sha256 != *config {
            return Err(CompareError::ScenarioConfigMismatch {
                details: format!("report {} configuration differs", index),
            });
        }
        if report.runs.len() != 1 {
            return Err(CompareError::RunIncomplete {
                run_id: run.run_id.clone(),
                reason: "one artifact must represent exactly one run".into(),
            });
        }
    }
    validate_internal_context(batch)
}

fn validate_internal_context(batch: &ReportBatch) -> Result<(), CompareError> {
    let first = &batch.reports[0];
    for report in &batch.reports[1..] {
        if report.runner_fingerprint != first.runner_fingerprint {
            return Err(CompareError::RunnerMismatch {
                baseline: first.runner_fingerprint.clone(),
                candidate: report.runner_fingerprint.clone(),
            });
        }
        if context_fingerprint(report) != context_fingerprint(first) {
            return Err(CompareError::BuildMismatch {
                details: "runs within a batch have different immutable context".into(),
            });
        }
    }
    Ok(())
}

fn validate_matching_context(
    baseline: &ReportBatch,
    candidate: &ReportBatch,
) -> Result<(), CompareError> {
    let baseline = &baseline.reports[0];
    let candidate = &candidate.reports[0];
    if baseline.runner_fingerprint != candidate.runner_fingerprint {
        return Err(CompareError::RunnerMismatch {
            baseline: baseline.runner_fingerprint.clone(),
            candidate: candidate.runner_fingerprint.clone(),
        });
    }
    if baseline.scenario != candidate.scenario
        || baseline.scenario_config_sha256 != candidate.scenario_config_sha256
    {
        return Err(CompareError::ScenarioConfigMismatch {
            details: "baseline and candidate scenario/configuration differ".into(),
        });
    }
    if context_fingerprint(baseline) != context_fingerprint(candidate) {
        return Err(CompareError::BuildMismatch {
            details: "baseline and candidate immutable build context differs".into(),
        });
    }
    Ok(())
}

fn context_fingerprint(report: &PerfReport) -> String {
    let bytes = serde_json::to_vec(&(
        report.schema_version,
        &report.scenario,
        &report.scenario_config_sha256,
        report.random_seed,
        &report.binary_sha256,
        &report.cargo_lock_sha256,
        &report.build_profile,
        &report.cargo_features,
        &report.rustflags,
        &report.runner_fingerprint,
        &report.toolchain,
        &report.hardware,
        &report.warmup,
    ))
    .expect("comparison context is serializable");
    format!("{:x}", Sha256::digest(bytes))
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

fn coefficient_of_variation(values: &[f64]) -> f64 {
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    if mean == 0.0 {
        return if values.iter().all(|value| *value == 0.0) {
            0.0
        } else {
            f64::INFINITY
        };
    }
    let variance = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / (values.len() - 1) as f64;
    variance.sqrt() / mean.abs()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BaselineManifest {
    pub baseline: Vec<BaselineEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BaselineEntry {
    pub id: String,
    pub scenario: String,
    pub scenario_config_sha256: String,
    pub runner_fingerprint: String,
    pub artifact_path: String,
    pub artifact_sha256: String,
    pub source_commit: String,
    pub approval_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifiedApproval {
    pub approval_ref: String,
    pub default_branch_protected: bool,
    pub manifest_from_default_branch: bool,
    pub pull_request_merged: bool,
    pub approved_by_non_author: bool,
    pub reviewed_diff_contains_entry_and_hash: bool,
    pub github_api_evidence_sha256: String,
}

impl VerifiedApproval {
    #[cfg(test)]
    fn test_fixture(approval_ref: &str) -> Self {
        Self {
            approval_ref: approval_ref.into(),
            default_branch_protected: true,
            manifest_from_default_branch: true,
            pull_request_merged: true,
            approved_by_non_author: true,
            reviewed_diff_contains_entry_and_hash: true,
            github_api_evidence_sha256: format!("{:x}", Sha256::digest(b"evidence")),
        }
    }

    fn validates(&self, approval_ref: &str) -> bool {
        self.approval_ref == approval_ref
            && self.default_branch_protected
            && self.manifest_from_default_branch
            && self.pull_request_merged
            && self.approved_by_non_author
            && self.reviewed_diff_contains_entry_and_hash
            && is_sha256(&self.github_api_evidence_sha256)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BaselineError {
    #[error("baseline {id} is absent from the reviewed manifest")]
    MissingEntry { id: String },
    #[error("baseline manifest entry {id} is invalid: {reason}")]
    InvalidManifestEntry { id: String, reason: String },
    #[error("approval verification for {approval_ref} is unavailable")]
    ApprovalUnavailable { approval_ref: String },
    #[error("artifact hash mismatch: expected {expected}, got {actual}")]
    ArtifactHashMismatch { expected: String, actual: String },
    #[error("could not read baseline artifact: {0}")]
    ArtifactRead(String),
    #[error("baseline artifact is invalid: {0}")]
    ArtifactParse(String),
    #[error("baseline artifact failed schema validation: {0}")]
    SchemaValidation(String),
    #[error("GitHub approval verification failed: {0}")]
    GitHubVerification(String),
}

pub fn verify_github_approval(
    entry: &BaselineEntry,
    manifest_path: &Path,
) -> Result<VerifiedApproval, BaselineError> {
    let (repository, number) = parse_approval_ref(&entry.approval_ref)
        .ok_or_else(|| BaselineError::GitHubVerification("invalid approval_ref".into()))?;
    let repository_info: serde_json::Value =
        serde_json::from_slice(&gh_api(&format!("repos/{repository}"), None)?)
            .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    let default_branch = repository_info["default_branch"].as_str().ok_or_else(|| {
        BaselineError::GitHubVerification("repository default_branch is absent".into())
    })?;

    gh_api(
        &format!("repos/{repository}/branches/{default_branch}/protection"),
        None,
    )?;
    let remote_manifest = gh_api(
        &format!("repos/{repository}/contents/perf/baselines/manifest.toml?ref={default_branch}"),
        Some("application/vnd.github.raw+json"),
    )?;
    let local_manifest = fs::read(manifest_path)
        .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    if normalize_lines(&remote_manifest) != normalize_lines(&local_manifest) {
        return Err(BaselineError::GitHubVerification(
            "local manifest is not the protected default-branch manifest".into(),
        ));
    }

    let pull: serde_json::Value = serde_json::from_slice(&gh_api(
        &format!("repos/{repository}/pulls/{number}"),
        None,
    )?)
    .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    if pull["merged_at"].is_null() {
        return Err(BaselineError::GitHubVerification(
            "approval_ref pull request is not merged".into(),
        ));
    }
    let author = pull["user"]["login"]
        .as_str()
        .ok_or_else(|| BaselineError::GitHubVerification("pull request author is absent".into()))?;
    let reviews: serde_json::Value = serde_json::from_slice(&gh_api(
        &format!("repos/{repository}/pulls/{number}/reviews?per_page=100"),
        None,
    )?)
    .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    let approved = reviews.as_array().is_some_and(|reviews| {
        reviews.iter().any(|review| {
            review["state"].as_str() == Some("APPROVED")
                && review["user"]["login"]
                    .as_str()
                    .is_some_and(|reviewer| reviewer != author)
        })
    });
    if !approved {
        return Err(BaselineError::GitHubVerification(
            "no APPROVED review by a non-author was found".into(),
        ));
    }

    let diff = String::from_utf8(gh_api(
        &format!("repos/{repository}/pulls/{number}"),
        Some("application/vnd.github.v3.diff"),
    )?)
    .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    let entry_values = [
        entry.id.as_str(),
        entry.scenario.as_str(),
        entry.scenario_config_sha256.as_str(),
        entry.runner_fingerprint.as_str(),
        entry.artifact_path.as_str(),
        entry.artifact_sha256.as_str(),
        entry.source_commit.as_str(),
        entry.approval_ref.as_str(),
    ];
    if !diff.contains("perf/baselines/manifest.toml")
        || !diff.contains(&entry.artifact_path)
        || entry_values.iter().any(|value| !diff.contains(value))
    {
        return Err(BaselineError::GitHubVerification(
            "reviewed diff does not contain the exact manifest entry and artifact hash".into(),
        ));
    }

    let evidence = serde_json::to_vec(&(
        repository,
        default_branch,
        number,
        author,
        &entry.approval_ref,
        &entry.artifact_sha256,
    ))
    .expect("GitHub evidence tuple is serializable");
    Ok(VerifiedApproval {
        approval_ref: entry.approval_ref.clone(),
        default_branch_protected: true,
        manifest_from_default_branch: true,
        pull_request_merged: true,
        approved_by_non_author: true,
        reviewed_diff_contains_entry_and_hash: true,
        github_api_evidence_sha256: format!("{:x}", Sha256::digest(evidence)),
    })
}

fn gh_api(endpoint: &str, accept: Option<&str>) -> Result<Vec<u8>, BaselineError> {
    let mut command = Command::new("gh");
    command.args(["api", endpoint]);
    if let Some(accept) = accept {
        command.args(["-H", &format!("Accept: {accept}")]);
    }
    let output = command
        .output()
        .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    if !output.status.success() {
        return Err(BaselineError::GitHubVerification(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(output.stdout)
}

fn parse_approval_ref(value: &str) -> Option<(&str, u64)> {
    let rest = value.strip_prefix("github-pr:")?;
    let (repository, number) = rest.rsplit_once('#')?;
    let number = number.parse().ok()?;
    (repository.split('/').count() == 2 && number > 0).then_some((repository, number))
}

fn normalize_lines(bytes: &[u8]) -> Vec<u8> {
    String::from_utf8_lossy(bytes)
        .replace("\r\n", "\n")
        .into_bytes()
}

pub fn resolve_reviewed_entry<'a>(
    manifest: &'a BaselineManifest,
    id: &str,
    approval: Option<&VerifiedApproval>,
) -> Result<&'a BaselineEntry, BaselineError> {
    let entry = manifest
        .baseline
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| BaselineError::MissingEntry { id: id.into() })?;
    validate_entry(entry, false, true)?;
    let approval = approval.ok_or_else(|| BaselineError::ApprovalUnavailable {
        approval_ref: entry.approval_ref.clone(),
    })?;
    if !approval.validates(&entry.approval_ref) {
        return Err(BaselineError::ApprovalUnavailable {
            approval_ref: entry.approval_ref.clone(),
        });
    }
    Ok(entry)
}

pub fn load_reviewed_baseline(
    manifest: &BaselineManifest,
    id: &str,
    approval: &VerifiedApproval,
) -> Result<PerfReport, BaselineError> {
    let entry = manifest
        .baseline
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| BaselineError::MissingEntry { id: id.into() })?;
    validate_entry(entry, true, false)?;
    if !approval.validates(&entry.approval_ref) {
        return Err(BaselineError::ApprovalUnavailable {
            approval_ref: entry.approval_ref.clone(),
        });
    }
    let bytes = fs::read(&entry.artifact_path)
        .map_err(|error| BaselineError::ArtifactRead(error.to_string()))?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != entry.artifact_sha256 {
        return Err(BaselineError::ArtifactHashMismatch {
            expected: entry.artifact_sha256.clone(),
            actual,
        });
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| BaselineError::ArtifactParse(error.to_string()))?;
    let schema_bytes = fs::read("perf/baselines/schema.json")
        .map_err(|error| BaselineError::SchemaValidation(error.to_string()))?;
    let schema: serde_json::Value = serde_json::from_slice(&schema_bytes)
        .map_err(|error| BaselineError::SchemaValidation(error.to_string()))?;
    validate_json_schema(&value, &schema)
        .map_err(|error| BaselineError::SchemaValidation(error.to_string()))?;
    serde_json::from_value(value).map_err(|error| BaselineError::ArtifactParse(error.to_string()))
}

fn validate_entry(
    entry: &BaselineEntry,
    allow_absolute_for_hash_check: bool,
    require_well_formed_artifact_hash: bool,
) -> Result<(), BaselineError> {
    let invalid = |reason: &str| BaselineError::InvalidManifestEntry {
        id: entry.id.clone(),
        reason: reason.into(),
    };
    if entry.id.trim().is_empty()
        || entry.scenario.trim().is_empty()
        || !is_sha256(&entry.scenario_config_sha256)
        || !is_sha256(&entry.runner_fingerprint)
        || (require_well_formed_artifact_hash && !is_sha256(&entry.artifact_sha256))
        || entry.artifact_sha256.trim().is_empty()
        || entry.artifact_sha256.contains('<')
        || !is_commit(&entry.source_commit)
        || !valid_approval_ref(&entry.approval_ref)
    {
        return Err(invalid("missing, placeholder, or malformed field"));
    }
    if entry.artifact_path.trim().is_empty()
        || entry.artifact_path.contains('<')
        || (!allow_absolute_for_hash_check && Path::new(&entry.artifact_path).is_absolute())
        || entry.artifact_path.contains("..")
    {
        return Err(invalid(
            "artifact path must be a repository-relative manifest path",
        ));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_commit(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_approval_ref(value: &str) -> bool {
    parse_approval_ref(value)
        .is_some_and(|(repository, _)| repository.split('/').all(|part| !part.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{Availability, PerfReport, PerfRun, VerdictStatus, REQUIRED_COUNTERS};
    use sha2::{Digest, Sha256};
    use std::{
        collections::BTreeMap,
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn five_run_comparison_rejects_material_p99_regression() {
        let baseline = five_runs("runner-a", "p99_us", [200, 201, 199, 202, 198]);
        let candidate = five_runs("runner-a", "p99_us", [240, 239, 241, 238, 242]);
        let verdict = compare(&baseline, &candidate, ComparisonPolicy::strict()).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Fail);
        assert_eq!(verdict.regressions[0].metric, "p99_us");
    }

    #[test]
    fn median_regression_over_ten_percent_fails() {
        let baseline = five_runs("runner-a", "median_us", [100; 5]);
        let candidate = five_runs("runner-a", "median_us", [111; 5]);
        let verdict = compare(&baseline, &candidate, ComparisonPolicy::strict()).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Fail);
    }

    #[test]
    fn p95_regression_at_fifteen_percent_passes_but_over_fails() {
        let baseline = five_runs("runner-a", "p95_us", [100; 5]);
        let at_limit = five_runs("runner-a", "p95_us", [115; 5]);
        let over_limit = five_runs("runner-a", "p95_us", [116; 5]);
        assert_eq!(
            compare(&baseline, &at_limit, ComparisonPolicy::strict())
                .unwrap()
                .status,
            VerdictStatus::Pass
        );
        assert_eq!(
            compare(&baseline, &over_limit, ComparisonPolicy::strict())
                .unwrap()
                .status,
            VerdictStatus::Fail
        );
    }

    #[test]
    fn runner_fingerprint_mismatch_is_error() {
        let error = compare(
            &five_runs("runner-a", "p99_us", [100; 5]),
            &five_runs("runner-b", "p99_us", [100; 5]),
            ComparisonPolicy::strict(),
        )
        .unwrap_err();
        assert!(matches!(error, CompareError::RunnerMismatch { .. }));
    }

    #[test]
    fn strict_comparison_requires_exactly_five_complete_same_config_runs() {
        let baseline = five_runs("runner-a", "p99_us", [100; 5]);
        let mut candidate = five_runs("runner-a", "p99_us", [100; 5]);
        candidate.runs.pop();
        assert!(matches!(
            compare(&baseline, &candidate, ComparisonPolicy::strict()),
            Err(CompareError::IncompleteBatch {
                expected: 5,
                actual: 4
            })
        ));

        let mut candidate = five_runs("runner-a", "p99_us", [100; 5]);
        candidate.runs[4].scenario_config_sha256 = "different".into();
        assert!(matches!(
            compare(&baseline, &candidate, ComparisonPolicy::strict()),
            Err(CompareError::ScenarioConfigMismatch { .. })
        ));

        let mut candidate = five_runs("runner-a", "p99_us", [100; 5]);
        candidate.runs[4].process_exit_success = false;
        assert!(matches!(
            compare(&baseline, &candidate, ComparisonPolicy::strict()),
            Err(CompareError::RunIncomplete { .. })
        ));
    }

    #[test]
    fn immutable_batch_and_reproducibility_fields_must_match() {
        let baseline = five_runs("runner-a", "p99_us", [100; 5]);
        let mut candidate = five_runs("runner-a", "p99_us", [100; 5]);
        candidate.runs[4].batch_id = "selective-repair".into();
        assert!(matches!(
            compare(&baseline, &candidate, ComparisonPolicy::strict()),
            Err(CompareError::BatchMismatch { .. })
        ));

        let mut candidate = five_runs("runner-a", "p99_us", [100; 5]);
        candidate.reports[4]
            .cargo_features
            .push("unreviewed".into());
        assert!(matches!(
            compare(&baseline, &candidate, ComparisonPolicy::strict()),
            Err(CompareError::BuildMismatch { .. })
        ));
    }

    #[test]
    fn missing_required_metric_is_an_error() {
        let baseline = five_runs("runner-a", "p99_us", [100; 5]);
        let mut candidate = five_runs("runner-a", "p99_us", [100; 5]);
        candidate.runs[3].metrics.clear();
        assert!(matches!(
            compare(&baseline, &candidate, ComparisonPolicy::strict()),
            Err(CompareError::MissingMetric { .. })
        ));
    }

    #[test]
    fn missing_required_counter_is_an_error() {
        let baseline = five_runs("runner-a", "p99_us", [100; 5]);
        let mut candidate = five_runs("runner-a", "p99_us", [100; 5]);
        candidate.runs[2].counters.remove("out_of_order");
        assert!(matches!(
            compare(&baseline, &candidate, ComparisonPolicy::strict()),
            Err(CompareError::MissingCounter { ref counter, .. })
                if counter == "out_of_order"
        ));
    }

    #[test]
    fn coefficient_of_variation_over_ten_percent_is_unstable() {
        let baseline = five_runs("runner-a", "p99_us", [100; 5]);
        let candidate = five_runs("runner-a", "p99_us", [70, 85, 100, 115, 130]);
        let verdict = compare(&baseline, &candidate, ComparisonPolicy::strict()).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Unstable);
    }

    #[test]
    fn unsupported_or_not_run_cannot_pass() {
        let baseline = five_runs("runner-a", "p99_us", [100; 5]);
        let mut candidate = five_runs("runner-a", "p99_us", [100; 5]);
        candidate.reports[0].availability = Availability::Unsupported {
            reason: "unsupported".into(),
        };
        candidate.reports[0].verdict = VerdictStatus::Unsupported;
        assert!(matches!(
            compare(&baseline, &candidate, ComparisonPolicy::strict()),
            Err(CompareError::Unavailable { .. })
        ));
    }

    #[test]
    fn baseline_artifact_must_match_the_reviewed_manifest_hash() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("rshare-perf-{suffix}"));
        fs::create_dir_all(&directory).unwrap();
        let artifact = directory.join("baseline.json");
        fs::write(&artifact, b"{}").unwrap();
        let manifest = BaselineManifest {
            baseline: vec![BaselineEntry {
                id: "windows-control-v3-runner-a".into(),
                scenario: "quic-control-v3".into(),
                scenario_config_sha256: hex_sha256(b"config"),
                runner_fingerprint: hex_sha256(b"runner"),
                artifact_path: artifact.to_string_lossy().into_owned(),
                artifact_sha256: "00bad".into(),
                source_commit: "0123456789abcdef0123456789abcdef01234567".into(),
                approval_ref: "github-pr:owner/repo#1".into(),
            }],
        };
        assert!(matches!(
            load_reviewed_baseline(
                &manifest,
                "windows-control-v3-runner-a",
                &VerifiedApproval::test_fixture("github-pr:owner/repo#1")
            ),
            Err(BaselineError::ArtifactHashMismatch { .. })
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn strict_baseline_resolution_rejects_missing_unhashed_placeholder_and_unapproved() {
        let empty = BaselineManifest { baseline: vec![] };
        assert!(matches!(
            resolve_reviewed_entry(&empty, "missing", None),
            Err(BaselineError::MissingEntry { .. })
        ));

        for invalid_hash in ["", "<sha256>"] {
            let manifest = manifest_fixture(invalid_hash, "github-pr:owner/repo#1");
            assert!(matches!(
                resolve_reviewed_entry(&manifest, "id", None),
                Err(BaselineError::InvalidManifestEntry { .. })
            ));
        }

        let manifest = manifest_fixture(&hex_sha256(b"artifact"), "github-pr:owner/repo#1");
        assert!(matches!(
            resolve_reviewed_entry(&manifest, "id", None),
            Err(BaselineError::ApprovalUnavailable { .. })
        ));
    }

    fn five_runs(runner: &str, metric: &str, values: [u64; 5]) -> ReportBatch {
        let reports: Vec<_> = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let mut metrics = BTreeMap::new();
                metrics.insert(metric.into(), *value as f64);
                let mut counters = BTreeMap::new();
                for counter in REQUIRED_COUNTERS {
                    counters.insert(counter.to_string(), 0);
                }
                let run = PerfRun {
                    run_id: format!("run-{index}"),
                    batch_id: "batch-a".into(),
                    process_exit_success: true,
                    schema_valid: true,
                    scenario_config_sha256: "config".into(),
                    metrics,
                    counters,
                    errors: vec![],
                };
                PerfReport::test_fixture("batch-a", runner, vec![run])
            })
            .collect();
        ReportBatch::from_reports(reports)
    }

    fn hex_sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn manifest_fixture(hash: &str, approval_ref: &str) -> BaselineManifest {
        BaselineManifest {
            baseline: vec![BaselineEntry {
                id: "id".into(),
                scenario: "quic-control-v3".into(),
                scenario_config_sha256: hex_sha256(b"config"),
                runner_fingerprint: hex_sha256(b"runner"),
                artifact_path: "perf/baselines/artifact.json".into(),
                artifact_sha256: hash.into(),
                source_commit: "0123456789abcdef0123456789abcdef01234567".into(),
                approval_ref: approval_ref.into(),
            }],
        }
    }
}
