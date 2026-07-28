use crate::report::{
    scenario_config_sha256, Availability, DurationSpec, HardwareFingerprint, PerfReport,
    ToolchainFingerprint, VerdictStatus, PERF_SCHEMA_VERSION,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub fn not_run_without_framed_ipc() -> PerfReport {
    let parameters = json!({"reason": "framed daemon IPC seam is scheduled for Task 14"});
    PerfReport {
        schema_version: PERF_SCHEMA_VERSION,
        scenario: "daemon-control".into(),
        scenario_parameters: serde_json::from_value(parameters.clone()).unwrap_or_default(),
        scenario_config_sha256: scenario_config_sha256(&parameters)
            .expect("static scenario is serializable"),
        random_seed: 0,
        commit: "unavailable".into(),
        dirty: false,
        binary_sha256: BTreeMap::from([("rshare-perf".into(), digest("unavailable-binary"))]),
        cargo_lock_sha256: digest("unavailable-lockfile"),
        build_profile: "unavailable".into(),
        cargo_features: vec![],
        rustflags: String::new(),
        runner_id: "unavailable".into(),
        runner_fingerprint: digest("unavailable-runner"),
        availability: Availability::NotRun {
            reason:
                "real daemon IPC benchmark is deferred until Task 14; no fake echo server was used"
                    .into(),
        },
        toolchain: ToolchainFingerprint {
            rustc: "unavailable".into(),
            cargo: "unavailable".into(),
            target: std::env::consts::ARCH.into(),
        },
        hardware: HardwareFingerprint {
            os: std::env::consts::OS.into(),
            cpu: "unavailable".into(),
            logical_cores: std::thread::available_parallelism()
                .map(|value| value.get() as u32)
                .unwrap_or(0),
            memory_bytes: 0,
        },
        warmup: DurationSpec { millis: 0 },
        batch_artifacts: vec![],
        runs: vec![],
        metrics: BTreeMap::new(),
        queues: BTreeMap::new(),
        errors: vec!["framed daemon IPC seam is not implemented".into()],
        rss: None,
        measurement_provenance: BTreeMap::new(),
        verdict: VerdictStatus::NotRun,
        local_schema_validated: false,
    }
}

fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{Availability, VerdictStatus};

    #[test]
    fn daemon_control_is_fail_closed_until_task_14() {
        let report = not_run_without_framed_ipc();
        assert!(matches!(report.availability, Availability::NotRun { .. }));
        assert_eq!(report.verdict, VerdictStatus::NotRun);
    }
}
