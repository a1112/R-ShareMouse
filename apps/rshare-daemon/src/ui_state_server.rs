use std::{
    future::Future,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use rshare_core::{
    DaemonResponse, LocalControlDeviceSnapshot, LocalInputDiagnosticEvent, UiCursor, UiEnvelope,
};
use serde::Deserialize;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{broadcast, Semaphore},
    time::{interval_at, timeout, Instant, MissedTickBehavior},
};
use tokio_tungstenite::{
    accept_hdr_async_with_config,
    tungstenite::{
        handshake::server::{ErrorResponse, Request, Response},
        http::{Response as HttpResponse, StatusCode, Uri},
        protocol::WebSocketConfig,
        Message as WsMessage,
    },
    WebSocketStream,
};

use crate::state_aggregator::StateAggregatorHandle;

const UI_STATE_PATH: &str = "/ui-state";
const LOCAL_CONTROLS_PATH: &str = "/local-controls";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
pub const DEFAULT_MAX_UI_WEBSOCKET_CONNECTIONS: usize = 64;
pub const DEFAULT_MAX_UI_WEBSOCKET_MESSAGE_BYTES: usize = 64 * 1024;
pub const DEFAULT_UI_WEBSOCKET_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_UI_WEBSOCKET_SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
pub struct UiStateServerConfig {
    pub max_connections: usize,
    pub handshake_timeout: Duration,
    pub subscribe_timeout: Duration,
    pub max_message_bytes: usize,
}

impl Default for UiStateServerConfig {
    fn default() -> Self {
        Self {
            max_connections: DEFAULT_MAX_UI_WEBSOCKET_CONNECTIONS,
            handshake_timeout: DEFAULT_UI_WEBSOCKET_HANDSHAKE_TIMEOUT,
            subscribe_timeout: DEFAULT_UI_WEBSOCKET_SUBSCRIBE_TIMEOUT,
            max_message_bytes: DEFAULT_MAX_UI_WEBSOCKET_MESSAGE_BYTES,
        }
    }
}

pub type LocalControlsSnapshotFuture =
    Pin<Box<dyn Future<Output = Result<LocalControlDeviceSnapshot>> + Send + 'static>>;
pub type LocalControlsSnapshotProvider =
    Arc<dyn Fn() -> LocalControlsSnapshotFuture + Send + Sync + 'static>;

#[derive(Clone)]
pub struct LocalControlsFeed {
    snapshot: LocalControlsSnapshotProvider,
    events: broadcast::Sender<LocalInputDiagnosticEvent>,
}

impl LocalControlsFeed {
    pub fn new(
        snapshot: LocalControlsSnapshotProvider,
        events: broadcast::Sender<LocalInputDiagnosticEvent>,
    ) -> Self {
        Self { snapshot, events }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebSocketRoute {
    UiState,
    LocalControls,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum UiStateClientMessage {
    Subscribe {
        #[serde(default)]
        cursor: Option<UiCursor>,
    },
    Resync,
}

pub async fn run_ui_state_server(
    address: SocketAddr,
    ui_state: StateAggregatorHandle,
    local_controls: LocalControlsFeed,
    shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    if !address.ip().is_loopback() {
        bail!("UI state websocket must bind to a loopback address");
    }
    let listener = TcpListener::bind(address).await?;
    run_ui_state_server_on_listener_with_config(
        listener,
        ui_state,
        local_controls,
        shutdown_rx,
        UiStateServerConfig::default(),
    )
    .await
}

pub async fn run_ui_state_server_on_listener(
    listener: TcpListener,
    ui_state: StateAggregatorHandle,
    local_controls: LocalControlsFeed,
    shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    run_ui_state_server_on_listener_with_config(
        listener,
        ui_state,
        local_controls,
        shutdown_rx,
        UiStateServerConfig::default(),
    )
    .await
}

pub async fn run_ui_state_server_on_listener_with_config(
    listener: TcpListener,
    ui_state: StateAggregatorHandle,
    local_controls: LocalControlsFeed,
    mut shutdown_rx: broadcast::Receiver<()>,
    config: UiStateServerConfig,
) -> Result<()> {
    let address = listener.local_addr()?;
    if !address.ip().is_loopback() {
        bail!("UI state websocket listener must be loopback-only");
    }
    if config.max_connections == 0 {
        bail!("UI state websocket connection limit must be non-zero");
    }
    if config.max_message_bytes == 0 {
        bail!("UI state websocket message limit must be non-zero");
    }
    if config.handshake_timeout.is_zero() || config.subscribe_timeout.is_zero() {
        bail!("UI state websocket timeouts must be non-zero");
    }
    tracing::info!("UI state websocket listening on {address}");
    let permits = Arc::new(Semaphore::new(config.max_connections));

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, peer) = accepted?;
                if !peer.ip().is_loopback() {
                    tracing::warn!("Rejected non-loopback UI websocket peer {peer}");
                    continue;
                }
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    tracing::warn!("Rejected UI websocket peer {peer}: connection limit reached");
                    continue;
                };
                let ui_state = ui_state.clone();
                let local_controls = local_controls.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(error) = handle_client(stream, ui_state, local_controls, config).await {
                        tracing::debug!("UI websocket client error: {error:#}");
                    }
                });
            }
            result = shutdown_rx.recv() => {
                match result {
                    Ok(()) | Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => break,
                }
            }
        }
    }
    Ok(())
}

async fn handle_client(
    stream: TcpStream,
    ui_state: StateAggregatorHandle,
    local_controls: LocalControlsFeed,
    config: UiStateServerConfig,
) -> Result<()> {
    let mut selected_route = None;
    let mut websocket_config = WebSocketConfig::default();
    websocket_config.max_message_size = Some(config.max_message_bytes);
    websocket_config.max_frame_size = Some(config.max_message_bytes);
    let websocket = timeout(
        config.handshake_timeout,
        accept_hdr_async_with_config(
            stream,
            |request: &Request, response: Response| {
                let route = validate_upgrade(request)?;
                selected_route = Some(route);
                Ok(response)
            },
            Some(websocket_config),
        ),
    )
    .await
    .context("websocket handshake timed out")?
    .context("websocket handshake rejected")?;
    let route = selected_route.context("websocket route was not selected")?;

    match route {
        WebSocketRoute::UiState => {
            stream_ui_state(websocket, ui_state, config.subscribe_timeout).await
        }
        WebSocketRoute::LocalControls => stream_local_controls(websocket, local_controls).await,
    }
}

fn validate_upgrade(request: &Request) -> std::result::Result<WebSocketRoute, ErrorResponse> {
    let route = match request.uri().path() {
        UI_STATE_PATH => WebSocketRoute::UiState,
        LOCAL_CONTROLS_PATH => WebSocketRoute::LocalControls,
        _ => return Err(reject(StatusCode::NOT_FOUND, "unknown websocket path")),
    };

    let origin = request
        .headers()
        .get("origin")
        .and_then(|value| value.to_str().ok());
    let origin_allowed = origin.map(is_loopback_origin).unwrap_or(false);
    if !origin_allowed && !(route == WebSocketRoute::LocalControls && origin.is_none()) {
        return Err(reject(
            StatusCode::FORBIDDEN,
            "websocket origin must be loopback",
        ));
    }
    Ok(route)
}

fn reject(status: StatusCode, message: &str) -> ErrorResponse {
    HttpResponse::builder()
        .status(status)
        .body(Some(message.to_string()))
        .expect("static websocket rejection response")
}

fn is_loopback_origin(origin: &str) -> bool {
    let Ok(uri) = origin.parse::<Uri>() else {
        return false;
    };
    if !matches!(uri.scheme_str(), Some("http" | "https" | "tauri")) {
        return false;
    }
    let Some(host) = uri.host() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case("tauri.localhost")
        || host.to_ascii_lowercase().ends_with(".localhost")
    {
        return true;
    }
    host.parse::<IpAddr>()
        .map(|address| address.is_loopback())
        .unwrap_or(false)
}

async fn stream_ui_state(
    mut websocket: WebSocketStream<TcpStream>,
    ui_state: StateAggregatorHandle,
    subscribe_timeout: Duration,
) -> Result<()> {
    let first = timeout(
        subscribe_timeout,
        next_ui_state_client_message(&mut websocket),
    )
    .await
    .context("UI websocket subscribe timed out")??;
    let mut subscriber = match first {
        UiStateClientMessage::Subscribe { cursor } => ui_state.subscribe(cursor).await?,
        UiStateClientMessage::Resync => ui_state.subscribe(None).await?,
    };
    let mut heartbeat = interval_at(Instant::now() + HEARTBEAT_INTERVAL, HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            envelope = subscriber.recv() => {
                send_ui_envelope(&mut websocket, &envelope?).await?;
            }
            tick = heartbeat.tick() => {
                let sent_at_ms = timestamp_ms(tick);
                send_ui_envelope(&mut websocket, &ui_state.heartbeat(sent_at_ms)).await?;
            }
            message = websocket.next() => {
                match decode_ui_state_client_message(message).await? {
                    Some(UiStateClientMessage::Subscribe { cursor }) => {
                        subscriber = ui_state.subscribe(cursor).await?;
                    }
                    Some(UiStateClientMessage::Resync) => {
                        subscriber = ui_state.subscribe(None).await?;
                    }
                    None => break,
                }
            }
        }
    }
    Ok(())
}

async fn next_ui_state_client_message(
    websocket: &mut WebSocketStream<TcpStream>,
) -> Result<UiStateClientMessage> {
    loop {
        match decode_ui_state_client_message(websocket.next().await).await? {
            Some(message) => return Ok(message),
            None => bail!("websocket closed before UI subscription"),
        }
    }
}

async fn decode_ui_state_client_message(
    message: Option<std::result::Result<WsMessage, tokio_tungstenite::tungstenite::Error>>,
) -> Result<Option<UiStateClientMessage>> {
    let Some(message) = message else {
        return Ok(None);
    };
    match message? {
        WsMessage::Text(text) => Ok(Some(
            serde_json::from_str(&text).context("invalid UI websocket command")?,
        )),
        WsMessage::Close(_) => Ok(None),
        WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Binary(_) | WsMessage::Frame(_) => {
            bail!("UI websocket accepts text subscription commands only")
        }
    }
}

async fn send_ui_envelope(
    websocket: &mut WebSocketStream<TcpStream>,
    envelope: &UiEnvelope,
) -> Result<()> {
    websocket
        .send(WsMessage::Text(serde_json::to_string(envelope)?))
        .await?;
    Ok(())
}

async fn stream_local_controls(
    mut websocket: WebSocketStream<TcpStream>,
    feed: LocalControlsFeed,
) -> Result<()> {
    let response = DaemonResponse::LocalControls((feed.snapshot)().await?);
    websocket
        .send(WsMessage::Text(serde_json::to_string(&response)?))
        .await?;

    let mut events = feed.events.subscribe();
    loop {
        tokio::select! {
            event = events.recv() => {
                match event {
                    Ok(event) => {
                        websocket.send(WsMessage::Text(serde_json::to_string(
                            &DaemonResponse::LocalControlEvent(event),
                        )?)).await?;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            message = websocket.next() => {
                match message {
                    Some(Ok(WsMessage::Close(_))) | None => break,
                    Some(Ok(WsMessage::Ping(payload))) => {
                        websocket.send(WsMessage::Pong(payload)).await?;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(error.into()),
                }
            }
        }
    }
    Ok(())
}

fn timestamp_ms(_: Instant) -> u64 {
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
    use tokio_tungstenite::tungstenite::http::Request as HttpRequest;

    fn request(path: &str, origin: Option<&str>) -> Request {
        let mut request = HttpRequest::builder()
            .uri(format!("ws://127.0.0.1:27436{path}"))
            .body(())
            .unwrap();
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert("origin", origin.parse().unwrap());
        }
        request
    }

    #[test]
    fn validates_route_and_origin_before_upgrade() {
        assert_eq!(
            validate_upgrade(&request(UI_STATE_PATH, Some("http://localhost:5176"))).unwrap(),
            WebSocketRoute::UiState
        );
        assert_eq!(
            validate_upgrade(&request(LOCAL_CONTROLS_PATH, None)).unwrap(),
            WebSocketRoute::LocalControls
        );
        assert_eq!(
            validate_upgrade(&request("/unknown", Some("http://localhost:5176")))
                .unwrap_err()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            validate_upgrade(&request(UI_STATE_PATH, Some("https://example.com")))
                .unwrap_err()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
}
