use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use rshare_core::{ControlSessionState, MouseButton, SessionEpoch};
use tokio::sync::{mpsc, watch, Notify};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputUiMutation {
    KeyButton(InputDiscreteProjection),
    Session(ControlSessionState),
}

#[derive(Clone)]
pub struct InputStatePublisher {
    authoritative: watch::Sender<Arc<InputReliableUiProjection>>,
    reliable_tx: mpsc::Sender<InputUiMutation>,
    pointer_tx: watch::Sender<Option<InputPointerProjection>>,
    dirty: Arc<DirtyProjectionNotifier>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputPointerProjection {
    pub session_epoch: SessionEpoch,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDiscreteProjection {
    pub session_epoch: SessionEpoch,
    pub pressed_keys: Vec<u32>,
    pub pressed_buttons: Vec<MouseButton>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputReliableUiProjection {
    pub discrete: InputDiscreteProjection,
    pub session: ControlSessionState,
}

#[derive(Default)]
pub struct DirtyProjectionNotifier {
    dirty: AtomicBool,
    wake: Notify,
}

impl DirtyProjectionNotifier {
    pub fn mark(&self) {
        if !self.dirty.swap(true, Ordering::AcqRel) {
            self.wake.notify_waiters();
        }
    }

    pub fn take(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
    }

    pub async fn notified(&self) {
        if self.is_dirty() {
            return;
        }
        let notified = self.wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.is_dirty() {
            return;
        }
        notified.await;
    }
}

pub struct InputStateFeeds {
    pub reliable_rx: mpsc::Receiver<InputUiMutation>,
    pub authoritative_rx: watch::Receiver<Arc<InputReliableUiProjection>>,
    pub pointer_rx: watch::Receiver<Option<InputPointerProjection>>,
    pub dirty: Arc<DirtyProjectionNotifier>,
}

pub fn input_state_channel(capacity: usize) -> (InputStatePublisher, InputStateFeeds) {
    assert!(capacity > 0, "input state delta capacity must be non-zero");
    let initial = Arc::new(InputReliableUiProjection {
        discrete: InputDiscreteProjection {
            session_epoch: SessionEpoch(0),
            pressed_keys: Vec::new(),
            pressed_buttons: Vec::new(),
        },
        session: ControlSessionState::LocalReady,
    });
    let (authoritative, authoritative_rx) = watch::channel(initial);
    let (reliable_tx, reliable_rx) = mpsc::channel(capacity);
    let (pointer_tx, pointer_rx) = watch::channel(None);
    let dirty = Arc::new(DirtyProjectionNotifier::default());
    (
        InputStatePublisher {
            authoritative,
            reliable_tx,
            pointer_tx,
            dirty: dirty.clone(),
        },
        InputStateFeeds {
            reliable_rx,
            authoritative_rx,
            pointer_rx,
            dirty,
        },
    )
}

impl InputStatePublisher {
    pub fn publish_pointer(&self, pointer: InputPointerProjection) {
        self.pointer_tx.send_replace(Some(pointer));
    }

    pub fn publish_discrete(&self, discrete: InputDiscreteProjection) {
        let current = self.authoritative.borrow().clone();
        self.authoritative
            .send_replace(Arc::new(InputReliableUiProjection {
                discrete: discrete.clone(),
                session: current.session.clone(),
            }));
        if self
            .reliable_tx
            .try_send(InputUiMutation::KeyButton(discrete))
            .is_err()
        {
            self.dirty.mark();
        }
    }

    pub fn publish_session(&self, session: ControlSessionState) {
        let current = self.authoritative.borrow().clone();
        self.authoritative
            .send_replace(Arc::new(InputReliableUiProjection {
                discrete: current.discrete.clone(),
                session: session.clone(),
            }));
        if self
            .reliable_tx
            .try_send(InputUiMutation::Session(session))
            .is_err()
        {
            self.dirty.mark();
        }
    }
}

#[derive(Default)]
pub struct ControlMetrics {
    captured: AtomicU64,
    routed: AtomicU64,
    realtime_replaced: AtomicU64,
    realtime_dropped: AtomicU64,
    reliable_overflow: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlMetricSnapshot {
    pub captured: u64,
    pub routed: u64,
    pub realtime_replaced: u64,
    pub realtime_dropped: u64,
    pub reliable_overflow: u64,
}

impl ControlMetrics {
    pub fn record_captured(&self) {
        self.captured.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_routed(&self) {
        self.routed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_realtime_replaced(&self, count: u64) {
        self.realtime_replaced.fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_realtime_dropped(&self, count: u64) {
        self.realtime_dropped.fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_reliable_overflow(&self, count: u64) {
        self.reliable_overflow.fetch_add(count, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> ControlMetricSnapshot {
        ControlMetricSnapshot {
            captured: self.captured.load(Ordering::Relaxed),
            routed: self.routed.load(Ordering::Relaxed),
            realtime_replaced: self.realtime_replaced.load(Ordering::Relaxed),
            realtime_dropped: self.realtime_dropped.load(Ordering::Relaxed),
            reliable_overflow: self.reliable_overflow.load(Ordering::Relaxed),
        }
    }
}
