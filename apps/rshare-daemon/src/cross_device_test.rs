//! Daemon-owned automatic cross-device verification and stress runner.

use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Result};
use rshare_core::{
    CrossDeviceTestKind, CrossDeviceTestMetrics, CrossDeviceTestReport, CrossDeviceTestRequest,
    CrossDeviceTestRunState, CrossDeviceTestSample, CrossDeviceTestStatusSnapshot, DeviceId,
    EndpointEventKind, EndpointEventPayload, EndpointInjectError, EndpointInjectMode,
    EndpointInjectRequest, EndpointInjectTarget, CROSS_DEVICE_MAX_DURATION_MS,
    CROSS_DEVICE_MAX_INTERVAL_MS, CROSS_DEVICE_MAX_SAMPLES, CROSS_DEVICE_MAX_TIMEOUT_MS,
    CROSS_DEVICE_REPORT_SAMPLE_LIMIT,
};
use rshare_input::InputInjectionHandle;
use rshare_net::NetworkManager;
use tokio::sync::{broadcast, watch, Mutex, RwLock};
use tokio::time::{sleep, timeout, Duration};

use crate::endpoint_runtime::inject_endpoint_event;
use crate::{is_device_connected, timestamp_ms_now, DaemonState};
use rshare_core::LocalInputDiagnosticEvent;

const DEFAULT_STRESS_INTERVAL_MS: u64 = 10;
const STOP_WAIT_MS: u64 = 2_500;

#[derive(Clone, Default)]
pub(crate) struct CrossDeviceTestRuntime {
    active: Arc<Mutex<Option<ActiveRun>>>,
    status: Arc<RwLock<CrossDeviceTestStatusSnapshot>>,
}

struct ActiveRun {
    test_id: String,
    cancel: watch::Sender<bool>,
}

#[derive(Clone)]
pub(crate) struct RunContext {
    pub(crate) network_manager: Arc<Mutex<NetworkManager>>,
    pub(crate) injection: InputInjectionHandle,
    pub(crate) state: Arc<RwLock<DaemonState>>,
    pub(crate) local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
}

struct RunAccumulator {
    report: CrossDeviceTestReport,
    latencies_us: Vec<u64>,
    last_position: Option<(i32, i32)>,
    keyboard_pressed: bool,
}

impl CrossDeviceTestRuntime {
    pub(crate) async fn run_once(
        &self,
        context: RunContext,
        request: CrossDeviceTestRequest,
    ) -> Result<CrossDeviceTestReport> {
        let target = resolve_target(&context.state, request.device_id).await?;
        let request = normalize_request(request, false)?;
        let (test_id, cancel_rx) = self.begin(target, &request).await?;
        let result = self
            .run_worker(context, target, request, test_id.clone(), cancel_rx)
            .await;
        match &result {
            Ok(report) => self.finish(&test_id, Some(report)).await,
            Err(error) => {
                self.finish_with_error(
                    &test_id,
                    CrossDeviceTestRunState::Failed,
                    error.to_string(),
                )
                .await;
            }
        }
        result
    }

    pub(crate) async fn start_stress(
        &self,
        context: RunContext,
        request: CrossDeviceTestRequest,
    ) -> Result<CrossDeviceTestStatusSnapshot> {
        let target = resolve_target(&context.state, request.device_id).await?;
        let request = normalize_request(request, true)?;
        let (test_id, cancel_rx) = self.begin(target, &request).await?;
        let runner = self.clone();
        tokio::spawn(async move {
            let result = runner
                .run_worker(context, target, request, test_id.clone(), cancel_rx)
                .await;
            match result {
                Ok(report) => runner.finish(&test_id, Some(&report)).await,
                Err(error) => {
                    runner
                        .finish_with_error(
                            &test_id,
                            CrossDeviceTestRunState::Failed,
                            error.to_string(),
                        )
                        .await;
                }
            }
        });
        Ok(self.status().await)
    }

    pub(crate) async fn stop(&self) -> CrossDeviceTestStatusSnapshot {
        let cancel = {
            self.active
                .lock()
                .await
                .as_ref()
                .map(|run| run.cancel.clone())
        };
        if let Some(cancel) = cancel {
            let _ = cancel.send(true);
            let _ = timeout(Duration::from_millis(STOP_WAIT_MS), async {
                loop {
                    if !self.status().await.active {
                        break;
                    }
                    sleep(Duration::from_millis(20)).await;
                }
            })
            .await;
        }
        self.status().await
    }

    pub(crate) async fn status(&self) -> CrossDeviceTestStatusSnapshot {
        self.status.read().await.clone()
    }

    async fn begin(
        &self,
        target: DeviceId,
        request: &CrossDeviceTestRequest,
    ) -> Result<(String, watch::Receiver<bool>)> {
        let mut active = self.active.lock().await;
        if active.is_some() {
            return Err(anyhow!("a cross-device test is already running"));
        }

        let test_id = format!("cross-{}", DeviceId::new_v4().simple());
        let (cancel, cancel_rx) = watch::channel(false);
        *active = Some(ActiveRun {
            test_id: test_id.clone(),
            cancel,
        });
        drop(active);

        let report = initial_report(&test_id, target, request);
        let mut status = self.status.write().await;
        status.active = true;
        status.report = Some(report);
        Ok((test_id, cancel_rx))
    }

    async fn run_worker(
        &self,
        context: RunContext,
        target: DeviceId,
        request: CrossDeviceTestRequest,
        test_id: String,
        mut cancel_rx: watch::Receiver<bool>,
    ) -> Result<CrossDeviceTestReport> {
        let started = Instant::now();
        let interval_ms = effective_interval_ms(&request);
        let duration_limit = if request.duration_ms == 0 {
            None
        } else {
            Some(Duration::from_millis(request.duration_ms))
        };
        let mut accumulator = RunAccumulator {
            report: initial_report(&test_id, target, &request),
            latencies_us: Vec::new(),
            last_position: None,
            keyboard_pressed: false,
        };
        let mut index = 0u32;
        let mut cancelled = false;

        loop {
            if *cancel_rx.borrow() {
                cancelled = true;
                break;
            }
            if request.duration_ms == 0 && index >= request.sample_count {
                break;
            }
            if duration_limit.is_some_and(|limit| started.elapsed() >= limit) {
                break;
            }
            if index >= CROSS_DEVICE_MAX_SAMPLES {
                break;
            }

            let position =
                match request.kind {
                    CrossDeviceTestKind::MouseMove => Some((
                        request.start_x.saturating_add(
                            (index as i64).saturating_mul(request.step_x as i64) as i32,
                        ),
                        request.start_y.saturating_add(
                            (index as i64).saturating_mul(request.step_y as i64) as i32,
                        ),
                    )),
                    CrossDeviceTestKind::KeyboardShift => None,
                };
            let correlation_id = format!("{test_id}-{index}");
            let payload = match request.kind {
                CrossDeviceTestKind::MouseMove => {
                    let (x, y) = position.expect("mouse test position is present");
                    EndpointEventPayload::MouseMove {
                        x,
                        y,
                        display_id: None,
                    }
                }
                CrossDeviceTestKind::KeyboardShift => EndpointEventPayload::Keyboard {
                    key: "ShiftLeft".to_string(),
                    state: if index % 2 == 0 {
                        "Pressed".to_string()
                    } else {
                        "Released".to_string()
                    },
                },
            };
            let device_kind = match request.kind {
                CrossDeviceTestKind::MouseMove => EndpointEventKind::Mouse,
                CrossDeviceTestKind::KeyboardShift => EndpointEventKind::Keyboard,
            };
            let inject_request = EndpointInjectRequest {
                correlation_id: correlation_id.clone(),
                device_kind,
                payload,
                mode: EndpointInjectMode::TestLoopback,
                timeout_ms: request.timeout_ms,
            };
            let sent_timestamp_ms = timestamp_ms_now();
            let request_started = Instant::now();
            accumulator.report.metrics.requested =
                accumulator.report.metrics.requested.saturating_add(1);
            accumulator.report.metrics.sent = accumulator.report.metrics.sent.saturating_add(1);
            let result = inject_endpoint_event(
                &context.network_manager,
                &context.injection,
                &context.state,
                &context.local_events_tx,
                EndpointInjectTarget::Remote(target),
                inject_request,
            )
            .await;
            let elapsed_us = request_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
            accumulator.latencies_us.push(elapsed_us);
            if result.accepted {
                accumulator.report.metrics.accepted =
                    accumulator.report.metrics.accepted.saturating_add(1);
            } else {
                accumulator.report.metrics.failed =
                    accumulator.report.metrics.failed.saturating_add(1);
                if matches!(result.error, Some(EndpointInjectError::Timeout)) {
                    accumulator.report.metrics.timed_out =
                        accumulator.report.metrics.timed_out.saturating_add(1);
                }
            }
            let event = result.observed_event.clone();
            let observed = result.accepted
                && event.as_ref().is_some_and(|event| {
                    event.correlation_id.as_deref() == Some(correlation_id.as_str())
                });
            if observed {
                accumulator.report.metrics.observed =
                    accumulator.report.metrics.observed.saturating_add(1);
            }
            if request.kind == CrossDeviceTestKind::KeyboardShift {
                accumulator.keyboard_pressed = index % 2 == 0 && result.accepted;
            }
            if let Some((x, y)) = position {
                accumulator.last_position = Some((x, y));
            }
            accumulator.latencies_us.sort_unstable();
            let sample = CrossDeviceTestSample {
                index,
                correlation_id,
                event_kind: device_kind,
                position_x: position.map(|(x, _)| x),
                position_y: position.map(|(_, y)| y),
                display_id: None,
                sent_timestamp_ms,
                response_timestamp_ms: Some(timestamp_ms_now()),
                remote_event_timestamp_ms: event.as_ref().map(|event| event.timestamp_ms),
                elapsed_us: Some(elapsed_us),
                accepted: result.accepted,
                observed,
                remote_event_id: event.as_ref().map(|event| event.event_id),
                event,
                error: result.error,
            };
            accumulator.report.samples.push(sample);
            if accumulator.report.samples.len() > CROSS_DEVICE_REPORT_SAMPLE_LIMIT {
                let excess = accumulator.report.samples.len() - CROSS_DEVICE_REPORT_SAMPLE_LIMIT;
                accumulator.report.samples.drain(0..excess);
            }
            update_metrics(
                &mut accumulator.report,
                &accumulator.latencies_us,
                started.elapsed(),
            );
            self.publish(&test_id, &accumulator.report).await;
            index = index.saturating_add(1);

            if interval_ms > 0 {
                tokio::select! {
                    _ = sleep(Duration::from_millis(interval_ms)) => {}
                    changed = cancel_rx.changed() => {
                        if changed.is_ok() && *cancel_rx.borrow() {
                            cancelled = true;
                            break;
                        }
                    }
                }
            }
        }

        if accumulator.keyboard_pressed {
            let cleanup = EndpointInjectRequest {
                correlation_id: format!("{test_id}-cleanup"),
                device_kind: EndpointEventKind::Keyboard,
                payload: EndpointEventPayload::Keyboard {
                    key: "ShiftLeft".to_string(),
                    state: "Released".to_string(),
                },
                mode: EndpointInjectMode::TestLoopback,
                timeout_ms: request.timeout_ms,
            };
            let cleanup_result = inject_endpoint_event(
                &context.network_manager,
                &context.injection,
                &context.state,
                &context.local_events_tx,
                EndpointInjectTarget::Remote(target),
                cleanup,
            )
            .await;
            if !cleanup_result.accepted {
                accumulator.report.error = Some(format!(
                    "keyboard cleanup failed: {:?}",
                    cleanup_result.error
                ));
            }
        }

        accumulator.report.state = if accumulator.report.error.is_some() {
            CrossDeviceTestRunState::Failed
        } else if cancelled {
            accumulator.report.metrics.cancelled =
                accumulator.report.metrics.cancelled.saturating_add(1);
            CrossDeviceTestRunState::Stopped
        } else {
            CrossDeviceTestRunState::Completed
        };
        accumulator.report.finished_timestamp_ms = Some(timestamp_ms_now());
        accumulator.report.duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        update_metrics(
            &mut accumulator.report,
            &accumulator.latencies_us,
            started.elapsed(),
        );
        self.publish(&test_id, &accumulator.report).await;
        Ok(accumulator.report)
    }

    async fn publish(&self, test_id: &str, report: &CrossDeviceTestReport) {
        let mut status = self.status.write().await;
        if status
            .report
            .as_ref()
            .is_some_and(|current| current.test_id == test_id)
        {
            status.report = Some(report.clone());
        }
    }

    async fn finish(&self, test_id: &str, report: Option<&CrossDeviceTestReport>) {
        let mut active = self.active.lock().await;
        if active.as_ref().is_some_and(|run| run.test_id == test_id) {
            *active = None;
            let mut status = self.status.write().await;
            status.active = false;
            if let Some(report) = report {
                status.report = Some(report.clone());
            }
        }
    }

    async fn finish_with_error(
        &self,
        test_id: &str,
        state: CrossDeviceTestRunState,
        error: String,
    ) {
        let mut active = self.active.lock().await;
        if active.as_ref().is_some_and(|run| run.test_id == test_id) {
            *active = None;
            let mut status = self.status.write().await;
            status.active = false;
            if let Some(report) = status.report.as_mut() {
                report.state = state;
                report.error = Some(error);
                report.finished_timestamp_ms = Some(timestamp_ms_now());
            }
        }
    }
}

fn initial_report(
    test_id: &str,
    target: DeviceId,
    request: &CrossDeviceTestRequest,
) -> CrossDeviceTestReport {
    CrossDeviceTestReport {
        test_id: test_id.to_string(),
        target_device_id: Some(target),
        kind: request.kind,
        state: CrossDeviceTestRunState::Running,
        started_timestamp_ms: timestamp_ms_now(),
        finished_timestamp_ms: None,
        duration_ms: 0,
        metrics: CrossDeviceTestMetrics::default(),
        samples: Vec::new(),
        error: None,
    }
}

async fn resolve_target(
    state: &Arc<RwLock<DaemonState>>,
    requested: Option<DeviceId>,
) -> Result<DeviceId> {
    let state = state.read().await;
    if let Some(device_id) = requested {
        if is_device_connected(&state, device_id) {
            return Ok(device_id);
        }
        return Err(anyhow!("target device {device_id} is not connected"));
    }
    state
        .devices
        .values()
        .filter(|device| device.connected)
        .map(|device| device.id)
        .min()
        .ok_or_else(|| anyhow!("no connected peer is available for automatic testing"))
}

fn normalize_request(
    mut request: CrossDeviceTestRequest,
    stress: bool,
) -> Result<CrossDeviceTestRequest> {
    if request.duration_ms > CROSS_DEVICE_MAX_DURATION_MS {
        request.duration_ms = CROSS_DEVICE_MAX_DURATION_MS;
    }
    if request.interval_ms > CROSS_DEVICE_MAX_INTERVAL_MS {
        request.interval_ms = CROSS_DEVICE_MAX_INTERVAL_MS;
    }
    if request.timeout_ms == 0 {
        request.timeout_ms = 1_000;
    }
    request.timeout_ms = request.timeout_ms.min(CROSS_DEVICE_MAX_TIMEOUT_MS);
    request.sample_count = request.sample_count.min(CROSS_DEVICE_MAX_SAMPLES);
    if !stress && request.duration_ms > 0 {
        return Err(anyhow!(
            "duration_ms is only valid for continuous stress tests"
        ));
    }
    if !stress && request.sample_count == 0 {
        request.sample_count = CrossDeviceTestRequest::default().sample_count;
    }
    if stress {
        if request.duration_ms == 0 {
            return Err(anyhow!("continuous stress test requires duration_ms > 0"));
        }
        if request.interval_ms == 0 {
            request.interval_ms = DEFAULT_STRESS_INTERVAL_MS;
        }
    }
    Ok(request)
}

fn effective_interval_ms(request: &CrossDeviceTestRequest) -> u64 {
    if request.duration_ms > 0 && request.interval_ms == 0 {
        DEFAULT_STRESS_INTERVAL_MS
    } else {
        request.interval_ms
    }
}

fn update_metrics(
    report: &mut CrossDeviceTestReport,
    latencies_us: &[u64],
    elapsed: std::time::Duration,
) {
    report.duration_ms = elapsed.as_millis().min(u64::MAX as u128) as u64;
    report.metrics.p50_us = percentile(latencies_us, 50);
    report.metrics.p95_us = percentile(latencies_us, 95);
    report.metrics.p99_us = percentile(latencies_us, 99);
    report.metrics.max_us = latencies_us.last().copied();
    report.metrics.throughput_per_second_milli = if elapsed.as_micros() == 0 {
        None
    } else {
        Some(
            (report.metrics.accepted as u128)
                .saturating_mul(1_000_000_000)
                .checked_div(elapsed.as_micros())
                .unwrap_or_default()
                .min(u64::MAX as u128) as u64,
        )
    };
}

fn percentile(values: &[u64], percentile: u64) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let rank = ((values.len() as u64)
        .saturating_mul(percentile)
        .saturating_add(99)
        / 100)
        .max(1)
        .min(values.len() as u64);
    values.get((rank - 1) as usize).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_is_ceil_ranked_and_bounded() {
        assert_eq!(percentile(&[10, 20, 30, 40], 50), Some(20));
        assert_eq!(percentile(&[10, 20, 30, 40], 95), Some(40));
        assert_eq!(percentile(&[], 50), None);
    }

    #[test]
    fn stress_request_is_bounded_and_gets_safe_defaults() {
        let request = normalize_request(
            CrossDeviceTestRequest {
                duration_ms: CROSS_DEVICE_MAX_DURATION_MS + 1,
                interval_ms: 0,
                timeout_ms: 0,
                ..CrossDeviceTestRequest::default()
            },
            true,
        )
        .unwrap();
        assert_eq!(request.duration_ms, CROSS_DEVICE_MAX_DURATION_MS);
        assert_eq!(request.interval_ms, DEFAULT_STRESS_INTERVAL_MS);
        assert_eq!(request.timeout_ms, 1_000);
    }
}
