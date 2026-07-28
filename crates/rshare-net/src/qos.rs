use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use futures_util::stream::{FuturesUnordered, StreamExt};
use rshare_core::{
    ControlConnectionId, DeviceId, Message, RealtimeInputFrame, ReleaseAllReason,
    ReliableInputEvent, ReliableInputFrame, SessionEpoch,
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::handshake::PeerAuthContext;

pub const QOS_LANE_MAGIC: &[u8; 4] = b"RSQ3";
pub(crate) const TERMINAL_CANCEL_RESET_CODE: u32 = 0x525351;
pub(crate) const AWAITED_CANCEL_RESET_CODE: u32 = 0x525352;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LaneDiscriminator {
    ReliableInput = 1,
    Emergency = 2,
    Control = 3,
    Bulk = 4,
    Telemetry = 5,
    ReliableCompat = 6,
}

#[derive(Debug, Clone)]
pub struct ControlFrame {
    message: Message,
}

#[derive(Debug, Clone)]
pub struct TelemetryFrame {
    message: Message,
}

#[derive(Debug, Clone)]
pub struct BulkFrame {
    message: Message,
}

#[derive(Debug, Clone)]
pub struct ReliableCompatFrame {
    message: Message,
}

#[derive(Debug, Clone)]
pub enum ClassifiedMessage {
    Control(ControlFrame),
    Bulk(BulkFrame),
    Telemetry(TelemetryFrame),
    ReliableCompat(ReliableCompatFrame),
    Unsupported,
}

impl TryFrom<Message> for ClassifiedMessage {
    type Error = TransportSendError;

    fn try_from(message: Message) -> Result<Self, Self::Error> {
        Ok(match message {
            message @ (Message::Goodbye { .. }
            | Message::ClipboardRequest
            | Message::Heartbeat { .. }
            | Message::Ack { .. }
            | Message::Error { .. }
            | Message::ScreenEnter { .. }
            | Message::ScreenLeave { .. }
            | Message::ScreenUpdate { .. }) => Self::Control(ControlFrame { message }),
            message @ (Message::InputDiagnostic { .. }
            | Message::EndpointEventSubscribe { .. }
            | Message::EndpointEventSnapshot { .. }
            | Message::EndpointEventDelta { .. }
            | Message::EndpointInjectRequest { .. }
            | Message::EndpointInjectResult { .. }
            | Message::LatencyProbe { .. }
            | Message::LatencyProbeAck { .. }) => Self::Telemetry(TelemetryFrame { message }),
            message @ (Message::AudioStreamStart { .. }
            | Message::AudioFrame { .. }
            | Message::AudioStreamStop { .. }
            | Message::AudioStreamError { .. }
            | Message::UsbDeviceAttached { .. }
            | Message::UsbDeviceDetached { .. }
            | Message::UsbTransfer { .. }
            | Message::UsbTransferComplete { .. }
            | Message::UsbForwardingError { .. }
            | Message::UsbDeviceClaimRequest { .. }
            | Message::UsbDeviceClaimResponse { .. }
            | Message::UsbDeviceRelease { .. }
            | Message::UsbDeviceReset { .. }
            | Message::UsbTransferCancel { .. }
            | Message::UsbFlowControl { .. }
            | Message::ClipboardData { .. }
            | Message::ClipboardResponse { .. }) => Self::Bulk(BulkFrame { message }),
            message @ (Message::MouseButton { .. }
            | Message::MouseWheel { .. }
            | Message::Key { .. }
            | Message::KeyExtended { .. }
            | Message::GamepadConnected { .. }
            | Message::GamepadDisconnected { .. }) => {
                Self::ReliableCompat(ReliableCompatFrame { message })
            }
            Message::Hello { .. }
            | Message::HelloBack { .. }
            | Message::HelloRejected { .. }
            | Message::MouseMove { .. }
            | Message::GamepadState { .. } => Self::Unsupported,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransportSendError {
    #[error("reliable input lane is full")]
    ReliableLaneFull,
    #[error("transport lane is closed")]
    LaneClosed,
    #[error("emergency slot is occupied")]
    EmergencySlotFull,
    #[error("message is not supported by a QoS lane")]
    UnsupportedMessage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeSendOutcome {
    Sent,
    DroppedLatest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalReleaseEvent {
    pub auth: Arc<PeerAuthContext>,
    pub epoch: SessionEpoch,
    pub reason: ReleaseAllReason,
}

#[derive(Clone)]
pub(crate) struct TerminalReleaseEmitter {
    target: mpsc::Sender<TerminalReleaseEvent>,
    latest_tx: watch::Sender<Option<TerminalReleaseEvent>>,
    pending: Arc<AtomicBool>,
    #[cfg(test)]
    in_flight: Arc<AtomicUsize>,
    #[cfg(test)]
    workers: Arc<AtomicUsize>,
}

impl TerminalReleaseEmitter {
    pub(crate) fn new(target: mpsc::Sender<TerminalReleaseEvent>) -> Self {
        let (latest_tx, mut latest_rx) = watch::channel(None);
        let pending = Arc::new(AtomicBool::new(false));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let workers = Arc::new(AtomicUsize::new(1));
        let worker_pending = pending.clone();
        let worker_in_flight = in_flight.clone();
        let worker_count = workers.clone();
        let worker_target = target.clone();
        tokio::spawn(async move {
            let mut next = None;
            'worker: loop {
                let event = if let Some(event) = next.take() {
                    Some(event)
                } else {
                    if latest_rx.changed().await.is_err() {
                        break;
                    }
                    latest_rx.borrow_and_update().clone()
                };
                worker_pending.store(false, Ordering::Release);
                let Some(event) = event else { continue };
                worker_in_flight.store(1, Ordering::Release);
                let send = worker_target.send(event);
                tokio::pin!(send);
                let mut latest_changed = false;
                let sent = loop {
                    tokio::select! {
                        biased;
                        result = &mut send => break result,
                        changed = latest_rx.changed() => {
                            if changed.is_err() {
                                break 'worker;
                            }
                            latest_changed = true;
                        }
                    }
                };
                if sent.is_err() {
                    break;
                }
                worker_in_flight.store(0, Ordering::Release);
                if latest_changed || latest_rx.has_changed().unwrap_or(false) {
                    next = latest_rx.borrow_and_update().clone();
                }
            }
            worker_in_flight.store(0, Ordering::Release);
            worker_count.store(0, Ordering::Release);
        });
        Self {
            target,
            latest_tx,
            pending,
            #[cfg(test)]
            in_flight,
            #[cfg(test)]
            workers,
        }
    }

    pub(crate) fn emit(&self, event: TerminalReleaseEvent) {
        match self.target.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(event)) => {
                self.pending.store(true, Ordering::Release);
                self.latest_tx.send_replace(Some(event));
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }

    #[cfg(test)]
    fn worker_count_for_test(&self) -> usize {
        self.workers.load(Ordering::Acquire)
    }

    #[cfg(test)]
    fn backlog_for_test(&self) -> usize {
        usize::from(self.pending.load(Ordering::Acquire)) + self.in_flight.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn target_for_test(&self) -> mpsc::Sender<TerminalReleaseEvent> {
        self.target.clone()
    }
}

struct AwaitedFrame<T> {
    frame: T,
    written: oneshot::Sender<Result<(), TransportSendError>>,
}

#[derive(Default)]
struct AwaitedWriteProbes {
    #[cfg(test)]
    started: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    completed: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    cancelled: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    reset_succeeded: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    preface_barrier: Mutex<Option<Arc<tokio::sync::Notify>>>,
    #[cfg(test)]
    preface_barrier_waiting: AtomicBool,
}

#[derive(Default)]
struct SenderEpochState {
    active: Option<SessionEpoch>,
    retired_through: Option<SessionEpoch>,
}

impl SenderEpochState {
    fn retired(&self, epoch: SessionEpoch) -> bool {
        self.retired_through
            .is_some_and(|retired| epoch.0 <= retired.0)
    }

    fn retire(&mut self, epoch: SessionEpoch) {
        if self
            .retired_through
            .is_none_or(|retired| epoch.0 > retired.0)
        {
            self.retired_through = Some(epoch);
        }
        if self.active == Some(epoch) {
            self.active = None;
        }
    }
}

struct PeerTransportInner {
    auth: Arc<PeerAuthContext>,
    realtime_connection: Option<quinn::Connection>,
    reliable_tx: mpsc::Sender<ReliableInputFrame>,
    emergency_tx: mpsc::Sender<ReliableInputFrame>,
    control_tx: mpsc::Sender<AwaitedFrame<ControlFrame>>,
    reliable_compat_tx: mpsc::Sender<AwaitedFrame<ReliableCompatFrame>>,
    bulk_tx: mpsc::Sender<AwaitedFrame<BulkFrame>>,
    telemetry_tx: mpsc::Sender<TelemetryFrame>,
    epoch_state: Mutex<SenderEpochState>,
    admission: Mutex<()>,
    reliable_cancelled_through_tx: watch::Sender<Option<SessionEpoch>>,
    release_emitter: TerminalReleaseEmitter,
    emergency_reserved: AtomicBool,
    reliable_write_started: std::sync::atomic::AtomicU64,
    reliable_write_completed: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    reliable_preface_barrier: Mutex<Option<Arc<tokio::sync::Notify>>>,
    #[cfg(test)]
    reliable_preface_barrier_waiting: AtomicBool,
    #[cfg(test)]
    reliable_cancel_reset_succeeded: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    reliable_payload_poll_paused: AtomicBool,
    #[cfg(test)]
    awaited_write_probes: Arc<AwaitedWriteProbes>,
    #[cfg_attr(not(test), allow(dead_code))]
    worker_count: Arc<AtomicUsize>,
}

impl Drop for PeerTransportInner {
    fn drop(&mut self) {
        if let Some(connection) = &self.realtime_connection {
            connection.close(0u32.into(), b"qos handle dropped");
        }
    }
}

#[derive(Clone)]
pub struct PeerTransportHandle {
    inner: Arc<PeerTransportInner>,
}

#[cfg(test)]
pub(crate) struct QosDropProbe(std::sync::Weak<PeerTransportInner>);

#[cfg(test)]
impl QosDropProbe {
    pub(crate) fn released(&self) -> bool {
        self.0.upgrade().is_none()
    }
}

#[cfg(test)]
pub(crate) struct QosWorkerProbe(Arc<AtomicUsize>);

#[cfg(test)]
impl QosWorkerProbe {
    pub(crate) fn running(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }
}

impl PeerTransportHandle {
    pub(crate) fn from_quinn(
        auth: Arc<PeerAuthContext>,
        connection: quinn::Connection,
        release_emitter: TerminalReleaseEmitter,
    ) -> Self {
        let (reliable_tx, reliable_rx) = mpsc::channel(256);
        let (emergency_tx, emergency_rx) = mpsc::channel(1);
        let (control_tx, control_rx) = mpsc::channel(64);
        let (reliable_compat_tx, reliable_compat_rx) = mpsc::channel(128);
        let (bulk_tx, bulk_rx) = mpsc::channel(8);
        let (telemetry_tx, telemetry_rx) = mpsc::channel(32);
        let (reliable_cancelled_through_tx, reliable_cancelled_through_rx) = watch::channel(None);
        let worker_count = Arc::new(AtomicUsize::new(0));
        let awaited_write_probes = Arc::new(AwaitedWriteProbes::default());
        let inner = Arc::new(PeerTransportInner {
            auth,
            realtime_connection: Some(connection.clone()),
            reliable_tx,
            emergency_tx,
            control_tx,
            reliable_compat_tx,
            bulk_tx,
            telemetry_tx,
            epoch_state: Mutex::new(SenderEpochState::default()),
            admission: Mutex::new(()),
            reliable_cancelled_through_tx,
            release_emitter,
            emergency_reserved: AtomicBool::new(false),
            reliable_write_started: std::sync::atomic::AtomicU64::new(0),
            reliable_write_completed: std::sync::atomic::AtomicU64::new(0),
            #[cfg(test)]
            reliable_preface_barrier: Mutex::new(None),
            #[cfg(test)]
            reliable_preface_barrier_waiting: AtomicBool::new(false),
            #[cfg(test)]
            reliable_cancel_reset_succeeded: std::sync::atomic::AtomicU64::new(0),
            #[cfg(test)]
            reliable_payload_poll_paused: AtomicBool::new(false),
            #[cfg(test)]
            awaited_write_probes: awaited_write_probes.clone(),
            worker_count: worker_count.clone(),
        });
        spawn_reliable_writer(
            Arc::downgrade(&inner),
            connection.clone(),
            reliable_rx,
            reliable_cancelled_through_rx,
            worker_count.clone(),
        );
        spawn_emergency_writer(
            Arc::downgrade(&inner),
            connection.clone(),
            emergency_rx,
            worker_count.clone(),
        );
        spawn_awaited_writer(
            connection.clone(),
            LaneDiscriminator::ReliableCompat,
            reliable_compat_rx,
            ReliableCompatFrame::into_message,
            worker_count.clone(),
            awaited_write_probes.clone(),
        );
        spawn_awaited_writer(
            connection.clone(),
            LaneDiscriminator::Control,
            control_rx,
            ControlFrame::into_message,
            worker_count.clone(),
            awaited_write_probes.clone(),
        );
        spawn_awaited_writer(
            connection.clone(),
            LaneDiscriminator::Bulk,
            bulk_rx,
            BulkFrame::into_message,
            worker_count.clone(),
            awaited_write_probes,
        );
        spawn_telemetry_writer(connection, telemetry_rx, worker_count);
        Self { inner }
    }

    pub fn auth(&self) -> Arc<PeerAuthContext> {
        self.inner.auth.clone()
    }

    pub fn try_send_realtime(
        &self,
        frame: RealtimeInputFrame,
    ) -> Result<RealtimeSendOutcome, TransportSendError> {
        let _admission = self.inner.admission.lock().expect("qos admission poisoned");
        if self.is_tombstoned(frame.session_epoch) {
            return Ok(RealtimeSendOutcome::DroppedLatest);
        }
        let Some(connection) = &self.inner.realtime_connection else {
            return Ok(RealtimeSendOutcome::DroppedLatest);
        };
        let encoded = crate::codec::RealtimeInputCodec::encode(&frame)
            .map_err(|_| TransportSendError::UnsupportedMessage)?;
        match connection.send_datagram(encoded) {
            Ok(()) => Ok(RealtimeSendOutcome::Sent),
            Err(_) => Ok(RealtimeSendOutcome::DroppedLatest),
        }
    }

    pub fn try_send_reliable_input(
        &self,
        frame: ReliableInputFrame,
    ) -> Result<(), TransportSendError> {
        let _admission = self.inner.admission.lock().expect("qos admission poisoned");
        {
            let mut state = self
                .inner
                .epoch_state
                .lock()
                .expect("qos epoch state poisoned");
            match &frame.event {
                ReliableInputEvent::Enter { .. } => {
                    if state.active.is_some() || state.retired(frame.session_epoch) {
                        return Err(TransportSendError::UnsupportedMessage);
                    }
                    state.active = Some(frame.session_epoch);
                }
                ReliableInputEvent::ReleaseAll { .. } => {
                    if state.active != Some(frame.session_epoch) {
                        return Err(TransportSendError::UnsupportedMessage);
                    }
                    state.retire(frame.session_epoch);
                    drop(state);
                    self.cancel_reliable_through_locked(frame.session_epoch);
                    return self.enqueue_emergency_locked(frame);
                }
                _ => {
                    if state.active != Some(frame.session_epoch)
                        || state.retired(frame.session_epoch)
                    {
                        return Err(TransportSendError::UnsupportedMessage);
                    }
                }
            }
        }
        match self.inner.reliable_tx.try_send(frame.clone()) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.fail_reliable_frame(frame)?;
                Err(TransportSendError::ReliableLaneFull)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                let _ = self.fail_reliable_frame(frame);
                Err(TransportSendError::LaneClosed)
            }
        }
    }

    pub fn try_send_emergency(&self, frame: ReliableInputFrame) -> Result<(), TransportSendError> {
        let _admission = self.inner.admission.lock().expect("qos admission poisoned");
        if !matches!(frame.event, ReliableInputEvent::ReleaseAll { .. }) {
            return Err(TransportSendError::UnsupportedMessage);
        }
        let state = self
            .inner
            .epoch_state
            .lock()
            .expect("qos epoch state poisoned");
        if state.retired(frame.session_epoch) {
            return Err(TransportSendError::UnsupportedMessage);
        }
        let active = state.active;
        drop(state);
        if let Some(active) = active {
            if active != frame.session_epoch {
                self.fail_future_emergency_locked(active);
                return Err(TransportSendError::UnsupportedMessage);
            }
        }
        self.retire_epoch_locked(frame.session_epoch);
        self.cancel_reliable_through_locked(frame.session_epoch);
        self.enqueue_emergency_locked(frame)
    }

    fn enqueue_emergency_locked(
        &self,
        frame: ReliableInputFrame,
    ) -> Result<(), TransportSendError> {
        if !matches!(frame.event, ReliableInputEvent::ReleaseAll { .. }) {
            return Err(TransportSendError::UnsupportedMessage);
        }
        if self
            .inner
            .emergency_reserved
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.fail_emergency_locked(frame);
            return Err(TransportSendError::EmergencySlotFull);
        }
        self.inner
            .emergency_tx
            .try_send(frame.clone())
            .map_err(|error| {
                self.fail_emergency_locked(frame);
                match error {
                    mpsc::error::TrySendError::Full(_) => TransportSendError::EmergencySlotFull,
                    mpsc::error::TrySendError::Closed(_) => TransportSendError::LaneClosed,
                }
            })
    }

    pub async fn send_control(&self, frame: ControlFrame) -> Result<(), TransportSendError> {
        send_awaited(&self.inner.control_tx, frame).await
    }

    pub(crate) async fn send_reliable_compat(
        &self,
        frame: ReliableCompatFrame,
    ) -> Result<(), TransportSendError> {
        send_awaited(&self.inner.reliable_compat_tx, frame).await
    }

    pub async fn send_bulk(&self, frame: BulkFrame) -> Result<(), TransportSendError> {
        send_awaited(&self.inner.bulk_tx, frame).await
    }

    pub fn try_send_telemetry(&self, frame: TelemetryFrame) -> Result<(), TransportSendError> {
        self.inner
            .telemetry_tx
            .try_send(frame)
            .map_err(|_| TransportSendError::LaneClosed)
    }

    pub fn is_tombstoned(&self, epoch: SessionEpoch) -> bool {
        self.inner
            .epoch_state
            .lock()
            .expect("qos epoch state poisoned")
            .retired(epoch)
    }

    #[cfg(test)]
    pub(crate) fn occupy_emergency_slot_for_test(&self) {
        self.inner.emergency_reserved.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn drop_probe_for_test(&self) -> QosDropProbe {
        QosDropProbe(Arc::downgrade(&self.inner))
    }

    #[cfg(test)]
    pub(crate) fn worker_probe_for_test(&self) -> QosWorkerProbe {
        QosWorkerProbe(self.inner.worker_count.clone())
    }

    #[cfg(test)]
    pub(crate) fn reliable_write_counts_for_test(&self) -> (u64, u64) {
        (
            self.inner.reliable_write_started.load(Ordering::Acquire),
            self.inner.reliable_write_completed.load(Ordering::Acquire),
        )
    }

    #[cfg(test)]
    pub(crate) fn reliable_available_for_test(&self) -> usize {
        self.inner.reliable_tx.capacity()
    }

    #[cfg(test)]
    pub(crate) fn set_reliable_preface_barrier_for_test(&self, barrier: Arc<tokio::sync::Notify>) {
        *self
            .inner
            .reliable_preface_barrier
            .lock()
            .expect("reliable preface barrier poisoned") = Some(barrier);
    }

    #[cfg(test)]
    pub(crate) fn reliable_preface_waiting_for_test(&self) -> bool {
        self.inner
            .reliable_preface_barrier_waiting
            .load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn reliable_cancel_reset_succeeded_for_test(&self) -> u64 {
        self.inner
            .reliable_cancel_reset_succeeded
            .load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn pause_reliable_payload_poll_for_test(&self) {
        self.inner
            .reliable_payload_poll_paused
            .store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn awaited_write_counts_for_test(&self) -> (u64, u64, u64, u64) {
        (
            self.inner
                .awaited_write_probes
                .started
                .load(Ordering::Acquire),
            self.inner
                .awaited_write_probes
                .completed
                .load(Ordering::Acquire),
            self.inner
                .awaited_write_probes
                .cancelled
                .load(Ordering::Acquire),
            self.inner
                .awaited_write_probes
                .reset_succeeded
                .load(Ordering::Acquire),
        )
    }

    #[cfg(test)]
    pub(crate) fn publish_cancel_for_test(&self, epoch: SessionEpoch) {
        let _admission = self.inner.admission.lock().expect("qos admission poisoned");
        self.retire_epoch_locked(epoch);
        self.cancel_reliable_through_locked(epoch);
    }

    #[cfg(test)]
    pub(crate) fn set_awaited_preface_barrier_for_test(&self, barrier: Arc<tokio::sync::Notify>) {
        *self
            .inner
            .awaited_write_probes
            .preface_barrier
            .lock()
            .expect("awaited preface barrier poisoned") = Some(barrier);
    }

    #[cfg(test)]
    pub(crate) fn awaited_preface_waiting_for_test(&self) -> bool {
        self.inner
            .awaited_write_probes
            .preface_barrier_waiting
            .load(Ordering::Acquire)
    }

    fn fail_emergency_locked(&self, frame: ReliableInputFrame) {
        self.retire_epoch_locked(frame.session_epoch);
        if let Some(connection) = &self.inner.realtime_connection {
            connection.close(0u32.into(), b"emergency input slot unavailable");
        }
        let reason = match frame.event {
            ReliableInputEvent::ReleaseAll { reason } => reason,
            _ => ReleaseAllReason::BackendFailure,
        };
        self.inner.release_emitter.emit(TerminalReleaseEvent {
            auth: self.inner.auth.clone(),
            epoch: frame.session_epoch,
            reason,
        });
    }

    fn fail_future_emergency_locked(&self, active: SessionEpoch) {
        self.retire_epoch_locked(active);
        self.cancel_reliable_through_locked(active);
        if let Some(connection) = &self.inner.realtime_connection {
            connection.close(0u32.into(), b"future emergency epoch mismatch");
        }
        self.inner.release_emitter.emit(TerminalReleaseEvent {
            auth: self.inner.auth.clone(),
            epoch: active,
            reason: ReleaseAllReason::BackendFailure,
        });
    }

    fn fail_reliable_frame(&self, frame: ReliableInputFrame) -> Result<(), TransportSendError> {
        self.retire_epoch_locked(frame.session_epoch);
        self.cancel_reliable_through_locked(frame.session_epoch);
        let terminal = ReliableInputFrame {
            protocol_version: frame.protocol_version,
            session_epoch: frame.session_epoch,
            sequence: frame.sequence,
            captured_at: frame.captured_at,
            event: ReliableInputEvent::ReleaseAll {
                reason: ReleaseAllReason::BackendFailure,
            },
        };
        self.enqueue_emergency_locked(terminal)
    }

    fn retire_epoch_locked(&self, epoch: SessionEpoch) {
        self.inner
            .epoch_state
            .lock()
            .expect("qos epoch state poisoned")
            .retire(epoch);
    }

    fn cancel_reliable_through_locked(&self, epoch: SessionEpoch) {
        let current = *self.inner.reliable_cancelled_through_tx.borrow();
        if current.is_none_or(|cancelled| epoch.0 > cancelled.0) {
            self.inner
                .reliable_cancelled_through_tx
                .send_replace(Some(epoch));
        }
    }
}

impl ControlFrame {
    pub fn heartbeat(sequence: u64, timestamp: u64) -> Self {
        Self {
            message: Message::Heartbeat {
                sequence,
                timestamp,
            },
        }
    }

    fn into_message(self) -> Result<Message, TransportSendError> {
        Ok(self.message)
    }
}

impl BulkFrame {
    pub fn audio_stream_stop(stream_id: uuid::Uuid, reason: String) -> Self {
        Self {
            message: Message::AudioStreamStop { stream_id, reason },
        }
    }

    #[cfg(test)]
    pub(crate) fn test_payload(size: usize) -> Self {
        Self {
            message: Message::ClipboardData {
                mime_type: "application/octet-stream".into(),
                data: vec![0; size],
            },
        }
    }

    fn into_message(self) -> Result<Message, TransportSendError> {
        Ok(self.message)
    }
}

impl TelemetryFrame {
    pub fn latency_probe(
        sequence: u64,
        timestamp_ms: u64,
        endpoint_switch: bool,
        origin_sequence: Option<u64>,
    ) -> Self {
        Self {
            message: Message::LatencyProbe {
                sequence,
                timestamp_ms,
                endpoint_switch,
                origin_sequence,
            },
        }
    }

    fn into_message(self) -> Message {
        self.message
    }
}

impl ReliableCompatFrame {
    fn into_message(self) -> Result<Message, TransportSendError> {
        Ok(self.message)
    }
}

struct WorkerGuard(Arc<AtomicUsize>);

impl WorkerGuard {
    fn new(count: Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::AcqRel);
        Self(count)
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn reliable_frame_cancelled(
    cancelled_through_rx: &mut watch::Receiver<Option<SessionEpoch>>,
    epoch: SessionEpoch,
) -> bool {
    cancelled_through_rx
        .borrow_and_update()
        .is_some_and(|cancelled| cancelled.0 >= epoch.0)
}

fn reset_cancelled_reliable_stream(
    stream: &mut Option<quinn::SendStream>,
    _inner_weak: &std::sync::Weak<PeerTransportInner>,
) {
    let reset_succeeded = stream
        .as_mut()
        .is_some_and(|active| active.reset(TERMINAL_CANCEL_RESET_CODE.into()).is_ok());
    #[cfg(test)]
    if reset_succeeded {
        if let Some(inner) = _inner_weak.upgrade() {
            inner
                .reliable_cancel_reset_succeeded
                .fetch_add(1, Ordering::AcqRel);
        }
    }
    #[cfg(not(test))]
    let _ = reset_succeeded;
    *stream = None;
}

fn fail_reliable_writer_frame(
    inner_weak: &std::sync::Weak<PeerTransportInner>,
    frame: ReliableInputFrame,
) {
    let Some(inner) = inner_weak.upgrade() else {
        return;
    };
    let handle = PeerTransportHandle {
        inner: inner.clone(),
    };
    let _admission = inner.admission.lock().expect("qos admission poisoned");
    let _ = handle.fail_reliable_frame(frame);
}

fn spawn_reliable_writer(
    inner_weak: std::sync::Weak<PeerTransportInner>,
    connection: quinn::Connection,
    mut rx: mpsc::Receiver<ReliableInputFrame>,
    mut cancelled_through_rx: watch::Receiver<Option<SessionEpoch>>,
    worker_count: Arc<AtomicUsize>,
) {
    let guard = WorkerGuard::new(worker_count);
    tokio::spawn(async move {
        let _guard = guard;
        let mut stream: Option<quinn::SendStream> = None;
        'frames: while let Some(frame) = rx.recv().await {
            let Some(inner) = inner_weak.upgrade() else {
                return;
            };
            if reliable_frame_cancelled(&mut cancelled_through_rx, frame.session_epoch) {
                continue;
            }
            if inner
                .epoch_state
                .lock()
                .expect("qos epoch state poisoned")
                .retired(frame.session_epoch)
            {
                continue;
            }
            drop(inner);
            if stream.is_none() {
                let open = connection.open_uni();
                tokio::pin!(open);
                let opened = loop {
                    tokio::select! {
                        biased;
                        changed = cancelled_through_rx.changed() => {
                            if changed.is_err() {
                                return;
                            }
                            if reliable_frame_cancelled(
                                &mut cancelled_through_rx,
                                frame.session_epoch,
                            ) {
                                continue 'frames;
                            }
                        }
                        result = &mut open => break result,
                    }
                };
                let opened = match opened {
                    Ok(opened) => opened,
                    Err(_)
                        if reliable_frame_cancelled(
                            &mut cancelled_through_rx,
                            frame.session_epoch,
                        ) =>
                    {
                        continue;
                    }
                    Err(_) => {
                        fail_reliable_writer_frame(&inner_weak, frame);
                        return;
                    }
                };
                stream = Some(opened);
                if reliable_frame_cancelled(&mut cancelled_through_rx, frame.session_epoch) {
                    reset_cancelled_reliable_stream(&mut stream, &inner_weak);
                    continue;
                }
                let preface_result = {
                    let opened = stream
                        .as_mut()
                        .expect("reliable stream was just initialized");
                    let _ = opened.set_priority(100);
                    let preface = async {
                        #[cfg(test)]
                        {
                            let barrier = inner_weak.upgrade().and_then(|inner| {
                                inner
                                    .reliable_preface_barrier
                                    .lock()
                                    .expect("reliable preface barrier poisoned")
                                    .take()
                            });
                            if let Some(barrier) = barrier {
                                if let Some(inner) = inner_weak.upgrade() {
                                    inner
                                        .reliable_preface_barrier_waiting
                                        .store(true, Ordering::Release);
                                }
                                barrier.notified().await;
                                if let Some(inner) = inner_weak.upgrade() {
                                    inner
                                        .reliable_preface_barrier_waiting
                                        .store(false, Ordering::Release);
                                }
                            }
                        }
                        opened
                            .write_all(&[
                                QOS_LANE_MAGIC[0],
                                QOS_LANE_MAGIC[1],
                                QOS_LANE_MAGIC[2],
                                QOS_LANE_MAGIC[3],
                                LaneDiscriminator::ReliableInput as u8,
                            ])
                            .await
                    };
                    tokio::pin!(preface);
                    loop {
                        tokio::select! {
                            biased;
                            changed = cancelled_through_rx.changed() => {
                                if changed.is_err() {
                                    return;
                                }
                                if reliable_frame_cancelled(
                                    &mut cancelled_through_rx,
                                    frame.session_epoch,
                                ) {
                                    break None;
                                }
                            }
                            result = &mut preface => break Some(result),
                        }
                    }
                };
                if preface_result.is_none()
                    || reliable_frame_cancelled(&mut cancelled_through_rx, frame.session_epoch)
                {
                    reset_cancelled_reliable_stream(&mut stream, &inner_weak);
                    continue;
                }
                if preface_result.is_some_and(|result| result.is_err()) {
                    let _ = stream
                        .as_mut()
                        .expect("failed reliable stream remains initialized")
                        .reset(0u32.into());
                    fail_reliable_writer_frame(&inner_weak, frame);
                    return;
                }
            }
            let Ok(encoded) = crate::codec::ReliableInputCodec::encode(&frame) else {
                fail_reliable_writer_frame(&inner_weak, frame);
                return;
            };
            let mut wire = Vec::with_capacity(4 + encoded.len());
            wire.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
            wire.extend_from_slice(&encoded);
            let Some(inner) = inner_weak.upgrade() else {
                return;
            };
            inner.reliable_write_started.fetch_add(1, Ordering::AcqRel);
            drop(inner);
            let result = {
                let active = stream.as_mut().expect("reliable stream initialized");
                let write_all = active.write_all(&wire);
                tokio::pin!(write_all);
                let write = std::future::poll_fn(|cx| {
                    #[cfg(test)]
                    if inner_weak.upgrade().is_some_and(|inner| {
                        inner.reliable_payload_poll_paused.load(Ordering::Acquire)
                    }) {
                        return std::task::Poll::Pending;
                    }
                    std::future::Future::poll(write_all.as_mut(), cx)
                });
                tokio::pin!(write);
                loop {
                    tokio::select! {
                        biased;
                        changed = cancelled_through_rx.changed() => {
                            if changed.is_err() {
                                return;
                            }
                            if reliable_frame_cancelled(
                                &mut cancelled_through_rx,
                                frame.session_epoch,
                            ) {
                                break None;
                            }
                        }
                        result = &mut write => break Some(result),
                        _ = connection.closed() => {
                            fail_reliable_writer_frame(&inner_weak, frame);
                            return;
                        },
                    }
                }
            };
            if result.is_none()
                || reliable_frame_cancelled(&mut cancelled_through_rx, frame.session_epoch)
            {
                reset_cancelled_reliable_stream(&mut stream, &inner_weak);
                continue;
            }
            let result = result.expect("non-cancelled reliable write has a result");
            let Some(inner) = inner_weak.upgrade() else {
                return;
            };
            inner
                .reliable_write_completed
                .fetch_add(1, Ordering::AcqRel);
            if result.is_err() {
                let _ = stream
                    .as_mut()
                    .expect("failed reliable stream remains initialized")
                    .reset(0u32.into());
                drop(inner);
                fail_reliable_writer_frame(&inner_weak, frame);
                return;
            }
        }
    });
}

fn spawn_emergency_writer(
    inner_weak: std::sync::Weak<PeerTransportInner>,
    connection: quinn::Connection,
    mut rx: mpsc::Receiver<ReliableInputFrame>,
    worker_count: Arc<AtomicUsize>,
) {
    let guard = WorkerGuard::new(worker_count);
    tokio::spawn(async move {
        let _guard = guard;
        while let Some(frame) = rx.recv().await {
            let Some(inner) = inner_weak.upgrade() else {
                return;
            };
            drop(inner);
            let result = async {
                let mut stream = connection.open_uni().await?;
                let _ = stream.set_priority(255);
                stream
                    .write_all(&[
                        QOS_LANE_MAGIC[0],
                        QOS_LANE_MAGIC[1],
                        QOS_LANE_MAGIC[2],
                        QOS_LANE_MAGIC[3],
                        LaneDiscriminator::Emergency as u8,
                    ])
                    .await?;
                let encoded = crate::codec::ReliableInputCodec::encode(&frame)
                    .map_err(|_| quinn::WriteError::ClosedStream)?;
                stream
                    .write_all(&(encoded.len() as u32).to_be_bytes())
                    .await?;
                stream.write_all(&encoded).await?;
                stream.finish()?;
                Ok::<(), quinn::WriteError>(())
            }
            .await;
            let Some(inner) = inner_weak.upgrade() else {
                return;
            };
            if result.is_ok() {
                inner.emergency_reserved.store(false, Ordering::Release);
            } else {
                let handle = PeerTransportHandle {
                    inner: inner.clone(),
                };
                {
                    let _admission = inner.admission.lock().expect("qos admission poisoned");
                    handle.fail_emergency_locked(frame);
                }
                return;
            }
        }
    });
}

fn spawn_awaited_writer<T, F>(
    connection: quinn::Connection,
    lane: LaneDiscriminator,
    mut rx: mpsc::Receiver<AwaitedFrame<T>>,
    convert: F,
    worker_count: Arc<AtomicUsize>,
    _probes: Arc<AwaitedWriteProbes>,
) where
    T: Send + 'static,
    F: Fn(T) -> Result<Message, TransportSendError> + Send + Sync + 'static,
{
    let guard = WorkerGuard::new(worker_count);
    tokio::spawn(async move {
        let _guard = guard;
        let mut stream: Option<quinn::SendStream> = None;
        while let Some(AwaitedFrame { frame, mut written }) = rx.recv().await {
            let result = match convert(frame) {
                Ok(message) => {
                    #[cfg(test)]
                    _probes.started.fetch_add(1, Ordering::AcqRel);
                    tokio::select! {
                        result = write_qos_message(
                            &connection,
                            lane,
                            &mut stream,
                            &message,
                            Some(&_probes),
                        ) => {
                            #[cfg(test)]
                            _probes.completed.fetch_add(1, Ordering::AcqRel);
                            Some(result)
                        },
                        _ = written.closed() => {
                            #[cfg(test)]
                            _probes.cancelled.fetch_add(1, Ordering::AcqRel);
                            None
                        },
                        _ = connection.closed() => return,
                    }
                }
                Err(error) => Some(Err(error)),
            };
            if let Some(result) = result {
                let _ = written.send(result);
            } else if let Some(mut abandoned) = stream.take() {
                if abandoned.reset(AWAITED_CANCEL_RESET_CODE.into()).is_ok() {
                    #[cfg(test)]
                    _probes.reset_succeeded.fetch_add(1, Ordering::AcqRel);
                }
            }
        }
    });
}

fn spawn_telemetry_writer(
    connection: quinn::Connection,
    mut rx: mpsc::Receiver<TelemetryFrame>,
    worker_count: Arc<AtomicUsize>,
) {
    let guard = WorkerGuard::new(worker_count);
    tokio::spawn(async move {
        let _guard = guard;
        let mut stream = None;
        while let Some(frame) = rx.recv().await {
            let message = frame.into_message();
            tokio::select! {
                _ = write_qos_message(
                    &connection,
                    LaneDiscriminator::Telemetry,
                    &mut stream,
                    &message,
                    None,
                ) => {}
                _ = connection.closed() => return,
            }
        }
    });
}

async fn write_qos_message(
    connection: &quinn::Connection,
    lane: LaneDiscriminator,
    stream: &mut Option<quinn::SendStream>,
    message: &Message,
    _probes: Option<&AwaitedWriteProbes>,
) -> Result<(), TransportSendError> {
    if stream.is_none() {
        let opened = connection
            .open_uni()
            .await
            .map_err(|_| TransportSendError::LaneClosed)?;
        *stream = Some(opened);
        let opened = stream.as_mut().expect("qos stream was just initialized");
        let _ = opened.set_priority(match lane {
            LaneDiscriminator::Control => 80,
            LaneDiscriminator::Bulk => -10,
            LaneDiscriminator::Telemetry => 0,
            _ => 0,
        });
        let preface = [
            QOS_LANE_MAGIC[0],
            QOS_LANE_MAGIC[1],
            QOS_LANE_MAGIC[2],
            QOS_LANE_MAGIC[3],
            lane as u8,
        ];
        #[cfg(test)]
        if let Some(probes) = _probes {
            let barrier = probes
                .preface_barrier
                .lock()
                .expect("awaited preface barrier poisoned")
                .take();
            if let Some(barrier) = barrier {
                probes
                    .preface_barrier_waiting
                    .store(true, Ordering::Release);
                barrier.notified().await;
                probes
                    .preface_barrier_waiting
                    .store(false, Ordering::Release);
            }
        }
        if opened.write_all(&preface).await.is_err() {
            *stream = None;
            return Err(TransportSendError::LaneClosed);
        }
    }
    let payload = crate::codec::ControlMessageCodec::encode(message)
        .map_err(|_| TransportSendError::UnsupportedMessage)?;
    let mut wire = Vec::with_capacity(4 + payload.len());
    wire.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    wire.extend_from_slice(&payload);
    if stream
        .as_mut()
        .expect("qos stream initialized")
        .write_all(&wire)
        .await
        .is_err()
    {
        *stream = None;
        return Err(TransportSendError::LaneClosed);
    }
    Ok(())
}

async fn send_awaited<T>(
    tx: &mpsc::Sender<AwaitedFrame<T>>,
    frame: T,
) -> Result<(), TransportSendError> {
    let (written, ack) = oneshot::channel();
    tx.send(AwaitedFrame { frame, written })
        .await
        .map_err(|_| TransportSendError::LaneClosed)?;
    ack.await.map_err(|_| TransportSendError::LaneClosed)?
}

#[derive(Clone)]
pub struct RegisteredPeer {
    pub auth: Arc<PeerAuthContext>,
    pub transport: PeerTransportHandle,
}

pub struct ConnectionRegistry {
    peers: RwLock<HashMap<DeviceId, RegisteredPeer>>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self {
            peers: RwLock::new(HashMap::new()),
        }
    }

    pub fn insert(&self, id: DeviceId, peer: RegisteredPeer) -> Option<RegisteredPeer> {
        self.peers
            .write()
            .expect("connection registry poisoned")
            .insert(id, peer)
    }

    pub fn peer(&self, id: &DeviceId) -> Option<RegisteredPeer> {
        self.peers
            .read()
            .expect("connection registry poisoned")
            .get(id)
            .cloned()
    }

    pub fn snapshot(&self) -> Vec<(DeviceId, RegisteredPeer)> {
        self.peers
            .read()
            .expect("connection registry poisoned")
            .iter()
            .map(|(id, peer)| (*id, peer.clone()))
            .collect()
    }

    pub fn remove_if_generation(
        &self,
        id: DeviceId,
        connection_id: ControlConnectionId,
    ) -> Option<RegisteredPeer> {
        let mut peers = self.peers.write().expect("connection registry poisoned");
        if peers
            .get(&id)
            .is_some_and(|peer| peer.auth.control_connection_id == connection_id)
        {
            peers.remove(&id)
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.peers
            .read()
            .expect("connection registry poisoned")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clones all handles while the read lock is held, then performs writes
    /// concurrently without retaining the registry lock across `.await`.
    pub async fn broadcast_control(
        &self,
        frame: ControlFrame,
    ) -> Vec<(DeviceId, Result<(), TransportSendError>)> {
        let peers = self.snapshot();
        let mut sends = FuturesUnordered::new();
        for (device_id, peer) in peers {
            let frame = frame.clone();
            sends.push(async move { (device_id, peer.transport.send_control(frame).await) });
        }
        let mut results = Vec::new();
        while let Some(result) = sends.next().await {
            results.push(result);
        }
        results
    }

    pub(crate) async fn broadcast_control_with_generation(
        &self,
        frame: ControlFrame,
    ) -> Vec<(
        DeviceId,
        ControlConnectionId,
        Result<(), TransportSendError>,
    )> {
        let mut sends = FuturesUnordered::new();
        for (device_id, peer) in self.snapshot() {
            let frame = frame.clone();
            sends.push(async move {
                (
                    device_id,
                    peer.auth.control_connection_id,
                    peer.transport.send_control(frame).await,
                )
            });
        }
        let mut results = Vec::new();
        while let Some(result) = sends.next().await {
            results.push(result);
        }
        results
    }

    pub async fn broadcast_bulk(
        &self,
        frame: BulkFrame,
    ) -> Vec<(DeviceId, Result<(), TransportSendError>)> {
        let peers = self.snapshot();
        let mut sends = FuturesUnordered::new();
        for (device_id, peer) in peers {
            let frame = frame.clone();
            sends.push(async move { (device_id, peer.transport.send_bulk(frame).await) });
        }
        let mut results = Vec::new();
        while let Some(result) = sends.next().await {
            results.push(result);
        }
        results
    }

    pub(crate) async fn broadcast_bulk_with_generation(
        &self,
        frame: BulkFrame,
    ) -> Vec<(
        DeviceId,
        ControlConnectionId,
        Result<(), TransportSendError>,
    )> {
        let mut sends = FuturesUnordered::new();
        for (device_id, peer) in self.snapshot() {
            let frame = frame.clone();
            sends.push(async move {
                (
                    device_id,
                    peer.auth.control_connection_id,
                    peer.transport.send_bulk(frame).await,
                )
            });
        }
        let mut results = Vec::new();
        while let Some(result) = sends.next().await {
            results.push(result);
        }
        results
    }

    pub(crate) async fn broadcast_reliable_compat(
        &self,
        frame: ReliableCompatFrame,
    ) -> Vec<(DeviceId, Result<(), TransportSendError>)> {
        let mut sends = FuturesUnordered::new();
        for (device_id, peer) in self.snapshot() {
            let frame = frame.clone();
            sends
                .push(async move { (device_id, peer.transport.send_reliable_compat(frame).await) });
        }
        let mut results = Vec::new();
        while let Some(result) = sends.next().await {
            results.push(result);
        }
        results
    }

    pub(crate) async fn broadcast_reliable_compat_with_generation(
        &self,
        frame: ReliableCompatFrame,
    ) -> Vec<(
        DeviceId,
        ControlConnectionId,
        Result<(), TransportSendError>,
    )> {
        let mut sends = FuturesUnordered::new();
        for (device_id, peer) in self.snapshot() {
            let frame = frame.clone();
            sends.push(async move {
                (
                    device_id,
                    peer.auth.control_connection_id,
                    peer.transport.send_reliable_compat(frame).await,
                )
            });
        }
        let mut results = Vec::new();
        while let Some(result) = sends.next().await {
            results.push(result);
        }
        results
    }

    pub fn broadcast_telemetry(
        &self,
        frame: TelemetryFrame,
    ) -> Vec<(DeviceId, Result<(), TransportSendError>)> {
        self.snapshot()
            .into_iter()
            .map(|(device_id, peer)| (device_id, peer.transport.try_send_telemetry(frame.clone())))
            .collect()
    }

    pub(crate) fn broadcast_telemetry_with_generation(
        &self,
        frame: TelemetryFrame,
    ) -> Vec<(
        DeviceId,
        ControlConnectionId,
        Result<(), TransportSendError>,
    )> {
        self.snapshot()
            .into_iter()
            .map(|(device_id, peer)| {
                (
                    device_id,
                    peer.auth.control_connection_id,
                    peer.transport.try_send_telemetry(frame.clone()),
                )
            })
            .collect()
    }

    pub fn broadcast_realtime(
        &self,
        frame: RealtimeInputFrame,
    ) -> Vec<(DeviceId, Result<RealtimeSendOutcome, TransportSendError>)> {
        self.snapshot()
            .into_iter()
            .map(|(device_id, peer)| (device_id, peer.transport.try_send_realtime(frame.clone())))
            .collect()
    }
}

impl Default for ConnectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
struct QosProbe {
    reliable_rx: mpsc::Receiver<ReliableInputFrame>,
}

#[cfg(test)]
fn fixture_auth(peer_id: DeviceId, connection_id: ControlConnectionId) -> Arc<PeerAuthContext> {
    Arc::new(PeerAuthContext {
        peer_id,
        certificate_fingerprint: crate::encryption::PeerCertificateFingerprint::from_der(b"test"),
        control_connection_id: connection_id,
    })
}

#[cfg(test)]
fn fixture_handle(
    reliable_capacity: usize,
) -> (
    PeerTransportHandle,
    QosProbe,
    mpsc::Receiver<TerminalReleaseEvent>,
) {
    let auth = fixture_auth(DeviceId::new_v4(), ControlConnectionId::new());
    let (reliable_tx, reliable_rx) = mpsc::channel(reliable_capacity);
    let (emergency_tx, mut emergency_rx) = mpsc::channel::<ReliableInputFrame>(1);
    let (control_tx, _control_rx) = mpsc::channel(1);
    let (reliable_compat_tx, _reliable_compat_rx) = mpsc::channel(1);
    let (bulk_tx, _bulk_rx) = mpsc::channel(1);
    let (telemetry_tx, _telemetry_rx) = mpsc::channel(1);
    let (release_tx, release_rx) = mpsc::channel(4);
    let release_emitter = TerminalReleaseEmitter::new(release_tx.clone());
    let worker_release_tx = release_tx.clone();
    let (reliable_cancelled_through_tx, _reliable_cancelled_through_rx) = watch::channel(None);
    let release_auth = auth.clone();
    tokio::spawn(async move {
        while let Some(frame) = emergency_rx.recv().await {
            if let ReliableInputEvent::ReleaseAll { reason } = frame.event {
                let _ = worker_release_tx
                    .send(TerminalReleaseEvent {
                        auth: release_auth.clone(),
                        epoch: frame.session_epoch,
                        reason,
                    })
                    .await;
            }
        }
    });
    (
        PeerTransportHandle {
            inner: Arc::new(PeerTransportInner {
                auth,
                realtime_connection: None,
                reliable_tx,
                emergency_tx,
                control_tx,
                reliable_compat_tx,
                bulk_tx,
                telemetry_tx,
                epoch_state: Mutex::new(SenderEpochState::default()),
                admission: Mutex::new(()),
                reliable_cancelled_through_tx,
                release_emitter,
                emergency_reserved: AtomicBool::new(false),
                reliable_write_started: std::sync::atomic::AtomicU64::new(0),
                reliable_write_completed: std::sync::atomic::AtomicU64::new(0),
                #[cfg(test)]
                reliable_preface_barrier: Mutex::new(None),
                #[cfg(test)]
                reliable_preface_barrier_waiting: AtomicBool::new(false),
                #[cfg(test)]
                reliable_cancel_reset_succeeded: std::sync::atomic::AtomicU64::new(0),
                #[cfg(test)]
                reliable_payload_poll_paused: AtomicBool::new(false),
                #[cfg(test)]
                awaited_write_probes: Arc::new(AwaitedWriteProbes::default()),
                worker_count: Arc::new(AtomicUsize::new(0)),
            }),
        },
        QosProbe { reliable_rx },
        release_rx,
    )
}

#[cfg(test)]
fn qos_fixture_with_blocked_bulk() -> (PeerTransportHandle, QosProbe) {
    let (handle, probe, _releases) = fixture_handle(256);
    (handle, probe)
}

#[cfg(test)]
fn qos_fixture_with_blocked_reliable(
    capacity: usize,
) -> (
    PeerTransportHandle,
    QosProbe,
    mpsc::Receiver<TerminalReleaseEvent>,
) {
    fixture_handle(capacity)
}

#[cfg(test)]
fn registered_peer_fixture(
    peer_id: DeviceId,
    connection_id: ControlConnectionId,
) -> RegisteredPeer {
    let (transport, _probe, _releases) = fixture_handle(1);
    RegisteredPeer {
        auth: fixture_auth(peer_id, connection_id),
        transport,
    }
}

#[cfg(test)]
fn fixture_handle_with_stalled_emergency() -> (
    PeerTransportHandle,
    mpsc::Receiver<ReliableInputFrame>,
    mpsc::Receiver<TerminalReleaseEvent>,
) {
    let auth = fixture_auth(DeviceId::new_v4(), ControlConnectionId::new());
    let (reliable_tx, _reliable_rx) = mpsc::channel(1);
    let (emergency_tx, emergency_rx) = mpsc::channel(1);
    let (control_tx, _control_rx) = mpsc::channel(1);
    let (reliable_compat_tx, _reliable_compat_rx) = mpsc::channel(1);
    let (bulk_tx, _bulk_rx) = mpsc::channel(1);
    let (telemetry_tx, _telemetry_rx) = mpsc::channel(1);
    let (release_tx, release_rx) = mpsc::channel(4);
    let release_emitter = TerminalReleaseEmitter::new(release_tx);
    let (reliable_cancelled_through_tx, _reliable_cancelled_through_rx) = watch::channel(None);
    (
        PeerTransportHandle {
            inner: Arc::new(PeerTransportInner {
                auth,
                realtime_connection: None,
                reliable_tx,
                emergency_tx,
                control_tx,
                reliable_compat_tx,
                bulk_tx,
                telemetry_tx,
                epoch_state: Mutex::new(SenderEpochState::default()),
                admission: Mutex::new(()),
                reliable_cancelled_through_tx,
                release_emitter,
                emergency_reserved: AtomicBool::new(false),
                reliable_write_started: std::sync::atomic::AtomicU64::new(0),
                reliable_write_completed: std::sync::atomic::AtomicU64::new(0),
                #[cfg(test)]
                reliable_preface_barrier: Mutex::new(None),
                #[cfg(test)]
                reliable_preface_barrier_waiting: AtomicBool::new(false),
                #[cfg(test)]
                reliable_cancel_reset_succeeded: std::sync::atomic::AtomicU64::new(0),
                #[cfg(test)]
                reliable_payload_poll_paused: AtomicBool::new(false),
                #[cfg(test)]
                awaited_write_probes: Arc::new(AwaitedWriteProbes::default()),
                worker_count: Arc::new(AtomicUsize::new(0)),
            }),
        },
        emergency_rx,
        release_rx,
    )
}

#[cfg(test)]
fn fixture_registry_with_slow_and_fast_peers() -> (
    ConnectionRegistry,
    PeerTransportHandle,
    PeerTransportHandle,
    mpsc::Receiver<AwaitedFrame<ControlFrame>>,
    mpsc::Receiver<ReliableInputFrame>,
) {
    let registry = ConnectionRegistry::new();
    let slow_id = DeviceId::new_v4();
    let fast_id = DeviceId::new_v4();
    let (mut slow, _slow_probe, _slow_releases) = fixture_handle(1);
    let (slow_control_tx, slow_control_rx) = mpsc::channel(1);
    Arc::get_mut(&mut slow.inner)
        .expect("fixture handle must have unique ownership")
        .control_tx = slow_control_tx;
    let (fast, fast_probe, _fast_releases) = fixture_handle(1);
    registry.insert(
        slow_id,
        RegisteredPeer {
            auth: slow.auth(),
            transport: slow.clone(),
        },
    );
    registry.insert(
        fast_id,
        RegisteredPeer {
            auth: fast.auth(),
            transport: fast.clone(),
        },
    );
    (
        registry,
        slow,
        fast,
        slow_control_rx,
        fast_probe.reliable_rx,
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use tokio::time::timeout;

    use rshare_core::{
        ClockDomainId, ControlConnectionId, DeviceId, KeyState, MonotonicStamp, ReliableInputEvent,
        ReliableInputFrame, SessionEpoch, INPUT_PROTOCOL_VERSION,
    };

    use super::*;

    fn reliable_key_down(epoch: u64, sequence: u64) -> ReliableInputFrame {
        ReliableInputFrame {
            protocol_version: INPUT_PROTOCOL_VERSION,
            session_epoch: SessionEpoch(epoch),
            sequence,
            captured_at: MonotonicStamp::new(ClockDomainId(1), sequence),
            event: ReliableInputEvent::Key {
                keycode: 0x41,
                state: KeyState::Pressed,
            },
        }
    }

    fn reliable_enter(epoch: u64, sequence: u64) -> ReliableInputFrame {
        ReliableInputFrame {
            protocol_version: INPUT_PROTOCOL_VERSION,
            session_epoch: SessionEpoch(epoch),
            sequence,
            captured_at: MonotonicStamp::new(ClockDomainId(1), sequence),
            event: ReliableInputEvent::Enter {
                target_display_id: "primary".into(),
                x: 0,
                y: 0,
            },
        }
    }

    fn reliable_release(epoch: u64, sequence: u64) -> ReliableInputFrame {
        ReliableInputFrame {
            protocol_version: INPUT_PROTOCOL_VERSION,
            session_epoch: SessionEpoch(epoch),
            sequence,
            captured_at: MonotonicStamp::new(ClockDomainId(1), sequence),
            event: ReliableInputEvent::ReleaseAll {
                reason: ReleaseAllReason::BackendFailure,
            },
        }
    }

    #[tokio::test]
    async fn blocked_bulk_lane_does_not_delay_reliable_input() {
        let (handle, mut probe) = qos_fixture_with_blocked_bulk();
        let bulk = tokio::spawn({
            let handle = handle.clone();
            async move { handle.send_bulk(BulkFrame::test_payload(1_000_000)).await }
        });

        handle
            .try_send_reliable_input(reliable_enter(1, 1))
            .unwrap();
        let received = tokio::time::timeout(Duration::from_millis(100), probe.reliable_rx.recv())
            .await
            .expect("reliable input must bypass blocked bulk")
            .expect("reliable lane must remain open");
        assert_eq!(received.sequence, 1);
        bulk.abort();
    }

    #[tokio::test]
    async fn delayed_old_generation_remove_keeps_replacement() {
        let registry = ConnectionRegistry::new();
        let peer_id = DeviceId::new_v4();
        let old = registered_peer_fixture(peer_id, ControlConnectionId::new());
        let replacement = registered_peer_fixture(peer_id, ControlConnectionId::new());
        let old_id = old.auth.control_connection_id;
        let replacement_id = replacement.auth.control_connection_id;

        registry.insert(peer_id, old);
        registry.insert(peer_id, replacement);

        assert!(registry.remove_if_generation(peer_id, old_id).is_none());
        assert_eq!(
            registry.peer(&peer_id).unwrap().auth.control_connection_id,
            replacement_id
        );
    }

    #[test]
    fn message_classifier_keeps_bulk_and_telemetry_out_of_control() {
        let audio = rshare_core::Message::AudioStreamStop {
            stream_id: uuid::Uuid::new_v4(),
            reason: "done".into(),
        };
        assert!(matches!(
            ClassifiedMessage::try_from(audio).unwrap(),
            ClassifiedMessage::Bulk(_)
        ));

        let diagnostic = rshare_core::Message::LatencyProbe {
            sequence: 1,
            timestamp_ms: 2,
            endpoint_switch: false,
            origin_sequence: None,
        };
        assert!(matches!(
            ClassifiedMessage::try_from(diagnostic).unwrap(),
            ClassifiedMessage::Telemetry(_)
        ));
    }

    #[test]
    fn authenticated_compatibility_messages_have_explicit_closed_lanes() {
        assert!(matches!(
            ClassifiedMessage::try_from(Message::Key {
                keycode: 0x41,
                state: KeyState::Pressed,
            })
            .unwrap(),
            ClassifiedMessage::ReliableCompat(_)
        ));
        assert!(matches!(
            ClassifiedMessage::try_from(Message::ClipboardData {
                mime_type: "text/plain".into(),
                data: vec![1, 2, 3],
            })
            .unwrap(),
            ClassifiedMessage::Bulk(_)
        ));
        assert!(matches!(
            ClassifiedMessage::try_from(Message::AudioStreamStop {
                stream_id: uuid::Uuid::new_v4(),
                reason: "done".into(),
            })
            .unwrap(),
            ClassifiedMessage::Bulk(_)
        ));
        assert!(matches!(
            ClassifiedMessage::try_from(Message::UsbDeviceDetached {
                bus_id: "usb:1-2".into(),
                reason: "gone".into(),
            })
            .unwrap(),
            ClassifiedMessage::Bulk(_)
        ));
    }

    #[tokio::test]
    async fn reliable_overflow_tombstones_before_terminal_release_callback() {
        let (handle, _blocked_reliable, mut releases) = qos_fixture_with_blocked_reliable(1);
        handle
            .try_send_reliable_input(reliable_enter(7, 1))
            .unwrap();
        let overflow = handle
            .try_send_reliable_input(reliable_key_down(7, 2))
            .unwrap_err();
        assert_eq!(overflow, TransportSendError::ReliableLaneFull);

        let release = tokio::time::timeout(Duration::from_millis(100), releases.recv())
            .await
            .expect("terminal release callback must not depend on the blocked reliable stream")
            .expect("release callback lane must remain open");
        assert_eq!(release.epoch, SessionEpoch(7));
        assert!(handle.is_tombstoned(SessionEpoch(7)));
        assert_eq!(
            release.reason,
            rshare_core::ReleaseAllReason::BackendFailure
        );
    }

    #[tokio::test]
    async fn occupied_emergency_slot_fails_closed_with_typed_release() {
        let (handle, _stalled_emergency, mut releases) = fixture_handle_with_stalled_emergency();
        handle.try_send_emergency(reliable_release(9, 1)).unwrap();
        assert_eq!(
            handle.try_send_emergency(reliable_release(10, 1)),
            Err(TransportSendError::EmergencySlotFull)
        );
        let release = releases.recv().await.unwrap();
        assert_eq!(release.epoch, SessionEpoch(10));
        assert_eq!(release.reason, ReleaseAllReason::BackendFailure);
        assert!(handle.is_tombstoned(SessionEpoch(9)));
        assert!(handle.is_tombstoned(SessionEpoch(10)));
    }

    #[tokio::test]
    async fn slow_peer_does_not_block_fast_peer_or_registry_read() {
        let (registry, slow, fast, _blocked_control, mut fast_reliable) =
            fixture_registry_with_slow_and_fast_peers();
        let slow_send =
            tokio::spawn(async move { slow.send_control(ControlFrame::heartbeat(1, 1)).await });

        fast.try_send_reliable_input(reliable_enter(1, 1)).unwrap();
        assert_eq!(registry.snapshot().len(), 2);
        tokio::time::timeout(Duration::from_millis(100), fast_reliable.recv())
            .await
            .expect("fast peer must bypass blocked slow peer")
            .expect("fast peer reliable lane must remain open");
        slow_send.abort();
    }

    #[tokio::test]
    async fn reliable_epoch_requires_enter_and_terminal_retires_immediately() {
        let (handle, _probe, _releases) = fixture_handle(8);
        assert!(
            handle
                .try_send_reliable_input(reliable_key_down(1, 1))
                .is_err(),
            "non-Enter cannot create an epoch"
        );
        let enter = ReliableInputFrame {
            protocol_version: INPUT_PROTOCOL_VERSION,
            session_epoch: SessionEpoch(1),
            sequence: 1,
            captured_at: MonotonicStamp::new(ClockDomainId(1), 1),
            event: ReliableInputEvent::Enter {
                target_display_id: "primary".into(),
                x: 0,
                y: 0,
            },
        };
        handle.try_send_reliable_input(enter).unwrap();
        handle
            .try_send_reliable_input(reliable_release(1, 2))
            .unwrap();
        assert!(handle
            .try_send_reliable_input(reliable_key_down(1, 3))
            .is_err());
        assert!(handle.is_tombstoned(SessionEpoch(1)));
    }

    #[tokio::test]
    async fn future_emergency_releases_only_current_epoch_without_retiring_future() {
        let (handle, _probe, mut releases) = fixture_handle(8);
        handle
            .try_send_reliable_input(reliable_enter(1, 1))
            .unwrap();

        assert_eq!(
            handle.try_send_emergency(reliable_release(2, 1)),
            Err(TransportSendError::UnsupportedMessage)
        );
        let release = releases.recv().await.unwrap();
        assert_eq!(release.epoch, SessionEpoch(1));
        assert_eq!(release.reason, ReleaseAllReason::BackendFailure);
        assert!(handle.is_tombstoned(SessionEpoch(1)));
        assert!(
            !handle.is_tombstoned(SessionEpoch(2)),
            "untrusted future epoch must not advance the retirement high-water mark"
        );
    }

    #[tokio::test]
    async fn delayed_retired_emergency_cannot_release_new_active_epoch() {
        let (handle, mut probe, mut releases) = fixture_handle(8);
        handle
            .try_send_reliable_input(reliable_enter(1, 1))
            .unwrap();
        assert_eq!(
            probe.reliable_rx.recv().await.unwrap().session_epoch,
            SessionEpoch(1)
        );
        handle
            .try_send_reliable_input(reliable_release(1, 2))
            .unwrap();
        assert_eq!(releases.recv().await.unwrap().epoch, SessionEpoch(1));

        handle
            .try_send_reliable_input(reliable_enter(2, 1))
            .unwrap();
        assert_eq!(
            probe.reliable_rx.recv().await.unwrap().session_epoch,
            SessionEpoch(2)
        );
        assert_eq!(
            handle.try_send_emergency(reliable_release(1, 3)),
            Err(TransportSendError::UnsupportedMessage)
        );
        assert!(
            timeout(Duration::from_millis(20), releases.recv())
                .await
                .is_err(),
            "a retired old emergency must not emit a release for the active epoch"
        );
        handle
            .try_send_reliable_input(ReliableInputFrame {
                protocol_version: INPUT_PROTOCOL_VERSION,
                session_epoch: SessionEpoch(2),
                sequence: 2,
                captured_at: rshare_core::MonotonicStamp::new(rshare_core::ClockDomainId(1), 2),
                event: ReliableInputEvent::Key {
                    keycode: 0x41,
                    state: rshare_core::KeyState::Pressed,
                },
            })
            .expect("the current epoch must remain active");
    }

    #[tokio::test]
    async fn cancelled_through_starts_empty_never_decreases_and_accepts_max_epoch() {
        let (handle, _probe, _releases) = fixture_handle(8);
        assert_eq!(*handle.inner.reliable_cancelled_through_tx.borrow(), None);

        handle.publish_cancel_for_test(SessionEpoch(u64::MAX));
        handle.publish_cancel_for_test(SessionEpoch(u64::MAX - 1));

        assert_eq!(
            *handle.inner.reliable_cancelled_through_tx.borrow(),
            Some(SessionEpoch(u64::MAX))
        );
        assert!(handle.is_tombstoned(SessionEpoch(u64::MAX)));
        assert_eq!(
            handle.try_send_reliable_input(reliable_enter(u64::MAX, 1)),
            Err(TransportSendError::UnsupportedMessage)
        );
    }

    #[test]
    fn reliable_cancel_watermark_is_rechecked_after_stage_completion() {
        let (tx, mut rx) = watch::channel(None);
        let epoch = SessionEpoch(41);
        assert!(!reliable_frame_cancelled(&mut rx, epoch));

        tx.send_replace(Some(epoch));

        assert!(
            reliable_frame_cancelled(&mut rx, epoch),
            "a stage completion must not hide a concurrently published cumulative cancellation"
        );
    }

    #[tokio::test]
    async fn closed_reliable_channel_retires_enter_and_emits_terminal_release() {
        let (handle, probe, mut releases) = fixture_handle(1);
        drop(probe.reliable_rx);

        assert_eq!(
            handle.try_send_reliable_input(reliable_enter(7, 1)),
            Err(TransportSendError::LaneClosed)
        );
        assert!(handle.is_tombstoned(SessionEpoch(7)));
        let release = releases.recv().await.unwrap();
        assert_eq!(release.epoch, SessionEpoch(7));
        assert_eq!(release.reason, ReleaseAllReason::BackendFailure);
    }

    #[tokio::test]
    async fn terminal_release_emitter_has_one_worker_and_bounded_coalesced_backlog() {
        let auth = fixture_auth(DeviceId::new_v4(), ControlConnectionId::new());
        let (tx, mut rx) = mpsc::channel(8);
        for epoch in 10..18 {
            tx.send(TerminalReleaseEvent {
                auth: auth.clone(),
                epoch: SessionEpoch(epoch),
                reason: ReleaseAllReason::SessionEnded,
            })
            .await
            .unwrap();
        }
        let emitter = TerminalReleaseEmitter::new(tx);
        emitter.emit(TerminalReleaseEvent {
            auth: auth.clone(),
            epoch: SessionEpoch(1),
            reason: ReleaseAllReason::BackendFailure,
        });
        timeout(Duration::from_secs(1), async {
            while emitter.in_flight.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the first coalesced release must become the single in-flight item");
        for epoch in 2..=1_000 {
            emitter.emit(TerminalReleaseEvent {
                auth: auth.clone(),
                epoch: SessionEpoch(epoch),
                reason: ReleaseAllReason::BackendFailure,
            });
        }
        tokio::task::yield_now().await;
        assert_eq!(emitter.worker_count_for_test(), 1);
        assert!(emitter.backlog_for_test() <= 2);
        for epoch in 10..18 {
            assert_eq!(rx.recv().await.unwrap().epoch, SessionEpoch(epoch));
        }
        assert_eq!(rx.recv().await.unwrap().epoch, SessionEpoch(1));
        assert_eq!(rx.recv().await.unwrap().epoch, SessionEpoch(1_000));
        let workers = emitter.workers.clone();
        drop(emitter);
        timeout(Duration::from_secs(1), async {
            while workers.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("dropping the emitter must stop its idle worker");
    }

    #[tokio::test]
    async fn dropping_emitter_cancels_worker_blocked_on_full_target() {
        let auth = fixture_auth(DeviceId::new_v4(), ControlConnectionId::new());
        let (tx, _rx) = mpsc::channel(1);
        tx.send(TerminalReleaseEvent {
            auth: auth.clone(),
            epoch: SessionEpoch(1),
            reason: ReleaseAllReason::SessionEnded,
        })
        .await
        .unwrap();
        let emitter = TerminalReleaseEmitter::new(tx);
        emitter.emit(TerminalReleaseEvent {
            auth,
            epoch: SessionEpoch(2),
            reason: ReleaseAllReason::BackendFailure,
        });
        timeout(Duration::from_secs(1), async {
            while emitter.in_flight.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the worker must be blocked on the full target");

        let workers = emitter.workers.clone();
        drop(emitter);
        timeout(Duration::from_secs(1), async {
            while workers.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("closing the watch sender must cancel the blocked worker");
    }
}
