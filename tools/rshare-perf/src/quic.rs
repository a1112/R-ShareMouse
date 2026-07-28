use crate::report::{percentile, PerfRun, QueueSummary, VerdictStatus, REQUIRED_COUNTERS};
use anyhow::{anyhow, Context, Result};
use rshare_core::{AudioFormat, AudioFramePayload, DeviceId, Message};
use rshare_net::{ConnectionPool, QuicTransport};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::mpsc, task::JoinSet, time::timeout};

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
    pub global_deadline: Option<Duration>,
    pub bind_addr: Option<String>,
    pub batch_id: String,
    pub run_index: usize,
    sender_delay: Option<Duration>,
}

impl LoopbackRunOptions {
    pub fn measured(batch_id: String, run_index: usize) -> Self {
        Self {
            effective_duration: None,
            global_deadline: None,
            bind_addr: None,
            batch_id,
            run_index,
            sender_delay: None,
        }
    }

    #[cfg(test)]
    fn test(duration: Duration) -> Self {
        Self {
            effective_duration: Some(duration),
            global_deadline: None,
            bind_addr: None,
            batch_id: "test-batch".into(),
            run_index: 0,
            sender_delay: None,
        }
    }

    #[cfg(test)]
    fn test_with_sender_delay(duration: Duration, sender_delay: Duration) -> Self {
        let mut options = Self::test(duration);
        options.sender_delay = Some(sender_delay);
        options
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
    let configured_duration = match scenario {
        QuicScenario::Rate { duration_secs, .. } => Duration::from_secs(*duration_secs),
        QuicScenario::SlowFastPeerIsolation | QuicScenario::StallRecovery { .. } => {
            Duration::from_secs(10)
        }
    };
    let measurement_duration = options.effective_duration.unwrap_or(configured_duration);
    let global_deadline = options
        .global_deadline
        .unwrap_or(measurement_duration + Duration::from_secs(5));
    let bind_addr = options.bind_addr.as_deref().unwrap_or("127.0.0.1:0");
    let local_id = DeviceId::new_v4();
    let mut server = QuicTransport::new(local_id);
    server.start_server(bind_addr).await?;
    let result = timeout(
        global_deadline,
        run_loopback_once_started(scenario, options, local_id, &mut server),
    )
    .await
    .map_err(|_| anyhow!("QUIC loopback run exceeded global deadline"));
    let close_result = server.close().await;
    match result {
        Ok(result) => {
            close_result?;
            result
        }
        Err(error) => {
            let _ = close_result;
            Err(error)
        }
    }
}

async fn run_loopback_once_started(
    scenario: &QuicScenario,
    options: LoopbackRunOptions,
    local_id: DeviceId,
    server: &mut QuicTransport,
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
    let fast_id = DeviceId::new_v4();
    let address = server
        .local_addr()
        .context("QUIC loopback server did not expose its ephemeral address")?;
    let mut incoming = server.incoming();

    let mut fast_client = QuicTransport::new(fast_id);
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

    let pool = ConnectionPool::new(local_id);
    pool.insert(fast_id, fast_sender).await;
    let mut slow_id = None;
    let mut slow_receiver = None;
    if slow_fast {
        let id = DeviceId::new_v4();
        let mut slow_client = QuicTransport::new(id);
        let sender = slow_client.connect(&address.to_string(), local_id).await?;
        let receiver = timeout(Duration::from_secs(3), incoming.recv())
            .await
            .context("timed out accepting slow QUIC peer")?
            .context("slow QUIC accept channel closed")?
            .connection;
        pool.insert(id, sender).await;
        slow_id = Some(id);
        slow_receiver = Some(receiver);
    }

    let expected_sent = ((duration.as_secs_f64() * rate_hz as f64).round() as u64).max(1);
    let channel_capacity = usize::try_from(expected_sent.min(4_096)).unwrap_or(4_096);
    let (fast_tx, mut fast_rx) = mpsc::channel::<u64>(channel_capacity);
    let scheduled_times = Arc::new(Mutex::new(HashMap::<u64, Instant>::new()));
    let receive_state = Arc::new(Mutex::new(ReceiveState::default()));
    let transport_send_completed = Arc::new(AtomicU64::new(0));
    let production_fanout_calls = Arc::new(AtomicU64::new(0));
    let producer_dropped = Arc::new(AtomicU64::new(0));
    let send_errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let slow_send_us = Arc::new(Mutex::new(Vec::<f64>::new()));
    let mut tasks = JoinSet::new();
    let mut task_count = 0_usize;
    let producer_started = Instant::now();
    let measurement_ended = producer_started + duration;

    let receiver_state = Arc::clone(&receive_state);
    let receiver_scheduled_times = Arc::clone(&scheduled_times);
    let stall_trigger_sequence = (expected_sent / 2).max(1);
    tasks.spawn(async move {
        while let Some(message) = fast_messages.recv().await {
            let Message::MouseMove { x, .. } = message else {
                continue;
            };
            let sequence = x as u64;
            let should_stall = {
                let mut state = receiver_state
                    .lock()
                    .expect("receiver state mutex poisoned");
                if stall_ms.is_some()
                    && state.stall_started.is_none()
                    && sequence >= stall_trigger_sequence
                {
                    state.stall_started = Some(Instant::now());
                    true
                } else {
                    false
                }
            };
            if should_stall {
                tokio::time::sleep(Duration::from_millis(stall_ms.unwrap())).await;
            }

            let received_at = Instant::now();
            let scheduled_at = receiver_scheduled_times
                .lock()
                .expect("scheduled time mutex poisoned")
                .remove(&sequence);
            let mut state = receiver_state
                .lock()
                .expect("receiver state mutex poisoned");
            if received_at > measurement_ended {
                break;
            }
            let unique_delivery = state.received_sequences.insert(sequence);
            if unique_delivery {
                if let Some(scheduled_at) = scheduled_at {
                    state
                        .latencies_us
                        .push(received_at.duration_since(scheduled_at).as_micros() as f64);
                }
            } else {
                state.duplicate += 1;
            }
            let consecutive = state
                .last_received
                .is_some_and(|previous| sequence == previous + 1);
            if let Some(previous) = state.last_received {
                if sequence < previous {
                    state.out_of_order += 1;
                }
            }
            if let Some(stall_started) = state.stall_started {
                if state.stall_recovery_us.is_none() {
                    state.post_stall_consecutive = if consecutive {
                        state.post_stall_consecutive + 1
                    } else {
                        1
                    };
                    if state.post_stall_consecutive >= 3 {
                        state.stall_recovery_us = Some(stall_started.elapsed().as_micros() as f64);
                        state.stall_recovery_consecutive_deliveries = state.post_stall_consecutive;
                    }
                }
            }
            state.last_received = Some(sequence);
            if sequence >= expected_sent {
                break;
            }
        }
    });
    task_count += 1;

    let fast_pool = pool.clone();
    let fast_scheduled_times = Arc::clone(&scheduled_times);
    let fast_transport_send_completed = Arc::clone(&transport_send_completed);
    let fast_production_fanout_calls = Arc::clone(&production_fanout_calls);
    let fast_errors = Arc::clone(&send_errors);
    let fast_fanout_costs = Arc::clone(&slow_send_us);
    let load_messages = load.to_vec();
    let sender_delay = options.sender_delay;
    tasks.spawn(async move {
        while let Some(sequence) = fast_rx.recv().await {
            if let Some(delay) = sender_delay {
                tokio::time::sleep(delay).await;
            }
            let message = Message::MouseMove {
                x: sequence as i32,
                y: sequence.wrapping_neg() as i32,
            };
            let send_started = Instant::now();
            let send_result = if slow_fast {
                fast_pool.broadcast(&message).await
            } else {
                fast_pool.send_to(&fast_id, &message).await
            };
            if slow_fast {
                fast_fanout_costs
                    .lock()
                    .expect("fanout cost mutex poisoned")
                    .push(send_started.elapsed().as_micros() as f64);
            }
            match send_result {
                Ok(()) => {
                    fast_transport_send_completed.fetch_add(1, Ordering::Relaxed);
                    if slow_fast {
                        fast_production_fanout_calls.fetch_add(1, Ordering::Relaxed);
                    }
                    if sequence % 100 == 0 {
                        for kind in &load_messages {
                            if let Err(error) = fast_pool
                                .send_to(&fast_id, &load_message(kind, sequence))
                                .await
                            {
                                fast_errors
                                    .lock()
                                    .expect("send error mutex poisoned")
                                    .push(error.to_string());
                            }
                        }
                    }
                }
                Err(error) => {
                    fast_scheduled_times
                        .lock()
                        .expect("scheduled time mutex poisoned")
                        .remove(&sequence);
                    fast_errors
                        .lock()
                        .expect("send error mutex poisoned")
                        .push(error.to_string());
                }
            }
        }
    });
    task_count += 1;

    let period = Duration::from_secs_f64(1.0 / rate_hz as f64);
    for sequence in 1..=expected_sent {
        let deadline = producer_started + period.mul_f64((sequence - 1) as f64);
        tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
        scheduled_times
            .lock()
            .expect("scheduled time mutex poisoned")
            .insert(sequence, deadline);
        if fast_tx.try_send(sequence).is_err() {
            producer_dropped.fetch_add(1, Ordering::Relaxed);
            scheduled_times
                .lock()
                .expect("scheduled time mutex poisoned")
                .remove(&sequence);
        }
    }
    drop(fast_tx);
    for _ in 0..task_count {
        match timeout(Duration::from_secs(2), tasks.join_next()).await {
            Ok(Some(Ok(()))) => {}
            Ok(Some(Err(error))) => send_errors
                .lock()
                .expect("send error mutex poisoned")
                .push(format!("QUIC loopback task failed: {error}")),
            Ok(None) => break,
            Err(_) => {
                tasks.abort_all();
                send_errors
                    .lock()
                    .expect("send error mutex poisoned")
                    .push("QUIC loopback task exceeded cleanup deadline".into());
                break;
            }
        }
    }

    let fast_diagnostics = pool
        .diagnostics_for(&fast_id)
        .await
        .context("fast QUIC transport diagnostics unavailable")?;
    let slow_diagnostics = if let Some(id) = slow_id {
        pool.diagnostics_for(&id).await
    } else {
        None
    };
    let state = receive_state.lock().expect("receiver state mutex poisoned");
    let mut counters = BTreeMap::from(
        REQUIRED_COUNTERS
            .into_iter()
            .map(|counter| (counter.to_string(), 0_u64))
            .collect::<BTreeMap<_, _>>(),
    );
    let delivered_within_window = state.received_sequences.len() as u64;
    let end_window_backlog = expected_sent.saturating_sub(delivered_within_window);
    counters.insert("expected_sent".into(), expected_sent);
    counters.insert("actual_sent".into(), delivered_within_window);
    counters.insert("delivered_within_window".into(), delivered_within_window);
    counters.insert("end_window_backlog".into(), end_window_backlog);
    counters.insert(
        "transport_send_completed".into(),
        transport_send_completed.load(Ordering::Relaxed),
    );
    counters.insert("independent_producer".into(), 1);
    counters.insert(
        "producer_queue_dropped".into(),
        producer_dropped.load(Ordering::Relaxed),
    );
    let fanout_calls = production_fanout_calls.load(Ordering::Relaxed);
    counters.insert("production_fanout_calls".into(), fanout_calls);
    counters.insert("shared_pool_fanout".into(), u64::from(fanout_calls > 0));
    counters.insert(
        "stall_recovery_consecutive_deliveries".into(),
        state.stall_recovery_consecutive_deliveries,
    );
    counters.insert("gap".into(), end_window_backlog);
    counters.insert("duplicate".into(), state.duplicate);
    counters.insert("out_of_order".into(), state.out_of_order);
    let datagram_drops = fast_diagnostics.datagram_tx_dropped
        + slow_diagnostics
            .as_ref()
            .map(|diagnostics| diagnostics.datagram_tx_dropped)
            .unwrap_or(0);
    let reliable_resets = fast_diagnostics.reliable_stream_reset_count
        + slow_diagnostics
            .as_ref()
            .map(|diagnostics| diagnostics.reliable_stream_reset_count)
            .unwrap_or(0);
    counters.insert(
        "overwrite".into(),
        datagram_drops + producer_dropped.load(Ordering::Relaxed),
    );
    counters.insert("reliable_overflow".into(), reliable_resets);

    let mut latencies_us = state.latencies_us.clone();
    let stall_recovery_us = state.stall_recovery_us;
    drop(state);
    latencies_us.sort_by(f64::total_cmp);
    let median = percentile(&latencies_us, 0.50);
    let p95 = percentile(&latencies_us, 0.95);
    let p99 = percentile(&latencies_us, 0.99);
    let achieved_hz = delivered_within_window as f64 / duration.as_secs_f64();
    let mut metrics = BTreeMap::from([
        ("median_us".into(), median),
        ("p95_us".into(), p95),
        ("p99_us".into(), p99),
        ("achieved_hz".into(), achieved_hz),
    ]);
    if let Some(recovery) = stall_recovery_us {
        metrics.insert("stall_recovery_us".into(), recovery);
    }
    if slow_fast {
        metrics.insert("fast_peer_p99_us".into(), p99);
        let mut costs = slow_send_us
            .lock()
            .expect("slow send mutex poisoned")
            .clone();
        costs.sort_by(f64::total_cmp);
        metrics.insert("slow_send_p99_us".into(), percentile(&costs, 0.99));
    }
    let mut errors = send_errors
        .lock()
        .expect("send error mutex poisoned")
        .clone();
    if delivered_within_window * 100 < expected_sent * 90 {
        errors.push(format!(
            "achieved send rate {:.2} Hz is below 90% of configured {rate_hz} Hz",
            achieved_hz
        ));
    }
    if stall_ms.is_some() && stall_recovery_us.is_none() {
        errors.push("stall recovery did not reach three consecutive deliveries".into());
    }
    let run = PerfRun {
        run_id: format!("{}-{}", options.batch_id, options.run_index),
        batch_id: options.batch_id,
        process_exit_success: errors.is_empty(),
        schema_valid: false,
        scenario_config_sha256: String::new(),
        metrics,
        counters,
        errors,
    };
    if let Some(connection) = pool.remove(&fast_id).await {
        connection.close().await;
    }
    if let Some(id) = slow_id {
        if let Some(connection) = pool.remove(&id).await {
            connection.close().await;
        }
    }
    fast_receiver.close().await;
    if let Some(receiver) = slow_receiver {
        receiver.close().await;
    }
    Ok(LoopbackMeasurement {
        run,
        queues: BTreeMap::new(),
        transport_handshake_completed: true,
    })
}

#[derive(Default)]
struct ReceiveState {
    latencies_us: Vec<f64>,
    received_sequences: HashSet<u64>,
    last_received: Option<u64>,
    duplicate: u64,
    out_of_order: u64,
    stall_started: Option<Instant>,
    stall_recovery_us: Option<f64>,
    post_stall_consecutive: u64,
    stall_recovery_consecutive_deliveries: u64,
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
        assert!(
            measured.queues.is_empty(),
            "transport does not expose real queue capacity/high-watermark diagnostics"
        );
    }

    #[tokio::test]
    async fn loopback_runner_records_independent_producer_rate_gate() {
        let measured = run_loopback_once(
            &QuicScenario::Rate {
                rate_hz: 1_000,
                duration_secs: 60,
                load: vec![],
            },
            LoopbackRunOptions::test(Duration::from_millis(120)),
        )
        .await
        .unwrap();

        assert_eq!(measured.run.counters["expected_sent"], 120);
        assert_eq!(
            measured.run.counters["actual_sent"] + measured.run.counters["end_window_backlog"],
            measured.run.counters["expected_sent"]
        );
        assert!(
            measured.run.counters["actual_sent"] * 100
                >= measured.run.counters["expected_sent"] * 90
        );
        assert!(measured.run.metrics["achieved_hz"] >= 900.0);
        assert_eq!(measured.run.counters["independent_producer"], 1);
    }

    #[tokio::test]
    async fn slow_sender_backlog_cannot_inflate_window_rate_and_latency_includes_queueing() {
        let measured = run_loopback_once(
            &QuicScenario::Rate {
                rate_hz: 125,
                duration_secs: 10,
                load: vec![],
            },
            LoopbackRunOptions::test_with_sender_delay(
                Duration::from_millis(80),
                Duration::from_millis(20),
            ),
        )
        .await
        .unwrap();

        let expected = measured.run.counters["expected_sent"];
        let delivered = measured.run.counters["delivered_within_window"];
        assert!(delivered < expected);
        assert_eq!(
            measured.run.counters["end_window_backlog"],
            expected - delivered
        );
        assert_eq!(measured.run.counters["actual_sent"], delivered);
        assert!(measured.run.metrics["achieved_hz"] < 125.0);
        assert!(measured.run.metrics["p99_us"] >= 20_000.0);
    }

    #[tokio::test]
    async fn slow_fast_isolation_uses_shared_pool_fanout_and_measures_slow_cost() {
        let measured = run_loopback_once(
            &QuicScenario::SlowFastPeerIsolation,
            LoopbackRunOptions::test(Duration::from_millis(120)),
        )
        .await
        .unwrap();

        assert_eq!(measured.run.counters["shared_pool_fanout"], 1);
        assert_eq!(
            measured.run.counters["production_fanout_calls"],
            measured.run.counters["expected_sent"]
        );
        assert!(measured.run.metrics.contains_key("fast_peer_p99_us"));
        assert!(measured.run.metrics.contains_key("slow_send_p99_us"));
        assert_eq!(
            measured.run.counters["actual_sent"] + measured.run.counters["end_window_backlog"],
            measured.run.counters["expected_sent"]
        );
        assert!(
            measured.run.counters["actual_sent"] * 100
                >= measured.run.counters["expected_sent"] * 90
        );
        assert!(
            measured.run.counters["actual_sent"]
                <= measured.run.counters["production_fanout_calls"]
        );
    }

    #[tokio::test]
    async fn stall_recovery_uses_sequence_timeline_not_whole_run_p99() {
        let measured = run_loopback_once(
            &QuicScenario::StallRecovery { stall_ms: 100 },
            LoopbackRunOptions::test(Duration::from_millis(250)),
        )
        .await
        .unwrap();

        assert!(measured.run.metrics["stall_recovery_us"] >= 100_000.0);
        assert_ne!(
            measured.run.metrics["stall_recovery_us"],
            measured.run.metrics["p99_us"]
        );
        assert!(measured.run.counters["stall_recovery_consecutive_deliveries"] >= 3);
    }

    #[tokio::test]
    async fn global_deadline_closes_loopback_endpoint_and_aborts_tasks() {
        let reservation = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let bind_addr = reservation.local_addr().unwrap().to_string();
        drop(reservation);
        let mut options = LoopbackRunOptions::test(Duration::from_secs(2));
        options.global_deadline = Some(Duration::from_millis(20));
        options.bind_addr = Some(bind_addr.clone());

        let result = run_loopback_once(
            &QuicScenario::Rate {
                rate_hz: 125,
                duration_secs: 10,
                load: vec![],
            },
            options,
        )
        .await;
        assert!(result.is_err());

        let mut replacement = QuicTransport::new(DeviceId::new_v4());
        replacement.start_server(&bind_addr).await.unwrap();
        assert!(replacement.is_running());
        replacement.close().await.unwrap();
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
