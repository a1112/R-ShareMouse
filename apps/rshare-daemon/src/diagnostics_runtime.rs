use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use rshare_core::{ControlConnectionId, DeviceId, LocalInputDiagnosticEvent};
use tokio::sync::{mpsc, watch};

use crate::input_state::{ControlMetricSnapshot, ControlMetrics};

pub const DIAGNOSTICS_SAMPLE_PERIOD: Duration = Duration::from_millis(50);
pub const DIAGNOSTICS_HISTORY_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiagnosticSubscriberId {
    pub peer_id: DeviceId,
    pub control_connection_id: ControlConnectionId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticPayload {
    Metrics(ControlMetricSnapshot),
    Discrete(LocalInputDiagnosticEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticPublicationItem {
    pub payload: DiagnosticPayload,
    pub json: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticPublication {
    pub sequence: u64,
    pub items: Arc<[DiagnosticPublicationItem]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticHistoryEntry {
    Metrics(ControlMetricSnapshot),
    Discrete(LocalInputDiagnosticEvent),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiagnosticsRuntimeStats {
    pub samples: u64,
    pub formatted: u64,
    /// Number of subscriber watch slots replaced with a sampled batch.
    ///
    /// This is a local publication count, not confirmation of transport
    /// delivery to a remote endpoint.
    pub publications: u64,
}

#[derive(Default)]
struct RuntimeCounters {
    samples: AtomicU64,
    formatted: AtomicU64,
    publications: AtomicU64,
}

impl RuntimeCounters {
    fn snapshot(&self) -> DiagnosticsRuntimeStats {
        DiagnosticsRuntimeStats {
            samples: self.samples.load(Ordering::Relaxed),
            formatted: self.formatted.load(Ordering::Relaxed),
            publications: self.publications.load(Ordering::Relaxed),
        }
    }
}

struct SubscriptionEntry {
    token: u64,
    tx: watch::Sender<Option<Arc<DiagnosticPublication>>>,
}

struct PeerSubscriptionState {
    generation: ControlConnectionId,
    subscription: Option<SubscriptionEntry>,
}

/// Explicit, generation-scoped diagnostics subscribers.
///
/// The registry has one replaceable slot per peer. A stale subscription can
/// only remove itself when its connection generation is still current.
#[derive(Default)]
pub struct SubscriptionRegistry {
    peers: Mutex<HashMap<DeviceId, PeerSubscriptionState>>,
    next_token: AtomicU64,
}

impl SubscriptionRegistry {
    fn activate_generation(&self, id: DiagnosticSubscriberId) {
        let mut peers = self
            .peers
            .lock()
            .expect("diagnostics subscription registry poisoned");
        match peers.get_mut(&id.peer_id) {
            Some(peer) if peer.generation == id.control_connection_id => {}
            Some(peer) => {
                *peer = PeerSubscriptionState {
                    generation: id.control_connection_id,
                    subscription: None,
                };
            }
            None => {
                peers.insert(
                    id.peer_id,
                    PeerSubscriptionState {
                        generation: id.control_connection_id,
                        subscription: None,
                    },
                );
            }
        }
    }

    fn subscribe_current(
        self: &Arc<Self>,
        id: DiagnosticSubscriberId,
    ) -> Option<DiagnosticsSubscription> {
        let mut peers = self
            .peers
            .lock()
            .expect("diagnostics subscription registry poisoned");
        let peer = peers.get_mut(&id.peer_id)?;
        if peer.generation != id.control_connection_id || peer.subscription.is_some() {
            return None;
        }
        let token = self
            .next_token
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("diagnostics subscription tokens exhausted");
        let (tx, rx) = watch::channel(None);
        peer.subscription = Some(SubscriptionEntry { token, tx });
        Some(DiagnosticsSubscription {
            id,
            token,
            rx,
            registry: Arc::downgrade(self),
        })
    }

    fn unsubscribe_exact(&self, id: DiagnosticSubscriberId, token: u64) -> bool {
        let mut peers = self
            .peers
            .lock()
            .expect("diagnostics subscription registry poisoned");
        if peers.get(&id.peer_id).is_some_and(|peer| {
            peer.generation == id.control_connection_id
                && peer
                    .subscription
                    .as_ref()
                    .is_some_and(|entry| entry.token == token)
        }) {
            peers
                .get_mut(&id.peer_id)
                .expect("generation checked above")
                .subscription = None;
            true
        } else {
            false
        }
    }

    fn unsubscribe(&self, id: DiagnosticSubscriberId) -> bool {
        let mut peers = self
            .peers
            .lock()
            .expect("diagnostics subscription registry poisoned");
        if peers.get(&id.peer_id).is_some_and(|peer| {
            peer.generation == id.control_connection_id && peer.subscription.is_some()
        }) {
            peers
                .get_mut(&id.peer_id)
                .expect("generation checked above")
                .subscription = None;
            true
        } else {
            false
        }
    }

    fn clear_generation(&self, id: DiagnosticSubscriberId) -> bool {
        let mut peers = self
            .peers
            .lock()
            .expect("diagnostics subscription registry poisoned");
        if peers
            .get(&id.peer_id)
            .is_some_and(|peer| peer.generation == id.control_connection_id)
        {
            peers.remove(&id.peer_id);
            true
        } else {
            false
        }
    }

    fn has_subscribers(&self) -> bool {
        self.peers
            .lock()
            .expect("diagnostics subscription registry poisoned")
            .values()
            .any(|peer| peer.subscription.is_some())
    }

    fn publish(&self, publication: Arc<DiagnosticPublication>) -> u64 {
        let peers = self
            .peers
            .lock()
            .expect("diagnostics subscription registry poisoned");
        peers
            .values()
            .filter_map(|peer| peer.subscription.as_ref())
            .map(|subscription| {
                subscription.tx.send_replace(Some(publication.clone()));
                1_u64
            })
            .sum()
    }
}

pub struct DiagnosticsSubscription {
    id: DiagnosticSubscriberId,
    token: u64,
    rx: watch::Receiver<Option<Arc<DiagnosticPublication>>>,
    registry: Weak<SubscriptionRegistry>,
}

impl DiagnosticsSubscription {
    pub fn id(&self) -> DiagnosticSubscriberId {
        self.id
    }

    pub fn is_closed(&self) -> bool {
        self.rx.has_changed().is_err()
    }

    pub fn try_recv(&mut self) -> Option<Arc<DiagnosticPublication>> {
        match self.rx.has_changed() {
            Ok(true) => self.rx.borrow_and_update().clone(),
            Ok(false) | Err(_) => None,
        }
    }

    pub async fn recv(&mut self) -> Option<Arc<DiagnosticPublication>> {
        if self.rx.changed().await.is_err() {
            return None;
        }
        self.rx.borrow_and_update().clone()
    }
}

impl Drop for DiagnosticsSubscription {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            registry.unsubscribe_exact(self.id, self.token);
        }
    }
}

#[derive(Clone)]
pub struct DiagnosticsHandle {
    subscribers: Arc<SubscriptionRegistry>,
    discrete: Arc<Mutex<VecDeque<LocalInputDiagnosticEvent>>>,
    discrete_capacity: usize,
    counters: Arc<RuntimeCounters>,
}

impl DiagnosticsHandle {
    pub fn activate_generation(&self, id: DiagnosticSubscriberId) {
        self.subscribers.activate_generation(id);
    }

    pub fn subscribe_current(&self, id: DiagnosticSubscriberId) -> Option<DiagnosticsSubscription> {
        self.subscribers.subscribe_current(id)
    }

    pub fn unsubscribe(&self, id: DiagnosticSubscriberId) -> bool {
        self.subscribers.unsubscribe(id)
    }

    pub fn clear_generation(&self, id: DiagnosticSubscriberId) -> bool {
        self.subscribers.clear_generation(id)
    }

    /// Enqueues a low-frequency discrete diagnostic without awaiting a sink.
    ///
    /// The oldest pending item is discarded on saturation. Input events never
    /// use this queue; their metrics stay in `ControlMetrics`.
    pub fn record_discrete(&self, event: LocalInputDiagnosticEvent) -> bool {
        let mut discrete = self
            .discrete
            .lock()
            .expect("diagnostics discrete queue poisoned");
        if discrete.len() == self.discrete_capacity {
            discrete.pop_front();
        }
        discrete.push_back(event);
        true
    }

    pub fn stats(&self) -> DiagnosticsRuntimeStats {
        self.counters.snapshot()
    }
}

/// Low-priority, sampled diagnostics actor.
///
/// Its input side consists only of atomic counter reads and a bounded
/// low-frequency discrete queue. Subscriber delivery is latest-only and never
/// awaits a blocked sink.
pub struct DiagnosticsRuntime {
    metrics: Arc<ControlMetrics>,
    latest_tx: watch::Sender<ControlMetricSnapshot>,
    latest: watch::Receiver<ControlMetricSnapshot>,
    subscribers: Arc<SubscriptionRegistry>,
    interval: tokio::time::Interval,
    next_sample_at: Duration,
    recent: VecDeque<DiagnosticHistoryEntry>,
    history_capacity: usize,
    last_metric: Option<ControlMetricSnapshot>,
    publication_sequence: u64,
    discrete: Arc<Mutex<VecDeque<LocalInputDiagnosticEvent>>>,
    counters: Arc<RuntimeCounters>,
}

impl DiagnosticsRuntime {
    pub fn new(metrics: Arc<ControlMetrics>, history_capacity: usize) -> Self {
        assert!(
            history_capacity > 0,
            "diagnostics history capacity must be non-zero"
        );
        let initial = metrics.snapshot();
        let (latest_tx, latest) = watch::channel(initial);
        let mut interval = tokio::time::interval_at(
            tokio::time::Instant::now() + DIAGNOSTICS_SAMPLE_PERIOD,
            DIAGNOSTICS_SAMPLE_PERIOD,
        );
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        Self {
            metrics,
            latest_tx,
            latest,
            subscribers: Arc::new(SubscriptionRegistry::default()),
            interval,
            next_sample_at: DIAGNOSTICS_SAMPLE_PERIOD,
            recent: VecDeque::with_capacity(history_capacity),
            history_capacity,
            last_metric: None,
            publication_sequence: 0,
            discrete: Arc::new(Mutex::new(VecDeque::with_capacity(history_capacity))),
            counters: Arc::new(RuntimeCounters::default()),
        }
    }

    pub fn handle(&self) -> DiagnosticsHandle {
        DiagnosticsHandle {
            subscribers: self.subscribers.clone(),
            discrete: self.discrete.clone(),
            discrete_capacity: self.history_capacity,
            counters: self.counters.clone(),
        }
    }

    pub fn activate_generation(&self, id: DiagnosticSubscriberId) {
        self.subscribers.activate_generation(id);
    }

    pub fn subscribe_current(&self, id: DiagnosticSubscriberId) -> Option<DiagnosticsSubscription> {
        self.subscribers.subscribe_current(id)
    }

    pub fn unsubscribe(&self, id: DiagnosticSubscriberId) -> bool {
        self.subscribers.unsubscribe(id)
    }

    pub fn clear_generation(&self, id: DiagnosticSubscriberId) -> bool {
        self.subscribers.clear_generation(id)
    }

    pub fn record_discrete(&self, event: LocalInputDiagnosticEvent) -> bool {
        self.handle().record_discrete(event)
    }

    pub fn latest(&self) -> ControlMetricSnapshot {
        *self.latest.borrow()
    }

    pub fn latest_receiver(&self) -> watch::Receiver<ControlMetricSnapshot> {
        self.latest.clone()
    }

    pub fn history(&self) -> &VecDeque<DiagnosticHistoryEntry> {
        &self.recent
    }

    pub fn stats(&self) -> DiagnosticsRuntimeStats {
        self.counters.snapshot()
    }

    /// Deterministic sampling seam used by tests with a fake monotonic time.
    pub fn sample_at(&mut self, now: Duration) -> bool {
        if now < self.next_sample_at {
            return false;
        }
        self.next_sample_at = now.saturating_add(DIAGNOSTICS_SAMPLE_PERIOD);
        self.sample();
        true
    }

    pub async fn run(mut self, mut shutdown: mpsc::Receiver<()>) {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.recv() => break,
                _ = self.interval.tick() => self.sample(),
            }
        }
    }

    fn sample(&mut self) {
        self.counters.samples.fetch_add(1, Ordering::Relaxed);
        let snapshot = self.metrics.snapshot();
        let latest_changed = *self.latest.borrow() != snapshot;
        if latest_changed {
            self.latest_tx.send_replace(snapshot);
        }

        let mut payloads = vec![DiagnosticPayload::Metrics(snapshot)];
        if self.last_metric != Some(snapshot) {
            self.last_metric = Some(snapshot);
            self.push_history(DiagnosticHistoryEntry::Metrics(snapshot));
        }

        let discrete = {
            let mut pending = self
                .discrete
                .lock()
                .expect("diagnostics discrete queue poisoned");
            pending.drain(..).collect::<Vec<_>>()
        };
        for event in discrete {
            self.push_history(DiagnosticHistoryEntry::Discrete(event.clone()));
            payloads.push(DiagnosticPayload::Discrete(event));
        }

        if !self.subscribers.has_subscribers() {
            return;
        }
        let items = payloads
            .into_iter()
            .map(|payload| {
                let json = format_payload(&payload);
                DiagnosticPublicationItem {
                    payload,
                    json: Arc::from(json),
                }
            })
            .collect::<Vec<_>>();
        self.counters
            .formatted
            .fetch_add(items.len() as u64, Ordering::Relaxed);
        self.publication_sequence = self.publication_sequence.saturating_add(1);
        let publications = self.subscribers.publish(Arc::new(DiagnosticPublication {
            sequence: self.publication_sequence,
            items: items.into(),
        }));
        self.counters
            .publications
            .fetch_add(publications, Ordering::Relaxed);
    }

    fn push_history(&mut self, entry: DiagnosticHistoryEntry) {
        if self.recent.len() == self.history_capacity {
            self.recent.pop_front();
        }
        self.recent.push_back(entry);
    }
}

fn format_payload(payload: &DiagnosticPayload) -> String {
    match payload {
        DiagnosticPayload::Metrics(snapshot) => serde_json::json!({
            "captured": snapshot.captured,
            "routed": snapshot.routed,
            "realtime_replaced": snapshot.realtime_replaced,
            "realtime_dropped": snapshot.realtime_dropped,
            "reliable_overflow": snapshot.reliable_overflow,
        })
        .to_string(),
        DiagnosticPayload::Discrete(event) => {
            serde_json::to_string(event).expect("diagnostic events must serialize")
        }
    }
}
