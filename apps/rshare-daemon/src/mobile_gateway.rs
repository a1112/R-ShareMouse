use anyhow::{anyhow, bail, Context, Result};
use rshare_core::{
    DaemonRequest, DaemonResponse, EndpointEventKind, EndpointEventPayload, EndpointInjectMode,
    EndpointInjectRequest, EndpointInjectResult, EndpointInjectTarget, LocalInputDiagnosticEvent,
    MobileAccessSnapshot,
};
use rshare_input::InjectBackend;
use rshare_net::NetworkManager;
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, Mutex, RwLock, Semaphore};
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout, Duration, Instant};

use crate::{endpoint_runtime::inject_endpoint_event, DaemonState};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 128 * 1024;
const MAX_MOBILE_CLIENT_ID_BYTES: usize = 128;
const MAX_MOBILE_RELEASE_BATCH_REQUESTS: usize = 96;
const MAX_MOBILE_HELD_KEYS: usize = 64;
const MAX_MOBILE_HELD_MOUSE_BUTTONS: usize = 16;

#[derive(Debug, Clone)]
struct MobileGatewayLimits {
    max_active_clients: usize,
    max_client_sessions: usize,
    read_deadline: Duration,
    write_deadline: Duration,
    held_input_lease: Duration,
    accept_error_initial_backoff: Duration,
    accept_error_max_backoff: Duration,
}

impl Default for MobileGatewayLimits {
    fn default() -> Self {
        Self {
            max_active_clients: 64,
            max_client_sessions: 512,
            read_deadline: Duration::from_secs(5),
            write_deadline: Duration::from_secs(5),
            held_input_lease: Duration::from_secs(15),
            accept_error_initial_backoff: Duration::from_millis(25),
            accept_error_max_backoff: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MobileInjectEnvelope {
    client_id: String,
    sequence: u64,
    request: DaemonRequest,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MobileReleaseBatch {
    client_id: String,
    sequence: u64,
    requests: Vec<DaemonRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MobileInjectBody {
    Single(MobileInjectEnvelope),
    ReleaseBatch(MobileReleaseBatch),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct HeldKeyIdentity(u64);

#[derive(Debug)]
struct MobileClientSession {
    last_sequence: u64,
    last_seen: Instant,
    held_keys: BTreeMap<HeldKeyIdentity, rshare_input::KeyCode>,
    held_mouse_buttons: BTreeMap<u8, rshare_input::MouseButton>,
    last_mouse_position: (i32, i32),
}

impl MobileClientSession {
    fn new(now: Instant) -> Self {
        Self {
            last_sequence: 0,
            last_seen: now,
            held_keys: BTreeMap::new(),
            held_mouse_buttons: BTreeMap::new(),
            last_mouse_position: (0, 0),
        }
    }

    fn has_held_input(&self) -> bool {
        !self.held_keys.is_empty() || !self.held_mouse_buttons.is_empty()
    }
}

#[derive(Debug, Clone)]
struct MobileClientSessions {
    sessions: Arc<Mutex<HashMap<String, Arc<Mutex<MobileClientSession>>>>>,
    max_sessions: usize,
    held_input_lease: Duration,
}

impl MobileClientSessions {
    fn new(max_sessions: usize, held_input_lease: Duration) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            max_sessions: max_sessions.max(1),
            held_input_lease,
        }
    }

    async fn session_at(
        &self,
        client_id: &str,
        now: Instant,
    ) -> Result<Arc<Mutex<MobileClientSession>>> {
        validate_mobile_client_id(client_id)?;
        let mut sessions = self.sessions.lock().await;
        self.prune_idle_locked(&mut sessions, now);
        if let Some(session) = sessions.get(client_id) {
            return Ok(session.clone());
        }
        if sessions.len() >= self.max_sessions {
            bail!("mobile client session limit reached");
        }
        let session = Arc::new(Mutex::new(MobileClientSession::new(now)));
        sessions.insert(client_id.to_string(), session.clone());
        Ok(session)
    }

    fn prune_idle_locked(
        &self,
        sessions: &mut HashMap<String, Arc<Mutex<MobileClientSession>>>,
        now: Instant,
    ) {
        let prune_after = self
            .held_input_lease
            .checked_mul(2)
            .unwrap_or(Duration::MAX);
        sessions.retain(|_, session| {
            if Arc::strong_count(session) != 1 {
                return true;
            }
            let Ok(session) = session.try_lock() else {
                return true;
            };
            session.has_held_input()
                || now
                    .checked_duration_since(session.last_seen)
                    .unwrap_or_default()
                    < prune_after
        });
    }

    async fn refresh_client_at(&self, client_id: &str, now: Instant) -> Result<()> {
        let session = self.session_at(client_id, now).await?;
        session.lock().await.last_seen = now;
        Ok(())
    }

    async fn reap_expired_at(
        &self,
        now: Instant,
        network_manager: &Arc<Mutex<NetworkManager>>,
        inject_backend: &Arc<Mutex<Box<dyn InjectBackend>>>,
        state: &Arc<RwLock<DaemonState>>,
        local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    ) {
        let sessions = self.snapshot().await;

        for (client_id, session) in &sessions {
            let mut session = session.lock().await;
            let elapsed = now
                .checked_duration_since(session.last_seen)
                .unwrap_or_default();
            if elapsed < self.held_input_lease || !session.has_held_input() {
                continue;
            }
            release_held_inputs(
                client_id,
                &mut session,
                network_manager,
                inject_backend,
                state,
                local_events_tx,
            )
            .await;
        }
        drop(sessions);

        let mut sessions = self.sessions.lock().await;
        self.prune_idle_locked(&mut sessions, now);
    }

    async fn release_all_held_inputs(
        &self,
        network_manager: &Arc<Mutex<NetworkManager>>,
        inject_backend: &Arc<Mutex<Box<dyn InjectBackend>>>,
        state: &Arc<RwLock<DaemonState>>,
        local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    ) {
        for (client_id, session) in self.snapshot().await {
            let mut session = session.lock().await;
            release_held_inputs(
                &client_id,
                &mut session,
                network_manager,
                inject_backend,
                state,
                local_events_tx,
            )
            .await;
        }
    }

    async fn snapshot(&self) -> Vec<(String, Arc<Mutex<MobileClientSession>>)> {
        self.sessions
            .lock()
            .await
            .iter()
            .map(|(client_id, session)| (client_id.clone(), session.clone()))
            .collect()
    }
}

async fn release_held_inputs(
    client_id: &str,
    session: &mut MobileClientSession,
    network_manager: &Arc<Mutex<NetworkManager>>,
    inject_backend: &Arc<Mutex<Box<dyn InjectBackend>>>,
    state: &Arc<RwLock<DaemonState>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
) {
    let held_keys = session
        .held_keys
        .iter()
        .map(|(identity, keycode)| (*identity, *keycode))
        .collect::<Vec<_>>();
    for (identity, keycode) in held_keys {
        let request = lease_release_request(
            client_id,
            EndpointEventKind::Keyboard,
            EndpointEventPayload::Keyboard {
                key: canonical_key_release_name(keycode),
                state: "Released".to_string(),
            },
        );
        let result = inject_endpoint_event(
            network_manager,
            inject_backend,
            state,
            local_events_tx,
            EndpointInjectTarget::Local,
            request,
        )
        .await;
        if result.accepted {
            session.held_keys.remove(&identity);
        }
    }

    let held_buttons = session
        .held_mouse_buttons
        .iter()
        .map(|(identity, button)| (*identity, *button))
        .collect::<Vec<_>>();
    for (identity, button) in held_buttons {
        let (x, y) = session.last_mouse_position;
        let request = lease_release_request(
            client_id,
            EndpointEventKind::Mouse,
            EndpointEventPayload::MouseButton {
                button: canonical_mouse_button_release_name(button),
                state: "Released".to_string(),
                x,
                y,
            },
        );
        let result = inject_endpoint_event(
            network_manager,
            inject_backend,
            state,
            local_events_tx,
            EndpointInjectTarget::Local,
            request,
        )
        .await;
        if result.accepted {
            session.held_mouse_buttons.remove(&identity);
        }
    }
}

fn validate_mobile_client_id(client_id: &str) -> Result<()> {
    if client_id.is_empty() || client_id.len() > MAX_MOBILE_CLIENT_ID_BYTES {
        bail!("mobile client_id must contain 1-{MAX_MOBILE_CLIENT_ID_BYTES} bytes");
    }
    if !client_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        bail!("mobile client_id contains unsupported characters");
    }
    Ok(())
}

fn lease_release_request(
    client_id: &str,
    device_kind: EndpointEventKind,
    payload: EndpointEventPayload,
) -> EndpointInjectRequest {
    EndpointInjectRequest {
        correlation_id: format!("mobile-lease-{}-{}", client_id, mobile_timestamp_ms_now()),
        device_kind,
        payload,
        mode: EndpointInjectMode::BestEffort,
        timeout_ms: 750,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MobileGatewayAccess {
    enabled: bool,
    bind_addr: SocketAddr,
    token: String,
    advertise_host: String,
    activity: Arc<StdMutex<MobileGatewayActivity>>,
}

#[derive(Debug, Default)]
struct MobileGatewayActivity {
    last_client_addr: Option<String>,
    last_client_seen_at_ms: Option<u64>,
    client_count: u64,
}

impl MobileGatewayAccess {
    pub(crate) fn new(bind_addr: SocketAddr, token: String, advertise_host: String) -> Self {
        Self {
            enabled: true,
            bind_addr,
            token,
            advertise_host,
            activity: Arc::new(StdMutex::new(MobileGatewayActivity::default())),
        }
    }

    pub(crate) fn disabled(_reason: String) -> Self {
        Self {
            enabled: false,
            bind_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            token: String::new(),
            advertise_host: String::new(),
            activity: Arc::new(StdMutex::new(MobileGatewayActivity::default())),
        }
    }

    pub(crate) fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    pub(crate) fn record_client(&self, addr: SocketAddr) {
        if let Ok(mut activity) = self.activity.lock() {
            activity.client_count = activity.client_count.saturating_add(1);
            activity.last_client_addr = Some(addr.to_string());
            activity.last_client_seen_at_ms = Some(mobile_timestamp_ms_now());
        }
    }

    fn activity_snapshot(&self) -> (Option<String>, Option<u64>, u64) {
        self.activity
            .lock()
            .map(|activity| {
                (
                    activity.last_client_addr.clone(),
                    activity.last_client_seen_at_ms,
                    activity.client_count,
                )
            })
            .unwrap_or((None, None, 0))
    }

    pub(crate) fn snapshot(&self) -> MobileAccessSnapshot {
        if !self.enabled {
            return MobileAccessSnapshot {
                enabled: false,
                bind_address: "不可用".to_string(),
                page_url: "不可用".to_string(),
                token: String::new(),
                last_client_addr: None,
                last_client_seen_at_ms: None,
                client_count: 0,
            };
        }

        let (last_client_addr, last_client_seen_at_ms, client_count) = self.activity_snapshot();
        MobileAccessSnapshot {
            enabled: true,
            bind_address: self.bind_addr.to_string(),
            page_url: format!(
                "http://{}:{}/mobile{}",
                self.advertise_host,
                self.bind_addr.port(),
                mobile_token_query(&self.token)
            ),
            token: self.token.clone(),
            last_client_addr,
            last_client_seen_at_ms,
            client_count,
        }
    }
}

fn mobile_timestamp_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Clone)]
pub(crate) struct MobileHttpRequest {
    pub(crate) method: String,
    pub(crate) target: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl MobileHttpRequest {
    #[cfg(test)]
    pub(crate) fn new(method: &str, target: &str, headers: Vec<(&str, &str)>) -> Self {
        Self {
            method: method.to_ascii_uppercase(),
            target: target.to_string(),
            headers: headers
                .into_iter()
                .map(|(key, value)| (key.to_ascii_lowercase(), value.to_string()))
                .collect(),
            body: Vec::new(),
        }
    }

    fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .get(&key.to_ascii_lowercase())
            .map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MobileGatewayRoute {
    Page,
    LocalControls,
    Inject,
    NotFound,
}

pub(crate) fn route_mobile_http_request(method: &str, target: &str) -> MobileGatewayRoute {
    let path = target
        .split('?')
        .next()
        .unwrap_or(target)
        .trim_end_matches('/');
    match (method.to_ascii_uppercase().as_str(), path) {
        ("GET", "" | "/mobile") => MobileGatewayRoute::Page,
        ("GET", "/api/local-controls") => MobileGatewayRoute::LocalControls,
        ("POST", "/api/inject") => MobileGatewayRoute::Inject,
        _ => MobileGatewayRoute::NotFound,
    }
}

pub(crate) fn is_authorized_mobile_request(request: &MobileHttpRequest, expected: &str) -> bool {
    if expected.is_empty() {
        return false;
    }

    mobile_token_from_target(&request.target)
        .or_else(|| mobile_token_from_authorization(request.header("authorization")))
        .or_else(|| request.header("x-rshare-mobile-token").map(str::to_string))
        .as_deref()
        == Some(expected)
}

pub(crate) fn preferred_mobile_advertise_host(hostname: &str) -> String {
    detect_lan_ipv4()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|| {
            let trimmed = hostname.trim();
            if trimmed.is_empty() {
                "127.0.0.1".to_string()
            } else {
                trimmed.to_string()
            }
        })
}

pub(crate) async fn run_mobile_gateway_server(
    access: MobileGatewayAccess,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    inject_backend: Arc<Mutex<Box<dyn InjectBackend>>>,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    let listener = match TcpListener::bind(access.bind_addr()).await {
        Ok(listener) => listener,
        Err(error) => {
            let reason = format!(
                "Mobile gateway unavailable on {}: {}",
                access.bind_addr(),
                error
            );
            tracing::warn!("{}", reason);
            {
                let mut state = state.write().await;
                state.mobile_access = MobileGatewayAccess::disabled(reason);
            }
            let _ = shutdown_rx.recv().await;
            return Ok(());
        }
    };
    tracing::info!("Mobile gateway listening on {}", access.bind_addr());

    run_mobile_gateway_server_on_listener(
        listener,
        access,
        state,
        network_manager,
        inject_backend,
        local_events_tx,
        shutdown_rx,
        MobileGatewayLimits::default(),
    )
    .await
}

async fn run_mobile_gateway_server_on_listener(
    listener: TcpListener,
    access: MobileGatewayAccess,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    inject_backend: Arc<Mutex<Box<dyn InjectBackend>>>,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
    limits: MobileGatewayLimits,
) -> Result<()> {
    let active_clients = Arc::new(Semaphore::new(limits.max_active_clients.max(1)));
    let sessions = MobileClientSessions::new(limits.max_client_sessions, limits.held_input_lease);
    let mut client_tasks = JoinSet::new();
    let mut accept_backoff = limits.accept_error_initial_backoff;

    let reaper_sessions = sessions.clone();
    let reaper_network_manager = network_manager.clone();
    let reaper_inject_backend = inject_backend.clone();
    let reaper_state = state.clone();
    let reaper_local_events_tx = local_events_tx.clone();
    let reaper_lease = limits.held_input_lease;
    let mut reaper_shutdown_rx = shutdown_rx.resubscribe();
    let reaper_task = tokio::spawn(async move {
        let interval_duration = (reaper_lease / 2).max(Duration::from_millis(25));
        let mut interval = tokio::time::interval(interval_duration);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    reaper_sessions.reap_expired_at(
                        Instant::now(),
                        &reaper_network_manager,
                        &reaper_inject_backend,
                        &reaper_state,
                        &reaper_local_events_tx,
                    ).await;
                }
                _ = reaper_shutdown_rx.recv() => break,
            }
        }
    });

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (mut stream, peer_addr) = match result {
                    Ok(accepted) => {
                        accept_backoff = limits.accept_error_initial_backoff;
                        accepted
                    }
                    Err(error) => {
                        tracing::warn!("Mobile gateway accept failed: {error}");
                        let delay = accept_backoff.min(limits.accept_error_max_backoff);
                        accept_backoff = accept_backoff
                            .checked_mul(2)
                            .unwrap_or(limits.accept_error_max_backoff)
                            .min(limits.accept_error_max_backoff);
                        tokio::select! {
                            _ = sleep(delay) => {}
                            _ = shutdown_rx.recv() => break,
                        }
                        continue;
                    }
                };
                let permit = match active_clients.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        let _ = write_mobile_response_with_deadline(
                            &mut stream,
                            503,
                            "application/json; charset=utf-8",
                            json!({ "error": "mobile gateway busy" }).to_string().into_bytes(),
                            limits.write_deadline,
                        ).await;
                        continue;
                    }
                };
                let access = access.clone();
                let state = state.clone();
                let network_manager = network_manager.clone();
                let inject_backend = inject_backend.clone();
                let local_events_tx = local_events_tx.clone();
                let sessions = sessions.clone();
                let limits = limits.clone();
                client_tasks.spawn(async move {
                    let _permit = permit;
                    if let Err(error) = handle_mobile_gateway_client_with_context(
                        stream,
                        peer_addr,
                        access,
                        state,
                        network_manager,
                        inject_backend,
                        local_events_tx,
                        sessions,
                        limits,
                    )
                    .await
                    {
                        tracing::debug!("Mobile gateway client error: {}", error);
                    }
                });
            }
            _ = shutdown_rx.recv() => break,
            Some(result) = client_tasks.join_next(), if !client_tasks.is_empty() => {
                if let Err(error) = result {
                    tracing::debug!("Mobile gateway client task failed: {error}");
                }
            }
        }
    }

    client_tasks.abort_all();
    while client_tasks.join_next().await.is_some() {}
    reaper_task.abort();
    let _ = reaper_task.await;
    sessions
        .release_all_held_inputs(&network_manager, &inject_backend, &state, &local_events_tx)
        .await;
    Ok(())
}

#[cfg(test)]
async fn handle_mobile_gateway_client(
    stream: TcpStream,
    peer_addr: SocketAddr,
    access: MobileGatewayAccess,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    inject_backend: Arc<Mutex<Box<dyn InjectBackend>>>,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
) -> Result<()> {
    let limits = MobileGatewayLimits::default();
    let sessions = MobileClientSessions::new(limits.max_client_sessions, limits.held_input_lease);
    handle_mobile_gateway_client_with_context(
        stream,
        peer_addr,
        access,
        state,
        network_manager,
        inject_backend,
        local_events_tx,
        sessions,
        limits,
    )
    .await
}

async fn handle_mobile_gateway_client_with_context(
    mut stream: TcpStream,
    peer_addr: SocketAddr,
    access: MobileGatewayAccess,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    inject_backend: Arc<Mutex<Box<dyn InjectBackend>>>,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
    sessions: MobileClientSessions,
    limits: MobileGatewayLimits,
) -> Result<()> {
    let request = read_mobile_http_request_with_deadline(&mut stream, limits.read_deadline).await?;
    let route = route_mobile_http_request(&request.method, &request.target);

    if route == MobileGatewayRoute::NotFound {
        return write_mobile_response_with_deadline(
            &mut stream,
            404,
            "application/json; charset=utf-8",
            json!({ "error": "not found" }).to_string().into_bytes(),
            limits.write_deadline,
        )
        .await;
    }

    if !is_authorized_mobile_request(&request, access.token()) {
        return write_mobile_response_with_deadline(
            &mut stream,
            401,
            "application/json; charset=utf-8",
            json!({ "error": "unauthorized" }).to_string().into_bytes(),
            limits.write_deadline,
        )
        .await;
    }
    access.record_client(peer_addr);

    match route {
        MobileGatewayRoute::Page => {
            write_mobile_response_with_deadline(
                &mut stream,
                200,
                "text/html; charset=utf-8",
                render_mobile_page_with_token(access.token()).into_bytes(),
                limits.write_deadline,
            )
            .await
        }
        MobileGatewayRoute::LocalControls => {
            if let Some(client_id) = mobile_query_value(&request.target, "client_id") {
                if let Err(error) = sessions.refresh_client_at(&client_id, Instant::now()).await {
                    return write_mobile_response_with_deadline(
                        &mut stream,
                        400,
                        "application/json; charset=utf-8",
                        json!({
                            "error": "invalid mobile client",
                            "detail": error.to_string(),
                        })
                        .to_string()
                        .into_bytes(),
                        limits.write_deadline,
                    )
                    .await;
                }
            }
            let snapshot = {
                let mut state = state.write().await;
                state.refresh_local_controls_platform();
                state.local_control_snapshot()
            };
            write_mobile_json_with_deadline(
                &mut stream,
                &DaemonResponse::LocalControls(snapshot),
                limits.write_deadline,
            )
            .await
        }
        MobileGatewayRoute::Inject => {
            let body = match mobile_inject_body_from_body(&request.body) {
                Ok(body) => body,
                Err(error) => {
                    return write_mobile_response_with_deadline(
                        &mut stream,
                        400,
                        "application/json; charset=utf-8",
                        json!({
                            "error": "invalid mobile inject request",
                            "detail": error.to_string(),
                        })
                        .to_string()
                        .into_bytes(),
                        limits.write_deadline,
                    )
                    .await;
                }
            };
            let response = match body {
                MobileInjectBody::Single(envelope) => process_mobile_inject_envelope(
                    &sessions,
                    &network_manager,
                    &inject_backend,
                    &state,
                    &local_events_tx,
                    envelope,
                )
                .await
                .map(|result| {
                    serde_json::to_value(DaemonResponse::EndpointInjectResult(result))
                        .expect("daemon response must serialize")
                }),
                MobileInjectBody::ReleaseBatch(batch) => process_mobile_release_batch(
                    &sessions,
                    &network_manager,
                    &inject_backend,
                    &state,
                    &local_events_tx,
                    batch,
                )
                .await
                .map(|results| json!({ "MobileReleaseBatchResult": { "results": results } })),
            };
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    return write_mobile_response_with_deadline(
                        &mut stream,
                        409,
                        "application/json; charset=utf-8",
                        json!({
                            "error": "mobile inject rejected",
                            "detail": error.to_string(),
                        })
                        .to_string()
                        .into_bytes(),
                        limits.write_deadline,
                    )
                    .await;
                }
            };
            write_mobile_json_with_deadline(&mut stream, &response, limits.write_deadline).await
        }
        MobileGatewayRoute::NotFound => unreachable!("handled above"),
    }
}

#[cfg(test)]
fn mobile_inject_envelope_from_body(body: &[u8]) -> Result<MobileInjectEnvelope> {
    if body.is_empty() {
        bail!("empty mobile inject body");
    }

    let envelope = serde_json::from_slice::<MobileInjectEnvelope>(body)
        .context("failed to decode mobile inject envelope")?;
    validate_mobile_inject_envelope(&envelope)?;
    Ok(envelope)
}

fn mobile_inject_body_from_body(body: &[u8]) -> Result<MobileInjectBody> {
    if body.is_empty() {
        bail!("empty mobile inject body");
    }
    let body = serde_json::from_slice::<MobileInjectBody>(body)
        .context("failed to decode mobile inject body")?;
    match &body {
        MobileInjectBody::Single(envelope) => validate_mobile_inject_envelope(envelope)?,
        MobileInjectBody::ReleaseBatch(batch) => validate_mobile_release_batch(batch)?,
    }
    Ok(body)
}

fn validate_mobile_inject_envelope(envelope: &MobileInjectEnvelope) -> Result<()> {
    validate_mobile_client_id(&envelope.client_id)?;
    validate_mobile_sequence(envelope.sequence)?;
    match &envelope.request {
        DaemonRequest::InjectEndpointEvent {
            target: EndpointInjectTarget::Local,
            ..
        } => Ok(()),
        DaemonRequest::InjectEndpointEvent {
            target: EndpointInjectTarget::Remote(_),
            ..
        } => bail!("mobile gateway only injects into the local endpoint"),
        _ => bail!("mobile envelope only accepts InjectEndpointEvent"),
    }
}

fn validate_mobile_release_batch(batch: &MobileReleaseBatch) -> Result<()> {
    validate_mobile_client_id(&batch.client_id)?;
    validate_mobile_sequence(batch.sequence)?;
    if batch.requests.is_empty() || batch.requests.len() > MAX_MOBILE_RELEASE_BATCH_REQUESTS {
        bail!("mobile release batch must contain 1-{MAX_MOBILE_RELEASE_BATCH_REQUESTS} requests");
    }
    for request in &batch.requests {
        let request = match request {
            DaemonRequest::InjectEndpointEvent {
                target: EndpointInjectTarget::Local,
                request,
            } => request,
            _ => bail!("mobile release batch only accepts local InjectEndpointEvent requests"),
        };
        let event = crate::endpoint_payload_to_input_event(request)
            .context("invalid mobile release batch event")?;
        if !matches!(
            event,
            rshare_input::InputEvent::Key {
                state: rshare_input::ButtonState::Released,
                ..
            } | rshare_input::InputEvent::MouseButton {
                state: rshare_input::ButtonState::Released,
                ..
            }
        ) {
            bail!("mobile release batch only accepts released key or mouse button events");
        }
    }
    Ok(())
}

fn validate_mobile_sequence(sequence: u64) -> Result<()> {
    if sequence == 0 {
        bail!("mobile sequence must be greater than zero");
    }
    Ok(())
}

async fn process_mobile_inject_envelope(
    sessions: &MobileClientSessions,
    network_manager: &Arc<Mutex<NetworkManager>>,
    inject_backend: &Arc<Mutex<Box<dyn InjectBackend>>>,
    state: &Arc<RwLock<DaemonState>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    envelope: MobileInjectEnvelope,
) -> Result<EndpointInjectResult> {
    let now = Instant::now();
    let session = sessions.session_at(&envelope.client_id, now).await?;
    let mut session = session.lock().await;
    accept_mobile_sequence(&mut session, envelope.sequence, now)?;

    let request = match envelope.request {
        DaemonRequest::InjectEndpointEvent {
            target: EndpointInjectTarget::Local,
            request,
        } => request,
        _ => bail!("mobile envelope only accepts a local InjectEndpointEvent"),
    };
    let held_transition = mobile_held_transition(&request)?;
    validate_mobile_held_capacity(&session, held_transition)?;
    let provisional_press = provision_mobile_held_press(&mut session, held_transition);
    let result = inject_endpoint_event(
        network_manager,
        inject_backend,
        state,
        local_events_tx,
        EndpointInjectTarget::Local,
        request.clone(),
    )
    .await;
    if result.accepted {
        apply_mobile_held_transition(&mut session, held_transition);
    } else {
        rollback_mobile_held_press(&mut session, provisional_press);
    }
    Ok(result)
}

async fn process_mobile_release_batch(
    sessions: &MobileClientSessions,
    network_manager: &Arc<Mutex<NetworkManager>>,
    inject_backend: &Arc<Mutex<Box<dyn InjectBackend>>>,
    state: &Arc<RwLock<DaemonState>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    batch: MobileReleaseBatch,
) -> Result<Vec<EndpointInjectResult>> {
    validate_mobile_release_batch(&batch)?;
    let now = Instant::now();
    let session = sessions.session_at(&batch.client_id, now).await?;
    let mut session = session.lock().await;
    accept_mobile_sequence(&mut session, batch.sequence, now)?;

    let mut results = Vec::with_capacity(batch.requests.len());
    for request in batch.requests {
        let request = match request {
            DaemonRequest::InjectEndpointEvent {
                target: EndpointInjectTarget::Local,
                request,
            } => request,
            _ => unreachable!("release batch was validated before processing"),
        };
        let held_transition = mobile_held_transition(&request)
            .expect("validated release request must have a held-input transition");
        let result = inject_endpoint_event(
            network_manager,
            inject_backend,
            state,
            local_events_tx,
            EndpointInjectTarget::Local,
            request.clone(),
        )
        .await;
        if result.accepted {
            apply_mobile_held_transition(&mut session, held_transition);
        }
        results.push(result);
    }
    Ok(results)
}

fn accept_mobile_sequence(
    session: &mut MobileClientSession,
    sequence: u64,
    now: Instant,
) -> Result<()> {
    if sequence <= session.last_sequence {
        bail!(
            "mobile sequence {} is not greater than last sequence {}",
            sequence,
            session.last_sequence
        );
    }
    session.last_sequence = sequence;
    session.last_seen = now;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum MobileHeldTransition {
    Key {
        identity: HeldKeyIdentity,
        keycode: rshare_input::KeyCode,
        pressed: bool,
    },
    MouseButton {
        identity: u8,
        button: rshare_input::MouseButton,
        pressed: bool,
        x: i32,
        y: i32,
    },
    MousePosition {
        x: i32,
        y: i32,
    },
}

#[derive(Debug, Clone, Copy)]
enum ProvisionalHeldPress {
    Key {
        identity: HeldKeyIdentity,
        introduced: bool,
    },
    MouseButton {
        identity: u8,
        introduced: bool,
        previous_position: (i32, i32),
    },
}

fn mobile_held_transition(request: &EndpointInjectRequest) -> Result<Option<MobileHeldTransition>> {
    match &request.payload {
        EndpointEventPayload::Keyboard { .. } => {
            let event = crate::endpoint_payload_to_input_event(request)?;
            let rshare_input::InputEvent::Key { keycode, state } = event else {
                bail!("mobile keyboard payload did not produce a key event");
            };
            let keycode = canonical_key_code(keycode);
            Ok(Some(MobileHeldTransition::Key {
                identity: held_key_identity(keycode),
                keycode,
                pressed: state.is_pressed(),
            }))
        }
        EndpointEventPayload::MouseButton { x, y, .. } => {
            let event = crate::endpoint_payload_to_input_event(request)?;
            let rshare_input::InputEvent::MouseButton { button, state } = event else {
                bail!("mobile mouse-button payload did not produce a button event");
            };
            let button = canonical_mouse_button(button);
            Ok(Some(MobileHeldTransition::MouseButton {
                identity: button.to_code(),
                button,
                pressed: state.is_pressed(),
                x: *x,
                y: *y,
            }))
        }
        EndpointEventPayload::MouseMove { x, y, .. }
        | EndpointEventPayload::MouseWheel { x, y, .. } => {
            Ok(Some(MobileHeldTransition::MousePosition { x: *x, y: *y }))
        }
        _ => Ok(None),
    }
}

fn validate_mobile_held_capacity(
    session: &MobileClientSession,
    transition: Option<MobileHeldTransition>,
) -> Result<()> {
    match transition {
        Some(MobileHeldTransition::Key {
            identity,
            pressed: true,
            ..
        }) if !session.held_keys.contains_key(&identity)
            && session.held_keys.len() >= MAX_MOBILE_HELD_KEYS =>
        {
            bail!("mobile held key limit reached")
        }
        Some(MobileHeldTransition::MouseButton {
            identity,
            pressed: true,
            ..
        }) if !session.held_mouse_buttons.contains_key(&identity)
            && session.held_mouse_buttons.len() >= MAX_MOBILE_HELD_MOUSE_BUTTONS =>
        {
            bail!("mobile held mouse button limit reached")
        }
        _ => Ok(()),
    }
}

fn provision_mobile_held_press(
    session: &mut MobileClientSession,
    transition: Option<MobileHeldTransition>,
) -> Option<ProvisionalHeldPress> {
    match transition {
        Some(MobileHeldTransition::Key {
            identity,
            keycode,
            pressed: true,
        }) => {
            let introduced = session.held_keys.insert(identity, keycode).is_none();
            Some(ProvisionalHeldPress::Key {
                identity,
                introduced,
            })
        }
        Some(MobileHeldTransition::MouseButton {
            identity,
            button,
            pressed: true,
            x,
            y,
        }) => {
            let previous_position = session.last_mouse_position;
            session.last_mouse_position = (x, y);
            let introduced = session
                .held_mouse_buttons
                .insert(identity, button)
                .is_none();
            Some(ProvisionalHeldPress::MouseButton {
                identity,
                introduced,
                previous_position,
            })
        }
        _ => None,
    }
}

fn rollback_mobile_held_press(
    session: &mut MobileClientSession,
    provisional: Option<ProvisionalHeldPress>,
) {
    match provisional {
        Some(ProvisionalHeldPress::Key {
            identity,
            introduced: true,
        }) => {
            session.held_keys.remove(&identity);
        }
        Some(ProvisionalHeldPress::MouseButton {
            identity,
            introduced,
            previous_position,
        }) => {
            session.last_mouse_position = previous_position;
            if introduced {
                session.held_mouse_buttons.remove(&identity);
            }
        }
        _ => {}
    }
}

fn apply_mobile_held_transition(
    session: &mut MobileClientSession,
    transition: Option<MobileHeldTransition>,
) {
    match transition {
        Some(MobileHeldTransition::Key {
            identity,
            keycode,
            pressed: true,
        }) => {
            session.held_keys.insert(identity, keycode);
        }
        Some(MobileHeldTransition::Key {
            identity,
            pressed: false,
            ..
        }) => {
            session.held_keys.remove(&identity);
        }
        Some(MobileHeldTransition::MouseButton {
            identity,
            button,
            pressed: true,
            x,
            y,
        }) => {
            session.last_mouse_position = (x, y);
            session.held_mouse_buttons.insert(identity, button);
        }
        Some(MobileHeldTransition::MouseButton {
            identity,
            pressed: false,
            x,
            y,
            ..
        }) => {
            session.last_mouse_position = (x, y);
            session.held_mouse_buttons.remove(&identity);
        }
        Some(MobileHeldTransition::MousePosition { x, y }) => {
            session.last_mouse_position = (x, y);
        }
        None => {}
    }
}

fn canonical_key_code(keycode: rshare_input::KeyCode) -> rshare_input::KeyCode {
    use rshare_input::KeyCode;
    match keycode {
        KeyCode::Char(value) => KeyCode::Char(value.to_ascii_uppercase()),
        other => other,
    }
}

fn held_key_identity(keycode: rshare_input::KeyCode) -> HeldKeyIdentity {
    use rshare_input::KeyCode;
    const NAMED_CLASS: u64 = 0;
    const CHAR_CLASS: u64 = 1;
    const RAW_CLASS: u64 = 2;
    let (class, value) = match keycode {
        KeyCode::Char(value) => (CHAR_CLASS, value as u32),
        KeyCode::Raw(value) => (RAW_CLASS, value),
        other => (NAMED_CLASS, other.to_raw()),
    };
    HeldKeyIdentity((class << 32) | value as u64)
}

fn canonical_key_release_name(keycode: rshare_input::KeyCode) -> String {
    use rshare_input::KeyCode;
    match keycode {
        KeyCode::Char(value) => (value as char).to_string(),
        KeyCode::Escape => "Escape".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Insert => "Insert".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::ShiftLeft => "ShiftLeft".to_string(),
        KeyCode::ShiftRight => "ShiftRight".to_string(),
        KeyCode::ControlLeft => "ControlLeft".to_string(),
        KeyCode::ControlRight => "ControlRight".to_string(),
        KeyCode::AltLeft => "AltLeft".to_string(),
        KeyCode::AltRight => "AltRight".to_string(),
        KeyCode::SuperLeft => "SuperLeft".to_string(),
        KeyCode::SuperRight => "SuperRight".to_string(),
        KeyCode::F1 => "F1".to_string(),
        KeyCode::F2 => "F2".to_string(),
        KeyCode::F3 => "F3".to_string(),
        KeyCode::F4 => "F4".to_string(),
        KeyCode::F5 => "F5".to_string(),
        KeyCode::F6 => "F6".to_string(),
        KeyCode::F7 => "F7".to_string(),
        KeyCode::F8 => "F8".to_string(),
        KeyCode::F9 => "F9".to_string(),
        KeyCode::F10 => "F10".to_string(),
        KeyCode::F11 => "F11".to_string(),
        KeyCode::F12 => "F12".to_string(),
        KeyCode::Space => "Space".to_string(),
        KeyCode::CapsLock => "CapsLock".to_string(),
        KeyCode::NumLock => "NumLock".to_string(),
        KeyCode::Keypad0 => "Keypad0".to_string(),
        KeyCode::Keypad1 => "Keypad1".to_string(),
        KeyCode::Keypad2 => "Keypad2".to_string(),
        KeyCode::Keypad3 => "Keypad3".to_string(),
        KeyCode::Keypad4 => "Keypad4".to_string(),
        KeyCode::Keypad5 => "Keypad5".to_string(),
        KeyCode::Keypad6 => "Keypad6".to_string(),
        KeyCode::Keypad7 => "Keypad7".to_string(),
        KeyCode::Keypad8 => "Keypad8".to_string(),
        KeyCode::Keypad9 => "Keypad9".to_string(),
        KeyCode::KeypadAdd => "KeypadAdd".to_string(),
        KeyCode::KeypadSubtract => "KeypadSubtract".to_string(),
        KeyCode::KeypadMultiply => "KeypadMultiply".to_string(),
        KeyCode::KeypadDivide => "KeypadDivide".to_string(),
        KeyCode::KeypadDecimal => "KeypadDecimal".to_string(),
        KeyCode::KeypadEnter => "KeypadEnter".to_string(),
        KeyCode::Raw(value) => format!("Raw({value})"),
    }
}

fn canonical_mouse_button(button: rshare_input::MouseButton) -> rshare_input::MouseButton {
    rshare_input::MouseButton::from_code(button.to_code())
}

fn canonical_mouse_button_release_name(button: rshare_input::MouseButton) -> String {
    use rshare_input::MouseButton;
    match button {
        MouseButton::Left => "Left".to_string(),
        MouseButton::Middle => "Middle".to_string(),
        MouseButton::Right => "Right".to_string(),
        MouseButton::Back => "Back".to_string(),
        MouseButton::Forward => "Forward".to_string(),
        MouseButton::Other(value) => format!("Other({value})"),
    }
}

async fn write_mobile_json_with_deadline<T: serde::Serialize>(
    stream: &mut TcpStream,
    value: &T,
    deadline: Duration,
) -> Result<()> {
    write_mobile_response_with_deadline(
        stream,
        200,
        "application/json; charset=utf-8",
        serde_json::to_vec(value)?,
        deadline,
    )
    .await
}

async fn read_mobile_http_request_with_deadline(
    stream: &mut TcpStream,
    deadline: Duration,
) -> Result<MobileHttpRequest> {
    timeout(deadline, read_mobile_http_request(stream))
        .await
        .context("mobile HTTP read deadline exceeded")?
}

async fn read_mobile_http_request(stream: &mut TcpStream) -> Result<MobileHttpRequest> {
    let mut header_bytes = Vec::new();
    let mut byte = [0u8; 1];
    while !header_bytes.ends_with(b"\r\n\r\n") {
        let read = stream.read(&mut byte).await?;
        if read == 0 {
            bail!("mobile HTTP client closed before headers");
        }
        header_bytes.push(byte[0]);
        if header_bytes.len() > MAX_HEADER_BYTES {
            bail!("mobile HTTP headers exceeded limit");
        }
    }

    let header_text =
        std::str::from_utf8(&header_bytes).context("mobile HTTP headers not UTF-8")?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| anyhow!("missing mobile HTTP request line"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing mobile HTTP method"))?
        .to_ascii_uppercase();
    let target = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing mobile HTTP target"))?
        .to_string();
    let version = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing mobile HTTP version"))?;
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1") || request_parts.next().is_some() {
        bail!("invalid mobile HTTP request line");
    }

    let mut headers = BTreeMap::new();
    let mut content_length = None;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once(':')
            .ok_or_else(|| anyhow!("invalid mobile HTTP header"))?;
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if key.is_empty() {
            bail!("invalid empty mobile HTTP header name");
        }
        if key == "transfer-encoding" {
            bail!("unsupported mobile HTTP Transfer-Encoding");
        }
        if key == "content-length" {
            if content_length.is_some() {
                bail!("duplicate Content-Length header");
            }
            if value.is_empty() || value.starts_with(['+', '-']) {
                bail!("invalid Content-Length header");
            }
            let parsed = value
                .parse::<u64>()
                .context("invalid Content-Length header")?;
            let parsed = usize::try_from(parsed).context("Content-Length overflow")?;
            content_length = Some(parsed);
        }
        headers.insert(key, value.to_string());
    }

    if method == "POST" && content_length.is_none() {
        bail!("missing Content-Length header for POST request");
    }
    let content_length = content_length.unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        bail!("mobile HTTP body exceeded limit");
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body).await?;
    }

    Ok(MobileHttpRequest {
        method,
        target,
        headers,
        body,
    })
}

async fn write_mobile_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: Vec<u8>,
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        409 => "Conflict",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(&body).await?;
    Ok(())
}

async fn write_mobile_response_with_deadline(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: Vec<u8>,
    deadline: Duration,
) -> Result<()> {
    timeout(
        deadline,
        write_mobile_response(stream, status, content_type, body),
    )
    .await
    .context("mobile HTTP write deadline exceeded")?
}

fn mobile_token_from_authorization(value: Option<&str>) -> Option<String> {
    let value = value?;
    value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

fn mobile_token_from_target(target: &str) -> Option<String> {
    mobile_query_value(target, "t").or_else(|| mobile_query_value(target, "token"))
}

fn mobile_query_value(target: &str, expected_key: &str) -> Option<String> {
    let query = target.split_once('?')?.1;
    query.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == expected_key).then(|| percent_decode_query_value(value))
    })
}

fn percent_decode_query_value(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (
                hex_digit_value(bytes[index + 1]),
                hex_digit_value(bytes[index + 2]),
            ) {
                decoded.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(decoded).unwrap_or_else(|_| value.to_string())
}

fn percent_encode_query_value(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push('%');
                encoded.push(HEX[(byte >> 4) as usize] as char);
                encoded.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    encoded
}

fn hex_digit_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn detect_lan_ipv4() -> Option<std::net::Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(addr) if !addr.is_loopback() => Some(addr),
        _ => None,
    }
}

fn render_mobile_page() -> String {
    render_mobile_page_with_token("")
}

fn render_mobile_page_with_token(token: &str) -> String {
    let token_json = serde_json::to_string(token).unwrap_or_else(|_| "\"\"".to_string());

    r#"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
  <title>R-ShareMouse Mobile</title>
  <style>
    :root { color-scheme: dark; font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    * { box-sizing: border-box; }
    html, body { width: 100%; min-height: 100%; margin: 0; overflow: auto; background: #101214; color: #edf2ef; }
    body { overscroll-behavior: none; touch-action: manipulation; -webkit-touch-callout: none; }
    main { min-height: 100dvh; display: flex; flex-direction: column; gap: 12px; padding: 12px; max-width: 720px; margin: 0 auto; }
    header { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
    h1 { margin: 0; font-size: 14px; line-height: 1.2; }
    .sub { margin-top: 3px; font-size: 12px; color: #8f9b96; }
    .status { border: 1px solid #2a302d; border-radius: 6px; padding: 5px 8px; color: #47c27a; font-size: 12px; max-width: 45%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    #pad { flex: 1; min-height: min(260px, 42dvh); border: 1px solid #29302d; border-radius: 6px; background: linear-gradient(135deg, rgba(71,194,122,.08), rgba(255,255,255,.02)); touch-action: none; display: grid; place-items: center; user-select: none; -webkit-user-select: none; -webkit-touch-callout: none; }
    .dot { width: 12px; height: 12px; border-radius: 50%; background: #47c27a; box-shadow: 0 0 24px rgba(71,194,122,.5); }
    .grid3 { display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px; }
    .grid4 { display: grid; grid-template-columns: repeat(4, 1fr); gap: 8px; }
    button, input, textarea { border-radius: 6px; font: inherit; font-size: 14px; }
    button, input { height: 48px; }
    button { border: 1px solid #29302d; background: #171b1d; color: #d8dedb; touch-action: manipulation; user-select: none; -webkit-user-select: none; -webkit-touch-callout: none; }
    button:active { background: rgba(71,194,122,.18); border-color: #47c27a; }
    .textRow { display: flex; gap: 8px; }
    .rangeRow { display: flex; align-items: center; gap: 10px; min-height: 40px; border: 1px solid #29302d; border-radius: 6px; background: #171b1d; padding: 7px 10px; }
    .rangeRow label { flex: 0 0 auto; font-size: 12px; font-weight: 600; }
    .rangeRow input { flex: 1; min-width: 0; height: 28px; accent-color: #47c27a; }
    .rangeValue { width: 42px; text-align: right; font-size: 12px; color: #8f9b96; }
    .inputWrap { min-width: 0; flex: 1; min-height: 76px; display: flex; align-items: flex-start; border: 1px solid #29302d; border-radius: 6px; background: #171b1d; padding: 9px 12px; }
    input, textarea { min-width: 0; flex: 1; border: 0; outline: 0; background: transparent; color: #edf2ef; font-size: 16px; }
    textarea { min-height: 56px; resize: none; line-height: 1.35; }
    .send { width: 58px; background: #47c27a; color: #07110b; border-color: #47c27a; }
  </style>
</head>
<body>
<main>
  <header>
    <div>
      <h1>R-ShareMouse Mobile</h1>
      <div class="sub" id="pos">0, 0 / 1920x1080</div>
      <div class="sub" id="backendStatus">等待输入后端</div>
    </div>
    <div class="status" id="status">连接中</div>
  </header>
  <section id="pad"><div class="dot"></div></section>
  <section class="rangeRow">
    <label for="sensitivity">灵敏度</label>
    <input id="sensitivity" type="range" aria-label="触控板灵敏度" min="0.5" max="3" step="0.05" value="1.35">
    <span class="rangeValue" id="sensitivityValue">1.35</span>
  </section>
  <section class="grid3">
    <button data-button="Left">左键</button>
    <button data-button="Middle">中键</button>
    <button data-button="Right">右键</button>
    <button data-button="Back">后退</button>
    <button data-button="Forward">前进</button>
    <button data-double-click="Left">双击</button>
    <button data-release-all>释放全部</button>
  </section>
  <section class="grid4">
    <button data-wheel="3">上滚</button>
    <button data-wheel="-3">下滚</button>
    <button data-key="Backspace">退格</button>
    <button data-key="Enter">回车</button>
  </section>
  <section class="grid4">
    <button data-key="Left">左</button>
    <button data-key="Up">上</button>
    <button data-key="Down">下</button>
    <button data-key="Right">右</button>
  </section>
  <section class="grid4">
    <button data-key="ControlLeft">Ctrl</button>
    <button data-key="ShiftLeft">Shift</button>
    <button data-key="AltLeft">Alt</button>
    <button data-key="SuperLeft">Win</button>
  </section>
  <section class="grid4">
    <button data-key="Escape">Esc</button>
    <button data-key="Tab">Tab</button>
    <button data-key="Space">Space</button>
    <button data-key="Delete">Del</button>
  </section>
  <section class="grid4">
    <button data-key="Home">Home</button>
    <button data-key="End">End</button>
    <button data-key="PageUp">PgUp</button>
    <button data-key="PageDown">PgDn</button>
  </section>
  <section class="grid4">
    <button data-shortcut="ControlLeft,C">复制</button>
    <button data-shortcut="ControlLeft,V">粘贴</button>
    <button data-shortcut="ControlLeft,X">剪切</button>
    <button data-shortcut="ControlLeft,A">全选</button>
  </section>
  <section class="textRow">
    <div class="inputWrap"><textarea id="text" placeholder="文本" autocomplete="off" autocapitalize="none" autocorrect="off" spellcheck="false" enterkeyhint="send" rows="3"></textarea></div>
    <button class="send" id="send">发送</button>
  </section>
</main>
<script>
const token = __MOBILE_TOKEN_JSON__ || new URLSearchParams(location.search).get("t") || "";
function newMobileClientId() {
  return crypto.randomUUID ? crypto.randomUUID() : `page-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}
let clientId = newMobileClientId();
let mobileSequence = 0;
let inputGeneration = 0;
let inputSuspended = false;
let refreshAbortController = null;
const heldKeys = new Set();
const heldMouseButtons = new Set();
const statusEl = document.getElementById("status");
const posEl = document.getElementById("pos");
const backendStatusEl = document.getElementById("backendStatus");
const pad = document.getElementById("pad");
const textInput = document.getElementById("text");
const sensitivityInput = document.getElementById("sensitivity");
const sensitivityValue = document.getElementById("sensitivityValue");
const mobileEventOptions = { capture: true, passive: false };
let pointer = { x: 0, y: 0, minX: 0, minY: 0, width: 1920, height: 1080, displayId: null };
let displayEntries = [];
let activePointer = null;
let lastPoint = null;
let tapStart = null;
let touchPoints = new Map();
let lastWheelTouches = null;
let twoFingerTapStart = null;
let pendingMove = null;
let pendingMoveFrame = 0;
let dragTimer = 0;
let dragPointer = null;
const LONG_PRESS_DRAG_DELAY_MS = 420;
const POINTER_SENSITIVITY_STORAGE_KEY = "rshare.mobile.pointerSensitivity";
const POINTER_SENSITIVITY_DEFAULT = 1.35;
const POINTER_SENSITIVITY_MIN = 0.5;
const POINTER_SENSITIVITY_MAX = 3;
const POINTER_SENSITIVITY_STEP = 0.05;
function clampPointerSensitivity(value) {
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) return POINTER_SENSITIVITY_DEFAULT;
  const clamped = Math.max(POINTER_SENSITIVITY_MIN, Math.min(POINTER_SENSITIVITY_MAX, parsed));
  return Number((Math.round(clamped / POINTER_SENSITIVITY_STEP) * POINTER_SENSITIVITY_STEP).toFixed(2));
}
function loadPointerSensitivity() {
  try {
    return clampPointerSensitivity(localStorage.getItem(POINTER_SENSITIVITY_STORAGE_KEY));
  } catch {
    return POINTER_SENSITIVITY_DEFAULT;
  }
}
let pointerSensitivity = clampPointerSensitivity(loadPointerSensitivity());
function setPointerSensitivity(value) {
  pointerSensitivity = clampPointerSensitivity(value);
  sensitivityInput.value = String(pointerSensitivity);
  sensitivityValue.textContent = pointerSensitivity.toFixed(2);
  try {
    localStorage.setItem(POINTER_SENSITIVITY_STORAGE_KEY, String(pointerSensitivity));
  } catch {}
}
sensitivityInput.value = String(pointerSensitivity);
sensitivityValue.textContent = pointerSensitivity.toFixed(2);
sensitivityInput.addEventListener("input", (event) => setPointerSensitivity(event.target.value));
function cid(prefix) {
  return `${prefix}-${crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random().toString(16).slice(2)}`}`;
}
function mobileEnvelope(request) {
  return { client_id: clientId, sequence: ++mobileSequence, request };
}
function mobileInjectPayload(request) {
  return request?.InjectEndpointEvent?.request?.payload || null;
}
function trackHeldInputBeforeInject(request) {
  const payload = mobileInjectPayload(request);
  const kind = String(payload?.kind || "");
  const data = payload?.data || {};
  const state = String(data.state || "").toLowerCase();
  if (state !== "pressed") return;
  if (kind === "Keyboard" && data.key) heldKeys.add(String(data.key));
  if (kind === "MouseButton" && data.button) heldMouseButtons.add(String(data.button));
}
function clearReleasedHeldInput(request, accepted) {
  if (!accepted) return;
  const payload = mobileInjectPayload(request);
  const kind = String(payload?.kind || "");
  const data = payload?.data || {};
  const state = String(data.state || "").toLowerCase();
  if (state !== "released") return;
  if (kind === "Keyboard" && data.key) heldKeys.delete(String(data.key));
  if (kind === "MouseButton" && data.button) heldMouseButtons.delete(String(data.button));
}
function formatMobileError(error, scope = "移动端") {
  const message = error instanceof Error ? error.message : String(error || "");
  if (/failed to fetch|networkerror|fetch failed|load failed/i.test(message)) {
    return `${scope}网关不可用，请确认桌面服务正在运行并且手机与电脑在同一网络`;
  }
  return `${scope}请求失败：${message || "未知错误"}`;
}
function editableMobileTarget(target) {
  if (!target || typeof target !== "object") return false;
  const tagName = String(target.tagName || "").toUpperCase();
  if (tagName === "INPUT" || tagName === "TEXTAREA" || tagName === "SELECT") return true;
  if (target.isContentEditable === true) return true;
  return Boolean(target.closest && target.closest("input, textarea, select, [contenteditable='true']"));
}
function displayIdForPointer(displays, x, y, fallbackId = null) {
  if (!Array.isArray(displays)) return fallbackId || null;
  for (const display of displays) {
    if (!display || typeof display !== "object") continue;
    const displayId = display.display_id || display.id;
    const left = Number(display.x || 0);
    const top = Number(display.y || 0);
    const width = Number(display.width || display.w || 0);
    const height = Number(display.height || display.h || 0);
    if (displayId && Number.isFinite(left) && Number.isFinite(top) && Number.isFinite(width) && Number.isFinite(height) && width > 0 && height > 0 && x >= left && x < left + width && y >= top && y < top + height) {
      return String(displayId);
    }
  }
  return fallbackId || null;
}
function shouldPreventBrowserNavigationEvent(event) {
  const type = String(event?.type || "");
  const button = Number(event?.button);
  if (/^(mouse|pointer|auxclick)/i.test(type) && Number.isFinite(button) && (button === 3 || button === 4)) return true;
  if (type === "keydown") {
    const key = String(event?.key || "");
    if (key === "BrowserBack" || key === "BrowserForward") return true;
    if (event?.altKey && (key === "ArrowLeft" || key === "ArrowRight")) return true;
  }
  return false;
}
function preventBrowserNavigationEvent(event) {
  if (!shouldPreventBrowserNavigationEvent(event)) return false;
  event.preventDefault();
  event.returnValue = false;
  return true;
}
function preventMobileGestureDefault(event) {
  if (editableMobileTarget(event.target)) return false;
  if (!["contextmenu", "dragstart", "selectstart", "gesturestart", "gesturechange", "gestureend"].includes(String(event.type || "").toLowerCase())) return false;
  event.preventDefault();
  event.returnValue = false;
  return true;
}
["mousedown", "mouseup", "auxclick", "pointerdown", "pointerup", "keydown"].forEach((eventName) => {
  window.addEventListener(eventName, preventBrowserNavigationEvent, mobileEventOptions);
});
["contextmenu", "dragstart", "selectstart", "gesturestart", "gesturechange", "gestureend"].forEach((eventName) => {
  document.addEventListener(eventName, preventMobileGestureDefault, mobileEventOptions);
});
async function api(path, options = {}) {
  const headers = { Authorization: `Bearer ${token}`, ...(options.headers || {}) };
  const response = await fetch(path, { ...options, headers });
  const payload = await response.json().catch(() => null);
  if (!response.ok) throw new Error(payload?.error || `HTTP ${response.status}`);
  return payload;
}
function daemonRequest(deviceKind, payload, correlationId, mode = "BestEffort", timeoutMs = 250) {
  return {
    InjectEndpointEvent: {
      target: "Local",
      request: { correlation_id: correlationId, device_kind: deviceKind, payload, mode, timeout_ms: timeoutMs }
    }
  };
}
function backendHealthReason(health) {
  if (typeof health === "string") return health;
  if (health?.Degraded && typeof health.Degraded === "object") return String(health.Degraded.reason || "Degraded");
  if (health && typeof health === "object") return String(Object.keys(health)[0] || "未知状态");
  return "未知状态";
}
function formatBackendStatus(snapshot) {
  const backend = snapshot.inject_backend || {};
  if (backend.active == null && backend.health == null) {
    return { state: "pending", label: "等待输入后端", detail: "尚未收到注入后端状态" };
  }
  const kind = String(backend.kind || backend.mode || "未知后端");
  if (backend.active === true || backend.health === "Healthy") {
    return { state: "ready", label: "输入注入就绪", detail: kind };
  }
  return { state: "blocked", label: "输入注入不可用", detail: `${kind}: ${backendHealthReason(backend.health)}` };
}
function formatEndpointInjectError(error) {
  switch (String(error || "")) {
    case "BackendUnavailable": return "输入后端不可用";
    case "BackendDegraded": return "输入后端异常";
    case "PermissionDenied": return "权限不足";
    case "UnsupportedEvent": return "当前输入事件不支持";
    case "TargetDisconnected": return "目标设备已断开";
    case "Timeout": return "注入超时";
    case "RejectedByPolicy": return "请求被策略拒绝";
    case "TransportFailed": return "传输失败";
    case "Failed": return "注入失败";
    default: return "未知错误";
  }
}
function formatInjectResultStatus(result) {
  const injectResult = result?.EndpointInjectResult || result || {};
  if (injectResult.accepted !== false) {
    return { accepted: true, status: "已连接" };
  }
  const backendKind = injectResult.backend_kind ? ` · ${injectResult.backend_kind}` : "";
  return { accepted: false, status: `注入失败：${formatEndpointInjectError(injectResult.error)}${backendKind}` };
}
function prepareOrdinaryInjectEnvelope(request, expectedGeneration) {
  if (inputSuspended || expectedGeneration !== inputGeneration) return null;
  trackHeldInputBeforeInject(request);
  return mobileEnvelope(request);
}
async function inject(request, expectedGeneration = inputGeneration) {
  const envelope = prepareOrdinaryInjectEnvelope(request, expectedGeneration);
  if (!envelope) return false;
  try {
    const result = await api("/api/inject", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(envelope)
    });
    if (inputSuspended || expectedGeneration !== inputGeneration) return false;
    const feedback = formatInjectResultStatus(result);
    clearReleasedHeldInput(request, feedback.accepted);
    statusEl.textContent = feedback.status;
    return feedback.accepted;
  } catch (error) {
    if (inputSuspended || expectedGeneration !== inputGeneration) return false;
    statusEl.textContent = formatMobileError(error, "移动端注入");
    return false;
  }
}
async function refresh() {
  if (inputSuspended) return;
  if (refreshAbortController) refreshAbortController.abort();
  const controller = new AbortController();
  refreshAbortController = controller;
  const pollingClientId = clientId;
  const pollingGeneration = inputGeneration;
  try {
    const payload = await api(`/api/local-controls?client_id=${encodeURIComponent(pollingClientId)}`, { signal: controller.signal });
    if (inputSuspended || pollingGeneration !== inputGeneration || pollingClientId !== clientId) return;
    const snapshot = payload.LocalControls || {};
    const mouse = snapshot.mouse || {};
    const display = snapshot.display || {};
    displayEntries = Array.isArray(display.displays) ? display.displays : [];
    const primary = displayEntries.find((item) => item.primary) || displayEntries[0] || {};
    const minX = Number(display.virtual_x ?? primary.x ?? 0);
    const minY = Number(display.virtual_y ?? primary.y ?? 0);
    const layoutWidth = Number(display.layout_width ?? display.primary_width ?? primary.width ?? primary.w ?? 1920);
    const layoutHeight = Number(display.layout_height ?? display.primary_height ?? primary.height ?? primary.h ?? 1080);
    pointer = {
      x: Number(mouse.x || 0),
      y: Number(mouse.y || 0),
      minX: Number.isFinite(minX) ? Math.floor(minX) : 0,
      minY: Number.isFinite(minY) ? Math.floor(minY) : 0,
      width: Math.max(1, Math.floor(Number.isFinite(layoutWidth) ? layoutWidth : 1920)),
      height: Math.max(1, Math.floor(Number.isFinite(layoutHeight) ? layoutHeight : 1080)),
      displayId: displayIdForPointer(displayEntries, Number(mouse.x || 0), Number(mouse.y || 0), mouse.current_display_id || primary.display_id || primary.id || null)
    };
    posEl.textContent = `${pointer.x}, ${pointer.y} / ${pointer.width}x${pointer.height}`;
    const backendStatus = formatBackendStatus(snapshot);
    backendStatusEl.textContent = `${backendStatus.label} · ${backendStatus.detail}`;
    backendStatusEl.style.color = backendStatus.state === "ready" ? '#47c27a' : '#d6a64b';
    statusEl.textContent = "已连接";
  } catch (error) {
    if (error?.name === "AbortError") return;
    statusEl.textContent = formatMobileError(error, "移动端状态");
  } finally {
    if (refreshAbortController === controller) refreshAbortController = null;
  }
}
function sendMoveNow(next) {
  inject(daemonRequest("Mouse", { kind: "MouseMove", data: { x: next.x, y: next.y, display_id: next.displayId } }, cid("mobile-move")));
}
function scheduleMove(next) {
  pendingMove = { ...next };
  if (pendingMoveFrame) return;
  pendingMoveFrame = requestAnimationFrame(() => {
    pendingMoveFrame = 0;
    const next = pendingMove;
    pendingMove = null;
    if (next) sendMoveNow(next);
  });
}
function flushMove() {
  const next = pendingMove;
  pendingMove = null;
  if (pendingMoveFrame) {
    cancelAnimationFrame(pendingMoveFrame);
    pendingMoveFrame = 0;
  }
  if (next) sendMoveNow(next);
}
function isTouchpadTap(start, end) {
  if (!start || !end) return false;
  const duration = end.timeMs - start.timeMs;
  if (duration < 0 || duration > 260) return false;
  return Math.hypot(end.x - start.x, end.y - start.y) <= 12;
}
function isTouchpadLongPressDrag(start, current) {
  if (!start || !current) return false;
  const duration = current.timeMs - start.timeMs;
  if (duration < LONG_PRESS_DRAG_DELAY_MS) return false;
  return Math.hypot(current.x - start.x, current.y - start.y) <= 12;
}
async function sendTapClick() {
  const generation = inputGeneration;
  await inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: "Left", state: "Pressed", x: pointer.x, y: pointer.y } }, cid("mobile-tap-down")), generation);
  await inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: "Left", state: "Released", x: pointer.x, y: pointer.y } }, cid("mobile-tap-up")), generation);
}
async function sendDoubleClick(buttonName) {
  const generation = inputGeneration;
  if (buttonName === "Left") {
    await inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: "Left", state: "Pressed", x: pointer.x, y: pointer.y } }, cid("mobile-double-Left-1-down")), generation);
    await inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: "Left", state: "Released", x: pointer.x, y: pointer.y } }, cid("mobile-double-Left-1-up")), generation);
    await inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: "Left", state: "Pressed", x: pointer.x, y: pointer.y } }, cid("mobile-double-Left-2-down")), generation);
    await inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: "Left", state: "Released", x: pointer.x, y: pointer.y } }, cid("mobile-double-Left-2-up")), generation);
    return;
  }
  await inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: buttonName, state: "Pressed", x: pointer.x, y: pointer.y } }, cid(`mobile-double-${buttonName}-1-down`)), generation);
  await inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: buttonName, state: "Released", x: pointer.x, y: pointer.y } }, cid(`mobile-double-${buttonName}-1-up`)), generation);
  await inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: buttonName, state: "Pressed", x: pointer.x, y: pointer.y } }, cid(`mobile-double-${buttonName}-2-down`)), generation);
  await inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: buttonName, state: "Released", x: pointer.x, y: pointer.y } }, cid(`mobile-double-${buttonName}-2-up`)), generation);
}
function releaseAllRequests(prefix) {
  const mouseButtons = [
    ["Left", `${prefix}-mouse-left`],
    ["Middle", `${prefix}-mouse-middle`],
    ["Right", `${prefix}-mouse-right`],
    ["Back", `${prefix}-mouse-back`],
    ["Forward", `${prefix}-mouse-forward`]
  ];
  const modifierKeys = [
    ["ControlLeft", `${prefix}-key-controlleft`],
    ["ShiftLeft", `${prefix}-key-shiftleft`],
    ["AltLeft", `${prefix}-key-altleft`],
    ["SuperLeft", `${prefix}-key-superleft`]
  ];
  const knownMouseButtons = new Set(mouseButtons.map(([buttonName]) => buttonName));
  const knownModifierKeys = new Set(modifierKeys.map(([key]) => key));
  return [
    ...mouseButtons.map(([buttonName, correlationId]) => daemonRequest("Mouse", { kind: "MouseButton", data: { button: buttonName, state: "Released", x: pointer.x, y: pointer.y } }, cid(correlationId))),
    ...modifierKeys.map(([key, correlationId]) => daemonRequest("Keyboard", { kind: "Keyboard", data: { key, state: "Released" } }, cid(correlationId), "BestEffort", 750)),
    ...Array.from(heldMouseButtons).filter((buttonName) => !knownMouseButtons.has(buttonName)).map((buttonName) => daemonRequest("Mouse", { kind: "MouseButton", data: { button: buttonName, state: "Released", x: pointer.x, y: pointer.y } }, cid(`${prefix}-mouse-${buttonName.toLowerCase()}`))),
    ...Array.from(heldKeys).filter((key) => !knownModifierKeys.has(key)).map((key) => daemonRequest("Keyboard", { kind: "Keyboard", data: { key, state: "Released" } }, cid(`${prefix}-key-${key.toLowerCase()}`), "BestEffort", 750))
  ];
}
async function sendReleaseAll() {
  const generation = inputGeneration;
  for (const request of releaseAllRequests("mobile-release-all")) {
    await inject(request, generation);
  }
}
function keepaliveReleaseBatch(requests) {
  if (!Array.isArray(requests) || requests.length === 0) return false;
  const body = JSON.stringify({ client_id: clientId, sequence: ++mobileSequence, requests: requests });
  const path = `/api/inject?t=${encodeURIComponent(token)}`;
  try {
    if (navigator.sendBeacon) {
      const blob = new Blob([body], { type: "application/json" });
      if (navigator.sendBeacon(path, blob)) return true;
    }
  } catch {}
  try {
    fetch(path, {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
      body,
      keepalive: true
    }).catch(() => {});
    return true;
  } catch {
    return false;
  }
}
function releaseAllWithKeepalive() {
  inputSuspended = true;
  inputGeneration += 1;
  if (refreshAbortController) {
    refreshAbortController.abort();
    refreshAbortController = null;
  }
  keepaliveReleaseBatch(releaseAllRequests("mobile-release-all-keepalive"));
}
function clearDragTimer() {
  if (dragTimer) {
    clearTimeout(dragTimer);
    dragTimer = 0;
  }
}
function sendDragButton(state) {
  inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: "Left", state, x: pointer.x, y: pointer.y } }, cid(`mobile-touchpad-drag-${state}`)));
}
function beginTouchpadDrag(pointerId) {
  dragTimer = 0;
  if (activePointer !== pointerId || dragPointer !== null || touchPoints.size !== 1) return;
  const current = touchPoints.get(pointerId);
  if (!tapStart || !current) return;
  if (!isTouchpadLongPressDrag(tapStart, { ...current, timeMs: tapStart.timeMs + LONG_PRESS_DRAG_DELAY_MS })) return;
  dragPointer = pointerId;
  tapStart = null;
  sendDragButton("Pressed");
}
function releaseTouchpadDrag(pointerId = null) {
  if (dragPointer === null) return;
  if (pointerId !== null && dragPointer !== pointerId) return;
  dragPointer = null;
  sendDragButton("Released");
}
function releaseTouchpadInteraction() {
  clearDragTimer();
  flushMove();
  releaseTouchpadDrag();
  activePointer = null;
  lastPoint = null;
  tapStart = null;
  lastWheelTouches = null;
  twoFingerTapStart = null;
  touchPoints.clear();
}
function releaseTouchpadInteractionWhenHidden() {
  if (document.visibilityState === "hidden") {
    releaseTouchpadInteraction();
  }
}
function releaseAllWithKeepaliveWhenHidden() {
  if (document.visibilityState === "hidden") releaseAllWithKeepalive();
}
function rotateMobileClientSession() {
  clientId = newMobileClientId();
  mobileSequence = 0;
  inputGeneration += 1;
  heldKeys.clear();
  heldMouseButtons.clear();
}
function resumeMobileInput() {
  if (!inputSuspended) return;
  rotateMobileClientSession();
  inputSuspended = false;
  refresh();
}
function resumeMobileInputWhenVisible() {
  if (document.visibilityState === "visible") resumeMobileInput();
}
function touchPointsSnapshot() {
  return Array.from(touchPoints.values()).sort((left, right) => left.id - right.id);
}
function centerOfTouches(touches) {
  return { x: (touches[0].x + touches[1].x) / 2, y: (touches[0].y + touches[1].y) / 2 };
}
function twoFingerWheelDelta(previousTouches, currentTouches) {
  if (!previousTouches || !currentTouches || previousTouches.length !== 2 || currentTouches.length !== 2) return null;
  if (previousTouches[0].id !== currentTouches[0].id || previousTouches[1].id !== currentTouches[1].id) return null;
  const previousCenter = centerOfTouches(previousTouches);
  const currentCenter = centerOfTouches(currentTouches);
  const dx = currentCenter.x - previousCenter.x;
  const dy = currentCenter.y - previousCenter.y;
  if (Math.max(Math.abs(dx), Math.abs(dy)) < 6) return null;
  const wheel = { deltaX: Math.round(dx * 0.12), deltaY: Math.round(dy * 0.12) };
  return wheel.deltaX || wheel.deltaY ? wheel : null;
}
function isTwoFingerTap(startTouches, endTouches, startTimeMs, endTimeMs) {
  if (!startTouches || !endTouches || startTouches.length !== 2 || endTouches.length !== 2) return false;
  if (startTouches[0].id !== endTouches[0].id || startTouches[1].id !== endTouches[1].id) return false;
  const duration = endTimeMs - startTimeMs;
  if (duration < 0 || duration > 260) return false;
  const startCenter = centerOfTouches(startTouches);
  const endCenter = centerOfTouches(endTouches);
  if (Math.hypot(endCenter.x - startCenter.x, endCenter.y - startCenter.y) > 12) return false;
  const startDistance = Math.hypot(startTouches[1].x - startTouches[0].x, startTouches[1].y - startTouches[0].y);
  const endDistance = Math.hypot(endTouches[1].x - endTouches[0].x, endTouches[1].y - endTouches[0].y);
  return Math.abs(endDistance - startDistance) <= 12;
}
function sendWheelDelta(wheel) {
  inject(daemonRequest("Mouse", { kind: "MouseWheel", data: { delta_x: wheel.deltaX, delta_y: wheel.deltaY, x: pointer.x, y: pointer.y } }, cid("mobile-wheel")));
}
async function sendTwoFingerTapClick() {
  const generation = inputGeneration;
  await inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: "Right", state: "Pressed", x: pointer.x, y: pointer.y } }, cid("mobile-two-finger-tap-down")), generation);
  await inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: "Right", state: "Released", x: pointer.x, y: pointer.y } }, cid("mobile-two-finger-tap-up")), generation);
}
pad.addEventListener("pointerdown", (event) => {
  touchPoints.set(event.pointerId, { id: event.pointerId, x: event.clientX, y: event.clientY });
  activePointer = event.pointerId;
  lastPoint = { x: event.clientX, y: event.clientY };
  tapStart = { x: event.clientX, y: event.clientY, timeMs: event.timeStamp };
  pad.setPointerCapture(event.pointerId);
  clearDragTimer();
  dragTimer = setTimeout(() => beginTouchpadDrag(event.pointerId), LONG_PRESS_DRAG_DELAY_MS);
  if (touchPoints.size >= 2) {
    clearDragTimer();
    releaseTouchpadDrag();
    flushMove();
    const touches = touchPoints.size === 2 ? touchPointsSnapshot() : null;
    lastWheelTouches = touches;
    twoFingerTapStart = touches ? { touches, timeMs: event.timeStamp } : null;
    activePointer = null;
    lastPoint = null;
    tapStart = null;
  }
});
pad.addEventListener("pointermove", (event) => {
  if (touchPoints.has(event.pointerId)) {
    touchPoints.set(event.pointerId, { id: event.pointerId, x: event.clientX, y: event.clientY });
  }
  if (touchPoints.size >= 2) {
    clearDragTimer();
    releaseTouchpadDrag();
    if (touchPoints.size > 2) {
      lastWheelTouches = null;
      twoFingerTapStart = null;
      return;
    }
    const currentTouches = touchPointsSnapshot();
    const wheel = twoFingerWheelDelta(lastWheelTouches, currentTouches);
    lastWheelTouches = currentTouches;
    if (wheel) {
      twoFingerTapStart = null;
      sendWheelDelta(wheel);
    }
    return;
  }
  if (activePointer !== event.pointerId || !lastPoint) return;
  if (tapStart && Math.hypot(event.clientX - tapStart.x, event.clientY - tapStart.y) > 12) {
    clearDragTimer();
  }
  const dx = Math.round((event.clientX - lastPoint.x) * pointerSensitivity);
  const dy = Math.round((event.clientY - lastPoint.y) * pointerSensitivity);
  lastPoint = { x: event.clientX, y: event.clientY };
  const maxX = pointer.minX + pointer.width - 1;
  const maxY = pointer.minY + pointer.height - 1;
  pointer = { ...pointer, x: Math.max(pointer.minX, Math.min(maxX, pointer.x + dx)), y: Math.max(pointer.minY, Math.min(maxY, pointer.y + dy)) };
  pointer.displayId = displayIdForPointer(displayEntries, pointer.x, pointer.y, pointer.displayId);
  posEl.textContent = `${pointer.x}, ${pointer.y} / ${pointer.width}x${pointer.height}`;
  scheduleMove(pointer);
});
function clearPointer(event) {
  if (touchPoints.has(event.pointerId)) {
    touchPoints.set(event.pointerId, { id: event.pointerId, x: event.clientX, y: event.clientY });
  }
  if (touchPoints.size === 2 && twoFingerTapStart) {
    const start = twoFingerTapStart;
    const currentTouches = touchPointsSnapshot();
    twoFingerTapStart = null;
    if (isTwoFingerTap(start.touches, currentTouches, start.timeMs, event.timeStamp)) {
      sendTwoFingerTapClick();
    }
  }
  if (activePointer === event.pointerId) {
    clearDragTimer();
    flushMove();
    if (dragPointer === event.pointerId) {
      releaseTouchpadDrag(event.pointerId);
    } else if (isTouchpadTap(tapStart, { x: event.clientX, y: event.clientY, timeMs: event.timeStamp })) {
      sendTapClick();
    }
    activePointer = null;
    lastPoint = null;
    tapStart = null;
  }
  touchPoints.delete(event.pointerId);
  if (touchPoints.size !== 2) {
    lastWheelTouches = null;
    twoFingerTapStart = null;
  }
}
function cancelPointer(event) {
  if (activePointer === event.pointerId) {
    clearDragTimer();
    releaseTouchpadDrag(event.pointerId);
    flushMove();
    activePointer = null;
    lastPoint = null;
    tapStart = null;
  }
  touchPoints.delete(event.pointerId);
  if (touchPoints.size !== 2) {
    lastWheelTouches = null;
    twoFingerTapStart = null;
  }
}
pad.addEventListener("pointerup", clearPointer);
pad.addEventListener("pointercancel", cancelPointer);
window.addEventListener("blur", releaseTouchpadInteraction);
window.addEventListener("pagehide", releaseTouchpadInteraction);
document.addEventListener("visibilitychange", releaseTouchpadInteractionWhenHidden);
function attachHeldButton(button, sendState) {
  let activePointer = null;
  function resetHeldButtonPointerState() {
    activePointer = null;
  }
  function release(force, pointerId = null) {
    if (activePointer === null) return;
    if (!force && pointerId !== null && activePointer !== pointerId) return;
    activePointer = null;
    sendState("Released");
  }
  button.addEventListener("pointerdown", (event) => {
    release(true);
    activePointer = event.pointerId;
    button.setPointerCapture(event.pointerId);
    sendState("Pressed");
  });
  button.addEventListener("pointerup", (event) => release(false, event.pointerId));
  button.addEventListener("pointercancel", (event) => release(false, event.pointerId));
  button.addEventListener("pointerleave", (event) => {
    if (event.buttons) release(false, event.pointerId);
  });
  window.addEventListener("blur", resetHeldButtonPointerState);
  window.addEventListener("pagehide", resetHeldButtonPointerState);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") resetHeldButtonPointerState();
  });
}
document.querySelectorAll("[data-button]").forEach((button) => {
  const name = button.dataset.button;
  const sendButton = (state) => inject(daemonRequest("Mouse", { kind: "MouseButton", data: { button: name, state, x: pointer.x, y: pointer.y } }, cid(`mobile-${name}-${state}`)));
  attachHeldButton(button, (state) => {
    if (state === "Pressed") return sendButton("Pressed");
    return sendButton("Released");
  });
});
document.querySelectorAll("[data-double-click]").forEach((button) => button.addEventListener("click", () => {
  sendDoubleClick(button.dataset.doubleClick || "Left");
}));
document.querySelectorAll("[data-release-all]").forEach((button) => button.addEventListener("click", () => {
  sendReleaseAll();
}));
document.querySelectorAll("[data-wheel]").forEach((button) => button.addEventListener("click", () => {
  inject(daemonRequest("Mouse", { kind: "MouseWheel", data: { delta_x: 0, delta_y: Number(button.dataset.wheel), x: pointer.x, y: pointer.y } }, cid("mobile-wheel")));
}));
function sendKeyState(button, state) {
  const key = button.dataset.key;
  return inject(daemonRequest("Keyboard", { kind: "Keyboard", data: { key, state } }, cid(`mobile-${key}-${state}`), "RequireHealthyBackend", 750));
}
async function sendKeyChord(keys) {
  const generation = inputGeneration;
  for (const key of keys) {
    await inject(daemonRequest("Keyboard", { kind: "Keyboard", data: { key, state: "Pressed" } }, cid(`mobile-shortcut-${key}-down`), "RequireHealthyBackend", 750), generation);
  }
  for (const key of [...keys].reverse()) {
    await inject(daemonRequest("Keyboard", { kind: "Keyboard", data: { key, state: "Released" } }, cid(`mobile-shortcut-${key}-up`), "RequireHealthyBackend", 750), generation);
  }
}
document.querySelectorAll("[data-key]").forEach((button) => {
  attachHeldButton(button, (state) => {
    if (state === "Pressed") return sendKeyState(button, "Pressed");
    return sendKeyState(button, "Released");
  });
});
document.querySelectorAll("[data-shortcut]").forEach((button) => button.addEventListener("click", () => {
  const keys = String(button.dataset.shortcut || "").split(",").filter(Boolean);
  sendKeyChord(keys);
}));
async function sendText() {
  const text = textInput.value;
  if (!text) return;
  if (await inject(daemonRequest("Keyboard", { kind: "TextCommit", data: { text } }, cid("mobile-text"), "RequireHealthyBackend", 750))) {
    textInput.value = "";
  }
}
function shouldSendTextOnKeydown(event) {
  return event.key === "Enter" && !event.shiftKey && !event.isComposing && event.keyCode !== 229;
}
document.getElementById("send").addEventListener("click", sendText);
textInput.addEventListener("keydown", (event) => { if (shouldSendTextOnKeydown(event)) { event.preventDefault(); sendText(); } });
window.addEventListener("blur", releaseAllWithKeepalive);
window.addEventListener("pagehide", releaseAllWithKeepalive);
document.addEventListener("visibilitychange", releaseAllWithKeepaliveWhenHidden);
window.addEventListener("focus", resumeMobileInput);
window.addEventListener("pageshow", resumeMobileInput);
document.addEventListener("visibilitychange", resumeMobileInputWhenVisible);
refresh();
setInterval(refresh, 1500);
</script>
</body>
</html>
"#
    .replace("__MOBILE_TOKEN_JSON__", &token_json)
}

fn mobile_token_query(token: &str) -> String {
    if token.is_empty() {
        String::new()
    } else {
        format!("?t={}", percent_encode_query_value(token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rshare_core::{
        BackendFailureReason, BackendHealth, BackendKind, DeviceId, EndpointEventKind,
        EndpointEventPayload, EndpointInjectMode, EndpointInjectRequest, ServiceStatusSnapshot,
    };
    use rshare_input::InputEvent;
    use std::fmt;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Notify;
    use tokio::time::{sleep, timeout, Duration};

    async fn connected_tcp_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        (client, server)
    }

    async fn post_mobile_json(addr: SocketAddr, body: Vec<u8>) -> Vec<u8> {
        let mut client = TcpStream::connect(addr).await.unwrap();
        let request = format!(
            "POST /api/inject?t=mobile-secret HTTP/1.1\r\nHost: mobile\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        client.write_all(request.as_bytes()).await.unwrap();
        client.write_all(&body).await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        response
    }

    struct NoopInjectBackend;

    impl fmt::Debug for NoopInjectBackend {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("NoopInjectBackend").finish()
        }
    }

    impl InjectBackend for NoopInjectBackend {
        fn kind(&self) -> BackendKind {
            BackendKind::Portable
        }

        fn health(&self) -> BackendHealth {
            BackendHealth::Healthy
        }

        fn inject(&mut self, _event: InputEvent) -> Result<()> {
            Ok(())
        }

        fn is_active(&self) -> bool {
            true
        }
    }

    #[derive(Debug)]
    struct RecordingInjectBackend {
        injected: Arc<StdMutex<Vec<InputEvent>>>,
    }

    impl InjectBackend for RecordingInjectBackend {
        fn kind(&self) -> BackendKind {
            BackendKind::Portable
        }

        fn health(&self) -> BackendHealth {
            BackendHealth::Healthy
        }

        fn inject(&mut self, event: InputEvent) -> Result<()> {
            self.injected.lock().unwrap().push(event);
            Ok(())
        }

        fn is_active(&self) -> bool {
            true
        }
    }

    #[derive(Debug)]
    struct SignalingInjectBackend {
        injected: Arc<StdMutex<Vec<InputEvent>>>,
        pressed_injected: Arc<Notify>,
    }

    impl InjectBackend for SignalingInjectBackend {
        fn kind(&self) -> BackendKind {
            BackendKind::Portable
        }

        fn health(&self) -> BackendHealth {
            BackendHealth::Healthy
        }

        fn inject(&mut self, event: InputEvent) -> Result<()> {
            let pressed = matches!(
                event,
                InputEvent::Key {
                    state: rshare_input::ButtonState::Pressed,
                    ..
                } | InputEvent::MouseButton {
                    state: rshare_input::ButtonState::Pressed,
                    ..
                }
            );
            self.injected.lock().unwrap().push(event);
            if pressed {
                self.pressed_injected.notify_one();
            }
            Ok(())
        }

        fn is_active(&self) -> bool {
            true
        }
    }

    #[derive(Debug)]
    struct RejectingInjectBackend {
        attempted: Arc<StdMutex<Vec<InputEvent>>>,
    }

    impl InjectBackend for RejectingInjectBackend {
        fn kind(&self) -> BackendKind {
            BackendKind::Portable
        }

        fn health(&self) -> BackendHealth {
            BackendHealth::Healthy
        }

        fn inject(&mut self, event: InputEvent) -> Result<()> {
            self.attempted.lock().unwrap().push(event);
            bail!("test backend rejection")
        }

        fn is_active(&self) -> bool {
            true
        }
    }

    #[derive(Debug)]
    struct RejectSecondMousePressBackend {
        injected: Arc<StdMutex<Vec<InputEvent>>>,
        mouse_press_count: usize,
    }

    impl InjectBackend for RejectSecondMousePressBackend {
        fn kind(&self) -> BackendKind {
            BackendKind::Portable
        }

        fn health(&self) -> BackendHealth {
            BackendHealth::Healthy
        }

        fn inject(&mut self, event: InputEvent) -> Result<()> {
            let mouse_pressed = matches!(
                event,
                InputEvent::MouseButton {
                    state: rshare_input::ButtonState::Pressed,
                    ..
                }
            );
            self.injected.lock().unwrap().push(event);
            if mouse_pressed {
                self.mouse_press_count += 1;
                if self.mouse_press_count == 2 {
                    bail!("reject second mouse press")
                }
            }
            Ok(())
        }

        fn is_active(&self) -> bool {
            true
        }
    }

    #[derive(Debug)]
    struct DegradingInjectBackend {
        active: Arc<AtomicBool>,
        injected: Arc<StdMutex<Vec<InputEvent>>>,
    }

    impl InjectBackend for DegradingInjectBackend {
        fn kind(&self) -> BackendKind {
            BackendKind::Portable
        }

        fn health(&self) -> BackendHealth {
            if self.active.load(Ordering::SeqCst) {
                BackendHealth::Healthy
            } else {
                BackendHealth::Degraded {
                    reason: BackendFailureReason::Unavailable,
                }
            }
        }

        fn inject(&mut self, event: InputEvent) -> Result<()> {
            self.injected.lock().unwrap().push(event);
            Ok(())
        }

        fn is_active(&self) -> bool {
            self.active.load(Ordering::SeqCst)
        }
    }

    fn test_mobile_runtime(
        backend: Box<dyn InjectBackend>,
    ) -> (
        Arc<RwLock<DaemonState>>,
        Arc<Mutex<NetworkManager>>,
        Arc<Mutex<Box<dyn InjectBackend>>>,
        broadcast::Sender<LocalInputDiagnosticEvent>,
    ) {
        let state = Arc::new(RwLock::new(DaemonState::new(ServiceStatusSnapshot::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local-host".to_string(),
            "0.0.0.0:27431".to_string(),
            27432,
            42,
        ))));
        let network_manager = Arc::new(Mutex::new(NetworkManager::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local-host".to_string(),
        )));
        let inject_backend = Arc::new(Mutex::new(backend));
        let (local_events_tx, _) = broadcast::channel(32);
        (state, network_manager, inject_backend, local_events_tx)
    }

    fn keyboard_envelope(
        client_id: &str,
        sequence: u64,
        key: &str,
        state: &str,
    ) -> MobileInjectEnvelope {
        MobileInjectEnvelope {
            client_id: client_id.to_string(),
            sequence,
            request: DaemonRequest::InjectEndpointEvent {
                target: EndpointInjectTarget::Local,
                request: EndpointInjectRequest {
                    correlation_id: format!("{client_id}-{sequence}"),
                    device_kind: EndpointEventKind::Keyboard,
                    payload: EndpointEventPayload::Keyboard {
                        key: key.to_string(),
                        state: state.to_string(),
                    },
                    mode: EndpointInjectMode::RequireHealthyBackend,
                    timeout_ms: 750,
                },
            },
        }
    }

    fn mouse_button_envelope(
        client_id: &str,
        sequence: u64,
        button: &str,
        state: &str,
        x: i32,
        y: i32,
    ) -> MobileInjectEnvelope {
        MobileInjectEnvelope {
            client_id: client_id.to_string(),
            sequence,
            request: DaemonRequest::InjectEndpointEvent {
                target: EndpointInjectTarget::Local,
                request: EndpointInjectRequest {
                    correlation_id: format!("{client_id}-{sequence}"),
                    device_kind: EndpointEventKind::Mouse,
                    payload: EndpointEventPayload::MouseButton {
                        button: button.to_string(),
                        state: state.to_string(),
                        x,
                        y,
                    },
                    mode: EndpointInjectMode::RequireHealthyBackend,
                    timeout_ms: 750,
                },
            },
        }
    }

    #[tokio::test]
    async fn partial_mobile_header_expires_at_the_overall_read_deadline() {
        let (mut client, mut server) = connected_tcp_pair().await;
        client
            .write_all(b"GET /mobile HTTP/1.1\r\nHost: mobile")
            .await
            .unwrap();

        let result =
            read_mobile_http_request_with_deadline(&mut server, Duration::from_millis(25)).await;

        assert!(result.unwrap_err().to_string().contains("deadline"));
    }

    #[tokio::test]
    async fn duplicate_content_length_is_rejected_instead_of_overwritten() {
        let (mut client, mut server) = connected_tcp_pair().await;
        client
            .write_all(
                b"POST /api/inject HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx",
            )
            .await
            .unwrap();

        let error = read_mobile_http_request(&mut server).await.unwrap_err();

        assert!(error.to_string().contains("duplicate Content-Length"));
    }

    #[tokio::test]
    async fn malformed_or_missing_lengths_and_transfer_encoding_are_rejected() {
        for request in [
            b"POST /api/inject HTTP/1.1\r\n\r\n".as_slice(),
            b"POST /api/inject HTTP/1.1\r\nContent-Length: nope\r\n\r\n".as_slice(),
            b"POST /api/inject HTTP/1.1\r\nContent-Length: -1\r\n\r\n".as_slice(),
            b"POST /api/inject HTTP/1.1\r\nContent-Length: 999999999999999999999999\r\n\r\n"
                .as_slice(),
            b"POST /api/inject HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".as_slice(),
        ] {
            let (mut client, mut server) = connected_tcp_pair().await;
            client.write_all(request).await.unwrap();
            let result = timeout(
                Duration::from_millis(100),
                read_mobile_http_request(&mut server),
            )
            .await
            .expect("strict parser must not wait for an invalid request body");
            assert!(result.is_err(), "request should be rejected: {request:?}");
        }
    }

    #[test]
    fn mobile_inject_requires_client_sequence_envelope() {
        let request = DaemonRequest::InjectEndpointEvent {
            target: EndpointInjectTarget::Local,
            request: EndpointInjectRequest {
                correlation_id: "mobile-envelope-1".to_string(),
                device_kind: EndpointEventKind::Keyboard,
                payload: EndpointEventPayload::Keyboard {
                    key: "A".to_string(),
                    state: "Pressed".to_string(),
                },
                mode: EndpointInjectMode::RequireHealthyBackend,
                timeout_ms: 750,
            },
        };
        let body = serde_json::to_vec(&json!({
            "client_id": "page-client-1",
            "sequence": 1,
            "request": request,
        }))
        .unwrap();

        let decoded = mobile_inject_envelope_from_body(&body).unwrap();

        assert_eq!(decoded.client_id, "page-client-1");
        assert_eq!(decoded.sequence, 1);
    }

    #[tokio::test]
    async fn connection_limit_refuses_an_extra_socket_without_spawning_it() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let access =
            MobileGatewayAccess::new(addr, "mobile-secret".to_string(), "127.0.0.1".to_string());
        let (state, network_manager, inject_backend, local_events_tx) =
            test_mobile_runtime(Box::new(NoopInjectBackend));
        state.write().await.mobile_access = access.clone();
        let mut limits = MobileGatewayLimits::default();
        limits.max_active_clients = 1;
        limits.read_deadline = Duration::from_secs(2);
        limits.write_deadline = Duration::from_millis(100);
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
        let server = tokio::spawn(run_mobile_gateway_server_on_listener(
            listener,
            access,
            state,
            network_manager,
            inject_backend,
            local_events_tx,
            shutdown_rx,
            limits,
        ));

        let mut stalled = TcpStream::connect(addr).await.unwrap();
        stalled
            .write_all(b"GET /mobile HTTP/1.1\r\n")
            .await
            .unwrap();
        sleep(Duration::from_millis(25)).await;

        let mut extra = TcpStream::connect(addr).await.unwrap();
        let mut response = Vec::new();
        let read_result = timeout(Duration::from_millis(500), extra.read_to_end(&mut response))
            .await
            .unwrap();
        if let Err(error) = read_result {
            assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
        }

        assert!(String::from_utf8_lossy(&response).contains("503 Service Unavailable"));
        let _ = shutdown_tx.send(());
        timeout(Duration::from_secs(1), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn late_sequence_one_press_cannot_inject_after_sequence_two_release() {
        let injected = Arc::new(StdMutex::new(Vec::new()));
        let (state, network_manager, inject_backend, local_events_tx) =
            test_mobile_runtime(Box::new(RecordingInjectBackend {
                injected: injected.clone(),
            }));
        let sessions = MobileClientSessions::new(8, Duration::from_secs(10));
        let allow_late_sequence = Arc::new(Notify::new());
        let late_task = {
            let sessions = sessions.clone();
            let state = state.clone();
            let network_manager = network_manager.clone();
            let inject_backend = inject_backend.clone();
            let local_events_tx = local_events_tx.clone();
            let allow_late_sequence = allow_late_sequence.clone();
            tokio::spawn(async move {
                allow_late_sequence.notified().await;
                process_mobile_inject_envelope(
                    &sessions,
                    &network_manager,
                    &inject_backend,
                    &state,
                    &local_events_tx,
                    keyboard_envelope("ordered-client", 1, "A", "Pressed"),
                )
                .await
            })
        };

        let release = process_mobile_inject_envelope(
            &sessions,
            &network_manager,
            &inject_backend,
            &state,
            &local_events_tx,
            keyboard_envelope("ordered-client", 2, "A", "Released"),
        )
        .await
        .unwrap();
        allow_late_sequence.notify_one();
        let late_error = late_task.await.unwrap().unwrap_err();

        assert!(release.accepted);
        assert!(late_error.to_string().contains("sequence"));
        let injected = injected.lock().unwrap();
        assert_eq!(injected.len(), 1);
        assert!(matches!(
            injected[0],
            InputEvent::Key {
                keycode: rshare_input::KeyCode::Char(b'A'),
                state: rshare_input::ButtonState::Released,
            }
        ));
    }

    #[tokio::test]
    async fn expired_lease_releases_every_dynamic_key_and_mouse_button() {
        let injected = Arc::new(StdMutex::new(Vec::new()));
        let (state, network_manager, inject_backend, local_events_tx) =
            test_mobile_runtime(Box::new(RecordingInjectBackend {
                injected: injected.clone(),
            }));
        let lease = Duration::from_millis(50);
        let sessions = MobileClientSessions::new(8, lease);

        for envelope in [
            keyboard_envelope("lease-client", 1, "A", "Pressed"),
            keyboard_envelope("lease-client", 2, "F12", "Pressed"),
            mouse_button_envelope("lease-client", 3, "Back", "Pressed", 17, 23),
            mouse_button_envelope("lease-client", 4, "Other(9)", "Pressed", 31, 37),
        ] {
            assert!(
                process_mobile_inject_envelope(
                    &sessions,
                    &network_manager,
                    &inject_backend,
                    &state,
                    &local_events_tx,
                    envelope,
                )
                .await
                .unwrap()
                .accepted
            );
        }

        sessions
            .reap_expired_at(
                tokio::time::Instant::now() + lease + Duration::from_millis(1),
                &network_manager,
                &inject_backend,
                &state,
                &local_events_tx,
            )
            .await;
        let after_first_reap = injected.lock().unwrap().len();
        sessions
            .reap_expired_at(
                tokio::time::Instant::now() + lease + Duration::from_secs(1),
                &network_manager,
                &inject_backend,
                &state,
                &local_events_tx,
            )
            .await;

        let injected = injected.lock().unwrap();
        assert_eq!(after_first_reap, 8);
        assert_eq!(
            injected.len(),
            8,
            "successfully released controls must be cleared"
        );
        assert!(injected.iter().any(|event| matches!(
            event,
            InputEvent::Key {
                keycode: rshare_input::KeyCode::Char(b'A'),
                state: rshare_input::ButtonState::Released,
            }
        )));
        assert!(injected.iter().any(|event| matches!(
            event,
            InputEvent::Key {
                keycode: rshare_input::KeyCode::F12,
                state: rshare_input::ButtonState::Released,
            }
        )));
        assert!(injected.iter().any(|event| matches!(
            event,
            InputEvent::MouseButton {
                button: rshare_input::MouseButton::Back,
                state: rshare_input::ButtonState::Released,
            }
        )));
        assert!(injected.iter().any(|event| matches!(
            event,
            InputEvent::MouseButton {
                button: rshare_input::MouseButton::Other(9),
                state: rshare_input::ButtonState::Released,
            }
        )));
    }

    #[tokio::test]
    async fn shutdown_cleanup_releases_every_tracked_control() {
        let injected = Arc::new(StdMutex::new(Vec::new()));
        let (state, network_manager, inject_backend, local_events_tx) =
            test_mobile_runtime(Box::new(RecordingInjectBackend {
                injected: injected.clone(),
            }));
        let sessions = MobileClientSessions::new(8, Duration::from_secs(10));
        for envelope in [
            keyboard_envelope("shutdown-client", 1, "A", "Pressed"),
            mouse_button_envelope("shutdown-client", 2, "Left", "Pressed", 9, 11),
        ] {
            assert!(
                process_mobile_inject_envelope(
                    &sessions,
                    &network_manager,
                    &inject_backend,
                    &state,
                    &local_events_tx,
                    envelope,
                )
                .await
                .unwrap()
                .accepted
            );
        }

        sessions
            .release_all_held_inputs(&network_manager, &inject_backend, &state, &local_events_tx)
            .await;

        let injected = injected.lock().unwrap();
        assert_eq!(injected.len(), 4);
        assert!(matches!(
            injected[2],
            InputEvent::Key {
                keycode: rshare_input::KeyCode::Char(b'A'),
                state: rshare_input::ButtonState::Released,
            }
        ));
        assert!(matches!(
            injected[3],
            InputEvent::MouseButton {
                button: rshare_input::MouseButton::Left,
                state: rshare_input::ButtonState::Released,
            }
        ));
    }

    #[tokio::test]
    async fn listener_shutdown_releases_held_controls_before_server_returns() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let access =
            MobileGatewayAccess::new(addr, "mobile-secret".to_string(), "127.0.0.1".to_string());
        let injected = Arc::new(StdMutex::new(Vec::new()));
        let (state, network_manager, inject_backend, local_events_tx) =
            test_mobile_runtime(Box::new(RecordingInjectBackend {
                injected: injected.clone(),
            }));
        state.write().await.mobile_access = access.clone();
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
        let server = tokio::spawn(run_mobile_gateway_server_on_listener(
            listener,
            access,
            state,
            network_manager,
            inject_backend,
            local_events_tx,
            shutdown_rx,
            MobileGatewayLimits::default(),
        ));

        for envelope in [
            keyboard_envelope("shutdown-http", 1, "A", "Pressed"),
            mouse_button_envelope("shutdown-http", 2, "Left", "Pressed", 21, 34),
        ] {
            let body = serde_json::to_vec(&json!({
                "client_id": envelope.client_id,
                "sequence": envelope.sequence,
                "request": envelope.request,
            }))
            .unwrap();
            let response = post_mobile_json(addr, body).await;
            assert!(String::from_utf8_lossy(&response).contains("200 OK"));
        }

        let _ = shutdown_tx.send(());
        timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        let injected = injected.lock().unwrap();
        assert_eq!(injected.len(), 4);
        assert!(injected[2..].iter().all(|event| matches!(
            event,
            InputEvent::Key {
                state: rshare_input::ButtonState::Released,
                ..
            } | InputEvent::MouseButton {
                state: rshare_input::ButtonState::Released,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn release_batch_is_processed_atomically_under_one_sequence() {
        let injected = Arc::new(StdMutex::new(Vec::new()));
        let (state, network_manager, inject_backend, local_events_tx) =
            test_mobile_runtime(Box::new(RecordingInjectBackend {
                injected: injected.clone(),
            }));
        let sessions = MobileClientSessions::new(8, Duration::from_secs(10));
        for envelope in [
            keyboard_envelope("batch-client", 1, "A", "Pressed"),
            mouse_button_envelope("batch-client", 2, "Left", "Pressed", 5, 7),
        ] {
            process_mobile_inject_envelope(
                &sessions,
                &network_manager,
                &inject_backend,
                &state,
                &local_events_tx,
                envelope,
            )
            .await
            .unwrap();
        }
        let batch = MobileReleaseBatch {
            client_id: "batch-client".to_string(),
            sequence: 3,
            requests: vec![
                keyboard_envelope("ignored", 1, "A", "Released").request,
                mouse_button_envelope("ignored", 2, "Left", "Released", 5, 7).request,
            ],
        };

        let results = process_mobile_release_batch(
            &sessions,
            &network_manager,
            &inject_backend,
            &state,
            &local_events_tx,
            batch,
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| result.accepted));
        sessions
            .release_all_held_inputs(&network_manager, &inject_backend, &state, &local_events_tx)
            .await;
        assert_eq!(injected.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn failed_best_effort_release_batch_retains_server_held_state() {
        let injected = Arc::new(StdMutex::new(Vec::new()));
        let (state, network_manager, inject_backend, local_events_tx) =
            test_mobile_runtime(Box::new(RecordingInjectBackend {
                injected: injected.clone(),
            }));
        let sessions = MobileClientSessions::new(8, Duration::from_secs(10));
        assert!(
            process_mobile_inject_envelope(
                &sessions,
                &network_manager,
                &inject_backend,
                &state,
                &local_events_tx,
                keyboard_envelope("failed-release-batch", 1, "A", "Pressed"),
            )
            .await
            .unwrap()
            .accepted
        );

        let attempted = Arc::new(StdMutex::new(Vec::new()));
        *inject_backend.lock().await = Box::new(RejectingInjectBackend {
            attempted: attempted.clone(),
        });
        let mut release = keyboard_envelope("ignored", 1, "A", "Released").request;
        let DaemonRequest::InjectEndpointEvent { request, .. } = &mut release else {
            unreachable!("keyboard helper must build an inject request");
        };
        request.mode = EndpointInjectMode::BestEffort;

        let results = process_mobile_release_batch(
            &sessions,
            &network_manager,
            &inject_backend,
            &state,
            &local_events_tx,
            MobileReleaseBatch {
                client_id: "failed-release-batch".to_string(),
                sequence: 2,
                requests: vec![release],
            },
        )
        .await
        .unwrap();

        assert!(!results[0].accepted);
        assert_eq!(attempted.lock().unwrap().len(), 1);
        let session = sessions
            .session_at("failed-release-batch", Instant::now())
            .await
            .unwrap();
        assert_eq!(session.lock().await.held_keys.len(), 1);
    }

    #[tokio::test]
    async fn rotated_client_traffic_cannot_stale_the_old_generation_cleanup() {
        let injected = Arc::new(StdMutex::new(Vec::new()));
        let (state, network_manager, inject_backend, local_events_tx) =
            test_mobile_runtime(Box::new(RecordingInjectBackend {
                injected: injected.clone(),
            }));
        let sessions = MobileClientSessions::new(8, Duration::from_secs(10));
        assert!(
            process_mobile_inject_envelope(
                &sessions,
                &network_manager,
                &inject_backend,
                &state,
                &local_events_tx,
                keyboard_envelope("generation-old", 1, "A", "Pressed"),
            )
            .await
            .unwrap()
            .accepted
        );
        assert!(
            process_mobile_inject_envelope(
                &sessions,
                &network_manager,
                &inject_backend,
                &state,
                &local_events_tx,
                keyboard_envelope("generation-new", 1, "B", "Pressed"),
            )
            .await
            .unwrap()
            .accepted
        );
        let old_cleanup = MobileReleaseBatch {
            client_id: "generation-old".to_string(),
            sequence: 2,
            requests: vec![keyboard_envelope("ignored", 1, "A", "Released").request],
        };

        let cleanup = process_mobile_release_batch(
            &sessions,
            &network_manager,
            &inject_backend,
            &state,
            &local_events_tx,
            old_cleanup,
        )
        .await
        .unwrap();

        assert!(cleanup[0].accepted);
        sessions
            .release_all_held_inputs(&network_manager, &inject_backend, &state, &local_events_tx)
            .await;
        let injected = injected.lock().unwrap();
        assert_eq!(injected.len(), 4);
        assert!(matches!(
            injected[2],
            InputEvent::Key {
                keycode: rshare_input::KeyCode::Char(b'A'),
                state: rshare_input::ButtonState::Released,
            }
        ));
        assert!(matches!(
            injected[3],
            InputEvent::Key {
                keycode: rshare_input::KeyCode::Char(b'B'),
                state: rshare_input::ButtonState::Released,
            }
        ));
    }

    #[tokio::test]
    async fn inject_route_accepts_one_bounded_release_batch_body() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let access =
            MobileGatewayAccess::new(addr, "mobile-secret".to_string(), "127.0.0.1".to_string());
        let injected = Arc::new(StdMutex::new(Vec::new()));
        let (state, network_manager, inject_backend, local_events_tx) =
            test_mobile_runtime(Box::new(RecordingInjectBackend {
                injected: injected.clone(),
            }));
        state.write().await.mobile_access = access.clone();
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
        let server = tokio::spawn(run_mobile_gateway_server_on_listener(
            listener,
            access,
            state,
            network_manager,
            inject_backend,
            local_events_tx,
            shutdown_rx,
            MobileGatewayLimits::default(),
        ));

        for envelope in [
            keyboard_envelope("batch-http", 1, "A", "Pressed"),
            mouse_button_envelope("batch-http", 2, "Left", "Pressed", 4, 6),
        ] {
            let response = post_mobile_json(
                addr,
                serde_json::to_vec(&json!({
                    "client_id": envelope.client_id,
                    "sequence": envelope.sequence,
                    "request": envelope.request,
                }))
                .unwrap(),
            )
            .await;
            assert!(String::from_utf8_lossy(&response).contains("200 OK"));
        }
        let batch_body = serde_json::to_vec(&json!({
            "client_id": "batch-http",
            "sequence": 3,
            "requests": [
                keyboard_envelope("ignored", 1, "A", "Released").request,
                mouse_button_envelope("ignored", 2, "Left", "Released", 4, 6).request,
            ],
        }))
        .unwrap();

        let response = post_mobile_json(addr, batch_body).await;

        assert!(String::from_utf8_lossy(&response).contains("MobileReleaseBatchResult"));
        let _ = shutdown_tx.send(());
        timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(injected.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn held_key_cap_rejects_a_new_press_before_backend_injection() {
        let injected = Arc::new(StdMutex::new(Vec::new()));
        let (state, network_manager, inject_backend, local_events_tx) =
            test_mobile_runtime(Box::new(RecordingInjectBackend {
                injected: injected.clone(),
            }));
        let sessions = MobileClientSessions::new(8, Duration::from_secs(10));

        for index in 0..64u64 {
            let result = process_mobile_inject_envelope(
                &sessions,
                &network_manager,
                &inject_backend,
                &state,
                &local_events_tx,
                keyboard_envelope(
                    "capacity-client",
                    index + 1,
                    &format!("Raw({})", 1_000 + index),
                    "Pressed",
                ),
            )
            .await
            .unwrap();
            assert!(result.accepted);
        }
        let error = process_mobile_inject_envelope(
            &sessions,
            &network_manager,
            &inject_backend,
            &state,
            &local_events_tx,
            keyboard_envelope("capacity-client", 65, "Raw(2000)", "Pressed"),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("held key limit"));
        assert_eq!(injected.lock().unwrap().len(), 64);
    }

    #[tokio::test]
    async fn raw_values_stay_canonical_and_named_aliases_collapse() {
        let injected = Arc::new(StdMutex::new(Vec::new()));
        let (state, network_manager, inject_backend, local_events_tx) =
            test_mobile_runtime(Box::new(RecordingInjectBackend {
                injected: injected.clone(),
            }));
        let sessions = MobileClientSessions::new(8, Duration::from_secs(10));
        for envelope in [
            keyboard_envelope("canonical-client", 1, "Raw(65)", "Pressed"),
            keyboard_envelope("canonical-client", 2, "A", "Pressed"),
            keyboard_envelope("canonical-client", 3, "Esc", "Pressed"),
            keyboard_envelope("canonical-client", 4, "Escape", "Pressed"),
            mouse_button_envelope("canonical-client", 5, "Other(1)", "Pressed", 3, 4),
            mouse_button_envelope("canonical-client", 6, "Left", "Pressed", 7, 8),
        ] {
            process_mobile_inject_envelope(
                &sessions,
                &network_manager,
                &inject_backend,
                &state,
                &local_events_tx,
                envelope,
            )
            .await
            .unwrap();
        }

        sessions
            .release_all_held_inputs(&network_manager, &inject_backend, &state, &local_events_tx)
            .await;

        let injected = injected.lock().unwrap();
        assert_eq!(injected.len(), 10, "named aliases must share one release");
        let releases = &injected[6..];
        assert!(releases.iter().any(|event| matches!(
            event,
            InputEvent::Key {
                keycode: rshare_input::KeyCode::Raw(65),
                state: rshare_input::ButtonState::Released,
            }
        )));
        assert!(releases.iter().any(|event| matches!(
            event,
            InputEvent::Key {
                keycode: rshare_input::KeyCode::Char(b'A'),
                state: rshare_input::ButtonState::Released,
            }
        )));
        assert_eq!(
            releases
                .iter()
                .filter(|event| matches!(
                    event,
                    InputEvent::Key {
                        keycode: rshare_input::KeyCode::Escape,
                        state: rshare_input::ButtonState::Released,
                    }
                ))
                .count(),
            1
        );
        assert_eq!(
            releases
                .iter()
                .filter(|event| matches!(
                    event,
                    InputEvent::MouseButton {
                        button: rshare_input::MouseButton::Left,
                        state: rshare_input::ButtonState::Released,
                    }
                ))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn cancelled_after_backend_press_still_has_shutdown_compensation() {
        let injected = Arc::new(StdMutex::new(Vec::new()));
        let pressed_injected = Arc::new(Notify::new());
        let (state, network_manager, inject_backend, local_events_tx) =
            test_mobile_runtime(Box::new(SignalingInjectBackend {
                injected: injected.clone(),
                pressed_injected: pressed_injected.clone(),
            }));
        let sessions = MobileClientSessions::new(8, Duration::from_secs(10));
        let state_guard = state.write().await;
        let task = {
            let sessions = sessions.clone();
            let state = state.clone();
            let network_manager = network_manager.clone();
            let inject_backend = inject_backend.clone();
            let local_events_tx = local_events_tx.clone();
            tokio::spawn(async move {
                process_mobile_inject_envelope(
                    &sessions,
                    &network_manager,
                    &inject_backend,
                    &state,
                    &local_events_tx,
                    keyboard_envelope("cancel-client", 1, "A", "Pressed"),
                )
                .await
            })
        };

        timeout(Duration::from_secs(1), pressed_injected.notified())
            .await
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        drop(state_guard);

        sessions
            .release_all_held_inputs(&network_manager, &inject_backend, &state, &local_events_tx)
            .await;

        let injected = injected.lock().unwrap();
        assert_eq!(injected.len(), 2);
        assert!(matches!(
            injected[1],
            InputEvent::Key {
                keycode: rshare_input::KeyCode::Char(b'A'),
                state: rshare_input::ButtonState::Released,
            }
        ));
    }

    #[tokio::test]
    async fn rejected_new_press_rolls_back_provisional_held_identity() {
        let attempted = Arc::new(StdMutex::new(Vec::new()));
        let (state, network_manager, inject_backend, local_events_tx) =
            test_mobile_runtime(Box::new(RejectingInjectBackend {
                attempted: attempted.clone(),
            }));
        let sessions = MobileClientSessions::new(8, Duration::from_secs(10));

        let result = process_mobile_inject_envelope(
            &sessions,
            &network_manager,
            &inject_backend,
            &state,
            &local_events_tx,
            keyboard_envelope("reject-client", 1, "A", "Pressed"),
        )
        .await
        .unwrap();
        assert!(!result.accepted);
        sessions
            .release_all_held_inputs(&network_manager, &inject_backend, &state, &local_events_tx)
            .await;

        assert_eq!(attempted.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn rejected_mouse_repress_restores_last_accepted_release_coordinates() {
        let injected = Arc::new(StdMutex::new(Vec::new()));
        let (state, network_manager, inject_backend, local_events_tx) =
            test_mobile_runtime(Box::new(RejectSecondMousePressBackend {
                injected: injected.clone(),
                mouse_press_count: 0,
            }));
        let sessions = MobileClientSessions::new(8, Duration::from_secs(10));

        let first = process_mobile_inject_envelope(
            &sessions,
            &network_manager,
            &inject_backend,
            &state,
            &local_events_tx,
            mouse_button_envelope("mouse-rollback", 1, "Left", "Pressed", 11, 17),
        )
        .await
        .unwrap();
        assert!(first.accepted);
        let rejected = process_mobile_inject_envelope(
            &sessions,
            &network_manager,
            &inject_backend,
            &state,
            &local_events_tx,
            mouse_button_envelope("mouse-rollback", 2, "Left", "Pressed", 91, 97),
        )
        .await
        .unwrap();
        assert!(!rejected.accepted);

        sessions
            .release_all_held_inputs(&network_manager, &inject_backend, &state, &local_events_tx)
            .await;

        let state = state.read().await;
        let release = state.local_controls.recent_events.last().unwrap();
        assert_eq!(
            release.payload.get("state").map(String::as_str),
            Some("Released")
        );
        assert_eq!(release.payload.get("x").map(String::as_str), Some("11"));
        assert_eq!(release.payload.get("y").map(String::as_str), Some("17"));
    }

    #[tokio::test]
    async fn compensation_attempts_release_after_backend_becomes_inactive() {
        let active = Arc::new(AtomicBool::new(true));
        let injected = Arc::new(StdMutex::new(Vec::new()));
        let (state, network_manager, inject_backend, local_events_tx) =
            test_mobile_runtime(Box::new(DegradingInjectBackend {
                active: active.clone(),
                injected: injected.clone(),
            }));
        let sessions = MobileClientSessions::new(8, Duration::from_secs(10));

        let press = process_mobile_inject_envelope(
            &sessions,
            &network_manager,
            &inject_backend,
            &state,
            &local_events_tx,
            keyboard_envelope("degraded-cleanup", 1, "A", "Pressed"),
        )
        .await
        .unwrap();
        assert!(press.accepted);
        active.store(false, Ordering::SeqCst);

        sessions
            .release_all_held_inputs(&network_manager, &inject_backend, &state, &local_events_tx)
            .await;
        sessions
            .release_all_held_inputs(&network_manager, &inject_backend, &state, &local_events_tx)
            .await;

        let injected = injected.lock().unwrap();
        assert_eq!(injected.len(), 2, "accepted cleanup must clear held state");
        assert!(matches!(
            injected[1],
            InputEvent::Key {
                keycode: rshare_input::KeyCode::Char(b'A'),
                state: rshare_input::ButtonState::Released,
            }
        ));
    }

    #[test]
    fn rendered_mobile_page_sequences_envelopes_and_releases_complete_held_sets() {
        let page = render_mobile_page();

        assert!(page.contains("let clientId = newMobileClientId();"));
        assert!(page.contains("let mobileSequence = 0"));
        assert!(page.contains("function mobileEnvelope(request)"));
        assert!(page.contains("sequence: ++mobileSequence"));
        assert!(page.contains("client_id: clientId"));
        assert!(page.contains("/api/local-controls?client_id="));
        assert!(page.contains("const heldKeys = new Set()"));
        assert!(page.contains("const heldMouseButtons = new Set()"));
        assert!(page.contains("Array.from(heldKeys)"));
        assert!(page.contains("Array.from(heldMouseButtons)"));
        assert!(page.contains("function keepaliveReleaseBatch(requests)"));
        assert!(page.contains("requests: requests"));
        assert!(page.contains("keepaliveReleaseBatch(releaseAllRequests"));
        assert!(!page.contains(
            "for (const request of releaseAllRequests(\"mobile-release-all-keepalive\"))"
        ));
    }

    #[test]
    fn rendered_mobile_page_makes_batch_the_final_lifecycle_network_action() {
        let page = render_mobile_page();

        assert!(page.contains("function resetHeldButtonPointerState()"));
        assert!(!page.contains("window.addEventListener(\"blur\", () => release(true));"));
        assert!(!page.contains("window.addEventListener(\"pagehide\", () => release(true));"));
        assert!(!page.contains("if (document.visibilityState === \"hidden\") release(true);"));
        for listener in [
            "window.addEventListener(\"blur\", releaseAllWithKeepalive);",
            "window.addEventListener(\"pagehide\", releaseAllWithKeepalive);",
            "document.addEventListener(\"visibilitychange\", releaseAllWithKeepaliveWhenHidden);",
        ] {
            let batch_listener = page
                .rfind(listener)
                .expect("missing lifecycle batch listener");
            let per_button_setup = page
                .find("function attachHeldButton")
                .expect("missing held-button setup");
            assert!(batch_listener > per_button_setup);
        }
    }

    #[test]
    fn rendered_mobile_page_suspends_async_input_and_polling_before_lifecycle_batch() {
        let page = render_mobile_page();
        assert!(page.contains("let inputSuspended = false"));

        let prepare_start = page
            .find("function prepareOrdinaryInjectEnvelope(request, expectedGeneration)")
            .expect("missing ordinary-input gate");
        let inject_start = page
            .find("async function inject(request, expectedGeneration = inputGeneration)")
            .expect("missing inject function");
        let prepare = &page[prepare_start..inject_start];
        let gate = prepare
            .find("if (inputSuspended || expectedGeneration !== inputGeneration) return null;")
            .unwrap();
        let tracking = prepare
            .find("trackHeldInputBeforeInject(request);")
            .unwrap();
        let sequence = prepare.find("return mobileEnvelope(request);").unwrap();
        assert!(gate < tracking && tracking < sequence);

        let refresh_start = page
            .find("async function refresh()")
            .expect("missing refresh function");
        let inject = &page[inject_start..refresh_start];
        assert!(
            inject
                .find(
                    "const envelope = prepareOrdinaryInjectEnvelope(request, expectedGeneration);"
                )
                .unwrap()
                < inject.find("if (!envelope) return false;").unwrap()
        );
        assert!(
            inject.find("if (!envelope) return false;").unwrap()
                < inject.find("await api(\"/api/inject\"").unwrap()
        );
        assert!(!inject.contains("mobileEnvelope(request)"));
        let refresh_end = page[refresh_start..]
            .find("function sendMoveNow")
            .map(|offset| refresh_start + offset)
            .unwrap();
        let refresh = &page[refresh_start..refresh_end];
        assert!(
            refresh.find("if (inputSuspended) return;").unwrap()
                < refresh.find("api(`/api/local-controls").unwrap()
        );

        let lifecycle_start = page
            .find("function releaseAllWithKeepalive()")
            .expect("missing lifecycle release");
        let lifecycle_end = page[lifecycle_start..]
            .find("function clearDragTimer")
            .map(|offset| lifecycle_start + offset)
            .unwrap();
        let lifecycle = &page[lifecycle_start..lifecycle_end];
        assert!(
            lifecycle.find("inputSuspended = true;").unwrap()
                < lifecycle.find("keepaliveReleaseBatch").unwrap()
        );
        assert!(page.contains("window.addEventListener(\"focus\", resumeMobileInput);"));
        assert!(page.contains("window.addEventListener(\"pageshow\", resumeMobileInput);"));
        assert!(page.contains(
            "document.addEventListener(\"visibilitychange\", resumeMobileInputWhenVisible);"
        ));
    }

    #[test]
    fn rendered_mobile_page_discards_stale_inject_completion_before_mutation() {
        let page = render_mobile_page();
        let inject_start = page
            .find("async function inject(request, expectedGeneration = inputGeneration)")
            .expect("missing inject function");
        let refresh_start = page[inject_start..]
            .find("async function refresh()")
            .map(|offset| inject_start + offset)
            .expect("missing refresh function");
        let inject = &page[inject_start..refresh_start];
        let await_api = inject.find("await api(\"/api/inject\"").unwrap();
        let catch_start = inject.find("} catch (error) {").unwrap();
        let success = &inject[await_api..catch_start];
        let generation_guard = success
            .find("if (inputSuspended || expectedGeneration !== inputGeneration) return false;")
            .expect("missing post-await generation guard");
        for mutation in [
            "clearReleasedHeldInput(request, feedback.accepted);",
            "statusEl.textContent = feedback.status;",
            "return feedback.accepted;",
        ] {
            assert!(generation_guard < success.find(mutation).unwrap());
        }

        let catch = &inject[catch_start..];
        assert!(
            catch
                .find("if (inputSuspended || expectedGeneration !== inputGeneration) return false;")
                .expect("missing catch generation guard")
                < catch
                    .find("statusEl.textContent = formatMobileError")
                    .unwrap()
        );

        let send_text_start = page
            .find("async function sendText()")
            .expect("missing text flow");
        let send_text_end = page[send_text_start..]
            .find("function shouldSendTextOnKeydown")
            .map(|offset| send_text_start + offset)
            .unwrap();
        let send_text = &page[send_text_start..send_text_end];
        assert!(
            send_text.find("if (await inject").unwrap()
                < send_text.find("textInput.value = \"\";").unwrap()
        );
    }

    #[test]
    fn rendered_mobile_release_batch_uses_best_effort_for_keyboard_releases() {
        let page = render_mobile_page();
        let release_start = page
            .find("function releaseAllRequests(prefix)")
            .expect("missing release batch builder");
        let release_end = page[release_start..]
            .find("async function sendReleaseAll()")
            .map(|offset| release_start + offset)
            .unwrap();
        let release = &page[release_start..release_end];

        assert_eq!(release.matches("daemonRequest(\"Keyboard\"").count(), 2);
        assert_eq!(release.matches("\"BestEffort\", 750").count(), 2);
        assert!(!release.contains("RequireHealthyBackend"));
    }

    #[test]
    fn rendered_mobile_page_rotates_client_generation_before_resume() {
        let page = render_mobile_page();

        assert!(page.contains("function newMobileClientId()"));
        assert!(page.contains("let clientId = newMobileClientId();"));
        assert!(page.contains("let inputGeneration = 0;"));
        let rotate_start = page
            .find("function rotateMobileClientSession()")
            .expect("missing client-session rotation");
        let resume_start = page
            .find("function resumeMobileInput()")
            .expect("missing lifecycle resume");
        let rotate = &page[rotate_start..resume_start];
        assert!(rotate.contains("clientId = newMobileClientId();"));
        assert!(rotate.contains("mobileSequence = 0;"));
        assert!(rotate.contains("heldKeys.clear();"));
        assert!(rotate.contains("heldMouseButtons.clear();"));

        let resume_end = page[resume_start..]
            .find("function resumeMobileInputWhenVisible")
            .map(|offset| resume_start + offset)
            .unwrap();
        let resume = &page[resume_start..resume_end];
        assert!(
            resume.find("rotateMobileClientSession();").unwrap()
                < resume.find("inputSuspended = false;").unwrap()
        );
        assert!(
            resume.find("inputSuspended = false;").unwrap() < resume.find("refresh();").unwrap()
        );

        assert!(page.contains("expectedGeneration !== inputGeneration"));
        assert!(
            page.contains("async function inject(request, expectedGeneration = inputGeneration)")
        );
        assert!(page.contains("const pollingClientId = clientId;"));
        assert!(page.contains("encodeURIComponent(pollingClientId)"));
        for flow in [
            "async function sendTapClick() {\n  const generation = inputGeneration;",
            "async function sendDoubleClick(buttonName) {\n  const generation = inputGeneration;",
            "async function sendTwoFingerTapClick() {\n  const generation = inputGeneration;",
            "async function sendKeyChord(keys) {\n  const generation = inputGeneration;",
        ] {
            assert!(page.contains(flow), "missing generation capture in {flow}");
        }
    }

    #[test]
    fn decodes_mobile_daemon_inject_request_as_local_only() {
        let request = DaemonRequest::InjectEndpointEvent {
            target: EndpointInjectTarget::Local,
            request: EndpointInjectRequest {
                correlation_id: "mobile-text-1".to_string(),
                device_kind: EndpointEventKind::Keyboard,
                payload: EndpointEventPayload::TextCommit {
                    text: "你好".to_string(),
                },
                mode: EndpointInjectMode::RequireHealthyBackend,
                timeout_ms: 750,
            },
        };
        let body = serde_json::to_vec(&json!({
            "client_id": "page-client-1",
            "sequence": 1,
            "request": request,
        }))
        .unwrap();

        let decoded = mobile_inject_envelope_from_body(&body).unwrap();

        assert!(matches!(
            decoded.request,
            DaemonRequest::InjectEndpointEvent {
                request: EndpointInjectRequest {
                    payload: EndpointEventPayload::TextCommit { text },
                    ..
                },
                ..
            } if text == "你好"
        ));
    }

    #[test]
    fn rejects_mobile_remote_inject_request() {
        let request = DaemonRequest::InjectEndpointEvent {
            target: EndpointInjectTarget::Remote(DeviceId::nil()),
            request: EndpointInjectRequest {
                correlation_id: "mobile-remote-1".to_string(),
                device_kind: EndpointEventKind::Keyboard,
                payload: EndpointEventPayload::Keyboard {
                    key: "Enter".to_string(),
                    state: "Pressed".to_string(),
                },
                mode: EndpointInjectMode::BestEffort,
                timeout_ms: 250,
            },
        };
        let body = serde_json::to_vec(&json!({
            "client_id": "page-client-1",
            "sequence": 1,
            "request": request,
        }))
        .unwrap();

        let error = mobile_inject_envelope_from_body(&body).unwrap_err();

        assert!(error.to_string().contains("local endpoint"));
    }

    #[test]
    fn rendered_mobile_page_coalesces_pointer_moves_and_flushes_on_release() {
        let page = render_mobile_page();

        assert!(page.contains("function scheduleMove(next)"));
        assert!(page.contains("requestAnimationFrame"));
        assert!(page.contains("function flushMove()"));
        assert!(page.contains("cancelAnimationFrame"));
        assert!(page.contains("scheduleMove(pointer);"));
        assert!(page.contains("flushMove();"));
    }

    #[test]
    fn rendered_mobile_page_turns_short_touchpad_taps_into_left_clicks() {
        let page = render_mobile_page();

        assert!(page.contains("function isTouchpadTap"));
        assert!(page.contains("function sendTapClick()"));
        assert!(page.contains("sendTapClick();"));
        assert!(page.contains("pad.addEventListener(\"pointercancel\", cancelPointer);"));
        assert!(page.contains("button: \"Left\", state: \"Pressed\""));
        assert!(page.contains("button: \"Left\", state: \"Released\""));
    }

    #[test]
    fn rendered_mobile_page_exposes_explicit_left_double_click() {
        let page = render_mobile_page();

        assert!(page.contains("data-double-click=\"Left\""));
        assert!(page.contains("function sendDoubleClick"));
        assert!(page.contains("mobile-double-Left-1-down"));
        assert!(page.contains("mobile-double-Left-2-up"));
    }

    #[test]
    fn rendered_mobile_page_exposes_mouse_back_and_forward_buttons() {
        let page = render_mobile_page();

        assert!(page.contains("data-button=\"Back\""));
        assert!(page.contains("data-button=\"Forward\""));
        assert!(page.contains(">后退</button>"));
        assert!(page.contains(">前进</button>"));
    }

    #[test]
    fn rendered_mobile_page_exposes_release_all_input_control() {
        let page = render_mobile_page();

        assert!(page.contains("data-release-all"));
        assert!(page.contains("function sendReleaseAll()"));
        assert!(page.contains("function releaseAllRequests(prefix)"));
        assert!(page.contains("releaseAllRequests(\"mobile-release-all\")"));
        assert!(page.contains("${prefix}-mouse-left"));
        assert!(page.contains("${prefix}-key-controlleft"));
        assert!(page.contains(">释放全部</button>"));
    }

    #[test]
    fn rendered_mobile_page_supports_two_finger_wheel_gestures() {
        let page = render_mobile_page();

        assert!(page.contains("function twoFingerWheelDelta"));
        assert!(page.contains("function touchPointsSnapshot"));
        assert!(page.contains("sendWheelDelta(wheel);"));
        assert!(page.contains("kind: \"MouseWheel\""));
        assert!(page.contains("lastWheelTouches ="));
        assert!(page.contains("touchPointsSnapshot()"));
    }

    #[test]
    fn rendered_mobile_page_turns_two_finger_taps_into_right_clicks() {
        let page = render_mobile_page();

        assert!(page.contains("function isTwoFingerTap"));
        assert!(page.contains("function sendTwoFingerTapClick()"));
        assert!(page.contains("sendTwoFingerTapClick();"));
        assert!(page.contains("button: \"Right\", state: \"Pressed\""));
        assert!(page.contains("button: \"Right\", state: \"Released\""));
        assert!(page.contains("twoFingerTapStart"));
    }

    #[test]
    fn rendered_mobile_page_respects_ime_composition_before_text_commit() {
        let page = render_mobile_page();

        assert!(page.contains("enterkeyhint=\"send\""));
        assert!(page.contains("function shouldSendTextOnKeydown"));
        assert!(page.contains("event.isComposing"));
        assert!(page.contains("event.keyCode !== 229"));
        assert!(page.contains("if (shouldSendTextOnKeydown(event))"));
    }

    #[test]
    fn rendered_mobile_page_uses_multiline_textarea_for_ime_input() {
        let page = render_mobile_page();

        assert!(page.contains("<textarea id=\"text\""));
        assert!(page.contains("rows=\"3\""));
        assert!(page.contains("!event.shiftKey"));
        assert!(!page.contains("<input id=\"text\""));
    }

    #[test]
    fn rendered_mobile_page_does_not_advertise_installable_pwa_assets() {
        let page = render_mobile_page_with_token("mobile+secret/token=");

        assert!(!page.contains("rel='manifest'"));
        assert!(!page.contains("mobile.webmanifest"));
        assert!(!page.contains("mobile-icon.svg"));
        assert!(!page.contains("theme-color"));
        assert!(!page.contains("mobile-web-app-capable"));
        assert!(!page.contains("apple-mobile-web-app-capable"));
        assert!(page.contains("const token = \"mobile+secret/token=\" ||"));
    }

    #[test]
    fn rendered_mobile_page_hides_raw_fetch_failures_and_keeps_text_on_failure() {
        let page = render_mobile_page();

        assert!(page.contains("function formatMobileError"));
        assert!(page.contains("failed to fetch|networkerror|fetch failed|load failed"));
        assert!(page.contains("网关不可用，请确认桌面服务正在运行并且手机与电脑在同一网络"));
        assert!(page.contains("return false;"));
        assert!(page.contains("if (await inject"));
        assert!(page.contains("textInput.value = \"\";"));
    }

    #[test]
    fn rendered_mobile_page_shows_injection_backend_readiness() {
        let page = render_mobile_page();

        assert!(page.contains("id=\"backendStatus\""));
        assert!(page.contains("function formatBackendStatus(snapshot)"));
        assert!(page.contains("snapshot.inject_backend"));
        assert!(page.contains("输入注入就绪"));
        assert!(page.contains("输入注入不可用"));
        assert!(page.contains("等待输入后端"));
    }

    #[test]
    fn rendered_mobile_page_reports_rejected_endpoint_inject_results() {
        let page = render_mobile_page();

        assert!(page.contains("function formatInjectResultStatus(result)"));
        assert!(page.contains("EndpointInjectResult"));
        assert!(page.contains("injectResult.error"));
        assert!(page.contains("PermissionDenied"));
        assert!(page.contains("输入后端不可用"));
        assert!(page.contains("statusEl.textContent = feedback.status"));
        assert!(page.contains("return feedback.accepted"));
    }

    #[test]
    fn rendered_mobile_page_holds_keyboard_buttons_until_pointer_release() {
        let page = render_mobile_page();

        assert!(page.contains("function sendKeyState"));
        assert!(page.contains("data-key"));
        assert!(page.contains("button.addEventListener(\"pointerdown\""));
        assert!(page.contains("button.addEventListener(\"pointerup\""));
        assert!(page.contains("button.addEventListener(\"pointercancel\""));
        assert!(page.contains("sendKeyState(button, \"Pressed\")"));
        assert!(page.contains("sendKeyState(button, \"Released\")"));
    }

    #[test]
    fn rendered_mobile_page_exposes_common_keyboard_keys_and_shortcuts() {
        let page = render_mobile_page();

        for key in ["ControlLeft", "ShiftLeft", "AltLeft", "SuperLeft"] {
            assert!(page.contains(&format!("data-key=\"{key}\"")));
        }
        for key in [
            "Escape", "Tab", "Space", "Delete", "Home", "End", "PageUp", "PageDown",
        ] {
            assert!(page.contains(&format!("data-key=\"{key}\"")));
        }
        for shortcut in [
            "ControlLeft,C",
            "ControlLeft,V",
            "ControlLeft,X",
            "ControlLeft,A",
        ] {
            assert!(page.contains(&format!("data-shortcut=\"{shortcut}\"")));
        }
        assert!(page.contains("async function sendKeyChord(keys)"));
        assert!(page.contains("for (const key of keys)"));
        assert!(page.contains("for (const key of [...keys].reverse())"));
        assert!(page.contains("overflow: auto"));
        assert!(page.contains("min-height: 100dvh"));
    }

    #[test]
    fn removed_install_assets_are_not_routable() {
        assert_eq!(
            route_mobile_http_request("GET", "/mobile.webmanifest?t=secret"),
            MobileGatewayRoute::NotFound
        );
        assert_eq!(
            route_mobile_http_request("GET", "/mobile-icon.svg?t=secret"),
            MobileGatewayRoute::NotFound
        );
    }

    #[test]
    fn rendered_mobile_page_releases_held_buttons_when_pointer_or_page_cancels() {
        let page = render_mobile_page();

        assert!(page.contains("function attachHeldButton"));
        assert!(page.contains("button.addEventListener(\"pointerleave\""));
        assert!(page.contains("window.addEventListener(\"blur\""));
        assert!(page.contains("window.addEventListener(\"pagehide\""));
        assert!(page.contains("document.addEventListener(\"visibilitychange\""));
        assert!(page.contains("document.visibilityState === \"hidden\""));
        assert!(page.contains("release(true);"));
    }

    #[test]
    fn rendered_mobile_page_supports_touchpad_long_press_drag() {
        let page = render_mobile_page();

        assert!(page.contains("const LONG_PRESS_DRAG_DELAY_MS"));
        assert!(page.contains("function beginTouchpadDrag"));
        assert!(page.contains("function clearDragTimer"));
        assert!(page.contains("dragTimer = setTimeout"));
        assert!(page.contains("sendDragButton(\"Pressed\")"));
        assert!(page.contains("sendDragButton(\"Released\")"));
    }

    #[test]
    fn rendered_mobile_page_exposes_persistent_touchpad_sensitivity() {
        let page = render_mobile_page();

        assert!(page.contains("const POINTER_SENSITIVITY_STORAGE_KEY"));
        assert!(page.contains("function clampPointerSensitivity"));
        assert!(page.contains("let pointerSensitivity = clampPointerSensitivity"));
        assert!(page.contains("<input id=\"sensitivity\" type=\"range\""));
        assert!(page.contains("aria-label=\"触控板灵敏度\""));
        assert!(page.contains("localStorage.getItem(POINTER_SENSITIVITY_STORAGE_KEY)"));
        assert!(page.contains("localStorage.setItem(POINTER_SENSITIVITY_STORAGE_KEY"));
        assert!(page.contains("event.clientX - lastPoint.x) * pointerSensitivity"));
        assert!(page.contains("event.clientY - lastPoint.y) * pointerSensitivity"));
    }

    #[test]
    fn rendered_mobile_page_uses_virtual_desktop_bounds_for_pointer_movement() {
        let page = render_mobile_page();

        assert!(page.contains("display.virtual_x"));
        assert!(page.contains("display.virtual_y"));
        assert!(page.contains("display.layout_width"));
        assert!(page.contains("display.layout_height"));
        assert!(page.contains("minX"));
        assert!(page.contains("minY"));
    }

    #[test]
    fn rendered_mobile_page_updates_display_id_from_pointer_coordinates() {
        let page = render_mobile_page();

        assert!(page.contains("function displayIdForPointer"));
        assert!(page.contains("display.display_id || display.id"));
        assert!(page.contains("pointer.displayId = displayIdForPointer"));
    }

    #[test]
    fn rendered_mobile_page_releases_touchpad_drag_when_page_lifecycle_cancels_input() {
        let page = render_mobile_page();

        assert!(page.contains("function releaseTouchpadInteraction()"));
        assert!(page.contains("releaseTouchpadDrag();"));
        assert!(page.contains("window.addEventListener(\"blur\", releaseTouchpadInteraction);"));
        assert!(page.contains("window.addEventListener(\"pagehide\", releaseTouchpadInteraction);"));
        assert!(page.contains(
            "document.addEventListener(\"visibilitychange\", releaseTouchpadInteractionWhenHidden);"
        ));
        assert!(page.contains("document.visibilityState === \"hidden\""));
    }

    #[test]
    fn rendered_mobile_page_uses_keepalive_release_all_when_page_lifecycle_ends() {
        let page = render_mobile_page();

        assert!(page.contains("function releaseAllWithKeepalive()"));
        assert!(page.contains("navigator.sendBeacon"));
        assert!(page.contains("keepalive: true"));
        assert!(page.contains("mobile-release-all-keepalive"));
        assert!(page.contains("window.addEventListener(\"pagehide\", releaseAllWithKeepalive);"));
        assert!(page
            .contains("if (document.visibilityState === \"hidden\") releaseAllWithKeepalive();"));
    }

    #[test]
    fn rendered_mobile_page_does_not_claim_wake_lock_over_plain_http() {
        let page = render_mobile_page();

        assert!(!page.contains("wakeLock"));
        assert!(!page.contains("requestMobileWakeLock"));
        assert!(!page.contains("navigator.wakeLock"));
    }

    #[test]
    fn rendered_mobile_page_prevents_browser_navigation_and_default_gestures() {
        let page = render_mobile_page();

        assert!(page.contains("function shouldPreventBrowserNavigationEvent(event)"));
        assert!(page.contains("function preventBrowserNavigationEvent(event)"));
        assert!(page.contains("function preventMobileGestureDefault(event)"));
        assert!(
            page.contains("target.closest(\"input, textarea, select, [contenteditable='true']\")")
        );
        assert!(page.contains("[\"mousedown\", \"mouseup\", \"auxclick\", \"pointerdown\", \"pointerup\", \"keydown\"]"));
        assert!(page.contains("[\"contextmenu\", \"dragstart\", \"selectstart\", \"gesturestart\", \"gesturechange\", \"gestureend\"]"));
        assert!(page.contains("window.addEventListener(eventName, preventBrowserNavigationEvent, mobileEventOptions);"));
        assert!(page.contains("document.addEventListener(eventName, preventMobileGestureDefault, mobileEventOptions);"));
        assert!(page.contains("touch-action: manipulation"));
        assert!(page.contains("-webkit-touch-callout: none"));
    }

    #[test]
    fn disabled_mobile_access_snapshot_does_not_expose_a_token_link() {
        let access = MobileGatewayAccess::disabled("mobile port is already in use".to_string());

        let snapshot = access.snapshot();

        assert!(!snapshot.enabled);
        assert_eq!(snapshot.bind_address, "不可用");
        assert_eq!(snapshot.page_url, "不可用");
        assert_eq!(snapshot.token, "");
        assert_eq!(snapshot.last_client_addr, None);
        assert_eq!(snapshot.last_client_seen_at_ms, None);
        assert_eq!(snapshot.client_count, 0);
    }

    #[test]
    fn mobile_access_snapshot_percent_encodes_token_link() {
        let access = MobileGatewayAccess::new(
            SocketAddr::from(([127, 0, 0, 1], 27437)),
            "mobile+secret/token=".to_string(),
            "192.168.1.50".to_string(),
        );

        let snapshot = access.snapshot();

        assert_eq!(
            snapshot.page_url,
            "http://192.168.1.50:27437/mobile?t=mobile%2Bsecret%2Ftoken%3D"
        );
        assert_eq!(snapshot.token, "mobile+secret/token=");
    }

    #[tokio::test]
    async fn authorized_mobile_request_updates_last_client_snapshot() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listener_addr = listener.local_addr().unwrap();
        let client_task = tokio::spawn(async move {
            let mut client = TcpStream::connect(listener_addr).await.unwrap();
            client
                .write_all(b"GET /mobile?t=mobile-secret HTTP/1.1\r\nHost: mobile\r\n\r\n")
                .await
                .unwrap();
            let mut response = Vec::new();
            client.read_to_end(&mut response).await.unwrap();
            response
        });
        let (server_stream, peer_addr) = listener.accept().await.unwrap();
        let access = MobileGatewayAccess::new(
            SocketAddr::from(([127, 0, 0, 1], 27437)),
            "mobile-secret".to_string(),
            "127.0.0.1".to_string(),
        );
        let state = Arc::new(RwLock::new(DaemonState::new(ServiceStatusSnapshot::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local-host".to_string(),
            "0.0.0.0:27431".to_string(),
            27432,
            42,
        ))));
        {
            let mut state = state.write().await;
            state.mobile_access = access.clone();
        }
        let network_manager = Arc::new(Mutex::new(NetworkManager::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local-host".to_string(),
        )));
        let inject_backend: Arc<Mutex<Box<dyn InjectBackend>>> =
            Arc::new(Mutex::new(Box::new(NoopInjectBackend)));
        let (local_events_tx, _) = broadcast::channel(1);

        handle_mobile_gateway_client(
            server_stream,
            peer_addr,
            access,
            state.clone(),
            network_manager,
            inject_backend,
            local_events_tx,
        )
        .await
        .unwrap();
        let response = client_task.await.unwrap();
        assert!(String::from_utf8_lossy(&response).contains("200 OK"));

        let snapshot = state.read().await.mobile_access.snapshot();
        let expected_peer_addr = peer_addr.to_string();
        assert_eq!(
            snapshot.last_client_addr.as_deref(),
            Some(expected_peer_addr.as_str())
        );
        assert!(snapshot.last_client_seen_at_ms.unwrap_or_default() > 0);
        assert_eq!(snapshot.client_count, 1);
    }

    #[tokio::test]
    async fn invalid_mobile_inject_request_returns_json_bad_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listener_addr = listener.local_addr().unwrap();
        let body = b"{not-json";
        let client_task = tokio::spawn(async move {
            let mut client = TcpStream::connect(listener_addr).await.unwrap();
            let request = format!(
                "POST /api/inject?t=mobile-secret HTTP/1.1\r\n\
                 Host: mobile\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n",
                body.len()
            );
            client.write_all(request.as_bytes()).await.unwrap();
            client.write_all(body).await.unwrap();
            let mut response = Vec::new();
            client.read_to_end(&mut response).await.unwrap();
            response
        });
        let (server_stream, peer_addr) = listener.accept().await.unwrap();
        let access = MobileGatewayAccess::new(
            SocketAddr::from(([127, 0, 0, 1], 27437)),
            "mobile-secret".to_string(),
            "127.0.0.1".to_string(),
        );
        let state = Arc::new(RwLock::new(DaemonState::new(ServiceStatusSnapshot::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local-host".to_string(),
            "0.0.0.0:27431".to_string(),
            27432,
            42,
        ))));
        {
            let mut state = state.write().await;
            state.mobile_access = access.clone();
        }
        let network_manager = Arc::new(Mutex::new(NetworkManager::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local-host".to_string(),
        )));
        let inject_backend: Arc<Mutex<Box<dyn InjectBackend>>> =
            Arc::new(Mutex::new(Box::new(NoopInjectBackend)));
        let (local_events_tx, _) = broadcast::channel(1);

        handle_mobile_gateway_client(
            server_stream,
            peer_addr,
            access,
            state,
            network_manager,
            inject_backend,
            local_events_tx,
        )
        .await
        .unwrap();
        let response = String::from_utf8_lossy(&client_task.await.unwrap()).to_string();

        assert!(response.contains("400 Bad Request"));
        assert!(response.contains("application/json"));
        assert!(response.contains("\"error\":\"invalid mobile inject request\""));
    }

    #[tokio::test]
    async fn bind_failure_marks_mobile_access_disabled_without_failing_daemon() {
        let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = occupied.local_addr().unwrap();
        let state = Arc::new(RwLock::new(DaemonState::new(ServiceStatusSnapshot::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local-host".to_string(),
            "0.0.0.0:27431".to_string(),
            27432,
            42,
        ))));
        let access =
            MobileGatewayAccess::new(addr, "mobile-secret".to_string(), "127.0.0.1".to_string());
        {
            let mut state = state.write().await;
            state.mobile_access = access.clone();
        }
        let network_manager = Arc::new(Mutex::new(NetworkManager::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local-host".to_string(),
        )));
        let inject_backend: Arc<Mutex<Box<dyn InjectBackend>>> =
            Arc::new(Mutex::new(Box::new(NoopInjectBackend)));
        let (local_events_tx, _) = broadcast::channel(1);
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        let task = tokio::spawn(run_mobile_gateway_server(
            access,
            state.clone(),
            network_manager,
            inject_backend,
            local_events_tx,
            shutdown_rx,
        ));

        timeout(Duration::from_secs(1), async {
            loop {
                if !state.read().await.mobile_access.snapshot().enabled {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        let snapshot = state.read().await.mobile_access.snapshot();
        assert!(!snapshot.enabled);
        assert_eq!(snapshot.page_url, "不可用");

        let _ = shutdown_tx.send(());
        let result = timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_ok());
    }
}
