//! Bounded, semantic ingress for latency-sensitive input capture callbacks.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rshare_core::{ClockDomainId, GamepadButtonState, GamepadState, MonotonicStamp};
use tokio::sync::Notify;

use crate::events::InputEvent;

static NEXT_CLOCK_DOMAIN: AtomicU64 = AtomicU64::new(1);

/// Monotonic clock used to timestamp capture ingress.
pub trait IngressClock: Send + Sync + 'static {
    fn now(&self) -> MonotonicStamp;
}

#[derive(Debug)]
struct SystemIngressClock {
    domain: ClockDomainId,
    started_at: Instant,
}

impl SystemIngressClock {
    fn new() -> Self {
        let id = NEXT_CLOCK_DOMAIN
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("input ingress clock domains exhausted");
        Self {
            domain: ClockDomainId(id),
            started_at: Instant::now(),
        }
    }
}

impl IngressClock for SystemIngressClock {
    fn now(&self) -> MonotonicStamp {
        let elapsed = self.started_at.elapsed().as_micros();
        MonotonicStamp::new(self.domain, u64::try_from(elapsed).unwrap_or(u64::MAX))
    }
}

/// Capture mechanism that produced an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureSource {
    PortableHook,
    WindowsHook,
    WindowsFilter,
    Evdev,
    Gamepad,
    Test,
}

/// Numeric hot-path capture identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CaptureOrigin {
    pub source: CaptureSource,
    pub device_token: u64,
    pub instance_token: u64,
}

impl Default for CaptureOrigin {
    fn default() -> Self {
        Self {
            source: CaptureSource::PortableHook,
            device_token: 0,
            instance_token: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerPosition {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerSample {
    Absolute {
        x: i32,
        y: i32,
    },
    Relative {
        dx: i32,
        dy: i32,
        observed_x: Option<i32>,
        observed_y: Option<i32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GamepadAxes {
    pub gamepad_id: u8,
    pub sequence: u64,
    pub buttons: Vec<GamepadButtonState>,
    pub left_stick_x: i16,
    pub left_stick_y: i16,
    pub right_stick_x: i16,
    pub right_stick_y: i16,
    pub left_trigger: u16,
    pub right_trigger: u16,
    pub timestamp_ms: u64,
}

impl From<&GamepadState> for GamepadAxes {
    fn from(state: &GamepadState) -> Self {
        Self {
            gamepad_id: state.gamepad_id,
            sequence: state.sequence,
            buttons: state.buttons.clone(),
            left_stick_x: state.left_stick_x,
            left_stick_y: state.left_stick_y,
            right_stick_x: state.right_stick_x,
            right_stick_y: state.right_stick_y,
            left_trigger: state.left_trigger,
            right_trigger: state.right_trigger,
            timestamp_ms: state.timestamp_ms,
        }
    }
}

impl From<GamepadAxes> for GamepadState {
    fn from(axes: GamepadAxes) -> Self {
        Self {
            gamepad_id: axes.gamepad_id,
            sequence: axes.sequence,
            buttons: axes.buttons,
            left_stick_x: axes.left_stick_x,
            left_stick_y: axes.left_stick_y,
            right_stick_x: axes.right_stick_x,
            right_stick_y: axes.right_stick_y,
            left_trigger: axes.left_trigger,
            right_trigger: axes.right_trigger,
            timestamp_ms: axes.timestamp_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuousInput {
    Pointer(PointerSample),
    GamepadAxes(GamepadAxes),
}

#[derive(Debug, Clone)]
pub enum CapturedInputPayload {
    Continuous(ContinuousInput),
    Discrete(InputEvent),
}

impl CapturedInputPayload {
    fn is_replaceable(&self) -> bool {
        matches!(self, Self::Continuous(_))
    }

    fn same_replaceable_class(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Continuous(ContinuousInput::Pointer(PointerSample::Absolute { .. })),
                Self::Continuous(ContinuousInput::Pointer(PointerSample::Absolute { .. })),
            )
            | (
                Self::Continuous(ContinuousInput::Pointer(PointerSample::Relative { .. })),
                Self::Continuous(ContinuousInput::Pointer(PointerSample::Relative { .. })),
            ) => true,
            (
                Self::Continuous(ContinuousInput::GamepadAxes(left)),
                Self::Continuous(ContinuousInput::GamepadAxes(right)),
            ) => left.gamepad_id == right.gamepad_id,
            _ => false,
        }
    }

    pub fn from_input_event(event: InputEvent) -> Self {
        match event {
            InputEvent::MouseMove { x, y } => {
                Self::Continuous(ContinuousInput::Pointer(PointerSample::Absolute { x, y }))
            }
            other => Self::Discrete(other),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CapturedInput {
    pub captured_at: MonotonicStamp,
    pub ingress_enqueued_at: MonotonicStamp,
    pub origin: CaptureOrigin,
    pub pointer: Option<PointerPosition>,
    pub payload: CapturedInputPayload,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum InputEventConversionError {
    #[error("relative input has no complete observed pointer position")]
    RelativePointerWithoutObservedPosition,
}

impl CapturedInput {
    pub fn into_input_event(self) -> Result<InputEvent, InputEventConversionError> {
        match self.payload {
            CapturedInputPayload::Discrete(event) => Ok(event),
            CapturedInputPayload::Continuous(ContinuousInput::Pointer(
                PointerSample::Absolute { x, y },
            )) => Ok(InputEvent::mouse_move(x, y)),
            CapturedInputPayload::Continuous(ContinuousInput::Pointer(
                PointerSample::Relative {
                    observed_x: Some(x),
                    observed_y: Some(y),
                    ..
                },
            )) => Ok(InputEvent::mouse_move(x, y)),
            CapturedInputPayload::Continuous(ContinuousInput::Pointer(
                PointerSample::Relative { .. },
            )) => Err(InputEventConversionError::RelativePointerWithoutObservedPosition),
            CapturedInputPayload::Continuous(ContinuousInput::GamepadAxes(axes)) => {
                Ok(InputEvent::gamepad_state(axes.into()))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    Enqueued,
    Coalesced,
    RealtimeReplaced,
    RealtimeDropped,
    ReliableOverflow,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngressFault {
    ReliableOverflow,
    /// Native capture stopped or lost continuity and may have missed releases.
    CaptureDiscontinuity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngressStats {
    pub capacity: usize,
    pub pending_items: usize,
    pub enqueued: u64,
    pub dequeued: u64,
    pub coalesced_motion: u64,
    pub replaced_realtime: u64,
    pub dropped_realtime: u64,
    pub reliable_overflow: u64,
    pub reliable_overflow_latched: bool,
    pub closed: bool,
}

struct IngressState {
    queue: VecDeque<CapturedInput>,
    capacity: usize,
    latest_pointer: Option<PointerPosition>,
    enqueued: u64,
    dequeued: u64,
    coalesced_motion: u64,
    replaced_realtime: u64,
    dropped_realtime: u64,
    reliable_overflow: u64,
    reliable_overflow_latched: bool,
    capture_discontinuity_latched: bool,
    closed: bool,
}

impl IngressState {
    fn stats(&self) -> IngressStats {
        IngressStats {
            capacity: self.capacity,
            pending_items: self.queue.len(),
            enqueued: self.enqueued,
            dequeued: self.dequeued,
            coalesced_motion: self.coalesced_motion,
            replaced_realtime: self.replaced_realtime,
            dropped_realtime: self.dropped_realtime,
            reliable_overflow: self.reliable_overflow,
            reliable_overflow_latched: self.reliable_overflow_latched,
            closed: self.closed,
        }
    }

    fn oldest_replaceable_index(&self) -> Option<usize> {
        self.queue
            .iter()
            .position(|item| item.payload.is_replaceable())
    }

    fn pop_fault(&mut self) -> Option<IngressFault> {
        if self.capture_discontinuity_latched {
            self.capture_discontinuity_latched = false;
            Some(IngressFault::CaptureDiscontinuity)
        } else if self.reliable_overflow_latched {
            self.reliable_overflow_latched = false;
            Some(IngressFault::ReliableOverflow)
        } else {
            None
        }
    }

    fn update_pointer(&mut self, item: &mut CapturedInput) {
        match &item.payload {
            CapturedInputPayload::Continuous(ContinuousInput::Pointer(
                PointerSample::Absolute { x, y },
            )) => {
                self.latest_pointer = Some(PointerPosition { x: *x, y: *y });
                item.pointer = self.latest_pointer;
            }
            CapturedInputPayload::Continuous(ContinuousInput::Pointer(
                PointerSample::Relative {
                    observed_x,
                    observed_y,
                    ..
                },
            )) => {
                if let (Some(x), Some(y)) = (observed_x, observed_y) {
                    self.latest_pointer = Some(PointerPosition { x: *x, y: *y });
                }
                item.pointer = self.latest_pointer;
            }
            CapturedInputPayload::Discrete(_) => {
                item.pointer = self.latest_pointer;
            }
            CapturedInputPayload::Continuous(ContinuousInput::GamepadAxes(_)) => {}
        }
    }
}

struct SharedIngress {
    state: Mutex<IngressState>,
    ready: Notify,
    clock: Arc<dyn IngressClock>,
    producer_count: AtomicUsize,
}

impl fmt::Debug for SharedIngress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SharedIngress")
            .field(
                "producer_count",
                &self.producer_count.load(Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct SemanticInputProducer {
    shared: Arc<SharedIngress>,
}

impl Clone for SemanticInputProducer {
    fn clone(&self) -> Self {
        self.shared
            .producer_count
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("input ingress producer count exhausted");
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl Drop for SemanticInputProducer {
    fn drop(&mut self) {
        if self.shared.producer_count.fetch_sub(1, Ordering::AcqRel) == 1 {
            let mut state = self
                .shared
                .state
                .lock()
                .expect("input ingress state poisoned");
            state.closed = true;
            drop(state);
            self.shared.ready.notify_waiters();
        }
    }
}

#[derive(Debug)]
pub struct SemanticInputConsumer {
    shared: Arc<SharedIngress>,
}

impl Drop for SemanticInputConsumer {
    fn drop(&mut self) {
        let mut state = self
            .shared
            .state
            .lock()
            .expect("input ingress state poisoned");
        state.closed = true;
        drop(state);
        self.shared.ready.notify_waiters();
    }
}

pub struct SemanticInputIngress;

#[derive(Debug, Clone)]
pub enum IngressEvent {
    Input(CapturedInput),
    Fault(IngressFault),
}

impl SemanticInputIngress {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(capacity: usize) -> (SemanticInputProducer, SemanticInputConsumer) {
        Self::with_clock(capacity, Arc::new(SystemIngressClock::new()))
    }

    pub fn with_clock(
        capacity: usize,
        clock: Arc<dyn IngressClock>,
    ) -> (SemanticInputProducer, SemanticInputConsumer) {
        assert!(capacity > 0, "input ingress capacity must be non-zero");
        let shared = Arc::new(SharedIngress {
            state: Mutex::new(IngressState {
                queue: VecDeque::with_capacity(capacity),
                capacity,
                latest_pointer: None,
                enqueued: 0,
                dequeued: 0,
                coalesced_motion: 0,
                replaced_realtime: 0,
                dropped_realtime: 0,
                reliable_overflow: 0,
                reliable_overflow_latched: false,
                capture_discontinuity_latched: false,
                closed: false,
            }),
            ready: Notify::new(),
            clock,
            producer_count: AtomicUsize::new(1),
        });
        (
            SemanticInputProducer {
                shared: shared.clone(),
            },
            SemanticInputConsumer { shared },
        )
    }
}

impl SemanticInputProducer {
    pub fn now(&self) -> MonotonicStamp {
        self.shared.clock.now()
    }

    pub fn capture(&self, origin: CaptureOrigin, payload: CapturedInputPayload) -> CapturedInput {
        let captured_at = self.now();
        CapturedInput {
            captured_at,
            ingress_enqueued_at: captured_at,
            origin,
            pointer: None,
            payload,
        }
    }

    pub fn try_push_event(&self, origin: CaptureOrigin, event: InputEvent) -> PushOutcome {
        let item = self.capture(origin, CapturedInputPayload::from_input_event(event));
        self.try_push(item)
    }

    pub fn try_push(&self, mut item: CapturedInput) -> PushOutcome {
        item.ingress_enqueued_at = self.shared.clock.now();
        let mut state = self
            .shared
            .state
            .lock()
            .expect("input ingress state poisoned");
        if state.closed {
            return PushOutcome::Closed;
        }

        state.update_pointer(&mut item);
        if item.payload.is_replaceable()
            && state.queue.back().is_some_and(|back| {
                back.origin == item.origin && back.payload.same_replaceable_class(&item.payload)
            })
        {
            let coalesced = {
                let back = state.queue.back_mut().expect("back item was checked");
                coalesce_back(back, item)
            };
            match coalesced {
                Ok(()) => {
                    state.coalesced_motion = checked_increment(state.coalesced_motion);
                    return PushOutcome::Coalesced;
                }
                Err(()) => {
                    state.dropped_realtime = checked_increment(state.dropped_realtime);
                    return PushOutcome::RealtimeDropped;
                }
            }
        }

        if state.queue.len() == state.capacity {
            if let Some(index) = state.oldest_replaceable_index() {
                state.queue.remove(index);
                state.queue.push_back(item);
                state.replaced_realtime = checked_increment(state.replaced_realtime);
                drop(state);
                self.shared.ready.notify_one();
                return PushOutcome::RealtimeReplaced;
            }
            if item.payload.is_replaceable() {
                state.dropped_realtime = checked_increment(state.dropped_realtime);
                return PushOutcome::RealtimeDropped;
            }
            state.reliable_overflow = checked_increment(state.reliable_overflow);
            state.reliable_overflow_latched = true;
            drop(state);
            self.shared.ready.notify_one();
            return PushOutcome::ReliableOverflow;
        }

        state.queue.push_back(item);
        state.enqueued = checked_increment(state.enqueued);
        drop(state);
        self.shared.ready.notify_one();
        PushOutcome::Enqueued
    }

    pub fn report_fault(&self, fault: IngressFault) {
        let mut state = self
            .shared
            .state
            .lock()
            .expect("input ingress state poisoned");
        if state.closed {
            return;
        }
        match fault {
            IngressFault::ReliableOverflow => {
                state.reliable_overflow = checked_increment(state.reliable_overflow);
                state.reliable_overflow_latched = true;
            }
            IngressFault::CaptureDiscontinuity => {
                state.capture_discontinuity_latched = true;
            }
        }
        drop(state);
        self.shared.ready.notify_one();
    }

    pub fn try_pop_fault(&self) -> Option<IngressFault> {
        self.shared
            .state
            .lock()
            .expect("input ingress state poisoned")
            .pop_fault()
    }

    pub fn stats(&self) -> IngressStats {
        self.shared
            .state
            .lock()
            .expect("input ingress state poisoned")
            .stats()
    }

    pub fn close(&self) {
        let mut state = self
            .shared
            .state
            .lock()
            .expect("input ingress state poisoned");
        state.closed = true;
        drop(state);
        self.shared.ready.notify_waiters();
    }
}

impl SemanticInputConsumer {
    pub fn try_recv(&mut self) -> Option<CapturedInput> {
        let mut state = self
            .shared
            .state
            .lock()
            .expect("input ingress state poisoned");
        let item = state.queue.pop_front();
        if item.is_some() {
            state.dequeued = checked_increment(state.dequeued);
        }
        item
    }

    pub fn try_pop_fault(&self) -> Option<IngressFault> {
        self.shared
            .state
            .lock()
            .expect("input ingress state poisoned")
            .pop_fault()
    }

    pub fn stats(&self) -> IngressStats {
        self.shared
            .state
            .lock()
            .expect("input ingress state poisoned")
            .stats()
    }

    pub async fn recv(&mut self) -> Option<CapturedInput> {
        loop {
            let notified = self.shared.ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            {
                let mut state = self
                    .shared
                    .state
                    .lock()
                    .expect("input ingress state poisoned");
                if let Some(item) = state.queue.pop_front() {
                    state.dequeued = checked_increment(state.dequeued);
                    return Some(item);
                }
                if state.closed {
                    return None;
                }
            }

            notified.await;
        }
    }

    pub async fn recv_event(&mut self) -> Option<IngressEvent> {
        loop {
            let notified = self.shared.ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            {
                let mut state = self
                    .shared
                    .state
                    .lock()
                    .expect("input ingress state poisoned");
                if let Some(fault) = state.pop_fault() {
                    return Some(IngressEvent::Fault(fault));
                }
                if let Some(item) = state.queue.pop_front() {
                    state.dequeued = checked_increment(state.dequeued);
                    return Some(IngressEvent::Input(item));
                }
                if state.closed {
                    return None;
                }
            }

            notified.await;
        }
    }
}

fn coalesce_back(back: &mut CapturedInput, incoming: CapturedInput) -> Result<(), ()> {
    match (&mut back.payload, incoming.payload) {
        (
            CapturedInputPayload::Continuous(ContinuousInput::Pointer(PointerSample::Relative {
                dx: back_dx,
                dy: back_dy,
                observed_x: back_observed_x,
                observed_y: back_observed_y,
            })),
            CapturedInputPayload::Continuous(ContinuousInput::Pointer(PointerSample::Relative {
                dx,
                dy,
                observed_x,
                observed_y,
            })),
        ) => {
            let next_dx = back_dx.checked_add(dx).ok_or(())?;
            let next_dy = back_dy.checked_add(dy).ok_or(())?;
            *back_dx = next_dx;
            *back_dy = next_dy;
            if observed_x.is_some() {
                *back_observed_x = observed_x;
            }
            if observed_y.is_some() {
                *back_observed_y = observed_y;
            }
            back.ingress_enqueued_at = incoming.ingress_enqueued_at;
            back.pointer = incoming.pointer;
            Ok(())
        }
        (back_payload, incoming_payload) => {
            *back_payload = incoming_payload;
            back.captured_at = incoming.captured_at;
            back.ingress_enqueued_at = incoming.ingress_enqueued_at;
            back.origin = incoming.origin;
            back.pointer = incoming.pointer;
            Ok(())
        }
    }
}

fn checked_increment(value: u64) -> u64 {
    value
        .checked_add(1)
        .expect("input ingress statistics counter exhausted")
}
