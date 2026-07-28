use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};

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
    #[serde(skip)]
    pub(crate) local_schema_validated: bool,
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
    pub binary_roles: Vec<String>,
}

impl ScenarioContract {
    pub fn for_scenario(scenario: &str) -> Result<Self, ReportError> {
        let (metrics, binary_roles) = match scenario {
            "quic-control-v3" => (vec!["median_us", "p95_us", "p99_us"], vec!["rshare-perf"]),
            "daemon-control" => (
                vec!["median_us", "p95_us", "p99_us"],
                vec!["rshare-daemon", "rshare-perf"],
            ),
            "desktop-control" | "windows-media" => (
                vec!["median_us", "p95_us", "p99_us"],
                vec!["rshare-daemon", "rshare-desktop", "rshare-perf"],
            ),
            other => return Err(ReportError::UnknownScenario(other.into())),
        };
        Ok(Self {
            metrics: metrics.into_iter().map(str::to_string).collect(),
            counters: REQUIRED_COUNTERS
                .iter()
                .map(|counter| counter.to_string())
                .collect(),
            binary_roles: binary_roles.into_iter().map(str::to_string).collect(),
        })
    }

    pub fn for_report(report: &PerfReport) -> Result<Self, ReportError> {
        let mut contract = Self::for_scenario(&report.scenario)?;
        if report.scenario == "quic-control-v3" {
            match report
                .scenario_parameters
                .get("kind")
                .and_then(Value::as_str)
            {
                Some("stall_recovery") => contract.metrics.push("stall_recovery_us".into()),
                Some("slow_fast_peer_isolation") => {
                    contract.metrics.push("fast_peer_p99_us".into())
                }
                _ => {}
            }
        }
        Ok(contract)
    }
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
    #[error("scenario {0} has no predefined performance contract")]
    UnknownScenario(String),
    #[error("complete report requires exactly five runs, got {actual}")]
    RunCount { actual: usize },
    #[error("complete report repeats run id {0}")]
    DuplicateRunId(String),
    #[error("report was not validated locally against the repository JSON schema")]
    LocalSchemaValidationMissing,
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
        if !self.local_schema_validated {
            return Err(ReportError::LocalSchemaValidationMissing);
        }
        self.validate_reproducibility()?;
        if self.runs.len() != 5 {
            return Err(ReportError::RunCount {
                actual: self.runs.len(),
            });
        }
        let mut run_ids = HashSet::with_capacity(5);
        for run in &self.runs {
            if !run_ids.insert(run.run_id.as_str()) {
                return Err(ReportError::DuplicateRunId(run.run_id.clone()));
            }
        }
        for role in &contract.binary_roles {
            if !self.binary_sha256.contains_key(role) {
                return Err(ReportError::InvalidFingerprint {
                    field: format!("binary_sha256.{role}"),
                });
            }
        }

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
            local_schema_validated: true,
        }
    }

    pub fn locally_schema_validated(&self) -> bool {
        self.local_schema_validated
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
    let validator =
        jsonschema::validator_for(schema).map_err(|error| ReportError::SchemaValidation {
            reason: format!("invalid schema: {error}"),
        })?;
    validator
        .validate(value)
        .map_err(|error| ReportError::SchemaValidation {
            reason: error.to_string(),
        })
}

pub fn parse_and_validate_report(bytes: &[u8], schema: &Value) -> Result<PerfReport, ReportError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| ReportError::SchemaValidation {
            reason: error.to_string(),
        })?;
    validate_json_schema(&value, schema)?;
    let mut report: PerfReport =
        serde_json::from_value(value).map_err(|error| ReportError::SchemaValidation {
            reason: error.to_string(),
        })?;
    report.local_schema_validated = true;
    for run in &mut report.runs {
        run.schema_valid = true;
    }
    Ok(report)
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

    #[test]
    fn schema_validation_rejects_nested_unknown_and_wrong_type() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["nested"],
            "properties": {
                "nested": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["count"],
                    "properties": {"count": {"type": "integer", "minimum": 1}}
                }
            }
        });
        assert!(validate_json_schema(
            &json!({"nested": {"count": "one", "unexpected": true}}),
            &schema
        )
        .is_err());
    }

    #[test]
    fn schema_validation_enforces_refs_patterns_and_min_lengths() {
        let schema = json!({
            "$defs": {
                "fingerprint": {
                    "type": "string",
                    "minLength": 64,
                    "pattern": "^[0-9a-f]{64}$"
                }
            },
            "type": "object",
            "required": ["fingerprint"],
            "properties": {
                "fingerprint": {"$ref": "#/$defs/fingerprint"}
            }
        });
        assert!(validate_json_schema(&json!({"fingerprint": "not-a-hash"}), &schema).is_err());
    }

    #[test]
    fn scenario_contracts_predeclare_metrics_and_participating_binary_roles() {
        let loopback = ScenarioContract::for_scenario("quic-control-v3").unwrap();
        assert_eq!(loopback.metrics, vec!["median_us", "p95_us", "p99_us"]);
        assert_eq!(loopback.binary_roles, vec!["rshare-perf"]);

        let daemon = ScenarioContract::for_scenario("daemon-control").unwrap();
        assert_eq!(daemon.binary_roles, vec!["rshare-daemon", "rshare-perf"]);
        assert!(ScenarioContract::for_scenario("unknown").is_err());
    }

    #[test]
    fn complete_report_requires_exactly_five_unique_run_ids() {
        let mut report = valid_report();
        let run = report.runs[0].clone();
        report.runs = vec![run; 5];
        assert!(report.validate_complete(&required_contract()).is_err());
    }

    #[test]
    fn malicious_schema_valid_true_cannot_bypass_local_schema_validation() {
        let schema: Value =
            serde_json::from_str(include_str!("../../../perf/baselines/schema.json")).unwrap();
        let mut value = serde_json::to_value(valid_report()).unwrap();
        value["runner_fingerprint"] = Value::String("malicious".into());
        value["runs"][0]["schema_valid"] = Value::Bool(true);
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(parse_and_validate_report(&bytes, &schema).is_err());
    }

    #[test]
    fn local_schema_validation_derives_run_validation_state() {
        let schema: Value =
            serde_json::from_str(include_str!("../../../perf/baselines/schema.json")).unwrap();
        let bytes = serde_json::to_vec(&valid_report()).unwrap();
        let report = parse_and_validate_report(&bytes, &schema).unwrap();
        assert!(report.locally_schema_validated());
        assert!(report.runs.iter().all(|run| run.schema_valid));
    }

    fn required_contract() -> ScenarioContract {
        ScenarioContract {
            metrics: vec!["latency_us".into()],
            counters: REQUIRED_COUNTERS
                .iter()
                .map(|value| value.to_string())
                .collect(),
            binary_roles: vec!["rshare-perf".into()],
        }
    }

    fn valid_report() -> PerfReport {
        let mut counters = BTreeMap::new();
        for counter in REQUIRED_COUNTERS {
            counters.insert(counter.to_string(), 0);
        }
        let mut run_metrics = BTreeMap::new();
        run_metrics.insert("latency_us".into(), 100.0);
        let runs = (1..=5)
            .map(|index| PerfRun {
                run_id: format!("run-{index}"),
                batch_id: "batch-a".into(),
                process_exit_success: true,
                schema_valid: true,
                scenario_config_sha256: "config".into(),
                metrics: run_metrics.clone(),
                counters: counters.clone(),
                errors: vec![],
            })
            .collect();
        PerfReport::test_fixture("batch-a", "runner-a", runs)
    }
}
