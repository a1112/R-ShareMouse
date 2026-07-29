use std::collections::{HashSet, VecDeque};
use std::future::pending;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use futures_util::future::BoxFuture;
use rshare_core::{
    ButtonState, CapabilityRegistrySnapshot, DaemonDeviceSnapshot, DeviceId, KeyState,
    LatencyFeedbackSnapshot, LayoutGraph, LocalDisplayState, LocalGamepadState,
    ServiceStatusSnapshot, UiActiveSessions, UiChange, UiCursor, UiDelta, UiDiscreteInputState,
    UiEnvelope, UiMediaSession, UiPointerState, UiResyncReason, UiRevisionSequencer, UiSnapshot,
    UiView, UI_STATE_PROTOCOL_VERSION,
};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::Instant;

use crate::input_state::{
    ControlMetricSnapshot, InputDiscreteProjection, InputPointerProjection,
    InputReliableUiProjection, InputStateFeeds, InputStatePublisher, InputUiMutation,
};

pub const DEFAULT_REPLAY_CAPACITY: usize = 1024;
const DEFAULT_RELIABLE_CAPACITY: usize = 64;
const DEFAULT_COMMAND_CAPACITY: usize = 32;
const NETWORK_RECONCILE_DEBOUNCE: Duration = Duration::from_millis(50);

pub trait UiProjectionSource: Send + Sync + 'static {
    fn project(&self) -> BoxFuture<'_, Result<UiSnapshot>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateChange {
    Status(ServiceStatusSnapshot),
    Capabilities(CapabilityRegistrySnapshot),
    DeviceUpsert(DaemonDeviceSnapshot),
    DeviceRemove(DeviceId),
    Topology(LayoutGraph),
    DisplayInventory(LocalDisplayState),
    KeyButton(UiDiscreteInputState),
    Session(UiActiveSessions),
    Diagnostics(LatencyFeedbackSnapshot),
    MediaSessionUpsert(UiMediaSession),
    MediaSessionRemove(DeviceId),
}

impl StateChange {
    fn into_ui_change(self) -> UiChange {
        match self {
            Self::Status(value) => UiChange::Status(value),
            Self::Capabilities(value) => UiChange::Capabilities(value),
            Self::DeviceUpsert(value) => UiChange::DeviceUpsert(value),
            Self::DeviceRemove(value) => UiChange::DeviceRemove(value),
            Self::Topology(value) => UiChange::Topology(value),
            Self::DisplayInventory(value) => UiChange::DisplayInventory(value),
            Self::KeyButton(value) => UiChange::KeyButton(value),
            Self::Session(value) => UiChange::Session(value),
            Self::Diagnostics(value) => UiChange::Diagnostics(value),
            Self::MediaSessionUpsert(value) => UiChange::MediaSessionUpsert(value),
            Self::MediaSessionRemove(value) => UiChange::MediaSessionRemove(value),
        }
    }
}

enum AggregatorCommand {
    Subscribe {
        cursor: Option<UiCursor>,
        response: oneshot::Sender<StateSubscriber>,
    },
    RecoverLag {
        response: oneshot::Sender<(broadcast::Receiver<UiEnvelope>, Arc<UiSnapshot>)>,
    },
    Reconcile {
        response: oneshot::Sender<std::result::Result<(), String>>,
    },
}

#[derive(Clone)]
struct PublishedUiState {
    base: Arc<UiSnapshot>,
    cursor: UiCursor,
    pointer: Option<UiPointerState>,
    gamepads: Arc<Vec<LocalGamepadState>>,
}

impl PublishedUiState {
    fn from_snapshot(snapshot: &UiSnapshot) -> Self {
        Self {
            base: Arc::new(snapshot.clone()),
            cursor: snapshot.cursor(),
            pointer: snapshot.dynamic_state.pointer.clone(),
            gamepads: Arc::new(snapshot.dynamic_state.gamepads.clone()),
        }
    }

    fn materialize(&self) -> Arc<UiSnapshot> {
        let mut snapshot = self.base.as_ref().clone();
        snapshot.boot_id = self.cursor.boot_id;
        snapshot.revision = self.cursor.revision;
        snapshot.dynamic_state.pointer = self.pointer.clone();
        snapshot.dynamic_state.gamepads = self.gamepads.as_ref().clone();
        Arc::new(snapshot)
    }
}

#[derive(Clone)]
pub struct StateAggregatorHandle {
    reliable_tx: mpsc::Sender<StateChange>,
    command_tx: mpsc::Sender<AggregatorCommand>,
    published: watch::Receiver<Arc<PublishedUiState>>,
    pointer: watch::Receiver<Option<InputPointerProjection>>,
    reliable_history_len: Arc<AtomicUsize>,
    #[cfg(test)]
    full_snapshot_publications: Arc<AtomicUsize>,
    // Keeps the Task-12 channels open for aggregators that have no external input actor.
    _internal_input: Option<InputStatePublisher>,
}

pub type StateAggregator = StateAggregatorHandle;

impl StateAggregatorHandle {
    pub fn new(initial: UiSnapshot, subscriber_capacity: usize) -> Self {
        Self::try_new(initial, subscriber_capacity)
            .expect("StateAggregator initial configuration must satisfy the UI contract")
    }

    pub fn try_new(initial: UiSnapshot, subscriber_capacity: usize) -> Result<Self> {
        Self::try_spawn(initial, subscriber_capacity, None, None, None, None)
    }

    pub fn with_input(
        initial: UiSnapshot,
        subscriber_capacity: usize,
        input: InputStateFeeds,
    ) -> Self {
        Self::try_with_input(initial, subscriber_capacity, input)
            .expect("StateAggregator initial configuration must satisfy the UI contract")
    }

    pub fn try_with_input(
        initial: UiSnapshot,
        subscriber_capacity: usize,
        input: InputStateFeeds,
    ) -> Result<Self> {
        Self::try_spawn(initial, subscriber_capacity, Some(input), None, None, None)
    }

    pub fn with_projection(
        initial: UiSnapshot,
        subscriber_capacity: usize,
        input: InputStateFeeds,
        projection: Arc<dyn UiProjectionSource>,
    ) -> Self {
        Self::try_with_projection(initial, subscriber_capacity, input, projection)
            .expect("StateAggregator initial configuration must satisfy the UI contract")
    }

    pub fn try_with_projection(
        initial: UiSnapshot,
        subscriber_capacity: usize,
        input: InputStateFeeds,
        projection: Arc<dyn UiProjectionSource>,
    ) -> Result<Self> {
        Self::try_spawn(
            initial.clone(),
            subscriber_capacity,
            Some(input),
            Some(projection),
            None,
            None,
        )
    }

    pub fn try_with_projection_and_diagnostics(
        initial: UiSnapshot,
        subscriber_capacity: usize,
        input: InputStateFeeds,
        projection: Arc<dyn UiProjectionSource>,
        diagnostics_rx: watch::Receiver<ControlMetricSnapshot>,
        network_rx: watch::Receiver<u64>,
    ) -> Result<Self> {
        Self::try_spawn(
            initial,
            subscriber_capacity,
            Some(input),
            Some(projection),
            Some(diagnostics_rx),
            Some(network_rx),
        )
    }

    fn try_spawn(
        mut initial: UiSnapshot,
        subscriber_capacity: usize,
        mut input: Option<InputStateFeeds>,
        projection: Option<Arc<dyn UiProjectionSource>>,
        diagnostics_rx: Option<watch::Receiver<ControlMetricSnapshot>>,
        network_rx: Option<watch::Receiver<u64>>,
    ) -> Result<Self> {
        anyhow::ensure!(
            subscriber_capacity > 0,
            "UI subscriber capacity must be non-zero"
        );
        let mut input_cut_generation = 0;
        if let Some(input) = input.as_mut() {
            let authoritative = input.authoritative_rx.borrow_and_update().clone();
            input_cut_generation = authoritative.generation;
            let pointer = *input.pointer_rx.borrow_and_update();
            let _ = input.gamepads_rx.borrow_and_update();
            overlay_input_projection(&mut initial, authoritative.as_ref(), pointer);
        }
        let boot_id = DeviceId::new_v4();
        initial.protocol_version = UI_STATE_PROTOCOL_VERSION;
        initial.boot_id = boot_id;
        initial.revision = 0;
        let (reliable_tx, reliable_rx) = mpsc::channel(DEFAULT_RELIABLE_CAPACITY);
        let (command_tx, command_rx) = mpsc::channel(DEFAULT_COMMAND_CAPACITY);
        let view = UiView::from_snapshot(initial)
            .context("StateAggregator initial snapshot violates the UI contract")?;
        let (published_tx, published) =
            watch::channel(Arc::new(PublishedUiState::from_snapshot(view.snapshot())));
        let (envelopes, _) = broadcast::channel(subscriber_capacity);
        let reliable_history_len = Arc::new(AtomicUsize::new(0));
        #[cfg(test)]
        let full_snapshot_publications = Arc::new(AtomicUsize::new(1));

        let (
            input_reliable_rx,
            input_authoritative_rx,
            pointer_rx,
            gamepads_rx,
            dirty,
            internal_input,
            pointer,
        ) = match input {
            Some(input) => {
                let pointer = input.pointer_rx.clone();
                (
                    Some(input.reliable_rx),
                    Some(input.authoritative_rx),
                    Some(input.pointer_rx),
                    Some(input.gamepads_rx),
                    Some(input.dirty),
                    None,
                    pointer,
                )
            }
            None => {
                let (publisher, feeds) = crate::input_state::input_state_channel(1);
                let pointer = feeds.pointer_rx.clone();
                (None, None, None, None, None, Some(publisher), pointer)
            }
        };

        let actor = AggregatorActor {
            sequencer: UiRevisionSequencer::new(boot_id),
            view,
            projection,
            reliable_rx,
            gamepads_rx,
            command_tx: command_tx.clone(),
            command_rx,
            input_reliable_rx,
            input_authoritative_rx,
            pointer_rx,
            diagnostics_rx,
            network_rx,
            network_reconcile_due: None,
            dirty,
            input_cut_generation,
            published_tx,
            envelopes,
            replay: VecDeque::with_capacity(DEFAULT_REPLAY_CAPACITY),
            replay_capacity: DEFAULT_REPLAY_CAPACITY,
            reliable_history_len: reliable_history_len.clone(),
            #[cfg(test)]
            full_snapshot_publications: full_snapshot_publications.clone(),
        };
        tokio::spawn(actor.run());

        Ok(Self {
            reliable_tx,
            command_tx,
            published,
            pointer,
            reliable_history_len,
            #[cfg(test)]
            full_snapshot_publications,
            _internal_input: internal_input,
        })
    }

    pub async fn publish(
        &self,
        change: StateChange,
    ) -> std::result::Result<(), mpsc::error::SendError<StateChange>> {
        self.reliable_tx.send(change).await
    }

    pub async fn subscribe(&self, cursor: Option<UiCursor>) -> Result<StateSubscriber> {
        let (response, receiver) = oneshot::channel();
        self.command_tx
            .send(AggregatorCommand::Subscribe { cursor, response })
            .await
            .context("UI state aggregator stopped before subscription")?;
        receiver
            .await
            .context("UI state aggregator stopped during subscription")
    }

    pub async fn reconcile_from_projection(&self) -> Result<()> {
        let (response, receiver) = oneshot::channel();
        self.command_tx
            .send(AggregatorCommand::Reconcile { response })
            .await
            .context("UI state aggregator stopped before reconcile")?;
        receiver
            .await
            .context("UI state aggregator stopped during reconcile")?
            .map_err(anyhow::Error::msg)
    }

    pub fn latest_snapshot(&self) -> Arc<UiSnapshot> {
        self.published.borrow().materialize()
    }

    pub fn heartbeat(&self, sent_at_ms: u64) -> UiEnvelope {
        let cursor = self.published.borrow().cursor;
        UiEnvelope::Heartbeat {
            boot_id: cursor.boot_id,
            revision: cursor.revision,
            sent_at_ms,
        }
    }

    pub async fn wait_for_revision(&self, revision: u64) -> Result<Arc<UiSnapshot>> {
        let mut published = self.published.clone();
        loop {
            let state = published.borrow().clone();
            if state.cursor.revision >= revision {
                return Ok(state.materialize());
            }
            published
                .changed()
                .await
                .context("UI state aggregator stopped before reaching the requested revision")?;
        }
    }

    pub fn latest_pointer(&self) -> Option<InputPointerProjection> {
        *self.pointer.borrow()
    }

    pub fn reliable_history_len(&self) -> usize {
        self.reliable_history_len.load(Ordering::Acquire)
    }

    #[cfg(test)]
    fn full_snapshot_publication_count(&self) -> usize {
        self.full_snapshot_publications.load(Ordering::Acquire)
    }
}

pub struct StateSubscriber {
    pending: VecDeque<UiEnvelope>,
    live: broadcast::Receiver<UiEnvelope>,
    command_tx: mpsc::Sender<AggregatorCommand>,
}

impl StateSubscriber {
    pub async fn recv(&mut self) -> Result<UiEnvelope> {
        if let Some(envelope) = self.pending.pop_front() {
            return Ok(envelope);
        }
        match self.live.recv().await {
            Ok(envelope) => Ok(envelope),
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let (response, receiver) = oneshot::channel();
                self.command_tx
                    .send(AggregatorCommand::RecoverLag { response })
                    .await
                    .context("UI state aggregator stopped during lag recovery")?;
                let (live, snapshot) = receiver
                    .await
                    .context("UI state aggregator stopped during lag recovery")?;
                self.live = live;
                self.pending
                    .push_back(UiEnvelope::Snapshot((*snapshot).clone()));
                Ok(UiEnvelope::ResyncRequired {
                    boot_id: snapshot.boot_id,
                    current_revision: snapshot.revision,
                    reason: UiResyncReason::RevisionGap,
                })
            }
            Err(broadcast::error::RecvError::Closed) => {
                anyhow::bail!("UI state aggregator stopped")
            }
        }
    }
}

struct AggregatorActor {
    sequencer: UiRevisionSequencer,
    view: UiView,
    projection: Option<Arc<dyn UiProjectionSource>>,
    reliable_rx: mpsc::Receiver<StateChange>,
    gamepads_rx: Option<watch::Receiver<Vec<LocalGamepadState>>>,
    command_tx: mpsc::Sender<AggregatorCommand>,
    command_rx: mpsc::Receiver<AggregatorCommand>,
    input_reliable_rx: Option<mpsc::Receiver<InputUiMutation>>,
    input_authoritative_rx: Option<watch::Receiver<Arc<InputReliableUiProjection>>>,
    pointer_rx: Option<watch::Receiver<Option<InputPointerProjection>>>,
    diagnostics_rx: Option<watch::Receiver<ControlMetricSnapshot>>,
    network_rx: Option<watch::Receiver<u64>>,
    network_reconcile_due: Option<Instant>,
    dirty: Option<Arc<crate::input_state::DirtyProjectionNotifier>>,
    input_cut_generation: u64,
    published_tx: watch::Sender<Arc<PublishedUiState>>,
    envelopes: broadcast::Sender<UiEnvelope>,
    replay: VecDeque<UiDelta>,
    replay_capacity: usize,
    reliable_history_len: Arc<AtomicUsize>,
    #[cfg(test)]
    full_snapshot_publications: Arc<AtomicUsize>,
}

impl AggregatorActor {
    async fn run(mut self) {
        loop {
            tokio::select! {
                biased;
                _ = wait_for_dirty(&self.dirty), if self.dirty.is_some() => {
                    if self.dirty.as_ref().is_some_and(|dirty| dirty.take()) {
                        let generation = self
                            .input_authoritative_rx
                            .as_ref()
                            .map(|state| state.borrow().generation)
                            .unwrap_or_default();
                        if generation > self.input_cut_generation
                            && self.rebuild_from_authoritative_projection().await.is_err()
                        {
                            break;
                        }
                    }
                }
                command = self.command_rx.recv() => {
                    let Some(command) = command else { break };
                    if self.handle_command(command).await.is_err() {
                        break;
                    }
                }
                change = self.reliable_rx.recv() => {
                    let Some(change) = change else { break };
                    if self.emit_reliable(change.into_ui_change()).is_err()
                        && self.rebuild_from_authoritative_projection().await.is_err()
                    {
                        break;
                    }
                }
                mutation = recv_input_mutation(&mut self.input_reliable_rx), if self.input_reliable_rx.is_some() => {
                    match mutation {
                        Some(mutation) => {
                            if self.apply_input_mutation(mutation).is_err()
                                && self.rebuild_from_authoritative_projection().await.is_err()
                            {
                                break;
                            }
                        }
                        None => self.input_reliable_rx = None,
                    }
                }
                changed = wait_for_pointer(&mut self.pointer_rx), if self.pointer_rx.is_some() => {
                    if changed {
                        let pointer = self.pointer_rx.as_mut().and_then(|pointer_rx| {
                            *pointer_rx.borrow_and_update()
                        });
                        if let Some(pointer) = pointer {
                            if self.emit_realtime(UiChange::Pointer(UiPointerState {
                                x: pointer.x,
                                y: pointer.y,
                                display_id: None,
                                observed_at_ms: timestamp_ms_now(),
                            })).is_err() {
                                break;
                            }
                        }
                    } else {
                        self.pointer_rx = None;
                    }
                }
                changed = wait_for_gamepads(&mut self.gamepads_rx), if self.gamepads_rx.is_some() => {
                    if changed {
                        let gamepads = self.gamepads_rx.as_mut().map(|gamepads_rx| {
                            gamepads_rx.borrow_and_update().clone()
                        });
                        if let Some(gamepads) = gamepads {
                            if self.emit_realtime(UiChange::Gamepads(gamepads)).is_err() {
                                break;
                            }
                        }
                    } else {
                        self.gamepads_rx = None;
                    }
                }
                changed = wait_for_diagnostics(&mut self.diagnostics_rx), if self.diagnostics_rx.is_some() => {
                    if changed {
                        if self.reconcile_from_authoritative_projection().await.is_err() {
                            break;
                        }
                    } else {
                        self.diagnostics_rx = None;
                    }
                }
                _ = wait_for_network_reconcile(self.network_reconcile_due), if self.network_reconcile_due.is_some() => {
                    self.network_reconcile_due = None;
                    if self.reconcile_from_authoritative_projection().await.is_err() {
                        break;
                    }
                }
                changed = wait_for_network(&mut self.network_rx), if self.network_rx.is_some() && self.network_reconcile_due.is_none() => {
                    if changed {
                        self.network_reconcile_due =
                            Some(Instant::now() + NETWORK_RECONCILE_DEBOUNCE);
                    } else {
                        self.network_rx = None;
                    }
                }
            }
        }
    }

    async fn handle_command(&mut self, command: AggregatorCommand) -> Result<()> {
        match command {
            AggregatorCommand::Subscribe { cursor, response } => {
                let live = self.envelopes.subscribe();
                let snapshot = self.view.snapshot().clone();
                let mut pending = VecDeque::new();
                match cursor {
                    None => pending.push_back(UiEnvelope::Snapshot(snapshot)),
                    Some(cursor) if cursor.boot_id != snapshot.boot_id => {
                        push_resync_snapshot(&mut pending, &snapshot, UiResyncReason::BootChanged);
                    }
                    Some(cursor) if cursor.revision > snapshot.revision => {
                        push_resync_snapshot(&mut pending, &snapshot, UiResyncReason::RevisionGap);
                    }
                    Some(cursor) if cursor.revision == snapshot.revision => {}
                    Some(cursor) => {
                        let expected = cursor.revision.saturating_add(1);
                        let replay = self
                            .replay
                            .iter()
                            .filter(|delta| delta.revision >= expected)
                            .cloned()
                            .collect::<Vec<_>>();
                        let contiguous = replay
                            .iter()
                            .enumerate()
                            .all(|(offset, delta)| delta.revision == expected + offset as u64)
                            && replay
                                .last()
                                .is_some_and(|delta| delta.revision == snapshot.revision);
                        if contiguous {
                            pending.extend(replay.into_iter().map(UiEnvelope::Delta));
                        } else {
                            push_resync_snapshot(
                                &mut pending,
                                &snapshot,
                                UiResyncReason::HistoryExpired,
                            );
                        }
                    }
                }
                let _ = response.send(StateSubscriber {
                    pending,
                    live,
                    command_tx: self.command_tx.clone(),
                });
            }
            AggregatorCommand::RecoverLag { response } => {
                let live = self.envelopes.subscribe();
                let snapshot = Arc::new(self.view.snapshot().clone());
                let _ = response.send((live, snapshot));
            }
            AggregatorCommand::Reconcile { response } => {
                let result = self
                    .reconcile_from_authoritative_projection()
                    .await
                    .map_err(|error| format!("{error:#}"));
                let failed = result.is_err();
                let _ = response.send(result);
                if failed {
                    anyhow::bail!("authoritative UI projection reconcile failed");
                }
            }
        }
        Ok(())
    }

    fn emit_reliable(&mut self, change: UiChange) -> Result<()> {
        let delta = self.apply_delta(change, true)?;
        if self.replay.len() == self.replay_capacity {
            self.replay.pop_front();
        }
        self.replay.push_back(delta);
        self.reliable_history_len
            .store(self.replay.len(), Ordering::Release);
        Ok(())
    }

    fn emit_realtime(&mut self, change: UiChange) -> Result<()> {
        self.apply_delta(change, false).map(|_| ())
    }

    fn apply_delta(&mut self, change: UiChange, publish_full: bool) -> Result<UiDelta> {
        let mut sequencer = self.sequencer.clone();
        let delta = sequencer
            .emit_delta(change)
            .context("failed to allocate the next UI state revision")?;
        self.view
            .apply(UiEnvelope::Delta(delta.clone()))
            .context("UI state mutation violated the projection contract")?;
        self.sequencer = sequencer;
        self.publish_view(&delta.change, publish_full);
        let _ = self.envelopes.send(UiEnvelope::Delta(delta.clone()));
        Ok(delta)
    }

    fn publish_view(&self, change: &UiChange, publish_full: bool) {
        let previous = self.published_tx.borrow().clone();
        let pointer = match change {
            UiChange::Pointer(pointer) => Some(pointer.clone()),
            _ => previous.pointer.clone(),
        };
        let gamepads = match change {
            UiChange::Gamepads(gamepads) => Arc::new(gamepads.clone()),
            _ => previous.gamepads.clone(),
        };
        let base = if publish_full {
            #[cfg(test)]
            self.full_snapshot_publications
                .fetch_add(1, Ordering::AcqRel);
            Arc::new(self.view.snapshot().clone())
        } else {
            previous.base.clone()
        };
        self.published_tx.send_replace(Arc::new(PublishedUiState {
            base,
            cursor: self.view.snapshot().cursor(),
            pointer,
            gamepads,
        }));
    }

    fn apply_input_mutation(&mut self, mutation: InputUiMutation) -> Result<()> {
        match mutation {
            InputUiMutation::KeyButton {
                generation,
                projection,
            } => {
                if generation <= self.input_cut_generation {
                    return Ok(());
                }
                for change in discrete_changes(self.view.snapshot(), &projection) {
                    self.emit_reliable(UiChange::KeyButton(change))?;
                }
                self.input_cut_generation = generation;
            }
            InputUiMutation::GamepadButtons {
                generation,
                transitions,
            } => {
                if generation <= self.input_cut_generation {
                    return Ok(());
                }
                for transition in transitions {
                    if gamepad_button_transition_changes(self.view.snapshot(), &transition) {
                        self.emit_reliable(UiChange::KeyButton(transition))?;
                    }
                }
                self.input_cut_generation = generation;
            }
            InputUiMutation::Session {
                generation,
                session,
            } => {
                if generation <= self.input_cut_generation {
                    return Ok(());
                }
                if self.view.snapshot().active_sessions.control.as_ref() == Some(&session) {
                    self.input_cut_generation = generation;
                    return Ok(());
                }
                let mut sessions = self.view.snapshot().active_sessions.clone();
                sessions.control = Some(session);
                self.emit_reliable(UiChange::Session(sessions))?;
                self.input_cut_generation = generation;
            }
        }
        Ok(())
    }

    async fn rebuild_from_authoritative_projection(&mut self) -> Result<()> {
        let (mut snapshot, input_cut_generation) = self.project_authoritative_snapshot().await?;
        UiView::from_snapshot(snapshot.clone())
            .context("rebuilt authoritative UI projection is invalid")?;
        let boot_id = DeviceId::new_v4();
        snapshot.boot_id = boot_id;
        snapshot.revision = 0;
        let view =
            UiView::from_snapshot(snapshot).context("replacement UI projection is invalid")?;
        let snapshot = view.snapshot().clone();
        self.sequencer = UiRevisionSequencer::new(boot_id);
        self.view = view;
        self.input_cut_generation = input_cut_generation;
        self.replay.clear();
        self.reliable_history_len.store(0, Ordering::Release);
        self.published_tx
            .send_replace(Arc::new(PublishedUiState::from_snapshot(&snapshot)));
        #[cfg(test)]
        self.full_snapshot_publications
            .fetch_add(1, Ordering::AcqRel);
        let resync = UiEnvelope::ResyncRequired {
            boot_id: snapshot.boot_id,
            current_revision: snapshot.revision,
            reason: UiResyncReason::ProjectionRebuilt,
        };
        let _ = self.envelopes.send(resync);
        let _ = self.envelopes.send(UiEnvelope::Snapshot(snapshot));
        Ok(())
    }

    async fn reconcile_from_authoritative_projection(&mut self) -> Result<()> {
        let (mut snapshot, input_cut_generation) = self.project_authoritative_snapshot().await?;
        if pointer_identity_eq(
            snapshot.dynamic_state.pointer.as_ref(),
            self.view.snapshot().dynamic_state.pointer.as_ref(),
        ) {
            snapshot.dynamic_state.pointer = self.view.snapshot().dynamic_state.pointer.clone();
        }
        if capabilities_semantically_eq(&snapshot.capabilities, &self.view.snapshot().capabilities)
        {
            snapshot.capabilities.generated_at_ms =
                self.view.snapshot().capabilities.generated_at_ms;
        }
        if latency_feedback_semantically_eq(
            &snapshot.status.latency_feedback,
            &self.view.snapshot().status.latency_feedback,
        ) {
            snapshot.status.latency_feedback.generated_at_ms =
                self.view.snapshot().status.latency_feedback.generated_at_ms;
            snapshot.dynamic_state.diagnostics.generated_at_ms = self
                .view
                .snapshot()
                .dynamic_state
                .diagnostics
                .generated_at_ms;
        }
        for projected in &mut snapshot.devices {
            if let Some(current) = self
                .view
                .snapshot()
                .devices
                .iter()
                .find(|current| current.id == projected.id)
            {
                if devices_semantically_eq_ignoring_last_seen(projected, current) {
                    projected.last_seen_secs = current.last_seen_secs;
                }
            }
        }
        snapshot = UiView::from_snapshot(snapshot)
            .context("authoritative UI projection is invalid")?
            .snapshot()
            .clone();

        let current = self.view.snapshot().clone();
        if current.status != snapshot.status {
            self.emit_reliable(UiChange::Status(snapshot.status.clone()))?;
        }
        if current.capabilities != snapshot.capabilities {
            self.emit_reliable(UiChange::Capabilities(snapshot.capabilities.clone()))?;
        }

        let mut removed = current
            .devices
            .iter()
            .filter(|device| {
                !snapshot
                    .devices
                    .iter()
                    .any(|candidate| candidate.id == device.id)
            })
            .map(|device| device.id)
            .collect::<Vec<_>>();
        removed.sort();
        for device_id in removed {
            self.emit_reliable(UiChange::DeviceRemove(device_id))?;
        }
        let mut upserts = snapshot
            .devices
            .iter()
            .filter(|device| {
                current
                    .devices
                    .iter()
                    .find(|candidate| candidate.id == device.id)
                    != Some(*device)
            })
            .cloned()
            .collect::<Vec<_>>();
        upserts.sort_by_key(|device| device.id);
        for device in upserts {
            self.emit_reliable(UiChange::DeviceUpsert(device))?;
        }

        if current.layout != snapshot.layout {
            self.emit_reliable(UiChange::Topology(snapshot.layout.clone()))?;
        }
        if current.display_inventory != snapshot.display_inventory {
            self.emit_reliable(UiChange::DisplayInventory(
                snapshot.display_inventory.clone(),
            ))?;
        }
        let projected_discrete = InputDiscreteProjection {
            session_epoch: rshare_core::SessionEpoch(0),
            pressed_keys: snapshot.dynamic_state.pressed_keys.clone(),
            pressed_buttons: snapshot.dynamic_state.pressed_mouse_buttons.clone(),
        };
        for change in discrete_changes(self.view.snapshot(), &projected_discrete) {
            self.emit_reliable(UiChange::KeyButton(change))?;
        }
        for change in gamepad_pressed_changes(
            self.view.snapshot(),
            &snapshot.dynamic_state.pressed_gamepad_buttons,
        ) {
            self.emit_reliable(UiChange::KeyButton(change))?;
        }
        if current.dynamic_state.pointer != snapshot.dynamic_state.pointer {
            if let Some(pointer) = snapshot.dynamic_state.pointer.clone() {
                self.emit_realtime(UiChange::Pointer(pointer))?;
            }
        }
        if current.dynamic_state.gamepads != snapshot.dynamic_state.gamepads {
            self.emit_realtime(UiChange::Gamepads(snapshot.dynamic_state.gamepads.clone()))?;
        }
        if self.view.snapshot().dynamic_state.diagnostics != snapshot.dynamic_state.diagnostics {
            self.emit_reliable(UiChange::Diagnostics(
                snapshot.dynamic_state.diagnostics.clone(),
            ))?;
        }
        if current.active_sessions.control != snapshot.active_sessions.control {
            self.emit_reliable(UiChange::Session(snapshot.active_sessions.clone()))?;
        } else {
            let mut removed_sessions = current
                .active_sessions
                .media_sessions
                .iter()
                .filter(|session| {
                    !snapshot
                        .active_sessions
                        .media_sessions
                        .iter()
                        .any(|candidate| candidate.session_id == session.session_id)
                })
                .map(|session| session.session_id)
                .collect::<Vec<_>>();
            removed_sessions.sort();
            for session_id in removed_sessions {
                self.emit_reliable(UiChange::MediaSessionRemove(session_id))?;
            }
            let mut media_upserts = snapshot
                .active_sessions
                .media_sessions
                .iter()
                .filter(|session| {
                    current
                        .active_sessions
                        .media_sessions
                        .iter()
                        .find(|candidate| candidate.session_id == session.session_id)
                        != Some(*session)
                })
                .cloned()
                .collect::<Vec<_>>();
            media_upserts.sort_by_key(|session| session.session_id);
            for session in media_upserts {
                self.emit_reliable(UiChange::MediaSessionUpsert(session))?;
            }
        }
        self.input_cut_generation = input_cut_generation;
        Ok(())
    }

    async fn project_authoritative_snapshot(&mut self) -> Result<(UiSnapshot, u64)> {
        let mut snapshot = self
            .projection
            .as_ref()
            .context("no authoritative UI projection source is configured")?
            .project()
            .await
            .context("failed to rebuild authoritative UI projection")?;
        let mut input_cut_generation = self.input_cut_generation;
        if let Some(authoritative) = &mut self.input_authoritative_rx {
            let authoritative = authoritative.borrow_and_update().clone();
            input_cut_generation = authoritative.generation;
            let pointer = self
                .pointer_rx
                .as_mut()
                .and_then(|pointer_rx| *pointer_rx.borrow_and_update());
            if let Some(gamepads_rx) = &mut self.gamepads_rx {
                let _ = gamepads_rx.borrow_and_update();
            }
            overlay_input_projection(&mut snapshot, authoritative.as_ref(), pointer);
        }
        snapshot.protocol_version = UI_STATE_PROTOCOL_VERSION;
        snapshot.boot_id = self.sequencer.cursor().boot_id;
        snapshot.revision = self.sequencer.revision();
        Ok((snapshot, input_cut_generation))
    }
}

fn pointer_identity_eq(left: Option<&UiPointerState>, right: Option<&UiPointerState>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.x == right.x && left.y == right.y && left.display_id == right.display_id
        }
        _ => false,
    }
}

fn capabilities_semantically_eq(
    left: &CapabilityRegistrySnapshot,
    right: &CapabilityRegistrySnapshot,
) -> bool {
    left.local_device_id == right.local_device_id && left.devices == right.devices
}

fn devices_semantically_eq_ignoring_last_seen(
    left: &DaemonDeviceSnapshot,
    right: &DaemonDeviceSnapshot,
) -> bool {
    left.id == right.id
        && left.name == right.name
        && left.hostname == right.hostname
        && left.addresses == right.addresses
        && left.connected == right.connected
}

fn latency_feedback_semantically_eq(
    left: &LatencyFeedbackSnapshot,
    right: &LatencyFeedbackSnapshot,
) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.generated_at_ms = 0;
    right.generated_at_ms = 0;
    left == right
}

fn gamepad_button_transition_changes(
    snapshot: &UiSnapshot,
    transition: &UiDiscreteInputState,
) -> bool {
    let UiDiscreteInputState::GamepadButton {
        gamepad_id,
        button,
        state,
        ..
    } = transition
    else {
        return true;
    };
    let pressed = snapshot
        .dynamic_state
        .pressed_gamepad_buttons
        .iter()
        .any(|candidate| candidate.gamepad_id == *gamepad_id && candidate.button == *button);
    pressed != (*state == ButtonState::Pressed)
}

fn gamepad_pressed_changes(
    snapshot: &UiSnapshot,
    target: &[rshare_core::UiPressedGamepadButton],
) -> Vec<UiDiscreteInputState> {
    let observed_at_ms = timestamp_ms_now();
    snapshot
        .dynamic_state
        .pressed_gamepad_buttons
        .iter()
        .filter(|button| !target.contains(button))
        .map(|button| UiDiscreteInputState::GamepadButton {
            gamepad_id: button.gamepad_id,
            button: button.button,
            state: ButtonState::Released,
            observed_at_ms,
        })
        .chain(
            target
                .iter()
                .filter(|button| {
                    !snapshot
                        .dynamic_state
                        .pressed_gamepad_buttons
                        .contains(button)
                })
                .map(|button| UiDiscreteInputState::GamepadButton {
                    gamepad_id: button.gamepad_id,
                    button: button.button,
                    state: ButtonState::Pressed,
                    observed_at_ms,
                }),
        )
        .collect()
}

fn overlay_input_projection(
    snapshot: &mut UiSnapshot,
    reliable: &InputReliableUiProjection,
    pointer: Option<InputPointerProjection>,
) {
    snapshot.dynamic_state.pressed_keys = reliable.discrete.pressed_keys.clone();
    snapshot.dynamic_state.pressed_keys.sort_unstable();
    snapshot.dynamic_state.pressed_keys.dedup();
    snapshot.dynamic_state.pressed_mouse_buttons = reliable.discrete.pressed_buttons.clone();
    snapshot
        .dynamic_state
        .pressed_mouse_buttons
        .sort_by_key(|button| button.to_code());
    snapshot.dynamic_state.pressed_mouse_buttons.dedup();
    snapshot.dynamic_state.pressed_gamepad_buttons = reliable.pressed_gamepad_buttons.clone();
    snapshot.active_sessions.control = Some(reliable.session.clone());
    snapshot.dynamic_state.gamepads = reliable.gamepads.clone();
    if let Some(pointer) = pointer {
        snapshot.dynamic_state.pointer = Some(UiPointerState {
            x: pointer.x,
            y: pointer.y,
            display_id: None,
            observed_at_ms: timestamp_ms_now(),
        });
    }
}

fn discrete_changes(
    snapshot: &UiSnapshot,
    projection: &InputDiscreteProjection,
) -> Vec<UiDiscreteInputState> {
    let old_keys = snapshot
        .dynamic_state
        .pressed_keys
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let new_keys = projection
        .pressed_keys
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let mut old_buttons = snapshot.dynamic_state.pressed_mouse_buttons.clone();
    old_buttons.sort_by_key(|button| button.to_code());
    old_buttons.dedup();
    let mut new_buttons = projection.pressed_buttons.clone();
    new_buttons.sort_by_key(|button| button.to_code());
    new_buttons.dedup();
    let observed_at_ms = timestamp_ms_now();
    let mut changes = Vec::new();

    let mut released_keys = old_keys.difference(&new_keys).copied().collect::<Vec<_>>();
    released_keys.sort_unstable();
    changes.extend(
        released_keys
            .into_iter()
            .map(|key_code| UiDiscreteInputState::Key {
                key_code,
                state: KeyState::Released,
                observed_at_ms,
            }),
    );
    let mut pressed_keys = new_keys.difference(&old_keys).copied().collect::<Vec<_>>();
    pressed_keys.sort_unstable();
    changes.extend(
        pressed_keys
            .into_iter()
            .map(|key_code| UiDiscreteInputState::Key {
                key_code,
                state: KeyState::Pressed,
                observed_at_ms,
            }),
    );
    let mut released_buttons = old_buttons
        .iter()
        .filter(|button| !new_buttons.contains(button))
        .copied()
        .collect::<Vec<_>>();
    released_buttons.sort_by_key(|button| button.to_code());
    changes.extend(
        released_buttons
            .into_iter()
            .map(|button| UiDiscreteInputState::MouseButton {
                button,
                state: ButtonState::Released,
                observed_at_ms,
            }),
    );
    let mut pressed_buttons = new_buttons
        .iter()
        .filter(|button| !old_buttons.contains(button))
        .copied()
        .collect::<Vec<_>>();
    pressed_buttons.sort_by_key(|button| button.to_code());
    changes.extend(
        pressed_buttons
            .into_iter()
            .map(|button| UiDiscreteInputState::MouseButton {
                button,
                state: ButtonState::Pressed,
                observed_at_ms,
            }),
    );
    changes
}

fn push_resync_snapshot(
    pending: &mut VecDeque<UiEnvelope>,
    snapshot: &UiSnapshot,
    reason: UiResyncReason,
) {
    pending.push_back(UiEnvelope::ResyncRequired {
        boot_id: snapshot.boot_id,
        current_revision: snapshot.revision,
        reason,
    });
    pending.push_back(UiEnvelope::Snapshot(snapshot.clone()));
}

async fn recv_input_mutation(
    input: &mut Option<mpsc::Receiver<InputUiMutation>>,
) -> Option<InputUiMutation> {
    match input {
        Some(input) => input.recv().await,
        None => pending().await,
    }
}

async fn wait_for_pointer(
    input: &mut Option<watch::Receiver<Option<InputPointerProjection>>>,
) -> bool {
    match input {
        Some(input) => input.changed().await.is_ok(),
        None => pending().await,
    }
}

async fn wait_for_gamepads(input: &mut Option<watch::Receiver<Vec<LocalGamepadState>>>) -> bool {
    match input {
        Some(input) => input.changed().await.is_ok(),
        None => pending().await,
    }
}

async fn wait_for_diagnostics(input: &mut Option<watch::Receiver<ControlMetricSnapshot>>) -> bool {
    match input {
        Some(input) => input.changed().await.is_ok(),
        None => pending().await,
    }
}

async fn wait_for_network(input: &mut Option<watch::Receiver<u64>>) -> bool {
    match input {
        Some(input) => input.changed().await.is_ok(),
        None => pending().await,
    }
}

async fn wait_for_network_reconcile(due: Option<Instant>) {
    match due {
        Some(due) => tokio::time::sleep_until(due).await,
        None => pending().await,
    }
}

async fn wait_for_dirty(input: &Option<Arc<crate::input_state::DirtyProjectionNotifier>>) {
    match input {
        Some(input) => input.notified().await,
        None => pending().await,
    }
}

fn timestamp_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_state::{input_state_channel, InputPointerProjection};
    use futures_util::future::BoxFuture;
    use rshare_core::{
        CapabilityRegistrySnapshot, DaemonDeviceSnapshot, DeviceId, GamepadButton,
        GamepadButtonState, GamepadState, LayoutGraph, LocalDisplayState, ServiceStatusSnapshot,
        UiActiveSessions, UiDynamicState, UiEnvelope, UiPressedGamepadButton, UiResyncReason,
        UiSnapshot, UI_STATE_PROTOCOL_VERSION,
    };
    use std::sync::Arc;

    fn fixture_snapshot() -> UiSnapshot {
        let local_id = DeviceId::from_u128(1);
        UiSnapshot {
            protocol_version: UI_STATE_PROTOCOL_VERSION,
            boot_id: DeviceId::nil(),
            revision: 99,
            status: ServiceStatusSnapshot::new(
                local_id,
                "local".into(),
                "host".into(),
                "127.0.0.1:27435".into(),
                27432,
                42,
            ),
            devices: Vec::new(),
            layout: LayoutGraph::new(local_id),
            capabilities: CapabilityRegistrySnapshot {
                local_device_id: local_id,
                generated_at_ms: 0,
                devices: Vec::new(),
            },
            display_inventory: LocalDisplayState::default(),
            dynamic_state: UiDynamicState::default(),
            active_sessions: UiActiveSessions::default(),
        }
    }

    fn device(index: u128) -> DaemonDeviceSnapshot {
        DaemonDeviceSnapshot {
            id: DeviceId::from_u128(index + 10),
            name: format!("peer-{index}"),
            hostname: format!("host-{index}"),
            addresses: vec!["127.0.0.1:27432".into()],
            connected: true,
            last_seen_secs: Some(0),
        }
    }

    #[derive(Clone)]
    struct FixtureProjection {
        snapshot: UiSnapshot,
    }

    impl UiProjectionSource for FixtureProjection {
        fn project(&self) -> BoxFuture<'_, anyhow::Result<UiSnapshot>> {
            Box::pin(async move { Ok(self.snapshot.clone()) })
        }
    }

    struct FailingProjection;

    impl UiProjectionSource for FailingProjection {
        fn project(&self) -> BoxFuture<'_, anyhow::Result<UiSnapshot>> {
            Box::pin(async move { anyhow::bail!("synthetic projection failure") })
        }
    }

    #[derive(Clone)]
    struct MutableProjection {
        snapshot: Arc<std::sync::RwLock<UiSnapshot>>,
    }

    impl MutableProjection {
        fn replace(&self, snapshot: UiSnapshot) {
            *self.snapshot.write().unwrap() = snapshot;
        }
    }

    impl UiProjectionSource for MutableProjection {
        fn project(&self) -> BoxFuture<'_, anyhow::Result<UiSnapshot>> {
            let snapshot = self.snapshot.read().unwrap().clone();
            Box::pin(async move { Ok(snapshot) })
        }
    }

    #[derive(Clone)]
    struct CountingProjection {
        snapshot: UiSnapshot,
        calls: Arc<AtomicUsize>,
    }

    impl UiProjectionSource for CountingProjection {
        fn project(&self) -> BoxFuture<'_, anyhow::Result<UiSnapshot>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let snapshot = self.snapshot.clone();
            Box::pin(async move { Ok(snapshot) })
        }
    }

    #[derive(Clone)]
    struct BlockingProjection {
        snapshot: UiSnapshot,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    impl UiProjectionSource for BlockingProjection {
        fn project(&self) -> BoxFuture<'_, anyhow::Result<UiSnapshot>> {
            let snapshot = self.snapshot.clone();
            let entered = self.entered.clone();
            let release = self.release.clone();
            Box::pin(async move {
                entered.notify_one();
                release.notified().await;
                Ok(snapshot)
            })
        }
    }

    #[tokio::test]
    async fn initial_snapshot_is_revision_zero_and_new_boot_per_aggregator() {
        let first = StateAggregator::new(fixture_snapshot(), 4);
        let second = StateAggregator::new(fixture_snapshot(), 4);

        let UiEnvelope::Snapshot(first_snapshot) =
            first.subscribe(None).await.unwrap().recv().await.unwrap()
        else {
            panic!("first envelope must be a snapshot");
        };
        let UiEnvelope::Snapshot(second_snapshot) =
            second.subscribe(None).await.unwrap().recv().await.unwrap()
        else {
            panic!("first envelope must be a snapshot");
        };

        assert_eq!(first_snapshot.revision, 0);
        assert_ne!(first_snapshot.boot_id, second_snapshot.boot_id);
    }

    #[test]
    fn fallible_constructor_rejects_invalid_initial_snapshot_and_zero_capacity() {
        let mut invalid = fixture_snapshot();
        invalid.devices = vec![device(1), device(1)];
        assert!(StateAggregator::try_new(invalid, 4).is_err());
        assert!(StateAggregator::try_new(fixture_snapshot(), 0).is_err());
    }

    #[tokio::test]
    async fn reliable_changes_are_contiguous_and_replayable() {
        let aggregator = StateAggregator::new(fixture_snapshot(), 8);
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let UiEnvelope::Snapshot(initial) = subscriber.recv().await.unwrap() else {
            panic!("expected snapshot");
        };

        aggregator
            .publish(StateChange::DeviceUpsert(device(1)))
            .await
            .unwrap();
        aggregator
            .publish(StateChange::DeviceUpsert(device(2)))
            .await
            .unwrap();

        let UiEnvelope::Delta(first) = subscriber.recv().await.unwrap() else {
            panic!("expected first delta");
        };
        let UiEnvelope::Delta(second) = subscriber.recv().await.unwrap() else {
            panic!("expected second delta");
        };
        assert_eq!((first.revision, second.revision), (1, 2));

        let mut resumed = aggregator
            .subscribe(Some(rshare_core::UiCursor::new(
                initial.boot_id,
                first.revision,
            )))
            .await
            .unwrap();
        assert_eq!(resumed.recv().await.unwrap(), UiEnvelope::Delta(second));
    }

    #[tokio::test]
    async fn lagged_subscriber_is_told_to_resync_then_receives_latest_snapshot() {
        let aggregator = StateAggregator::new(fixture_snapshot(), 4);
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let _ = subscriber.recv().await.unwrap();

        for index in 0..20 {
            aggregator
                .publish(StateChange::DeviceUpsert(device(index)))
                .await
                .unwrap();
        }

        assert!(matches!(
            subscriber.recv().await.unwrap(),
            UiEnvelope::ResyncRequired { .. }
        ));
        let UiEnvelope::Snapshot(snapshot) = subscriber.recv().await.unwrap() else {
            panic!("resync must be followed by latest snapshot");
        };
        assert_eq!(snapshot.revision, 20);
    }

    #[tokio::test]
    async fn pointer_flood_uses_latest_slot_without_filling_reliable_history() {
        let (input, feeds) = input_state_channel(4);
        let aggregator = StateAggregator::with_input(fixture_snapshot(), 8, feeds);

        for x in 0..100_000 {
            input.publish_pointer(InputPointerProjection {
                session_epoch: rshare_core::SessionEpoch(1),
                x,
                y: 7,
            });
        }

        assert_eq!(aggregator.reliable_history_len(), 0);
        assert_eq!(aggregator.latest_pointer().unwrap().x, 99_999);
    }

    #[tokio::test]
    async fn pointer_hot_path_does_not_publish_full_large_snapshots() {
        let mut initial = fixture_snapshot();
        initial.devices = (1..=4_096).map(device).collect();
        let (input, feeds) = input_state_channel(4);
        let aggregator = StateAggregator::with_input(initial, 8, feeds);
        assert_eq!(aggregator.full_snapshot_publication_count(), 1);

        for x in 0..100_000 {
            input.publish_pointer(InputPointerProjection {
                session_epoch: rshare_core::SessionEpoch(1),
                x,
                y: 11,
            });
        }

        let snapshot = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            aggregator.wait_for_revision(1),
        )
        .await
        .expect("pointer latest value did not reach the aggregator")
        .unwrap();
        assert_eq!(snapshot.dynamic_state.pointer.as_ref().unwrap().x, 99_999);
        assert_eq!(
            aggregator.full_snapshot_publication_count(),
            1,
            "realtime pointer publication must not clone/publish the full topology"
        );
    }

    #[tokio::test]
    async fn closed_input_watches_are_disabled_without_starving_commands() {
        let (input, feeds) = input_state_channel(4);
        let aggregator = StateAggregator::with_input(fixture_snapshot(), 8, feeds);
        drop(input);

        let mut subscriber = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            aggregator.subscribe(None),
        )
        .await
        .expect("closed input watches must not spin and starve commands")
        .unwrap();
        let _ = subscriber.recv().await.unwrap();
        aggregator
            .publish(StateChange::DeviceUpsert(device(77)))
            .await
            .unwrap();
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(1), subscriber.recv())
                .await
                .expect("closed input watches must not starve reliable changes")
                .unwrap(),
            UiEnvelope::Delta(_)
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn network_send_flood_is_coalesced_without_starving_pointer_updates() {
        let initial = fixture_snapshot();
        let calls = Arc::new(AtomicUsize::new(0));
        let source = Arc::new(CountingProjection {
            snapshot: initial.clone(),
            calls: calls.clone(),
        });
        let (input, feeds) = input_state_channel(4);
        let (_diagnostics_tx, diagnostics_rx) = watch::channel(ControlMetricSnapshot {
            captured: 0,
            routed: 0,
            realtime_replaced: 0,
            realtime_dropped: 0,
            reliable_overflow: 0,
        });
        let (network_tx, network_rx) = watch::channel(0);
        let aggregator = StateAggregator::try_with_projection_and_diagnostics(
            initial,
            8,
            feeds,
            source,
            diagnostics_rx,
            network_rx,
        )
        .unwrap();
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let _ = subscriber.recv().await.unwrap();

        for revision in 1..=100_000 {
            network_tx.send_replace(revision);
        }
        input.publish_pointer(InputPointerProjection {
            session_epoch: rshare_core::SessionEpoch(1),
            x: 71,
            y: 29,
        });

        let UiEnvelope::Delta(pointer) =
            tokio::time::timeout(Duration::from_millis(1), subscriber.recv())
                .await
                .expect("network activity must not starve the pointer lane")
                .unwrap()
        else {
            panic!("pointer update must remain the first live delta");
        };
        assert!(matches!(
            pointer.change,
            UiChange::Pointer(UiPointerState { x: 71, y: 29, .. })
        ));
        assert_eq!(
            calls.load(Ordering::Relaxed),
            0,
            "raw network activity must be debounced before full projection"
        );

        tokio::time::advance(NETWORK_RECONCILE_DEBOUNCE).await;
        tokio::task::yield_now().await;
        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "one hundred thousand transport notifications must coalesce into one projection"
        );
    }

    #[tokio::test]
    async fn gamepad_flood_uses_input_latest_slot_without_filling_reliable_history() {
        let (input, feeds) = input_state_channel(1);
        let aggregator = StateAggregator::with_input(fixture_snapshot(), 4, feeds);
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let _ = subscriber.recv().await.unwrap();

        for sequence in 1..=100_000_u64 {
            let mut state = GamepadState::neutral(3, sequence, sequence);
            state.left_stick_x = (sequence % i16::MAX as u64) as i16;
            let mut gamepad =
                LocalGamepadState::from_state(&state, Some("Latest Pad".into()), true);
            gamepad.event_count = sequence;
            input.publish_gamepads(vec![gamepad]);
        }

        let snapshot = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            aggregator.wait_for_revision(1),
        )
        .await
        .expect("latest gamepad state did not reach the aggregator")
        .unwrap();
        assert_eq!(snapshot.dynamic_state.gamepads.len(), 1);
        assert_eq!(snapshot.dynamic_state.gamepads[0].event_count, 100_000);
        assert_eq!(snapshot.dynamic_state.gamepads[0].name, "Latest Pad");
        let UiEnvelope::Delta(streamed) = subscriber.recv().await.unwrap() else {
            panic!("gamepad latest slot must emit a typed live delta");
        };
        let UiChange::Gamepads(streamed) = streamed.change else {
            panic!("gamepad latest slot must emit the gamepad projection");
        };
        assert_eq!(streamed[0].event_count, 100_000);
        assert_eq!(aggregator.reliable_history_len(), 0);
    }

    #[tokio::test]
    async fn gamepad_button_truth_survives_axes_updates_and_release_is_reliable() {
        let (input, feeds) = input_state_channel(4);
        let aggregator = StateAggregator::with_input(fixture_snapshot(), 8, feeds);

        let mut pressed = GamepadState::neutral(3, 1, 10);
        pressed.buttons.push(GamepadButtonState {
            button: GamepadButton::South,
            pressed: true,
        });
        input.publish_gamepads(vec![LocalGamepadState::from_state(
            &pressed,
            Some("Pad".into()),
            true,
        )]);
        let snapshot = aggregator.wait_for_revision(2).await.unwrap();
        assert_eq!(
            snapshot.dynamic_state.pressed_gamepad_buttons,
            vec![UiPressedGamepadButton {
                gamepad_id: 3,
                button: GamepadButton::South,
            }]
        );
        assert_eq!(aggregator.reliable_history_len(), 1);

        let mut axes = pressed.clone();
        axes.sequence = 2;
        axes.timestamp_ms = 20;
        axes.left_stick_x = 123;
        input.publish_gamepads(vec![LocalGamepadState::from_state(
            &axes,
            Some("Pad".into()),
            true,
        )]);
        let snapshot = aggregator.wait_for_revision(3).await.unwrap();
        assert_eq!(
            snapshot.dynamic_state.pressed_gamepad_buttons,
            vec![UiPressedGamepadButton {
                gamepad_id: 3,
                button: GamepadButton::South,
            }]
        );
        assert_eq!(aggregator.reliable_history_len(), 1);

        let mut released = axes;
        released.sequence = 3;
        released.timestamp_ms = 30;
        released.buttons[0].pressed = false;
        input.publish_gamepads(vec![LocalGamepadState::from_state(
            &released,
            Some("Pad".into()),
            true,
        )]);
        let snapshot = aggregator.wait_for_revision(5).await.unwrap();
        assert!(snapshot.dynamic_state.pressed_gamepad_buttons.is_empty());
        assert_eq!(aggregator.reliable_history_len(), 2);
    }

    #[tokio::test]
    async fn dirty_rebuild_snapshot_contains_authoritative_pressed_gamepad_truth() {
        let (input, feeds) = input_state_channel(1);
        let initial = fixture_snapshot();
        let aggregator = StateAggregator::with_projection(
            initial.clone(),
            8,
            feeds,
            Arc::new(FixtureProjection { snapshot: initial }),
        );
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let _ = subscriber.recv().await.unwrap();

        let mut first = GamepadState::neutral(4, 1, 10);
        first.buttons.push(GamepadButtonState {
            button: GamepadButton::South,
            pressed: true,
        });
        input.publish_gamepads(vec![LocalGamepadState::from_state(
            &first,
            Some("Pad".into()),
            true,
        )]);
        let mut second = first;
        second.sequence = 2;
        second.buttons.push(GamepadButtonState {
            button: GamepadButton::East,
            pressed: true,
        });
        input.publish_gamepads(vec![LocalGamepadState::from_state(
            &second,
            Some("Pad".into()),
            true,
        )]);

        assert!(matches!(
            subscriber.recv().await.unwrap(),
            UiEnvelope::ResyncRequired {
                reason: UiResyncReason::ProjectionRebuilt,
                ..
            }
        ));
        let UiEnvelope::Snapshot(snapshot) = subscriber.recv().await.unwrap() else {
            panic!("dirty gamepad lane must publish a replacement snapshot");
        };
        assert_eq!(
            snapshot.dynamic_state.pressed_gamepad_buttons,
            vec![
                UiPressedGamepadButton {
                    gamepad_id: 4,
                    button: GamepadButton::South,
                },
                UiPressedGamepadButton {
                    gamepad_id: 4,
                    button: GamepadButton::East,
                },
            ]
        );
        assert_eq!(snapshot.dynamic_state.gamepads[0].buttons.len(), 2);
    }

    #[tokio::test]
    async fn offline_pre_rebuild_cursor_gets_replacement_boot_and_new_revision_one() {
        let (input, feeds) = input_state_channel(1);
        let initial = fixture_snapshot();
        let projection = MutableProjection {
            snapshot: Arc::new(std::sync::RwLock::new(initial.clone())),
        };
        let aggregator = StateAggregator::with_projection(
            initial.clone(),
            8,
            feeds,
            Arc::new(projection.clone()),
        );
        let mut original = aggregator.subscribe(None).await.unwrap();
        let _ = original.recv().await.unwrap();
        aggregator
            .publish(StateChange::DeviceUpsert(device(55)))
            .await
            .unwrap();
        let UiEnvelope::Delta(before_rebuild) = original.recv().await.unwrap() else {
            panic!("pre-rebuild reliable mutation must emit a delta");
        };
        let old_cursor = UiCursor::new(before_rebuild.boot_id, before_rebuild.revision);
        assert_eq!(old_cursor.revision, 1);
        drop(original);
        let mut authoritative_base = initial;
        authoritative_base.devices.push(device(55));
        projection.replace(authoritative_base);

        let target = DeviceId::from_u128(88);
        input.publish_discrete(InputDiscreteProjection {
            session_epoch: rshare_core::SessionEpoch(2),
            pressed_keys: vec![0x41],
            pressed_buttons: Vec::new(),
        });
        input.publish_session(rshare_core::ControlSessionState::RemoteActive {
            target,
            entered_via: rshare_core::Direction::Right,
        });
        let mut gamepad_state = GamepadState::neutral(9, 1, 50);
        gamepad_state.buttons.push(GamepadButtonState {
            button: GamepadButton::South,
            pressed: true,
        });
        input.publish_gamepads(vec![LocalGamepadState::from_state(
            &gamepad_state,
            Some("Recovered Pad".into()),
            true,
        )]);

        let mut resumed = aggregator.subscribe(Some(old_cursor)).await.unwrap();
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(1), resumed.recv())
                .await
                .expect("old cursor must not wait silently after authoritative replacement")
                .unwrap(),
            UiEnvelope::ResyncRequired {
                reason: UiResyncReason::BootChanged,
                ..
            }
        ));
        let UiEnvelope::Snapshot(replacement) = resumed.recv().await.unwrap() else {
            panic!("old cursor must receive the authoritative replacement snapshot");
        };
        assert_ne!(replacement.boot_id, old_cursor.boot_id);
        assert_eq!(replacement.revision, 0);
        assert!(replacement
            .devices
            .iter()
            .any(|candidate| candidate.id == device(55).id));
        assert_eq!(replacement.dynamic_state.pressed_keys, vec![0x41]);
        assert_eq!(
            replacement.active_sessions.control,
            Some(rshare_core::ControlSessionState::RemoteActive {
                target,
                entered_via: rshare_core::Direction::Right,
            })
        );
        assert_eq!(
            replacement.dynamic_state.pressed_gamepad_buttons,
            vec![UiPressedGamepadButton {
                gamepad_id: 9,
                button: GamepadButton::South,
            }]
        );

        let mut status = replacement.status.clone();
        status.device_name = "post-rebuild".into();
        aggregator
            .publish(StateChange::Status(status))
            .await
            .unwrap();
        let UiEnvelope::Delta(first_after_rebuild) = resumed.recv().await.unwrap() else {
            panic!("first post-rebuild mutation must be a delta");
        };
        assert_eq!(first_after_rebuild.boot_id, replacement.boot_id);
        assert_eq!(first_after_rebuild.revision, 1);
    }

    #[tokio::test]
    async fn rebuild_marks_latest_watches_without_emitting_false_realtime_delta() {
        let (input, feeds) = input_state_channel(1);
        let initial = fixture_snapshot();
        let aggregator = StateAggregator::with_projection(
            initial.clone(),
            8,
            feeds,
            Arc::new(FixtureProjection { snapshot: initial }),
        );
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let _ = subscriber.recv().await.unwrap();

        input.publish_pointer(InputPointerProjection {
            session_epoch: rshare_core::SessionEpoch(1),
            x: 10,
            y: 20,
        });
        let mut first = GamepadState::neutral(5, 1, 10);
        first.buttons.push(GamepadButtonState {
            button: GamepadButton::South,
            pressed: true,
        });
        input.publish_gamepads(vec![LocalGamepadState::from_state(
            &first,
            Some("Pad".into()),
            true,
        )]);
        let mut second = first;
        second.sequence = 2;
        second.buttons.push(GamepadButtonState {
            button: GamepadButton::East,
            pressed: true,
        });
        input.publish_gamepads(vec![LocalGamepadState::from_state(
            &second,
            Some("Pad".into()),
            true,
        )]);

        assert!(matches!(
            subscriber.recv().await.unwrap(),
            UiEnvelope::ResyncRequired {
                reason: UiResyncReason::ProjectionRebuilt,
                ..
            }
        ));
        let UiEnvelope::Snapshot(replacement) = subscriber.recv().await.unwrap() else {
            panic!("dirty rebuild must publish a replacement snapshot");
        };
        assert_eq!(replacement.dynamic_state.pointer.as_ref().unwrap().x, 10);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), subscriber.recv())
                .await
                .is_err(),
            "consumed pointer/gamepad watches must not emit a duplicate delta"
        );

        input.publish_pointer(InputPointerProjection {
            session_epoch: rshare_core::SessionEpoch(1),
            x: 11,
            y: 21,
        });
        let UiEnvelope::Delta(pointer) = subscriber.recv().await.unwrap() else {
            panic!("new pointer update after rebuild must remain visible");
        };
        assert!(matches!(
            pointer.change,
            UiChange::Pointer(UiPointerState { x: 11, y: 21, .. })
        ));

        let mut axes = second;
        axes.sequence = 3;
        axes.left_stick_x = 123;
        input.publish_gamepads(vec![LocalGamepadState::from_state(
            &axes,
            Some("Pad".into()),
            true,
        )]);
        let UiEnvelope::Delta(gamepads) = subscriber.recv().await.unwrap() else {
            panic!("new axes update after rebuild must remain visible");
        };
        assert!(matches!(gamepads.change, UiChange::Gamepads(_)));
    }

    #[tokio::test]
    async fn initial_overlay_consumes_existing_watches_and_queued_idempotent_mutations() {
        let (input, feeds) = input_state_channel(8);
        let target = DeviceId::from_u128(66);
        input.publish_discrete(InputDiscreteProjection {
            session_epoch: rshare_core::SessionEpoch(1),
            pressed_keys: vec![0x41],
            pressed_buttons: Vec::new(),
        });
        input.publish_discrete(InputDiscreteProjection {
            session_epoch: rshare_core::SessionEpoch(2),
            pressed_keys: Vec::new(),
            pressed_buttons: Vec::new(),
        });
        input.publish_session(rshare_core::ControlSessionState::LocalReady);
        input.publish_session(rshare_core::ControlSessionState::RemoteActive {
            target,
            entered_via: rshare_core::Direction::Left,
        });
        input.publish_pointer(InputPointerProjection {
            session_epoch: rshare_core::SessionEpoch(3),
            x: -5,
            y: 7,
        });
        let mut pressed = GamepadState::neutral(6, 1, 70);
        pressed.buttons.push(GamepadButtonState {
            button: GamepadButton::South,
            pressed: true,
        });
        input.publish_gamepads(vec![LocalGamepadState::from_state(
            &pressed,
            Some("Preloaded Pad".into()),
            true,
        )]);

        let aggregator = StateAggregator::with_input(fixture_snapshot(), 8, feeds);
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let UiEnvelope::Snapshot(initial) = subscriber.recv().await.unwrap() else {
            panic!("initial subscription must receive a snapshot");
        };
        assert_eq!(initial.revision, 0);
        assert!(initial.dynamic_state.pressed_keys.is_empty());
        assert_eq!(initial.dynamic_state.pointer.as_ref().unwrap().x, -5);
        assert_eq!(
            initial.active_sessions.control,
            Some(rshare_core::ControlSessionState::RemoteActive {
                target,
                entered_via: rshare_core::Direction::Left,
            })
        );
        assert_eq!(
            initial.status.session_state, initial.active_sessions.control,
            "initial overlay must expose one control-session truth"
        );
        assert_eq!(initial.status.active_target, Some(target));
        assert_eq!(
            initial.dynamic_state.pressed_gamepad_buttons,
            vec![UiPressedGamepadButton {
                gamepad_id: 6,
                button: GamepadButton::South,
            }]
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), subscriber.recv())
                .await
                .is_err(),
            "preloaded watches and queued idempotent mutations must not emit duplicate deltas"
        );

        input.publish_pointer(InputPointerProjection {
            session_epoch: rshare_core::SessionEpoch(3),
            x: -4,
            y: 8,
        });
        let UiEnvelope::Delta(first_live) = subscriber.recv().await.unwrap() else {
            panic!("new update after initial overlay must remain visible");
        };
        assert_eq!(first_live.revision, 1);
        assert!(matches!(
            first_live.change,
            UiChange::Pointer(UiPointerState { x: -4, y: 8, .. })
        ));
    }

    #[tokio::test]
    async fn live_session_mutation_updates_compatibility_status_atomically() {
        let (input, feeds) = input_state_channel(4);
        let aggregator = StateAggregator::with_input(fixture_snapshot(), 8, feeds);
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let _ = subscriber.recv().await.unwrap();
        let target = DeviceId::from_u128(91);

        input.publish_session(rshare_core::ControlSessionState::RemoteActive {
            target,
            entered_via: rshare_core::Direction::Right,
        });
        let UiEnvelope::Delta(delta) = subscriber.recv().await.unwrap() else {
            panic!("session publication must emit one typed delta");
        };
        assert!(matches!(delta.change, UiChange::Session(_)));

        let snapshot = aggregator.wait_for_revision(delta.revision).await.unwrap();
        assert_eq!(
            snapshot.active_sessions.control,
            Some(rshare_core::ControlSessionState::RemoteActive {
                target,
                entered_via: rshare_core::Direction::Right,
            })
        );
        assert_eq!(
            snapshot.status.session_state,
            snapshot.active_sessions.control
        );
        assert_eq!(snapshot.status.active_target, Some(target));
    }

    #[tokio::test]
    async fn heartbeat_repeats_current_cursor_without_consuming_revision_or_history() {
        let aggregator = StateAggregator::new(fixture_snapshot(), 8);
        aggregator
            .publish(StateChange::DeviceUpsert(device(1)))
            .await
            .unwrap();
        let snapshot = aggregator.wait_for_revision(1).await.unwrap();
        let history_len = aggregator.reliable_history_len();

        assert_eq!(
            aggregator.heartbeat(123),
            UiEnvelope::Heartbeat {
                boot_id: snapshot.boot_id,
                revision: 1,
                sent_at_ms: 123,
            }
        );
        assert_eq!(aggregator.latest_snapshot().revision, 1);
        assert_eq!(aggregator.reliable_history_len(), history_len);
    }

    #[tokio::test]
    async fn saturated_input_lane_rebuilds_from_authoritative_source_without_false_delta() {
        let mut rebuilt = fixture_snapshot();
        rebuilt.status.device_name = "rebuilt-from-source".into();
        let source = Arc::new(FixtureProjection { snapshot: rebuilt });
        let (input, feeds) = input_state_channel(1);
        let aggregator = StateAggregator::with_projection(fixture_snapshot(), 8, feeds, source);
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let _ = subscriber.recv().await.unwrap();

        input.publish_discrete(crate::input_state::InputDiscreteProjection {
            session_epoch: rshare_core::SessionEpoch(1),
            pressed_keys: vec![1],
            pressed_buttons: Vec::new(),
        });
        // The bounded lane is still full in this synchronous burst. This mutation
        // exists only in the Task-12 authoritative watch projection.
        input.publish_discrete(crate::input_state::InputDiscreteProjection {
            session_epoch: rshare_core::SessionEpoch(1),
            pressed_keys: vec![2],
            pressed_buttons: Vec::new(),
        });

        assert!(matches!(
            subscriber.recv().await.unwrap(),
            UiEnvelope::ResyncRequired {
                reason: UiResyncReason::ProjectionRebuilt,
                ..
            }
        ));
        let UiEnvelope::Snapshot(snapshot) = subscriber.recv().await.unwrap() else {
            panic!("projection rebuild must publish a replacement snapshot");
        };
        assert_eq!(snapshot.status.device_name, "rebuilt-from-source");
        assert_eq!(snapshot.dynamic_state.pressed_keys, vec![2]);
        assert_eq!(snapshot.revision, 0);
        assert_eq!(aggregator.reliable_history_len(), 0);
    }

    #[tokio::test]
    async fn rebuild_cut_skips_mutations_already_covered_during_projection_wait() {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let (input, feeds) = input_state_channel(1);
        let aggregator = StateAggregator::with_projection(
            fixture_snapshot(),
            8,
            feeds,
            Arc::new(BlockingProjection {
                snapshot: fixture_snapshot(),
                entered: entered.clone(),
                release: release.clone(),
            }),
        );
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let _ = subscriber.recv().await.unwrap();

        input.publish_discrete(InputDiscreteProjection {
            session_epoch: rshare_core::SessionEpoch(1),
            pressed_keys: vec![1],
            pressed_buttons: Vec::new(),
        });
        input.publish_discrete(InputDiscreteProjection {
            session_epoch: rshare_core::SessionEpoch(1),
            pressed_keys: vec![2],
            pressed_buttons: Vec::new(),
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
            .await
            .expect("dirty rebuild did not enter the blocking projection");

        input.publish_discrete(InputDiscreteProjection {
            session_epoch: rshare_core::SessionEpoch(2),
            pressed_keys: vec![3],
            pressed_buttons: Vec::new(),
        });
        input.publish_discrete(InputDiscreteProjection {
            session_epoch: rshare_core::SessionEpoch(2),
            pressed_keys: Vec::new(),
            pressed_buttons: Vec::new(),
        });
        let target = DeviceId::from_u128(92);
        input.publish_session(rshare_core::ControlSessionState::RemoteActive {
            target,
            entered_via: rshare_core::Direction::Left,
        });
        release.notify_one();

        assert!(matches!(
            subscriber.recv().await.unwrap(),
            UiEnvelope::ResyncRequired {
                reason: UiResyncReason::ProjectionRebuilt,
                ..
            }
        ));
        let UiEnvelope::Snapshot(snapshot) = subscriber.recv().await.unwrap() else {
            panic!("rebuild must publish the generation-cut snapshot");
        };
        assert!(snapshot.dynamic_state.pressed_keys.is_empty());
        assert_eq!(
            snapshot.status.session_state,
            snapshot.active_sessions.control
        );
        assert_eq!(snapshot.status.active_target, Some(target));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), subscriber.recv())
                .await
                .is_err(),
            "mutations at or before the projection cut must not be replayed as false deltas"
        );
    }

    #[tokio::test]
    async fn reconcile_cut_emits_reliable_input_truth_before_skipping_covered_mutation() {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let (input, feeds) = input_state_channel(4);
        let aggregator = StateAggregator::with_projection(
            fixture_snapshot(),
            8,
            feeds,
            Arc::new(BlockingProjection {
                snapshot: fixture_snapshot(),
                entered: entered.clone(),
                release: release.clone(),
            }),
        );
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let _ = subscriber.recv().await.unwrap();
        let reconcile = {
            let aggregator = aggregator.clone();
            tokio::spawn(async move { aggregator.reconcile_from_projection().await })
        };
        tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
            .await
            .expect("reconcile did not enter the blocking projection");

        input.publish_discrete(InputDiscreteProjection {
            session_epoch: rshare_core::SessionEpoch(7),
            pressed_keys: vec![0x41],
            pressed_buttons: Vec::new(),
        });
        release.notify_one();
        reconcile.await.unwrap().unwrap();

        let UiEnvelope::Delta(delta) = subscriber.recv().await.unwrap() else {
            panic!("reconcile must emit covered reliable input truth");
        };
        assert!(matches!(
            delta.change,
            UiChange::KeyButton(UiDiscreteInputState::Key {
                key_code: 0x41,
                state: KeyState::Pressed,
                ..
            })
        ));
        assert_eq!(
            aggregator.latest_snapshot().dynamic_state.pressed_keys,
            vec![0x41]
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), subscriber.recv())
                .await
                .is_err(),
            "the queued mutation at the reconcile cut must not emit a duplicate delta"
        );
    }

    #[tokio::test]
    async fn expired_and_different_boot_cursors_resync_immediately() {
        let aggregator = StateAggregator::new(fixture_snapshot(), 8);
        let initial = aggregator.latest_snapshot();

        for index in 0..=DEFAULT_REPLAY_CAPACITY {
            aggregator
                .publish(StateChange::DeviceUpsert(device(index as u128)))
                .await
                .unwrap();
        }
        let latest = aggregator
            .wait_for_revision(DEFAULT_REPLAY_CAPACITY as u64 + 1)
            .await
            .unwrap();
        assert_eq!(latest.revision, DEFAULT_REPLAY_CAPACITY as u64 + 1);

        let mut expired = aggregator
            .subscribe(Some(rshare_core::UiCursor::new(initial.boot_id, 0)))
            .await
            .unwrap();
        assert!(matches!(
            expired.recv().await.unwrap(),
            UiEnvelope::ResyncRequired {
                reason: UiResyncReason::HistoryExpired,
                ..
            }
        ));
        assert!(matches!(
            expired.recv().await.unwrap(),
            UiEnvelope::Snapshot(_)
        ));

        let mut restarted = aggregator
            .subscribe(Some(rshare_core::UiCursor::new(DeviceId::new_v4(), 0)))
            .await
            .unwrap();
        assert!(matches!(
            restarted.recv().await.unwrap(),
            UiEnvelope::ResyncRequired {
                reason: UiResyncReason::BootChanged,
                ..
            }
        ));
        assert!(matches!(
            restarted.recv().await.unwrap(),
            UiEnvelope::Snapshot(_)
        ));
    }

    #[tokio::test]
    async fn lag_resync_moves_live_receiver_to_tail_before_next_delta() {
        let aggregator = StateAggregator::new(fixture_snapshot(), 2);
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let _ = subscriber.recv().await.unwrap();
        for index in 0..5 {
            aggregator
                .publish(StateChange::DeviceUpsert(device(index)))
                .await
                .unwrap();
        }

        assert!(matches!(
            subscriber.recv().await.unwrap(),
            UiEnvelope::ResyncRequired { .. }
        ));
        let UiEnvelope::Snapshot(snapshot) = subscriber.recv().await.unwrap() else {
            panic!("lag recovery requires snapshot");
        };
        aggregator
            .publish(StateChange::DeviceUpsert(device(99)))
            .await
            .unwrap();
        let UiEnvelope::Delta(next) = subscriber.recv().await.unwrap() else {
            panic!("tail must deliver the next live delta");
        };
        assert_eq!(next.revision, snapshot.revision + 1);
    }

    #[tokio::test]
    async fn invalid_change_without_authoritative_projection_fails_closed() {
        let aggregator = StateAggregator::new(fixture_snapshot(), 8);
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let _ = subscriber.recv().await.unwrap();
        let session = rshare_core::UiMediaSession {
            session_id: DeviceId::from_u128(700),
            peer_id: DeviceId::from_u128(701),
            display_id: Some("display-1".into()),
            state: rshare_core::UiMediaSessionState::Starting,
        };
        aggregator
            .publish(StateChange::Session(UiActiveSessions {
                control: None,
                media_sessions: vec![session.clone(), session],
            }))
            .await
            .unwrap();

        let closed = tokio::time::timeout(std::time::Duration::from_secs(1), subscriber.recv())
            .await
            .expect("invalid projection must close the stream promptly");
        assert!(
            closed.is_err(),
            "an aggregator without an authoritative source must not rebuild from its old snapshot"
        );
        assert_eq!(aggregator.latest_snapshot().revision, 0);
        assert!(
            aggregator
                .publish(StateChange::DeviceUpsert(device(8)))
                .await
                .is_err(),
            "the failed actor must reject later mutations"
        );
    }

    #[tokio::test]
    async fn projection_failure_closes_stream_instead_of_continuing_after_lost_truth() {
        let (input, feeds) = input_state_channel(1);
        let aggregator = StateAggregator::with_projection(
            fixture_snapshot(),
            8,
            feeds,
            Arc::new(FailingProjection),
        );
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let _ = subscriber.recv().await.unwrap();
        input.publish_discrete(crate::input_state::InputDiscreteProjection {
            session_epoch: rshare_core::SessionEpoch(1),
            pressed_keys: vec![1],
            pressed_buttons: Vec::new(),
        });
        input.publish_discrete(crate::input_state::InputDiscreteProjection {
            session_epoch: rshare_core::SessionEpoch(1),
            pressed_keys: vec![2],
            pressed_buttons: Vec::new(),
        });

        let closed = tokio::time::timeout(std::time::Duration::from_secs(1), subscriber.recv())
            .await
            .expect("projection failure must not leave subscribers hanging");
        assert!(
            closed.is_err(),
            "the actor must close subscribers when authoritative truth cannot be rebuilt"
        );
        assert!(
            aggregator.subscribe(None).await.is_err(),
            "new subscriptions must receive an error after fail-close, not panic"
        );
    }

    #[tokio::test]
    async fn production_reconcile_emits_typed_contiguous_diffs_without_timestamp_pointer_noise() {
        let mut initial = fixture_snapshot();
        initial.dynamic_state.pointer = Some(rshare_core::UiPointerState {
            x: 0,
            y: 0,
            display_id: None,
            observed_at_ms: 1,
        });
        let source = MutableProjection {
            snapshot: Arc::new(std::sync::RwLock::new(initial.clone())),
        };
        let (_input, feeds) = input_state_channel(4);
        let aggregator =
            StateAggregator::with_projection(initial.clone(), 16, feeds, Arc::new(source.clone()));
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let _ = subscriber.recv().await.unwrap();

        let mut changed = initial;
        changed.status.device_name = "renamed".into();
        changed.devices.push(device(33));
        changed.dynamic_state.pointer = Some(rshare_core::UiPointerState {
            x: 0,
            y: 0,
            display_id: None,
            observed_at_ms: 999_999,
        });
        source.replace(changed);
        aggregator.reconcile_from_projection().await.unwrap();

        let UiEnvelope::Delta(status) = subscriber.recv().await.unwrap() else {
            panic!("status diff must be typed");
        };
        let UiEnvelope::Delta(device) = subscriber.recv().await.unwrap() else {
            panic!("device diff must be typed");
        };
        assert!(matches!(status.change, rshare_core::UiChange::Status(_)));
        assert!(matches!(
            device.change,
            rshare_core::UiChange::DeviceUpsert(_)
        ));
        assert_eq!((status.revision, device.revision), (1, 2));
        assert_eq!(aggregator.latest_snapshot().revision, 2);
    }

    #[tokio::test]
    async fn volatile_projection_timestamps_do_not_advance_revision_or_history() {
        let mut initial = fixture_snapshot();
        initial.devices.push(device(1));
        let source = MutableProjection {
            snapshot: Arc::new(std::sync::RwLock::new(initial.clone())),
        };
        let (_input, feeds) = input_state_channel(4);
        let aggregator =
            StateAggregator::with_projection(initial.clone(), 8, feeds, Arc::new(source.clone()));
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let _ = subscriber.recv().await.unwrap();

        let mut generated_later = initial;
        generated_later.capabilities.generated_at_ms = 999_999;
        generated_later.devices[0].last_seen_secs = Some(42);
        generated_later.status.latency_feedback.generated_at_ms = 999_999;
        generated_later.dynamic_state.diagnostics.generated_at_ms = 999_999;
        source.replace(generated_later);
        aggregator.reconcile_from_projection().await.unwrap();

        assert_eq!(aggregator.latest_snapshot().revision, 0);
        assert_eq!(aggregator.reliable_history_len(), 0);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), subscriber.recv())
                .await
                .is_err(),
            "volatile generated-at/last-seen fields must not create no-op deltas"
        );
    }

    #[tokio::test]
    async fn lag_recovery_never_returns_delta_at_or_before_replacement_snapshot() {
        for round in 0..16_u128 {
            let aggregator = StateAggregator::new(fixture_snapshot(), 4);
            let mut subscriber = aggregator.subscribe(None).await.unwrap();
            let _ = subscriber.recv().await.unwrap();
            for index in 0..64_u128 {
                aggregator
                    .publish(StateChange::DeviceUpsert(device(round * 1000 + index)))
                    .await
                    .unwrap();
            }
            assert!(matches!(
                subscriber.recv().await.unwrap(),
                UiEnvelope::ResyncRequired { .. }
            ));
            let UiEnvelope::Snapshot(snapshot) = subscriber.recv().await.unwrap() else {
                panic!("lag recovery requires snapshot");
            };

            let producer = {
                let aggregator = aggregator.clone();
                tokio::spawn(async move {
                    for index in 64..128_u128 {
                        aggregator
                            .publish(StateChange::DeviceUpsert(device(round * 1000 + index)))
                            .await
                            .unwrap();
                        tokio::task::yield_now().await;
                    }
                })
            };
            let next = subscriber.recv().await.unwrap();
            match next {
                UiEnvelope::Delta(delta) => {
                    assert!(delta.revision > snapshot.revision);
                }
                UiEnvelope::ResyncRequired { .. } => {
                    let UiEnvelope::Snapshot(next_snapshot) = subscriber.recv().await.unwrap()
                    else {
                        panic!("repeated lag must still yield replacement snapshot");
                    };
                    assert!(next_snapshot.revision >= snapshot.revision);
                }
                other => panic!("unexpected envelope after lag recovery: {other:?}"),
            }
            producer.await.unwrap();
        }
    }
}
