use crate::report::{
    parse_and_validate_report, Availability, BatchArtifactReference, MetricDirection, PerfReport,
    PerfRun, ScenarioContract, VerdictStatus, PERF_SCHEMA_VERSION, REQUIRED_COUNTERS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    path::{Component, Path},
    process::Command,
};

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
            reviewed_manifest_id: None,
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
    pub baseline_artifacts: BuildArtifactHashes,
    pub candidate_artifacts: BuildArtifactHashes,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildArtifactHashes {
    pub binary_sha256: BTreeMap<String, String>,
    pub cargo_lock_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Regression {
    pub metric: String,
    pub direction: MetricDirection,
    pub baseline_median: f64,
    pub candidate_median: f64,
    pub regression: f64,
    pub allowed: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComparedMetric {
    pub metric: String,
    pub direction: MetricDirection,
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
    #[error("run id {run_id} appears more than once in a strict batch")]
    DuplicateRunId { run_id: String },
    #[error("scenario contract has no direction for metric {metric}")]
    MissingMetricDirection { metric: String },
    #[error("report is unavailable: {reason}")]
    Unavailable { reason: String },
    #[error("reviewed baseline metric {metric} has coefficient of variation {actual:.4}, above {limit:.4}")]
    UnstableBaseline {
        metric: String,
        actual: f64,
        limit: f64,
    },
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
    let contract = ScenarioContract::for_report(&baseline.reports[0]).map_err(|_| {
        CompareError::ScenarioConfigMismatch {
            details: "baseline scenario has no comparison contract".into(),
        }
    })?;
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
        let baseline_cv = coefficient_of_variation(&baseline_values);
        if baseline_cv > policy.cv_limit {
            return Err(CompareError::UnstableBaseline {
                metric: metric.clone(),
                actual: baseline_cv,
                limit: policy.cv_limit,
            });
        }
        let candidate_cv = coefficient_of_variation(&candidate_values);
        unstable |= candidate_cv > policy.cv_limit;
        let direction = contract
            .metric_directions
            .get(&metric)
            .copied()
            .ok_or_else(|| CompareError::MissingMetricDirection {
                metric: metric.clone(),
            })?;
        let allowed = if metric.contains("p95") || metric.contains("p99") {
            policy.tail_regression_limit
        } else {
            policy.median_regression_limit
        };
        let regression = match direction {
            MetricDirection::LowerIsBetter => {
                if baseline_median == 0.0 {
                    if candidate_median == 0.0 {
                        0.0
                    } else {
                        f64::INFINITY
                    }
                } else {
                    candidate_median / baseline_median - 1.0
                }
            }
            MetricDirection::HigherIsBetter => {
                if candidate_median == 0.0 {
                    if baseline_median == 0.0 {
                        0.0
                    } else {
                        f64::INFINITY
                    }
                } else {
                    baseline_median / candidate_median - 1.0
                }
            }
        };
        comparisons.push(ComparedMetric {
            metric: metric.clone(),
            direction,
            baseline_median,
            candidate_median,
            candidate_cv,
        });
        if regression > allowed {
            regressions.push(Regression {
                metric,
                direction,
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
        baseline_artifacts: BuildArtifactHashes {
            binary_sha256: baseline.reports[0].binary_sha256.clone(),
            cargo_lock_sha256: baseline.reports[0].cargo_lock_sha256.clone(),
        },
        candidate_artifacts: BuildArtifactHashes {
            binary_sha256: candidate.reports[0].binary_sha256.clone(),
            cargo_lock_sha256: candidate.reports[0].cargo_lock_sha256.clone(),
        },
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
    let mut run_ids = HashSet::with_capacity(batch.runs.len());
    for (index, run) in batch.runs.iter().enumerate() {
        if !run_ids.insert(run.run_id.as_str()) {
            return Err(CompareError::DuplicateRunId {
                run_id: run.run_id.clone(),
            });
        }
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
        if run.raw_samples.is_empty() || run.raw_samples.values().any(|samples| samples.is_empty())
        {
            return Err(CompareError::RunIncomplete {
                run_id: run.run_id.clone(),
                reason: "run has no raw latency samples".into(),
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
        if report.verdict != VerdictStatus::Pass {
            return Err(CompareError::Unavailable {
                reason: format!("strict comparison report verdict is {:?}", report.verdict),
            });
        }
        if report.batch_artifacts.is_empty() {
            return Err(CompareError::RunIncomplete {
                run_id: run.run_id.clone(),
                reason: "strict comparison artifact has no raw batch sidecars".into(),
            });
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
        if within_batch_context_fingerprint(report) != within_batch_context_fingerprint(first) {
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
    let baseline_roles: BTreeSet<_> = baseline.binary_sha256.keys().collect();
    let candidate_roles: BTreeSet<_> = candidate.binary_sha256.keys().collect();
    if baseline_roles != candidate_roles {
        return Err(CompareError::BuildMismatch {
            details: "baseline and candidate binary role sets differ".into(),
        });
    }
    if cross_revision_context_fingerprint(baseline) != cross_revision_context_fingerprint(candidate)
    {
        return Err(CompareError::BuildMismatch {
            details: "baseline and candidate immutable build context differs".into(),
        });
    }
    Ok(())
}

fn within_batch_context_fingerprint(report: &PerfReport) -> String {
    let bytes = serde_json::to_vec(&(
        cross_revision_context_fingerprint(report),
        &report.commit,
        &report.binary_sha256,
        &report.cargo_lock_sha256,
    ))
    .expect("within-batch comparison context is serializable");
    format!("{:x}", Sha256::digest(bytes))
}

fn cross_revision_context_fingerprint(report: &PerfReport) -> String {
    let bytes = serde_json::to_vec(&(
        report.schema_version,
        &report.scenario,
        &report.scenario_config_sha256,
        report.random_seed,
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
    pub reviewed_artifact_matches_hash: bool,
    pub github_api_evidence: serde_json::Value,
    pub github_api_evidence_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubTrustPolicy {
    pub expected_repository: String,
    pub manifest_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GithubApiEvidence {
    pub repository: String,
    pub default_branch: String,
    pub default_branch_protected: bool,
    pub default_branch_manifest: String,
    pub pull_request_number: u64,
    pub pull_request_merged: bool,
    pub pull_request_author: String,
    pub pull_request_base_repository: String,
    pub pull_request_base_ref: String,
    pub pull_request_base_sha: String,
    pub pull_request_head_sha: String,
    pub merge_commit_sha: String,
    pub merge_commit_reachable_from_default: bool,
    pub pr_base_manifest: String,
    pub pr_head_manifest: String,
    pub pr_head_artifact_sha256: String,
    pub pr_head_batch_artifact_references: BTreeMap<String, String>,
    pub pr_head_batch_artifact_sha256: BTreeMap<String, String>,
    pub reviews: Vec<GithubReview>,
    pub changed_files: Vec<GithubChangedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GithubReview {
    pub review_id: u64,
    pub reviewer: String,
    pub state: String,
    pub commit_id: String,
    pub repository_permission: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GithubChangedFile {
    pub filename: String,
    pub patch: String,
}

impl VerifiedApproval {
    #[cfg(test)]
    fn test_fixture(approval_ref: &str) -> Self {
        let github_api_evidence = serde_json::json!({ "fixture": true });
        Self {
            approval_ref: approval_ref.into(),
            default_branch_protected: true,
            manifest_from_default_branch: true,
            pull_request_merged: true,
            approved_by_non_author: true,
            reviewed_diff_contains_entry_and_hash: true,
            reviewed_artifact_matches_hash: true,
            github_api_evidence_sha256: format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&github_api_evidence).unwrap())
            ),
            github_api_evidence,
        }
    }

    fn validates(&self, approval_ref: &str) -> bool {
        self.approval_ref == approval_ref
            && self.default_branch_protected
            && self.manifest_from_default_branch
            && self.pull_request_merged
            && self.approved_by_non_author
            && self.reviewed_diff_contains_entry_and_hash
            && self.reviewed_artifact_matches_hash
            && is_sha256(&self.github_api_evidence_sha256)
            && serde_json::to_vec(&self.github_api_evidence)
                .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
                .is_ok_and(|actual| actual == self.github_api_evidence_sha256)
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
    #[error("baseline artifact failed schema validation: {0}")]
    SchemaValidation(String),
    #[error("GitHub approval verification failed: {0}")]
    GitHubVerification(String),
    #[error(
        "baseline report field {field} does not match manifest: expected {expected}, got {actual}"
    )]
    ReportMismatch {
        field: String,
        expected: String,
        actual: String,
    },
}

pub fn validate_github_trust(
    policy: &GithubTrustPolicy,
    entry: &BaselineEntry,
    local_manifest: &[u8],
    evidence: &GithubApiEvidence,
) -> Result<VerifiedApproval, BaselineError> {
    validate_entry(entry, true)?;
    let (approval_repository, approval_number) = parse_approval_ref(&entry.approval_ref)
        .ok_or_else(|| BaselineError::GitHubVerification("invalid approval_ref".into()))?;
    if approval_repository != policy.expected_repository
        || evidence.repository != policy.expected_repository
    {
        return Err(BaselineError::GitHubVerification(
            "approval repository does not match canonical origin".into(),
        ));
    }
    if policy.manifest_path != "perf/baselines/manifest.toml" {
        return Err(BaselineError::GitHubVerification(
            "manifest path is not the protected repository baseline manifest".into(),
        ));
    }
    if evidence.default_branch.trim().is_empty()
        || evidence.pull_request_base_repository != policy.expected_repository
        || evidence.pull_request_base_ref != evidence.default_branch
    {
        return Err(BaselineError::GitHubVerification(
            "pull request base is not the canonical repository default branch".into(),
        ));
    }
    if !evidence.default_branch_protected
        || normalize_lines(evidence.default_branch_manifest.as_bytes())
            != normalize_lines(local_manifest)
    {
        return Err(BaselineError::GitHubVerification(
            "manifest is not the protected default-branch content".into(),
        ));
    }
    if !evidence.pull_request_merged || evidence.pull_request_number != approval_number {
        return Err(BaselineError::GitHubVerification(
            "approval_ref is not the verified merged pull request".into(),
        ));
    }
    if !is_commit(&evidence.merge_commit_sha) || !evidence.merge_commit_reachable_from_default {
        return Err(BaselineError::GitHubVerification(
            "pull request merge commit is not reachable from the default branch".into(),
        ));
    }
    if !is_commit(&evidence.pull_request_base_sha)
        || !is_commit(&evidence.pull_request_head_sha)
        || evidence.pull_request_base_sha == evidence.pull_request_head_sha
    {
        return Err(BaselineError::GitHubVerification(
            "pull request base/head commits are malformed or identical".into(),
        ));
    }
    let mut latest_reviews = BTreeMap::<&str, &GithubReview>::new();
    for review in &evidence.reviews {
        latest_reviews
            .entry(&review.reviewer)
            .and_modify(|latest| {
                if review.review_id > latest.review_id {
                    *latest = review;
                }
            })
            .or_insert(review);
    }
    let approved_head = latest_reviews.values().any(|review| {
        review.state == "APPROVED"
            && review.reviewer != evidence.pull_request_author
            && review.commit_id == evidence.pull_request_head_sha
            && matches!(
                review.repository_permission.as_str(),
                "write" | "maintain" | "admin"
            )
    });
    if !approved_head {
        return Err(BaselineError::GitHubVerification(
            "no non-author approval is bound to the verified pull request head".into(),
        ));
    }
    if !evidence
        .changed_files
        .iter()
        .any(|file| file.filename == policy.manifest_path)
    {
        return Err(BaselineError::GitHubVerification(
            "pull request did not change the protected manifest".into(),
        ));
    }
    if evidence.pr_head_batch_artifact_references != evidence.pr_head_batch_artifact_sha256
        || evidence
            .pr_head_batch_artifact_references
            .iter()
            .any(|(path, hash)| {
                !is_sha256(hash)
                    || !evidence
                        .changed_files
                        .iter()
                        .any(|file| file.filename == *path)
            })
    {
        return Err(BaselineError::GitHubVerification(
            "approved pull request head does not bind every referenced batch sidecar".into(),
        ));
    }
    if !evidence
        .changed_files
        .iter()
        .any(|file| file.filename == entry.artifact_path)
        || evidence.pr_head_artifact_sha256 != entry.artifact_sha256
    {
        return Err(BaselineError::GitHubVerification(
            "approved pull request head does not contain the reviewed baseline artifact hash"
                .into(),
        ));
    }
    let head_manifest: BaselineManifest = toml::from_str(&evidence.pr_head_manifest)
        .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    if !head_manifest
        .baseline
        .iter()
        .any(|head_entry| head_entry == entry)
    {
        return Err(BaselineError::GitHubVerification(
            "approved pull request head lacks the exact manifest entry".into(),
        ));
    }
    let base_manifest: BaselineManifest = toml::from_str(&evidence.pr_base_manifest)
        .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    if base_manifest
        .baseline
        .iter()
        .any(|base_entry| base_entry == entry)
    {
        return Err(BaselineError::GitHubVerification(
            "exact manifest entry already existed at the pull request base".into(),
        ));
    }
    let github_api_evidence = serde_json::to_value(evidence)
        .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    let evidence_bytes = serde_json::to_vec(&github_api_evidence)
        .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    Ok(VerifiedApproval {
        approval_ref: entry.approval_ref.clone(),
        default_branch_protected: true,
        manifest_from_default_branch: true,
        pull_request_merged: true,
        approved_by_non_author: true,
        reviewed_diff_contains_entry_and_hash: true,
        reviewed_artifact_matches_hash: true,
        github_api_evidence,
        github_api_evidence_sha256: format!("{:x}", Sha256::digest(evidence_bytes)),
    })
}

pub fn verify_github_approval(
    entry: &BaselineEntry,
    manifest_path: &Path,
    expected_repository: &str,
) -> Result<VerifiedApproval, BaselineError> {
    validate_entry(entry, true)?;
    let (approval_repository, number) = parse_approval_ref(&entry.approval_ref)
        .ok_or_else(|| BaselineError::GitHubVerification("invalid approval_ref".into()))?;
    if approval_repository != expected_repository {
        return Err(BaselineError::GitHubVerification(
            "approval repository does not match canonical origin".into(),
        ));
    }
    let repository = expected_repository;
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

    let pull: serde_json::Value = serde_json::from_slice(&gh_api(
        &format!("repos/{repository}/pulls/{number}"),
        None,
    )?)
    .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    let author = pull["user"]["login"]
        .as_str()
        .ok_or_else(|| BaselineError::GitHubVerification("pull request author is absent".into()))?;
    let head_sha = pull["head"]["sha"].as_str().ok_or_else(|| {
        BaselineError::GitHubVerification("pull request head SHA is absent".into())
    })?;
    let base_sha = pull["base"]["sha"].as_str().ok_or_else(|| {
        BaselineError::GitHubVerification("pull request base SHA is absent".into())
    })?;
    let base_repository = pull["base"]["repo"]["full_name"].as_str().ok_or_else(|| {
        BaselineError::GitHubVerification("pull request base repository is absent".into())
    })?;
    let base_ref = pull["base"]["ref"].as_str().ok_or_else(|| {
        BaselineError::GitHubVerification("pull request base ref is absent".into())
    })?;
    let merge_commit_sha = pull["merge_commit_sha"].as_str().ok_or_else(|| {
        BaselineError::GitHubVerification("pull request merge commit SHA is absent".into())
    })?;
    let compare_value: serde_json::Value = serde_json::from_slice(&gh_api(
        &format!("repos/{repository}/compare/{merge_commit_sha}...{default_branch}"),
        None,
    )?)
    .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    let merge_commit_reachable_from_default = matches!(
        compare_value["status"].as_str(),
        Some("ahead" | "identical")
    );
    let reviews_value = gh_api_paginated(
        &format!("repos/{repository}/pulls/{number}/reviews?per_page=100"),
        "pull request reviews",
    )?;
    let mut reviews = Vec::new();
    for review in &reviews_value {
        let Some(reviewer) = review["user"]["login"].as_str() else {
            continue;
        };
        let Some(state) = review["state"].as_str() else {
            continue;
        };
        let Some(commit_id) = review["commit_id"].as_str() else {
            continue;
        };
        let Some(review_id) = review["id"].as_u64() else {
            continue;
        };
        let permission_value: serde_json::Value = serde_json::from_slice(&gh_api(
            &format!("repos/{repository}/collaborators/{reviewer}/permission"),
            None,
        )?)
        .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
        let repository_permission = permission_value["permission"].as_str().ok_or_else(|| {
            BaselineError::GitHubVerification(format!(
                "repository permission is absent for reviewer {reviewer}"
            ))
        })?;
        reviews.push(GithubReview {
            review_id,
            reviewer: reviewer.into(),
            state: state.into(),
            commit_id: commit_id.into(),
            repository_permission: repository_permission.into(),
        });
    }
    let files_value = gh_api_paginated(
        &format!("repos/{repository}/pulls/{number}/files?per_page=100"),
        "pull request files",
    )?;
    let changed_files = files_value
        .iter()
        .filter_map(|file| {
            Some(GithubChangedFile {
                filename: file["filename"].as_str()?.into(),
                patch: file["patch"].as_str().unwrap_or_default().into(),
            })
        })
        .collect();
    let pr_head_manifest = String::from_utf8(gh_api(
        &format!("repos/{repository}/contents/perf/baselines/manifest.toml?ref={head_sha}"),
        Some("application/vnd.github.raw+json"),
    )?)
    .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    let pr_base_manifest = String::from_utf8(gh_api(
        &format!("repos/{repository}/contents/perf/baselines/manifest.toml?ref={base_sha}"),
        Some("application/vnd.github.raw+json"),
    )?)
    .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    let pr_head_artifact = gh_api(
        &format!(
            "repos/{repository}/contents/{}?ref={head_sha}",
            entry.artifact_path
        ),
        Some("application/vnd.github.raw+json"),
    )?;
    let artifact_value: serde_json::Value = serde_json::from_slice(&pr_head_artifact)
        .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    let mut pr_head_batch_artifact_references = BTreeMap::new();
    let mut pr_head_batch_artifact_sha256 = BTreeMap::new();
    let batch_artifacts = artifact_value["batch_artifacts"]
        .as_array()
        .ok_or_else(|| {
            BaselineError::GitHubVerification(
                "reviewed baseline artifact has no batch_artifacts array".into(),
            )
        })?;
    for reference in batch_artifacts {
        let batch_id = reference["batch_id"].as_str().ok_or_else(|| {
            BaselineError::GitHubVerification("batch artifact id is absent".into())
        })?;
        let path = reference["path"].as_str().ok_or_else(|| {
            BaselineError::GitHubVerification("batch artifact path is absent".into())
        })?;
        let sha256 = reference["sha256"].as_str().ok_or_else(|| {
            BaselineError::GitHubVerification("batch artifact SHA-256 is absent".into())
        })?;
        if Path::new(path).is_absolute()
            || Path::new(path)
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            || !is_sha256(sha256)
        {
            return Err(BaselineError::GitHubVerification(
                "batch artifact reference is not a safe repository-relative SHA-256 binding".into(),
            ));
        }
        let bytes = gh_api(
            &format!("repos/{repository}/contents/{path}?ref={head_sha}"),
            Some("application/vnd.github.raw+json"),
        )?;
        validate_sidecar_envelope(
            &bytes,
            artifact_value["scenario"].as_str().unwrap_or_default(),
            artifact_value["scenario_config_sha256"]
                .as_str()
                .unwrap_or_default(),
            batch_id,
        )
        .map_err(BaselineError::GitHubVerification)?;
        pr_head_batch_artifact_references.insert(path.into(), sha256.into());
        pr_head_batch_artifact_sha256.insert(path.into(), format!("{:x}", Sha256::digest(bytes)));
    }
    let evidence = GithubApiEvidence {
        repository: repository.into(),
        default_branch: default_branch.into(),
        default_branch_protected: true,
        default_branch_manifest: String::from_utf8(remote_manifest)
            .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?,
        pull_request_number: number,
        pull_request_merged: !pull["merged_at"].is_null(),
        pull_request_author: author.into(),
        pull_request_base_repository: base_repository.into(),
        pull_request_base_ref: base_ref.into(),
        pull_request_base_sha: base_sha.into(),
        pull_request_head_sha: head_sha.into(),
        merge_commit_sha: merge_commit_sha.into(),
        merge_commit_reachable_from_default,
        pr_base_manifest,
        pr_head_manifest,
        pr_head_artifact_sha256: format!("{:x}", Sha256::digest(pr_head_artifact)),
        pr_head_batch_artifact_references,
        pr_head_batch_artifact_sha256,
        reviews,
        changed_files,
    };
    validate_github_trust(
        &GithubTrustPolicy {
            expected_repository: repository.into(),
            manifest_path: "perf/baselines/manifest.toml".into(),
        },
        entry,
        &local_manifest,
        &evidence,
    )
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

fn gh_api_paginated(
    endpoint: &str,
    evidence_name: &str,
) -> Result<Vec<serde_json::Value>, BaselineError> {
    let output = Command::new("gh")
        .args(["api", "--paginate", "--slurp", endpoint])
        .output()
        .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    if !output.status.success() {
        return Err(BaselineError::GitHubVerification(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let pages = serde_json::from_slice(&output.stdout)
        .map_err(|error| BaselineError::GitHubVerification(error.to_string()))?;
    flatten_paginated_items(pages, evidence_name)
}

fn flatten_paginated_items(
    pages: serde_json::Value,
    evidence_name: &str,
) -> Result<Vec<serde_json::Value>, BaselineError> {
    const MAX_PAGES: usize = 100;
    const MAX_ITEMS: usize = 10_000;

    let pages = pages.as_array().ok_or_else(|| {
        BaselineError::GitHubVerification(format!("{evidence_name} pagination is not an array"))
    })?;
    if pages.len() > MAX_PAGES {
        return Err(BaselineError::GitHubVerification(format!(
            "{evidence_name} exceeds the {MAX_PAGES}-page verification limit"
        )));
    }
    let mut items = Vec::new();
    for page in pages {
        let page = page.as_array().ok_or_else(|| {
            BaselineError::GitHubVerification(format!(
                "{evidence_name} pagination contains a non-array page"
            ))
        })?;
        if items.len().saturating_add(page.len()) > MAX_ITEMS {
            return Err(BaselineError::GitHubVerification(format!(
                "{evidence_name} exceeds the {MAX_ITEMS}-item verification limit"
            )));
        }
        items.extend(page.iter().cloned());
    }
    Ok(items)
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
    validate_entry(entry, true)?;
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
    repository_root: &Path,
) -> Result<PerfReport, BaselineError> {
    let entry = manifest
        .baseline
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| BaselineError::MissingEntry { id: id.into() })?;
    validate_entry(entry, false)?;
    if !approval.validates(&entry.approval_ref) {
        return Err(BaselineError::ApprovalUnavailable {
            approval_ref: entry.approval_ref.clone(),
        });
    }
    let repository_root = repository_root
        .canonicalize()
        .map_err(|error| BaselineError::ArtifactRead(error.to_string()))?;
    let artifact_path = repository_root.join(&entry.artifact_path);
    let canonical_artifact = artifact_path
        .canonicalize()
        .map_err(|error| BaselineError::ArtifactRead(error.to_string()))?;
    if !canonical_artifact.starts_with(&repository_root) {
        return Err(BaselineError::InvalidManifestEntry {
            id: entry.id.clone(),
            reason: "artifact path escapes the repository root".into(),
        });
    }
    let bytes = fs::read(&canonical_artifact)
        .map_err(|error| BaselineError::ArtifactRead(error.to_string()))?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != entry.artifact_sha256 {
        return Err(BaselineError::ArtifactHashMismatch {
            expected: entry.artifact_sha256.clone(),
            actual,
        });
    }
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../perf/baselines/schema.json"))
            .map_err(|error| BaselineError::SchemaValidation(error.to_string()))?;
    let report = parse_and_validate_report(&bytes, &schema)
        .map_err(|error| BaselineError::SchemaValidation(error.to_string()))?;
    verify_report_matches_entry(&report, entry)?;
    verify_local_sidecars(&report, &repository_root)?;
    Ok(report)
}

fn verify_local_sidecars(report: &PerfReport, repository_root: &Path) -> Result<(), BaselineError> {
    if report.batch_artifacts.is_empty() {
        return Err(BaselineError::SchemaValidation(
            "reviewed baseline has no raw sidecars".into(),
        ));
    }
    for reference in &report.batch_artifacts {
        let relative = Path::new(&reference.path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(BaselineError::SchemaValidation(
                "reviewed baseline sidecar path is unsafe".into(),
            ));
        }
        let path = repository_root.join(relative);
        let canonical = path
            .canonicalize()
            .map_err(|error| BaselineError::ArtifactRead(error.to_string()))?;
        if !canonical.starts_with(repository_root) {
            return Err(BaselineError::SchemaValidation(
                "reviewed baseline sidecar escapes repository root".into(),
            ));
        }
        let bytes =
            fs::read(canonical).map_err(|error| BaselineError::ArtifactRead(error.to_string()))?;
        let actual = format!("{:x}", Sha256::digest(&bytes));
        if actual != reference.sha256 {
            return Err(BaselineError::SchemaValidation(format!(
                "reviewed baseline sidecar {} hash mismatch",
                reference.path
            )));
        }
        validate_sidecar_envelope(
            &bytes,
            &report.scenario,
            &report.scenario_config_sha256,
            &reference.batch_id,
        )
        .map_err(BaselineError::SchemaValidation)?;
    }
    Ok(())
}

fn validate_sidecar_envelope(
    bytes: &[u8],
    scenario: &str,
    scenario_config_sha256: &str,
    batch_id: &str,
) -> Result<(), String> {
    if bytes.is_empty() {
        return Err("reviewed baseline sidecar is empty".into());
    }
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let payload_present = value
        .get("payload")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|payload| !payload.is_empty());
    if value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
        || value.get("scenario").and_then(serde_json::Value::as_str) != Some(scenario)
        || value
            .get("scenario_config_sha256")
            .and_then(serde_json::Value::as_str)
            != Some(scenario_config_sha256)
        || value.get("batch_id").and_then(serde_json::Value::as_str) != Some(batch_id)
        || !value
            .get("source_sha256")
            .and_then(serde_json::Value::as_str)
            .is_some_and(is_sha256)
        || !payload_present
    {
        return Err(
            "reviewed baseline sidecar is not a non-empty scenario/config/batch-bound envelope"
                .into(),
        );
    }
    Ok(())
}

fn verify_report_matches_entry(
    report: &PerfReport,
    entry: &BaselineEntry,
) -> Result<(), BaselineError> {
    let fields = [
        ("scenario", &entry.scenario, &report.scenario),
        (
            "scenario_config_sha256",
            &entry.scenario_config_sha256,
            &report.scenario_config_sha256,
        ),
        (
            "runner_fingerprint",
            &entry.runner_fingerprint,
            &report.runner_fingerprint,
        ),
        ("commit", &entry.source_commit, &report.commit),
    ];
    for (field, expected, actual) in fields {
        if expected != actual {
            return Err(BaselineError::ReportMismatch {
                field: field.into(),
                expected: expected.clone(),
                actual: actual.clone(),
            });
        }
    }
    Ok(())
}

fn validate_entry(
    entry: &BaselineEntry,
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
        || Path::new(&entry.artifact_path).is_absolute()
        || Path::new(&entry.artifact_path)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
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
    fn achieved_rate_decrease_fails_but_increase_does_not() {
        let thousand = five_runs("runner-a", "achieved_hz", [1_000; 5]);
        let nine_hundred = five_runs("runner-a", "achieved_hz", [900; 5]);

        let regression = compare(&thousand, &nine_hundred, ComparisonPolicy::strict()).unwrap();
        assert_eq!(regression.status, VerdictStatus::Fail);
        assert_eq!(
            regression.regressions[0].direction,
            MetricDirection::HigherIsBetter
        );
        assert_eq!(
            regression.metrics[0].direction,
            MetricDirection::HigherIsBetter
        );
        assert_eq!(
            compare(&nine_hundred, &thousand, ComparisonPolicy::strict())
                .unwrap()
                .status,
            VerdictStatus::Pass
        );
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
    fn strict_comparison_allows_different_valid_build_hashes_across_revisions() {
        let baseline = five_runs("runner-a", "p99_us", [100; 5]);
        let mut candidate = five_runs("runner-a", "p99_us", [100; 5]);
        for report in &mut candidate.reports {
            report.commit = "fedcba9876543210fedcba9876543210fedcba98".into();
            report
                .binary_sha256
                .insert("rshare-perf".into(), hex_sha256(b"candidate-binary"));
            report.cargo_lock_sha256 = hex_sha256(b"candidate-lock");
        }

        let verdict = compare(&baseline, &candidate, ComparisonPolicy::strict()).unwrap();
        assert_eq!(
            verdict.baseline_artifacts.binary_sha256,
            baseline.reports[0].binary_sha256
        );
        assert_eq!(
            verdict.candidate_artifacts.binary_sha256,
            candidate.reports[0].binary_sha256
        );
        assert_eq!(
            verdict.baseline_artifacts.cargo_lock_sha256,
            baseline.reports[0].cargo_lock_sha256
        );
        assert_eq!(
            verdict.candidate_artifacts.cargo_lock_sha256,
            candidate.reports[0].cargo_lock_sha256
        );
    }

    #[test]
    fn strict_comparison_rejects_commit_drift_inside_one_batch() {
        let baseline = five_runs("runner-a", "p99_us", [100; 5]);
        let mut candidate = five_runs("runner-a", "p99_us", [100; 5]);
        candidate.reports[4].commit = "fedcba9876543210fedcba9876543210fedcba98".into();

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
    fn report_batch_does_not_invent_reviewed_baseline_identity() {
        let report = PerfReport::test_fixture(
            "batch-a",
            "runner-a",
            vec![PerfRun {
                run_id: "run-a".into(),
                batch_id: "batch-a".into(),
                process_exit_success: true,
                schema_valid: true,
                scenario_config_sha256: "config".into(),
                metrics: BTreeMap::from([("p99_us".into(), 1.0)]),
                counters: REQUIRED_COUNTERS
                    .into_iter()
                    .map(|counter| (counter.into(), 0))
                    .collect(),
                raw_samples: BTreeMap::from([("latency_us".into(), vec![1.0])]),
                errors: vec![],
            }],
        );

        assert!(ReportBatch::from_reports(vec![report])
            .reviewed_manifest_id
            .is_none());
    }

    #[test]
    fn strict_batch_rejects_duplicate_run_ids() {
        let baseline = five_runs("runner-a", "p99_us", [100; 5]);
        let mut candidate = five_runs("runner-a", "p99_us", [100; 5]);
        candidate.runs[4].run_id = candidate.runs[0].run_id.clone();
        assert!(compare(&baseline, &candidate, ComparisonPolicy::strict()).is_err());
    }

    #[test]
    fn coefficient_of_variation_over_ten_percent_is_unstable() {
        let baseline = five_runs("runner-a", "p99_us", [100; 5]);
        let candidate = five_runs("runner-a", "p99_us", [70, 85, 100, 115, 130]);
        let verdict = compare(&baseline, &candidate, ComparisonPolicy::strict()).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Unstable);
    }

    #[test]
    fn unstable_baseline_is_rejected_before_candidate_comparison() {
        let baseline = five_runs("runner-a", "p99_us", [70, 85, 100, 115, 130]);
        let candidate = five_runs("runner-a", "p99_us", [100; 5]);
        assert!(matches!(
            compare(&baseline, &candidate, ComparisonPolicy::strict()),
            Err(CompareError::UnstableBaseline { ref metric, .. }) if metric == "p99_us"
        ));
    }

    #[test]
    fn fail_or_unstable_verdict_cannot_be_used_as_a_strict_baseline() {
        for verdict in [VerdictStatus::Fail, VerdictStatus::Unstable] {
            let mut baseline = five_runs("runner-a", "p99_us", [100; 5]);
            for report in &mut baseline.reports {
                report.verdict = verdict;
            }
            let candidate = five_runs("runner-a", "p99_us", [100; 5]);
            assert!(matches!(
                compare(&baseline, &candidate, ComparisonPolicy::strict()),
                Err(CompareError::Unavailable { .. })
            ));
        }
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
                artifact_path: "baseline.json".into(),
                artifact_sha256: "00bad".into(),
                source_commit: "0123456789abcdef0123456789abcdef01234567".into(),
                approval_ref: "github-pr:owner/repo#1".into(),
            }],
        };
        assert!(matches!(
            load_reviewed_baseline(
                &manifest,
                "windows-control-v3-runner-a",
                &VerifiedApproval::test_fixture("github-pr:owner/repo#1"),
                &directory,
            ),
            Err(BaselineError::ArtifactHashMismatch { .. })
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn baseline_loader_rejects_absolute_and_escape_paths() {
        for artifact_path in [
            std::env::temp_dir()
                .join("outside-baseline.json")
                .display()
                .to_string(),
            "../outside-baseline.json".into(),
        ] {
            let manifest = BaselineManifest {
                baseline: vec![BaselineEntry {
                    id: "id".into(),
                    scenario: "quic-control-v3".into(),
                    scenario_config_sha256: hex_sha256(b"config"),
                    runner_fingerprint: hex_sha256(b"runner"),
                    artifact_path,
                    artifact_sha256: hex_sha256(b"artifact"),
                    source_commit: "0123456789abcdef0123456789abcdef01234567".into(),
                    approval_ref: "github-pr:owner/repo#1".into(),
                }],
            };

            assert!(matches!(
                load_reviewed_baseline(
                    &manifest,
                    "id",
                    &VerifiedApproval::test_fixture("github-pr:owner/repo#1"),
                    &std::env::temp_dir(),
                ),
                Err(BaselineError::InvalidManifestEntry { .. })
            ));
        }
    }

    #[test]
    fn loaded_baseline_must_match_manifest_scenario() {
        let result = load_manifest_mismatch("scenario");
        assert!(
            matches!(
                &result,
                Err(BaselineError::ReportMismatch { field, .. }) if field == "scenario"
            ),
            "{result:?}"
        );
    }

    #[test]
    fn loaded_baseline_must_match_manifest_scenario_config_hash() {
        assert!(matches!(
            load_manifest_mismatch("scenario_config_sha256"),
            Err(BaselineError::ReportMismatch { ref field, .. })
                if field == "scenario_config_sha256"
        ));
    }

    #[test]
    fn loaded_baseline_must_match_manifest_runner_fingerprint() {
        assert!(matches!(
            load_manifest_mismatch("runner_fingerprint"),
            Err(BaselineError::ReportMismatch { ref field, .. })
                if field == "runner_fingerprint"
        ));
    }

    #[test]
    fn loaded_baseline_commit_must_match_manifest_source_commit() {
        assert!(matches!(
            load_manifest_mismatch("source_commit"),
            Err(BaselineError::ReportMismatch { ref field, .. }) if field == "commit"
        ));
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

    #[test]
    fn github_trust_rejects_manifest_repository_mismatch() {
        let (policy, mut entry, local_manifest, evidence) = github_trust_fixture();
        entry.approval_ref = "github-pr:evil/fork#1".into();
        assert!(validate_github_trust(&policy, &entry, &local_manifest, &evidence).is_err());
    }

    #[test]
    fn github_trust_rejects_scattered_strings_without_exact_manifest_entry() {
        let (policy, entry, local_manifest, mut evidence) = github_trust_fixture();
        evidence.pr_head_manifest = "baseline = []".into();
        evidence.changed_files = vec![
            GithubChangedFile {
                filename: "perf/baselines/manifest.toml".into(),
                patch: entry.id.clone(),
            },
            GithubChangedFile {
                filename: "unrelated.txt".into(),
                patch: entry.artifact_sha256.clone(),
            },
        ];
        assert!(validate_github_trust(&policy, &entry, &local_manifest, &evidence).is_err());
    }

    #[test]
    fn github_trust_rejects_approval_for_stale_pr_head() {
        let (policy, entry, local_manifest, mut evidence) = github_trust_fixture();
        evidence.reviews[0].commit_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
        assert!(validate_github_trust(&policy, &entry, &local_manifest, &evidence).is_err());
    }

    #[test]
    fn github_trust_rejects_approval_superseded_by_changes_requested() {
        let (policy, entry, local_manifest, mut evidence) = github_trust_fixture();
        let mut later_review = evidence.reviews[0].clone();
        later_review.review_id += 1;
        later_review.state = "CHANGES_REQUESTED".into();
        evidence.reviews.push(later_review);
        assert!(validate_github_trust(&policy, &entry, &local_manifest, &evidence).is_err());
    }

    #[test]
    fn github_trust_rejects_approval_without_repository_write_permission() {
        let (policy, entry, local_manifest, mut evidence) = github_trust_fixture();
        evidence.reviews[0].repository_permission = "read".into();
        assert!(validate_github_trust(&policy, &entry, &local_manifest, &evidence).is_err());
    }

    #[test]
    fn github_trust_rejects_artifact_not_bound_to_approved_head() {
        let (policy, entry, local_manifest, mut evidence) = github_trust_fixture();
        evidence.pr_head_artifact_sha256 = hex_sha256(b"different artifact");
        assert!(validate_github_trust(&policy, &entry, &local_manifest, &evidence).is_err());
    }

    #[test]
    fn github_trust_rejects_unbound_batch_sidecar() {
        let (policy, entry, local_manifest, mut evidence) = github_trust_fixture();
        evidence.pr_head_batch_artifact_references.insert(
            "perf/baselines/raw/run-1.json".into(),
            hex_sha256(b"expected"),
        );
        evidence.pr_head_batch_artifact_sha256.insert(
            "perf/baselines/raw/run-1.json".into(),
            hex_sha256(b"different"),
        );
        assert!(validate_github_trust(&policy, &entry, &local_manifest, &evidence).is_err());
    }

    #[test]
    fn github_trust_rejects_comment_only_pr_when_base_already_has_exact_entry() {
        let (policy, entry, local_manifest, mut evidence) = github_trust_fixture();
        evidence.pull_request_base_sha = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
        evidence.pr_base_manifest = evidence.pr_head_manifest.clone();
        evidence.changed_files = vec![GithubChangedFile {
            filename: "perf/baselines/manifest.toml".into(),
            patch: "@@ -1 +1 @@\n-# old comment\n+# new comment".into(),
        }];

        assert!(validate_github_trust(&policy, &entry, &local_manifest, &evidence).is_err());
    }

    #[test]
    fn github_trust_accepts_exact_entry_hash_changed_from_base_to_approved_head() {
        let (policy, entry, local_manifest, mut evidence) = github_trust_fixture();
        let mut old_entry = entry.clone();
        old_entry.artifact_sha256 = hex_sha256(b"old-artifact");
        evidence.pull_request_base_sha = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
        evidence.pr_base_manifest = toml::to_string(&BaselineManifest {
            baseline: vec![old_entry],
        })
        .unwrap();

        assert!(validate_github_trust(&policy, &entry, &local_manifest, &evidence).is_ok());
    }

    #[test]
    fn github_trust_rejects_pr_merged_into_a_non_default_side_branch() {
        let (policy, entry, local_manifest, mut evidence) = github_trust_fixture();
        evidence.pull_request_base_ref = "release-side-branch".into();

        assert!(validate_github_trust(&policy, &entry, &local_manifest, &evidence).is_err());
    }

    #[test]
    fn github_trust_rejects_merge_commit_not_reachable_from_default_branch() {
        let (policy, entry, local_manifest, mut evidence) = github_trust_fixture();
        evidence.merge_commit_reachable_from_default = false;

        assert!(validate_github_trust(&policy, &entry, &local_manifest, &evidence).is_err());
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
                    raw_samples: BTreeMap::from([("latency_us".into(), vec![*value as f64])]),
                    errors: vec![],
                };
                let mut report = PerfReport::test_fixture("batch-a", runner, vec![run]);
                report.batch_artifacts.push(BatchArtifactReference {
                    batch_id: "batch-a".into(),
                    path: format!("raw/run-{index}.json"),
                    sha256: hex_sha256(format!("raw-{index}").as_bytes()),
                    verdict: VerdictStatus::Pass,
                });
                report
            })
            .collect();
        let mut batch = ReportBatch::from_reports(reports);
        batch.reviewed_manifest_id = Some("reviewed-test-baseline".into());
        batch
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

    fn load_manifest_mismatch(field: &str) -> Result<PerfReport, BaselineError> {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("rshare-perf-manifest-{suffix}"));
        fs::create_dir_all(&directory).unwrap();
        let artifact = directory.join("baseline.json");
        let report = five_runs("runner-a", "p99_us", [100; 5]).reports[0].clone();
        let bytes = serde_json::to_vec(&report).unwrap();
        fs::write(&artifact, &bytes).unwrap();
        let mut entry = BaselineEntry {
            id: "id".into(),
            scenario: report.scenario.clone(),
            scenario_config_sha256: report.scenario_config_sha256.clone(),
            runner_fingerprint: report.runner_fingerprint.clone(),
            artifact_path: "baseline.json".into(),
            artifact_sha256: hex_sha256(&bytes),
            source_commit: report.commit.clone(),
            approval_ref: "github-pr:owner/repo#1".into(),
        };
        match field {
            "scenario" => entry.scenario = "different-scenario".into(),
            "scenario_config_sha256" => entry.scenario_config_sha256 = hex_sha256(b"different"),
            "runner_fingerprint" => entry.runner_fingerprint = hex_sha256(b"different"),
            "source_commit" => {
                entry.source_commit = "ffffffffffffffffffffffffffffffffffffffff".into()
            }
            _ => unreachable!(),
        }
        let manifest = BaselineManifest {
            baseline: vec![entry],
        };
        let result = load_reviewed_baseline(
            &manifest,
            "id",
            &VerifiedApproval::test_fixture("github-pr:owner/repo#1"),
            &directory,
        );
        fs::remove_dir_all(directory).unwrap();
        result
    }

    fn github_trust_fixture() -> (GithubTrustPolicy, BaselineEntry, Vec<u8>, GithubApiEvidence) {
        let entry = BaselineEntry {
            id: "id".into(),
            scenario: "quic-control-v3".into(),
            scenario_config_sha256: hex_sha256(b"config"),
            runner_fingerprint: hex_sha256(b"runner"),
            artifact_path: "perf/baselines/artifact.json".into(),
            artifact_sha256: hex_sha256(b"artifact"),
            source_commit: "0123456789abcdef0123456789abcdef01234567".into(),
            approval_ref: "github-pr:owner/repo#1".into(),
        };
        let manifest = BaselineManifest {
            baseline: vec![entry.clone()],
        };
        let manifest_text = toml::to_string(&manifest).unwrap();
        (
            GithubTrustPolicy {
                expected_repository: "owner/repo".into(),
                manifest_path: "perf/baselines/manifest.toml".into(),
            },
            entry,
            manifest_text.as_bytes().to_vec(),
            GithubApiEvidence {
                repository: "owner/repo".into(),
                default_branch: "main".into(),
                default_branch_protected: true,
                default_branch_manifest: manifest_text.clone(),
                pull_request_number: 1,
                pull_request_merged: true,
                pull_request_author: "author".into(),
                pull_request_base_repository: "owner/repo".into(),
                pull_request_base_ref: "main".into(),
                pull_request_base_sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
                pull_request_head_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                merge_commit_sha: "cccccccccccccccccccccccccccccccccccccccc".into(),
                merge_commit_reachable_from_default: true,
                pr_base_manifest: toml::to_string(&BaselineManifest { baseline: vec![] }).unwrap(),
                pr_head_manifest: manifest_text,
                pr_head_artifact_sha256: hex_sha256(b"artifact"),
                pr_head_batch_artifact_references: BTreeMap::new(),
                pr_head_batch_artifact_sha256: BTreeMap::new(),
                reviews: vec![GithubReview {
                    review_id: 1,
                    reviewer: "reviewer".into(),
                    state: "APPROVED".into(),
                    commit_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                    repository_permission: "write".into(),
                }],
                changed_files: vec![
                    GithubChangedFile {
                        filename: "perf/baselines/manifest.toml".into(),
                        patch: "id artifact".into(),
                    },
                    GithubChangedFile {
                        filename: "perf/baselines/artifact.json".into(),
                        patch: "baseline artifact".into(),
                    },
                ],
            },
        )
    }

    #[test]
    fn github_pagination_preserves_evidence_after_first_hundred_items() {
        let first_page = (0..100)
            .map(|index| serde_json::json!({ "id": index }))
            .collect::<Vec<_>>();
        let second_page = vec![serde_json::json!({
            "id": 100,
            "state": "APPROVED",
        })];

        let items = flatten_paginated_items(
            serde_json::json!([first_page, second_page]),
            "pull request reviews",
        )
        .unwrap();

        assert_eq!(items.len(), 101);
        assert_eq!(items[100]["state"], "APPROVED");
    }
}
