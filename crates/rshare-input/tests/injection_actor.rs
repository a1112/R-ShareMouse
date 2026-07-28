use std::fmt;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use rshare_core::{
    AuthenticatedInputOwner, BackendHealth, BackendKind, ButtonState as WireButtonState,
    ClockDomainId, ControlConnectionId, DeviceId, KeyState, MonotonicStamp, MouseButton,
    RealtimeInputFrame, RealtimeInputPayload, ReleaseAllReason, ReliableInputEvent,
    ReliableInputFrame, SessionEpoch, INPUT_PROTOCOL_VERSION,
};
use rshare_input::{
    ButtonState, InjectBackend, InjectionActorConfig, InputEvent, InputInjectionHandle,
    MouseButton as InjectMouseButton, RealtimeSubmitResult,
};

const WAIT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Relative(i32, i32, String),
    Absolute(i32, i32, String),
    Button(InjectMouseButton, ButtonState, String),
    Key(u32, ButtonState, String),
}

#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<Call>>,
    changed: Condvar,
}

impl Recorder {
    fn push(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
        self.changed.notify_all();
    }

    fn wait_for(&self, count: usize) -> Vec<Call> {
        let deadline = Instant::now() + WAIT;
        let mut calls = self.calls.lock().unwrap();
        while calls.len() < count {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "timed out waiting for {count} calls");
            let (next, timeout) = self.changed.wait_timeout(calls, remaining).unwrap();
            calls = next;
            assert!(!timeout.timed_out() || calls.len() >= count);
        }
        calls.clone()
    }
}

struct RecordingBackend {
    recorder: Arc<Recorder>,
    delay: Duration,
    fail_relative: bool,
}

struct DropSignalingBackend {
    inner: RecordingBackend,
    dropped: mpsc::SyncSender<()>,
}

impl fmt::Debug for DropSignalingBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DropSignalingBackend")
            .finish_non_exhaustive()
    }
}

impl Drop for DropSignalingBackend {
    fn drop(&mut self) {
        let _ = self.dropped.send(());
    }
}

impl InjectBackend for DropSignalingBackend {
    fn kind(&self) -> BackendKind {
        self.inner.kind()
    }

    fn health(&self) -> BackendHealth {
        self.inner.health()
    }

    fn inject(&mut self, event: InputEvent) -> Result<()> {
        self.inner.inject(event)
    }

    fn inject_relative_pointer(&mut self, dx: i32, dy: i32) -> Result<()> {
        self.inner.inject_relative_pointer(dx, dy)
    }

    fn is_active(&self) -> bool {
        true
    }
}

impl fmt::Debug for RecordingBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordingBackend").finish_non_exhaustive()
    }
}

impl InjectBackend for RecordingBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Portable
    }

    fn health(&self) -> BackendHealth {
        BackendHealth::Healthy
    }

    fn inject(&mut self, event: InputEvent) -> Result<()> {
        if !self.delay.is_zero() {
            thread::sleep(self.delay);
        }
        let thread = thread::current().name().unwrap_or("<unnamed>").to_owned();
        let call = match event {
            InputEvent::MouseMove { x, y } => Call::Absolute(x, y, thread),
            InputEvent::MouseButton { button, state } => Call::Button(button, state, thread),
            InputEvent::Key { keycode, state } => Call::Key(keycode.to_raw(), state, thread),
            other => panic!("unexpected injected event: {other:?}"),
        };
        self.recorder.push(call);
        Ok(())
    }

    fn inject_relative_pointer(&mut self, dx: i32, dy: i32) -> Result<()> {
        if self.fail_relative {
            anyhow::bail!("synthetic relative injection failure");
        }
        if !self.delay.is_zero() {
            thread::sleep(self.delay);
        }
        self.recorder.push(Call::Relative(
            dx,
            dy,
            thread::current().name().unwrap_or("<unnamed>").to_owned(),
        ));
        Ok(())
    }

    fn is_active(&self) -> bool {
        true
    }
}

fn owner() -> AuthenticatedInputOwner {
    AuthenticatedInputOwner {
        peer_id: DeviceId::new_v4(),
        control_connection_id: ControlConnectionId::new(),
    }
}

fn stamp() -> MonotonicStamp {
    MonotonicStamp::new(ClockDomainId(7), 1)
}

fn realtime(epoch: u64, sequence: u64, dx: i32, dy: i32) -> RealtimeInputFrame {
    RealtimeInputFrame {
        protocol_version: INPUT_PROTOCOL_VERSION,
        session_epoch: SessionEpoch(epoch),
        sequence,
        captured_at: stamp(),
        payload: RealtimeInputPayload::RelativeMouse { dx, dy },
    }
}

fn reliable(epoch: u64, sequence: u64, event: ReliableInputEvent) -> ReliableInputFrame {
    ReliableInputFrame {
        protocol_version: INPUT_PROTOCOL_VERSION,
        session_epoch: SessionEpoch(epoch),
        sequence,
        captured_at: stamp(),
        event,
    }
}

fn button(epoch: u64, sequence: u64, anchor: u64, x: i32, y: i32) -> ReliableInputFrame {
    reliable(
        epoch,
        sequence,
        ReliableInputEvent::MouseButton {
            button: MouseButton::Left,
            state: WireButtonState::Pressed,
            x,
            y,
            realtime_anchor_sequence: anchor,
        },
    )
}

fn fixture(
    backend: RecordingBackend,
    realtime_coalesce_window: Duration,
) -> (InputInjectionHandle, Arc<Recorder>, AuthenticatedInputOwner) {
    let recorder = Arc::clone(&backend.recorder);
    let actor = InputInjectionHandle::spawn(
        Box::new(backend),
        InjectionActorConfig {
            reliable_capacity: 8,
            thread_name: "rshare-input-injection-test".into(),
            realtime_coalesce_window,
        },
    )
    .unwrap();
    let owner = owner();
    actor
        .begin_session(owner, SessionEpoch(1), Duration::from_secs(30))
        .unwrap();
    (actor, recorder, owner)
}

#[test]
fn only_current_authenticated_owner_epoch_and_connection_inject() {
    let recorder = Arc::new(Recorder::default());
    let backend = RecordingBackend {
        recorder: Arc::clone(&recorder),
        delay: Duration::ZERO,
        fail_relative: false,
    };
    let (actor, recorder, current) = fixture(backend, Duration::ZERO);
    let wrong_peer = AuthenticatedInputOwner {
        peer_id: DeviceId::new_v4(),
        control_connection_id: current.control_connection_id,
    };
    let wrong_connection = AuthenticatedInputOwner {
        peer_id: current.peer_id,
        control_connection_id: ControlConnectionId::new(),
    };

    assert_eq!(
        actor.submit_realtime(wrong_peer, realtime(1, 1, 1, 1)),
        RealtimeSubmitResult::WrongOwnerOrEpoch
    );
    assert_eq!(
        actor.submit_realtime(wrong_connection, realtime(1, 1, 1, 1)),
        RealtimeSubmitResult::WrongOwnerOrEpoch
    );
    assert_eq!(
        actor.submit_realtime(current, realtime(2, 1, 1, 1)),
        RealtimeSubmitResult::WrongOwnerOrEpoch
    );
    assert!(matches!(
        actor.submit_realtime(current, realtime(1, 1, 2, 3)),
        RealtimeSubmitResult::Accepted
    ));

    assert_eq!(
        recorder.wait_for(1)[0],
        Call::Relative(2, 3, "rshare-input-injection-test".into())
    );
    actor.shutdown().unwrap();
}

#[test]
fn realtime_latest_wins_without_regressing_sequence() {
    let recorder = Arc::new(Recorder::default());
    let backend = RecordingBackend {
        recorder: Arc::clone(&recorder),
        delay: Duration::ZERO,
        fail_relative: false,
    };
    let (actor, recorder, owner) = fixture(backend, Duration::from_millis(25));

    actor.submit_realtime(owner, realtime(1, 10, 10, 0));
    assert_eq!(
        recorder.wait_for(1)[0],
        Call::Relative(10, 0, "rshare-input-injection-test".into())
    );
    actor.submit_realtime(owner, realtime(1, 12, 12, 0));
    assert_eq!(
        actor.submit_realtime(owner, realtime(1, 11, 11, 0)),
        RealtimeSubmitResult::OutOfOrder
    );

    assert_eq!(
        recorder.wait_for(2)[1],
        Call::Relative(12, 0, "rshare-input-injection-test".into())
    );
    actor.shutdown().unwrap();
}

#[test]
fn missing_motion_anchor_repairs_pointer_before_click() {
    let recorder = Arc::new(Recorder::default());
    let backend = RecordingBackend {
        recorder: Arc::clone(&recorder),
        delay: Duration::ZERO,
        fail_relative: false,
    };
    let (actor, recorder, owner) = fixture(backend, Duration::ZERO);

    actor.submit_realtime(owner, realtime(1, 10, 100, 100));
    recorder.wait_for(1);
    actor
        .try_submit_reliable(owner, button(1, 1, 11, 240, 180))
        .unwrap();

    assert_eq!(
        recorder.wait_for(3),
        vec![
            Call::Relative(100, 100, "rshare-input-injection-test".into()),
            Call::Absolute(240, 180, "rshare-input-injection-test".into()),
            Call::Button(
                InjectMouseButton::Left,
                ButtonState::Pressed,
                "rshare-input-injection-test".into()
            ),
        ]
    );
    actor.shutdown().unwrap();
}

#[test]
fn future_realtime_waits_behind_earlier_reliable_anchor() {
    let recorder = Arc::new(Recorder::default());
    let backend = RecordingBackend {
        recorder: Arc::clone(&recorder),
        delay: Duration::ZERO,
        fail_relative: false,
    };
    let (actor, recorder, owner) = fixture(backend, Duration::from_millis(25));

    actor.submit_realtime(owner, realtime(1, 12, 300, 200));
    actor
        .try_submit_reliable(owner, button(1, 1, 11, 240, 180))
        .unwrap();

    assert_eq!(
        recorder.wait_for(3),
        vec![
            Call::Absolute(240, 180, "rshare-input-injection-test".into()),
            Call::Button(
                InjectMouseButton::Left,
                ButtonState::Pressed,
                "rshare-input-injection-test".into()
            ),
            Call::Relative(300, 200, "rshare-input-injection-test".into()),
        ]
    );
    actor.shutdown().unwrap();
}

#[test]
fn realtime_coalescing_never_crosses_a_non_pointer_reliable_barrier() {
    let recorder = Arc::new(Recorder::default());
    let backend = RecordingBackend {
        recorder: Arc::clone(&recorder),
        delay: Duration::from_millis(35),
        fail_relative: false,
    };
    let (actor, recorder, owner) = fixture(backend, Duration::from_millis(25));

    actor.submit_realtime(owner, realtime(1, 10, 10, 0));
    actor
        .try_submit_reliable(
            owner,
            reliable(
                1,
                1,
                ReliableInputEvent::Key {
                    keycode: 65,
                    state: KeyState::Pressed,
                },
            ),
        )
        .unwrap();
    actor.submit_realtime(owner, realtime(1, 12, 12, 0));

    assert_eq!(
        recorder.wait_for(3),
        vec![
            Call::Relative(10, 0, "rshare-input-injection-test".into()),
            Call::Key(
                65,
                ButtonState::Pressed,
                "rshare-input-injection-test".into()
            ),
            Call::Relative(12, 0, "rshare-input-injection-test".into()),
        ]
    );
    actor.request_release_all(ReleaseAllReason::SessionEnded);
    actor.shutdown().unwrap();
}

#[test]
fn terminal_release_purges_queue_and_rejects_late_same_epoch() {
    let recorder = Arc::new(Recorder::default());
    let backend = RecordingBackend {
        recorder: Arc::clone(&recorder),
        delay: Duration::ZERO,
        fail_relative: false,
    };
    let (actor, recorder, owner) = fixture(backend, Duration::from_millis(40));

    actor.submit_realtime(owner, realtime(1, 10, 10, 10));
    actor
        .try_submit_reliable(
            owner,
            reliable(
                1,
                1,
                ReliableInputEvent::Key {
                    keycode: 65,
                    state: KeyState::Pressed,
                },
            ),
        )
        .unwrap();
    actor
        .try_submit_reliable(
            owner,
            reliable(
                1,
                2,
                ReliableInputEvent::ReleaseAll {
                    reason: ReleaseAllReason::SessionEnded,
                },
            ),
        )
        .unwrap();

    assert!(actor
        .try_submit_reliable(
            owner,
            reliable(
                1,
                3,
                ReliableInputEvent::Key {
                    keycode: 66,
                    state: KeyState::Pressed,
                },
            ),
        )
        .is_err());
    assert_eq!(
        actor.submit_realtime(owner, realtime(1, 11, 20, 20)),
        RealtimeSubmitResult::EpochClosed
    );
    actor.shutdown().unwrap();
    assert!(recorder.calls.lock().unwrap().is_empty());
}

#[test]
fn reliable_gap_closes_epoch_and_releases_pressed_state() {
    let recorder = Arc::new(Recorder::default());
    let backend = RecordingBackend {
        recorder: Arc::clone(&recorder),
        delay: Duration::ZERO,
        fail_relative: false,
    };
    let (actor, recorder, owner) = fixture(backend, Duration::ZERO);

    actor
        .try_submit_reliable(
            owner,
            reliable(
                1,
                1,
                ReliableInputEvent::Key {
                    keycode: 65,
                    state: KeyState::Pressed,
                },
            ),
        )
        .unwrap();
    recorder.wait_for(1);
    actor
        .try_submit_reliable(
            owner,
            reliable(
                1,
                3,
                ReliableInputEvent::Key {
                    keycode: 66,
                    state: KeyState::Pressed,
                },
            ),
        )
        .unwrap();

    assert_eq!(
        recorder.wait_for(2),
        vec![
            Call::Key(
                65,
                ButtonState::Pressed,
                "rshare-input-injection-test".into()
            ),
            Call::Key(
                65,
                ButtonState::Released,
                "rshare-input-injection-test".into()
            ),
        ]
    );
    assert_eq!(
        actor.submit_realtime(owner, realtime(1, 1, 1, 1)),
        RealtimeSubmitResult::EpochClosed
    );
    actor.shutdown().unwrap();
}

#[test]
fn reliable_duplicate_closes_epoch_and_releases_pressed_state() {
    let recorder = Arc::new(Recorder::default());
    let backend = RecordingBackend {
        recorder: Arc::clone(&recorder),
        delay: Duration::ZERO,
        fail_relative: false,
    };
    let (actor, recorder, owner) = fixture(backend, Duration::ZERO);
    let key_down = reliable(
        1,
        1,
        ReliableInputEvent::Key {
            keycode: 65,
            state: KeyState::Pressed,
        },
    );

    actor.try_submit_reliable(owner, key_down.clone()).unwrap();
    recorder.wait_for(1);
    actor.try_submit_reliable(owner, key_down).unwrap();

    assert_eq!(
        recorder.wait_for(2)[1],
        Call::Key(
            65,
            ButtonState::Released,
            "rshare-input-injection-test".into()
        )
    );
    assert_eq!(
        actor.submit_realtime(owner, realtime(1, 1, 1, 1)),
        RealtimeSubmitResult::EpochClosed
    );
    actor.shutdown().unwrap();
}

#[test]
fn backend_failure_releases_held_controls_locally() {
    let recorder = Arc::new(Recorder::default());
    let backend = RecordingBackend {
        recorder: Arc::clone(&recorder),
        delay: Duration::ZERO,
        fail_relative: true,
    };
    let (actor, recorder, owner) = fixture(backend, Duration::ZERO);

    actor
        .try_submit_reliable(
            owner,
            reliable(
                1,
                1,
                ReliableInputEvent::Key {
                    keycode: 65,
                    state: KeyState::Pressed,
                },
            ),
        )
        .unwrap();
    recorder.wait_for(1);
    actor.submit_realtime(owner, realtime(1, 1, 4, 5));

    assert_eq!(
        recorder.wait_for(2)[1],
        Call::Key(
            65,
            ButtonState::Released,
            "rshare-input-injection-test".into()
        )
    );
    actor.shutdown().unwrap();
}

#[test]
fn lease_timeout_releases_held_controls_locally() {
    let recorder = Arc::new(Recorder::default());
    let backend = RecordingBackend {
        recorder: Arc::clone(&recorder),
        delay: Duration::ZERO,
        fail_relative: false,
    };
    let actor = InputInjectionHandle::spawn(
        Box::new(backend),
        InjectionActorConfig {
            reliable_capacity: 8,
            thread_name: "rshare-input-injection-test".into(),
            realtime_coalesce_window: Duration::ZERO,
        },
    )
    .unwrap();
    let owner = owner();
    actor
        .begin_session(owner, SessionEpoch(1), Duration::from_millis(40))
        .unwrap();
    actor
        .try_submit_reliable(
            owner,
            reliable(
                1,
                1,
                ReliableInputEvent::Key {
                    keycode: 65,
                    state: KeyState::Pressed,
                },
            ),
        )
        .unwrap();

    assert_eq!(
        recorder.wait_for(2),
        vec![
            Call::Key(
                65,
                ButtonState::Pressed,
                "rshare-input-injection-test".into()
            ),
            Call::Key(
                65,
                ButtonState::Released,
                "rshare-input-injection-test".into()
            ),
        ]
    );
    actor.shutdown().unwrap();
}

#[test]
fn connection_loss_request_releases_held_controls_locally() {
    let recorder = Arc::new(Recorder::default());
    let backend = RecordingBackend {
        recorder: Arc::clone(&recorder),
        delay: Duration::ZERO,
        fail_relative: false,
    };
    let (actor, recorder, owner) = fixture(backend, Duration::ZERO);
    actor
        .try_submit_reliable(
            owner,
            reliable(
                1,
                1,
                ReliableInputEvent::Key {
                    keycode: 65,
                    state: KeyState::Pressed,
                },
            ),
        )
        .unwrap();
    recorder.wait_for(1);

    actor.request_release_all(ReleaseAllReason::SessionEnded);

    assert_eq!(
        recorder.wait_for(2)[1],
        Call::Key(
            65,
            ButtonState::Released,
            "rshare-input-injection-test".into()
        )
    );
    actor.shutdown().unwrap();
}

#[test]
fn injection_records_local_start_and_completion_stamps() {
    let recorder = Arc::new(Recorder::default());
    let backend = RecordingBackend {
        recorder: Arc::clone(&recorder),
        delay: Duration::ZERO,
        fail_relative: false,
    };
    let (actor, recorder, owner) = fixture(backend, Duration::ZERO);

    actor.submit_realtime(owner, realtime(1, 4, 2, 3));
    recorder.wait_for(1);
    let sample = actor.latest_timing().expect("missing timing sample");

    assert_eq!(sample.epoch, SessionEpoch(1));
    assert_eq!(sample.sequence, 4);
    assert!(!sample.reliable);
    assert_eq!(
        sample.stamps.received.domain,
        sample.stamps.injection_started.unwrap().domain
    );
    assert!(sample.stamps.received.value_us <= sample.stamps.injection_started.unwrap().value_us);
    assert!(
        sample.stamps.injection_started.unwrap().value_us
            <= sample.stamps.injection_completed.unwrap().value_us
    );
    actor.shutdown().unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn slow_backend_does_not_block_tokio_receiver() {
    let recorder = Arc::new(Recorder::default());
    let backend = RecordingBackend {
        recorder,
        delay: Duration::from_millis(150),
        fail_relative: false,
    };
    let (actor, _, owner) = fixture(backend, Duration::ZERO);

    let started = Instant::now();
    assert_eq!(
        actor.submit_realtime(owner, realtime(1, 1, 1, 1)),
        RealtimeSubmitResult::Accepted
    );
    assert!(
        started.elapsed() < Duration::from_millis(25),
        "submission blocked the async receiver for {:?}",
        started.elapsed()
    );
    tokio::task::yield_now().await;
    actor.shutdown().unwrap();
}

#[test]
fn dropping_last_handle_stops_actor_and_drops_backend() {
    let (dropped_tx, dropped_rx) = mpsc::sync_channel(1);
    let backend = DropSignalingBackend {
        inner: RecordingBackend {
            recorder: Arc::new(Recorder::default()),
            delay: Duration::ZERO,
            fail_relative: false,
        },
        dropped: dropped_tx,
    };
    let actor =
        InputInjectionHandle::spawn(Box::new(backend), InjectionActorConfig::default()).unwrap();

    drop(actor);

    dropped_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("last handle drop must stop the actor thread");
}
