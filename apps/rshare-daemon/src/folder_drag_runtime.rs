//! Associate a local file drag with one authenticated remote button release.
use crate::input_runtime::FileDragObserver;
use anyhow::{bail, Result};
use futures_util::future::BoxFuture;
use rshare_core::{
    file_transfer::FolderDropPoint, AuthenticatedInputOwner, ButtonState, ControlConnectionId,
    DeviceId, MouseButton, ReliableInputEvent, ReliableInputFrame,
};
use rshare_input::{
    CapturedInput, CapturedInputPayload, InputEvent, InputInjectionHandle, KeyCode,
};
use rshare_net::{
    file_transfer::{FileTransferService, FolderDropResolver},
    qos::ConnectionRegistry,
};
use rshare_platform::folder_drop::{self, DragOrigin};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::mpsc;

pub struct NativeFolderResolver(pub InputInjectionHandle);
impl FolderDropResolver for NativeFolderResolver {
    fn resolve(
        &self,
        owner: AuthenticatedInputOwner,
        point: FolderDropPoint,
    ) -> BoxFuture<'_, Result<PathBuf>> {
        Box::pin(async move {
            // The reliable input and bulk streams can arrive in either order.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
            while !self.0.take_folder_drop_receipt(owner, &point) {
                if tokio::time::Instant::now() >= deadline {
                    bail!("没有匹配的跨屏松手事件，请重新拖拽");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            folder_drop::destination_folder(point.x, point.y).await
        })
    }
}

#[derive(Debug)]
enum GestureEvent {
    Begin {
        x: i32,
        y: i32,
    },
    Cross {
        peer: DeviceId,
        epoch: u64,
    },
    Drop {
        peer: DeviceId,
        point: FolderDropPoint,
    },
}

pub struct FolderDragRuntime {
    tx: mpsc::Sender<(u64, GestureEvent)>,
    generation: Arc<AtomicU64>,
    movement: Mutex<Option<(i32, i32, bool)>>,
}

impl FolderDragRuntime {
    pub fn start(registry: Arc<ConnectionRegistry>, files: Arc<FileTransferService>) -> Arc<Self> {
        let (tx, rx) = mpsc::channel(32);
        let generation = Arc::new(AtomicU64::new(0));
        tokio::spawn(run(rx, generation.clone(), registry, files));
        Arc::new(Self {
            tx,
            generation,
            movement: Mutex::new(None),
        })
    }

    fn send(&self, event: GestureEvent) {
        let generation = self.generation.load(Ordering::Acquire);
        if self.tx.try_send((generation, event)).is_err() {
            // Overflow invalidates the gesture, never completing a stale drop.
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
    }
}

impl FileDragObserver for FolderDragRuntime {
    fn captured(&self, input: &CapturedInput, remote: bool) {
        if !remote
            && matches!(
                &input.payload,
                CapturedInputPayload::Continuous(rshare_input::ContinuousInput::Pointer(_))
            )
        {
            if let (Some(pointer), Some((x, y, moved))) =
                (input.pointer, self.movement.lock().unwrap().as_mut())
            {
                *moved |= (i64::from(pointer.x) - i64::from(*x)).abs() >= 8
                    || (i64::from(pointer.y) - i64::from(*y)).abs() >= 8;
            }
        }
        match &input.payload {
            CapturedInputPayload::Discrete(InputEvent::MouseButton {
                button: rshare_input::MouseButton::Left,
                state,
            }) if !remote => {
                self.cancel();
                if state.is_pressed() {
                    if let Some(pointer) = input.pointer {
                        *self.movement.lock().unwrap() = Some((pointer.x, pointer.y, false));
                        self.send(GestureEvent::Begin {
                            x: pointer.x,
                            y: pointer.y,
                        });
                    }
                }
            }
            CapturedInputPayload::Discrete(
                InputEvent::Key {
                    keycode: KeyCode::Escape,
                    ..
                }
                | InputEvent::KeyExtended {
                    keycode: KeyCode::Escape,
                    ..
                },
            ) => self.cancel(),
            _ => {}
        }
    }

    fn sent(&self, target: DeviceId, frame: &ReliableInputFrame) {
        match frame.event {
            ReliableInputEvent::Enter { .. } => {
                if self
                    .movement
                    .lock()
                    .unwrap()
                    .is_some_and(|(_, _, moved)| moved)
                {
                    self.send(GestureEvent::Cross {
                        peer: target,
                        epoch: frame.session_epoch.0,
                    });
                }
            }
            ReliableInputEvent::MouseButton {
                button: MouseButton::Left,
                state: ButtonState::Released,
                x,
                y,
                ..
            } => self.send(GestureEvent::Drop {
                peer: target,
                point: FolderDropPoint {
                    x,
                    y,
                    session_epoch: frame.session_epoch.0,
                    release_sequence: frame.sequence,
                },
            }),
            _ => {}
        }
    }

    fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        *self.movement.lock().unwrap() = None;
    }
}

struct Gesture {
    generation: u64,
    origin: DragOrigin,
    target: Option<(DeviceId, u64, ControlConnectionId)>,
    paths: Vec<String>,
}

impl Gesture {
    fn release_matches(
        &self,
        generation: u64,
        peer: DeviceId,
        point: &FolderDropPoint,
        connection: ControlConnectionId,
    ) -> bool {
        self.generation == generation
            && !self.paths.is_empty()
            && self.target == Some((peer, point.session_epoch, connection))
    }
}

async fn run(
    mut rx: mpsc::Receiver<(u64, GestureEvent)>,
    current: Arc<AtomicU64>,
    registry: Arc<ConnectionRegistry>,
    files: Arc<FileTransferService>,
) {
    let mut gesture: Option<Gesture> = None;
    while let Some((generation, event)) = rx.recv().await {
        if generation != current.load(Ordering::Acquire) {
            gesture = None;
            continue;
        }
        match event {
            GestureEvent::Begin { x, y } => {
                gesture = folder_drop::begin_drag(x, y)
                    .await
                    .ok()
                    .map(|origin| Gesture {
                        generation,
                        origin,
                        target: None,
                        paths: vec![],
                    });
            }
            GestureEvent::Cross { peer, epoch } => {
                let Some(mut candidate) = gesture
                    .take()
                    .filter(|g| g.generation == generation && g.target.is_none())
                else {
                    continue;
                };
                let paths = match folder_drop::dragged_files(candidate.origin.clone()).await {
                    Ok(paths) if !paths.is_empty() => paths,
                    Ok(_) => continue,
                    Err(error) => {
                        files.record_folder_drag_failure(
                            peer,
                            format!("无法读取拖拽文件：{error:#}"),
                        );
                        continue;
                    }
                };
                if current.load(Ordering::Acquire) != generation {
                    continue;
                }
                // The real local drag has been captured. Escape releases Finder /
                // Explorer's native drag loop while the physical gesture continues.
                if let Err(error) = folder_drop::cancel_native_drag().await {
                    files.record_folder_drag_failure(peer, format!("无法结束本机拖拽：{error:#}"));
                    continue;
                }
                let Some(connection) = registry.peer(&peer).filter(|p| p.folder_drop_version == 1)
                else {
                    files.record_folder_drag_failure(
                        peer,
                        "远端不支持文件夹跨屏拖拽，请更新两端 R-ShareMouse".into(),
                    );
                    continue;
                };
                candidate.paths = paths;
                candidate.target = Some((peer, epoch, connection.auth.control_connection_id));
                gesture = Some(candidate);
            }
            GestureEvent::Drop { peer, point } => {
                let Some(candidate) = gesture.take() else {
                    continue;
                };
                let Some(connection) = registry.peer(&peer) else {
                    continue;
                };
                if current.load(Ordering::Acquire) != generation
                    || !candidate.release_matches(
                        generation,
                        peer,
                        &point,
                        connection.auth.control_connection_id,
                    )
                {
                    continue;
                }
                if let Err(error) = files.send_files_to_folder(peer, candidate.paths, point) {
                    files.record_folder_drag_failure(peer, format!("跨屏文件投放失败：{error:#}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(payload: CapturedInputPayload, x: i32, y: i32) -> CapturedInput {
        let stamp = rshare_core::MonotonicStamp::new(rshare_core::ClockDomainId(1), 1);
        CapturedInput {
            captured_at: stamp,
            ingress_enqueued_at: stamp,
            origin: Default::default(),
            pointer: Some(rshare_input::PointerPosition { x, y }),
            payload,
        }
    }

    #[test]
    fn local_release_escape_and_queue_overflow_invalidate_pending_gestures() {
        let (tx, mut rx) = mpsc::channel(2);
        let observer = FolderDragRuntime {
            tx,
            generation: Arc::new(AtomicU64::new(0)),
            movement: Mutex::new(None),
        };
        let down = sample(
            CapturedInputPayload::Discrete(InputEvent::MouseButton {
                button: rshare_input::MouseButton::Left,
                state: rshare_input::ButtonState::Pressed,
            }),
            10,
            20,
        );
        observer.captured(&down, false);
        let (begin, _) = rx.try_recv().unwrap();
        assert_eq!(begin, observer.generation.load(Ordering::Acquire));
        let up = sample(
            CapturedInputPayload::Discrete(InputEvent::MouseButton {
                button: rshare_input::MouseButton::Left,
                state: rshare_input::ButtonState::Released,
            }),
            10,
            20,
        );
        observer.captured(&up, true);
        assert_eq!(begin, observer.generation.load(Ordering::Acquire));
        observer.captured(&up, false);
        assert_ne!(begin, observer.generation.load(Ordering::Acquire));
        observer.captured(&down, true);
        assert!(rx.try_recv().is_err()); // Remote button presses cannot become file origins.
        observer.captured(&down, false);
        let (begin, _) = rx.try_recv().unwrap();
        observer.captured(
            &sample(
                CapturedInputPayload::Discrete(InputEvent::Key {
                    keycode: KeyCode::Escape,
                    state: rshare_input::ButtonState::Pressed,
                }),
                10,
                20,
            ),
            true,
        );
        assert_ne!(begin, observer.generation.load(Ordering::Acquire));
        let before = observer.generation.load(Ordering::Acquire);
        for _ in 0..3 {
            observer.send(GestureEvent::Begin { x: 10, y: 20 });
        }
        assert_ne!(before, observer.generation.load(Ordering::Acquire));
    }

    #[test]
    fn crossing_requires_drag_distance_and_preserves_remote_release_coordinates() {
        let (tx, mut rx) = mpsc::channel(8);
        let observer = FolderDragRuntime {
            tx,
            generation: Arc::new(AtomicU64::new(0)),
            movement: Mutex::new(None),
        };
        let down = sample(
            CapturedInputPayload::Discrete(InputEvent::MouseButton {
                button: rshare_input::MouseButton::Left,
                state: rshare_input::ButtonState::Pressed,
            }),
            10,
            20,
        );
        observer.captured(&down, false);
        rx.try_recv().unwrap();
        let peer = DeviceId::new_v4();
        let mut frame = ReliableInputFrame {
            protocol_version: rshare_core::INPUT_PROTOCOL_VERSION,
            session_epoch: rshare_core::SessionEpoch(3),
            sequence: 1,
            captured_at: down.captured_at,
            event: ReliableInputEvent::Enter {
                x: 0,
                y: 200,
                target_display_id: "remote".into(),
            },
        };
        observer.sent(peer, &frame);
        assert!(rx.try_recv().is_err());
        observer.captured(
            &sample(
                CapturedInputPayload::Continuous(rshare_input::ContinuousInput::Pointer(
                    rshare_input::PointerSample::Absolute { x: 40, y: 20 },
                )),
                40,
                20,
            ),
            false,
        );
        observer.sent(peer, &frame);
        assert!(
            matches!(rx.try_recv().unwrap().1, GestureEvent::Cross { peer: target, epoch: 3 } if target == peer)
        );
        frame.sequence = 19;
        frame.event = ReliableInputEvent::MouseButton {
            button: MouseButton::Left,
            state: ButtonState::Released,
            x: -450,
            y: 680,
            realtime_anchor_sequence: 8,
        };
        observer.sent(peer, &frame);
        let GestureEvent::Drop { point, .. } = rx.try_recv().unwrap().1 else {
            panic!("missing drop");
        };
        assert_eq!(
            point,
            FolderDropPoint {
                x: -450,
                y: 680,
                session_epoch: 3,
                release_sequence: 19
            }
        );
    }

    #[test]
    fn release_requires_the_original_gesture_target_epoch_and_connection() {
        let peer = DeviceId::new_v4();
        let connection = ControlConnectionId::new();
        let mut gesture = Gesture {
            generation: 7,
            origin: DragOrigin::default(),
            target: Some((peer, 9, connection)),
            paths: vec!["/test/file.txt".into()],
        };
        let point = FolderDropPoint {
            x: 420,
            y: 360,
            session_epoch: 9,
            release_sequence: 42,
        };
        assert!(gesture.release_matches(7, peer, &point, connection));
        assert!(!gesture.release_matches(8, peer, &point, connection));
        assert!(!gesture.release_matches(7, DeviceId::new_v4(), &point, connection));
        assert!(!gesture.release_matches(7, peer, &point, ControlConnectionId::new()));
        assert!(!gesture.release_matches(
            7,
            peer,
            &FolderDropPoint {
                session_epoch: 10,
                ..point
            },
            connection
        ));
        gesture.paths.clear();
        assert!(!gesture.release_matches(7, peer, &point, connection));
    }
}
