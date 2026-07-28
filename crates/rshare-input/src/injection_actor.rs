//! Ordered, fail-safe remote input injection on one dedicated OS thread.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rshare_core::perf::ReceiverStageStamps;
use rshare_core::{
    AcceptRealtime, AcceptReliable, AuthenticatedInputOwner, ButtonState as WireButtonState,
    ClockDomainId, GamepadButtonState, GamepadState, InputOwnershipGate, KeyState, MonotonicStamp,
    MouseButton as WireMouseButton, PressedStateLedger, RealtimeInputFrame, RealtimeInputPayload,
    ReleaseAllReason, ReliableInputEvent, ReliableInputFrame, SessionEpoch, INPUT_PROTOCOL_VERSION,
};
use thiserror::Error;

use crate::{ButtonState, InjectBackend, InputEvent, KeyCode, MouseButton};

static NEXT_CLOCK_DOMAIN: AtomicU64 = AtomicU64::new(1);
const CONTROL_QUEUE_CAPACITY: usize = 4;
const REALTIME_SEGMENT_CAPACITY: usize = 8;
const RELATIVE_COMPONENT_CAPACITY: usize = 64;

/// Construction settings for the injection actor.
#[derive(Debug, Clone)]
pub struct InjectionActorConfig {
    /// Maximum number of loss-intolerant events held for the active session.
    pub reliable_capacity: usize,
    /// Name assigned to the backend-owning OS thread.
    pub thread_name: String,
    /// Optional explicit coalescing window. Production defaults to zero.
    pub realtime_coalesce_window: Duration,
}

impl Default for InjectionActorConfig {
    fn default() -> Self {
        Self {
            reliable_capacity: 32,
            thread_name: "rshare-input-injection".to_owned(),
            realtime_coalesce_window: Duration::ZERO,
        }
    }
}

/// A nonblocking realtime-lane admission result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeSubmitResult {
    Accepted,
    Replaced,
    Accumulated,
    OverflowDropped,
    CapacityDropped,
    OutOfOrder,
    WrongOwnerOrEpoch,
    EpochClosed,
    ProtocolViolation,
    ShuttingDown,
}

/// Queue admission failure. The release/fault path is reserved and does not
/// share the ordinary reliable capacity.
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum InjectionQueueFull {
    #[error("the reliable injection queue is full")]
    QueueFull,
    #[error("the frame does not belong to the current authenticated owner and epoch")]
    WrongOwnerOrEpoch,
    #[error("the session epoch is already terminal")]
    EpochClosed,
    #[error("the input protocol or sequence is invalid")]
    ProtocolViolation,
    #[error("the injection actor is shutting down")]
    ShuttingDown,
}

#[derive(Debug, Error)]
pub enum InjectionStartError {
    #[error("the reliable capacity must be nonzero")]
    ZeroReliableCapacity,
    #[error("failed to spawn the injection actor: {0}")]
    Spawn(#[source] std::io::Error),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum InjectionShutdownError {
    #[error("the injection actor was already shut down")]
    AlreadyShutdown,
    #[error("the injection actor thread panicked")]
    WorkerPanicked,
}

/// Most recent receiver-side injection timing sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InjectionTimingSample {
    pub epoch: SessionEpoch,
    pub sequence: u64,
    pub reliable: bool,
    pub stamps: ReceiverStageStamps,
}

/// Cheap cloneable producer handle; all backend calls happen on the actor
/// thread and none of these submissions waits for backend I/O.
#[derive(Clone)]
pub struct InputInjectionHandle {
    queue: Arc<InjectionQueue>,
    worker: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl InputInjectionHandle {
    pub fn spawn(
        backend: Box<dyn InjectBackend>,
        config: InjectionActorConfig,
    ) -> Result<Self, InjectionStartError> {
        if config.reliable_capacity == 0 {
            return Err(InjectionStartError::ZeroReliableCapacity);
        }

        let clock = LocalMonotonicClock::new();
        let queue = Arc::new(InjectionQueue {
            state: Mutex::new(QueueState::new(config.reliable_capacity)),
            changed: Condvar::new(),
            latest_timing: Mutex::new(None),
            clock,
            realtime_coalesce_window: config.realtime_coalesce_window,
        });
        let worker_queue = Arc::clone(&queue);
        let worker = thread::Builder::new()
            .name(config.thread_name)
            .spawn(move || run_worker(worker_queue, backend))
            .map_err(InjectionStartError::Spawn)?;

        Ok(Self {
            queue,
            worker: Arc::new(Mutex::new(Some(worker))),
        })
    }

    pub fn begin_session(
        &self,
        owner: AuthenticatedInputOwner,
        epoch: SessionEpoch,
        lease_duration: Duration,
    ) -> Result<(), InjectionQueueFull> {
        let mut state = self.queue.state.lock().unwrap();
        if state.shutdown {
            return Err(InjectionQueueFull::ShuttingDown);
        }
        let required_control_slots =
            1 + usize::from(state.active.is_some_and(|active| !active.closed));
        if state.controls.len() + required_control_slots > CONTROL_QUEUE_CAPACITY {
            return Err(InjectionQueueFull::QueueFull);
        }
        if let Some(active) = state.active {
            if epoch.0 <= active.key.epoch.0 {
                return Err(InjectionQueueFull::ProtocolViolation);
            }
            if !active.closed {
                state.controls.push_back(ControlCommand::Close {
                    key: active.key,
                    reason: ReleaseAllReason::OwnershipTransfer,
                });
            }
            state.clear_ordinary();
        }

        let key = SessionKey { owner, epoch };
        state.active = Some(AdmissionSession {
            key,
            closed: false,
            last_realtime_sequence: None,
            last_reliable_sequence: None,
            lease_duration,
            deadline: Instant::now() + lease_duration,
        });
        state.controls.push_back(ControlCommand::Begin { key });
        self.queue.changed.notify_one();
        Ok(())
    }

    pub fn submit_realtime(
        &self,
        from: AuthenticatedInputOwner,
        frame: RealtimeInputFrame,
    ) -> RealtimeSubmitResult {
        let mut state = self.queue.state.lock().unwrap();
        if state.shutdown {
            return RealtimeSubmitResult::ShuttingDown;
        }
        let Some(mut active) = state.active else {
            return RealtimeSubmitResult::WrongOwnerOrEpoch;
        };
        let key = SessionKey {
            owner: from,
            epoch: frame.session_epoch,
        };
        if active.key != key {
            return RealtimeSubmitResult::WrongOwnerOrEpoch;
        }
        if active.closed {
            return RealtimeSubmitResult::EpochClosed;
        }
        if frame.protocol_version != INPUT_PROTOCOL_VERSION {
            close_admission(&mut state, key, ReleaseAllReason::BackendFailure);
            self.queue.changed.notify_one();
            return RealtimeSubmitResult::ProtocolViolation;
        }
        if active
            .last_realtime_sequence
            .is_some_and(|last| frame.sequence <= last)
        {
            return RealtimeSubmitResult::OutOfOrder;
        }
        active.last_realtime_sequence = Some(frame.sequence);
        active.deadline = Instant::now() + active.lease_duration;
        state.active = Some(active);
        let queued = QueuedRealtime::new(frame, self.queue.clock.now(), Instant::now());
        let outcome = match state.ordinary.back_mut() {
            Some(QueuedInput::Realtime(previous)) => merge_adjacent_realtime(previous, queued),
            _ => RealtimeMerge::Incompatible(queued),
        };
        let result = match outcome {
            RealtimeMerge::Replaced => RealtimeSubmitResult::Replaced,
            RealtimeMerge::Accumulated => RealtimeSubmitResult::Accumulated,
            RealtimeMerge::OverflowDropped => RealtimeSubmitResult::OverflowDropped,
            RealtimeMerge::CapacityDropped => RealtimeSubmitResult::CapacityDropped,
            RealtimeMerge::Incompatible(queued) => {
                let segment_len = state
                    .ordinary
                    .iter()
                    .rev()
                    .take_while(|item| matches!(item, QueuedInput::Realtime(_)))
                    .count();
                if segment_len >= REALTIME_SEGMENT_CAPACITY {
                    RealtimeSubmitResult::CapacityDropped
                } else {
                    state.ordinary.push_back(QueuedInput::Realtime(queued));
                    RealtimeSubmitResult::Accepted
                }
            }
        };
        self.queue.changed.notify_one();
        result
    }

    pub fn try_submit_reliable(
        &self,
        from: AuthenticatedInputOwner,
        frame: ReliableInputFrame,
    ) -> Result<(), InjectionQueueFull> {
        let mut state = self.queue.state.lock().unwrap();
        if state.shutdown {
            return Err(InjectionQueueFull::ShuttingDown);
        }
        let Some(mut active) = state.active else {
            return Err(InjectionQueueFull::WrongOwnerOrEpoch);
        };
        let key = SessionKey {
            owner: from,
            epoch: frame.session_epoch,
        };
        if active.key != key {
            return Err(InjectionQueueFull::WrongOwnerOrEpoch);
        }
        if active.closed {
            return Err(InjectionQueueFull::EpochClosed);
        }
        if frame.protocol_version != INPUT_PROTOCOL_VERSION {
            close_admission(&mut state, key, ReleaseAllReason::BackendFailure);
            self.queue.changed.notify_one();
            return Err(InjectionQueueFull::ProtocolViolation);
        }

        if let ReliableInputEvent::ReleaseAll { reason } = frame.event {
            close_admission(&mut state, key, reason);
            self.queue.changed.notify_one();
            return Ok(());
        }

        if let Some(previous) = active.last_reliable_sequence {
            if frame.sequence != previous.saturating_add(1) {
                close_admission(&mut state, key, ReleaseAllReason::BackendFailure);
                self.queue.changed.notify_one();
                return Ok(());
            }
        }
        if state.reliable_len >= state.reliable_capacity {
            return Err(InjectionQueueFull::QueueFull);
        }
        active.last_reliable_sequence = Some(frame.sequence);
        active.deadline = Instant::now() + active.lease_duration;
        state.active = Some(active);
        state
            .ordinary
            .push_back(QueuedInput::Reliable(QueuedReliable {
                frame,
                received_at: self.queue.clock.now(),
            }));
        state.reliable_len += 1;
        self.queue.changed.notify_one();
        Ok(())
    }

    /// Close the active epoch locally. This is also the connection-loss entry
    /// point; it does not require the network to remain writable.
    pub fn request_release_all(&self, reason: ReleaseAllReason) {
        let mut state = self.queue.state.lock().unwrap();
        if let Some(active) = state.active {
            close_admission(&mut state, active.key, reason);
            self.queue.changed.notify_one();
        }
    }

    pub fn latest_timing(&self) -> Option<InjectionTimingSample> {
        *self.queue.latest_timing.lock().unwrap()
    }

    pub fn shutdown(&self) -> Result<(), InjectionShutdownError> {
        let worker = {
            let mut worker = self.worker.lock().unwrap();
            worker
                .take()
                .ok_or(InjectionShutdownError::AlreadyShutdown)?
        };
        {
            let mut state = self.queue.state.lock().unwrap();
            state.shutdown = true;
            state.clear_ordinary();
            self.queue.changed.notify_one();
        }
        worker
            .join()
            .map_err(|_| InjectionShutdownError::WorkerPanicked)
    }
}

impl Drop for InputInjectionHandle {
    fn drop(&mut self) {
        if Arc::strong_count(&self.worker) != 1 {
            return;
        }
        let worker = self.worker.lock().unwrap().take();
        if let Some(worker) = worker {
            {
                let mut state = self.queue.state.lock().unwrap();
                state.shutdown = true;
                state.clear_ordinary();
                self.queue.changed.notify_one();
            }
            // Dropping a JoinHandle detaches it. Explicit `shutdown` remains
            // available when a caller needs a synchronous join.
            drop(worker);
        }
    }
}

struct InjectionQueue {
    state: Mutex<QueueState>,
    changed: Condvar,
    latest_timing: Mutex<Option<InjectionTimingSample>>,
    clock: LocalMonotonicClock,
    realtime_coalesce_window: Duration,
}

struct QueueState {
    active: Option<AdmissionSession>,
    controls: VecDeque<ControlCommand>,
    ordinary: VecDeque<QueuedInput>,
    reliable_len: usize,
    reliable_capacity: usize,
    shutdown: bool,
}

impl QueueState {
    fn new(reliable_capacity: usize) -> Self {
        Self {
            active: None,
            controls: VecDeque::new(),
            ordinary: VecDeque::with_capacity(
                reliable_capacity.saturating_mul(2).saturating_add(1),
            ),
            reliable_len: 0,
            reliable_capacity,
            shutdown: false,
        }
    }

    fn clear_ordinary(&mut self) {
        self.ordinary.clear();
        self.reliable_len = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionKey {
    owner: AuthenticatedInputOwner,
    epoch: SessionEpoch,
}

#[derive(Debug, Clone, Copy)]
struct AdmissionSession {
    key: SessionKey,
    closed: bool,
    last_realtime_sequence: Option<u64>,
    last_reliable_sequence: Option<u64>,
    lease_duration: Duration,
    deadline: Instant,
}

enum ControlCommand {
    Begin {
        key: SessionKey,
    },
    Close {
        key: SessionKey,
        reason: ReleaseAllReason,
    },
}

struct QueuedRealtime {
    frame: RealtimeInputFrame,
    received_at: MonotonicStamp,
    submitted_at: Instant,
    relative_components: Vec<RelativeComponent>,
}

#[derive(Clone)]
struct RelativeComponent {
    sequence: u64,
    captured_at: MonotonicStamp,
    dx: i32,
    dy: i32,
    received_at: MonotonicStamp,
    submitted_at: Instant,
}

impl QueuedRealtime {
    fn new(frame: RealtimeInputFrame, received_at: MonotonicStamp, submitted_at: Instant) -> Self {
        let relative_components = match frame.payload {
            RealtimeInputPayload::RelativeMouse { dx, dy } => vec![RelativeComponent {
                sequence: frame.sequence,
                captured_at: frame.captured_at,
                dx,
                dy,
                received_at,
                submitted_at,
            }],
            _ => Vec::new(),
        };
        Self {
            frame,
            received_at,
            submitted_at,
            relative_components,
        }
    }

    fn from_relative_components(
        protocol_version: u16,
        session_epoch: SessionEpoch,
        components: Vec<RelativeComponent>,
    ) -> Self {
        let first = components
            .first()
            .expect("relative component group must not be empty");
        let last = components
            .last()
            .expect("relative component group must not be empty");
        let mut dx = 0_i32;
        let mut dy = 0_i32;
        for component in &components {
            dx = dx
                .checked_add(component.dx)
                .expect("previously admitted relative x components must remain representable");
            dy = dy
                .checked_add(component.dy)
                .expect("previously admitted relative y components must remain representable");
        }
        Self {
            frame: RealtimeInputFrame {
                protocol_version,
                session_epoch,
                sequence: last.sequence,
                captured_at: last.captured_at,
                payload: RealtimeInputPayload::RelativeMouse { dx, dy },
            },
            received_at: first.received_at,
            submitted_at: last.submitted_at,
            relative_components: components,
        }
    }
}

struct QueuedReliable {
    frame: ReliableInputFrame,
    received_at: MonotonicStamp,
}

enum QueuedInput {
    Realtime(QueuedRealtime),
    Reliable(QueuedReliable),
}

enum RealtimeMerge {
    Replaced,
    Accumulated,
    OverflowDropped,
    CapacityDropped,
    Incompatible(QueuedRealtime),
}

fn merge_adjacent_realtime(
    previous: &mut QueuedRealtime,
    incoming: QueuedRealtime,
) -> RealtimeMerge {
    match (&mut previous.frame.payload, &incoming.frame.payload) {
        (
            RealtimeInputPayload::RelativeMouse { dx, dy },
            RealtimeInputPayload::RelativeMouse {
                dx: incoming_dx,
                dy: incoming_dy,
            },
        ) => {
            if previous.relative_components.len() >= RELATIVE_COMPONENT_CAPACITY {
                return RealtimeMerge::CapacityDropped;
            }
            let Some(merged_dx) = dx.checked_add(*incoming_dx) else {
                return RealtimeMerge::OverflowDropped;
            };
            let Some(merged_dy) = dy.checked_add(*incoming_dy) else {
                return RealtimeMerge::OverflowDropped;
            };
            *dx = merged_dx;
            *dy = merged_dy;
            previous.frame.sequence = incoming.frame.sequence;
            previous.frame.captured_at = incoming.frame.captured_at;
            previous.submitted_at = incoming.submitted_at;
            previous
                .relative_components
                .extend(incoming.relative_components);
            RealtimeMerge::Accumulated
        }
        (
            RealtimeInputPayload::AbsoluteAnchor { .. },
            RealtimeInputPayload::AbsoluteAnchor { .. },
        )
        | (RealtimeInputPayload::CursorVisual { .. }, RealtimeInputPayload::CursorVisual { .. }) => {
            *previous = incoming;
            RealtimeMerge::Replaced
        }
        (
            RealtimeInputPayload::GamepadAxes { gamepad_id, .. },
            RealtimeInputPayload::GamepadAxes {
                gamepad_id: incoming_id,
                ..
            },
        ) if gamepad_id == incoming_id => {
            *previous = incoming;
            RealtimeMerge::Replaced
        }
        _ => RealtimeMerge::Incompatible(incoming),
    }
}

fn close_admission(state: &mut QueueState, key: SessionKey, reason: ReleaseAllReason) {
    if let Some(active) = state.active.as_mut() {
        if active.key != key || active.closed {
            return;
        }
        active.closed = true;
    } else {
        return;
    }
    state.clear_ordinary();
    state
        .controls
        .push_back(ControlCommand::Close { key, reason });
}

struct WorkerSession {
    key: SessionKey,
    gate: InputOwnershipGate,
    last_realtime_applied: Option<u64>,
}

enum WorkItem {
    Control(ControlCommand),
    Realtime(QueuedRealtime),
    Reliable(QueuedReliable),
    LeaseExpired(SessionKey),
    Shutdown,
}

fn run_worker(queue: Arc<InjectionQueue>, mut backend: Box<dyn InjectBackend>) {
    let mut session: Option<WorkerSession> = None;
    let mut ledger = PressedStateLedger::new();
    let mut gamepads: HashMap<u8, GamepadState> = HashMap::new();

    loop {
        let item = next_work_item(&queue, session.as_ref());
        match item {
            WorkItem::Shutdown => {
                release_held(&mut *backend, &mut ledger, ReleaseAllReason::SessionEnded);
                return;
            }
            WorkItem::Control(ControlCommand::Begin { key }) => {
                if session.as_ref().is_some_and(|current| current.key != key) {
                    release_held(
                        &mut *backend,
                        &mut ledger,
                        ReleaseAllReason::OwnershipTransfer,
                    );
                }
                session = Some(WorkerSession {
                    key,
                    gate: InputOwnershipGate::new(key.owner, key.epoch),
                    last_realtime_applied: None,
                });
                gamepads.clear();
            }
            WorkItem::Control(ControlCommand::Close { key, reason }) => {
                if session.as_ref().is_some_and(|current| current.key == key) {
                    release_held(&mut *backend, &mut ledger, reason);
                    session = None;
                    gamepads.clear();
                }
            }
            WorkItem::LeaseExpired(key) => {
                if session.as_ref().is_some_and(|current| current.key == key) {
                    release_held(&mut *backend, &mut ledger, ReleaseAllReason::Timeout);
                    session = None;
                    gamepads.clear();
                }
            }
            WorkItem::Realtime(queued) => {
                let Some(current) = session.as_mut() else {
                    continue;
                };
                let key = SessionKey {
                    owner: current.key.owner,
                    epoch: queued.frame.session_epoch,
                };
                if key != current.key {
                    continue;
                }
                let accepted = current.gate.accept_realtime(
                    current.key.owner,
                    queued.frame.session_epoch,
                    queued.frame.sequence,
                );
                if matches!(
                    accepted,
                    AcceptRealtime::OutOfOrder | AcceptRealtime::WrongOwnerOrEpoch
                ) {
                    continue;
                }
                let sequence = queued.frame.sequence;
                let started = queue.clock.now();
                let result = inject_realtime(&mut *backend, &mut gamepads, queued.frame);
                let completed = queue.clock.now();
                record_timing(
                    &queue,
                    current.key.epoch,
                    sequence,
                    false,
                    queued.received_at,
                    started,
                    completed,
                );
                if result.is_err() {
                    fail_worker_session(&queue, current.key, &mut *backend, &mut ledger);
                    session = None;
                    gamepads.clear();
                } else {
                    current.last_realtime_applied = Some(sequence);
                }
            }
            WorkItem::Reliable(queued) => {
                let Some(current) = session.as_mut() else {
                    continue;
                };
                let accepted = current.gate.accept_reliable(
                    current.key.owner,
                    queued.frame.session_epoch,
                    queued.frame.sequence,
                );
                if !matches!(accepted, AcceptReliable::Accepted) {
                    fail_worker_session(&queue, current.key, &mut *backend, &mut ledger);
                    session = None;
                    gamepads.clear();
                    continue;
                }
                let sequence = queued.frame.sequence;
                let started = queue.clock.now();
                let result = inject_reliable(
                    &mut *backend,
                    &mut ledger,
                    &mut gamepads,
                    current,
                    queued.frame.event,
                );
                let completed = queue.clock.now();
                record_timing(
                    &queue,
                    current.key.epoch,
                    sequence,
                    true,
                    queued.received_at,
                    started,
                    completed,
                );
                if result.is_err() {
                    fail_worker_session(&queue, current.key, &mut *backend, &mut ledger);
                    session = None;
                    gamepads.clear();
                }
            }
        }
    }
}

fn next_work_item(queue: &InjectionQueue, session: Option<&WorkerSession>) -> WorkItem {
    let mut state = queue.state.lock().unwrap();
    loop {
        if state.shutdown {
            return WorkItem::Shutdown;
        }
        if let Some(control) = state.controls.pop_front() {
            return WorkItem::Control(control);
        }
        if let Some(current) = session {
            let admission_deadline = state
                .active
                .filter(|active| active.key == current.key && !active.closed)
                .map(|active| active.deadline);
            if admission_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                close_admission(&mut state, current.key, ReleaseAllReason::Timeout);
                // Consume the close we just reserved and surface a typed timeout.
                if matches!(
                    state.controls.back(),
                    Some(ControlCommand::Close { key, .. }) if *key == current.key
                ) {
                    state.controls.pop_back();
                }
                return WorkItem::LeaseExpired(current.key);
            }
        }

        if let Some(prefix) = split_front_relative_for_late_anchor(&mut state) {
            return WorkItem::Realtime(prefix);
        }

        if let Some(front) = state.ordinary.front() {
            if let QueuedInput::Realtime(realtime) = front {
                let anchored_reliable_index = match state.ordinary.get(1) {
                    Some(QueuedInput::Reliable(QueuedReliable {
                        frame:
                            ReliableInputFrame {
                                event:
                                    ReliableInputEvent::MouseButton {
                                        realtime_anchor_sequence,
                                        ..
                                    },
                                ..
                            },
                        ..
                    })) if realtime.frame.sequence > *realtime_anchor_sequence => Some(1),
                    _ => None,
                };
                if let Some(index) = anchored_reliable_index {
                    if let QueuedInput::Reliable(reliable) = state
                        .ordinary
                        .remove(index)
                        .expect("indexed item must exist")
                    {
                        state.reliable_len -= 1;
                        return WorkItem::Reliable(reliable);
                    }
                    unreachable!();
                }

                let age = realtime.submitted_at.elapsed();
                if state.ordinary.len() == 1
                    && age < queue.realtime_coalesce_window
                    && !queue.realtime_coalesce_window.is_zero()
                {
                    let wait = queue.realtime_coalesce_window - age;
                    let (next, _) = queue.changed.wait_timeout(state, wait).unwrap();
                    state = next;
                    continue;
                }
            }

            match state.ordinary.pop_front().expect("front item must exist") {
                QueuedInput::Realtime(realtime) => return WorkItem::Realtime(realtime),
                QueuedInput::Reliable(reliable) => {
                    state.reliable_len -= 1;
                    return WorkItem::Reliable(reliable);
                }
            }
        }

        if let Some(current) = session {
            if let Some(deadline) = state
                .active
                .filter(|active| active.key == current.key && !active.closed)
                .map(|active| active.deadline)
            {
                let wait = deadline.saturating_duration_since(Instant::now());
                let (next, _) = queue.changed.wait_timeout(state, wait).unwrap();
                state = next;
            } else {
                state = queue.changed.wait(state).unwrap();
            }
        } else {
            state = queue.changed.wait(state).unwrap();
        }
    }
}

fn split_front_relative_for_late_anchor(state: &mut QueueState) -> Option<QueuedRealtime> {
    let anchor = match state.ordinary.get(1) {
        Some(QueuedInput::Reliable(QueuedReliable {
            frame:
                ReliableInputFrame {
                    event:
                        ReliableInputEvent::MouseButton {
                            realtime_anchor_sequence,
                            ..
                        },
                    ..
                },
            ..
        })) => *realtime_anchor_sequence,
        _ => return None,
    };
    let split_index = match state.ordinary.front() {
        Some(QueuedInput::Realtime(realtime)) => realtime
            .relative_components
            .iter()
            .position(|component| component.sequence > anchor)?,
        _ => return None,
    };
    if split_index == 0 {
        return None;
    }

    let original = match state.ordinary.pop_front().expect("front must exist") {
        QueuedInput::Realtime(realtime) => realtime,
        QueuedInput::Reliable(_) => unreachable!(),
    };
    let reliable = state
        .ordinary
        .pop_front()
        .expect("anchored reliable must follow realtime");
    let protocol_version = original.frame.protocol_version;
    let session_epoch = original.frame.session_epoch;
    let mut components = original.relative_components;
    let suffix = components.split_off(split_index);
    let prefix =
        QueuedRealtime::from_relative_components(protocol_version, session_epoch, components);

    for group in group_representable_relative_components(suffix)
        .into_iter()
        .rev()
    {
        state.ordinary.push_front(QueuedInput::Realtime(
            QueuedRealtime::from_relative_components(protocol_version, session_epoch, group),
        ));
    }
    state.ordinary.push_front(reliable);
    Some(prefix)
}

fn group_representable_relative_components(
    components: Vec<RelativeComponent>,
) -> Vec<Vec<RelativeComponent>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();
    let mut current_dx = 0_i32;
    let mut current_dy = 0_i32;

    for component in components {
        let merged = current_dx
            .checked_add(component.dx)
            .zip(current_dy.checked_add(component.dy));
        if !current.is_empty() && merged.is_none() {
            groups.push(std::mem::take(&mut current));
            current_dx = 0;
            current_dy = 0;
        }
        current_dx = current_dx
            .checked_add(component.dx)
            .expect("one relative component always fits in i32");
        current_dy = current_dy
            .checked_add(component.dy)
            .expect("one relative component always fits in i32");
        current.push(component);
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

fn inject_realtime(
    backend: &mut dyn InjectBackend,
    gamepads: &mut HashMap<u8, GamepadState>,
    frame: RealtimeInputFrame,
) -> anyhow::Result<()> {
    match frame.payload {
        RealtimeInputPayload::RelativeMouse { dx, dy } => backend.inject_relative_pointer(dx, dy),
        RealtimeInputPayload::AbsoluteAnchor { x, y } => {
            backend.inject(InputEvent::MouseMove { x, y })
        }
        RealtimeInputPayload::GamepadAxes {
            gamepad_id,
            left_stick_x,
            left_stick_y,
            right_stick_x,
            right_stick_y,
            left_trigger,
            right_trigger,
        } => {
            let state = gamepads
                .entry(gamepad_id)
                .or_insert_with(|| GamepadState::neutral(gamepad_id, frame.sequence, 0));
            state.sequence = frame.sequence;
            state.left_stick_x = left_stick_x;
            state.left_stick_y = left_stick_y;
            state.right_stick_x = right_stick_x;
            state.right_stick_y = right_stick_y;
            state.left_trigger = left_trigger;
            state.right_trigger = right_trigger;
            backend.inject(InputEvent::GamepadState {
                state: state.clone(),
            })
        }
        RealtimeInputPayload::CursorVisual { .. } => Ok(()),
    }
}

fn inject_reliable(
    backend: &mut dyn InjectBackend,
    ledger: &mut PressedStateLedger,
    gamepads: &mut HashMap<u8, GamepadState>,
    session: &mut WorkerSession,
    event: ReliableInputEvent,
) -> anyhow::Result<()> {
    match event {
        ReliableInputEvent::Enter { x, y, .. } => backend.inject(InputEvent::MouseMove { x, y }),
        ReliableInputEvent::Leave => Ok(()),
        ReliableInputEvent::ReleaseAll { reason } => {
            release_held(backend, ledger, reason);
            Ok(())
        }
        ReliableInputEvent::Key { keycode, state } => {
            let inject_state = map_key_state(state);
            backend.inject(InputEvent::Key {
                keycode: KeyCode::Raw(keycode),
                state: inject_state,
            })?;
            ledger.record_key(keycode, state)?;
            Ok(())
        }
        ReliableInputEvent::TextCommit { text } => backend.inject(InputEvent::TextCommit { text }),
        ReliableInputEvent::MouseButton {
            button,
            state,
            x,
            y,
            realtime_anchor_sequence,
        } => {
            if session.last_realtime_applied != Some(realtime_anchor_sequence) {
                backend.inject(InputEvent::MouseMove { x, y })?;
            }
            let inject_button = map_mouse_button(button);
            let inject_state = map_button_state(state);
            backend.inject(InputEvent::MouseButton {
                button: inject_button,
                state: inject_state,
            })?;
            ledger.record_mouse_button(button, state, x, y, realtime_anchor_sequence)?;
            Ok(())
        }
        ReliableInputEvent::Wheel { delta_x, delta_y } => {
            backend.inject(InputEvent::MouseWheel { delta_x, delta_y })
        }
        ReliableInputEvent::GamepadConnected { info } => {
            gamepads.insert(
                info.gamepad_id,
                GamepadState::neutral(info.gamepad_id, 0, 0),
            );
            backend.inject(InputEvent::GamepadConnected { info })
        }
        ReliableInputEvent::GamepadDisconnected { gamepad_id } => {
            gamepads.remove(&gamepad_id);
            backend.inject(InputEvent::GamepadDisconnected { gamepad_id })
        }
        ReliableInputEvent::GamepadButton {
            gamepad_id,
            button,
            pressed,
        } => {
            let state = gamepads
                .entry(gamepad_id)
                .or_insert_with(|| GamepadState::neutral(gamepad_id, 0, 0));
            state.buttons.retain(|entry| entry.button != button);
            state.buttons.push(GamepadButtonState { button, pressed });
            backend.inject(InputEvent::GamepadButton {
                gamepad_id,
                button,
                pressed,
                state_after: state.clone(),
            })
        }
    }
}

fn release_held(
    backend: &mut dyn InjectBackend,
    ledger: &mut PressedStateLedger,
    reason: ReleaseAllReason,
) {
    let Ok(batch) = ledger.release_all_events(reason) else {
        return;
    };
    let mut complete = true;
    for event in batch.events() {
        let mapped = match event {
            ReliableInputEvent::Key { keycode, state } => InputEvent::Key {
                keycode: KeyCode::Raw(*keycode),
                state: map_key_state(*state),
            },
            ReliableInputEvent::MouseButton { button, state, .. } => InputEvent::MouseButton {
                button: map_mouse_button(*button),
                state: map_button_state(*state),
            },
            _ => continue,
        };
        if backend.inject(mapped).is_err() {
            complete = false;
        }
    }
    if complete {
        ledger.confirm_release_all(&batch);
    }
}

fn fail_worker_session(
    queue: &InjectionQueue,
    key: SessionKey,
    backend: &mut dyn InjectBackend,
    ledger: &mut PressedStateLedger,
) {
    {
        let mut state = queue.state.lock().unwrap();
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.key == key && !active.closed)
        {
            if let Some(active) = state.active.as_mut() {
                active.closed = true;
            }
            state.clear_ordinary();
        }
    }
    release_held(backend, ledger, ReleaseAllReason::BackendFailure);
}

fn record_timing(
    queue: &InjectionQueue,
    epoch: SessionEpoch,
    sequence: u64,
    reliable: bool,
    received: MonotonicStamp,
    injection_started: MonotonicStamp,
    injection_completed: MonotonicStamp,
) {
    *queue.latest_timing.lock().unwrap() = Some(InjectionTimingSample {
        epoch,
        sequence,
        reliable,
        stamps: ReceiverStageStamps {
            received,
            injection_started: Some(injection_started),
            injection_completed: Some(injection_completed),
        },
    });
}

fn map_key_state(state: KeyState) -> ButtonState {
    match state {
        KeyState::Pressed => ButtonState::Pressed,
        KeyState::Released => ButtonState::Released,
    }
}

fn map_button_state(state: WireButtonState) -> ButtonState {
    match state {
        WireButtonState::Pressed => ButtonState::Pressed,
        WireButtonState::Released => ButtonState::Released,
    }
}

fn map_mouse_button(button: WireMouseButton) -> MouseButton {
    match button {
        WireMouseButton::Left => MouseButton::Left,
        WireMouseButton::Middle => MouseButton::Middle,
        WireMouseButton::Right => MouseButton::Right,
        WireMouseButton::Back => MouseButton::Back,
        WireMouseButton::Forward => MouseButton::Forward,
        WireMouseButton::Other(value) => MouseButton::Other(value),
    }
}

#[derive(Clone, Copy)]
struct LocalMonotonicClock {
    origin: Instant,
    domain: ClockDomainId,
}

impl LocalMonotonicClock {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            domain: ClockDomainId(NEXT_CLOCK_DOMAIN.fetch_add(1, Ordering::Relaxed)),
        }
    }

    fn now(self) -> MonotonicStamp {
        let micros = self.origin.elapsed().as_micros().min(u64::MAX as u128) as u64;
        MonotonicStamp::new(self.domain, micros)
    }
}
