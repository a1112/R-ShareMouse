use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const PERF_SCHEMA_VERSION: u16 = 1;
pub const REQUIRED_COUNTERS: [&str; 5] = [
    "overwrite",
    "gap",
    "duplicate",
    "out_of_order",
    "reliable_overflow",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PerfReport {
    pub schema_version: u16,
    pub scenario: String,
    pub scenario_parameters: BTreeMap<String, Value>,
    pub scenario_config_sha256: String,
    pub random_seed: u64,
    pub commit: String,
    pub dirty: bool,
    pub binary_sha256: BTreeMap<String, String>,
    pub cargo_lock_sha256: String,
    pub build_profile: String,
    pub cargo_features: Vec<String>,
    pub rustflags: String,
    pub runner_id: String,
    pub runner_fingerprint: String,
    pub availability: Availability,
    pub toolchain: ToolchainFingerprint,
    pub hardware: HardwareFingerprint,
    pub warmup: DurationSpec,
    pub runs: Vec<PerfRun>,
    pub metrics: BTreeMap<String, MetricSummary>,
    pub queues: BTreeMap<String, QueueSummary>,
    pub errors: Vec<String>,
    pub rss: Option<RssSummary>,
    pub measurement_provenance: BTreeMap<String, MeasurementProvenance>,
    pub verdict: VerdictStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PerfRun {
    pub run_id: String,
    pub batch_id: String,
    pub process_exit_success: bool,
    pub schema_valid: bool,
    pub scenario_config_sha256: String,
    pub metrics: BTreeMap<String, f64>,
    pub counters: BTreeMap<String, u64>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolchainFingerprint {
    pub rustc: String,
    pub cargo: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HardwareFingerprint {
    pub os: String,
    pub cpu: String,
    pub logical_cores: u32,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DurationSpec {
    pub millis: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MetricSummary {
    pub unit: String,
    pub samples: u64,
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub coefficient_of_variation: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QueueSummary {
    pub capacity: u64,
    pub high_watermark: u64,
    pub overwrites: u64,
    pub overflows: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RssSummary {
    pub peak_bytes: u64,
    pub method: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MeasurementProvenance {
    pub method: String,
    pub uncertainty_us: Option<u64>,
    pub evidence_path: Option<String>,
    pub evidence_sha256: Option<String>,
    pub estimate_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Availability {
    Available,
    Unsupported { reason: String },
    NotRun { reason: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerdictStatus {
    Pass,
    Fail,
    Unstable,
    Unsupported,
    NotRun,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioContract {
    pub metrics: Vec<String>,
    pub counters: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReportError {
    #[error("report schema version {actual} is not supported; expected {expected}")]
    SchemaVersion { expected: u16, actual: u16 },
    #[error("report is unavailable: {reason}")]
    Unavailable { reason: String },
    #[error("run {run_id} did not complete: {reason}")]
    IncompleteRun { run_id: String, reason: String },
    #[error("run {run_id} has scenario configuration {actual}, expected {expected}")]
    ScenarioConfigMismatch {
        run_id: String,
        expected: String,
        actual: String,
    },
    #[error("run {run_id} is missing metric {metric}")]
    MissingMetric { run_id: String, metric: String },
    #[error("run {run_id} is missing counter {counter}")]
    MissingCounter { run_id: String, counter: String },
    #[error("invalid reproducibility field: {field}")]
    InvalidFingerprint { field: String },
    #[error("JSON schema validation failed: {reason}")]
    SchemaValidation { reason: String },
}

impl PerfReport {
    pub fn validate_complete(&self, contract: &ScenarioContract) -> Result<(), ReportError> {
        if self.schema_version != PERF_SCHEMA_VERSION {
            return Err(ReportError::SchemaVersion {
                expected: PERF_SCHEMA_VERSION,
                actual: self.schema_version,
            });
        }
        match &self.availability {
            Availability::Available => {}
            Availability::Unsupported { reason } | Availability::NotRun { reason } => {
                return Err(ReportError::Unavailable {
                    reason: reason.clone(),
                });
            }
        }
        if matches!(
            self.verdict,
            VerdictStatus::Unsupported | VerdictStatus::NotRun
        ) {
            return Err(ReportError::Unavailable {
                reason: format!("verdict is {:?}", self.verdict),
            });
        }
        self.validate_reproducibility()?;

        for run in &self.runs {
            if !run.process_exit_success {
                return Err(ReportError::IncompleteRun {
                    run_id: run.run_id.clone(),
                    reason: "process exit was unsuccessful".into(),
                });
            }
            if !run.schema_valid {
                return Err(ReportError::IncompleteRun {
                    run_id: run.run_id.clone(),
                    reason: "artifact did not validate against schema".into(),
                });
            }
            if !run.errors.is_empty() {
                return Err(ReportError::IncompleteRun {
                    run_id: run.run_id.clone(),
                    reason: run.errors.join("; "),
                });
            }
            if run.scenario_config_sha256 != self.scenario_config_sha256 {
                return Err(ReportError::ScenarioConfigMismatch {
                    run_id: run.run_id.clone(),
                    expected: self.scenario_config_sha256.clone(),
                    actual: run.scenario_config_sha256.clone(),
                });
            }
            for metric in &contract.metrics {
                if !run.metrics.contains_key(metric) {
                    return Err(ReportError::MissingMetric {
                        run_id: run.run_id.clone(),
                        metric: metric.clone(),
                    });
                }
            }
            for counter in &contract.counters {
                if !run.counters.contains_key(counter) {
                    return Err(ReportError::MissingCounter {
                        run_id: run.run_id.clone(),
                        counter: counter.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    pub fn validate_reproducibility(&self) -> Result<(), ReportError> {
        let fields = [
            ("scenario", self.scenario.as_str()),
            (
                "scenario_config_sha256",
                self.scenario_config_sha256.as_str(),
            ),
            ("commit", self.commit.as_str()),
            ("cargo_lock_sha256", self.cargo_lock_sha256.as_str()),
            ("build_profile", self.build_profile.as_str()),
            ("runner_id", self.runner_id.as_str()),
            ("runner_fingerprint", self.runner_fingerprint.as_str()),
        ];
        for (name, value) in fields {
            if value.trim().is_empty() {
                return Err(ReportError::InvalidFingerprint { field: name.into() });
            }
        }
        if !is_sha256(&self.scenario_config_sha256)
            || !is_sha256(&self.cargo_lock_sha256)
            || !is_sha256(&self.runner_fingerprint)
            || !is_commit(&self.commit)
        {
            return Err(ReportError::InvalidFingerprint {
                field: "hash_or_commit".into(),
            });
        }
        let parameters =
            serde_json::to_value(&self.scenario_parameters).expect("scenario parameters serialize");
        if scenario_config_sha256(&parameters).expect("scenario parameters hash")
            != self.scenario_config_sha256
        {
            return Err(ReportError::InvalidFingerprint {
                field: "scenario_config_sha256_integrity".into(),
            });
        }
        if self.binary_sha256.is_empty()
            || self
                .binary_sha256
                .iter()
                .any(|(role, hash)| role.trim().is_empty() || !is_sha256(hash))
        {
            return Err(ReportError::InvalidFingerprint {
                field: "binary_sha256".into(),
            });
        }
        if self.cargo_features != normalize_features(self.cargo_features.clone()) {
            return Err(ReportError::InvalidFingerprint {
                field: "cargo_features".into(),
            });
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn test_fixture(_batch_id: &str, runner: &str, mut runs: Vec<PerfRun>) -> Self {
        let scenario_parameters =
            BTreeMap::from([("fixture".into(), Value::String("config".into()))]);
        let config_hash = scenario_config_sha256(
            &serde_json::to_value(&scenario_parameters).expect("fixture parameters serialize"),
        )
        .expect("fixture parameters hash");
        for run in &mut runs {
            if run.scenario_config_sha256 == "config" {
                run.scenario_config_sha256 = config_hash.clone();
            }
        }
        let mut binaries = BTreeMap::new();
        binaries.insert("rshare-perf".into(), digest_text("binary"));
        Self {
            schema_version: PERF_SCHEMA_VERSION,
            scenario: "quic-control-v3".into(),
            scenario_parameters,
            scenario_config_sha256: config_hash,
            random_seed: 7,
            commit: "0123456789abcdef0123456789abcdef01234567".into(),
            dirty: false,
            binary_sha256: binaries,
            cargo_lock_sha256: digest_text("lock"),
            build_profile: "release".into(),
            cargo_features: vec!["control".into()],
            rustflags: "-C target-cpu=x86-64-v3".into(),
            runner_id: runner.into(),
            runner_fingerprint: digest_text(runner),
            availability: Availability::Available,
            toolchain: ToolchainFingerprint {
                rustc: "rustc 1.94.1".into(),
                cargo: "cargo 1.94.1".into(),
                target: "x86_64-pc-windows-msvc".into(),
            },
            hardware: HardwareFingerprint {
                os: "windows".into(),
                cpu: "fixture".into(),
                logical_cores: 8,
                memory_bytes: 16 * 1024 * 1024 * 1024,
            },
            warmup: DurationSpec { millis: 1_000 },
            runs,
            metrics: BTreeMap::new(),
            queues: BTreeMap::new(),
            errors: vec![],
            rss: None,
            measurement_provenance: BTreeMap::new(),
            verdict: VerdictStatus::Pass,
        }
    }
}

pub fn normalize_features(features: Vec<String>) -> Vec<String> {
    features
        .into_iter()
        .map(|feature| feature.trim().to_string())
        .filter(|feature| !feature.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn scenario_config_sha256(value: &Value) -> Result<String, serde_json::Error> {
    let canonical = canonicalize_json(value);
    let bytes = serde_json::to_vec(&canonical)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub fn validate_json_schema(value: &Value, schema: &Value) -> Result<(), ReportError> {
    let object = value
        .as_object()
        .ok_or_else(|| ReportError::SchemaValidation {
            reason: "artifact must be a JSON object".into(),
        })?;
    let schema_object = schema
        .as_object()
        .ok_or_else(|| ReportError::SchemaValidation {
            reason: "schema must be a JSON object".into(),
        })?;
    let required = schema_object
        .get("required")
        .and_then(Value::as_array)
        .ok_or_else(|| ReportError::SchemaValidation {
            reason: "schema must declare required fields".into(),
        })?;
    for field in required {
        let field = field
            .as_str()
            .ok_or_else(|| ReportError::SchemaValidation {
                reason: "required field names must be strings".into(),
            })?;
        if !object.contains_key(field) {
            return Err(ReportError::SchemaValidation {
                reason: format!("missing required field {field}"),
            });
        }
    }
    if let Some(expected) = schema_object
        .get("properties")
        .and_then(|value| value.get("schema_version"))
        .and_then(|value| value.get("const"))
    {
        if object.get("schema_version") != Some(expected) {
            return Err(ReportError::SchemaValidation {
                reason: "schema_version does not match the schema".into(),
            });
        }
    }
    Ok(())
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<_, _> = map
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize_json(value)))
                .collect();
            serde_json::to_value(sorted).expect("BTreeMap JSON conversion is infallible")
        }
        Value::Array(values) => Value::Array(values.iter().map(canonicalize_json).collect()),
        scalar => scalar.clone(),
    }
}

#[cfg(test)]
fn digest_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_commit(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn report_canonicalizes_recursive_configuration_and_features() {
        let left = json!({
            "load": ["diagnostics", "audio"],
            "transport": {"duration_secs": 60, "rate_hz": 1000}
        });
        let right = json!({
            "transport": {"rate_hz": 1000, "duration_secs": 60},
            "load": ["diagnostics", "audio"]
        });

        assert_eq!(
            scenario_config_sha256(&left).unwrap(),
            scenario_config_sha256(&right).unwrap()
        );
        assert_eq!(
            normalize_features(vec!["media".into(), "control".into(), "media".into()]),
            vec!["control", "media"]
        );
    }

    #[test]
    fn complete_report_requires_all_loss_and_ordering_counters() {
        let mut report = valid_report();
        report.runs[0].counters.remove("reliable_overflow");
        assert!(matches!(
            report.validate_complete(&required_contract()),
            Err(ReportError::MissingCounter { ref counter, .. })
                if counter == "reliable_overflow"
        ));
    }

    #[test]
    fn unsupported_and_not_run_reports_are_not_complete() {
        let mut report = valid_report();
        report.availability = Availability::Unsupported {
            reason: "no fixed runner".into(),
        };
        report.verdict = VerdictStatus::Unsupported;
        assert!(matches!(
            report.validate_complete(&required_contract()),
            Err(ReportError::Unavailable { .. })
        ));

        report.availability = Availability::NotRun {
            reason: "review unavailable".into(),
        };
        report.verdict = VerdictStatus::NotRun;
        assert!(matches!(
            report.validate_complete(&required_contract()),
            Err(ReportError::Unavailable { .. })
        ));
    }

    #[test]
    fn schema_validation_rejects_missing_required_fields() {
        let schema = json!({
            "type": "object",
            "required": ["schema_version", "scenario", "runs"],
            "properties": {"schema_version": {"const": 1}}
        });
        let incomplete = json!({"schema_version": 1, "scenario": "quic-control-v3"});
        assert!(matches!(
            validate_json_schema(&incomplete, &schema),
            Err(ReportError::SchemaValidation { .. })
        ));
    }

    fn required_contract() -> ScenarioContract {
        ScenarioContract {
            metrics: vec!["latency_us".into()],
            counters: REQUIRED_COUNTERS
                .iter()
                .map(|value| value.to_string())
                .collect(),
        }
    }

    fn valid_report() -> PerfReport {
        let mut counters = BTreeMap::new();
        for counter in REQUIRED_COUNTERS {
            counters.insert(counter.to_string(), 0);
        }
        let mut run_metrics = BTreeMap::new();
        run_metrics.insert("latency_us".into(), 100.0);
        PerfReport::test_fixture(
            "batch-a",
            "runner-a",
            vec![PerfRun {
                run_id: "run-1".into(),
                batch_id: "batch-a".into(),
                process_exit_success: true,
                schema_valid: true,
                scenario_config_sha256: "config".into(),
                metrics: run_metrics,
                counters,
                errors: vec![],
            }],
        )
    }
}
