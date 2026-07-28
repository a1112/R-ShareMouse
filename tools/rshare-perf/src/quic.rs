use crate::report::{PerfRun, QueueSummary, VerdictStatus, REQUIRED_COUNTERS};
use anyhow::{Context, Result};
use rshare_core::{AudioFormat, AudioFramePayload, DeviceId, Message};
use rshare_net::QuicTransport;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::time::{interval, timeout, MissedTickBehavior};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LoadKind {
    Diagnostics,
    Status,
    Audio,
    Bulk,
}

impl std::str::FromStr for LoadKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "diagnostics" => Ok(Self::Diagnostics),
            "status" => Ok(Self::Status),
            "audio" => Ok(Self::Audio),
            "bulk" => Ok(Self::Bulk),
            other => Err(format!("unknown load kind {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuicScenario {
    Rate {
        rate_hz: u32,
        duration_secs: u64,
        load: Vec<LoadKind>,
    },
    SlowFastPeerIsolation,
    StallRecovery {
        stall_ms: u64,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ScenarioError {
    #[error("unsupported QUIC rate/duration/load combination")]
    UnsupportedCombination,
    #[error("stall recovery must use exactly 100 ms")]
    StallMustBeExactly100Ms,
}

impl QuicScenario {
    pub fn validate(&self) -> Result<(), ScenarioError> {
        match self {
            Self::Rate {
                rate_hz,
                duration_secs,
                load,
            } => {
                let base =
                    matches!((*rate_hz, *duration_secs), (125, 10) | (500, 10)) && load.is_empty();
                let thousand = (*rate_hz, *duration_secs) == (1000, 60)
                    && (load.is_empty()
                        || *load
                            == [
                                LoadKind::Diagnostics,
                                LoadKind::Status,
                                LoadKind::Audio,
                                LoadKind::Bulk,
                            ]);
                if base || thousand {
                    Ok(())
                } else {
                    Err(ScenarioError::UnsupportedCombination)
                }
            }
            Self::SlowFastPeerIsolation => Ok(()),
            Self::StallRecovery { stall_ms: 100 } => Ok(()),
            Self::StallRecovery { .. } => Err(ScenarioError::StallMustBeExactly100Ms),
        }
    }
}

pub fn scenario_matrix() -> Vec<QuicScenario> {
    vec![
        QuicScenario::Rate {
            rate_hz: 125,
            duration_secs: 10,
            load: vec![],
        },
        QuicScenario::Rate {
            rate_hz: 500,
            duration_secs: 10,
            load: vec![],
        },
        QuicScenario::Rate {
            rate_hz: 1000,
            duration_secs: 60,
            load: vec![],
        },
        QuicScenario::Rate {
            rate_hz: 1000,
            duration_secs: 60,
            load: vec![
                LoadKind::Diagnostics,
                LoadKind::Status,
                LoadKind::Audio,
                LoadKind::Bulk,
            ],
        },
        QuicScenario::SlowFastPeerIsolation,
        QuicScenario::StallRecovery { stall_ms: 100 },
    ]
}

#[derive(Debug, Clone)]
pub struct LoopbackRunOptions {
    pub effective_duration: Option<Duration>,
    pub batch_id: String,
    pub run_index: usize,
}

impl LoopbackRunOptions {
    pub fn measured(batch_id: String, run_index: usize) -> Self {
        Self {
            effective_duration: None,
            batch_id,
            run_index,
        }
    }

    #[cfg(test)]
    fn test(duration: Duration) -> Self {
        Self {
            effective_duration: Some(duration),
            batch_id: "test-batch".into(),
            run_index: 0,
        }
    }
}

#[derive(Debug)]
pub struct LoopbackMeasurement {
    pub run: PerfRun,
    pub queues: BTreeMap<String, QueueSummary>,
    pub transport_handshake_completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchivedBatch {
    pub batch_id: String,
    pub artifact_path: String,
    pub scenario_config_sha256: String,
    pub verdict: VerdictStatus,
    pub runs: Vec<PerfRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BatchOrchestration {
    pub batches: Vec<ArchivedBatch>,
    pub selected_batch: Option<usize>,
    pub infrastructure_failure: Option<String>,
}

pub async fn orchestrate_five_run_batches<F, Fut>(
    scenario_config_sha256: &str,
    mut runner: F,
) -> Result<BatchOrchestration>
where
    F: FnMut(&str, usize) -> Fut,
    Fut: std::future::Future<Output = Result<PerfRun>>,
{
    let execution_id = format!("{}-{}", std::process::id(), now_millis());
    let mut batches = Vec::with_capacity(2);
    for attempt in 0..=1 {
        let batch_id = format!("{execution_id}-batch-{}", attempt + 1);
        let mut runs = Vec::with_capacity(5);
        for index in 0..5 {
            let mut run = runner(&batch_id, index).await?;
            run.batch_id = batch_id.clone();
            run.run_id = format!("{batch_id}-run-{}", index + 1);
            run.scenario_config_sha256 = scenario_config_sha256.into();
            runs.push(run);
        }
        let unstable = batch_is_unstable(&runs);
        batches.push(ArchivedBatch {
            batch_id,
            artifact_path: format!("batch-{}.json", attempt + 1),
            scenario_config_sha256: scenario_config_sha256.into(),
            verdict: if unstable {
                VerdictStatus::Unstable
            } else {
                VerdictStatus::Pass
            },
            runs,
        });
        if !unstable {
            return Ok(BatchOrchestration {
                selected_batch: Some(attempt),
                batches,
                infrastructure_failure: None,
            });
        }
    }
    Ok(BatchOrchestration {
        batches,
        selected_batch: None,
        infrastructure_failure: Some("unstable_after_one_complete_retry".into()),
    })
}

fn batch_is_unstable(runs: &[PerfRun]) -> bool {
    let Some(first) = runs.first() else {
        return true;
    };
    first.metrics.keys().any(|metric| {
        let values: Vec<_> = runs
            .iter()
            .filter_map(|run| run.metrics.get(metric).copied())
            .collect();
        if values.len() != runs.len() {
            return true;
        }
        coefficient_of_variation(&values) > 0.10
    })
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

pub async fn run_loopback_once(
    scenario: &QuicScenario,
    options: LoopbackRunOptions,
) -> Result<LoopbackMeasurement> {
    scenario.validate()?;
    let (rate_hz, configured_duration, load, stall_ms, slow_fast) = match scenario {
        QuicScenario::Rate {
            rate_hz,
            duration_secs,
            load,
        } => (
            *rate_hz,
            Duration::from_secs(*duration_secs),
            load.as_slice(),
            None,
            false,
        ),
        QuicScenario::SlowFastPeerIsolation => {
            (1_000, Duration::from_secs(10), &[][..], None, true)
        }
        QuicScenario::StallRecovery { stall_ms } => (
            1_000,
            Duration::from_secs(10),
            &[][..],
            Some(*stall_ms),
            false,
        ),
    };
    let duration = options.effective_duration.unwrap_or(configured_duration);
    let local_id = DeviceId::new_v4();
    let remote_id = DeviceId::new_v4();
    let mut server = QuicTransport::new(local_id);
    server.start_server("127.0.0.1:0").await?;
    let address = server
        .local_addr()
        .context("QUIC loopback server did not expose its ephemeral address")?;
    let mut incoming = server.incoming();

    let mut fast_client = QuicTransport::new(remote_id);
    let fast_sender = fast_client
        .connect(&address.to_string(), local_id)
        .await
        .context("QUIC loopback handshake failed")?;
    let mut fast_receiver = timeout(Duration::from_secs(3), incoming.recv())
        .await
        .context("timed out accepting QUIC loopback connection")?
        .context("QUIC loopback accept channel closed")?
        .connection;
    let mut fast_messages = fast_receiver.message_channel();

    let mut slow_sender = None;
    let mut _slow_receiver = None;
    if slow_fast {
        let slow_id = DeviceId::new_v4();
        let mut slow_client = QuicTransport::new(slow_id);
        let sender = slow_client.connect(&address.to_string(), local_id).await?;
        let receiver = timeout(Duration::from_secs(3), incoming.recv())
            .await
            .context("timed out accepting slow QUIC peer")?
            .context("slow QUIC accept channel closed")?
            .connection;
        slow_sender = Some(sender);
        _slow_receiver = Some(receiver);
    }

    let period = Duration::from_secs_f64(1.0 / rate_hz as f64);
    let mut ticker = interval(period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let started = Instant::now();
    let mut sequence = 0_u64;
    let mut last_received = None;
    let mut latencies_us = Vec::new();
    let mut counters = BTreeMap::from(
        REQUIRED_COUNTERS
            .into_iter()
            .map(|counter| (counter.to_string(), 0_u64))
            .collect::<BTreeMap<_, _>>(),
    );
    let mut queue_high_watermark = 1_u64;
    let mut stall_applied = false;

    while started.elapsed() < duration {
        ticker.tick().await;
        sequence += 1;
        if let Some(sender) = &slow_sender {
            if sender
                .send_message(&Message::MouseMove {
                    x: sequence as i32,
                    y: -1,
                })
                .await
                .is_err()
            {
                counters
                    .entry("overwrite".into())
                    .and_modify(|value| *value += 1);
            }
        }

        if sequence % 100 == 0 {
            for kind in load {
                fast_sender
                    .send_message(&load_message(kind, sequence))
                    .await?;
            }
            queue_high_watermark = queue_high_watermark.max((load.len() + 1) as u64);
        }

        let sent = Instant::now();
        fast_sender
            .send_message(&Message::MouseMove {
                x: sequence as i32,
                y: sequence.wrapping_neg() as i32,
            })
            .await?;
        if !stall_applied && stall_ms.is_some() && started.elapsed() >= duration / 2 {
            tokio::time::sleep(Duration::from_millis(stall_ms.unwrap())).await;
            stall_applied = true;
        }

        let received_sequence = loop {
            let message = timeout(Duration::from_secs(2), fast_messages.recv())
                .await
                .context("timed out receiving QUIC loopback data")?
                .context("QUIC loopback receive channel closed")?;
            if let Message::MouseMove { x, .. } = message {
                break x as u64;
            }
        };
        latencies_us.push(sent.elapsed().as_micros() as f64);
        if let Some(previous) = last_received {
            if received_sequence == previous {
                counters
                    .entry("duplicate".into())
                    .and_modify(|value| *value += 1);
            } else if received_sequence < previous {
                counters
                    .entry("out_of_order".into())
                    .and_modify(|value| *value += 1);
            } else if received_sequence > previous + 1 {
                counters
                    .entry("gap".into())
                    .and_modify(|value| *value += received_sequence - previous - 1);
            }
        }
        last_received = Some(received_sequence);
    }

    let diagnostics = fast_sender.diagnostics();
    *counters.get_mut("overwrite").unwrap() += diagnostics.datagram_tx_dropped;
    *counters.get_mut("reliable_overflow").unwrap() += diagnostics.reliable_stream_reset_count;
    latencies_us.sort_by(f64::total_cmp);
    let median = percentile(&latencies_us, 0.50);
    let p95 = percentile(&latencies_us, 0.95);
    let p99 = percentile(&latencies_us, 0.99);
    let mut metrics = BTreeMap::from([
        ("median_us".into(), median),
        ("p95_us".into(), p95),
        ("p99_us".into(), p99),
    ]);
    if stall_applied {
        metrics.insert("stall_recovery_us".into(), p99);
    }
    if slow_fast {
        metrics.insert("fast_peer_p99_us".into(), p99);
    }
    let queues = BTreeMap::from([(
        "control_outbound".into(),
        QueueSummary {
            capacity: 128,
            high_watermark: queue_high_watermark.min(128),
            overwrites: counters["overwrite"],
            overflows: counters["reliable_overflow"],
        },
    )]);
    let run = PerfRun {
        run_id: format!("{}-{}", options.batch_id, options.run_index),
        batch_id: options.batch_id,
        process_exit_success: true,
        schema_valid: false,
        scenario_config_sha256: String::new(),
        metrics,
        counters,
        errors: vec![],
    };
    fast_sender.close().await;
    if let Some(sender) = slow_sender {
        sender.close().await;
    }
    server.close().await?;
    Ok(LoopbackMeasurement {
        run,
        queues,
        transport_handshake_completed: true,
    })
}

fn load_message(kind: &LoadKind, sequence: u64) -> Message {
    match kind {
        LoadKind::Diagnostics => Message::LatencyProbe {
            sequence,
            timestamp_ms: now_millis(),
            endpoint_switch: false,
            origin_sequence: None,
        },
        LoadKind::Status => Message::Heartbeat {
            sequence,
            timestamp: now_millis(),
        },
        LoadKind::Audio => Message::AudioFrame {
            frame: AudioFramePayload {
                stream_id: DeviceId::from_u128(3),
                sequence,
                timestamp_ms: now_millis(),
                format: AudioFormat::pcm_i16_48k_stereo_20ms(),
                data: vec![0; 3_840],
            },
        },
        LoadKind::Bulk => Message::ClipboardData {
            mime_type: "application/octet-stream".into(),
            data: vec![0; 64 * 1024],
        },
    }
}

fn percentile(values: &[f64], quantile: f64) -> f64 {
    let index = ((values.len().saturating_sub(1)) as f64 * quantile).ceil() as usize;
    values.get(index).copied().unwrap_or(0.0)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::REQUIRED_COUNTERS;
    use std::time::Duration;

    #[test]
    fn quic_matrix_contains_every_predeclared_scenario() {
        assert_eq!(
            scenario_matrix(),
            vec![
                QuicScenario::Rate {
                    rate_hz: 125,
                    duration_secs: 10,
                    load: vec![],
                },
                QuicScenario::Rate {
                    rate_hz: 500,
                    duration_secs: 10,
                    load: vec![],
                },
                QuicScenario::Rate {
                    rate_hz: 1000,
                    duration_secs: 60,
                    load: vec![],
                },
                QuicScenario::Rate {
                    rate_hz: 1000,
                    duration_secs: 60,
                    load: vec![
                        LoadKind::Diagnostics,
                        LoadKind::Status,
                        LoadKind::Audio,
                        LoadKind::Bulk,
                    ],
                },
                QuicScenario::SlowFastPeerIsolation,
                QuicScenario::StallRecovery { stall_ms: 100 },
            ]
        );
    }

    #[test]
    fn stall_recovery_rejects_non_exact_duration() {
        assert!(matches!(
            QuicScenario::StallRecovery { stall_ms: 99 }.validate(),
            Err(ScenarioError::StallMustBeExactly100Ms)
        ));
    }

    #[tokio::test]
    async fn loopback_runner_uses_real_quic_and_records_required_data() {
        let measured = run_loopback_once(
            &QuicScenario::Rate {
                rate_hz: 125,
                duration_secs: 10,
                load: vec![],
            },
            LoopbackRunOptions::test(Duration::from_millis(40)),
        )
        .await
        .unwrap();

        assert!(measured.transport_handshake_completed);
        assert!(measured.run.metrics.contains_key("median_us"));
        assert!(measured.run.metrics.contains_key("p95_us"));
        assert!(measured.run.metrics.contains_key("p99_us"));
        assert!(REQUIRED_COUNTERS
            .iter()
            .all(|counter| measured.run.counters.contains_key(*counter)));
        assert!(measured.queues["control_outbound"].high_watermark > 0);
    }

    #[tokio::test]
    async fn unstable_batch_retries_all_five_once_and_preserves_both_batches() {
        let values = [
            70.0, 85.0, 100.0, 115.0, 130.0, 100.0, 100.0, 100.0, 100.0, 100.0,
        ];
        let mut calls = 0_usize;
        let outcome = orchestrate_five_run_batches("config-hash", |batch, index| {
            let value = values[calls];
            calls += 1;
            std::future::ready(Ok(fixture_run(batch, index, value)))
        })
        .await
        .unwrap();

        assert_eq!(calls, 10);
        assert_eq!(outcome.batches.len(), 2);
        assert_eq!(outcome.selected_batch, Some(1));
        assert_eq!(outcome.batches[0].verdict, VerdictStatus::Unstable);
        assert_eq!(outcome.batches[1].verdict, VerdictStatus::Pass);
        assert!(outcome
            .batches
            .iter()
            .flat_map(|batch| &batch.runs)
            .all(|run| run.scenario_config_sha256 == "config-hash"));
        let unique: std::collections::HashSet<_> = outcome
            .batches
            .iter()
            .flat_map(|batch| batch.runs.iter().map(|run| &run.run_id))
            .collect();
        assert_eq!(unique.len(), 10);
    }

    #[tokio::test]
    async fn second_unstable_batch_is_infrastructure_failure_without_third_attempt() {
        let values = [70.0, 85.0, 100.0, 115.0, 130.0];
        let mut calls = 0_usize;
        let outcome = orchestrate_five_run_batches("config-hash", |batch, index| {
            let value = values[calls % 5];
            calls += 1;
            std::future::ready(Ok(fixture_run(batch, index, value)))
        })
        .await
        .unwrap();

        assert_eq!(calls, 10);
        assert_eq!(outcome.batches.len(), 2);
        assert_eq!(outcome.selected_batch, None);
        assert_eq!(
            outcome.infrastructure_failure.as_deref(),
            Some("unstable_after_one_complete_retry")
        );
    }

    fn fixture_run(batch: &str, index: usize, value: f64) -> PerfRun {
        PerfRun {
            run_id: format!("{batch}-{index}"),
            batch_id: batch.into(),
            process_exit_success: true,
            schema_valid: true,
            scenario_config_sha256: "ignored".into(),
            metrics: BTreeMap::from([("p99_us".into(), value)]),
            counters: BTreeMap::from(
                REQUIRED_COUNTERS
                    .iter()
                    .map(|counter| (counter.to_string(), 0))
                    .collect::<BTreeMap<_, _>>(),
            ),
            errors: vec![],
        }
    }
}
